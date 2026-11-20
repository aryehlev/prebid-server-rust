//! The serde code of every ported type against Go's shapes (`testdata/golden/shapes.jsonl`,
//! written by `testdata/go/shapes.go`, which documents the format).
//!
//! Each field's samples, zero value, null and integer extremes are decoded with
//! `pbs_jsonutil::unmarshal` and written with `marshal` (which must write the bytes `serde_json`
//! writes), against `jsonutil.Unmarshal` and `jsonutil.Marshal`, and decoded through
//! `EncodingJson`, against `json.Unmarshal` and `json.Marshal`; samples, null and values of the
//! wrong JSON type are merged with the types' `MergeClone`, against `jsonutil.MergeClone`.
//! Results compare token by token, strings by their text and numbers by value, and every
//! difference must fall in a ledger entry, which the comparison names.

use core::iter::zip;
use std::collections::{BTreeMap, BTreeSet};

use pbs_gojson::gotype::go_type_name;
use pbs_jsonutil::{MergeClone, MergeCloneError, marshal, unmarshal};
use pbs_openrtb::{adcom1, gojson_bridge::EncodingJson, native1, openrtb2, openrtb3};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, value::RawValue};

use super::{
    index::{TYPES, for_each_type},
    support::{TestResult, crate_dir, ensure_eq, read},
};

/// `json.error-texts`: the library message inside Prebid's wrapper is `serde_json`'s.
const ERROR_TEXTS: &str = "json.error-texts";
/// `json.failed-merge-empty-slice`: a merge that fails on a nil slice leaves it empty, where Go
/// leaves it nil.
const FAILED_MERGE_SLICE: &str = "json.failed-merge-empty-slice";
/// `json.html-escape`: jsoniter escapes `<`, `>`, `&`, U+2028 and U+2029 in strings.
const HTML_ESCAPE: &str = "json.html-escape";
/// `json.float-format`: a number spelled differently.
const FLOAT_FORMAT: &str = "json.float-format";
/// `json.nil-raw-written`: Go writes no value for a nil `json.RawMessage`, the port `null`.
const NIL_RAW_WRITTEN: &str = "json.nil-raw-written";
/// `json.null-document`: Go reads the document `null`, serde rejects it.
const NULL_DOCUMENT: &str = "json.null-document";
/// `json.promoted-field-context`: a merge error in a promoted field names the struct that
/// declares it, where Go names the bare field.
const PROMOTED_CONTEXT: &str = "json.promoted-field-context";

/// The classes the goldens' differences fall in, each pinned here; any other difference fails.
const EXPECTED: &[&str] = &[
    ERROR_TEXTS,
    FAILED_MERGE_SLICE,
    FLOAT_FORMAT,
    HTML_ESCAPE,
    NIL_RAW_WRITTEN,
    NULL_DOCUMENT,
    PROMOTED_CONTEXT,
];

/// What one side made of a document: the bytes it wrote, or the decoder's or encoder's error.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// The encoded bytes.
    Encoded(String),
    /// The decoder's error text.
    DecodeFailed(String),
    /// The encoder's error text.
    EncodeFailed(String),
}

impl Outcome {
    /// A golden cell: `!` and the decoder's error, `?` and the encoder's, or the bytes.
    fn from_cell(cell: &str) -> Self {
        match cell.split_at_checked(1) {
            Some(("!", text)) => Self::DecodeFailed(text.to_owned()),
            Some(("?", text)) => Self::EncodeFailed(text.to_owned()),
            _ => Self::Encoded(cell.to_owned()),
        }
    }
}

/// A merge's error and the merged value written.
#[derive(Debug)]
struct Merged {
    /// The merge's error.
    error: Option<MergeCloneError>,
    /// The value after the merge, written.
    encoded: Outcome,
}

