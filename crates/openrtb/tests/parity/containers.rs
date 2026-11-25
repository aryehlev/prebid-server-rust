//! The serde code typegen generates for containers that hold bridged types, against Go's shapes of
//! the same fields (`testdata/golden/containers.jsonl`, written by `testdata/go/containers.go`).
//!
//! No `OpenRTB` type holds a Go `int`, `uint` or `json.RawMessage` inside a pointer, slice or map;
//! the `openrtb_ext` types do (`ExtImpPrebid.Bidder`, `ExtBidResponse.ResponseTimeMillis`,
//! `ExtRequestTargeting.DurationRangeSec`, `ImpExtConcert.Sizes`), and their generated code reads,
//! writes and merges them through `gojson_bridge`. [`Containers`] is what typegen generates for
//! a struct of one field of each shape (pbs-devtools' `maps_containers_of_bridged_types` checks
//! that it is, byte for byte); its cases are replayed as `shapes` replays the ported types'.

use std::collections::BTreeMap;

use pbs_gojson::{GoJson, RawJson, gotype::go_type_name};
use pbs_jsonutil::{MergeClone, MergeCloneError, is_nil_or_empty, merge_struct};
use pbs_openrtb::{gojson_bridge, openrtb2};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::{
    shapes::{
        Case, ELEMENT_CONTEXT, ERROR_TEXTS, FAILED_MERGE_ELEMENTS, FAILED_MERGE_SLICE, Merged,
        NIL_RAW_WRITTEN, Outcome, Runner, cases_in, compare, emptied_slices_as_nil,
        ensure_explained, go_merge_error, run, runner,
    },
    support::{TestResult, ensure_eq},
};

/// Go type `p.Containers`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, GoJson)]
#[serde(default)]
#[gojson(go_type = "p.Containers")]
pub struct Containers {
    /// Go field `Bidder`.
    #[serde(
        rename = "bidder",
        skip_serializing_if = "is_nil_or_empty",
        with = "gojson_bridge::bridged"
    )]
    #[gojson(go = "Bidder", json = "bidder,omitempty")]
    pub bidder: Option<BTreeMap<String, RawJson>>,
    /// Go field `ResponseTimeMillis`.
    #[serde(rename = "responsetimemillis", skip_serializing_if = "is_nil_or_empty")]
    #[gojson(go = "ResponseTimeMillis", json = "responsetimemillis,omitempty")]
    pub responsetimemillis: Option<BTreeMap<String, isize>>,
    /// Go field `DurationRangeSec`.
    #[serde(rename = "durationrangesec")]
    #[gojson(go = "DurationRangeSec", json = "durationrangesec")]
    pub durationrangesec: Option<Vec<isize>>,
    /// Go field `Sizes`.
    #[serde(rename = "sizes", skip_serializing_if = "Option::is_none")]
    #[gojson(go = "Sizes", json = "sizes,omitempty")]
    pub sizes: Option<Option<Vec<Option<Vec<isize>>>>>,
    /// Go field `Raws`.
    #[serde(
        rename = "raws",
        skip_serializing_if = "is_nil_or_empty",
        with = "gojson_bridge::bridged"
    )]
    #[gojson(go = "Raws", json = "raws,omitempty")]
    pub raws: Option<Vec<RawJson>>,
    /// Go field `RawPtr`.
    #[serde(
        rename = "rawptr",
        skip_serializing_if = "Option::is_none",
        with = "gojson_bridge::bridged"
    )]
    #[gojson(go = "RawPtr", json = "rawptr,omitempty")]
    pub rawptr: Option<RawJson>,
    /// Go field `Counts`.
    #[serde(rename = "counts", skip_serializing_if = "is_nil_or_empty")]
    #[gojson(go = "Counts", json = "counts,omitempty")]
    pub counts: Option<Vec<Option<isize>>>,
    /// Go field `Wide`.
    #[serde(rename = "wide", skip_serializing_if = "is_nil_or_empty")]
    #[gojson(go = "Wide", json = "wide,omitempty")]
    pub wide: Option<BTreeMap<String, usize>>,
}

