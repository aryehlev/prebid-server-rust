//! Replays `testdata/go/golden/template.go`: curated parse/exec cases over Prebid's macro
//! structs and a struct of every modelled kind, and 30000 random templates.

use super::{
    Template, TemplateError,
    node::{Arg, Branch, List, Node, Pipe},
};
use crate::{
    fmt::{MethodKind, Value},
    golden::{self, SplitMix64, Sweep},
    testutil::ensure_eq,
};

fn s(v: &str) -> Value {
    Value::from(v)
}

fn strings(ty: &str, items: Option<&[&str]>) -> Value {
    Value::Slice {
        ty: ty.to_owned(),
        elems: items.map(|items| items.iter().map(|i| s(i)).collect()),
    }
}

fn record(ty: &str, fields: Vec<(&str, Value)>) -> Value {
    Value::Struct {
        ty: ty.to_owned(),
        fields: fields.into_iter().map(|(n, v)| (n.to_owned(), v)).collect(),
    }
}

fn map(ty: &str, entries: Vec<(Value, Value)>) -> Value {
    Value::Map {
        ty: ty.to_owned(),
        entries: Some(entries),
    }
}

fn tinner(id: &str, level: i64) -> Value {
    record(
        "main.tinner",
        vec![("ID", s(id)), ("Level", Value::Int(level))],
    )
}

/// Go: `newTdata(withPtr)`.
fn tdata(with_ptr: bool) -> Value {
    let ptr = Value::Pointer {
        ty: "*main.tinner".to_owned(),
        addr: 0x00C0_0001_2000,
        elem: with_ptr.then(|| Box::new(tinner("p", 2))),
    };
    record(
        "main.tdata",
        vec![
            ("Name", s("x")),
            ("Count", Value::Int(3)),
            ("Price", Value::Float64(1.5)),
            ("OK", Value::Bool(true)),
            ("Tags", strings("[]string", Some(&["a", "b", "c"]))),
            ("Empty", strings("[]string", None)),
            (
                "Attrs",
                map(
                    "map[string]string",
                    vec![(s("k"), s("v")), (s("j"), s("w"))],
                ),
            ),
            (
                "Nums",
                map(
                    "map[string]int",
                    vec![
                        (s("b"), Value::Int(2)),
                        (s("a"), Value::Int(1)),
                        (s("c"), Value::Int(3)),
                    ],
                ),
            ),
            (
                "IntKeys",
                map("map[int]string", vec![(Value::Int(1), s("one"))]),
            ),
            ("Inner", tinner("i", 1)),
            ("Ptr", ptr),
            (
                "NilPtr",
                Value::Pointer {
                    ty: "*main.tinner".to_owned(),
                    addr: 0,
                    elem: None,
                },
            ),
            ("Any", tinner("any", 5)),
            ("Nil", Value::Nil),
            ("Err", Value::error("boom")),
            (
                "Bidder",
                Value::Named {
                    ty: "main.bidderName".to_owned(),
                    value: Box::new(s("appnexus")),
                },
            ),
            (
                "S",
                Value::Method {
                    kind: MethodKind::Stringer,
                    text: "stringer#7".to_owned(),
                    value: Box::new(record("main.stringer", vec![("id", Value::Int(7))])),
                },
            ),
            ("Raw", Value::Bytes(Some(b"hi".to_vec()))),
            (
                "Arr",
                Value::Array {
                    ty: "[2]int".to_owned(),
                    elems: vec![Value::Int(4), Value::Int(5)],
                },
            ),
            ("N", Value::Int(3)),
            ("U8", Value::Uint8(2)),
            ("hidden", s("h")),
        ],
    )
}

/// Go: `macros.EndpointTemplateParams` as `endpointParams()` fills it.
fn endpoint() -> Value {
    let set = [
        ("Host", "ads.example.com"),
        ("PublisherID", "pub-1"),
        ("ZoneID", "zone-9"),
        ("SourceId", "src"),
        ("AccountID", "acc-7"),
        ("Region", "us-east"),
    ];
    let names = [
        "Host",
        "PublisherID",
        "ZoneID",
        "SourceId",
        "AccountID",
        "AdUnit",
        "MediaType",
        "GvlID",
        "PageID",
        "SupplyId",
        "ImpID",
        "SspId",
        "SspID",
        "SeatID",
        "TokenID",
        "PartnerId",
        "Region",
        "PlacementID",
        "NetworkId",
        "SiteDomain",
        "AppDomain",
        "Bundle",
    ];
    let fields = names
        .iter()
        .map(|&n| {
            let v = set.iter().find(|(k, _)| *k == n).map_or("", |(_, v)| v);
            (n, s(v))
        })
        .collect();
    record("macros.EndpointTemplateParams", fields)
}