/// `value` written with `marshal`, which must write what `serde_json` writes.
fn encoded<T: Serialize>(value: &T) -> TestResult<Outcome> {
    match marshal(value) {
        Ok(bytes) => {
            let direct = serde_json::to_vec(value).map_err(|e| e.to_string())?;
            if direct != bytes {
                return Err(format!(
                    "serde_json writes {} where marshal writes {}",
                    String::from_utf8_lossy(&direct),
                    String::from_utf8_lossy(&bytes)
                ));
            }
            String::from_utf8(bytes)
                .map(Outcome::Encoded)
                .map_err(|e| e.to_string())
        }
        Err(e) => Ok(Outcome::EncodeFailed(e.to_string())),
    }
}

/// `doc` decoded as `T` and written back.
fn decode<T: DeserializeOwned + Serialize>(doc: &str) -> TestResult<Outcome> {
    match unmarshal::<T>(doc.as_bytes()) {
        Ok(value) => encoded(&value),
        Err(e) => Ok(Outcome::DecodeFailed(e.to_string())),
    }
}

/// `doc` decoded as `T` the way `json.Unmarshal` decodes it, and written back.
fn decode_std<T: DeserializeOwned + Serialize>(doc: &str) -> TestResult<Outcome> {
    match unmarshal::<EncodingJson<T>>(doc.as_bytes()) {
        Ok(EncodingJson(value)) => encoded(&value),
        Err(e) => Ok(Outcome::DecodeFailed(e.to_string())),
    }
}

/// `patch` merged into `doc` decoded as `T`.
fn merge<T: DeserializeOwned + Serialize + MergeClone>(
    doc: &str,
    patch: &str,
) -> TestResult<Merged> {
    let mut value = unmarshal::<T>(doc.as_bytes())
        .map_err(|e| format!("the merge target {doc} does not decode: {e}"))?;
    let patch: &RawValue = serde_json::from_str(patch).map_err(|e| e.to_string())?;
    let error = value.merge_clone(patch).err();
    Ok(Merged {
        error,
        encoded: encoded(&value)?,
    })
}

/// The serde code of one ported type.
struct Runner {
    /// Decodes a document and writes it back.
    decode: fn(&str) -> TestResult<Outcome>,
    /// Decodes a document as `json.Unmarshal` does and writes it back.
    decode_std: fn(&str) -> TestResult<Outcome>,
    /// Merges a patch into a decoded document.
    merge: fn(&str, &str) -> TestResult<Merged>,
}

/// The runner of every ported type by Go name; fails unless the names are those of [`TYPES`].
fn runners() -> TestResult<BTreeMap<String, Runner>> {
    let mut by_name = BTreeMap::new();
    macro_rules! runner {
        ($ty:ty) => {{
            let runner = Runner {
                decode: decode::<$ty>,
                decode_std: decode_std::<$ty>,
                merge: merge::<$ty>,
            };
            if by_name.insert(go_type_name::<$ty>(), runner).is_some() {
                return Err(format!(
                    "two Rust types for Go type {}",
                    go_type_name::<$ty>()
                ));
            }
        }};
    }
    for_each_type!(runner);
    let names: BTreeSet<&str> = by_name.keys().map(String::as_str).collect();
    let want: BTreeSet<&str> = TYPES.iter().copied().collect();
    ensure_eq(&names, &want, "the Go names of the ported types")?;
    Ok(by_name)
}

/// One case of the golden.
#[derive(Debug, Clone)]
enum Case {
    /// A document decoded and written back by both Go decoders.
    Decode {
        /// The Go type.
        ty: String,
        /// The case's label.
        label: String,
        /// The document.
        doc: String,
        /// jsoniter's outcome.
        jsoniter: Outcome,
        /// `encoding/json`'s outcome.
        std: Outcome,
    },
    /// A patch merged into a decoded document.
    Merge {
        /// The Go type.
        ty: String,
        /// The case's label.
        label: String,
        /// The document of the merge target.
        doc: String,
        /// The patch.
        patch: String,
        /// `jsonutil.MergeClone`'s error text, empty for none.
        err: String,
        /// The merged value written.
        merged: Outcome,
    },
}

impl Case {
    /// The Go type and label.
    fn name(&self) -> String {
        match self {
            Self::Decode { ty, label, .. } | Self::Merge { ty, label, .. } => {
                format!("{ty} {label}")
            }
        }
    }
}

