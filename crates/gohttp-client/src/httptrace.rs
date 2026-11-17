//! Go `net/http/httptrace`: the client trace hooks the transport calls.
//!
//! [`with_client_trace`] composes with a trace already on the context the way Go's
//! `WithClientTrace` does: the new trace's hooks run first, then the old ones.

use std::{fmt, sync::Arc, time::Duration};

use crate::{context::Context, error::HttpError};

/// Go: `httptrace.GotConnInfo` (without the `net.Conn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GotConnInfo {
    /// Go: `Reused`: the connection was used for an earlier request.
    pub reused: bool,
    /// Go: `WasIdle`: the connection came from the idle pool.
    pub was_idle: bool,
    /// Go: `IdleTime`: how long it was idle (when `was_idle`).
    pub idle_time: Duration,
}

/// Go: `httptrace.DNSStartInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DnsStartInfo {
    /// Go: `Host`.
    pub host: String,
}

/// Go: `httptrace.DNSDoneInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DnsDoneInfo {
    /// Go: `Addrs` as strings.
    pub addrs: Vec<String>,
    /// Go: `Err`.
    pub err: Option<HttpError>,
    /// Go: `Coalesced` (always false: lookups are not shared).
    pub coalesced: bool,
}

/// Go: `tls.ConnectionState`, reduced to what the client reports.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConnectionState {
    /// Go: `HandshakeComplete`.
    pub handshake_complete: bool,
    /// Go: `ServerName`.
    pub server_name: String,
    /// Go: `Version` (`0x0303` for TLS 1.2, `0x0304` for TLS 1.3).
    pub version: u16,
    /// Go: `NegotiatedProtocol`.
    pub negotiated_protocol: String,
}

/// Go: `httptrace.WroteRequestInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WroteRequestInfo {
    /// Go: `Err`.
    pub err: Option<HttpError>,
}

/// A hook with a `host:port` argument.
pub type HostPortHook = Arc<dyn Fn(&str) + Send + Sync>;
/// Go: the `GotConn` hook.
pub type GotConnHook = Arc<dyn Fn(GotConnInfo) + Send + Sync>;
/// A hook with an optional error.
pub type ErrHook = Arc<dyn Fn(Option<&HttpError>) + Send + Sync>;
/// A hook without arguments.
pub type UnitHook = Arc<dyn Fn() + Send + Sync>;
/// Go: the `DNSStart` hook.
pub type DnsStartHook = Arc<dyn Fn(&DnsStartInfo) + Send + Sync>;
/// Go: the `DNSDone` hook.
pub type DnsDoneHook = Arc<dyn Fn(&DnsDoneInfo) + Send + Sync>;
/// Go: the `ConnectStart` hook (`network`, `addr`).
pub type ConnectStartHook = Arc<dyn Fn(&str, &str) + Send + Sync>;
/// Go: the `ConnectDone` hook (`network`, `addr`, `err`).
pub type ConnectDoneHook = Arc<dyn Fn(&str, &str, Option<&HttpError>) + Send + Sync>;
/// Go: the `TLSHandshakeDone` hook.
pub type TlsDoneHook = Arc<dyn Fn(&ConnectionState, Option<&HttpError>) + Send + Sync>;
/// Go: the `WroteRequest` hook.
pub type WroteRequestHook = Arc<dyn Fn(&WroteRequestInfo) + Send + Sync>;

/// Go: `httptrace.ClientTrace`. Unset hooks are `None`.
#[derive(Clone, Default)]
pub struct ClientTrace {
    /// Go: `GetConn(hostPort)`.
    pub get_conn: Option<HostPortHook>,
    /// Go: `GotConn(info)`.
    pub got_conn: Option<GotConnHook>,
    /// Go: `PutIdleConn(err)`.
    pub put_idle_conn: Option<ErrHook>,
    /// Go: `GotFirstResponseByte()`.
    pub got_first_response_byte: Option<UnitHook>,
    /// Go: `DNSStart(info)`.
    pub dns_start: Option<DnsStartHook>,
    /// Go: `DNSDone(info)`.
    pub dns_done: Option<DnsDoneHook>,
    /// Go: `ConnectStart(network, addr)`.
    pub connect_start: Option<ConnectStartHook>,
    /// Go: `ConnectDone(network, addr, err)`.
    pub connect_done: Option<ConnectDoneHook>,
    /// Go: `TLSHandshakeStart()`.
    pub tls_handshake_start: Option<UnitHook>,
    /// Go: `TLSHandshakeDone(state, err)`.
    pub tls_handshake_done: Option<TlsDoneHook>,
    /// Go: `WroteHeaders()`.
    pub wrote_headers: Option<UnitHook>,
    /// Go: `WroteRequest(info)`.
    pub wrote_request: Option<WroteRequestHook>,
}

