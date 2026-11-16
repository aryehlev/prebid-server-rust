//! Replays `fmt_*.txt` from `testdata/go/golden/fmt.go`: every catalog operand under every
//! format in `fmtFormats`, the curated `fmtCalls`, and 50000 random formats.

use std::{collections::BTreeMap, slice};

use super::*;
use crate::{
    golden::{self, SplitMix64, Sweep},
    strconv::parse_float,
    testutil::ensure_eq,
};

type Addrs = BTreeMap<String, u64>;

fn addr(addrs: &Addrs, name: &str) -> Result<u64, String> {
    addrs
        .get(name)
        .copied()
        .ok_or_else(|| format!("no address for {name}"))
}

fn list(ty: &str, elems: Vec<Value>) -> Value {
    Value::Slice {
        ty: ty.to_owned(),
        elems: Some(elems),
    }
}

fn map(ty: &str, entries: Vec<(Value, Value)>) -> Value {
    Value::Map {
        ty: ty.to_owned(),
        entries: Some(entries),
    }
}

fn record(ty: &str, fields: Vec<(&str, Value)>) -> Value {
    Value::Struct {
        ty: ty.to_owned(),
        fields: fields.into_iter().map(|(n, v)| (n.to_owned(), v)).collect(),
    }
}

fn ptr(ty: &str, addr: u64, elem: Value) -> Value {
    Value::Pointer {
        ty: ty.to_owned(),
        addr,
        elem: Some(Box::new(elem)),
    }
}

fn named(ty: &str, value: Value) -> Value {
    Value::Named {
        ty: ty.to_owned(),
        value: Box::new(value),
    }
}

/// `main.stringer{id}`, whose value-receiver `String` method returns `stringer#id`.
fn stringer(id: i64) -> Value {
    Value::Method {
        kind: MethodKind::Stringer,
        text: format!("stringer#{id}"),
        value: Box::new(record("main.stringer", vec![("id", Value::Int(id))])),
    }
}

fn errors_new(text: &str, addr: u64) -> Value {
    Value::Method {
        kind: MethodKind::Error,
        text: text.to_owned(),
        value: Box::new(ptr(
            "*errors.errorString",
            addr,
            record("errors.errorString", vec![("s", text.into())]),
        )),
    }
}

/// Go: `fmt.Errorf("...%w...", inner)`, a `*fmt.wrapError`.
fn wrap_error(text: &str, addr: u64, inner: Value) -> Value {
    Value::Method {
        kind: MethodKind::Error,
        text: text.to_owned(),
        value: Box::new(ptr(
            "*fmt.wrapError",
            addr,
            record("fmt.wrapError", vec![("msg", text.into()), ("err", inner)]),
        )),
    }
}

fn ints(ty: &str, values: &[i64]) -> Value {
    list(ty, values.iter().copied().map(Value::Int).collect())
}

/// Go: the slices, arrays and maps of `fmtCatalog`.
fn collections(err: &Value, errw: &Value) -> Vec<(&'static str, Value)> {
    let nil_strs = Value::Slice {
        ty: "[]string".to_owned(),
        elems: None,
    };
    let arr = Value::Array {
        ty: "[2]bool".to_owned(),
        elems: vec![true.into(), false.into()],
    };
    let anys = vec![
        Value::Int(1),
        "x".into(),
        Value::Nil,
        err.clone(),
        2.5.into(),
    ];
    let u8s = Value::Array {
        ty: "[3]uint8".to_owned(),
        elems: vec![Value::Uint8(1), Value::Uint8(2), Value::Uint8(255)],
    };
    let strings = vec![("b", 2), ("a", 1), ("c", 3)];
    let nil_map = Value::Map {
        ty: "map[string]int".to_owned(),
        entries: None,
    };
    let by_int = vec![(3, "c"), (-1, "z"), (0, "")];
    let by_float = vec![(f64::NAN, true), (1.0, false), (-1.0, true)];
    let anys_by_name = vec![("k".into(), ints("[]int", &[1])), ("n".into(), Value::Nil)];
    let by_bool = vec![(true.into(), Value::Int(1)), (false.into(), Value::Int(0))];
    vec![
        ("strs", list("[]string", vec!["a".into(), "b c".into()])),
        ("strsnil", nil_strs),
        ("ints", ints("[]int", &[1, -2, 3])),
        ("arr", arr),
        ("anys", list("[]interface {}", anys)),
        ("u8s", u8s),
        ("errs", list("[]error", vec![err.clone(), errw.clone()])),
        (
            "map",
            map(
                "map[string]int",
                strings
                    .into_iter()
                    .map(|(k, v)| (k.into(), Value::Int(v)))
                    .collect(),
            ),
        ),
        ("mapnil", nil_map),
        (
            "mapint",
            map(
                "map[int]string",
                by_int
                    .into_iter()
                    .map(|(k, v)| (Value::Int(k), v.into()))
                    .collect(),
            ),
        ),
        (
            "mapf",
            map(
                "map[float64]bool",
                by_float
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect(),
            ),
        ),
        ("mapany", map("map[string]interface {}", anys_by_name)),
        ("mapbool", map("map[bool]int", by_bool)),
    ]
}

