//! The Go error values the client returns, with Go's texts and `errors.Is` relations.

use std::{borrow::Cow, error, fmt};

use pbs_gocompat::strconv::quote;

/// Go: `context.Canceled` and `context.DeadlineExceeded`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContextError {
    /// Go: `context.Canceled` (`context canceled`).
    Canceled,
    /// Go: `context.DeadlineExceeded` (`context deadline exceeded`).
    DeadlineExceeded,
}

impl fmt::Display for ContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Canceled => "context canceled",
            Self::DeadlineExceeded => "context deadline exceeded",
        })
    }
}

impl error::Error for ContextError {}

/// Every error the client, transport and body readers return.
///
/// [`fmt::Display`] is Go's `Error()` text. Wrapping variants keep the wrapped error so that
/// [`HttpError::is_deadline_exceeded`], [`HttpError::is_canceled`] and [`HttpError::is_eof`]
/// answer as Go's `errors.Is` does, and [`HttpError::timeout`] as `net.Error.Timeout()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpError {
    /// Go: `context.Canceled` / `context.DeadlineExceeded` (returned as the value itself, so a
    /// Go comparison `err == context.DeadlineExceeded` is `err == HttpError::Context(..)`).
    Context(ContextError),
    /// Go: `*url.Error`, as `Client.Do` returns every error.
    Url(Box<UrlError>),
    /// Go: `*net.OpError` (`dial tcp 1.2.3.4:80: connect: connection refused`).
    Op(Box<OpError>),
    /// Go: `*os.SyscallError` (`connect: connection refused`).
    Syscall(Box<SyscallError>),
    /// Go: `*net.DNSError` (`lookup x.invalid: no such host`).
    Dns(Box<DnsError>),
    /// Go: `io.EOF`.
    Eof,
    /// Go: `io.ErrUnexpectedEOF`.
    UnexpectedEof,
    /// A Go timeout error whose `Is(context.DeadlineExceeded)` is true: `net`'s `i/o timeout`,
    /// `net/http: timeout awaiting response headers`, and the `Client.Timeout` texts.
    Timeout(Cow<'static, str>),
    /// Go: `tlsHandshakeTimeoutError` (`net/http: TLS handshake timeout`).
    TlsHandshakeTimeout,
    /// Go: `fmt.Errorf("net/http: HTTP/1.x transport connection broken: %w", err)`.
    ConnectionBroken(Box<Self>),
    /// Go: `fmt.Errorf("<prefix>: %w", err)`.
    Wrapped(Box<WrappedError>),
    /// Go: an `errors.New` value.
    Static(&'static str),
    /// Go: a `fmt.Errorf` value without `%w`, or any other error known only by its text.
    Message(String),
    /// Go: `http.ErrUseLastResponse`, returned by a `CheckRedirect` function to stop following
    /// redirects and return the last response unclosed.
    UseLastResponse,
    /// Go: `tls.RecordHeaderError` for a first record that is not TLS, with the five bytes
    /// read (`Client.Do` reports `http: server gave HTTP response to HTTPS client` when they
    /// are `HTTP/`).
    RecordHeader([u8; 5]),
}

/// Go: `*url.Error`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlError {
    /// Go: `Op` (`"Get"`, `"Post"`, ...).
    pub op: String,
    /// Go: `URL`, the request URL with any password replaced by `***`.
    pub url: String,
    /// Go: `Err`.
    pub err: HttpError,
}

/// Go: `*net.OpError`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpError {
    /// Go: `Op` (`"dial"`, `"read"`, `"write"`, `"proxyconnect"`).
    pub op: &'static str,
    /// Go: `Net` (`"tcp"`).
    pub net: &'static str,
    /// Go: `Source.String()` when there is a source address.
    pub source: Option<String>,
    /// Go: `Addr.String()` when there is an address.
    pub addr: Option<String>,
    /// Go: `Err`.
    pub err: HttpError,
}

