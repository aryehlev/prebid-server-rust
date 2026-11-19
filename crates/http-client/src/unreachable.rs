//! Glue (Rule B (d)): the failed requests bidder.go reports as an unreachable bidder, seat non-bid
//! code 103 (`ErrorBidderUnreachable`) in the response's `ext.seatnonbid`
//! (exchange/non_bid_reason.go:41-57): a refused connection, `errors.Is(err,
//! syscall.ECONNREFUSED)`, or a host name that does not exist, a `*net.DNSError` whose
//! `IsNotFound` holds.
//!
//! reqwest's errors say only whether the connection failed. A refused connection still ends its
//! chain in the dial's I/O error, which
//! [`RequestError::is_connection_refused`](crate::RequestError::is_connection_refused) finds;
//! the clients' resolver fails with a [`DnsError`], whose not-found flag it reads from the system
//! resolver's message, since Rust's std turns getaddrinfo's code into `gai_strerror`'s text and
//! keeps no code (`http-client.unreachable-classification`, restored; pin
//! `crates/http-client/tests/errors.rs::unreachable_like_go` against
//! `testdata/golden/errors.json`).

use std::{error::Error, fmt, io};

/// A host-name lookup that failed: Go's `*net.DNSError`, as the clients' resolver fails with it
/// and as a transport that does not go to the network makes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsError {
    name: String,
    message: String,
    not_found: bool,
}

/// `gai_strerror`'s texts for `EAI_NONAME` and `EAI_NODATA`, the codes Go's cgo resolver reports
/// as not found (net/cgo_unix.go:189-190), in the C locale Rust programs run in: glibc's, musl's
/// `EAI_NONAME` and macOS's.
const NOT_FOUND: [&str; 5] = [
    "Name or service not known",
    "No address associated with hostname",
    "Name does not resolve",
    "nodename nor servname provided, or not known",
    "No address associated with nodename",
];

/// Go's `errNoSuchHost` text.
const NO_SUCH_HOST: &str = "no such host";

impl DnsError {
    /// The lookup of `name` failed with `message`; `not_found` is Go's `IsNotFound`, true when
    /// the name does not exist or has no address (a test's `&net.DNSError{IsNotFound: true}`).
    #[must_use]
    pub const fn new(name: String, message: String, not_found: bool) -> Self {
        Self {
            name,
            message,
            not_found,
        }
    }

    /// The system resolver's lookup of `name` failed with `error`: getaddrinfo's, through std,
    /// whose text names the code.
    pub(crate) fn from_lookup(name: &str, error: &io::Error) -> Self {
        let message = error.to_string();
        let not_found = NOT_FOUND.iter().any(|text| message.ends_with(text));
        Self::new(name.to_owned(), message, not_found)
    }

    /// `name` has no address: the lookup found none, as Go's resolvers report an empty answer,
    /// or the name is under `invalid.`, which never resolves (RFC 6761).
    pub(crate) fn no_such_host(name: &str) -> Self {
        Self::new(name.to_owned(), NO_SUCH_HOST.to_owned(), true)
    }

    /// The name looked up.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Go's `IsNotFound`: the name does not exist or has no address.
    #[must_use]
    pub const fn is_not_found(&self) -> bool {
        self.not_found
    }
}

impl fmt::Display for DnsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for DnsError {}

#[cfg(test)]
mod tests {
    use std::io;

    use super::DnsError;

    /// The error std makes of a getaddrinfo failure (library/std/src/sys/net/connection/socket/
    /// unix.rs `cvt_gai`): `gai_strerror`'s text behind its own words.
    fn gai(text: &str) -> io::Error {
        io::Error::other(format!("failed to lookup address information: {text}"))
    }

    // Go: net/cgo_unix.go:189-190 (EAI_NONAME and EAI_NODATA are errNoSuchHost, IsNotFound) and
    // net/dnsclient_unix.go (Go's resolver: no such name, or no address, is IsNotFound; a
    // failing server is not).
    #[test]
    fn not_found_codes_like_go() {
        for (libc, text, not_found) in [
            ("glibc EAI_NONAME", "Name or service not known", true),
            (
                "glibc EAI_NODATA",
                "No address associated with hostname",
                true,
            ),
            (
                "glibc EAI_AGAIN",
                "Temporary failure in name resolution",
                false,
            ),
            (
                "glibc EAI_FAIL",
                "Non-recoverable failure in name resolution",
                false,
            ),
            ("musl EAI_NONAME", "Name does not resolve", true),
            ("musl EAI_AGAIN", "Try again", false),
            (
                "macOS EAI_NONAME",
                "nodename nor servname provided, or not known",
                true,
            ),
            (
                "macOS EAI_NODATA",
                "No address associated with nodename",
                true,
            ),
            (
                "macOS EAI_AGAIN",
                "Temporary failure in name resolution",
                false,
            ),
        ] {
            let error = DnsError::from_lookup("bidder.test", &gai(text));
            assert_eq!(error.is_not_found(), not_found, "{libc}");
            assert_eq!(
                error.to_string(),
                format!("failed to lookup address information: {text}"),
                "{libc}: the system's text"
            );
        }
        assert!(
            DnsError::no_such_host("x.invalid").is_not_found(),
            "an empty answer"
        );
        assert!(
            !DnsError::from_lookup("bidder.test", &io::Error::other("background task failed"))
                .is_not_found(),
            "a lookup that never ran"
        );
    }
}