/// The golden's cases.
fn cases() -> TestResult<Vec<Case>> {
    let golden = crate_dir().join("testdata/golden/shapes.jsonl");
    let text = String::from_utf8(read(&golden)?).map_err(|e| e.to_string())?;
    let mut lines = text.lines();
    let head: Vec<String> =
        serde_json::from_str(lines.next().unwrap_or_default()).map_err(|e| e.to_string())?;
    if head.first().map(String::as_str) != Some("#go") {
        return Err(format!("{}: no [\"#go\", version] line", golden.display()));
    }
    lines
        .enumerate()
        .map(|(n, line)| {
            let row: Vec<String> = serde_json::from_str(line).map_err(|e| e.to_string())?;
            match row.as_slice() {
                [kind, ty, label, doc, jsoniter, std] if kind == "D" => Ok(Case::Decode {
                    ty: ty.clone(),
                    label: label.clone(),
                    doc: doc.clone(),
                    jsoniter: Outcome::from_cell(jsoniter),
                    std: Outcome::from_cell(std),
                }),
                [kind, ty, label, doc, patch, err, merged] if kind == "M" => Ok(Case::Merge {
                    ty: ty.clone(),
                    label: label.clone(),
                    doc: doc.clone(),
                    patch: patch.clone(),
                    err: err.clone(),
                    merged: Outcome::from_cell(merged),
                }),
                _ => Err(format!("shapes.jsonl:{}: bad row {line}", n + 2)),
            }
        })
        .collect()
}

/// A JSON token as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    /// `{`, `}`, `[`, `]`, `:` or `,`.
    Punct(u8),
    /// A string, quotes and escapes included.
    Str(&'a str),
    /// A number.
    Num(&'a str),
    /// `true`, `false` or `null`.
    Lit(&'a str),
}

/// The tokens of `text`, `None` when it is not a sequence of JSON tokens.
fn tokens(text: &str) -> Option<Vec<Token<'_>>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(&b) = bytes.get(at) {
        let end = match b {
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                out.push(Token::Punct(b));
                at + 1
            }
            b' ' | b'\t' | b'\n' | b'\r' => at + 1,
            b'"' => {
                let mut end = at + 1;
                loop {
                    match bytes.get(end)? {
                        b'\\' => end += 2,
                        b'"' => break,
                        _ => end += 1,
                    }
                }
                out.push(Token::Str(text.get(at..=end)?));
                end + 1
            }
            b't' | b'f' | b'n' => {
                let lit = ["true", "false", "null"]
                    .into_iter()
                    .find(|lit| text.get(at..).is_some_and(|rest| rest.starts_with(lit)))?;
                out.push(Token::Lit(lit));
                at + lit.len()
            }
            _ => {
                let len = bytes
                    .get(at..)?
                    .iter()
                    .take_while(|c| b"+-0123456789.eE".contains(c))
                    .count();
                if len == 0 {
                    return None;
                }
                out.push(Token::Num(text.get(at..at + len)?));
                at + len
            }
        };
        at = end;
    }
    Some(out)
}

/// Whether two string tokens hold the same text.
fn same_string(a: &str, b: &str) -> bool {
    let text = |token: &str| serde_json::from_str::<String>(token).ok();
    text(a).is_some() && text(a) == text(b)
}

