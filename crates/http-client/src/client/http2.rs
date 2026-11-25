//! The bidder client's HTTP/2 (`http-client.http2`, the user's decision of 2026-11-18): h2 over
//! TLS, and h2c with the cleartext bidders that answer a probe.
//!
//! Go's bidder client speaks HTTP/1.1 to every bidder: router.go's Transport sets neither
//! `ForceAttemptHTTP2` nor `Protocols` (router/router.go:143-158). The bidder client
//! ([`Builder::negotiate_http2`](crate::Builder::negotiate_http2)) takes HTTP/2 wherever a bidder
//! offers it, with no setting:
//!
//! - over TLS, reqwest offers `h2` and `http/1.1` in ALPN and speaks what the server picks;
//! - in cleartext, a request for an origin reached without a proxy goes over h2c (HTTP/2 with prior
//!   knowledge) once the origin has answered a probe with HTTP/2's SETTINGS frame.
//!
//! The probe (`http-client.h2c-probe`) starts when a request is about to go to a cleartext origin
//! with no verdict, or one whose verdict is older than [`H2C_VERDICT_TTL`], and the request goes
//! over HTTP/1.1 at once. In a task of its own, the probe opens one connection, writes the client
//! connection preface and an empty SETTINGS frame, and reads the first frame header of the
//! answer: a SETTINGS frame on stream 0 marks the origin h2c, and the probe says GOAWAY; any
//! other answer, a close, an error or no answer within [`H2C_PROBE_TIMEOUT`] keeps the origin on
//! HTTP/1.1. The probe never carries a request, and it goes around reqwest, so its connection is
//! in no connection metric.
//!
//! A request over h2c that fails moves its origin back to HTTP/1.1 (`http-client.h2c-demotion`,
//! [`Failure`]). It is sent again, once, over HTTP/1.1 only when the origin read none of it: the
//! origin answered the preface with bytes that are not HTTP/2 (HTTP/2's client reads an HTTP/1.x
//! status line as a frame header whose length is too large, as Go's framer does: "the frame
//! header looked like an HTTP/1.1 header", testdata/golden/h2.json), or the connection failed
//! before it was ready to take the request. A request that failed after the origin may have read
//! it is not sent again; it is a bidder error, as Go's failures are.
//!
//! The h2c connection carries requests for its origin only (`http-client.h2c-redirects`): a
//! redirect that stays on the origin is followed over it, and one to another origin comes back as
//! the answer, unfollowed, and moves the origin back to HTTP/1.1, where the next request's
//! redirects are followed as Go follows them.

use std::{error::Error as _, io, iter, net::SocketAddr, time::Duration};

use hyper_util::client::legacy;
use moka::{
    ops::compute::{CompResult, Op},
    sync::Cache,
};
use reqwest::{Response, StatusCode, Url, header::LOCATION};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpStream,
    runtime::Handle,
    time::{Instant, timeout_at},
};
use url::Host;

use super::resolve::lookup;
use crate::proxy::ProxyUrl;

/// How long a verdict on a cleartext origin holds: the origin's first request after it starts a
/// new probe.
pub const H2C_VERDICT_TTL: Duration = Duration::from_mins(10);

/// How long the probe waits for its connection and for the first frame header of the answer.
pub const H2C_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long an origin stays marked as being probed: past the probe's timeout, so that no second
/// probe of the origin starts while one runs.
const PROBING: Duration = Duration::from_secs(10);

/// The most origins the verdicts are kept for.
const ORIGINS: u64 = 10_000;

/// How long the cache keeps an entry on its own clock, only to bound its memory: a verdict's
/// `until`, on tokio's clock (which the tests pause), says whether it still holds.
const KEPT: Duration = Duration::from_mins(11);

/// The port of an `http` URL that names none.
const HTTP_PORT: u16 = 80;

/// HTTP/2's client connection preface (RFC 9113 section 3.4).
const PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// A SETTINGS frame with no settings: length 0, type SETTINGS, no flags, stream 0.
const EMPTY_SETTINGS: [u8; 9] = [0, 0, 0, SETTINGS, 0, 0, 0, 0, 0];

