//! Field types of generated bidder params that Go declares inline or unexported: the
//! unnamed struct types of `ImpExtAdnunitus.Targeting` (`imp_adnuntius.go`) and
//! `ExtImpAdUnitCode.Prebid` (`imp_gumgum.go`), and Rubicon's unexported `rubiconVideoParams`
//! and `impExtRubiconDebug` (`imp_rubicon.go`).

use std::collections::BTreeMap;

use pbs_gojson::{GoJson, JsonNumber};

/// The unnamed struct type of Go's `ImpExtAdnunitus.Targeting`.
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(
    go_type = "struct { Category []string \"json:\\\"c,omitempty\\\"\"; \
               Segments []string \"json:\\\"segments,omitempty\\\"\"; \
               Keywords []string \"json:\\\"keywords,omitempty\\\"\"; \
               KeyValues map[string][]string \"json:\\\"kv,omitempty\\\"\"; \
               AdUnitMatchingLabel []string \"json:\\\"auml,omitempty\\\"\" }",
    anonymous
)]
pub struct ImpExtAdnunitusTargeting {
    /// Go field `Category`.
    #[gojson(go = "Category", json = "c,omitempty")]
    pub category: Option<Vec<String>>,
    /// Go field `Segments`.
    #[gojson(go = "Segments", json = "segments,omitempty")]
    pub segments: Option<Vec<String>>,
    /// Go field `Keywords`.
    #[gojson(go = "Keywords", json = "keywords,omitempty")]
    pub keywords: Option<Vec<String>>,
    /// Go field `KeyValues`.
    #[gojson(go = "KeyValues", json = "kv,omitempty")]
    pub key_values: Option<BTreeMap<String, Option<Vec<String>>>>,
    /// Go field `AdUnitMatchingLabel`.
    #[gojson(go = "AdUnitMatchingLabel", json = "auml,omitempty")]
    pub ad_unit_matching_label: Option<Vec<String>>,
}

/// The unnamed struct type of Go's `ExtImpAdUnitCode.Prebid`.
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(
    go_type = "struct { AdUnitCode string \"json:\\\"adunitcode,omitempty\\\"\" }",
    anonymous
)]
pub struct ExtImpAdUnitCodePrebid {
    /// Go field `AdUnitCode`.
    #[gojson(go = "AdUnitCode", json = "adunitcode,omitempty")]
    pub ad_unit_code: String,
}

/// Go: `rubiconVideoParams` (unexported) defines the contract for
/// bidrequest.imp[i].ext.prebid.bidder.rubicon.video
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "openrtb_ext.rubiconVideoParams")]
pub struct RubiconVideoParams {
    /// Go field `Language`.
    #[gojson(go = "Language", json = "language,omitempty")]
    pub language: String,
    /// Go field `PlayerHeight`.
    #[gojson(go = "PlayerHeight", json = "playerHeight,omitempty")]
    pub player_height: JsonNumber,
    /// Go field `PlayerWidth`.
    #[gojson(go = "PlayerWidth", json = "playerWidth,omitempty")]
    pub player_width: JsonNumber,
    /// Go field `VideoSizeID`.
    #[gojson(go = "VideoSizeID", json = "size_id,omitempty")]
    pub video_size_id: isize,
    /// Go field `Skip`.
    #[gojson(go = "Skip", json = "skip,omitempty")]
    pub skip: isize,
    /// Go field `SkipDelay`.
    #[gojson(go = "SkipDelay", json = "skipdelay,omitempty")]
    pub skip_delay: isize,
}

/// Go: `impExtRubiconDebug` (unexported) defines the contract for
/// bidrequest.imp[i].ext.prebid.bidder.rubicon.debug
#[derive(Clone, Copy, Debug, Default, PartialEq, GoJson)]
#[gojson(go_type = "openrtb_ext.impExtRubiconDebug")]
pub struct ImpExtRubiconDebug {
    /// Go field `CpmOverride`.
    #[gojson(go = "CpmOverride", json = "cpmoverride,omitempty")]
    pub cpm_override: f64,
}