/// A number token as an exact decimal: whether it is negative, its significant digits and the
/// power of ten of the last one; zero as no digits, of either sign.
fn decimal(token: &str) -> Option<(bool, String, i64)> {
    let (negative, rest) = token
        .strip_prefix('-')
        .map_or((false, token), |rest| (true, rest));
    let (mantissa, exponent) = match rest.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
        None => (rest, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{int}{frac}");
    if int.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let significant = digits.trim_end_matches('0');
    let shift = i64::try_from(digits.len() - significant.len()).ok()?;
    let places = i64::try_from(frac.len()).ok()?;
    let significant = significant.trim_start_matches('0');
    if significant.is_empty() {
        return Some((false, String::new(), 0));
    }
    let power = exponent.checked_add(shift)?.checked_sub(places)?;
    Some((negative, significant.to_owned(), power))
}

/// Whether two number tokens hold the same value, compared as exact decimals: an integer beyond
/// float64's 53 bits that differs in its last digits differs.
fn same_number(a: &str, b: &str) -> bool {
    decimal(a).is_some() && decimal(a) == decimal(b)
}

#[test]
fn numbers_compare_as_exact_decimals() -> TestResult {
    let same = [
        ("1", "1.0"),
        ("1e+21", "1e21"),
        ("100000000000000000000", "1e20"),
        ("0", "-0.0"),
        ("0.5", "5e-1"),
        ("-12.5", "-1.25E1"),
    ];
    for (a, b) in same {
        ensure_eq(&same_number(a, b), &true, &format!("{a} and {b}"))?;
    }
    let different = [
        ("9223372036854775807", "9223372036854775806"),
        ("18446744073709551615", "1.8446744073709552e19"),
        ("1", "-1"),
        ("1.5", "15"),
        ("x", "x"),
    ];
    for (a, b) in different {
        ensure_eq(&same_number(a, b), &false, &format!("{a} and {b}"))?;
    }
    Ok(())
}

/// The ledger entries the difference between two token sequences falls in: strings with the
/// same text and numbers with the same value, spelled differently; `None` for any other
/// difference.
fn lexical(port: &[Token<'_>], go: &[Token<'_>]) -> Option<Vec<&'static str>> {
    if port.len() != go.len() {
        return None;
    }
    let mut classes = Vec::new();
    for (p, g) in zip(port, go) {
        let class = match (p, g) {
            _ if p == g => continue,
            (Token::Str(a), Token::Str(b)) if same_string(a, b) => HTML_ESCAPE,
            (Token::Num(a), Token::Num(b)) if same_number(a, b) => FLOAT_FORMAT,
            _ => return None,
        };
        if !classes.contains(&class) {
            classes.push(class);
        }
    }
    Some(classes)
}

/// The ledger entries a difference between the port's bytes and Go's falls in (none for the
/// same bytes), or what is unexplained.
fn compare_encoded(port: &str, go: &str) -> Result<Vec<&'static str>, String> {
    if port == go {
        return Ok(Vec::new());
    }
    let unexplained = || format!("port {port}, Go {go}");
    if serde_json::from_str::<Value>(go).is_err() {
        // Go writes a nil `json.RawMessage` without `omitempty` as nothing at all.
        let fixed = go
            .replace("\":,", "\":null,")
            .replace("\":}", "\":null}")
            .replace("\":]", "\":null]");
        if serde_json::from_str::<Value>(&fixed).is_err() {
            return Err(format!("{} (not JSON)", unexplained()));
        }
        let mut classes = vec![NIL_RAW_WRITTEN];
        classes.extend(compare_encoded(port, &fixed)?);
        return Ok(classes);
    }
    let (Some(port_tokens), Some(go_tokens)) = (tokens(port), tokens(go)) else {
        return Err(format!("{}: not JSON tokens", unexplained()));
    };
    lexical(&port_tokens, &go_tokens).ok_or_else(unexplained)
}

/// The ledger entries a difference between two outcomes falls in, or what is unexplained.
fn compare(doc: &str, port: &Outcome, go: &Outcome) -> Result<Vec<&'static str>, String> {
    match (port, go) {
        (Outcome::Encoded(p), Outcome::Encoded(g)) => compare_encoded(p, g),
        (Outcome::DecodeFailed(p), Outcome::DecodeFailed(g))
        | (Outcome::EncodeFailed(p), Outcome::EncodeFailed(g)) => Ok(if p == g {
            Vec::new()
        } else {
            vec![ERROR_TEXTS]
        }),
        (Outcome::DecodeFailed(_), Outcome::Encoded(_)) if doc.trim() == "null" => {
            Ok(vec![NULL_DOCUMENT])
        }
        _ => Err(format!("port {port:?}, Go {go:?}")),
    }
}

