//! The bidder client's pool of HTTP/2 connections (`http-client.http2-opt-in`, ruling 10 (a)):
//! the connections of each h2 or h2c origin the bidder client reaches without a proxy.
//!
//! reqwest keeps one HTTP/2 connection per origin and says nothing of the peer's
//! `SETTINGS_MAX_CONCURRENT_STREAMS`, so the pool speaks HTTP/2 itself, with h2, on connections it
//! opens as reqwest's connector would ([`dial`]), and writes each request as reqwest and hyper do
//! ([`stream`]):
//!
//! - a request takes the connection with the most free streams, the peer's
//!   `SETTINGS_MAX_CONCURRENT_STREAMS` less the streams open on it; while a connection is being
//!   opened, and until its peer's SETTINGS are known (the answer to a PING sent after the preface,
//!   waited for at most a second), only the request that opens it has a stream on it, and the
//!   requests that come meanwhile wait for it and then choose again;
//! - when every connection is full another is opened, each holding one of the origin's
//!   `max_connections_per_host` slots ([`HostLimiter`]) for as long as it is open, as Go's
//!   `MaxConnsPerHost` counts connections; once they are all taken, a request waits for a free
//!   stream or a slot, as a request waits for the per-host semaphore over HTTP/1.1;
//! - a connection that has had GOAWAY or has failed leaves the pool, and its streams in flight
//!   finish on it;
//! - a connection that has carried no stream for `idle_connection_timeout_seconds` is closed;
//! - a stream the peer refuses unread (`REFUSED_STREAM`), or leaves out of a graceful GOAWAY, goes
//!   again, at most twice more, as reqwest's default retry sends it again; one whose connection
//!   turns out gone before the stream opened goes to another connection.
//!
//! Over TLS the dial offers ALPN `h2` and `http/1.1`, as reqwest does; an origin that picks
//! `http/1.1` is left to reqwest for [`H2C_VERDICT_TTL`](super::H2C_VERDICT_TTL), its connection
//! closed unused. So is an h2c origin moved back to HTTP/1.1 ([`Pool::demote`]). Either way the
//! origin's pooled connections leave the pool then, so that, once their streams in flight have
//! ended, they close and give their `max_connections_per_host` slots back to the requests that now
//! go over HTTP/1.1.

mod dial;
mod stream;

use std::{
    collections::BTreeMap,
    fmt, mem,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
    time::Duration,
};

use bytes::Bytes;
use futures_util::{FutureExt as _, future::BoxFuture};
use h2::{
    Ping, PingPong, Reason,
    client::{ResponseFuture, SendRequest},
};
use moka::sync::Cache;
use reqwest::{StatusCode, header::HeaderMap};
use tokio::{
    sync::{Notify, OwnedSemaphorePermit, Semaphore, futures::OwnedNotified, watch},
    time::{Instant, sleep_until, timeout},
};
use url::Url;

use self::dial::{ASSUMED_STREAMS, Dialed, Opened};
pub(super) use self::{
    dial::{DialError, Dialer},
    stream::{Body, Hop},
};
use super::{
    limit::{HostLimiter, Key},
    limit_key,
};
use crate::metrics::Trace;

/// The times a stream refused unread goes again: reqwest's default retry policy
/// (`max_retries_per_request`).
const REFUSALS: usize = 2;

/// The times a request moves on from a connection found gone before its stream opened.
const LOST: usize = 2;

/// How long a connection's requests wait for its peer's SETTINGS before they take it as it stands.
const SETTLE: Duration = Duration::from_secs(1);

/// The most TLS origins whose ALPN answer is kept.
const ORIGINS: u64 = 10_000;

/// How long the ALPN verdicts stay in the cache on its own clock, only to bound its memory: a
/// verdict's `until`, on tokio's clock, says whether it holds.
const KEPT: Duration = Duration::from_mins(11);

/// The pool of one bidder client.
#[derive(Debug)]
pub(super) struct Pool {
    shared: Arc<Shared>,
}

