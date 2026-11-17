//! Go: `openrtb_ext/request.go`: price granularity with its `UnmarshalJSON`, `ExtMultiBid`'s
//! `String` and `ExtRequestPrebid`'s `Clone` (the structs are generated).

use std::collections::BTreeMap;

use pbs_gojson::{GoJson, GoUnmarshaler, JsonError, jsonutil};

use crate::{
    generated::{Adjustment, ExtMultiBid, ExtRequestPrebid, GranularityRange, PriceGranularityRaw},
    named_map::named_map,
};

/// `AdjustmentsByDealID` maps a dealID to a slice of bid adjustments
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdjustmentsByDealID(pub Option<BTreeMap<String, Option<Vec<Adjustment>>>>);

named_map!(
    AdjustmentsByDealID,
    "openrtb_ext.AdjustmentsByDealID",
    String,
    Option<Vec<Adjustment>>
);

/// `PriceGranularity` defines the allowed values for
/// bidrequest.ext.prebid.targeting.pricegranularity or
/// bidrequest.ext.prebid.targeting.mediatypepricegranularity.banner|video|native
#[derive(Clone, Debug, Default, PartialEq, GoJson)]
#[gojson(go_type = "openrtb_ext.PriceGranularity", unmarshaler)]
pub struct PriceGranularity {
    /// Go field `Precision`.
    #[gojson(go = "Precision", json = "precision,omitempty")]
    pub precision: Option<isize>,
    /// Go field `Ranges`.
    #[gojson(go = "Ranges", json = "ranges,omitempty")]
    pub ranges: Option<Vec<GranularityRange>>,
}

impl From<PriceGranularityRaw> for PriceGranularity {
    /// Go: `(PriceGranularity)(modernValue)`.
    fn from(raw: PriceGranularityRaw) -> Self {
        Self {
            precision: raw.precision,
            ranges: raw.ranges,
        }
    }
}

impl GoUnmarshaler for PriceGranularity {
    /// Go: `(*PriceGranularity).UnmarshalJSON`: a legacy string ID (`"medium"`, ...) when the
    /// value decodes as one, otherwise the custom model decoded with `jsonutil.Unmarshal`, which
    /// replaces the whole value on success.
    fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), JsonError> {
        // Go: openrtb_ext/request.go:240
        let mut legacy_id = String::new();
        if jsonutil::unmarshal(data, &mut legacy_id).is_ok()
            && let Some(legacy_value) = new_price_granularity_from_legacy_id(&legacy_id)
        {
            *self = legacy_value;
            return Ok(());
        }
        let mut modern_value = PriceGranularityRaw::default();
        match jsonutil::unmarshal(data, &mut modern_value) {
            Ok(()) => {
                *self = modern_value.into();
                Ok(())
            }
            Err(err) => Err(JsonError::msg(err.message.into_bytes())),
        }
    }
}

/// Go: `NewPriceGranularityDefault()`: the `medium` legacy granularity.
#[must_use]
pub fn new_price_granularity_default() -> PriceGranularity {
    new_price_granularity_from_legacy_id("medium").unwrap_or_default()
}

/// One range of a legacy granularity.
const fn range(min: f64, max: f64, increment: f64) -> GranularityRange {
    GranularityRange {
        min,
        max,
        increment,
    }
}

/// Go: `NewPriceGranularityFromLegacyID(v)`: the granularity a legacy string ID (`low`, `med`,
/// `medium`, `high`, `auto`, `dense`) stands for; `None` is Go's `(PriceGranularity{}, false)`.
#[must_use]
pub fn new_price_granularity_from_legacy_id(v: &str) -> Option<PriceGranularity> {
    // Go: openrtb_ext/request.go:266
    let ranges = match v {
        "low" => vec![range(0.0, 5.0, 0.5)],
        "med" | "medium" => vec![range(0.0, 20.0, 0.1)],
        "high" => vec![range(0.0, 20.0, 0.01)],
        "auto" => vec![
            range(0.0, 5.0, 0.05),
            range(5.0, 10.0, 0.1),
            range(10.0, 20.0, 0.5),
        ],
        "dense" => vec![
            range(0.0, 3.0, 0.01),
            range(3.0, 8.0, 0.05),
            range(8.0, 20.0, 0.5),
        ],
        _ => return None,
    };
    Some(PriceGranularity {
        precision: Some(2),
        ranges: Some(ranges),
    })
}

impl ExtMultiBid {
    /// Go: `ExtMultiBid.String()`:
    /// `{Bidder:%s, Bidders:%v, MaxBids:%s, TargetBidderCodePrefix:%s}`, `MaxBids` printed as
    /// `<nil>` when absent. `fmt` uses it wherever Go formats an `ExtMultiBid` with `%v`.
    #[must_use]
    pub fn string(&self) -> String {
        // Go: openrtb_ext/request.go:378
        let max_bid = self
            .max_bids
            .map_or_else(|| "<nil>".to_owned(), |max_bids| max_bids.to_string());
        // `%v` of a []string: the elements between brackets, separated by spaces.
        let bidders = self.bidders.as_deref().unwrap_or_default().join(" ");
        format!(
            "{{Bidder:{}, Bidders:[{bidders}], MaxBids:{max_bid}, TargetBidderCodePrefix:{}}}",
            self.bidder, self.target_bidder_code_prefix
        )
    }
}

impl ExtRequestPrebid {
    /// Go: `(*ExtRequestPrebid).Clone()`, with what Go's copy leaves out or changes:
    ///
    /// - `targeting` keeps only the fields Go copies, so `mediatypepricegranularity` and
    ///   `alwaysincludedeals` are dropped;
    /// - `currency.rates` becomes an empty map when it was nil (it encodes as `{}`, not `null`).
    ///
    /// Everything else is copied. Go shares the maps and pointers it does not copy explicitly
    /// (`sdk`, `macros`, `biddercontrols`, ...) between the two values; Rust copies them, which
    /// only differs when one side is later changed in place. Go panics on a nil `multibid` or
    /// `schains` element; here it stays nil.
    #[must_use]
    pub fn clone_go(&self) -> Self {
        // Go: openrtb_ext/request.go:386
        let mut clone = self.clone();
        if let Some(conversions) = clone.currency_conversions.as_deref_mut()
            && conversions.conversion_rates.is_none()
        {
            conversions.conversion_rates = Some(BTreeMap::new());
        }
        if let Some(targeting) = clone.targeting.as_deref_mut() {
            targeting.media_type_price_granularity = None;
            targeting.always_include_deals = false;
        }
        // The other structs Go rebuilds field by field (cache, bidder configs, data, experiment,
        // multibid, alternate bidder codes, floors) get every field they have.
        clone
    }
}
