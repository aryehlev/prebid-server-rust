//! JSON Schema validation of bidder params with gojsonschema-compatible error text.
//!
//! Prebid Server validates every `imp[i].ext.prebid.bidder.<bidder>` object against the
//! bidder's JSON schema in `static/bidder-params/` (Go: `openrtb_ext.NewBidderParamsValidator`,
//! backed by `github.com/xeipuuv/gojsonschema` v1.2.0). The error text is part of the HTTP
//! response, and no JSON Schema crate passes the workspace's gates (D-JSONSCHEMA), so this crate
//! validates the part of JSON Schema Prebid's schemas use with gojsonschema's texts:
//!
//! - [`bidders`]: the `BidderParamValidator` (loading, aliases, `Validate`, `Schema`);
//! - [`gojsonschema`]: the keywords and formats the 282 bidder schemas and
//!   `rules-engine-schema.json` use, on `serde_json` and the regex crate (patterns through
//!   `pbs_util::goregex`), with Go's error messages, order and scoring. A schema using any other
//!   keyword fails to load.
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