/// What the pool's requests, slots and connection tasks share.
#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    dialer: Dialer,
    idle_timeout: Option<Duration>,
    /// The origins whose requests go over reqwest, until when that holds: the TLS origins that
    /// answered ALPN with `http/1.1`, and the h2c origins moved back to HTTP/1.1.
    http1: Cache<Key, Instant>,
    /// How long such a verdict holds.
    verdict_ttl: Duration,
}

/// The origins' connections.
#[derive(Debug, Default)]
struct State {
    hosts: BTreeMap<Key, Host>,
    /// The next connection's id.
    next: u64,
}

/// One origin's connections.
#[derive(Debug, Default)]
struct Host {
    conns: Vec<Conn>,
    /// Told when a stream ends or a connection leaves: what a request waiting for a free stream
    /// waits for.
    changed: Arc<Notify>,
}

/// One connection.
#[derive(Debug)]
struct Conn {
    id: u64,
    link: Link,
    /// The streams of the pool's requests open on it.
    open: usize,
    /// Whether a request has taken it: the next one reuses it.
    used: bool,
    /// When its last stream ended, or when it opened.
    idle_since: Instant,
}

/// How a connection is reached.
#[derive(Debug)]
enum Link {
    /// Being opened, or open with its peer's SETTINGS not yet known: what will be said of it.
    /// Only the request that opens it has a stream on it meanwhile.
    Opening(watch::Receiver<Opening>),
    /// Open, its peer's SETTINGS applied.
    Ready(Box<SendRequest<Bytes>>),
}

/// What is said of a connection being opened.
#[derive(Debug, Clone)]
enum Opening {
    Pending,
    /// It is open and its peer's limit known, or it has ended: the requests that waited for it
    /// look again.
    Settled,
    /// Over TLS, the origin picked `http/1.1`.
    Http1,
    Failed(Arc<DialError>),
}

/// A pooled response's head and body.
#[derive(Debug)]
pub(super) struct Answer {
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) body: Body,
    /// Whether the stream went over a connection an earlier request had taken.
    pub(super) reused: bool,
    /// The time spent waiting for a free stream or a `max_connections_per_host` slot.
    pub(super) wait: Duration,
}

/// Why a request did not get its answer over the pool.
#[derive(Debug)]
pub(super) enum Failure {
    /// Over TLS the origin picked `http/1.1`, or the h2c origin has been moved back to HTTP/1.1:
    /// the request goes over reqwest, after `wait`.
    Http1 { wait: Duration },
    /// The URL cannot be written as HTTP/2's target.
    Target(http::Error),
    /// The connection could not be opened.
    Dial(Arc<DialError>),
    /// The origin read none of the request: HTTP/2's handshake failed, the connection was gone
    /// before the stream opened, or (in cleartext) the origin answered with bytes that are not
    /// HTTP/2, which h2 reads as a frame too large.
    Unread(h2::Error),
    /// The stream failed after it opened, or the connection under it did: the origin may have
    /// read the request.
    Failed(h2::Error),
}

/// A request's place on a connection: given back, and the connection's waiters told, when it is
/// dropped.
pub(super) struct Slot {
    shared: Arc<Shared>,
    key: Key,
    id: u64,
}

impl fmt::Debug for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Slot")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let now = Instant::now();
        let mut state = self.shared.lock();
        if let Some(host) = state.hosts.get_mut(&self.key) {
            if let Some(conn) = host.conns.iter_mut().find(|conn| conn.id == self.id) {
                conn.open = conn.open.saturating_sub(1);
                if conn.open == 0 {
                    conn.idle_since = now;
                }
            }
            host.changed.notify_waiters();
        }
    }
}

/// A stream to open: the connection's handle, the request's slot on it, whether an earlier
/// request had taken the connection, and whether this request opened it.
struct Checkout {
    send: SendRequest<Bytes>,
    slot: Slot,
    reused: bool,
    opened: bool,
}

/// What a request does once it has looked at its origin's connections.
enum Step {
    /// Opens a stream.
    Take(Box<Checkout>),
    /// Waits for the connection being opened.
    Ride(watch::Receiver<Opening>, Slot),
    /// Opens a connection.
    Open(watch::Sender<Opening>, Slot, Option<OwnedSemaphorePermit>),
    /// Waits for a free stream (the notification, registered while the request looked) or a
    /// slot.
    Wait(Pin<Box<OwnedNotified>>, Arc<Semaphore>),
}

