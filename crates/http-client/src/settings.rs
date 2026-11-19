//! The settings one outbound client is built from.
//!
//! They are Go's `config.HTTPClient` and `config.Dialer` (config/config.go:131-158), read with the
//! meaning Go's `http.Transport` and `net.Dialer` give their zero and negative values.

use std::time::Duration;

/// Go's `config.HTTPClient` without `throttle`, which bidder.go reads rather than the Transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpClientSettings {
    /// `max_connections_per_host`, Go's `Transport.MaxConnsPerHost`: at most this many requests
    /// to one host at once, the others waiting for one to end (zero or less: no limit).
    pub max_connections_per_host: i64,
    /// `max_idle_connections`, Go's `Transport.MaxIdleConns`: read but without effect, since
    /// reqwest caps idle connections per host only (`http-client.max-idle-global`).
    pub max_idle_connections: i64,
    /// `max_idle_connections_per_host`, Go's `Transport.MaxIdleConnsPerHost`: zero keeps Go's
    /// default of 2, and a negative value keeps no idle connection.
    pub max_idle_connections_per_host: i64,
    /// `idle_connection_timeout_seconds`, Go's `Transport.IdleConnTimeout`: how long an idle
    /// connection is kept (zero or less: as long as the server keeps it).
    pub idle_connection_timeout_seconds: i64,
    /// `tls_handshake_timeout_seconds`: read but without effect, since reqwest has no handshake
    /// timeout; the request's deadline bounds the handshake (`http-client.timeouts-other`).
    pub tls_handshake_timeout_seconds: i64,
    /// `expect_continue_timeout_seconds`: read but without effect (`http-client.timeouts-other`).
    pub expect_continue_timeout_seconds: i64,
    /// `dialer`.
    pub dialer: DialerSettings,
}

/// Go's `config.Dialer`: the `net.Dialer` router.go builds for each Transport
/// (router/router.go:146-149).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialerSettings {
    /// `timeout_seconds`, Go's `Dialer.Timeout`: bounds the lookup and the connect, which in
    /// reqwest includes the TLS handshake (zero: no bound; negative: every dial times out, as a
    /// deadline in the past does in Go).
    pub timeout_seconds: i64,
    /// `keep_alive_seconds`, Go's `Dialer.KeepAlive`: TCP keep-alive probes start after this
    /// much idle time (zero: Go's 15 seconds; negative: no keep-alive), then come every 15
    /// seconds, at most 9 of them (Go's `KeepAliveConfig` defaults, net/dial.go:18-27).
    pub keep_alive_seconds: i64,
}

impl HttpClientSettings {
    /// `http_client_cache`'s defaults (config/config.go:996-1003): the Prebid Cache client.
    pub const CACHE: Self = Self {
        max_connections_per_host: 0,
        max_idle_connections: 10,
        max_idle_connections_per_host: 2,
        idle_connection_timeout_seconds: 60,
        tls_handshake_timeout_seconds: 10,
        expect_continue_timeout_seconds: 1,
        dialer: DialerSettings {
            timeout_seconds: 30,
            keep_alive_seconds: 15,
        },
    };
    /// `http.DefaultTransport` (net/http/transport.go:44-55), which Go's `&http.Client{}` and
    /// `http.DefaultClient` use: the currency fetch (main.go:72) and the analytics client
    /// (analytics/clients/http.go).
    pub const DEFAULT_TRANSPORT: Self = Self {
        max_connections_per_host: 0,
        max_idle_connections: 100,
        max_idle_connections_per_host: 0,
        idle_connection_timeout_seconds: 90,
        tls_handshake_timeout_seconds: 10,
        expect_continue_timeout_seconds: 1,
        dialer: DialerSettings {
            timeout_seconds: 30,
            keep_alive_seconds: 30,
        },
    };
    /// `price_floors.fetcher.http_client`'s defaults (config/config.go:1267-1274): the floors
    /// fetcher's client.
    pub const FLOORS: Self = Self {
        max_connections_per_host: 0,
        max_idle_connections: 40,
        max_idle_connections_per_host: 2,
        idle_connection_timeout_seconds: 60,
        tls_handshake_timeout_seconds: 10,
        expect_continue_timeout_seconds: 1,
        dialer: DialerSettings {
            timeout_seconds: 30,
            keep_alive_seconds: 15,
        },
    };
    /// `http_client`'s defaults (config/config.go:983-990): router.go's general client, which
    /// bidders, modules, stored requests and the vendor list use.
    pub const GENERAL: Self = Self {
        max_connections_per_host: 0,
        max_idle_connections: 400,
        max_idle_connections_per_host: 10,
        idle_connection_timeout_seconds: 60,
        tls_handshake_timeout_seconds: 10,
        expect_continue_timeout_seconds: 1,
        dialer: DialerSettings {
            timeout_seconds: 30,
            keep_alive_seconds: 15,
        },
    };
    /// Go's zero `http.Transport` with a zero `net.Dialer`, which a client that builds its own
    /// Transport without these fields has: no per-host limit, two idle connections per host kept
    /// for as long as the server keeps them, no dial timeout and 15-second keep-alives (the
    /// `/optout` recaptcha check, pbs/usersync.go:37-44).
    pub const ZERO: Self = Self {
        max_connections_per_host: 0,
        max_idle_connections: 0,
        max_idle_connections_per_host: 0,
        idle_connection_timeout_seconds: 0,
        tls_handshake_timeout_seconds: 0,
        expect_continue_timeout_seconds: 0,
        dialer: DialerSettings {
            timeout_seconds: 0,
            keep_alive_seconds: 0,
        },
    };

