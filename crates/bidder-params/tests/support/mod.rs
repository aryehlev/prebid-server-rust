//! What the differential test files share.
//!
//! Reading the goldens, the validator as Go builds it, the comparison of one params document
//! with Go's outcome, and the inputs on which the port is known to differ from Go, each a
//! ledgered difference.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use pbs_bidder_params::{
    BidderParamValidator as _, ParamsError, StandardBidderParamValidator,
    gojsonschema::{ResultError, Schema, decode_json_using_number},
    new_bidder_params_validator,
};
use pbs_devtools::repo::go_root;
use serde_json::Value;

/// The goldens' directory.
const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden");

/// Every line of the golden `name`, decoded.
pub(super) fn read_lines(name: &str) -> Result<Vec<Value>, String> {
    let text =
        fs::read_to_string(format!("{GOLDEN_DIR}/{name}")).map_err(|e| format!("{name}: {e}"))?;
    text.lines()
        .map(|line| serde_json::from_str(line).map_err(|e| format!("{name}: {e}: {line}")))
        .collect()
}

/// The string `key` of a golden record.
pub(super) fn text<'r>(record: &'r Value, key: &str) -> Option<&'r str> {
    record.get(key).and_then(Value::as_str)
}

/// The string `key` of a golden record, which it must have.
pub(super) fn required<'r>(record: &'r Value, key: &str) -> Result<&'r str, String> {
    text(record, key).ok_or_else(|| format!("golden record without {key}: {record}"))
}