/// Go: `*os.SyscallError` over a `syscall.Errno`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyscallError {
    /// Go: `Syscall` (`"connect"`, `"read"`, `"write"`).
    pub syscall: &'static str,
    /// Go: `Errno.Error()` (`connection refused`).
    pub errno: String,
    /// Go: `Errno.Timeout()` (`EAGAIN`, `EWOULDBLOCK`, `ETIMEDOUT`).
    pub timeout: bool,
}

/// Go: `*net.DNSError`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DnsError {
    /// Go: `Err` (`no such host`).
    pub err: String,
    /// Go: `Name`.
    pub name: String,
    /// Go: `Server` (empty when unknown).
    pub server: String,
    /// Go: `IsTimeout`.
    pub is_timeout: bool,
    /// Go: `IsNotFound`.
    pub is_not_found: bool,
}

/// Go: the error `fmt.Errorf("<prefix>: %w", err)` returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrappedError {
    /// The text before `": "`.
    pub prefix: Cow<'static, str>,
    /// The wrapped error.
    pub err: HttpError,
}

impl HttpError {
    /// Go: `context.Canceled`.
    pub const CANCELED: Self = Self::Context(ContextError::Canceled);
    /// Go: `context.DeadlineExceeded`.
    pub const DEADLINE_EXCEEDED: Self = Self::Context(ContextError::DeadlineExceeded);

    /// Go: `fmt.Errorf("<prefix>: %w", err)`.
    #[must_use]
    pub fn wrap<P: Into<Cow<'static, str>>>(prefix: P, err: Self) -> Self {
        Self::Wrapped(Box::new(WrappedError {
            prefix: prefix.into(),
            err,
        }))
    }

    /// Go: `errors.Unwrap(err)`.
    #[must_use]
    pub fn unwrap_err(&self) -> Option<&Self> {
        match self {
            Self::Url(u) => Some(&u.err),
            Self::Op(o) => Some(&o.err),
            Self::ConnectionBroken(e) => Some(e),
            Self::Wrapped(w) => Some(&w.err),
            Self::Context(_)
            | Self::Syscall(_)
            | Self::Dns(_)
            | Self::Eof
            | Self::UnexpectedEof
            | Self::Timeout(_)
            | Self::TlsHandshakeTimeout
            | Self::Static(_)
            | Self::Message(_)
            | Self::UseLastResponse
            | Self::RecordHeader(_) => None,
        }
    }

    /// Walks the `errors.Is` chain, asking `pred` of each error.
    fn chain_any<F: Fn(&Self) -> bool>(&self, pred: F) -> bool {
        let mut e = Some(self);
        while let Some(cur) = e {
            if pred(cur) {
                return true;
            }
            e = cur.unwrap_err();
        }
        false
    }

    /// Go: `errors.Is(err, context.DeadlineExceeded)`, which also holds for Go's timeout errors
    /// (`i/o timeout`, `Client.Timeout exceeded`).
    #[must_use]
    pub fn is_deadline_exceeded(&self) -> bool {
        self.chain_any(|e| match e {
            Self::Context(c) => *c == ContextError::DeadlineExceeded,
            Self::Timeout(_) => true,
            Self::Dns(d) => d.is_timeout,
            Self::Url(_)
            | Self::Op(_)
            | Self::Syscall(_)
            | Self::Eof
            | Self::UnexpectedEof
            | Self::TlsHandshakeTimeout
            | Self::ConnectionBroken(_)
            | Self::Wrapped(_)
            | Self::Static(_)
            | Self::Message(_)
            | Self::UseLastResponse
            | Self::RecordHeader(_) => false,
        })
    }

    /// Go: `errors.Is(err, context.Canceled)`.
    #[must_use]
    pub fn is_canceled(&self) -> bool {
        self.chain_any(|e| *e == Self::CANCELED)
    }

    /// Go: `errors.Is(err, io.EOF)`.
    #[must_use]
    pub fn is_eof(&self) -> bool {
        self.chain_any(|e| *e == Self::Eof)
    }

    /// Go: `err.(net.Error).Timeout()` (false for errors that are not `net.Error`s).
    #[must_use]
    pub fn timeout(&self) -> bool {
        match self {
            Self::Context(c) => *c == ContextError::DeadlineExceeded,
            Self::Timeout(_) | Self::TlsHandshakeTimeout => true,
            Self::Url(u) => u.err.timeout(),
            Self::Op(o) => o.err.timeout(),
            Self::Syscall(s) => s.timeout,
            Self::Dns(d) => d.is_timeout,
            Self::Eof
            | Self::UnexpectedEof
            | Self::ConnectionBroken(_)
            | Self::Wrapped(_)
            | Self::Static(_)
            | Self::Message(_)
            | Self::UseLastResponse
            | Self::RecordHeader(_) => false,
        }
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context(c) => c.fmt(f),
            Self::Url(u) => write!(f, "{} {}: {}", u.op, quote(&u.url), u.err),
            Self::Op(o) => {
                f.write_str(o.op)?;
                if !o.net.is_empty() {
                    write!(f, " {}", o.net)?;
                }
                if let Some(src) = &o.source {
                    write!(f, " {src}")?;
                }
                if let Some(addr) = &o.addr {
                    let sep = if o.source.is_some() { "->" } else { " " };
                    write!(f, "{sep}{addr}")?;
                }
                write!(f, ": {}", o.err)
            }
            Self::Syscall(s) => write!(f, "{}: {}", s.syscall, s.errno),
            Self::Dns(d) => {
                write!(f, "lookup {}", d.name)?;
                if !d.server.is_empty() {
                    write!(f, " on {}", d.server)?;
                }
                write!(f, ": {}", d.err)
            }
            Self::Eof => f.write_str("EOF"),
            Self::UnexpectedEof => f.write_str("unexpected EOF"),
            Self::Timeout(s) => f.write_str(s),
            Self::TlsHandshakeTimeout => f.write_str("net/http: TLS handshake timeout"),
            Self::ConnectionBroken(e) => {
                write!(f, "net/http: HTTP/1.x transport connection broken: {e}")
            }
            Self::Wrapped(w) => write!(f, "{}: {}", w.prefix, w.err),
            Self::Static(s) => f.write_str(s),
            Self::Message(s) => f.write_str(s),
            Self::UseLastResponse => f.write_str("net/http: use last response"),
            Self::RecordHeader(h) => f.write_str(if h.first() == Some(&0x80) {
                "tls: unsupported SSLv2 handshake received"
            } else {
                "tls: first record does not look like a TLS handshake"
            }),
        }
    }
}

