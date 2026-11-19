//! The resolver the clients look host names up with: the system's (`getaddrinfo` through
//! tokio), timed for [`Event::Dns`], or under the loopback guard 127.0.0.1 for every name but
//! the names under `invalid.`. A lookup fails with a [`DnsError`].

use std::{
    future::Future,
    net::{Ipv4Addr, SocketAddr},
};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::{net::lookup_host, time::Instant};

use crate::{
    error::DnsError,
    metrics::{Event, Trace},
};

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
    /// The loopback guard: every name resolves to 127.0.0.1 at this port (0: the URL's), but a
    /// name under `invalid.`, which never resolves (RFC 6761) and so stands for a host that does
    /// not exist.
    pub(super) loopback: Option<u16>,
}

impl Resolve for Resolver {
    fn resolve(&self, name: Name) -> Resolving {
        // The task-local is read here, while the request's task polls its connect future.
        let trace = TRACE.try_with(Clone::clone).ok().flatten();
        let loopback = self.loopback;
        Box::pin(async move {
            let start = Instant::now();
            let host = name.as_str();
            let rooted = host.strip_suffix('.').unwrap_or(host);
            let found = match loopback {
                Some(_) if rooted == "invalid" || rooted.ends_with(".invalid") => {
                    Err(DnsError::no_such_host(host))
                }
                Some(port) => Ok(vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))]),
                None => match lookup_host((host, 0)).await {
                    Ok(addrs) => {
                        let addrs: Vec<SocketAddr> = addrs.collect();
                        if addrs.is_empty() {
                            Err(DnsError::no_such_host(host))
                        } else {
                            Ok(addrs)
                        }
                    }
                    Err(error) => Err(DnsError::from_lookup(host, &error)),
                },
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
