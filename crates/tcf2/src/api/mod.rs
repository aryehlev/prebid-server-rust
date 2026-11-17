//! Port of go-gdpr `api`: the interfaces implemented by parsed consent strings and vendor lists.
//!
//! go-gdpr's `vendorlist.VendorList` and `vendorlist.Vendor` (used as types by prebid-server's
//! `gdpr/vendorlist-fetching.go`) only embed these interfaces, so they map to the same traits.

mod consent;
mod vendorlist;

pub use consent::{ConsentTime, VendorConsents};
pub use vendorlist::{Vendor, VendorList};
