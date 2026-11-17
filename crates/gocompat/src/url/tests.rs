//! Replays the `url_*.txt` goldens written by `testdata/go/golden/url.go`.

use super::*;
use crate::{
    golden::{self, SplitMix64, Sweep},
    testutil::ensure_eq,
};

fn bool_text(b: bool) -> Vec<u8> {
    b.to_string().into_bytes()
}

fn err_bytes<T, E: fmt::Display>(r: &Result<T, E>) -> Vec<u8> {
    golden::err_text(r).into_bytes()
}

/// Go: `urlFields`. `None` when the Go generator skips the result (invalid UTF-8 decoded into
/// the host or userinfo, which the Rust port holds as `U+FFFD`).
fn url_fields(func: &str, input: &str) -> Option<Vec<Vec<u8>>> {
    let parsed = if func == "Parse" {
        Url::parse(input)
    } else {
        Url::parse_request_uri(input)
    };
    let head = |err: Vec<u8>| vec![func.as_bytes().to_vec(), input.as_bytes().to_vec(), err];
    let u = match parsed {
        Ok(u) => u,
        Err(e) => return Some(head(err_bytes::<(), _>(&Err(e)))),
    };
    let replaced = |s: &str| s.contains(char::REPLACEMENT_CHARACTER);
    if replaced(&u.host)
        || u.user
            .as_ref()
            .is_some_and(|ui| replaced(ui.username()) || ui.password().is_some_and(replaced))
    {
        return None;
    }
    let user = u.user.as_ref().map_or_else(
        || "nil".to_owned(),
        |ui| {
            format!(
                "{}:{}:{}:{ui}",
                ui.username(),
                ui.password().unwrap_or_default(),
                ui.password().is_some()
            )
        },
    );
    let mut out = head(Vec::new());
    out.extend(
        [
            u.scheme.clone(),
            u.opaque.clone(),
            user,
            u.host.clone(),
            u.path.clone(),
            u.raw_path.clone(),
        ]
        .map(String::into_bytes),
    );
    out.push(bool_text(u.omit_host));
    out.push(bool_text(u.force_query));
    out.extend(
        [
            u.raw_query.clone(),
            u.fragment.clone(),
            u.raw_fragment.clone(),
            u.to_string(),
            u.escaped_path(),
            u.escaped_fragment(),
            u.hostname().to_owned(),
            u.port().to_owned(),
            u.request_uri(),
        ]
        .map(String::into_bytes),
    );
    out.push(bool_text(u.is_abs()));
    out.push(u.query().encode().into_bytes());
    Some(out)
}

/// Go: `queryFields`.
fn query_fields(input: &str) -> Vec<Vec<u8>> {
    let (v, err) = parse_query_pair(input);
    let err = err.map_or_else(Vec::new, |e| format!("error: {e}").into_bytes());
    vec![
        b"ParseQuery".to_vec(),
        input.as_bytes().to_vec(),
        err,
        v.encode().into_bytes(),
        v.len().to_string().into_bytes(),
    ]
}

/// Go: `unescapeFields`.
fn unescape_fields(input: &str) -> Vec<Vec<u8>> {
    let q = query_unescape_bytes(input);
    let p = path_unescape_bytes(input);
    vec![
        b"Unescape".to_vec(),
        input.as_bytes().to_vec(),
        q.clone().unwrap_or_default(),
        err_bytes(&q),
        p.clone().unwrap_or_default(),
        err_bytes(&p),
    ]
}

/// Go: `urlLine`.
fn line(fields: &[Vec<u8>]) -> Vec<u8> {
    let quoted: Vec<String> = fields.iter().map(|f| quote_bytes(f)).collect();
    quoted.join("\t").into_bytes()
}

fn utf8(field: &[u8]) -> Result<&str, String> {
    str::from_utf8(field).map_err(|e| e.to_string())
}

#[test]
fn parse_rows_match_go() -> Result<(), String> {
    let rows = golden::rows_bytes(include_str!("../../testdata/go/golden/url_parse.txt"))?;
    for row in &rows {
        let [Some(func), Some(input)] = [row.first(), row.get(1)] else {
            return Err(format!("short row {row:?}"));
        };
        let got = url_fields(utf8(func)?, utf8(input)?);
        ensure_eq(&got.as_ref(), &Some(row), utf8(input)?)?;
    }
    ensure_eq(&(rows.len() > 90), &true, "row count")
}

/// Go writes the parameter-limit inputs as `<N params>` and their encodings as `<N bytes>`.
fn expand_params(input: &str) -> Option<String> {
    let n: usize = input
        .strip_prefix('<')?
        .strip_suffix(" params>")?
        .parse()
        .ok()?;
    Some(format!("{}a", "a&".repeat(n - 1)))
}

#[test]
fn query_rows_match_go() -> Result<(), String> {
    for row in golden::rows(include_str!("../../testdata/go/golden/url_query.txt"))? {
        let [_, input, err, encoded, count] = golden::fields::<5>(&row)?;
        let (input, abbreviated) = expand_params(&input).map_or((input, false), |s| (s, true));
        let got = query_fields(&input);
        let mut want = vec![
            b"ParseQuery".to_vec(),
            input.into_bytes(),
            err.into_bytes(),
            encoded.into_bytes(),
            count.into_bytes(),
        ];
        if abbreviated {
            let len = got.get(3).map_or(0, Vec::len);
            let w = want.get_mut(3).ok_or("no encode field")?;
            ensure_eq(
                w.as_slice(),
                format!("<{len} bytes>").as_bytes(),
                "encoded length",
            )?;
            w.clone_from(got.get(3).ok_or("no encode field")?);
        }
        ensure_eq(got.as_slice(), want.as_slice(), "ParseQuery")?;
    }
    Ok(())
}

