//! The connection events of one request.
//!
//! They stand for the httptrace hooks bidder.go installs (exchange/bidder.go:732-798): `GotConn`
//! with its `Reused` flag and the wait since `GetConn`, and the `DNSStart`/`DNSDone` lookup time.
//!
//! reqwest exposes neither its pool nor its connector, so the events are reconstructed
//! (`http-client.connection-metrics`): a lookup is timed by the client's resolver and charged to
//! the request whose task started it; reuse is read from the connection's addresses, which
//! reqwest puts on every response (a connection seen before is reused); the wait is the time the
//! request waited for its host's `max_connections_per_host` slot. There are no `ConnectStart`,
//! `ConnectDone` or TLS handshake events, and a request that fails before its response head
//! reports no `GotConn`.

use std::{
    collections::BTreeMap,
    fmt,
    net::SocketAddr,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use tokio::time::Instant;

/// One connection event of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// A host-name lookup for the request's connection ended (Go's `DNSStart` then `DNSDone`).
    Dns {
        /// How long the lookup took.
        elapsed: Duration,
    },
    /// The request's response came on a connection (Go's `GotConn`).
    GotConn {
        /// Whether the connection had carried an earlier request.
        reused: bool,
        /// How long the request waited for its host's `max_connections_per_host` slot.
        wait: Duration,
    },
}

/// Where a request's events go: bidder.go's hooks, which record them in the metrics.
#[derive(Clone)]
pub struct Trace(Arc<dyn Fn(Event) + Send + Sync>);

impl Trace {
    /// A trace that hands every event to `record`.
    pub fn new<F>(record: F) -> Self
    where
        F: Fn(Event) + Send + Sync + 'static,
    {
        Self(Arc::new(record))
    }

    /// Records `event`.
    pub(crate) fn emit(&self, event: Event) {
        (self.0)(event);
    }
}

impl fmt::Debug for Trace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Trace")
    }
}

/// The connections a client has seen, by their local and remote addresses, with when each was
/// last seen.
#[derive(Debug, Default)]
pub(crate) struct Connections {
    seen: Mutex<BTreeMap<(SocketAddr, SocketAddr), Instant>>,
}

/// How many connections the registry keeps before it forgets the oldest.
const REMEMBERED: usize = 4096;

impl Connections {
    /// Records that a response came on the connection `local` to `remote` at `now`, and returns
    /// whether that connection had been seen before. A connection unseen for longer than
    /// `horizon` (the pool's idle timeout) counts as new: the pool has closed it, and the
    /// addresses belong to a new one.
    pub(crate) fn observe(
        &self,
        local: SocketAddr,
        remote: SocketAddr,
        now: Instant,
        horizon: Option<Duration>,
    ) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        let reused = seen
            .insert((local, remote), now)
            .is_some_and(|last| horizon.is_none_or(|h| now.duration_since(last) <= h));
        if seen.len() > REMEMBERED {
            let mut times: Vec<Instant> = seen.values().copied().collect();
            times.sort_unstable();
            if let Some(&cut) = times.get(REMEMBERED / 2) {
                seen.retain(|_, last| *last >= cut);
            }
        }
        drop(seen);
        reused
    }
}

#[cfg(test)]
mod tests {
    use std::{
        net::{Ipv4Addr, SocketAddr},
        time::Duration,
    };

    use tokio::time::Instant;

    use super::Connections;

    // Go: net/http/transport.go persistConn.isReused (a connection that carried a request).
    #[test]
    fn a_connection_is_reused_once_seen() {
        let connections = Connections::default();
        let local = SocketAddr::from((Ipv4Addr::LOCALHOST, 50_000));
        let remote = SocketAddr::from((Ipv4Addr::LOCALHOST, 8080));
        let now = Instant::now();
        let horizon = Some(Duration::from_secs(60));
        assert!(
            !connections.observe(local, remote, now, horizon),
            "first response"
        );
        assert!(
            connections.observe(local, remote, now, horizon),
            "second response"
        );
        let later = now + Duration::from_secs(61);
        assert!(
            !connections.observe(local, remote, later, horizon),
            "past the idle timeout the addresses are a new connection's"
        );
    }
}