    /// The per-host request limit, when there is one.
    pub(crate) fn per_host_limit(&self) -> Option<usize> {
        (self.max_connections_per_host > 0).then(|| count(self.max_connections_per_host))
    }

    /// The idle connections kept per host: Go's `maxIdleConnsPerHost`, which turns zero into
    /// `DefaultMaxIdleConnsPerHost` (2), and none for a negative value (`tryPutIdleConn` never
    /// finds room).
    pub(crate) fn max_idle_per_host(&self) -> usize {
        match self.max_idle_connections_per_host {
            0 => 2,
            n if n < 0 => 0,
            n => count(n),
        }
    }

    /// How long an idle connection is kept, when there is a bound.
    pub(crate) fn idle_timeout(&self) -> Option<Duration> {
        (self.idle_connection_timeout_seconds > 0)
            .then(|| seconds(self.idle_connection_timeout_seconds))
    }
}

impl DialerSettings {
    /// The connect timeout, when there is one.
    pub(crate) const fn connect_timeout(&self) -> Option<Duration> {
        match self.timeout_seconds {
            0 => None,
            n if n < 0 => Some(Duration::ZERO),
            n => Some(seconds(n)),
        }
    }

    /// The TCP keep-alive idle time, or `None` for no keep-alive.
    pub(crate) const fn keep_alive(&self) -> Option<Duration> {
        match self.keep_alive_seconds {
            0 => Some(KEEP_ALIVE_DEFAULT),
            n if n < 0 => None,
            n => Some(seconds(n)),
        }
    }
}

/// Go's `defaultTCPKeepAliveIdle` and `defaultTCPKeepAliveInterval` (net/dial.go:20-24).
pub(crate) const KEEP_ALIVE_DEFAULT: Duration = Duration::from_secs(15);

/// Go's `defaultTCPKeepAliveCount` (net/dial.go:27).
pub(crate) const KEEP_ALIVE_PROBES: u32 = 9;

/// A positive count as a `usize`, saturating.
fn count(n: i64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// A positive number of seconds as a `Duration`.
const fn seconds(n: i64) -> Duration {
    Duration::from_secs(n.unsigned_abs())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{DialerSettings, HttpClientSettings};

    // Go: net/http/transport.go maxIdleConnsPerHost and tryPutIdleConn, net/dial.go newTCPConn.
    #[test]
    fn zero_and_negative_values_read_like_go() {
        let mut settings = HttpClientSettings::GENERAL;
        assert_eq!(settings.per_host_limit(), None, "zero is no limit");
        assert_eq!(settings.max_idle_per_host(), 10, "the configured value");
        settings.max_connections_per_host = -1;
        settings.max_idle_connections_per_host = 0;
        settings.idle_connection_timeout_seconds = 0;
        assert_eq!(settings.per_host_limit(), None, "negative is no limit");
        assert_eq!(
            settings.max_idle_per_host(),
            2,
            "DefaultMaxIdleConnsPerHost"
        );
        assert_eq!(settings.idle_timeout(), None, "zero keeps idle connections");
        settings.max_idle_connections_per_host = -1;
        assert_eq!(settings.max_idle_per_host(), 0, "negative keeps none");
        let dialer = DialerSettings {
            timeout_seconds: 0,
            keep_alive_seconds: 0,
        };
        assert_eq!(dialer.connect_timeout(), None, "zero is no dial timeout");
        assert_eq!(
            dialer.keep_alive(),
            Some(Duration::from_secs(15)),
            "zero is Go's 15 seconds"
        );
        let off = DialerSettings {
            timeout_seconds: -1,
            keep_alive_seconds: -1,
        };
        assert_eq!(
            off.connect_timeout(),
            Some(Duration::ZERO),
            "a past deadline"
        );
        assert_eq!(off.keep_alive(), None, "negative turns keep-alive off");
    }
}