#[test]
fn escape_rows_match_go() -> Result<(), String> {
    let rows = golden::rows_bytes(include_str!("../../testdata/go/golden/url_escape.txt"))?;
    for row in &rows {
        let [Some(s), Some(q), Some(p)] = [row.first(), row.get(1), row.get(2)] else {
            return Err(format!("short row {row:?}"));
        };
        ensure_eq(
            query_escape_bytes(s).as_bytes(),
            q.as_slice(),
            "QueryEscape",
        )?;
        ensure_eq(path_escape_bytes(s).as_bytes(), p.as_slice(), "PathEscape")?;
        if let Ok(text) = str::from_utf8(s) {
            ensure_eq(
                query_escape(text).as_bytes(),
                q.as_slice(),
                "QueryEscape str",
            )?;
            ensure_eq(path_escape(text).as_bytes(), p.as_slice(), "PathEscape str")?;
        }
    }
    ensure_eq(&rows.len(), &261, "row count")
}

#[test]
fn unescape_rows_match_go() -> Result<(), String> {
    for row in golden::rows_bytes(include_str!("../../testdata/go/golden/url_unescape.txt"))? {
        let input = utf8(row.get(1).ok_or("short row")?)?;
        ensure_eq(unescape_fields(input).as_slice(), row.as_slice(), input)?;
    }
    ensure_eq(
        &query_unescape("a%e2%82").as_deref(),
        &Ok("a\u{FFFD}\u{FFFD}"),
        "lossy QueryUnescape",
    )
}

#[test]
fn values_match_go() -> Result<(), String> {
    let rows = golden::rows(include_str!("../../testdata/go/golden/url_values.txt"))?;
    let mut v = Values::new();
    v.add("b", "2");
    v.add("a", "1 2");
    v.set("c", "x&y=z");
    v.add("a", "3");
    v.insert("empty", Vec::new());
    let build = vec![
        "build".to_owned(),
        v.encode(),
        v.get("a").to_owned(),
        v.has("empty").to_string(),
        v.has("d").to_string(),
    ];
    ensure_eq(&rows.first(), &Some(&build), "build")?;
    v.del("a");
    v.set("b", "\u{20ac}");
    let modify = vec![
        "modify".to_owned(),
        v.encode(),
        v.get("a").to_owned(),
        v.has("a").to_string(),
        v.get("b").to_owned(),
    ];
    ensure_eq(&rows.get(1), &Some(&modify), "modify")
}

/// Go: the `fill` closure of `genURL`.
fn fill(sweep: &mut Sweep, input: &str) {
    for func in ["Parse", "ParseRequestURI"] {
        if let Some(f) = url_fields(func, input) {
            sweep.add_bytes(&line(&f));
        }
    }
    sweep.add_bytes(&line(&query_fields(input)));
    sweep.add_bytes(&line(&unescape_fields(input)));
    sweep.add_bytes(&line(&[
        b"Escape".to_vec(),
        query_escape(input).into_bytes(),
        path_escape(input).into_bytes(),
    ]));
}

const SWEEP_TOKENS: [&str; 60] = [
    "a",
    "Z",
    "0",
    "9",
    "-",
    ".",
    "_",
    "~",
    ":",
    "/",
    "?",
    "#",
    "[",
    "]",
    "@",
    "!",
    "$",
    "&",
    "'",
    "(",
    ")",
    "*",
    "+",
    ",",
    ";",
    "=",
    "%",
    "%2",
    "%25",
    "%41",
    "%zz",
    "%ff",
    "%e2%82%ac",
    " ",
    "\x7f",
    "\u{e9}",
    "\u{20ac}",
    "http",
    "https:",
    "http://",
    "x://",
    "mailto:",
    "//",
    "::1",
    "1.2.3.4",
    "localhost",
    ":8080",
    "%2F",
    "%3A",
    "\"",
    "<",
    ">",
    "\\",
    "{",
    "|",
    "^",
    "`",
    "{{.Host}}",
    "a=b",
    "&c=d",
];

#[test]
fn url_sweeps_match_go() -> Result<(), String> {
    let sums = include_str!("../../testdata/go/golden/url_sweeps.txt");
    let mut corpus = Sweep::default();
    let inputs = golden::rows(include_str!("../../testdata/go/golden/url_corpus.txt"))?;
    for row in &inputs {
        fill(&mut corpus, row.first().ok_or("empty corpus row")?);
    }
    ensure_eq(&(inputs.len() > 1000), &true, "corpus size")?;
    ensure_eq(
        corpus.sum().as_str(),
        golden::checksum(sums, "url_corpus")?.as_str(),
        "corpus",
    )?;

    let tokens = SWEEP_TOKENS;
    let count = u64::try_from(tokens.len()).map_err(|e| e.to_string())?;
    let mut rng = SplitMix64(2);
    let mut random = Sweep::default();
    for _ in 0..50_000 {
        let n = rng.next_u64() % 12 + 1;
        let mut s = String::new();
        for _ in 0..n {
            let pick = usize::try_from(rng.next_u64() % count).map_err(|e| e.to_string())?;
            s.push_str(tokens.get(pick).ok_or("token")?);
        }
        fill(&mut random, &s);
    }
    ensure_eq(
        random.sum().as_str(),
        golden::checksum(sums, "url_random")?.as_str(),
        "random",
    )
}
