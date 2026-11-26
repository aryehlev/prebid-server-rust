//! Glue (Rule B (d)): gojsonschema's error texts, which Prebid joins into the 400 response of a
//! request whose bidder params fail their schema, on `serde_json` and the regex crate.
//!
//! Prebid Server validates `imp[].ext.prebid.bidder` against the schemas of
//! `static/bidder-params`, and the rules engine module's account config against
//! `rules-engine-schema.json`, with `github.com/xeipuuv/gojsonschema` v1.2.0. No JSON Schema
//! crate passes the gates (D-JSONSCHEMA), and Prebid writes gojsonschema's error strings into its
//! responses, so this module validates the part of JSON Schema those schemas use with
//! gojsonschema's texts, order of checks and closest-branch scoring:
//!
//! - the keywords `$schema` (drafts 4, 6 and 7, and the `Hybrid` default for any other URL),
//!   `title`, `description`, `type`, `properties`, `additionalProperties`, `required`,
//!   `dependencies`, `items` (one schema for every item), `minItems`, `maxItems`, `minimum`,
//!   `maximum`, `minLength`, `maxLength`, `pattern`, `format` (`uuid` and `hostname`), `enum` (of
//!   strings), `allOf`, `anyOf`, `oneOf`, `not`, and `if` with `then` (from draft 7);
//! - patterns compiled by `pbs_util::goregex`, so that they keep Go's meaning;
//! - documents read by `serde_json` once `pbs_jsonutil::replace_lone_surrogates` has rewritten
//!   unpaired surrogate escapes as Go decodes them.
//!
//! Every other keyword gojsonschema acts on (`$ref`, `multipleOf`, `patternProperties`, `const`,
//! the tuple form of `items`, a boolean schema, the formats it checks but those two, ...) fails
//! [`Schema::new`] with an error that names it, so that no schema is validated differently from
//! Go; the keys and format names gojsonschema ignores are ignored.
//!
//! ```
//! use pbs_bidder_params::gojsonschema::Schema;
//!
//! let schema = Schema::new(br#"{"type":"object","required":["placementId"]}"#)?;
//! let result = schema.validate(b"{}")?;
//! let text: Vec<String> = result.errors().iter().map(ToString::to_string).collect();
//! assert_eq!(text, ["(root): placementId is required"]);
//!
//! let refused = Schema::new(br#"{"multipleOf":2}"#)
//!     .err()
//!     .unwrap_or_default();
//! assert!(refused.starts_with("multipleOf is not supported"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Go walks maps in random order, so where one value fails several of its members Go orders
//! the errors differently from run to run; this module walks `serde_json`'s maps in key order,
//! one of Go's orders. Numbers are `serde_json`'s (jsonschema.number-precision), and so are the
//! texts of documents that are not JSON (jsonschema.decode-errors).

mod decode;
mod draft;
mod errors;
mod format_checkers;
mod json_context;
mod number;
mod schema;
mod schema_type;
mod sub_schema;
mod validation;

pub use self::{
    decode::decode_json_using_number,
    draft::Draft,
    errors::{ErrorType, ResultError, ValidationResult},
    schema::Schema,
    validation::{GoPanic, ValidateError},
};
