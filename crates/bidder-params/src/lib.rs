//! JSON Schema validation of bidder params with gojsonschema-compatible error text.
//!
//! Prebid Server validates every `imp[i].ext.prebid.bidder.<bidder>` object against the
//! bidder's JSON schema in `static/bidder-params/` (Go: `openrtb_ext.NewBidderParamsValidator`,
//! backed by `github.com/xeipuuv/gojsonschema` v1.2.0). The error text is part of the HTTP
//! response, so this crate reproduces gojsonschema itself rather than wrapping another JSON
//! Schema engine:
//!
//! - [`bidders`]: the `BidderParamValidator` (loading, aliases, `Validate`, `Schema`);
//! - [`gojsonschema`]: the schema engine (drafts 4/6/7/hybrid, Go's error messages, order and
//!   scoring, `encoding/json` decoding with `UseNumber`, `math/big` number semantics, Go `regexp`
//!   syntax).
//!
//! ```no_run
//! use std::collections::{BTreeMap, BTreeSet};
//!
//! use pbs_bidder_params::{BidderParamValidator, new_bidder_params_validator};
//!
//! let bidders: BTreeSet<String> = ["appnexus".to_owned()].into();
//! let validator =
//!     new_bidder_params_validator("static/bidder-params", &bidders, &BTreeMap::new())?;
//! match validator.validate("appnexus", br#"{"placementId": 12345}"#) {
//!     Ok(()) => {}
//!     // Go: "request.imp[%d].ext.prebid.bidder.%s failed validation.\n%v"
//!     Err(e) => println!("request.imp[0].ext.prebid.bidder.appnexus failed validation.\n{e}"),
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod bidders;
pub mod gojsonschema;

pub use self::bidders::{
    BidderParamValidator, LoadError, ParamsError, StandardBidderParamValidator,
    new_bidder_params_validator,
};