/// Go: the structs, pointers and method values of `fmtCatalog`.
fn records(addrs: &Addrs, err: Value, errw: Value) -> Result<Vec<(&'static str, Value)>, String> {
    let pint = ptr("*int", addr(addrs, "pint")?, Value::Int(42));
    let ext = vec![("z".into(), Value::Int(26)), ("a".into(), Value::Int(1))];
    let imp = record(
        "main.imp",
        vec![
            ("ID", "imp-1".into()),
            ("BidFloor", 0.5.into()),
            ("Tags", list("[]string", vec!["a".into(), "b c".into()])),
            ("Ext", map("map[string]int", ext)),
            ("Ptr", pint.clone()),
            ("hidden", true.into()),
        ],
    );
    let holder = record(
        "main.holder",
        vec![
            ("Name", "h".into()),
            ("Err", err.clone()),
            ("err", err.clone()),
            ("Any", Value::Int(3)),
            ("Nil", Value::Nil),
            ("S", stringer(1)),
            ("s", stringer(2)),
        ],
    );
    let pstringer = Value::Method {
        kind: MethodKind::Stringer,
        text: "stringer#8".to_owned(),
        value: Box::new(ptr(
            "*main.stringer",
            addr(addrs, "pstringer")?,
            record("main.stringer", vec![("id", Value::Int(8))]),
        )),
    };
    let nil_imp = Value::Pointer {
        ty: "*main.imp".to_owned(),
        addr: 0,
        elem: None,
    };
    let strs = list("[]string", vec!["x".into()]);
    let one = map("map[string]int", vec![("k".into(), Value::Int(1))]);
    Ok(vec![
        ("imp", imp.clone()),
        ("pimp", ptr("*main.imp", addr(addrs, "pimp")?, imp)),
        ("nilimp", nil_imp),
        ("pint", pint),
        ("holder", holder.clone()),
        (
            "pholder",
            ptr("*main.holder", addr(addrs, "pholder")?, holder),
        ),
        ("stringer", stringer(7)),
        ("pstringer", pstringer),
        ("err", err),
        ("errw", errw),
        ("pstrs", ptr("*[]string", addr(addrs, "pstrs")?, strs)),
        ("pmap", ptr("*map[string]int", addr(addrs, "pmap")?, one)),
    ])
}

