use std::collections::{BTreeMap, BTreeSet};

use pbs_gojson::{RawJson, jsoniter, stdjson};

use super::{CaptureSpec, Envelope, HeaderList, HeaderPair, MockSpec};
use crate::{
    golden::{TestResult, check, check_eq},
    ids::DecisionId,
};

fn capture(stages: &str) -> CaptureSpec {
    CaptureSpec {
        stages: RawJson::from_slice(stages.as_bytes()),
        ..CaptureSpec::default()
    }
}

fn filter_of(stages: &str) -> Result<Option<Vec<String>>, String> {
    capture(stages)
        .stage_filter()
        .map(|set| set.map(|ids| ids.into_iter().collect()))
        .map_err(|err| err.to_string())
}

// Go: parity/envelope_test.go TestStageFilter.
#[test]
fn stage_filter() -> TestResult {
    check_eq(&CaptureSpec::default().stage_filter(), &Ok(None), "omitted")?;
    check_eq(&filter_of("null")?, &None, "null")?;
    check_eq(&filter_of(r#""*""#)?, &None, "star")?;
    check_eq(
        &filter_of(r#"["http.in","made.up","http.in"]"#)?,
        &Some(vec!["http.in".to_owned(), "made.up".to_owned()]),
        "list (unregistered IDs are kept)",
    )?;
    check_eq(&filter_of("[]")?, &Some(Vec::new()), "empty list")?;
    check_eq(
        &filter_of(r#"[null,"http.in"]"#)?,
        &Some(vec![String::new(), "http.in".to_owned()]),
        "null element",
    )?;
    check_eq(
        &filter_of(r#""all""#),
        &Err(r#"capture.stages: string value must be "*", got "all""#.to_owned()),
        "other string",
    )?;
    check_eq(
        &filter_of("{}"),
        &Err(
            "capture.stages: json: cannot unmarshal object into Go value of type []parity.StageID"
                .to_owned(),
        ),
        "object",
    )?;
    check_eq(
        &filter_of("[1]"),
        &Err(
            "capture.stages: json: cannot unmarshal number into Go value of type parity.StageID"
                .to_owned(),
        ),
        "number element",
    )?;
    let ids: BTreeSet<String> = BTreeSet::from(["x".to_owned()]);
    check_eq(&capture(r#"["x"]"#).stage_filter(), &Ok(Some(ids)), "set")
}

// Go: parity/envelope_test.go TestCaptureDefaults.
#[test]
fn capture_flags_default_on() -> TestResult {
    let defaults = CaptureSpec::default();
    check(
        defaults.wants_metrics() && defaults.wants_analytics(),
        "defaults",
    )?;
    let explicit = CaptureSpec {
        metrics: Some(false),
        analytics: Some(true),
        ..CaptureSpec::default()
    };
    check(
        !explicit.wants_metrics() && explicit.wants_analytics(),
        "explicit",
    )
}

#[test]
fn mock_defaults() -> TestResult {
    let defaults = MockSpec::default();
    check_eq(&defaults.effective_mode(), &"oracle", "mode")?;
    check_eq(&defaults.effective_level(), &"http", "level")?;
    let inline = MockSpec {
        mode: "inline".to_owned(),
        level: "bidder_response".to_owned(),
        ..MockSpec::default()
    };
    check_eq(&inline.effective_mode(), &"inline", "inline")?;
    check_eq(
        &inline.effective_level(),
        &"bidder_response",
        "bidder_response",
    )
}

fn envelope_with(decisions: &[(&str, &str)]) -> Envelope {
    Envelope {
        decisions: Some(
            decisions
                .iter()
                .map(|(k, v)| ((*k).to_owned(), RawJson::from_slice(v.as_bytes())))
                .collect(),
        ),
        ..Envelope::default()
    }
}

// Go: parity/envelope_test.go TestDecisionInt and TestDecisionString.
#[test]
fn decisions() -> TestResult {
    let env = envelope_with(&[
        ("floors.skip", "1"),
        ("floors.data_rate", r#""1""#),
        ("floors.modelgroup", "2.5"),
        ("floors.enforce", "null"),
        ("usersync.shuffle", r#""identity""#),
    ]);
    check_eq(&env.decision_int(DecisionId::FloorsSkip), &Some(1), "int")?;
    check_eq(
        &env.decision_int(DecisionId::FloorsDataRate),
        &None,
        "string is no int",
    )?;
    check_eq(
        &env.decision_string(DecisionId::FloorsDataRate),
        &Some("1".to_owned()),
        "string",
    )?;
    check_eq(
        &env.decision_int(DecisionId::FloorsModelGroup),
        &None,
        "fraction",
    )?;
    check_eq(
        &env.decision_int(DecisionId::FloorsEnforce),
        &Some(0),
        "null reads as 0",
    )?;
    check_eq(
        &env.decision_string(DecisionId::FloorsEnforce),
        &Some(String::new()),
        "null reads as empty",
    )?;
    check_eq(
        &env.decision_string(DecisionId::UsersyncShuffle),
        &Some("identity".to_owned()),
        "enum",
    )?;
    check_eq(
        &env.decision_int(DecisionId::CategoryDedupe),
        &None,
        "absent",
    )?;
    check_eq(
        &env.decision(DecisionId::FloorsModelGroup)
            .map(|raw| raw.as_bytes().to_vec()),
        &Some(b"2.5".to_vec()),
        "raw",
    )?;
    check_eq(
        &Envelope::default().decision_int(DecisionId::FloorsSkip),
        &None,
        "no decisions",
    )
}

// Go: parity/envelope_test.go TestHeaderListValues.
#[test]
fn header_values_fold_case() -> TestResult {
    let list = HeaderList::from_pairs(vec![
        HeaderPair::new("Content-Type", "a"),
        HeaderPair::new("X", "b"),
        HeaderPair::new("content-type", "c"),
    ]);
    check_eq(&list.values("CONTENT-TYPE"), &vec!["a", "c"], "folded")?;
    check_eq(&list.values("missing"), &Vec::<&str>::new(), "missing")?;
    check_eq(&HeaderList::default().pairs(), &[].as_slice(), "nil")?;
    let encoded = stdjson::marshal(&list).map_err(|e| e.to_string())?;
    check_eq(
        encoded.as_slice(),
        br#"[["Content-Type","a"],["X","b"],["content-type","c"]]"#.as_slice(),
        "encoding",
    )?;
    let nil = stdjson::marshal(&HeaderList::default()).map_err(|e| e.to_string())?;
    check_eq(nil.as_slice(), b"null".as_slice(), "nil encoding")
}

fn std_pairs(input: &str) -> Result<Vec<HeaderPair>, String> {
    let mut list = HeaderList::default();
    stdjson::unmarshal(input.as_bytes(), &mut list).map_err(|e| e.to_string())?;
    Ok(list.pairs().to_vec())
}

fn jsoniter_pairs(input: &str) -> Result<Vec<HeaderPair>, String> {
    let mut list = HeaderList::default();
    jsoniter::unmarshal(input.as_bytes(), &mut list).map_err(|e| e.to_string())?;
    Ok(list.pairs().to_vec())
}

// Pairs decode like Go's [2]string: short arrays are zero-filled, extra elements dropped.
#[test]
fn header_pairs_decode_like_go_arrays() -> TestResult {
    let input = r#"[["a","1"],["only"],["x","y","z"],[],null,["k",null]]"#;
    let want = vec![
        HeaderPair::new("a", "1"),
        HeaderPair::new("only", ""),
        HeaderPair::new("x", "y"),
        HeaderPair::default(),
        HeaderPair::default(),
        HeaderPair::new("k", ""),
    ];
    check_eq(&std_pairs(input)?, &want, "encoding/json")?;
    check_eq(&jsoniter_pairs(input)?, &want, "json-iterator")?;
    check_eq(
        &std_pairs(r#"[{"a":1}]"#),
        &Err("json: cannot unmarshal object into Go value of type [2]string".to_owned()),
        "object pair",
    )?;
    check_eq(
        &std_pairs(r#"["ab"]"#),
        &Err("json: cannot unmarshal string into Go value of type [2]string".to_owned()),
        "string pair",
    )?;
    check_eq(
        &std_pairs("{}"),
        &Err("json: cannot unmarshal object into Go value of type parity.HeaderList".to_owned()),
        "object list",
    )?;
    check(
        jsoniter_pairs(r#"[{"a":1}]"#).is_err(),
        "jsoniter object pair",
    )
}

/// An envelope in the encoding Go writes (members in struct order, empty members omitted).
const ROUND_TRIP: &str = concat!(
    r#"{"schema":"pbs-parity-envelope/1","schema_version":"1.0.0","case_id":"c","#,
    r#""request":{"method":"POST","path":"/openrtb2/auction","query":"","#,
    r#""headers":[["A","b"]],"body":{"json":{"id":"1"}}},"mock":{"mode":"inline"},"#,
    r#""decisions":{"floors.skip":1},"fakes":{"int63":3},"#,
    r#""capture":{"stages":["http.in"],"metrics":false}}"#,
);

#[test]
fn envelope_round_trip() -> TestResult {
    let input = ROUND_TRIP.as_bytes();
    let mut env = Envelope::default();
    stdjson::unmarshal(input, &mut env).map_err(|e| e.to_string())?;
    check_eq(&env.case_id.as_str(), &"c", "case id")?;
    check_eq(&env.fakes.as_ref().map(|f| f.int63), &Some(3), "fakes")?;
    check_eq(&env.request.headers.values("a"), &vec!["b"], "headers")?;
    let raw = stdjson::marshal(&env).map_err(|e| e.to_string())?;
    check_eq(raw.as_slice(), input, "re-encoded")?;
    let mut again = Envelope::default();
    jsoniter::unmarshal(&raw, &mut again).map_err(|e| e.to_string())?;
    check_eq(&again, &env, "jsoniter decodes the same")?;
    let decisions: BTreeMap<String, RawJson> = env.decisions.unwrap_or_default();
    check_eq(&decisions.len(), &1, "decisions")
}
