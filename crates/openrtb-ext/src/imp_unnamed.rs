//! Field types of generated bidder params that Go declares inline or unexported: the
//! unnamed struct types of `ImpExtAdnunitus.Targeting` (`imp_adnuntius.go`) and
//! `ExtImpAdUnitCode.Prebid` (`imp_gumgum.go`), and Rubicon's unexported `rubiconVideoParams`
//! and `impExtRubiconDebug` (`imp_rubicon.go`).
//!
//! Their serde code is what typegen writes for a generated struct: `#[serde(default)]`, each
//! member under its JSON key with Go's `omitempty` test, null read into a value field as its zero
//! value, `json.Number` and Go `int` through `pbs_openrtb::gojson_bridge`, and a `MergeClone`
//! that merges member by member.

use std::collections::BTreeMap;

use pbs_gojson::{GoJson, JsonNumber};
use pbs_jsonutil::{
    MergeClone, MergeCloneError, is_nil_or_empty, is_zero, merge_field, merge_struct, null_default,
};
use pbs_openrtb::gojson_bridge;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// The unnamed struct type of Go's `ImpExtAdnunitus.Targeting`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, GoJson)]
#[serde(default)]
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
    #[serde(rename = "c", skip_serializing_if = "is_nil_or_empty")]
    #[gojson(go = "Category", json = "c,omitempty")]
    pub category: Option<Vec<String>>,
    /// Go field `Segments`.
    #[serde(rename = "segments", skip_serializing_if = "is_nil_or_empty")]
    #[gojson(go = "Segments", json = "segments,omitempty")]
    pub segments: Option<Vec<String>>,
    /// Go field `Keywords`.
    #[serde(rename = "keywords", skip_serializing_if = "is_nil_or_empty")]
    #[gojson(go = "Keywords", json = "keywords,omitempty")]
    pub keywords: Option<Vec<String>>,
    /// Go field `KeyValues`.
    #[serde(rename = "kv", skip_serializing_if = "is_nil_or_empty")]
    #[gojson(go = "KeyValues", json = "kv,omitempty")]
    pub key_values: Option<BTreeMap<String, Option<Vec<String>>>>,
    /// Go field `AdUnitMatchingLabel`.
    #[serde(rename = "auml", skip_serializing_if = "is_nil_or_empty")]
    #[gojson(go = "AdUnitMatchingLabel", json = "auml,omitempty")]
    pub ad_unit_matching_label: Option<Vec<String>>,
}

/// The unnamed struct type of Go's `ExtImpAdUnitCode.Prebid`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, GoJson)]
#[serde(default)]
#[gojson(
    go_type = "struct { AdUnitCode string \"json:\\\"adunitcode,omitempty\\\"\" }",
    anonymous
)]
pub struct ExtImpAdUnitCodePrebid {
    /// Go field `AdUnitCode`.
    #[serde(
        rename = "adunitcode",
        skip_serializing_if = "is_zero",
        deserialize_with = "null_default"
    )]
    #[gojson(go = "AdUnitCode", json = "adunitcode,omitempty")]
    pub ad_unit_code: String,
}

/// Go: `rubiconVideoParams` (unexported) defines the contract for
/// `bidrequest.imp[i].ext.prebid.bidder.rubicon.video`
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, GoJson)]
#[serde(default)]
#[gojson(go_type = "openrtb_ext.rubiconVideoParams")]
pub struct RubiconVideoParams {
    /// Go field `Language`.
    #[serde(
        rename = "language",
        skip_serializing_if = "is_zero",
        deserialize_with = "null_default"
    )]
    #[gojson(go = "Language", json = "language,omitempty")]
    pub language: String,
    /// Go field `PlayerHeight`.
    #[serde(
        rename = "playerHeight",
        skip_serializing_if = "gojson_bridge::json_number::is_empty",
        with = "gojson_bridge::json_number"
    )]
    #[gojson(go = "PlayerHeight", json = "playerHeight,omitempty")]
    pub player_height: JsonNumber,
    /// Go field `PlayerWidth`.
    #[serde(
        rename = "playerWidth",
        skip_serializing_if = "gojson_bridge::json_number::is_empty",
        with = "gojson_bridge::json_number"
    )]
    #[gojson(go = "PlayerWidth", json = "playerWidth,omitempty")]
    pub player_width: JsonNumber,
    /// Go field `VideoSizeID`.
    #[serde(
        rename = "size_id",
        skip_serializing_if = "is_zero",
        deserialize_with = "null_default"
    )]
    #[gojson(go = "VideoSizeID", json = "size_id,omitempty")]
    pub video_size_id: isize,
    /// Go field `Skip`.
    #[serde(
        rename = "skip",
        skip_serializing_if = "is_zero",
        deserialize_with = "null_default"
    )]
    #[gojson(go = "Skip", json = "skip,omitempty")]
    pub skip: isize,
    /// Go field `SkipDelay`.
    #[serde(
        rename = "skipdelay",
        skip_serializing_if = "is_zero",
        deserialize_with = "null_default"
    )]
    #[gojson(go = "SkipDelay", json = "skipdelay,omitempty")]
    pub skip_delay: isize,
}