impl error::Error for HttpError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        let inner = self.unwrap_err()?;
        Some(inner)
    }
}

impl From<ContextError> for HttpError {
    fn from(e: ContextError) -> Self {
        Self::Context(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::ensure_eq;

    #[test]
    fn texts_and_relations() -> Result<(), String> {
        let dial = HttpError::Op(Box::new(OpError {
            op: "dial",
            net: "tcp",
            source: None,
            addr: Some("127.0.0.1:1".to_owned()),
            err: HttpError::Syscall(Box::new(SyscallError {
                syscall: "connect",
                errno: "connection refused".to_owned(),
                timeout: false,
            })),
        }));
        let url = HttpError::Url(Box::new(UrlError {
            op: "Post".to_owned(),
            url: "http://127.0.0.1:1/x".to_owned(),
            err: dial,
        }));
        ensure_eq(
            &url.to_string(),
            &r#"Post "http://127.0.0.1:1/x": dial tcp 127.0.0.1:1: connect: connection refused"#
                .to_owned(),
            "dial text",
        )?;
        ensure_eq(&url.timeout(), &false, "refused is no timeout")?;
        let timeout = HttpError::Timeout(Cow::Borrowed("i/o timeout"));
        ensure_eq(
            &timeout.is_deadline_exceeded(),
            &true,
            "i/o timeout is deadline",
        )?;
        let broken = HttpError::ConnectionBroken(Box::new(HttpError::UnexpectedEof));
        ensure_eq(
            &broken.to_string(),
            &"net/http: HTTP/1.x transport connection broken: unexpected EOF".to_owned(),
            "broken",
        )?;
        ensure_eq(
            &broken.timeout(),
            &false,
            "fmt.Errorf wrapper is not a net.Error",
        )
    }
}