impl Pool {
    /// The pool of a client that dials with `dialer` and closes connections idle for
    /// `idle_timeout` (none: as long as the peer keeps them); an ALPN verdict holds for
    /// `verdict_ttl`.
    pub(super) fn new(
        dialer: Dialer,
        idle_timeout: Option<Duration>,
        verdict_ttl: Duration,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                dialer,
                idle_timeout,
                http1: Cache::builder()
                    .max_capacity(ORIGINS)
                    .time_to_live(KEPT)
                    .build(),
                verdict_ttl,
            }),
        }
    }

    /// Moves `key`'s origin off the pool for the verdicts' time: its requests go over reqwest, and
    /// its connections leave the pool, their streams in flight finishing, so that each closes
    /// and gives its `max_connections_per_host` slot back once its streams have ended. The h2c
    /// demotions call it (`http-client.h2c-demotion`, `http-client.h2c-redirects`).
    pub(super) fn demote(&self, key: &Key) {
        self.shared.demote(key);
    }

    /// Sends `hop` over one of its origin's connections and waits for the answer's head, the
    /// connection's slots counted by `limiter`, a new connection's lookup charged to `trace`.
    ///
    /// # Errors
    ///
    /// As [`Failure`] says.
    pub(super) async fn send(
        &self,
        hop: &Hop,
        limiter: Option<&HostLimiter>,
        trace: Option<&Trace>,
    ) -> Result<Answer, Failure> {
        let key = limit_key(&hop.url, None);
        let mut wait = Duration::ZERO;
        let mut refusals = 0;
        let mut lost = 0;
        loop {
            let (head, end) = hop.head().map_err(Failure::Target)?;
            let checkout = self
                .checkout(&key, &hop.url, limiter, trace, &mut wait)
                .await?;
            let Checkout {
                send,
                slot,
                reused,
                opened,
            } = checkout;
            let response = match open(send, head, end, hop.body.as_ref()).await {
                Ok(response) => response.await,
                Err(Unopened::Gone(error)) => {
                    let id = slot.id;
                    drop(slot);
                    self.shared.retire(&key, id);
                    if opened || lost >= LOST {
                        return Err(Failure::Unread(error));
                    }
                    lost += 1;
                    continue;
                }
                Err(Unopened::Refused(error)) => return Err(Failure::Failed(error)),
            };
            match response {
                Ok(response) => {
                    let (parts, stream) = response.into_parts();
                    let mut headers = parts.headers;
                    let body = Body::new(stream, &mut headers, slot);
                    return Ok(Answer {
                        status: parts.status,
                        headers,
                        body,
                        reused,
                        wait,
                    });
                }
                Err(error) => {
                    drop(slot);
                    if refused(&error) && refusals < REFUSALS {
                        refusals += 1;
                        continue;
                    }
                    return Err(if not_http2(&error) {
                        Failure::Unread(error)
                    } else {
                        Failure::Failed(error)
                    });
                }
            }
        }
    }

    /// A stream on one of `key`'s connections, opening one to `url`'s origin when every
    /// connection is full and a slot is free, adding the time it waits for a stream or a slot to
    /// `wait`.
    async fn checkout(
        &self,
        key: &Key,
        url: &Url,
        limiter: Option<&HostLimiter>,
        trace: Option<&Trace>,
        wait: &mut Duration,
    ) -> Result<Checkout, Failure> {
        let mut permit = None;
        loop {
            if self
                .shared
                .http1
                .get(key)
                .is_some_and(|until| until > Instant::now())
            {
                return Err(Failure::Http1 { wait: *wait });
            }
            let step = self.shared.step(key, limiter, &mut permit);
            match step {
                Step::Take(checkout) => return Ok(*checkout),
                Step::Ride(mut opening, place) => {
                    let outcome = opening
                        .wait_for(|opening| !matches!(opening, Opening::Pending))
                        .await
                        .map(|outcome| outcome.clone());
                    // The place only held the request's turn on the connection being opened.
                    drop(place);
                    match outcome {
                        Ok(Opening::Http1) => return Err(Failure::Http1 { wait: *wait }),
                        Ok(Opening::Failed(error)) => return Err(Failure::Dial(error)),
                        // Settled, or the opening request went away before it said: look again,
                        // with the peer's limit known.
                        Ok(Opening::Settled | Opening::Pending) | Err(_) => {}
                    }
                }
                Step::Open(opening, slot, held) => {
                    return self.open(key, url, trace, opening, slot, held).await;
                }
                Step::Wait(notified, semaphore) => {
                    let start = Instant::now();
                    tokio::select! {
                        () = notified => {}
                        acquired = semaphore.acquire_owned() => permit = acquired.ok(),
                    }
                    *wait = wait.saturating_add(start.elapsed());
                }
            }
        }
    }

    /// Opens the connection `opening` stands for, holding `held`, with the request's `slot` on
    /// it, and tells the requests that wait for it.
    async fn open(
        &self,
        key: &Key,
        url: &Url,
        trace: Option<&Trace>,
        opening: watch::Sender<Opening>,
        slot: Slot,
        held: Option<OwnedSemaphorePermit>,
    ) -> Result<Checkout, Failure> {
        // Should the request go away while it dials, the connection leaves with it.
        let mut guard = Dialing {
            shared: &self.shared,
            key,
            id: slot.id,
            armed: true,
        };
        let dialed = self.shared.dialer.dial(url, trace).await;
        match dialed {
            Ok(Dialed::H2(opened)) => {
                let Opened {
                    send,
                    connection,
                    ping,
                } = *opened;
                guard.armed = false;
                drop(tokio::spawn(drive(
                    Driven {
                        connection,
                        ping,
                        send: send.clone(),
                        opening,
                    },
                    Arc::downgrade(&self.shared),
                    key.clone(),
                    slot.id,
                    self.shared.idle_timeout,
                    held,
                )));
                Ok(Checkout {
                    send,
                    slot,
                    reused: false,
                    opened: true,
                })
            }
            Ok(Dialed::Http1) => {
                self.shared.demote(key);
                drop(opening.send(Opening::Http1));
                drop(guard);
                drop(slot);
                Err(Failure::Http1 {
                    wait: Duration::ZERO,
                })
            }
            Err(error) => {
                let error = Arc::new(error);
                drop(opening.send(Opening::Failed(Arc::clone(&error))));
                drop(guard);
                drop(slot);
                Err(Failure::Dial(error))
            }
        }
    }
}