/// Go: `impExtRubiconDebug` (unexported) defines the contract for
/// `bidrequest.imp[i].ext.prebid.bidder.rubicon.debug`
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, GoJson)]
#[serde(default)]
#[gojson(go_type = "openrtb_ext.impExtRubiconDebug")]
pub struct ImpExtRubiconDebug {
    /// Go field `CpmOverride`.
    #[serde(
        rename = "cpmoverride",
        skip_serializing_if = "is_zero",
        deserialize_with = "null_default"
    )]
    #[gojson(go = "CpmOverride", json = "cpmoverride,omitempty")]
    pub cpm_override: f64,
}

/// The Go type of `ImpExtAdnunitusTargeting`, as merge errors name it.
const ADNUNITUS_TARGETING: &str = "struct { Category []string \"json:\\\"c,omitempty\\\"\"; \
                                   Segments []string \"json:\\\"segments,omitempty\\\"\"; \
                                   Keywords []string \"json:\\\"keywords,omitempty\\\"\"; \
                                   KeyValues map[string][]string \"json:\\\"kv,omitempty\\\"\"; \
                                   AdUnitMatchingLabel []string \"json:\\\"auml,omitempty\\\"\" }";

impl MergeClone for ImpExtAdnunitusTargeting {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        const GO_TYPE: &str = ADNUNITUS_TARGETING;
        merge_struct(patch, |key, value| match key {
            "c" => merge_field(&mut self.category, value, GO_TYPE, "Category"),
            "segments" => merge_field(&mut self.segments, value, GO_TYPE, "Segments"),
            "keywords" => merge_field(&mut self.keywords, value, GO_TYPE, "Keywords"),
            "kv" => merge_field(&mut self.key_values, value, GO_TYPE, "KeyValues"),
            "auml" => merge_field(
                &mut self.ad_unit_matching_label,
                value,
                GO_TYPE,
                "AdUnitMatchingLabel",
            ),
            _ => Ok(()),
        })
    }
}

impl MergeClone for ExtImpAdUnitCodePrebid {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        const GO_TYPE: &str = "struct { AdUnitCode string \"json:\\\"adunitcode,omitempty\\\"\" }";
        merge_struct(patch, |key, value| match key {
            "adunitcode" => merge_field(&mut self.ad_unit_code, value, GO_TYPE, "AdUnitCode"),
            _ => Ok(()),
        })
    }
}

impl MergeClone for RubiconVideoParams {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        const GO_TYPE: &str = "openrtb_ext.rubiconVideoParams";
        merge_struct(patch, |key, value| match key {
            "language" => merge_field(&mut self.language, value, GO_TYPE, "Language"),
            "playerHeight" => {
                gojson_bridge::merge_field(&mut self.player_height, value, GO_TYPE, "PlayerHeight")
            }
            "playerWidth" => {
                gojson_bridge::merge_field(&mut self.player_width, value, GO_TYPE, "PlayerWidth")
            }
            "size_id" => {
                gojson_bridge::merge_field(&mut self.video_size_id, value, GO_TYPE, "VideoSizeID")
            }
            "skip" => gojson_bridge::merge_field(&mut self.skip, value, GO_TYPE, "Skip"),
            "skipdelay" => {
                gojson_bridge::merge_field(&mut self.skip_delay, value, GO_TYPE, "SkipDelay")
            }
            _ => Ok(()),
        })
    }
}

impl MergeClone for ImpExtRubiconDebug {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        const GO_TYPE: &str = "openrtb_ext.impExtRubiconDebug";
        merge_struct(patch, |key, value| match key {
            "cpmoverride" => merge_field(&mut self.cpm_override, value, GO_TYPE, "CpmOverride"),
            _ => Ok(()),
        })
    }
}
