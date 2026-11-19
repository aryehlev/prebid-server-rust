//! The loader's casts and weak decode against `testdata/golden/scalars.json`: spf13/cast v1.5.0
//! as Viper's `Get` applies it, and mapstructure v1.5.0 with `WeaklyTypedInput` and no hook into
//! every kind of target `config.Configuration` has.

use std::{collections::BTreeMap, fs, path::Path};

use serde_json::Value as Json;

use super::{
    cast,
    decode::{Decoder, GoTypes},
    value::{GoValue, compact_float},
};

/// A test result whose error is the failure message.
type TestResult<T = ()> = Result<T, String>;

/// The rows of `scalars.json`'s list `name`.
fn rows(name: &str) -> TestResult<Vec<Json>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("golden")
        .join("scalars.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: Json = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    doc.get(name)
        .and_then(Json::as_array)
        .cloned()
        .ok_or_else(|| format!("scalars.json has no list {name}"))
}

/// A Go `string`.
fn s(text: &str) -> GoValue {
    GoValue::String(text.to_owned())
}

/// A Go `map[string]interface{}`.
fn map<const N: usize>(entries: [(&str, GoValue); N]) -> GoValue {
    GoValue::Map(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

/// `scalars.go`'s `castInputs`, in its order, as the Go values they are.
fn cast_inputs() -> Vec<GoValue> {
    let mut inputs: Vec<GoValue> = [
        "",
        "true",
        "TRUE",
        "True",
        "tRuE",
        "t",
        "T",
        "1",
        "0",
        "f",
        "F",
        "false",
        "yes",
        "on",
        "y",
        "maybe",
        " 1",
        "1 ",
        "1.0",
        "1.00",
        "1.5",
        "-1.0",
        "0x1F",
        "0X1F",
        "0o17",
        "017",
        "0b11",
        "1_000",
        "_1",
        "+5",
        "-5",
        "1e3",
        "9223372036854775807",
        "9223372036854775808",
        "-9223372036854775809",
        "NaN",
        "Inf",
        "-inf",
        "infinity",
        "a b",
        "a,b",
        " a  b ",
        "a\tb\nc",
        "\u{a0}a\u{2003}b",
    ]
    .into_iter()
    .map(s)
    .collect();
    inputs.extend([
        GoValue::Int(0),
        GoValue::Int(1),
        GoValue::Int(-1),
        GoValue::Int(42),
        GoValue::Int64(1),
        GoValue::Int64(0),
        GoValue::Uint64(u64::MAX),
        GoValue::Float64(0.0),
        GoValue::Float64(1.0),
        GoValue::Float64(1.5),
        GoValue::Float64(-1.5),
        GoValue::Float64(1e21),
        GoValue::Float64(f64::NAN),
        GoValue::Float64(f64::INFINITY),
        GoValue::Bool(true),
        GoValue::Bool(false),
        GoValue::Nil,
        GoValue::List(vec![
            s("a"),
            GoValue::Int(1),
            GoValue::Nil,
            GoValue::Bool(true),
            GoValue::Float64(1.5),
        ]),
        GoValue::List(Vec::new()),
        GoValue::Strings(Some(vec!["a".to_owned(), "b".to_owned()])),
        map([("a", GoValue::Int(1))]),
        GoValue::Map(BTreeMap::new()),
    ]);
    inputs
}

/// `scalars.go`'s `weakInputs`: the cast inputs and the maps a struct and a map target take.
fn weak_inputs() -> Vec<GoValue> {
    let mut inputs = cast_inputs();
    inputs.extend([
        map([("tagged", s("5")), ("plain", GoValue::Int(1))]),
        map([
            ("TAGGED", GoValue::Int(2)),
            ("Plain", s("x")),
            ("other", GoValue::Bool(true)),
        ]),
        map([("tagged", GoValue::List(vec![GoValue::Int(1)]))]),
        GoValue::List(vec![
            map([("tagged", GoValue::Int(1))]),
            map([("plain", s("p"))]),
        ]),
        GoValue::List(vec![GoValue::List(vec![s("a"), s("b")]), s("c")]),
        GoValue::List(vec![s("x"), s("y"), s("z")]),
    ]);
    inputs
}

/// The Go types of `scalars.go`'s `weakTargets`, under their `reflect.Type.String()`.
const WEAK_TYPES: &str = r#"{"types": {
    "bool": {"kind": "bool"},
    "int": {"kind": "int", "bits": 64},
    "int8": {"kind": "int", "bits": 8},
    "int16": {"kind": "int", "bits": 16},
    "int32": {"kind": "int", "bits": 32},
    "int64": {"kind": "int", "bits": 64},
    "uint": {"kind": "uint", "bits": 64},
    "uint8": {"kind": "uint", "bits": 8},
    "uint16": {"kind": "uint", "bits": 16},
    "uint32": {"kind": "uint", "bits": 32},
    "uint64": {"kind": "uint", "bits": 64},
    "float32": {"kind": "float", "bits": 32},
    "float64": {"kind": "float", "bits": 64},
    "string": {"kind": "string"},
    "[]string": {"kind": "slice", "elem": "string"},
    "[][]string": {"kind": "slice", "elem": "[]string"},
    "[]int": {"kind": "slice", "elem": "int"},
    "[2]string": {"kind": "array", "elem": "string", "len": 2},
    "[]uint8": {"kind": "slice", "elem": "uint8"},
    "json.RawMessage": {"kind": "slice", "elem": "uint8"},
    "map[string]string": {"kind": "map", "key": "string", "elem": "string"},
    "map[string]interface {}": {"kind": "map", "key": "string", "elem": "interface {}"},
    "map[string]bool": {"kind": "map", "key": "string", "elem": "bool"},
    "interface {}": {"kind": "interface"},
    "*bool": {"kind": "ptr", "elem": "bool"},
    "*int": {"kind": "ptr", "elem": "int"},
    "main.weakStruct": {"kind": "struct", "fields": [
        {"name": "tagged", "type": "int"},
        {"name": "Plain", "type": "string"}
    ]},
    "*main.weakStruct": {"kind": "ptr", "elem": "main.weakStruct"},
    "[]main.weakStruct": {"kind": "slice", "elem": "main.weakStruct"}
}}"#;