/// Takes a connection being opened out of the pool while armed: the request opening it went away
/// or failed.
struct Dialing<'a> {
    shared: &'a Shared,
    key: &'a Key,
    id: u64,
    armed: bool,
}

impl Drop for Dialing<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.shared.retire(self.key, self.id);
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What a request for `key` does next, `permit` the slot it holds from an earlier wait.
    fn step(
        self: &Arc<Self>,
        key: &Key,
        limiter: Option<&HostLimiter>,
        permit: &mut Option<OwnedSemaphorePermit>,
    ) -> Step {
        let mut state = self.lock();
        let State { hosts, next } = &mut *state;
        let host = hosts.entry(key.clone()).or_default();
        let slot = |id| Slot {
            shared: Arc::clone(self),
            key: key.clone(),
            id,
        };
        while let Some(index) = least_loaded(&host.conns) {
            let Some(conn) = host.conns.get_mut(index) else {
                break;
            };
            match &conn.link {
                Link::Ready(send) => {
                    // A connection that has had GOAWAY, or whose task has ended, fails at once.
                    if matches!(
                        SendRequest::clone(send).ready().now_or_never(),
                        Some(Err(_))
                    ) {
                        let id = conn.id;
                        host.conns.retain(|other| other.id != id);
                        host.changed.notify_waiters();
                        continue;
                    }
                    let send = SendRequest::clone(send);
                    conn.open += 1;
                    let reused = mem::replace(&mut conn.used, true);
                    return Step::Take(Box::new(Checkout {
                        send,
                        slot: slot(conn.id),
                        reused,
                        opened: false,
                    }));
                }
                Link::Opening(opening) => {
                    let opening = opening.clone();
                    conn.open += 1;
                    return Step::Ride(opening, slot(conn.id));
                }
            }
        }
        let held = match limiter {
            None => None,
            Some(limiter) => {
                let earlier = permit.take();
                if earlier.is_some() {
                    earlier
                } else {
                    let semaphore = limiter.semaphore(key.clone());
                    if let Ok(acquired) = Arc::clone(&semaphore).try_acquire_owned() {
                        Some(acquired)
                    } else {
                        // Registered now, under the lock, so that no stream ending after this
                        // look goes untold.
                        let mut notified = Box::pin(Arc::clone(&host.changed).notified_owned());
                        let _registered = notified.as_mut().enable();
                        return Step::Wait(notified, semaphore);
                    }
                }
            }
        };
        let id = *next;
        *next = next.wrapping_add(1);
        let (opening, watching) = watch::channel(Opening::Pending);
        host.conns.push(Conn {
            id,
            link: Link::Opening(watching),
            open: 1,
            used: true,
            idle_since: Instant::now(),
        });
        Step::Open(opening, slot(id), held)
    }

    /// Marks connection `id` of `key` open, reached through `send`.
    fn ready(&self, key: &Key, id: u64, send: &SendRequest<Bytes>) {
        let mut state = self.lock();
        if let Some(conn) = state
            .hosts
            .get_mut(key)
            .and_then(|host| host.conns.iter_mut().find(|conn| conn.id == id))
        {
            conn.link = Link::Ready(Box::new(send.clone()));
        }
    }

    /// Sends `key`'s requests over reqwest until the verdicts' time has passed, and takes every
    /// connection of `key` out of the pool: their streams in flight finish on them.
    fn demote(&self, key: &Key) {
        let until = Instant::now()
            .checked_add(self.verdict_ttl)
            .unwrap_or_else(Instant::now);
        self.http1.insert(key.clone(), until);
        let mut state = self.lock();
        if let Some(host) = state.hosts.remove(key) {
            host.changed.notify_waiters();
        }
    }

    /// Takes connection `id` of `key` out of the pool: its streams in flight finish on it.
    fn retire(&self, key: &Key, id: u64) {
        let mut state = self.lock();
        if let Some(host) = state.hosts.get_mut(key) {
            host.conns.retain(|conn| conn.id != id);
            host.changed.notify_waiters();
            if host.conns.is_empty() {
                drop(state.hosts.remove(key));
            }
        }
    }

    /// Whether connection `id` of `key` has been idle for `timeout`, and so leaves the pool;
    /// otherwise when to look again.
    fn idle(&self, key: &Key, id: u64, timeout: Duration) -> Option<Instant> {
        let now = Instant::now();
        let state = self.lock();
        let conn = state
            .hosts
            .get(key)
            .and_then(|host| host.conns.iter().find(|conn| conn.id == id))?;
        let since = if conn.open == 0 { conn.idle_since } else { now };
        let until = since.checked_add(timeout).unwrap_or(now);
        if conn.open == 0 && until <= now {
            drop(state);
            self.retire(key, id);
            return None;
        }
        Some(until)
    }
}