impl fmt::Debug for ClientTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClientTrace")
    }
}

/// Every trace on a context, newest first (Go: the composed `*ClientTrace`).
#[derive(Debug, Default)]
pub struct Traces(Vec<ClientTrace>);

/// The context value holding the traces.
#[derive(Debug, Clone)]
struct TraceValue(Arc<Traces>);

/// Go: `httptrace.WithClientTrace(ctx, trace)`.
#[must_use]
pub fn with_client_trace(ctx: &Context, trace: ClientTrace) -> Context {
    let mut all = vec![trace];
    if let Some(old) = context_client_trace(ctx) {
        all.extend(old.0.iter().cloned());
    }
    ctx.with_value(TraceValue(Arc::new(Traces(all))))
}

/// Go: `httptrace.ContextClientTrace(ctx)`.
#[must_use]
pub fn context_client_trace(ctx: &Context) -> Option<Arc<Traces>> {
    ctx.value::<TraceValue>().map(|v| Arc::clone(&v.0))
}

impl Traces {
    /// Go: `GetConn(hostPort)`.
    pub fn get_conn(&self, host_port: &str) {
        self.0
            .iter()
            .filter_map(|t| t.get_conn.as_ref())
            .for_each(|h| h(host_port));
    }

    /// Go: `GotConn(info)`.
    pub fn got_conn(&self, info: GotConnInfo) {
        self.0
            .iter()
            .filter_map(|t| t.got_conn.as_ref())
            .for_each(|h| h(info));
    }

    /// Go: `PutIdleConn(err)`.
    pub fn put_idle_conn(&self, err: Option<&HttpError>) {
        self.0
            .iter()
            .filter_map(|t| t.put_idle_conn.as_ref())
            .for_each(|h| h(err));
    }

    /// Go: `GotFirstResponseByte()`.
    pub fn got_first_response_byte(&self) {
        self.0
            .iter()
            .filter_map(|t| t.got_first_response_byte.as_ref())
            .for_each(|h| h());
    }

    /// Go: `DNSStart(info)`.
    pub fn dns_start(&self, info: &DnsStartInfo) {
        self.0
            .iter()
            .filter_map(|t| t.dns_start.as_ref())
            .for_each(|h| h(info));
    }

    /// Go: `DNSDone(info)`.
    pub fn dns_done(&self, info: &DnsDoneInfo) {
        self.0
            .iter()
            .filter_map(|t| t.dns_done.as_ref())
            .for_each(|h| h(info));
    }

    /// Go: `ConnectStart(network, addr)`.
    pub fn connect_start(&self, network: &str, addr: &str) {
        self.0
            .iter()
            .filter_map(|t| t.connect_start.as_ref())
            .for_each(|h| h(network, addr));
    }

    /// Go: `ConnectDone(network, addr, err)`.
    pub fn connect_done(&self, network: &str, addr: &str, err: Option<&HttpError>) {
        self.0
            .iter()
            .filter_map(|t| t.connect_done.as_ref())
            .for_each(|h| h(network, addr, err));
    }

    /// Go: `TLSHandshakeStart()`.
    pub fn tls_handshake_start(&self) {
        self.0
            .iter()
            .filter_map(|t| t.tls_handshake_start.as_ref())
            .for_each(|h| h());
    }

    /// Go: `TLSHandshakeDone(state, err)`.
    pub fn tls_handshake_done(&self, state: &ConnectionState, err: Option<&HttpError>) {
        self.0
            .iter()
            .filter_map(|t| t.tls_handshake_done.as_ref())
            .for_each(|h| h(state, err));
    }

    /// Go: `WroteHeaders()`.
    pub fn wrote_headers(&self) {
        self.0
            .iter()
            .filter_map(|t| t.wrote_headers.as_ref())
            .for_each(|h| h());
    }

    /// Go: `WroteRequest(info)`.
    pub fn wrote_request(&self, info: &WroteRequestInfo) {
        self.0
            .iter()
            .filter_map(|t| t.wrote_request.as_ref())
            .for_each(|h| h(info));
    }
}
