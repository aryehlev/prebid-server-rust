use std::collections::{BTreeMap, BTreeSet};

use serde_json::value::RawValue;

use super::{CaptureSpec, Envelope, HeaderList, HeaderPair, MockSpec};
use crate::{
    golden::{TestResult, check, check_eq},
    ids::DecisionId,
};

/// `text` as a raw JSON value.
fn raw(text: &str) -> Result<Box<RawValue>, String> {
    RawValue::from_string(text.to_owned()).map_err(|e| format!("{text}: {e}"))
}

fn capture(stages: &str) -> Result<CaptureSpec, String> {
    Ok(CaptureSpec {
        stages: Some(raw(stages)?),
        ..CaptureSpec::default()
    })
}

fn filter_of(stages: &str) -> Result<Result<Option<Vec<String>>, String>, String> {
    Ok(capture(stages)?
        .stage_filter()
        .map(|set| set.map(|ids| ids.into_iter().collect()))
        .map_err(|err| err.to_string()))
}

// Go: parity/envelope_test.go TestCaptureStageFilter, its cases first (the wrong type's message
// is serde_json's, json.error-texts), then the port's own: unregistered and repeated IDs, an
// object and a number element.
#[test]
fn stage_filter() -> TestResult {
    check_eq(
        &CaptureSpec::default().stage_filter(),
        &Ok(None),
        "omitted means all",
    )?;
    check_eq(&filter_of("null")?, &Ok(None), "null means all")?;
    check_eq(&filter_of(r#""*""#)?, &Ok(None), "star means all")?;
    check_eq(
        &filter_of(r#"["http.in","http.response"]"#)?,
        &Ok(Some(vec!["http.in".to_owned(), "http.response".to_owned()])),
        "list",
    )?;
    check_eq(
        &filter_of("[]")?,
        &Ok(Some(Vec::new())),
        "empty list captures nothing",
    )?;
    check(
        filter_of(r#""all""#)?.is_err_and(|e| e.contains(r#"must be "*""#)),
        "other string",
    )?;
    check(
        filter_of(r#"{"a":1}"#)?.is_err_and(|e| e.contains("capture.stages")),
        "wrong type",
    )?;
    check_eq(
        &filter_of(r#"["http.in","made.up","http.in"]"#)?,
        &Ok(Some(vec!["http.in".to_owned(), "made.up".to_owned()])),
        "list (unregistered IDs are kept)",
    )?;
    check_eq(&filter_of("[]")?, &Ok(Some(Vec::new())), "empty list")?;
    check_eq(
        &filter_of(r#""all""#)?,
        &Err(r#"capture.stages: string value must be "*", got "all""#.to_owned()),
        "other string",
    )?;
    check_eq(
        &filter_of("{}")?,
        &Err(
            "capture.stages: invalid type: map, expected a sequence at line 1 column 0".to_owned(),
        ),
        "object",
    )?;
    check_eq(
        &filter_of("[1]")?,
        &Err(
            "capture.stages: invalid type: integer `1`, expected a string at line 1 column 2"
                .to_owned(),
        ),
        "number element",
    )?;
    let ids: BTreeSet<String> = BTreeSet::from(["x".to_owned()]);
    check_eq(&capture(r#"["x"]"#)?.stage_filter(), &Ok(Some(ids)), "set")
}

// Go: parity/envelope_test.go TestCaptureDefaults, case by case, and one flag of each.
#[test]
fn capture_flags_default_on() -> TestResult {
    let flags = |metrics, analytics| CaptureSpec {
        metrics,
        analytics,
        ..CaptureSpec::default()
    };
    for (name, spec, want) in [
        ("defaults on", CaptureSpec::default(), (true, true)),
        (
            "explicit off",
            flags(Some(false), Some(false)),
            (false, false),
        ),
        ("explicit on", flags(Some(true), Some(true)), (true, true)),
        ("one of each", flags(Some(false), Some(true)), (false, true)),
    ] {
        check_eq(&(spec.wants_metrics(), spec.wants_analytics()), &want, name)?;
    }
    Ok(())
}

// Go: parity/envelope_test.go TestMockSpecDefaults.
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

fn envelope_with(decisions: &[(&str, &str)]) -> Result<Envelope, String> {
    Ok(Envelope {
        decisions: Some(
            decisions
                .iter()
                .map(|(k, v)| Ok(((*k).to_owned(), raw(v)?)))
                .collect::<Result<_, String>>()?,
        ),
        ..Envelope::default()
    })
}

// Go: parity/envelope_test.go TestEnvelopeDecisions, case by case. Go's nil envelope is an
// envelope without decisions here: a Rust envelope is not a pointer.
#[test]
fn envelope_decisions() -> TestResult {
    let env = envelope_with(&[
        ("floors.skip", "1"),
        ("usersync.shuffle", r#""identity""#),
        ("floors.enforce", r#""oops""#),
    ])?;
    let nil = Envelope::default();
    check_eq(
        &env.decision_int(DecisionId::FloorsSkip),
        &Some(1),
        "int override",
    )?;
    check_eq(
        &env.decision_string(DecisionId::UsersyncShuffle),
        &Some("identity".to_owned()),
        "string override",
    )?;
    check_eq(
        &env.decision_int(DecisionId::FloorsEnforce),
        &None,
        "wrong type is no override",
    )?;
    check_eq(
        &env.decision_int(DecisionId::CategoryDedupe),
        &None,
        "absent",
    )?;
    check_eq(
        &nil.decision_int(DecisionId::FloorsSkip),
        &None,
        "nil envelope",
    )?;
    check_eq(
        &nil.decision_string(DecisionId::UsersyncShuffle),
        &None,
        "nil envelope string",
    )
}

// Go: parity/envelope.go DecisionInt and DecisionString (json.Unmarshal into an int and a
// string), beyond TestEnvelopeDecisions: a string is no int, a fraction is none, and null reads
// as the zero value.
#[test]
fn decisions() -> TestResult {
    let env = envelope_with(&[
        ("floors.skip", "1"),
        ("floors.data_rate", r#""1""#),
        ("floors.modelgroup", "2.5"),
        ("floors.enforce", "null"),
        ("usersync.shuffle", r#""identity""#),
    ])?;
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
            .map(RawValue::get),
        &Some("2.5"),
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
    check_eq(
        &serde_json::to_string(&list).map_err(|e| e.to_string())?,
        &r#"[["Content-Type","a"],["X","b"],["content-type","c"]]"#.to_owned(),
        "encoding",
    )?;
    check_eq(
        &serde_json::to_string(&HeaderList::default()).map_err(|e| e.to_string())?,
        &"null".to_owned(),
        "nil encoding",
    )?;
    let read: HeaderList =
        serde_json::from_str(r#"[["a","1"],["A","2"]]"#).map_err(|e| e.to_string())?;
    check_eq(&read.values("a"), &vec!["1", "2"], "decoded")
}

fn pairs(input: &str) -> Result<Vec<HeaderPair>, String> {
    serde_json::from_str::<HeaderList>(input)
        .map(|list| list.pairs().to_vec())
        .map_err(|e| e.to_string())
}

/// An envelope as the Go runner writes it (members in struct order, empty members omitted).
const ROUND_TRIP: &str = concat!(
    r#"{"schema":"pbs-parity-envelope/1","schema_version":"1.0.0","case_id":"c","#,
    r#""request":{"method":"POST","path":"/openrtb2/auction","query":"","#,
    r#""headers":[["A","b"]],"body":{"json":{"id":"1"}}},"mock":{"mode":"inline"},"#,
    r#""decisions":{"floors.skip":1},"fakes":{"int63":3},"#,
    r#""capture":{"stages":["http.in"],"metrics":false}}"#,
);

#[test]
fn envelope_round_trip() -> TestResult {
    let env: Envelope = serde_json::from_str(ROUND_TRIP).map_err(|e| e.to_string())?;
    check_eq(&env.case_id.as_str(), &"c", "case id")?;
    check_eq(&env.fakes.as_ref().map(|f| f.int63), &Some(3), "fakes")?;
    check_eq(&env.request.headers.values("a"), &vec!["b"], "headers")?;
    check_eq(
        &env.decisions.as_ref().map(BTreeMap::len),
        &Some(1),
        "decisions",
    )?;
    check_eq(
        &serde_json::to_string(&env).map_err(|e| e.to_string())?,
        &ROUND_TRIP.to_owned(),
        "written back in the same member order",
    )
}

// The documents are serde types read and written by serde_json (json.parity-documents), so a
// document the Go runner would never write reads otherwise than in Go: Go fills a member from a
// key that matches it only ignoring case, keeps the last of repeated members, reads null into a
// value member as its zero value, pads or cuts a header pair to two strings, reads a string
// holding bytes that are not UTF-8 and writes `<`, `>` and `&` as `\u` escapes; serde ignores the
// key, refuses the next three, and writes the characters as they are.
#[test]
fn documents_read_as_serde_reads_them() -> TestResult {
    let read = |input: &[u8]| serde_json::from_slice::<Envelope>(input).map_err(|e| e.to_string());
    let folded = read(br#"{"CASE_ID":"c","Request":{"PATH":"/status"}}"#)?;
    check_eq(
        &(folded.case_id.as_str(), folded.request.path.as_str()),
        &("", ""),
        "key case",
    )?;
    check_eq(
        &read(br#"{"case_id":"a","case_id":"b"}"#).err(),
        &Some("duplicate field `case_id` at line 1 column 24".to_owned()),
        "repeated member",
    )?;
    check_eq(
        &read(br#"{"case_id":null}"#).err(),
        &Some("invalid type: null, expected a string at line 1 column 15".to_owned()),
        "null into a value member",
    )?;
    check_eq(
        &read(b"{\"case_id\":\"\xff\"}").err(),
        &Some("invalid unicode code point at line 1 column 13".to_owned()),
        "invalid UTF-8",
    )?;
    for (input, error) in [
        (
            r#"[["only"]]"#,
            "invalid length 1, expected a tuple of size 2 at line 1 column 9",
        ),
        (
            r#"[["x","y","z"]]"#,
            "trailing characters at line 1 column 11",
        ),
        (
            "[[]]",
            "invalid length 0, expected a tuple of size 2 at line 1 column 3",
        ),
        (
            "[null]",
            "invalid type: null, expected a tuple of size 2 at line 1 column 5",
        ),
        (
            r#"[["k",null]]"#,
            "invalid type: null, expected a string at line 1 column 10",
        ),
    ] {
        check_eq(&pairs(input), &Err(error.to_owned()), input)?;
    }
    let html = Envelope {
        case_id: "<a&b>".to_owned(),
        ..Envelope::default()
    };
    check(
        serde_json::to_string(&html)
            .map_err(|e| e.to_string())?
            .contains(r#""case_id":"<a&b>""#),
        "no HTML escapes",
    )
}