/// Go: `fmtCatalog`, in the same order.
fn catalog(addrs: &Addrs) -> Result<Vec<(&'static str, Value)>, String> {
    let mut values: Vec<(&'static str, Value)> = vec![
        ("nil", Value::Nil),
        ("true", true.into()),
        ("false", false.into()),
        ("int0", Value::Int(0)),
        ("int", Value::Int(-42)),
        ("i2", Value::Int(2)),
        ("i6", Value::Int(6)),
        ("intbig", Value::Int(10_000_000)),
        ("int8", Value::Int8(-128)),
        ("int16", Value::Int16(300)),
        ("int32", Value::Int32(0x1F600)),
        ("int64", Value::Int64(i64::MIN)),
        ("uint", Value::Uint(7)),
        ("uint8", Value::Uint8(200)),
        ("uint16", Value::Uint16(65535)),
        ("uint32", Value::Uint32(0xD800)),
        ("uint64", Value::Uint64(u64::MAX)),
        ("uintptr", Value::Uintptr(0xBEEF)),
        ("f0", 0.0.into()),
        ("fneg0", (-0.0).into()),
        ("f", 1.5.into()),
        ("fbig", 1e21.into()),
        ("fsmall", 1e-7.into()),
        (
            "fpi",
            parse_float("3.14159265", 64)
                .map_err(|e| e.to_string())?
                .into(),
        ),
        ("f123", 123_456_789.0.into()),
        ("fnan", f64::NAN.into()),
        ("finf", f64::INFINITY.into()),
        ("fninf", f64::NEG_INFINITY.into()),
        ("f32", 0.1_f32.into()),
        ("fround", 2.675.into()),
        ("s0", "".into()),
        ("s", "hello".into()),
        ("sutf", "h\u{e9}llo w\u{f6}rld".into()),
        ("sesc", "tab\tq\"\\".into()),
        ("sback", "`back`".into()),
        ("sctl", "\u{0}\u{7f}".into()),
        ("scjk", "\u{65e5}\u{672c}\u{8a9e}".into()),
        ("sbom", "\u{feff}bom".into()),
        ("snum", "12".into()),
        ("bytes", b"hi\xff".to_vec().into()),
        ("bnil", Value::Bytes(None)),
        ("bempty", Vec::new().into()),
        (
            "raw",
            named("json.RawMessage", b"{\"a\":1}".to_vec().into()),
        ),
        ("name", named("main.bidderName", "appnexus".into())),
    ];
    let err = errors_new("boom", addr(addrs, "err")?);
    let errw = wrap_error("wrapped: boom", addr(addrs, "errw")?, err.clone());
    values.extend(collections(&err, &errw));
    values.extend(records(addrs, err, errw)?);
    Ok(values)
}

fn text(field: &[u8]) -> Result<&str, String> {
    str::from_utf8(field).map_err(|e| e.to_string())
}

/// The catalog, after checking its names against the Go catalog's order.
fn load_catalog() -> Result<Vec<(&'static str, Value)>, String> {
    let rows = golden::rows(include_str!("../../testdata/go/golden/fmt_catalog.txt"))?;
    let mut addrs = Addrs::new();
    let mut names = Vec::new();
    for row in &rows {
        match row.as_slice() {
            [kind, name, a] if kind == "addr" => {
                let a: u64 = a.parse().map_err(|e| format!("{e}"))?;
                let _previous = addrs.insert(name.clone(), a);
            }
            [kind, name] if kind == "value" => names.push(name.as_str()),
            other => return Err(format!("unknown catalog row {other:?}")),
        }
    }
    let values = catalog(&addrs)?;
    let got: Vec<&str> = values.iter().map(|(n, _)| *n).collect();
    ensure_eq(&got, &names, "catalog names")?;
    Ok(values)
}

fn lookup<'a>(values: &'a [(&str, Value)], name: &str) -> Result<&'a Value, String> {
    values
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v)
        .ok_or_else(|| format!("unknown operand {name}"))
}

fn args(values: &[(&str, Value)], names: &str) -> Result<Vec<Value>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    names
        .split(',')
        .map(|name| lookup(values, name).cloned())
        .collect()
}

#[test]
fn every_verb_on_every_operand_matches_go() -> Result<(), String> {
    let values = load_catalog()?;
    let rows = golden::rows_bytes(include_str!("../../testdata/go/golden/fmt_verbs.txt"))?;
    for row in &rows {
        let [format, name, want] = row.as_slice() else {
            return Err(format!("bad row {row:?}"));
        };
        let (format, name) = (text(format)?, text(name)?);
        let value = lookup(&values, name)?;
        let got = sprintf_bytes(format, slice::from_ref(value));
        ensure_eq(
            got.as_slice(),
            want.as_slice(),
            &format!("Sprintf({format:?}, {name})"),
        )?;
    }
    ensure_eq(&rows.len(), &(48 * values.len()), "row count")
}

