//! A port of `github.com/xeipuuv/gojsonschema` v1.2.0 (the version Prebid Server pins).
//!
//! Only what a self-contained schema needs is ported: every keyword of drafts 4, 6, 7 and the
//! `Hybrid` default, the default English locale, the error ordering and the `anyOf`/`oneOf`
//! "closest branch" scoring. Not ported (a schema using them fails to compile instead of
//! validating differently): `$ref` (no bidder schema uses it) and the format checkers backed by
//! other Go packages (`date`, `time`, `date-time`, `email`, `idn-email`, `ipv4`, `ipv6`, `uri`,
//! `uri-reference`, `iri`, `iri-reference`, `uri-template`).
//!
//! ```
//! use pbs_bidder_params::gojsonschema::Schema;
//!
//! let schema = Schema::new(br#"{"type":"object","required":["placementId"]}"#)?;
//! let result = schema.validate(b"{}")?;
//! let text: Vec<String> = result.errors().iter().map(ToString::to_string).collect();
//! assert_eq!(text, ["(root): placementId is required"]);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Go iterates maps in random order, so when one value produces several errors from map
//! iteration (several unknown properties, several properties failing) Go's order differs from
//! run to run; this port uses document order, which is one of Go's possible orders.

mod draft;
mod errors;
mod format_checkers;
mod go_url;
mod json_context;
mod json_loader;
mod marshal;
mod rat;
mod regexp;
mod schema;
mod schema_type;
mod sub_schema;
mod utils;
mod validation;

pub use self::{
    draft::Draft,
    errors::{ErrorType, ResultError, ValidationResult},
    json_loader::{Document, Node, NodeId, decode_json_using_number},
    regexp::GoRegexp,
    schema::Schema,
    validation::{GoPanic, ValidateError},
};