/// Go: `templateData()[key]`.
fn data(key: &str) -> Result<Value, String> {
    Ok(match key {
        "privacy" => record(
            "macros.UserSyncPrivacy",
            vec![
                ("GDPR", s("1")),
                (
                    "GDPRConsent",
                    s("BOPVK28OVJoTBABABAENBs-AAAAhuAKAANAAoACwAGgAPAAQAAgAEAAoACA"),
                ),
                ("USPrivacy", s("1YNN")),
                (
                    "GPP",
                    s("DBACNYA~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA~1NYN"),
                ),
                ("GPPSID", s("2")),
            ],
        ),
        "endpoint" => endpoint(),
        "endpointPtr" => Value::Pointer {
            ty: "*macros.EndpointTemplateParams".to_owned(),
            addr: 0x00C0_0001_3000,
            elem: Some(Box::new(endpoint())),
        },
        "nilEndpoint" => Value::Pointer {
            ty: "*macros.EndpointTemplateParams".to_owned(),
            addr: 0,
            elem: None,
        },
        "nil" => Value::Nil,
        "data" => tdata(true),
        "string" => s("hello"),
        "map" => map(
            "map[string]interface {}",
            vec![
                (s("a"), Value::Int(1)),
                (s("b"), s("two")),
                (s("n"), Value::Nil),
                (
                    s("m"),
                    map("map[string]int", vec![(s("z"), Value::Int(26))]),
                ),
            ],
        ),
        "slice" => Value::Slice {
            ty: "[]interface {}".to_owned(),
            elems: Some(vec![s("a"), Value::Int(1), Value::Nil]),
        },
        other => return Err(format!("unknown data {other}")),
    })
}

/// Go: `unsupportedNode` over every template of a parse.
fn uses_unsupported(tmpl: &Template) -> bool {
    tmpl.trees.values().any(|t| list_unsupported(&t.root))
}

fn list_unsupported(list: &List) -> bool {
    list.nodes.iter().any(node_unsupported)
}

fn branch_unsupported(b: &Branch) -> bool {
    pipe_unsupported(&b.pipe)
        || list_unsupported(&b.list)
        || b.else_list.as_ref().is_some_and(list_unsupported)
}

fn node_unsupported(n: &Node) -> bool {
    match n {
        Node::Action { pipe, .. } => pipe_unsupported(pipe),
        Node::If(b) | Node::Range(b) | Node::With(b) => branch_unsupported(b),
        Node::Template { pipe, .. } => pipe.as_ref().is_some_and(pipe_unsupported),
        Node::Text { .. }
        | Node::Break { .. }
        | Node::Continue { .. }
        | Node::End { .. }
        | Node::Else { .. } => false,
    }
}

fn pipe_unsupported(p: &Pipe) -> bool {
    p.cmds.iter().flat_map(|c| &c.args).any(arg_unsupported)
}

fn arg_unsupported(a: &Arg) -> bool {
    match a {
        Arg::Identifier { .. } => true,
        Arg::Number(n) => n.is_complex,
        Arg::Chain { node, .. } => arg_unsupported(node),
        Arg::Pipe(p) => pipe_unsupported(p),
        Arg::Variable { .. }
        | Arg::Dot { .. }
        | Arg::Nil { .. }
        | Arg::Field { .. }
        | Arg::Bool { .. }
        | Arg::String { .. } => false,
    }
}

/// Go: `runTemplate`.
fn run(name: &str, text: &str, data: &Value) -> String {
    let tmpl = match Template::parse(name, text) {
        Ok(tmpl) => tmpl,
        Err(e) => return format!("parse: {e}"),
    };
    let executed = tmpl.execute(data);
    if uses_unsupported(&tmpl) {
        // This port must refuse exactly the templates Go's tree walk flags, when it gets there.
        return match executed {
            Err(TemplateError::Unsupported(_) | TemplateError::Exec { .. }) | Ok(_) => {
                "unsupported".to_owned()
            }
            Err(TemplateError::Parse(e)) => format!("parse during exec: {e}"),
        };
    }
    match executed {
        Ok(out) => format!("ok: {out}"),
        Err(TemplateError::Exec {
            name: failed,
            message,
        }) => format!("exec: [{failed}] {message}"),
        Err(e) => format!("error: {e}"),
    }
}

#[test]
fn cases_match_go() -> Result<(), String> {
    let rows = golden::rows(include_str!("../../testdata/go/golden/template_cases.txt"))?;
    ensure_eq(&rows.len(), &197, "case count")?;
    for row in &rows {
        let [name, key, text, want] = golden::fields::<4>(row)?;
        let got = run(&name, &text, &data(&key)?);
        ensure_eq(&got, &want, &format!("{name:?} on {key}: {text:?}"))?;
    }
    Ok(())
}

#[test]
fn unsupported_templates_fail_with_go_prefix() -> Result<(), String> {
    let tmpl = Template::parse("t", "a{{len .Tags}}").map_err(|e| e.to_string())?;
    ensure_eq(
        &tmpl.execute(&tdata(true)),
        &Err(TemplateError::Unsupported(
            "template: t:1:3: executing \"t\" at <len>: builtin function \"len\" is not supported"
                .to_owned(),
        )),
        "len",
    )?;
    let complex = Template::parse("t", "{{1+2i}}").map_err(|e| e.to_string())?;
    ensure_eq(
        &complex.execute(&Value::Nil).map_err(|e| e.to_string()),
        &Err(
            "template: t:1:2: executing \"t\" at <1+2i>: complex constant is not supported"
                .to_owned(),
        ),
        "complex",
    )
}

