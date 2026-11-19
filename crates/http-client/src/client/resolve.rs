//! The resolver the clients look host names up with: the system's (`getaddrinfo` through
//! tokio), timed for [`Event::Dns`], or under the loopback guard 127.0.0.1 for every name.

use std::{
    future::Future,
    net::{Ipv4Addr, SocketAddr},
};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::{net::lookup_host, time::Instant};

use crate::metrics::{Event, Trace};

tokio::task_local! {
    /// The trace of the request whose task is running, which a lookup it starts is charged to.
    static TRACE: Option<Trace>;
}

/// Runs `future`, a request, with `trace` as the trace its lookups are charged to.
pub(super) async fn traced<F: Future>(trace: Option<Trace>, future: F) -> F::Output {
    TRACE.scope(trace, future).await
}

/// The client's resolver.
#[derive(Debug, Clone, Copy)]
pub(super) struct Resolver {
    /// The loopback guard: every name resolves to 127.0.0.1 at this port (0: the URL's).
    pub(super) loopback: Option<u16>,
}

impl Resolve for Resolver {
    fn resolve(&self, name: Name) -> Resolving {
        // The task-local is read here, while the request's task polls its connect future.
        let trace = TRACE.try_with(Clone::clone).ok().flatten();
        let loopback = self.loopback;
        Box::pin(async move {
            let start = Instant::now();
            let found = match loopback {
                Some(port) => Ok(vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))]),
                None => lookup_host((name.as_str(), 0))
                    .await
                    .map(Iterator::collect::<Vec<_>>),
            };
            if let Some(trace) = trace {
                trace.emit(Event::Dns {
                    elapsed: start.elapsed(),
                });
            }
            let addrs: Addrs = Box::new(found?.into_iter());
            Ok(addrs)
        })
    }
}