/// The connection with the most free streams, the first of those with as many; none when every
/// connection is full.
fn least_loaded(conns: &[Conn]) -> Option<usize> {
    conns
        .iter()
        .enumerate()
        .filter(|(_, conn)| free(conn) > 0)
        .max_by(|(i, a), (j, b)| free(a).cmp(&free(b)).then(j.cmp(i)))
        .map(|(index, _)| index)
}

/// The streams connection `conn` can take besides its open ones.
fn free(conn: &Conn) -> usize {
    let limit = match &conn.link {
        Link::Ready(send) => send.current_max_send_streams(),
        Link::Opening(_) => ASSUMED_STREAMS,
    };
    limit.saturating_sub(conn.open)
}

/// Why a stream did not open.
enum Unopened {
    /// The connection was gone (GOAWAY or a failure): nothing was sent.
    Gone(h2::Error),
    /// h2 refused the request itself.
    Refused(h2::Error),
}

/// Opens the request's stream over `send`: its head, ending the stream when `end`, then its
/// `body`.
async fn open(
    send: SendRequest<Bytes>,
    head: http::Request<()>,
    end: bool,
    body: Option<&Bytes>,
) -> Result<ResponseFuture, Unopened> {
    let gone = |error: h2::Error| {
        if error.is_go_away() || error.is_io() {
            Unopened::Gone(error)
        } else {
            Unopened::Refused(error)
        }
    };
    let mut send = send.ready().await.map_err(gone)?;
    let (response, mut stream) = send.send_request(head, end).map_err(gone)?;
    if !end && let Some(body) = body {
        // h2 buffers the body and sends it as the peer's windows allow.
        stream
            .send_data(body.clone(), true)
            .map_err(Unopened::Refused)?;
    }
    Ok(response)
}