/// Go's merge error text split into its field (`openrtb2.Imp.Banner`) and its message.
fn go_merge_error(text: &str) -> (Option<&str>, &str) {
    text.strip_prefix("cannot unmarshal ")
        .and_then(|rest| rest.split_once(": "))
        .map_or((None, text), |(context, message)| (Some(context), message))
}

/// The ledger entries a merge case falls in, or what is unexplained.
fn compare_merge(
    merged: &Merged,
    err: &str,
    go_merged: &Outcome,
) -> Result<Vec<&'static str>, String> {
    let mut classes = Vec::new();
    match (&merged.error, err.is_empty()) {
        (None, true) => {}
        (Some(error), false) => {
            let (go_context, message) = go_merge_error(err);
            match (error.context(), go_context) {
                (ours, theirs) if ours == theirs => {}
                (Some(ours), Some(theirs)) if is_promoted_context(ours, theirs) => {
                    classes.push(PROMOTED_CONTEXT);
                }
                (ours, theirs) => {
                    return Err(format!("merge error in {ours:?}, Go's in {theirs:?}"));
                }
            }
            if error.message() != message {
                classes.push(ERROR_TEXTS);
            }
        }
        (error, _) => return Err(format!("merge error {error:?}, Go's {err:?}")),
    }
    match compare("", &merged.encoded, go_merged) {
        Ok(found) => classes.extend(found),
        Err(e) => {
            let nil = emptied_slices_as_nil(merged).ok_or(e)?;
            classes.push(FAILED_MERGE_SLICE);
            classes.extend(compare("", &nil, go_merged)?);
        }
    }
    Ok(classes)
}

/// After a merge that failed, the merged value with its empty slices written as Go writes nil
/// ones, `None` when the merge succeeded or has no empty slices.
fn emptied_slices_as_nil(merged: &Merged) -> Option<Outcome> {
    match &merged.encoded {
        Outcome::Encoded(text) if merged.error.is_some() && text.contains(":[]") => {
            Some(Outcome::Encoded(text.replace(":[]", ":null")))
        }
        Outcome::Encoded(_) | Outcome::DecodeFailed(_) | Outcome::EncodeFailed(_) => None,
    }
}

/// Whether the port's merge error names `Type.Field` where Go's names the bare field, as jsoniter
/// does for a field promoted from an embedded struct.
fn is_promoted_context(port: &str, go: &str) -> bool {
    !go.contains('.') && port.strip_suffix(go).is_some_and(|ty| ty.ends_with('.'))
}

/// The ledger entries one case falls in, or what is unexplained.
fn check_case(
    runners: &BTreeMap<String, Runner>,
    case: &Case,
) -> Result<Vec<&'static str>, String> {
    match case {
        Case::Decode {
            ty,
            doc,
            jsoniter,
            std,
            ..
        } => {
            let runner = runners
                .get(ty)
                .ok_or_else(|| format!("no Rust type for {ty}"))?;
            let port = (runner.decode)(doc)?;
            let mut classes =
                compare(doc, &port, jsoniter).map_err(|e| format!("jsoniter: {e}"))?;
            let port_std = (runner.decode_std)(doc)?;
            classes
                .extend(compare(doc, &port_std, std).map_err(|e| format!("encoding/json: {e}"))?);
            Ok(classes)
        }
        Case::Merge {
            ty,
            doc,
            patch,
            err,
            merged,
            ..
        } => {
            let runner = runners
                .get(ty)
                .ok_or_else(|| format!("no Rust type for {ty}"))?;
            compare_merge(&(runner.merge)(doc, patch)?, err, merged)
        }
    }
}

/// Every case: the ledger entries the differences fall in, with the number of cases of each,
/// and what is unexplained.
fn run() -> TestResult<(BTreeMap<&'static str, usize>, Vec<String>)> {
    let runners = runners()?;
    let mut classes = BTreeMap::new();
    let mut unexplained = Vec::new();
    for case in cases()? {
        match check_case(&runners, &case) {
            Ok(found) => {
                for class in found {
                    *classes.entry(class).or_insert(0) += 1;
                }
            }
            Err(e) => unexplained.push(format!("{}: {e}", case.name())),
        }
    }
    Ok((classes, unexplained))
}