/// The number of `weakTargets`, which `scalars.go` writes one row of per input.
const WEAK_TARGETS: usize = 29;

/// A golden row's result: its value, or its error.
fn result_of(row: &Json) -> Result<Json, String> {
    row.get("error").and_then(Json::as_str).map_or_else(
        || Ok(row.get("value").cloned().unwrap_or(Json::Null)),
        |error| Err(error.to_owned()),
    )
}

/// Each cast Viper's `Get` applies by the type of a key's default gives cast's result, its zero
/// value where cast fails (`Get` drops the error).
// Go: viper.go:884-929 (Get, v1.12.0); cast v1.5.0 caste.go (ToBoolE, ToIntE, ToFloat64E,
// ToStringE, ToStringSliceE)
#[test]
fn cast_matches_spf13_cast() -> TestResult {
    let golden = rows("cast")?;
    let inputs = cast_inputs();
    if golden.len() != inputs.len() {
        return Err(format!(
            "{} cast rows, {} inputs",
            golden.len(),
            inputs.len()
        ));
    }
    let mut failures = Vec::new();
    for (row, input) in golden.iter().zip(&inputs) {
        if row.get("input") != Some(&input.compact()) {
            failures.push(format!("the golden's input is not {input:?}"));
            continue;
        }
        let checks = [
            (
                "to_bool",
                Json::Bool(cast::to_bool(input)),
                Json::Bool(false),
            ),
            ("to_int", Json::from(cast::to_int(input)), Json::from(0)),
            (
                "to_float64",
                compact_float(cast::to_float64(input)),
                compact_float(0.0),
            ),
            (
                "to_string",
                Json::String(cast::to_string(input)),
                Json::String(String::new()),
            ),
            (
                "to_string_slice",
                GoValue::Strings(cast::to_string_slice(input)).compact(),
                Json::Null,
            ),
        ];
        for (name, got, zero) in checks {
            let want = row
                .get(name)
                .map(result_of)
                .ok_or_else(|| format!("the golden has no {name}"))?;
            if got != want.clone().unwrap_or(zero) {
                failures.push(format!("{name}({input}): got {got}, want {want:?}"));
            }
        }
        let checked = cast::to_string_checked(input).is_some();
        if checked != row.get("to_string").is_some_and(|r| result_of(r).is_ok()) {
            failures.push(format!("ToStringE({input}) fails: {}", !checked));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

/// mapstructure's weak decode of every input into every target kind gives mapstructure's value,
/// or its error text.
// Go: mapstructure.go v1.5.0 (decode and the decode* functions with WeaklyTypedInput)
#[test]
fn weak_decode_matches_mapstructure() -> TestResult {
    let golden = rows("weak")?;
    let inputs = weak_inputs();
    if golden.len() != inputs.len() * WEAK_TARGETS {
        return Err(format!(
            "{} weak rows, {} inputs of {WEAK_TARGETS} targets",
            golden.len(),
            inputs.len()
        ));
    }
    let types: GoTypes = serde_json::from_str(WEAK_TYPES).map_err(|e| e.to_string())?;
    let decoder = Decoder::new(&types, false);
    let mut failures = Vec::new();
    for (row, input) in golden
        .iter()
        .zip(inputs.iter().flat_map(|input| [input; WEAK_TARGETS]))
    {
        if row.get("input") != Some(&input.compact()) {
            failures.push(format!("the golden's input is not {input:?}"));
            continue;
        }
        let target = row
            .get("target")
            .and_then(Json::as_str)
            .ok_or("a weak row without a target")?;
        let ty = match target {
            "struct" => "main.weakStruct",
            "*struct" => "*main.weakStruct",
            "[]struct" => "[]main.weakStruct",
            other => other,
        };
        let mut out = decoder.zero(ty).map_err(|e| e.to_string())?;
        let got = decoder
            .decode("", input, ty, &mut out)
            .map(|()| out.compact())
            .map_err(|e| e.to_string());
        let want = result_of(row);
        if got != want {
            failures.push(format!("{input} into {target}: got {got:?}, want {want:?}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}
