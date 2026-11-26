//! Glue (Rule B (c)): `max_connections_per_host` is an operator key reqwest has no knob for.
//!
//! Go's `Transport.MaxConnsPerHost` lets at most that many connections exist per connect key
//! (proxy, scheme and address; net/http/transport.go `connectMethod.key`), and a request that
//! needs one more waits, under its deadline, until one is free. Here a semaphore per key admits
//! that many requests at once; a request holds its slot until its body is read or dropped, so at
//! most that many connections carry requests to the host at once
//! (`http-client.max-conns-per-host`, restored; pin
//! `crates/http-client/tests/limits.rs::per_host_limit_waits` against
//! `testdata/golden/limits.json`). With `http_client.http2.enabled`, an HTTP/2 connection of the
//! bidder client's pool holds one of its origin's slots for as long as it is open, as Go counts
//! connections, and its streams take none.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Go's `connectMethodKey`: the proxy (empty when there is none), the target's scheme and its
/// `host:port`, which is empty for an `http` target sent through a proxy, since such requests
/// share the proxy's connections.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct Key {
    pub(super) proxy: String,
    pub(super) scheme: String,
    pub(super) addr: String,
}

/// The semaphores of one client, one per key.
#[derive(Debug)]
pub(super) struct HostLimiter {
    limit: usize,
    hosts: Mutex<BTreeMap<Key, Arc<Semaphore>>>,
}

impl HostLimiter {
    /// A limiter admitting `limit` requests per key at once.
    pub(super) fn new(limit: usize) -> Self {
        Self {
            limit: limit.min(Semaphore::MAX_PERMITS),
            hosts: Mutex::new(BTreeMap::new()),
        }
    }

    /// Waits for a slot of `key`.
    pub(super) async fn acquire(&self, key: Key) -> Option<OwnedSemaphorePermit> {
        // The semaphores are never closed, so acquiring only fails if that changes; the request
        // then goes ahead without a slot rather than failing.
        self.semaphore(key).acquire_owned().await.ok()
    }

    /// The semaphore of `key`'s slots: an HTTP/1.1 request holds one while it is in flight, and
    /// a pooled HTTP/2 connection while it is open.
    pub(super) fn semaphore(&self, key: Key) -> Arc<Semaphore> {
        let mut hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(
            hosts
                .entry(key)
                .or_insert_with(|| Arc::new(Semaphore::new(self.limit))),
        )
    }
}