/// A GOAWAY frame: length 8, type GOAWAY, no flags, stream 0, last stream 0, `NO_ERROR`, no
/// debug data.
const GOAWAY: [u8; 17] = [0, 0, 8, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// The SETTINGS frame type.
const SETTINGS: u8 = 0x4;

/// The SETTINGS frame's ACK flag.
const ACK: u8 = 0x1;

/// The bytes of one setting in a SETTINGS frame.
const SETTING_BYTES: u32 = 6;

/// `SETTINGS_MAX_FRAME_SIZE`'s initial value: no frame of a server's preface is longer.
const MAX_FRAME: u32 = 16_384;

/// The response header bytes Go's Transport takes by default, which bound an HTTP/2 response's
/// header list in Go as well (net/http/transport.go:329-334 `maxHeaderResponseSize`,
/// net/http/h2_bundle.go:7489-7501 `maxHeaderListSize`), in place of hyper's 16 KiB.
pub(super) const MAX_HEADER_LIST: u32 = 10 << 20;

/// A cleartext origin: its host as the URL writes it, and its port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct Origin {
    host: String,
    port: u16,
}

impl Origin {
    /// The origin of `url` when a request for it may go over h2c: an `http` URL whose request
    /// goes straight to the host, not through `proxy`.
    fn of(url: &Url, proxy: Option<&ProxyUrl>) -> Option<Self> {
        if url.scheme() != "http" || proxy.is_some() {
            return None;
        }
        Some(Self {
            host: url.host_str()?.to_owned(),
            port: url.port_or_known_default()?,
        })
    }
}

/// What is known of a cleartext origin, until when.
#[derive(Debug, Clone, Copy)]
struct Verdict {
    protocol: Protocol,
    until: Instant,
}

/// The protocol a verdict gives an origin's requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    /// A probe runs: HTTP/1.1 meanwhile.
    Probing,
    /// The origin answered the probe with SETTINGS.
    H2c,
    /// HTTP/1.1: the origin answered the probe otherwise or not at all, or failed over h2c.
    Http1,
}

/// `now` plus `duration`, or `now` past the clock's end.
fn after(now: Instant, duration: Duration) -> Instant {
    now.checked_add(duration).unwrap_or(now)
}

/// The bidder client's h2c: the verdicts on cleartext origins, and the client that speaks
/// HTTP/2 with prior knowledge to the origins marked h2c.
#[derive(Debug)]
pub(super) struct H2c {
    /// reqwest with prior knowledge: the bidder client's pool, resolver and TLS settings, no
    /// proxy, and redirects followed only on the origin.
    client: reqwest::Client,
    verdicts: Cache<Origin, Verdict>,
    /// The loopback guard, which the probe's lookups follow as the client's do.
    loopback: Option<u16>,
}

impl H2c {
    /// The h2c of a client whose prior-knowledge client is `client` and whose loopback guard is
    /// `loopback`.
    pub(super) fn new(client: reqwest::Client, loopback: Option<u16>) -> Self {
        Self {
            client,
            verdicts: Cache::builder()
                .max_capacity(ORIGINS)
                .time_to_live(KEPT)
                .build(),
            loopback,
        }
    }

    /// The origin a request for `url`, going straight to its host or through `proxy`, goes to
    /// over h2c: none unless the origin is a cleartext one marked h2c. A request for a cleartext
    /// origin with no verdict in force starts the origin's probe, and goes over HTTP/1.1.
    pub(super) fn origin(&self, url: &Url, proxy: Option<&ProxyUrl>) -> Option<Origin> {
        let origin = Origin::of(url, proxy)?;
        let now = Instant::now();
        if let Some(verdict) = self
            .verdicts
            .get(&origin)
            .filter(|verdict| verdict.until > now)
        {
            return (verdict.protocol == Protocol::H2c).then_some(origin);
        }
        self.probe(origin, url, now);
        None
    }

    /// The client that sends with prior knowledge.
    pub(super) const fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Moves `origin` back to HTTP/1.1 until its verdict runs out.
    pub(super) fn demote(&self, origin: &Origin) {
        self.verdicts.insert(
            origin.clone(),
            Verdict {
                protocol: Protocol::Http1,
                until: after(Instant::now(), H2C_VERDICT_TTL),
            },
        );
    }

    /// Starts the probe of `origin`, the origin of `url`, unless one runs or a verdict holds.
    fn probe(&self, origin: Origin, url: &Url, now: Instant) {
        let Ok(runtime) = Handle::try_current() else {
            return;
        };
        let Some(host) = url.host().map(|host| host.to_owned()) else {
            return;
        };
        let port = url.port();
        let claimed = self
            .verdicts
            .entry(origin.clone())
            .and_compute_with(|entry| match entry {
                Some(entry) if entry.value().until > now => Op::Nop,
                Some(_) | None => Op::Put(Verdict {
                    protocol: Protocol::Probing,
                    until: after(now, PROBING),
                }),
            });
        if !matches!(
            claimed,
            CompResult::Inserted(_) | CompResult::ReplacedWith(_)
        ) {
            return;
        }
        let verdicts = self.verdicts.clone();
        let loopback = self.loopback;
        drop(runtime.spawn(async move {
            let deadline = after(Instant::now(), H2C_PROBE_TIMEOUT);
            let mut stream = connect(loopback, &host, port, deadline).await;
            let h2c = match &mut stream {
                Some(stream) => answers_settings(stream, deadline).await,
                None => false,
            };
            let protocol = if h2c { Protocol::H2c } else { Protocol::Http1 };
            let until = after(Instant::now(), H2C_VERDICT_TTL);
            verdicts.insert(origin, Verdict { protocol, until });
            if let Some(stream) = stream {
                goodbye(stream, h2c, deadline).await;
            }
        }));
    }
}

