//! The serde code typegen generates for the types whose bridge module reads and writes them:
//! `json.Number`, `jsonutil.StringInt` and `jsonutil.IntString`.
//!
//! No `OpenRTB` type holds one; the `openrtb_ext` bidder params do (appnexus, rubicon,
//! freewheelssp, targetvideo, openx and a dozen more), and adapters read them with
//! `jsonutil.Unmarshal` (appnexus.go:262, rubicon.go:559, freewheelssp.go:31). [`Params`] is what
//! typegen generates for a struct of such fields (pbs-devtools'
//! `maps_numbers_and_lenient_ints_through_the_bridge` checks that it is, byte for byte); the
//! outcomes below are Go 1.26.2's for the same struct (`jsonutil.Unmarshal`, `MergeClone` and
//! `Marshal`), and the `GoJson` derive's.

use pbs_gojson::{GoJson, JsonNumber, jsonutil};
use pbs_jsonutil::{MergeClone, MergeCloneError, marshal, merge_clone, merge_struct, unmarshal};
use pbs_openrtb::gojson_bridge;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::support::{TestResult, ensure_eq};

/// Go type `p.Params`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, GoJson)]
#[serde(default)]
#[gojson(go_type = "p.Params")]
pub struct Params {
    /// Go field `PlacementId`.
    #[serde(rename = "placement_id", with = "gojson_bridge::string_int")]
    #[gojson(go = "PlacementId", json = "placement_id")]
    pub placement_id: jsonutil::StringInt,
    /// Go field `Member`.
    #[serde(rename = "member", with = "gojson_bridge::int_string")]
    #[gojson(go = "Member", json = "member")]
    pub member: jsonutil::IntString,
    /// Go field `AccountId`.
    #[serde(rename = "accountId", with = "gojson_bridge::json_number")]
    #[gojson(go = "AccountId", json = "accountId")]
    pub account_id: JsonNumber,
    /// Go field `PlayerHeight`.
    #[serde(
        rename = "playerHeight",
        skip_serializing_if = "gojson_bridge::json_number::is_empty",
        with = "gojson_bridge::json_number"
    )]
    #[gojson(go = "PlayerHeight", json = "playerHeight,omitempty")]
    pub player_height: JsonNumber,
    /// Go field `ZoneId`.
    #[serde(
        rename = "zoneId",
        skip_serializing_if = "gojson_bridge::string_int::is_empty",
        with = "gojson_bridge::string_int"
    )]
    #[gojson(go = "ZoneId", json = "zoneId,omitempty")]
    pub zone_id: jsonutil::StringInt,
    /// Go field `Floor`.
    #[serde(
        rename = "floor",
        skip_serializing_if = "Option::is_none",
        with = "gojson_bridge::bridged"
    )]
    #[gojson(go = "Floor", json = "floor,omitempty")]
    pub floor: Option<JsonNumber>,
}

impl MergeClone for Params {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        const GO_TYPE: &str = "p.Params";
        merge_struct(patch, |key, value| match key {
            "placement_id" => {
                gojson_bridge::merge_field(&mut self.placement_id, value, GO_TYPE, "PlacementId")
            }
            "member" => gojson_bridge::merge_field(&mut self.member, value, GO_TYPE, "Member"),
            "accountId" => {
                gojson_bridge::merge_field(&mut self.account_id, value, GO_TYPE, "AccountId")
            }
            "playerHeight" => {
                gojson_bridge::merge_field(&mut self.player_height, value, GO_TYPE, "PlayerHeight")
            }
            "zoneId" => gojson_bridge::merge_field(&mut self.zone_id, value, GO_TYPE, "ZoneId"),
            "floor" => gojson_bridge::merge_field(&mut self.floor, value, GO_TYPE, "Floor"),
            _ => Ok(()),
        })
    }
}

/// appnexus, rubicon and freewheelssp params, numbers given as strings and strings as numbers.
const PARAMS: &str = concat!(
    r#"{"placement_id":"123","member":958,"accountId":1001,"#,
    r#""playerHeight":"360","zoneId":"7","floor":"1.5"}"#,
);

/// A patch that empties, keeps and replaces fields.
const PATCH: &str =
    r#"{"member":"x","accountId":"42","floor":null,"zoneId":"","playerHeight":null}"#;

/// `params` as `jsonutil.Marshal` writes it.
fn written(params: &Params) -> TestResult<String> {
    let bytes = marshal(params).map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

#[test]
fn bidder_params_read_merge_and_write_as_go() -> TestResult {
    let params: Params = unmarshal(PARAMS.as_bytes()).map_err(|e| e.to_string())?;
    ensure_eq(
        written(&params)?.as_str(),
        concat!(
            r#"{"placement_id":123,"member":"958","accountId":1001,"#,
            r#""playerHeight":360,"zoneId":7,"floor":1.5}"#,
        ),
        "params written back",
    )?;
    let mut gojson = Params::default();
    jsonutil::unmarshal(PARAMS.as_bytes(), &mut gojson).map_err(|e| e.to_string())?;
    ensure_eq(&gojson, &params, "params read by the GoJson code")?;
    let empty: Params = unmarshal(b"{}").map_err(|e| e.to_string())?;
    ensure_eq(
        written(&empty)?.as_str(),
        r#"{"placement_id":0,"member":"","accountId":0}"#,
        "zero values",
    )?;
    let mut merged = params;
    merge_clone(&mut merged, PATCH.as_bytes()).map_err(|e| e.to_string())?;
    ensure_eq(
        written(&merged)?.as_str(),
        r#"{"placement_id":123,"member":"x","accountId":42,"zoneId":7}"#,
        "merged",
    )?;
    jsonutil::merge_clone(&mut gojson, PATCH.as_bytes()).map_err(|e| e.to_string())?;
    ensure_eq(&gojson, &merged, "merged by the GoJson code")?;
    // Go: `cannot unmarshal main.Params.PlacementId: Value looks like Number/Boolean/None, but
    // can't find its end: ',' or '}' symbol` and `cannot unmarshal main.Params.Member: invalid
    // type` (`json.error-texts`).
    let failed = [r#"{"placement_id":true}"#, r#"{"member":null}"#]
        .map(|doc| unmarshal::<Params>(doc.as_bytes()).is_err());
    ensure_eq(&failed, &[true, true], "params Go fails")
}