#[test]
fn curated_calls_match_go() -> Result<(), String> {
    let values = load_catalog()?;
    let rows = golden::rows(include_str!("../../testdata/go/golden/fmt_calls.txt"))?;
    for row in &rows {
        let [kind, format, names, want] = golden::fields::<4>(row)?;
        let args = args(&values, &names)?;
        let got = match kind.as_str() {
            "sprintf" => sprintf(&format, &args),
            "errorf" => errorf(&format, &args),
            "sprint" => sprint(&args),
            "sprintln" => sprintln(&args),
            other => return Err(format!("unknown call {other}")),
        };
        ensure_eq(&got, &want, &format!("{kind}({format:?}, {names})"))?;
    }
    ensure_eq(&rows.len(), &88, "row count")
}

const FLAG_CHARS: &[u8] = b"#0+- ";

const VERBS: [&str; 23] = [
    "v", "d", "s", "q", "x", "X", "t", "f", "F", "e", "E", "g", "G", "c", "U", "b", "o", "O", "T",
    "%", "w", "z", "\u{e9}",
];

fn pick(rng: &mut SplitMix64, n: usize) -> Result<usize, String> {
    let n = u64::try_from(n).map_err(|e| e.to_string())?;
    usize::try_from(rng.next_u64() % n).map_err(|e| e.to_string())
}

/// Go: `randomFormat`.
fn random_format(rng: &mut SplitMix64) -> Result<String, String> {
    let mut b = String::new();
    let n = 1 + rng.next_u64() % 3;
    for _ in 0..n {
        if rng.next_u64().is_multiple_of(4) {
            b.push('a');
        }
        b.push('%');
        for _ in 0..rng.next_u64() % 3 {
            let flag = FLAG_CHARS.get(pick(rng, FLAG_CHARS.len())?).ok_or("flag")?;
            b.push(char::from(*flag));
        }
        if rng.next_u64().is_multiple_of(6) {
            b.push('[');
            b.push_str(&(rng.next_u64() % 4).to_string());
            b.push(']');
        }
        match rng.next_u64() % 4 {
            1 => b.push_str(&(rng.next_u64() % 12).to_string()),
            2 => b.push('*'),
            _ => {}
        }
        match rng.next_u64() % 4 {
            1 => {
                b.push('.');
                b.push_str(&(rng.next_u64() % 8).to_string());
            }
            2 => b.push('.'),
            3 => b.push_str(".*"),
            _ => {}
        }
        b.push_str(VERBS.get(pick(rng, VERBS.len())?).ok_or("verb")?);
    }
    Ok(b)
}

#[test]
fn random_formats_match_go() -> Result<(), String> {
    let values = load_catalog()?;
    let mut rng = SplitMix64(4);
    let mut sweep = Sweep::default();
    for _ in 0..50_000 {
        let format = random_format(&mut rng)?;
        let mut args = Vec::new();
        for _ in 0..rng.next_u64() % 4 {
            let (_, v) = values.get(pick(&mut rng, values.len())?).ok_or("operand")?;
            args.push(v.clone());
        }
        let mut line = format.clone().into_bytes();
        line.push(b'\t');
        line.extend(sprintf_bytes(&format, &args));
        sweep.add_bytes(&line);
    }
    let sums = include_str!("../../testdata/go/golden/fmt_sweeps.txt");
    ensure_eq(
        sweep.sum().as_str(),
        golden::checksum(sums, "fmt_random")?.as_str(),
        "random",
    )
}

#[test]
fn error_helper_is_errors_new() -> Result<(), String> {
    let err = Value::error("boom");
    ensure_eq(
        &sprintf("%v|%T", &[err.clone(), err.clone()]),
        &"boom|*errors.errorString".to_owned(),
        "error",
    )?;
    ensure_eq(
        &sprintf("%d", &[err]),
        &"&{%!d(string=boom)}".to_owned(),
        "%d",
    )
}