/// Connects to `host` at `port` (none: 80) as the client's connector does: an IP address as it
/// is, a name at the addresses [`lookup`] gives, at their port when the URL names none, each
/// tried in turn until `deadline`.
async fn connect(
    loopback: Option<u16>,
    host: &Host<String>,
    port: Option<u16>,
    deadline: Instant,
) -> Option<TcpStream> {
    let default = port.unwrap_or(HTTP_PORT);
    let addresses = match host {
        Host::Ipv4(ip) => vec![SocketAddr::from((*ip, default))],
        Host::Ipv6(ip) => vec![SocketAddr::from((*ip, default))],
        Host::Domain(name) => {
            let mut found = timeout_at(deadline, lookup(loopback, name))
                .await
                .ok()?
                .ok()?;
            for address in &mut found {
                if port.is_some() || address.port() == 0 {
                    address.set_port(default);
                }
            }
            found
        }
    };
    for address in addresses {
        if let Ok(Ok(stream)) = timeout_at(deadline, TcpStream::connect(address)).await {
            return Some(stream);
        }
    }
    None
}

/// Writes the preface and an empty SETTINGS frame on `stream`, and reads whether the answer opens
/// with a server's preface, before `deadline`.
async fn answers_settings(stream: &mut TcpStream, deadline: Instant) -> bool {
    let mut hello = PREFACE.to_vec();
    hello.extend_from_slice(&EMPTY_SETTINGS);
    if !matches!(
        timeout_at(deadline, stream.write_all(&hello)).await,
        Ok(Ok(()))
    ) {
        return false;
    }
    let mut header = [0_u8; 9];
    matches!(
        timeout_at(deadline, stream.read_exact(&mut header)).await,
        Ok(Ok(_))
    ) && opens_preface(header)
}

/// Whether `header`, the first frame header of an answer, opens an HTTP/2 server's preface: a
/// SETTINGS frame on stream 0 that acknowledges nothing, holding whole settings within the
/// initial maximum frame size (RFC 9113 sections 3.4 and 6.5).
const fn opens_preface(header: [u8; 9]) -> bool {
    let [l0, l1, l2, kind, flags, s0, s1, s2, s3] = header;
    let length = u32::from_be_bytes([0, l0, l1, l2]);
    let stream = u32::from_be_bytes([s0 & 0x7F, s1, s2, s3]);
    kind == SETTINGS
        && flags & ACK == 0
        && stream == 0
        && length.is_multiple_of(SETTING_BYTES)
        && length <= MAX_FRAME
}

/// Ends the probe's connection: GOAWAY when the origin speaks HTTP/2, then a half-close, then
/// whatever the origin still writes until it closes or `deadline` passes, so that the origin
/// sees the connection end rather than reset.
async fn goodbye(mut stream: TcpStream, h2c: bool, deadline: Instant) {
    if h2c
        && !matches!(
            timeout_at(deadline, stream.write_all(&GOAWAY)).await,
            Ok(Ok(()))
        )
    {
        return;
    }
    if !matches!(timeout_at(deadline, stream.shutdown()).await, Ok(Ok(()))) {
        return;
    }
    let mut rest = [0_u8; 1024];
    while let Ok(Ok(read)) = timeout_at(deadline, stream.read(&mut rest)).await {
        if read == 0 {
            break;
        }
    }
}

/// How a request over h2c failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Failure {
    /// The origin read none of the request. Either it answered the preface with bytes that are
    /// not HTTP/2: HTTP/2's client tore the connection down on a frame too large, as an HTTP/1.x
    /// status line reads (its `HTT` is a length of 4,740,180). Or the connection failed before
    /// it was ready to take the request: in HTTP/2's handshake, or closed by then. The origin
    /// goes back to HTTP/1.1 and the request is sent again over it, once.
    Unread,
    /// The connection failed once it had the request: an HTTP/2 error of either side, the
    /// connection closing or resetting, or hyper giving the request up with the connection,
    /// after which the origin may have read the request. The origin goes back to HTTP/1.1; the
    /// request is not sent again.
    Failed,
    /// Nothing the origin's HTTP/2 said: the dial failed or timed out, or a redirect was refused.
    /// The verdict stays.
    Other,
}