/// Whether `error` is one reqwest's default retry sends the request again for: a stream the peer
/// refused unread, or one a graceful GOAWAY left out.
fn refused(error: &h2::Error) -> bool {
    error.is_remote()
        && ((error.is_reset() && error.reason() == Some(Reason::REFUSED_STREAM))
            || (error.is_go_away() && error.reason() == Some(Reason::NO_ERROR)))
}

/// Whether `error` is h2 tearing the connection down on a frame too large, as an HTTP/1.x status
/// line reads: the origin did not answer HTTP/2.
fn not_http2(error: &h2::Error) -> bool {
    error.is_go_away() && error.is_library() && error.reason() == Some(Reason::FRAME_SIZE_ERROR)
}

/// A connection the pool has opened, as its task takes it: the future that drives it, its pings,
/// its handle, and what will be said of it to the requests waiting for it.
struct Driven {
    connection: BoxFuture<'static, ()>,
    ping: Option<PingPong>,
    send: SendRequest<Bytes>,
    opening: watch::Sender<Opening>,
}

/// Drives connection `id` of `key` until it ends: settles it once its peer's SETTINGS are known,
/// closes it once it has been idle for `idle_timeout`, and holds its `max_connections_per_host`
/// slot meanwhile.
async fn drive(
    driven: Driven,
    shared: Weak<Shared>,
    key: Key,
    id: u64,
    idle_timeout: Option<Duration>,
    permit: Option<OwnedSemaphorePermit>,
) {
    let Driven {
        mut connection,
        ping,
        send,
        opening,
    } = driven;
    tokio::select! {
        () = &mut connection => {
            end(&shared, &key, id);
            drop(opening.send(Opening::Settled));
            drop(permit);
            return;
        }
        () = settle(ping) => {
            if let Some(shared) = shared.upgrade() {
                shared.ready(&key, id, &send);
            }
            drop(opening.send(Opening::Settled));
        }
    }
    drop(send);
    if let Some(timeout) = idle_timeout {
        let mut check = Instant::now().checked_add(timeout);
        while let Some(at) = check {
            tokio::select! {
                () = &mut connection => {
                    end(&shared, &key, id);
                    drop(permit);
                    return;
                }
                // Out of the pool, or the pool gone, the connection closes once its streams end:
                // h2 says GOAWAY when no handle and no stream is left.
                () = sleep_until(at) => {
                    check = shared
                        .upgrade()
                        .and_then(|shared| shared.idle(&key, id, timeout));
                }
            }
        }
    }
    connection.await;
    end(&shared, &key, id);
    drop(permit);
}

/// Waits until the peer's SETTINGS have been applied: the answer to a PING, which the peer sends
/// after the SETTINGS that open its side of the connection (RFC 9113 section 3.4), and which h2
/// reads after them. A peer that does not answer within [`SETTLE`] is taken as it stands.
async fn settle(ping: Option<PingPong>) {
    if let Some(mut ping) = ping {
        drop(timeout(SETTLE, ping.ping(Ping::opaque())).await);
    }
}

/// Connection `id` of `key` has ended: it leaves the pool if it is still there.
fn end(shared: &Weak<Shared>, key: &Key, id: u64) {
    if let Some(shared) = shared.upgrade() {
        shared.retire(key, id);
    }
}
