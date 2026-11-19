//! The errors of [`parse_eagerly`](super::parse_eagerly).

use core::fmt;
use std::error::Error;

/// An error from [`parse_eagerly`](super::parse_eagerly). `Display` gives the text prebid-server
/// logs with Go's `err.Error()` (gdpr/vendorlist-fetching.go:152).
#[derive(Debug)]
pub enum VendorListError {
    /// The document is not JSON or does not fit go-gdpr's contract. Go returns
    /// `encoding/json`'s error here (vendorlist2/eager-parsing.go:21-23); this is `serde_json`'s,
    /// whose text differs (`gvl.json-leniency` in the differences ledger).
    Json(serde_json::Error),
    /// `vendorListVersion` is 0 or missing (vendorlist2/eager-parsing.go:25-27).
    InvalidVersion,
}

impl fmt::Display for VendorListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(err) => fmt::Display::fmt(err, f),
            Self::InvalidVersion => {
                f.write_str("data.vendorListVersion was 0 or undefined. Versions should start at 1")
            }
        }
    }
}

impl Error for VendorListError {}