fn nested(open: &str, depth: usize, middle: &str, close: &str) -> String {
    let mut text = open.repeat(depth);
    text.push_str(middle);
    text.push_str(&close.repeat(depth));
    text
}

#[test]
fn nesting_limits_fail_cleanly() -> Result<(), String> {
    let data = tdata(true);
    let parens = |depth| format!("{{{{{}}}}}", nested("(", depth, ".Name", ")"));
    ensure_eq(
        &run("t", &parens(100), &data),
        &"ok: x".to_owned(),
        "100 parens",
    )?;
    ensure_eq(
        &run("t", &parens(101), &data),
        &"parse: template: t:1: max expression depth exceeded".to_owned(),
        "101 parens",
    )?;
    let ifs = |depth| nested("{{if .OK}}", depth, "deep", "{{end}}");
    ensure_eq(
        &run("t", &ifs(100), &data),
        &"ok: deep".to_owned(),
        "100 ifs",
    )?;
    ensure_eq(
        &run("t", &ifs(101), &data),
        &"parse: template: t:1: max control nesting depth exceeded".to_owned(),
        "101 ifs",
    )?;
    let calls = |depth: usize| {
        let mut defs = String::new();
        for i in 0..depth {
            defs.push_str("{{define \"d");
            defs.push_str(&i.to_string());
            defs.push_str("\"}}{{template \"d");
            defs.push_str(&(i + 1).to_string());
            defs.push_str("\" .}}{{end}}");
        }
        format!(
            "{defs}{{{{define \"d{depth}\"}}}}{{{{.Name}}}}{{{{end}}}}{{{{template \"d0\" .}}}}"
        )
    };
    ensure_eq(
        &run("t", &calls(99), &data),
        &"ok: x".to_owned(),
        "99 calls",
    )?;
    let deep = run("t", &calls(100), &data);
    ensure_eq(
        &deep.ends_with("exceeded maximum template depth (100000)"),
        &true,
        &deep,
    )
}

/// Go: `fuzzPieces`.
const FUZZ_PIECES: [&str; 82] = [
    "{{",
    "}}",
    "{{- ",
    " -}}",
    " ",
    "\n",
    "text",
    ".",
    ".Name",
    ".Inner.ID",
    ".Tags",
    ".Attrs.k",
    ".Nope",
    ".Count",
    ".N",
    ".Nil",
    ".U8",
    ".Price",
    ".Err.Error",
    ".S",
    ".Arr",
    ".Raw",
    ".hidden",
    ".Bidder",
    ".Nums",
    ".Empty",
    "$",
    "$x",
    "$y",
    "$x :=",
    "$x =",
    "$i, $e :=",
    ":=",
    "=",
    "|",
    "(",
    ")",
    ",",
    "if",
    "else",
    "end",
    "range",
    "with",
    "break",
    "continue",
    "define \"d\"",
    "block \"b\" .",
    "template \"d\"",
    "template \"b\" .",
    "\"s\"",
    "`raw`",
    "'a'",
    "1",
    "-2",
    "0x1F",
    "1.5",
    "1e3",
    "nil",
    "true",
    "false",
    "/*c*/",
    "{{/* c */}}",
    "len",
    "foo",
    "]",
    "#",
    "\"open",
    "'",
    "`",
    "{{.Name}}",
    "{{end}}",
    "{{else}}",
    "{{if .OK}}",
    "{{range .Tags}}",
    "{{with .Inner}}",
    "{{$x := .Count}}",
    "{{$x}}",
    "{{.}}",
    "%",
    "é",
    "{{range .N}}",
    "{{.Name | .Nope}}",
];

fn pick(rng: &mut SplitMix64, n: usize) -> Result<usize, String> {
    let n = u64::try_from(n).map_err(|e| e.to_string())?;
    usize::try_from(rng.next_u64() % n).map_err(|e| e.to_string())
}

#[test]
fn random_templates_match_go() -> Result<(), String> {
    let full = golden::rows(include_str!("../../testdata/go/golden/template_fuzz.txt"))?;
    let data = tdata(false);
    let mut rng = SplitMix64(5);
    let mut sweep = Sweep::default();
    for i in 0..30_000 {
        let mut text = String::new();
        for _ in 0..=rng.next_u64() % 10 {
            text.push_str(
                FUZZ_PIECES
                    .get(pick(&mut rng, FUZZ_PIECES.len())?)
                    .ok_or("piece")?,
            );
        }
        let result = run("fz", &text, &data);
        if let Some(row) = full.get(i) {
            let [want_text, want] = golden::fields::<2>(row)?;
            ensure_eq(&text, &want_text, "fuzz text")?;
            ensure_eq(&result, &want, &format!("fuzz {i}: {text:?}"))?;
        }
        sweep.add(&format!("{text}\t{result}"));
    }
    let sums = include_str!("../../testdata/go/golden/template_sweeps.txt");
    ensure_eq(
        sweep.sum().as_str(),
        golden::checksum(sums, "template_fuzz")?.as_str(),
        "fuzz checksum",
    )
}
