//! IAB TCF v2 consent string and global vendor list parsing compatible with prebid/go-gdpr.
//!
//! This crate ports `github.com/prebid/go-gdpr` v1.12.0 as prebid-server uses it, keeping Go's
//! observable behaviour: accessor results, accepted and rejected inputs, and byte-identical error
//! text.
//!
//! | Go package | Rust module |
//! |---|---|
//! | `api` | [`api`] |
//! | `bitutils` | [`bitutils`] |
//! | `consentconstants`, `consentconstants/tcf2` | [`consentconstants`] |
//! | `vendorconsent` | [`vendorconsent`] |
//! | `vendorconsent/tcf1` | [`vendorconsent::tcf1`] |
//! | `vendorconsent/tcf2` | [`vendorconsent::tcf2`] |
//! | `vendorlist2` | [`vendorlist2`] |
//! | Go `encoding/base64` | [`base64`] |
//!
//! The entry points prebid-server uses are [`vendorconsent::parse_string`] (returning a
//! [`vendorconsent::ParsedConsent`]), [`vendorconsent::tcf2::parse_string`] (returning a
//! [`vendorconsent::tcf2::ConsentMetadata`]), [`vendorconsent::parse`] and
//! [`vendorlist2::parse_eagerly`]. Parsed values implement the [`api::VendorConsents`],
//! [`api::VendorList`] and [`api::Vendor`] traits.
//!
//! Errors from the consent parsers are [`ConsentError`]; vendor list errors are
//! [`vendorlist2::VendorListError`]. Their `Display` output is Go's `err.Error()`.
//!
//! Not ported, because prebid-server never calls them: `vendorconsent.ParseVersion`, the lazy
//! vendor list parsers (`vendorlist2.ParseLazily`, `vendorlist.ParseLazily`) and the TCF 1
//! vendor list parser (`vendorlist.ParseEagerly`).
//!
//! # Divergences from Go
//!
//! - One malformed TCF 2 shape makes go-gdpr panic (see [`ConsentError::IndexOutOfRange`]); it is
//!   returned as an error carrying Go's panic text.
//! - TCF 1 `PurposeAllowed` panics in Go for purpose ids whose bit lies past the end of the data;
//!   [`api::VendorConsents::purpose_allowed`] returns false there and
//!   [`vendorconsent::tcf1::Consent::checked_purpose_allowed`] returns the panic as an error.
//! - A vendor list that lists the same vendor ID under several keys gives a random winner in Go
//!   (map iteration order); see [`vendorlist2::parse_eagerly`] for the deterministic choice made
//!   here.

/// Test-only `assert_eq!` that returns an error from the enclosing test instead of panicking.
#[cfg(test)]
macro_rules! check_eq {
    ($left:expr, $right:expr, $($context:tt)+) => {{
        let (left, right) = (&$left, &$right);
        if left != right {
            return Err(
                format!("{}: left {left:?} != right {right:?}", format_args!($($context)+)).into()
            );
        }
    }};
}

/// Test-only `assert!` that returns an error from the enclosing test instead of panicking.
#[cfg(test)]
macro_rules! check {
    ($condition:expr, $($context:tt)+) => {{
        if !$condition {
            return Err(format!($($context)+).into());
        }
    }};
}

pub mod api;
pub mod base64;
pub mod bitutils;
pub mod consentconstants;
mod error;
pub mod vendorconsent;

pub use error::ConsentError;

// Public (in test builds only) so that its pub(crate) helpers are not flagged as redundant.
#[cfg(test)]
pub mod test_support;