impl Failure {
    /// How `error`, reqwest's error for a request over h2c, failed.
    pub(super) fn of(error: &reqwest::Error) -> Self {
        if error.is_connect() || error.is_timeout() || error.is_redirect() {
            return Self::Other;
        }
        // Whether hyper-util's client gave the request to a connection: its error then names
        // the connection.
        let mut given = false;
        for cause in iter::successors(error.source(), |&cause| cause.source()) {
            if let Some(client) = cause.downcast_ref::<legacy::Error>() {
                // No connection named: the request failed while hyper-util set its connection
                // up, in HTTP/2's handshake or on a connection gone before it was ready, so no
                // connection took the request.
                if client.connect_info().is_none() {
                    return Self::Unread;
                }
                given = true;
            } else if let Some(h2) = cause.downcast_ref::<h2::Error>() {
                let frame_too_large = h2.is_go_away()
                    && h2.is_library()
                    && h2.reason() == Some(h2::Reason::FRAME_SIZE_ERROR);
                return if frame_too_large {
                    Self::Unread
                } else {
                    Self::Failed
                };
            } else if cause.is::<io::Error>() {
                return Self::Failed;
            }
        }
        // A connection's failure with no HTTP/2 or I/O error behind it: hyper gave the request
        // back unsent (its `Canceled`), or dropped it with the connection's task, which can
        // follow the request's stream opening (its `DispatchGone`). Only hyper's own error type
        // tells the two apart, so the request counts as one the origin may have read.
        if given { Self::Failed } else { Self::Other }
    }
}

/// Whether `response`, an answer over h2c from `origin`, is a redirect the client stopped at
/// because it leaves the origin: one reqwest follows (301, 302, 303, 307 or 308) whose `Location`
/// names another origin.
pub(super) fn leaves(response: &Response, origin: &Origin) -> bool {
    let followed = [
        StatusCode::MOVED_PERMANENTLY,
        StatusCode::FOUND,
        StatusCode::SEE_OTHER,
        StatusCode::TEMPORARY_REDIRECT,
        StatusCode::PERMANENT_REDIRECT,
    ];
    if !followed.contains(&response.status()) {
        return false;
    }
    let next = response
        .headers()
        .get(LOCATION)
        .and_then(|location| location.to_str().ok())
        .and_then(|location| response.url().join(location).ok());
    next.is_some_and(|next| Origin::of(&next, None).as_ref() != Some(origin))
}

#[cfg(test)]
mod tests {
    use super::{EMPTY_SETTINGS, GOAWAY, PREFACE, opens_preface};

    // Go: golang.org/x/net/http2 frame.go Framer.WriteSettings and WriteGoAway (the bytes
    // testdata/golden/h2.json records), and readFrameHeader with invalidHTTP1LookingFrameHeader.
    #[test]
    fn frames_read_as_http2_reads_them() {
        assert_eq!(
            PREFACE.as_slice(),
            b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n",
            "the preface"
        );
        assert_eq!(
            (EMPTY_SETTINGS, GOAWAY.get(..9)),
            (
                [0, 0, 0, 4, 0, 0, 0, 0, 0],
                Some([0, 0, 8, 7, 0, 0, 0, 0, 0].as_slice())
            ),
            "the frame headers"
        );
        assert!(
            opens_preface([0, 0, 0x1E, 4, 0, 0, 0, 0, 0]),
            "Go's h2c server: five settings"
        );
        assert!(
            opens_preface([0, 0, 0, 4, 0, 0, 0, 0, 0]),
            "an empty SETTINGS frame"
        );
        assert!(!opens_preface(*b"HTTP/1.1 "), "an HTTP/1.1 status line");
        assert!(
            !opens_preface([0, 0, 0, 4, 1, 0, 0, 0, 0]),
            "an acknowledgement"
        );
        assert!(
            !opens_preface([0, 0, 6, 4, 0, 0, 0, 0, 1]),
            "a SETTINGS frame on a stream"
        );
        assert!(
            !opens_preface([0, 0, 5, 4, 0, 0, 0, 0, 0]),
            "a partial setting"
        );
        assert!(
            !opens_preface([0, 0x40, 2, 4, 0, 0, 0, 0, 0]),
            "longer than the initial maximum frame size"
        );
        assert!(
            !opens_preface([0, 0, 4, 8, 0, 0, 0, 0, 0]),
            "a WINDOW_UPDATE first"
        );
    }
}