/// The strings of the array `key` of a golden record.
pub(super) fn strings(record: &Value, key: &str) -> Vec<String> {
    record
        .get(key)
        .and_then(Value::as_array)
        .map_or_default(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
}

/// A byte string stored as text (`key`) or, when it is not UTF-8, as hex (`hex_key`).
pub(super) fn bytes(record: &Value, key: &str, hex_key: &str) -> Result<Option<Vec<u8>>, String> {
    if let Some(text) = text(record, key) {
        return Ok(Some(text.as_bytes().to_vec()));
    }
    let Some(hex) = text(record, hex_key) else {
        return Ok(None);
    };
    hex.as_bytes()
        .chunks(2)
        .map(|pair| {
            str::from_utf8(pair)
                .ok()
                .and_then(|p| u8::from_str_radix(p, 16).ok())
                .ok_or_else(|| format!("bad hex in golden: {hex}"))
        })
        .collect::<Result<Vec<u8>, String>>()
        .map(Some)
}

/// The error strings of a result, sorted.
pub(super) fn sorted_strings(errors: &[ResultError]) -> Vec<String> {
    let mut out: Vec<String> = errors.iter().map(ToString::to_string).collect();
    out.sort();
    out
}

/// The error strings of a result, joined as Prebid joins them.
pub(super) fn joined(errors: &[ResultError]) -> String {
    errors.iter().map(ToString::to_string).collect()
}

/// Whether Go's joined `text` is the port's `errors` in another order, which Go's random walk
/// of maps decides where an object of the document has several members: the order in which
/// gojsonschema parsed a schema's `properties` and the order of the document's keys.
pub(super) fn reordered(text: &str, errors: &[ResultError], doc: &[u8]) -> bool {
    fn consume(text: &str, errors: &[String], used: &mut [bool]) -> bool {
        if text.is_empty() {
            return used.iter().all(|u| *u);
        }
        for (i, error) in errors.iter().enumerate() {
            if used.get(i) == Some(&false)
                && let Some(rest) = text.strip_prefix(error.as_str())
            {
                if let Some(u) = used.get_mut(i) {
                    *u = true;
                }
                if consume(rest, errors, used) {
                    return true;
                }
                if let Some(u) = used.get_mut(i) {
                    *u = false;
                }
            }
        }
        false
    }
    fn several_members(value: &Value) -> bool {
        match value {
            Value::Object(members) => members.len() > 1 || members.values().any(several_members),
            Value::Array(items) => items.iter().any(several_members),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
        }
    }
    let strings: Vec<String> = errors.iter().map(ToString::to_string).collect();
    let mut used = vec![false; strings.len()];
    decode_json_using_number(doc).is_ok_and(|value| several_members(&value))
        && consume(text, &strings, &mut used)
}

/// Whether a schema fails to load because it is not JSON to `serde_json`, which words the
/// error its own way (jsonschema.decode-errors).
pub(super) fn undecodable(schema: &[u8]) -> bool {
    decode_json_using_number(schema).is_err()
}

/// The validator built like Go builds it (bidders and aliases from validator.json), the schema
/// file of every bidder, and the directory of the schemas.
pub(super) struct Fixture {
    pub(super) validator: StandardBidderParamValidator,
    pub(super) files: BTreeMap<String, String>,
    pub(super) schema_dir: String,
    schemas: BTreeMap<String, Schema>,
}

impl Fixture {
    /// Loads the validator from the Go root's `static/bidder-params`.
    pub(super) fn load() -> Result<Self, String> {
        let text = fs::read_to_string(format!("{GOLDEN_DIR}/validator.json"))
            .map_err(|e| format!("validator.json: {e}"))?;
        let golden: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let bidders: BTreeSet<String> = strings(&golden, "bidders").into_iter().collect();
        let mut aliases = BTreeMap::new();
        let mut files = BTreeMap::new();
        for (key, target) in [("aliases", &mut aliases), ("schemas", &mut files)] {
            let entries = golden
                .get(key)
                .and_then(Value::as_object)
                .ok_or_else(|| format!("validator.json without {key}"))?;
            for (name, value) in entries {
                if let Some(value) = value.as_str() {
                    let _previous = target.insert(name.clone(), value.to_owned());
                }
            }
        }
        if bidders.len() < 300 || aliases.len() < 50 {
            return Err(format!(
                "too few bidders ({}) or aliases ({})",
                bidders.len(),
                aliases.len()
            ));
        }
        let dir = go_root()
            .map_err(|e| e.to_string())?
            .join("static")
            .join("bidder-params");
        let schema_dir = dir
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("{} is not UTF-8", dir.display()))?;
        let validator = new_bidder_params_validator(&schema_dir, &bidders, &aliases)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            validator,
            files,
            schema_dir,
            schemas: BTreeMap::new(),
        })
    }

    /// The errors of the bidder's schema for `ext`.
    fn errors(&mut self, bidder: &str, ext: &[u8]) -> Result<Vec<ResultError>, String> {
        let file = self.files.get(bidder).cloned().unwrap_or_default();
        if !self.schemas.contains_key(&file) {
            let bytes =
                fs::read(format!("{}/{file}", self.schema_dir)).map_err(|e| e.to_string())?;
            let _previous = self.schemas.insert(file.clone(), Schema::new(&bytes)?);
        }
        let schema = self.schemas.get(&file).ok_or("schema cache")?;
        let result = schema.validate(ext).map_err(|e| e.to_string())?;
        Ok(result.errors().to_vec())
    }

    /// Compares the validator's verdict on one golden bidder record with Go's.
    pub(super) fn compare(&mut self, record: &Value) -> Result<Verdict, String> {
        let bidder = required(record, "bidder")?;
        let ext = bytes(record, "ext", "ext_hex")?.unwrap_or_default();
        let kind = required(record, "kind")?;
        let outputs = strings(record, "outputs");
        let got = self.validator.validate(bidder, &ext);
        let same = match (kind, &got) {
            // A decode error's text is serde_json's (jsonschema.decode-errors).
            ("ok", Ok(()))
            | ("panic", Err(ParamsError::Panic(_)))
            | ("decode", Err(ParamsError::Decode(_))) => true,
            ("invalid", Err(ParamsError::Invalid(text))) if outputs.contains(text) => true,
            // Go reported these errors in several orders, or in an order its walk of maps
            // decided: compare them as a multiset.
            ("invalid", Err(ParamsError::Invalid(_))) => {
                let errors = self.errors(bidder, &ext)?;
                (outputs.len() > 1 && sorted_strings(&errors) == strings(record, "errors"))
                    || outputs.iter().any(|o| reordered(o, &errors, &ext))
            }
            _ => false,
        };
        if same {
            return Ok(Verdict::Same);
        }
        if let Some(entry) = known_difference(&ext) {
            return Ok(Verdict::Ledgered(entry));
        }
        Ok(Verdict::Differs(format!(
            "{bidder} {:?}: Go {kind} {outputs:?}, Rust {got:?}",
            String::from_utf8_lossy(&ext)
        )))
    }
}

/// How one case compares with Go.
#[derive(Debug)]
pub(super) enum Verdict {
    /// As Go.
    Same,
    /// Differs, on an input a ledgered difference names.
    Ledgered(&'static str),
    /// Differs: what and how.
    Differs(String),
}

/// Tallies the verdicts of one golden file.
#[derive(Debug, Default)]
pub(super) struct Tally {
    cases: usize,
    ledgered: BTreeMap<&'static str, usize>,
    failures: Vec<String>,
}

impl Tally {
    /// Records one verdict.
    pub(super) fn add(&mut self, verdict: Verdict) {
        self.cases += 1;
        match verdict {
            Verdict::Same => {}
            Verdict::Ledgered(entry) => *self.ledgered.entry(entry).or_default() += 1,
            Verdict::Differs(what) => self.failures.push(what),
        }
    }

