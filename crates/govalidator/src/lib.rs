//! Glue (Rule B (c)): govalidator's `IsURL`, `IsRequestURL`, `IsRequestURI` and `IsIP`, which
//! decide which bidder-info files, host config and account config Prebid accepts
//! (url.govalidator-checks).
//!
//! Prebid checks bidder endpoints (`config/bidderinfo.go:516`) and user sync URLs
//! (`usersync/syncer.go:179`) with [`is_url`] and [`is_request_url`], VAST event URLs the same
//! way (`config/events.go:141`), and fetches an account's price floors only from a URL
//! [`is_url`] accepts (`floors/fetcher.go:133`). The port follows
//! `github.com/asaskevich/govalidator` `v0.0.0-20210307081110-f21760c49a8d`, the version
//! Prebid's `go.mod` requires, from the same parts as Go's: govalidator's [`URL`] pattern
//! compiled by `pbs_util::goregex`, so that it keeps Go's meaning; the part of Go 1.26.2's
//! `url.Parse` and `url.ParseRequestURI` the checks read (`validator::url`); and `net.ParseIP`
//! as `pbs_util::iputil::net::parse_ip` reads it.
//!
//! ```
//! use pbs_govalidator::{is_ip, is_request_url, is_url};
//!
//! assert!(is_url("https://prebid.org/openrtb2/auction"));
//! assert!(is_url("localhost:8080"));
//! assert!(!is_url("http://-a.com"));
//! assert!(!is_request_url("prebid.org/path"));
//! assert!(is_ip("::ffff:1.2.3.4"));
//! ```
//!
//! Go strings may hold any bytes; these functions take `&str`, as configuration values are
//! UTF-8.

mod validator;

pub use validator::{URL, is_ip, is_request_uri, is_request_url, is_url};
