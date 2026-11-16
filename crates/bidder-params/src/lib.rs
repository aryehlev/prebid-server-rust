//! JSON Schema validation of bidder params with gojsonschema-compatible error text.
//!
//! Prebid Server validates every `imp[i].ext.prebid.bidder.<bidder>` object against the
//! bidder's JSON schema in `static/bidder-params/` (Go: `openrtb_ext.NewBidderParamsValidator`,
//! backed by `github.com/xeipuuv/gojsonschema` v1.2.0). The error text is part of the HTTP
//! response, so this crate reproduces gojsonschema itself rather than wrapping another JSON
//! Schema engine:
//!
//! - [`gojsonschema`]: the schema engine (drafts 4/6/7/hybrid, Go's error messages, order and
//!   scoring, `encoding/json` decoding with `UseNumber`, `math/big` number semantics, Go `regexp`
//!   syntax).

pub mod gojsonschema;