impl MergeClone for Containers {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        const GO_TYPE: &str = "p.Containers";
        merge_struct(patch, |key, value| match key {
            "bidder" => gojson_bridge::merge_field(&mut self.bidder, value, GO_TYPE, "Bidder"),
            "responsetimemillis" => gojson_bridge::merge_field(
                &mut self.responsetimemillis,
                value,
                GO_TYPE,
                "ResponseTimeMillis",
            ),
            "durationrangesec" => gojson_bridge::merge_field(
                &mut self.durationrangesec,
                value,
                GO_TYPE,
                "DurationRangeSec",
            ),
            "sizes" => gojson_bridge::merge_field(&mut self.sizes, value, GO_TYPE, "Sizes"),
            "raws" => gojson_bridge::merge_field(&mut self.raws, value, GO_TYPE, "Raws"),
            "rawptr" => gojson_bridge::merge_field(&mut self.rawptr, value, GO_TYPE, "RawPtr"),
            "counts" => gojson_bridge::merge_field(&mut self.counts, value, GO_TYPE, "Counts"),
            "wide" => gojson_bridge::merge_field(&mut self.wide, value, GO_TYPE, "Wide"),
            _ => Ok(()),
        })
    }
}

/// The runners of the golden's types: [`Containers`] and `openrtb2.BidRequest`, which its merges
/// that fail inside a slice of an `OpenRTB` type go into.
fn runners() -> BTreeMap<String, Runner> {
    BTreeMap::from([
        ("p.Containers".to_owned(), runner::<Containers>()),
        (
            go_type_name::<openrtb2::BidRequest>(),
            runner::<openrtb2::BidRequest>(),
        ),
    ])
}

/// Every case of the golden matches Go, or differs in the ledger entries listed, each of which
/// some case shows.
#[test]
fn containers_match_go() -> TestResult {
    let cases = cases_in("containers.jsonl")?;
    let (classes, unexplained) = run(&runners(), &cases);
    ensure_explained(&unexplained)?;
    ensure_eq(
        &classes.keys().copied().collect::<Vec<_>>(),
        &vec![
            ELEMENT_CONTEXT,
            ERROR_TEXTS,
            FAILED_MERGE_ELEMENTS,
            FAILED_MERGE_SLICE,
            NIL_RAW_WRITTEN,
        ],
        &format!("the ledger entries the differences fall in ({classes:?})"),
    )
}

/// A merge case of the golden with the port's merge and Go's error and merged value.
struct MergeRun {
    /// The type and label.
    name: String,
    /// The port's merge.
    port: Merged,
    /// Go's error text, empty for none.
    err: String,
    /// Go's merged value.
    go: Outcome,
}

/// Every merge case of the golden, merged by the port.
fn merges() -> TestResult<Vec<MergeRun>> {
    let runners = runners();
    let mut out = Vec::new();
    for case in cases_in("containers.jsonl")? {
        let Case::Merge {
            ty,
            label,
            doc,
            patch,
            err,
            merged,
        } = case
        else {
            continue;
        };
        let runner = runners
            .get(&ty)
            .ok_or_else(|| format!("no Rust type for {ty}"))?;
        out.push(MergeRun {
            name: format!("{ty} {label}"),
            port: (runner.merge)(&doc, &patch)?,
            err,
            go: merged,
        });
    }
    Ok(out)
}

/// `json.element-merge-context`: a merge that fails in an element of a slice, or on an inner
/// slice, names the field (`openrtb2.BidRequest.BCat`), where Go names the slice's type
/// (`[]string`); a struct inside a slice names its own field on both sides
/// (`openrtb2.Format.W`).
#[test]
fn element_merge_errors_name_the_field() -> TestResult {
    let mut found = Vec::new();
    for run in merges()? {
        let (Some(go), _) = go_merge_error(&run.err) else {
            continue;
        };
        let port = run.port.error.as_ref().and_then(MergeCloneError::context);
        if go.starts_with("[]") || run.name.ends_with("format wrong field") {
            found.push((run.name, port.map(str::to_owned), go.to_owned()));
        }
    }
    let want = [
        (
            "p.Containers durationrangesec wrong element",
            "p.Containers.DurationRangeSec",
            "[]int",
        ),
        (
            "p.Containers sizes wrong inner",
            "p.Containers.Sizes",
            "[][]int",
        ),
        (
            "openrtb2.BidRequest bcat wrong element",
            "openrtb2.BidRequest.BCat",
            "[]string",
        ),
        (
            "openrtb2.BidRequest imp iframebuster wrong element",
            "openrtb2.Imp.IframeBuster",
            "[]string",
        ),
        (
            "openrtb2.BidRequest imp banner format wrong field",
            "openrtb2.Format.W",
            "openrtb2.Format.W",
        ),
        (
            "openrtb2.BidRequest imp banner format wrong element",
            "openrtb2.Banner.Format",
            "[]openrtb2.Format",
        ),
    ]
    .map(|(name, port, go)| (name.to_owned(), Some(port.to_owned()), go.to_owned()));
    ensure_eq(
        &found,
        &want.to_vec(),
        "the fields slice element errors name",
    )
}