/// Every case of the golden matches Go, or differs in the ledger entries of [`EXPECTED`], each of
/// which some case shows.
#[test]
fn shapes_match_go() -> TestResult {
    let cases = cases()?;
    let types: BTreeSet<&str> = cases
        .iter()
        .map(|case| match case {
            Case::Decode { ty, .. } | Case::Merge { ty, .. } => ty.as_str(),
        })
        .collect();
    ensure_eq(
        &types,
        &TYPES.iter().copied().collect(),
        "the types the golden has cases for",
    )?;
    let (classes, unexplained) = run()?;
    if !unexplained.is_empty() {
        return Err(format!(
            "{} cases differ from Go outside the ledger:\n{}",
            unexplained.len(),
            unexplained
                .iter()
                .take(40)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    ensure_eq(
        &classes.keys().copied().collect::<Vec<_>>(),
        &EXPECTED.to_vec(),
        &format!("the ledger entries the differences fall in ({classes:?})"),
    )
}

/// The document and jsoniter's outcome of the decode case of `ty` labelled `label`.
fn decode_case(cases: &[Case], ty: &str, label: &str) -> TestResult<(String, Outcome)> {
    cases
        .iter()
        .find_map(|case| match case {
            Case::Decode {
                ty: t,
                label: l,
                doc,
                jsoniter,
                ..
            } if t == ty && l == label => Some((doc.clone(), jsoniter.clone())),
            Case::Decode { .. } | Case::Merge { .. } => None,
        })
        .ok_or_else(|| format!("no decode case {ty} {label}"))
}

/// `json.null-into-value`, restored: every field given null, and every field null at once, decodes
/// to jsoniter's value, a value field keeping its zero value.
#[test]
fn null_members_keep_zero_values() -> TestResult {
    let runners = runners()?;
    let cases = cases()?;
    let mut checked = 0;
    for case in &cases {
        let Case::Decode {
            ty,
            label,
            doc,
            jsoniter,
            ..
        } = case
        else {
            continue;
        };
        if !label.ends_with(" null") || doc == "null" {
            continue;
        }
        let runner = runners
            .get(ty)
            .ok_or_else(|| format!("no Rust type for {ty}"))?;
        let port = (runner.decode)(doc)?;
        if matches!(port, Outcome::DecodeFailed(_)) || compare(doc, &port, jsoniter).is_err() {
            return Err(format!("{ty} {label}: port {port:?}, Go {jsoniter:?}"));
        }
        checked += 1;
    }
    if checked < 1000 {
        return Err(format!("only {checked} null cases"));
    }
    // A value field given null keeps its zero value: `openrtb2.Imp`'s `id` and `bidfloor`.
    let (doc, jsoniter) = decode_case(&cases, "openrtb2.Imp", "bidfloor null")?;
    let imp: openrtb2::Imp = unmarshal(doc.as_bytes()).map_err(|e| e.to_string())?;
    ensure_eq(
        &imp,
        &openrtb2::Imp::default(),
        "openrtb2.Imp given bidfloor null",
    )?;
    ensure_eq(&decode::<openrtb2::Imp>(&doc)?, &jsoniter, "written back")
}

/// `json.null-raw`, restored: where the two Go decoders part over a `json.RawMessage` given null,
/// the generated types read nil, as `jsonutil.Unmarshal` does, and through `EncodingJson` the
/// bytes `null`, as `json.Unmarshal` does, each written back as its Go column writes it.
#[test]
fn raw_null_follows_the_decoder() -> TestResult {
    let runners = runners()?;
    let only =
        |classes: &[&str], allowed: &[&str]| classes.iter().all(|class| allowed.contains(class));
    let mut checked = 0;
    for case in cases()? {
        let Case::Decode {
            ty,
            label,
            doc,
            jsoniter,
            std,
        } = &case
        else {
            continue;
        };
        let parted =
            matches!((jsoniter, std), (Outcome::Encoded(a), Outcome::Encoded(b)) if a != b);
        if !label.ends_with(" null") || !parted {
            continue;
        }
        let runner = runners
            .get(ty)
            .ok_or_else(|| format!("no Rust type for {ty}"))?;
        let nil = compare(doc, &(runner.decode)(doc)?, jsoniter)?;
        let kept = compare(doc, &(runner.decode_std)(doc)?, std)?;
        if !only(&nil, &[FLOAT_FORMAT, HTML_ESCAPE, NIL_RAW_WRITTEN])
            || !only(&kept, &[FLOAT_FORMAT, HTML_ESCAPE])
        {
            return Err(format!(
                "{ty} {label}: {nil:?} against jsoniter, {kept:?} against encoding/json"
            ));
        }
        checked += 1;
    }
    if checked < 100 {
        return Err(format!(
            "only {checked} cases where the Go decoders part over null"
        ));
    }
    Ok(())
}

/// `json.promoted-field-context`: a merge that fails in a field `adcom1.App`, `Site` or `Dooh`
/// promote from `adcom1.DistributionChannel` names `adcom1.DistributionChannel.<Field>`, where Go
/// names the bare field, and leaves Go's value.
#[test]
fn merge_errors_name_promoted_fields_with_their_struct() -> TestResult {
    let runners = runners()?;
    let cases = cases()?;
    let mut checked = 0;
    for case in &cases {
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
        let (Some(context), _) = go_merge_error(err) else {
            continue;
        };
        if context.contains('.') {
            continue;
        }
        let runner = runners
            .get(ty)
            .ok_or_else(|| format!("no Rust type for {ty}"))?;
        let port = (runner.merge)(doc, patch)?;
        let want = format!("adcom1.DistributionChannel.{context}");
        ensure_eq(
            &port.error.as_ref().and_then(MergeCloneError::context),
            &Some(want.as_str()),
            &format!("{ty} {label}: the field the error names"),
        )?;
        ensure_eq(
            &compare("", &port.encoded, merged),
            &Ok(Vec::new()),
            &format!("{ty} {label}: the merged value"),
        )?;
        checked += 1;
    }
    // id, name, pub and content of adcom1.App, Site and DOOH.
    ensure_eq(&checked, &12, "the promoted-field merge errors")
}

/// `json.failed-merge-empty-slice`: a merge of a value that is not an array into a nil slice fails
/// on both sides; Go leaves the slice nil, which it writes as `null`, the port empty.
#[test]
fn failed_merges_leave_nil_slices_empty() -> TestResult {
    let runners = runners()?;
    let mut found = Vec::new();
    for case in cases()? {
        let Case::Merge {
            ty,
            label,
            doc,
            patch,
            merged,
            ..
        } = &case
        else {
            continue;
        };
        let runner = runners
            .get(ty)
            .ok_or_else(|| format!("no Rust type for {ty}"))?;
        let port = (runner.merge)(doc, patch)?;
        if compare("", &port.encoded, merged).is_ok() {
            continue;
        }
        let nil = emptied_slices_as_nil(&port)
            .ok_or_else(|| format!("{ty} {label}: {port:?}, Go {merged:?}"))?;
        ensure_eq(
            &compare("", &nil, merged),
            &Ok(Vec::new()),
            &format!("{ty} {label} with its empty slices nil"),
        )?;
        found.push(format!("{ty} {label}"));
    }
    ensure_eq(
        &found,
        &[
            "openrtb2.Audio mimes wrong",
            "openrtb2.BidRequest imp wrong",
            "openrtb2.SeatBid bid wrong",
            "openrtb2.SupplyChain nodes wrong",
            "openrtb2.Video mimes wrong",
            "openrtb3.Request item wrong",
            "adcom1.NativeFormat asset wrong",
            "request.EventTracker methods wrong",
            "request.Request assets wrong",
        ]
        .map(str::to_owned)
        .to_vec(),
        "the merges that leave a nil slice empty",
    )
}
