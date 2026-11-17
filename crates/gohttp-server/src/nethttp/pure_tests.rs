//! The pure functions against `testdata/golden/pure.golden`, which `testdata/go/pure` writes
//! from Go's own for generated inputs: `http.DetectContentType`, `httprouter.CleanPath`,
//! `path.Clean`, `textproto.CanonicalMIMEHeaderKey`, `mime.ParseMediaType` and
//! `http.StatusText`.

use core::fmt::Write as _;
use std::{collections::BTreeMap, fs};

use pbs_gocompat::strconv::{quote, quote_bytes, unquote_bytes};

use super::{
    bufio::canonical_mime_header_key_bytes, helpers::clean_path, mime::parse_media_type,
    sniff::detect_content_type, status::status_text,
};
use crate::httprouter;

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden/pure.golden");

/// Splits `"<quoted>" "<quoted>"` into its two unquoted parts.
fn split_quoted(rest: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
    let b = rest.as_bytes();
    if b.first() != Some(&b'"') {
        return Err(format!("no quoted input in {rest}"));
    }
    let mut end = 1;
    while let Some(&c) = b.get(end) {
        match c {
            b'\\' => end += 2,
            b'"' => break,
            _ => end += 1,
        }
    }
    let first = rest
        .get(..=end)
        .ok_or_else(|| format!("unterminated input in {rest}"))?;
    let second = rest
        .get(end + 2..)
        .ok_or_else(|| format!("no output in {rest}"))?;
    let unquote = |q: &str| unquote_bytes(q).map_err(|e| format!("{q}: {e}"));
    Ok((unquote(first)?, unquote(second)?))
}

fn utf8(b: &[u8]) -> Result<&str, String> {
    str::from_utf8(b).map_err(|e| format!("{}: {e}", quote_bytes(b)))
}

/// Go: the generator's `mediaType`: the type, the sorted parameters and the error.
fn media_type(input: &str) -> String {
    let (mut out, params, err) = match parse_media_type(input) {
        Ok((mt, params)) => (mt, params, "<nil>"),
        Err(e) => (e.media_type, BTreeMap::new(), e.text),
    };
    for (k, v) in &params {
        let _ok = write!(out, " {k}={}", quote(v));
    }
    let _ok = write!(out, " err={err}");
    out
}

fn call(func: &str, input: &[u8]) -> Result<Vec<u8>, String> {
    Ok(match func {
        "sniff" => detect_content_type(input).as_bytes().to_vec(),
        "routerclean" => httprouter::clean_path(utf8(input)?).into_bytes(),
        "pathclean" => clean_path(utf8(input)?).into_bytes(),
        // Go's exported function returns an invalid key unchanged.
        "canonkey" => canonical_mime_header_key_bytes(input)
            .map_or_else(|| input.to_vec(), String::into_bytes),
        "mediatype" => media_type(utf8(input)?).into_bytes(),
        "statustext" => {
            let code: u16 = utf8(input)?.parse().map_err(|e| format!("code: {e}"))?;
            status_text(code).as_bytes().to_vec()
        }
        _ => return Err(format!("unknown function {func}")),
    })
}

#[test]
fn pure_functions_match_go() -> Result<(), String> {
    let text = fs::read_to_string(GOLDEN).map_err(|e| format!("{GOLDEN}: {e}"))?;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut report = String::new();
    let mut failures = 0_usize;
    for (no, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |e: String| format!("{GOLDEN}:{}: {e}", no + 1);
        let (func, rest) = line.split_once(' ').ok_or_else(|| at(line.to_owned()))?;
        let (input, want) = split_quoted(rest).map_err(at)?;
        let got = call(func, &input).map_err(at)?;
        *counts.entry(func).or_default() += 1;
        if got != want {
            failures += 1;
            if failures <= 40 {
                let _ok = write!(
                    report,
                    "\n{func}({}) = {}, Go: {}",
                    quote_bytes(&input),
                    quote_bytes(&got),
                    quote_bytes(&want)
                );
            }
        }
    }
    let total: usize = counts.values().sum();
    if total < 20_000 || counts.len() < 6 {
        return Err(format!("only {counts:?} lines in {GOLDEN}"));
    }
    if failures > 0 {
        return Err(format!(
            "{failures} of {total} results differ from Go:{report}"
        ));
    }
    Ok(())
}