/// `json.failed-merge-elements`: after a merge that fails in a slice element or a map value, the
/// field holds what merged before the failure; Go's holds the element or entry that failed too,
/// with what it read (a zero value), and for a slice of strings the elements after it. (A merge
/// that fails on a whole slice is `json.failed-merge-empty-slice`.)
#[test]
fn failed_merges_keep_what_merged_before() -> TestResult {
    let mut found = Vec::new();
    for run in merges()? {
        let emptied = emptied_slices_as_nil(&run.port).map(|nil| compare("", &nil, &run.go));
        if run.port.error.is_none()
            || compare("", &run.port.encoded, &run.go).is_ok()
            || emptied.is_some_and(|result| result.is_ok())
        {
            continue;
        }
        let (Outcome::Encoded(port), Outcome::Encoded(go)) = (&run.port.encoded, &run.go) else {
            return Err(format!("{}: {:?}, Go {:?}", run.name, run.port, run.go));
        };
        found.push((run.name, port.clone(), go.clone()));
    }
    let want = [
        (
            "p.Containers responsetimemillis wrong",
            r#"{"durationrangesec":null}"#,
            r#"{"responsetimemillis":{"a":0},"durationrangesec":null}"#,
        ),
        (
            "p.Containers wide wrong",
            r#"{"durationrangesec":null}"#,
            r#"{"durationrangesec":null,"wide":{"a":0}}"#,
        ),
        (
            "p.Containers responsetimemillis wrong value",
            r#"{"responsetimemillis":{"a":1},"durationrangesec":null}"#,
            r#"{"responsetimemillis":{"a":1,"b":0},"durationrangesec":null}"#,
        ),
        (
            "p.Containers durationrangesec wrong element",
            r#"{"durationrangesec":[4]}"#,
            r#"{"durationrangesec":[4,0]}"#,
        ),
        (
            "p.Containers sizes wrong inner",
            r#"{"durationrangesec":null,"sizes":[[2]]}"#,
            r#"{"durationrangesec":null,"sizes":[[2],null]}"#,
        ),
        (
            "p.Containers wide wrong value",
            r#"{"durationrangesec":null}"#,
            r#"{"durationrangesec":null,"wide":{"a":0}}"#,
        ),
        (
            "openrtb2.BidRequest bcat wrong element",
            r#"{"id":"","imp":null,"bcat":["a"]}"#,
            r#"{"id":"","imp":null,"bcat":["a","","b"]}"#,
        ),
        (
            "openrtb2.BidRequest imp iframebuster wrong element",
            r#"{"id":"","imp":[]}"#,
            r#"{"id":"","imp":[{"id":"","iframebuster":[""]}]}"#,
        ),
        (
            "openrtb2.BidRequest imp banner format wrong field",
            r#"{"id":"","imp":[]}"#,
            r#"{"id":"","imp":[{"id":"","banner":{"format":[{}]}}]}"#,
        ),
        (
            "openrtb2.BidRequest imp banner format wrong element",
            r#"{"id":"","imp":[]}"#,
            r#"{"id":"","imp":[{"id":"","banner":{"format":[{}]}}]}"#,
        ),
    ]
    .map(|(name, port, go)| (name.to_owned(), port.to_owned(), go.to_owned()));
    ensure_eq(
        &found,
        &want.to_vec(),
        "the merges that fail inside a container",
    )
}