    /// Fails on any difference no ledger entry names, or on fewer than `minimum` cases.
    pub(super) fn finish(&self, what: &str, minimum: usize) -> Result<(), String> {
        if self.cases < minimum {
            return Err(format!("only {} {what} cases", self.cases));
        }
        if self.failures.is_empty() {
            return Ok(());
        }
        let shown: Vec<&str> = self.failures.iter().take(40).map(String::as_str).collect();
        Err(format!(
            "{} of {} {what} cases differ from Go (ledgered: {:?}):\n{}",
            self.failures.len(),
            self.cases,
            self.ledgered,
            shown.join("\n")
        ))
    }
}

/// The ledgered difference, if any, that `doc` (a schema or params document) may show: bytes
/// that are not UTF-8 (json.invalid-utf8), nesting of 128 levels (json.nesting-depth), a
/// surrogate escape before an escape it does not pair with (json.surrogate-pairing), a number
/// with a leading zero (jsonschema.decode-errors), or a number `f64` does not hold exactly
/// (jsonschema.number-precision).
pub(super) fn known_difference(doc: &[u8]) -> Option<&'static str> {
    if str::from_utf8(doc).is_err() {
        return Some("json.invalid-utf8");
    }
    let scan = Scan::of(doc);
    if scan.depth >= 128 {
        return Some("json.nesting-depth");
    }
    if scan.unpaired_escapes {
        return Some("json.surrogate-pairing");
    }
    if scan.numbers.iter().any(|n| leading_zero(n)) {
        return Some("jsonschema.decode-errors");
    }
    if scan.numbers.iter().any(|n| imprecise(n)) {
        return Some("jsonschema.number-precision");
    }
    None
}

/// What [`known_difference`] reads from a document, outside its strings.
struct Scan<'d> {
    depth: usize,
    numbers: Vec<&'d str>,
    unpaired_escapes: bool,
}

impl<'d> Scan<'d> {
    fn of(doc: &'d [u8]) -> Self {
        let mut scan = Self {
            depth: 0,
            numbers: Vec::new(),
            unpaired_escapes: false,
        };
        let mut level = 0usize;
        let mut i = 0;
        while let Some(&b) = doc.get(i) {
            match b {
                b'"' => i = scan.string(doc, i + 1),
                b'[' | b'{' => {
                    level += 1;
                    scan.depth = scan.depth.max(level);
                    i += 1;
                }
                b']' | b'}' => {
                    level = level.saturating_sub(1);
                    i += 1;
                }
                b'-' | b'0'..=b'9' => {
                    let end = doc
                        .get(i..)
                        .and_then(|rest| {
                            rest.iter().position(|c| {
                                !matches!(c, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                            })
                        })
                        .map_or(doc.len(), |n| i + n);
                    if let Some(token) = doc.get(i..end).and_then(|t| str::from_utf8(t).ok()) {
                        scan.numbers.push(token);
                    }
                    i = end;
                }
                _ => i += 1,
            }
        }
        scan
    }

    /// Reads the string whose contents start at `start`; returns the offset after it.
    fn string(&mut self, doc: &[u8], start: usize) -> usize {
        let mut i = start;
        let mut previous_surrogate: Option<u16> = None;
        while let Some(&b) = doc.get(i) {
            match b {
                b'"' => return i + 1,
                b'\\' if doc.get(i + 1) == Some(&b'u') => {
                    let unit = doc
                        .get(i + 2..i + 6)
                        .and_then(|h| str::from_utf8(h).ok())
                        .and_then(|h| u16::from_str_radix(h, 16).ok());
                    if let (Some(first), Some(second)) = (previous_surrogate, unit) {
                        let pairs =
                            (0xD800..0xDC00).contains(&first) && (0xDC00..0xE000).contains(&second);
                        if !pairs && (0xD800..0xE000).contains(&second) {
                            self.unpaired_escapes = true;
                        }
                    }
                    previous_surrogate = unit.filter(|u| (0xD800..0xE000).contains(u));
                    i += 6;
                    continue;
                }
                b'\\' => i += 2,
                _ => i += 1,
            }
            previous_surrogate = None;
        }
        i
    }
}

/// A number with a leading zero (`01`, `-00`), which Go's stream decoder reads as `0` at the
/// top level and `serde_json` refuses.
fn leading_zero(token: &str) -> bool {
    let digits = token.strip_prefix('-').unwrap_or(token).as_bytes();
    matches!(digits, [b'0', b'0'..=b'9', ..])
}

/// A number `serde_json` does not hold as Go's `big.Rat` does: an integer outside `i64` and
/// `u64`, a value beyond `f64`'s range or below its precision, a negative zero (one `f64` for
/// `-0` and `-0.0`), or more significant digits than `f64` keeps (15).
fn imprecise(token: &str) -> bool {
    let unsigned = token.strip_prefix('-').unwrap_or(token);
    let mantissa = unsigned.split(['e', 'E']).next().unwrap_or_default();
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let significant = digits.trim_start_matches('0').trim_end_matches('0');
    if significant.len() > 15 {
        return true;
    }
    let integer = !token.contains(['.', 'e', 'E']);
    if integer && token.parse::<i64>().is_err() && token.parse::<u64>().is_err() {
        return true;
    }
    token.parse::<f64>().is_ok_and(|f| {
        !f.is_finite() || (f == 0.0 && (!significant.is_empty() || f.is_sign_negative()))
    })
}
