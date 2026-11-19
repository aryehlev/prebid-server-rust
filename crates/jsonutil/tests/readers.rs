//! The seeded corpora of `testdata/golden/readers.json` (`testdata/go/gen/readers.go`), 50,000
//! cases each, compared through their batches' digests: strings with escapes and surrogates as
//! `jsonutil.Unmarshal` reads them, raw values as `jsonutil.Marshal` compacts them, messages in
//! jsoniter's shape through `tryExtractErrorMessage`, and the values `StringInt`, `IntString` and
//! `ParseIntoString` read. A case that meets a ledgered difference has the entry's name as its
//! outcome on both sides, and the test checks the Rust side of it.

#[cfg(test)]
pub mod seeded;
#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fmt::Write as _};

    use pbs_jsonutil::{
        IntString, StringInt, marshal, parse_into_string, raw_compact, try_extract_error_message,
        unmarshal, unmarshal_valid,
    };
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, value::RawValue};

    use super::{
        seeded::{SplitMix64, check_digests},
        support::{TestResult, ensure_eq, golden, list},
    };

    /// The corpora of `readers.json`, by the name `go run ./gen -dump` takes, in key order.
    const SETS: [&str; 6] = [
        "compact",
        "extract",
        "int_string",
        "parse_into_string",
        "string_int",
        "strings",
    ];

    /// Go's `hexOf`: the lower-case hex of each text's bytes, joined by commas.
    fn hex_of<S: AsRef<str>>(texts: &[S]) -> String {
        let parts: Vec<String> = texts
            .iter()
            .map(|text| {
                text.as_ref().bytes().fold(String::new(), |mut out, byte| {
                    let _written = write!(out, "{byte:02x}");
                    out
                })
            })
            .collect();
        parts.join(",")
    }

    /// Go's `backslashU`, which starts a `\u` escape.
    const BACKSLASH_U: &str = "\\u";

    /// Go's `strPlain`.
    const PLAIN: [&str; 7] = ["a", "b", " ", "\u{e9}", "\u{1f600}", "\u{7f}", "<"];

    /// Go's `strEscapes`.
    const ESCAPES: [&str; 5] = ["\\\\", "\\n", "\\\"", "\\/", "\\t"];

    /// Go's `strUnits`: code units of every kind, lone surrogates among them.
    const UNITS: [&str; 13] = [
        "0041", "00e9", "001f", "0000", "d800", "dbff", "dc00", "dfff", "d83d", "de00", "fffd",
        "2028", "ffff",
    ];

    /// Go's `strUnits[:4]`, the code units every Go reader decodes alike.
    const PLAIN_UNITS: [&str; 4] = ["0041", "00e9", "001f", "0000"];

    /// Go's `strHigh`.
    const HIGH: [&str; 3] = ["d800", "dbff", "d83d"];

    /// Go's `strLow`.
    const LOW: [&str; 3] = ["dc00", "dfff", "de00"];

    /// Go's `strInvalid`: escapes that are not JSON.
    const INVALID: [&str; 4] = ["\\x", "\\u12", "\\uzzzz", "\\"];

    /// Go's `strInvalid[:3]`: without the lone backslash, which would escape the backslash of a
    /// pair after it.
    const INVALID_WHOLE: [&str; 3] = ["\\x", "\\u12", "\\uzzzz"];

    /// Go's `strUnit`: a `\u` escape of one of `units`, upper-case half of the time.
    fn unit(rng: &mut SplitMix64, units: &[&str]) -> String {
        let digits = rng.pick(units);
        if rng.below(2) == 1 {
            format!("{BACKSLASH_U}{}", digits.to_ascii_uppercase())
        } else {
            format!("{BACKSLASH_U}{digits}")
        }
    }

    /// Go's `strContent`: the inside of a JSON string. Go draws a plain character both for
    /// `k <= 4` and for `k == 15` when `invalid` is false; without `surrogates`, an escape that is
    /// not JSON is never a lone backslash.
    fn content(rng: &mut SplitMix64, surrogates: bool, invalid: bool) -> String {
        let mut out = String::new();
        for _ in 0..rng.below(6) {
            match rng.below(16) {
                5 | 6 => out.push_str(rng.pick(&ESCAPES)),
                7..=11 => {
                    let units: &[&str] = if surrogates { &UNITS } else { &PLAIN_UNITS };
                    out.push_str(&unit(rng, units));
                }
                12..=14 => {
                    let high = unit(rng, &HIGH);
                    let low = unit(rng, &LOW);
                    out.push_str(&high);
                    out.push_str(&low);
                }
                15 if invalid => {
                    let escapes: &[&str] = if surrogates { &INVALID } else { &INVALID_WHOLE };
                    out.push_str(rng.pick(escapes));
                }
                _ => out.push_str(rng.pick(&PLAIN)),
            }
        }
        out
    }

    /// Go's `stringTarget`.
    #[derive(Deserialize)]
    struct StringTarget {
        s: String,
    }

    /// One case of Go's `stringsCorpus`: a string as a member, as a key or in an array, and the
    /// bytes `unmarshal` reads, or `error`.
    fn strings_case(rng: &mut SplitMix64) -> (String, String) {
        match rng.below(5) {
            0..=2 => {
                let doc = format!(r#"{{"s":"{}"}}"#, content(rng, true, true));
                let outcome = unmarshal::<StringTarget>(doc.as_bytes())
                    .map_or_else(|_| "error".to_owned(), |target| hex_of(&[target.s]));
                (doc, outcome)
            }
            3 => {
                let doc = format!(r#"{{"{}":1}}"#, content(rng, true, true));
                let outcome = unmarshal::<BTreeMap<String, i64>>(doc.as_bytes()).map_or_else(
                    |_| "error".to_owned(),
                    |map| hex_of(&map.into_keys().collect::<Vec<_>>()),
                );
                (doc, outcome)
            }
            _ => {
                let n = 1 + rng.below(3);
                let parts: Vec<String> = (0..n)
                    .map(|_| format!("\"{}\"", content(rng, true, true)))
                    .collect();
                let doc = format!("[{}]", parts.join(","));
                let outcome = unmarshal::<Vec<String>>(doc.as_bytes())
                    .map_or_else(|_| "error".to_owned(), |strings| hex_of(&strings));
                (doc, outcome)
            }
        }
    }

    /// Go's `cmpNumbers`: spellings both writers keep.
    const NUMBERS: [&str; 10] = [
        "0",
        "-0",
        "12",
        "-7",
        "2.50",
        "1E+2",
        "1e-7",
        "3.0",
        "123456789012345678901234567890",
        "1.5e300",
    ];

    /// Go's `cmpStrings`.
    const STRINGS: [&str; 8] = [
        r#""a b""#,
        r#""x  y <&>""#,
        r#""""#,
        r#""\"q\"""#,
        r#""\\""#,
        "\"\u{e9}\"",
        concat!("\"\\", "u00e9 \\/\""),
        concat!("\"\\", "ud83d\\", "ude00\""),
    ];

    /// Go's `cmpKeys`.
    const KEYS: [&str; 4] = [r#""a""#, r#""b""#, r#""a b""#, r#""""#];

    /// Go's `cmpSpace`.
    fn space(rng: &mut SplitMix64) -> &'static str {
        match rng.below(6) {
            3 => " ",
            4 => "\n  ",
            5 => "\t\r\n",
            _ => "",
        }
    }

    /// Go's `cmpValue`: a JSON value with whitespace between its tokens.
    fn value(rng: &mut SplitMix64, depth: usize) -> String {
        let mut n = rng.below(10);
        if depth >= 3 && n >= 7 {
            n = rng.below(7);
        }
        match n {
            0 => "null".to_owned(),
            1 => "true".to_owned(),
            2 => "false".to_owned(),
            3 | 4 => rng.pick(&NUMBERS).to_owned(),
            5 | 6 => rng.pick(&STRINGS).to_owned(),
            7 | 8 => {
                let mut members = Vec::new();
                for _ in 0..rng.below(4) {
                    let key = rng.pick(&KEYS);
                    let before_key = space(rng);
                    let after_key = space(rng);
                    let after_colon = space(rng);
                    let member = value(rng, depth + 1);
                    let after_member = space(rng);
                    members.push(format!(
                        "{before_key}{key}{after_key}:{after_colon}{member}{after_member}"
                    ));
                }
                let before_end = space(rng);
                format!("{{{}{before_end}}}", members.join(","))
            }
            _ => {
                let mut elements = Vec::new();
                for _ in 0..rng.below(4) {
                    let before = space(rng);
                    let element = value(rng, depth + 1);
                    let after = space(rng);
                    elements.push(format!("{before}{element}{after}"));
                }
                let before_end = space(rng);
                format!("[{}{before_end}]", elements.join(","))
            }
        }
    }

    /// Go's `rawOmit`, as typegen writes a `json.RawMessage` field with `omitempty`.
    #[derive(Serialize, Deserialize)]
    struct RawOmit {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            serialize_with = "raw_compact"
        )]
        ext: Option<Box<RawValue>>,
    }

    /// One case of Go's `compactCorpus`: a raw value with whitespace between its tokens, and what
    /// `marshal` writes of the member holding it.
    fn compact_case(rng: &mut SplitMix64) -> TestResult<(String, String)> {
        let before = space(rng);
        let raw = value(rng, 1);
        let after = space(rng);
        let doc = format!(r#"{{"ext":{before}{raw}{after}}}"#);
        let outcome = match unmarshal::<RawOmit>(doc.as_bytes()) {
            Ok(omit) => String::from_utf8(marshal(&omit)?)?,
            Err(error) => format!("error: {}", error.message()),
        };
        Ok((doc, outcome))
    }

    /// Go's `extractOps`.
    const OPERATIONS: [&str; 9] = [
        "openrtb2.Imp.Banner",
        "openrtb_ext.ExtRequestPrebid.Cache",
        "readObjectStart",
        "ReadString",
        "Skip",
        "unmarshalerDecoder",
        "main.T.F",
        "a b",
        "",
    ];

    /// Go's `extractDetails`.
    const DETAILS: [&str; 8] = [
        "expect { or n, but found m",
        "request.imp[0].ext is invalid",
        "do not know how to skip: 109",
        "x: y",
        "",
        "value: with: colons",
        "request.",
        "found in #3",
    ];

    /// Go's `extractSafe`: contexts without a `: `.
    const SAFE_CONTEXTS: [&str; 4] = ["abc", "", r#"{"a":1}"#, ", error found in #9"];

    /// Go's `extractAll`.
    const CONTEXTS: [&str; 6] = [
        "abc",
        "",
        r#"{"a":1}"#,
        ", error found in #9",
        "a: b",
        "x : y",
    ];

    /// Go's `extractMessage`: operations, a detail and, most of the time, jsoniter's context.
    fn message(rng: &mut SplitMix64) -> String {
        let k = rng.below(4);
        let operations: Vec<&str> = (0..k).map(|_| rng.pick(&OPERATIONS)).collect();
        let detail = rng.pick(&DETAILS);
        let msg = if k > 0 {
            format!("{}: {detail}", operations.join(": "))
        } else {
            detail.to_owned()
        };
        if rng.below(4) == 0 {
            return msg;
        }
        let contexts: &[&str] = if k == 0 && !detail.contains(": ") {
            &SAFE_CONTEXTS
        } else {
            &CONTEXTS
        };
        let at = rng.below(100);
        let first = rng.pick(contexts);
        let bigger = rng.pick(contexts);
        format!(
            "{msg}, error found in #{at} byte of ...|{first}|..., bigger context ...|{bigger}|..."
        )
    }

    /// Go's `sinDigits`: `n` digits, the first not a zero unless `leading_zero`, or `0` for none.
    fn digits(rng: &mut SplitMix64, n: usize, leading_zero: bool) -> String {
        if n == 0 {
            return "0".to_owned();
        }
        let mut out = String::new();
        for i in 0..n {
            let drawn = rng.below(10);
            let digit = if i == 0 && drawn == 0 && !leading_zero {
                1
            } else {
                drawn
            };
            out.push(
                u32::try_from(digit)
                    .ok()
                    .and_then(|d| char::from_digit(d, 10))
                    .unwrap_or('0'),
            );
        }
        out
    }

    /// Go's `sinNumber`: an integer of up to 21 digits, now and then with a fraction or exponent.
    fn int_number(rng: &mut SplitMix64) -> String {
        let sign = if rng.below(3) == 1 { "-" } else { "" };
        let n = rng.below(22);
        let mut text = format!("{sign}{}", digits(rng, n, false));
        if rng.below(8) == 0 {
            text.push_str(rng.pick(&[".5", "e2", ".0"]));
        }
        text
    }

    /// Go's `stringIntQuirk`: the spellings of `json.string-int-parse`, a lone minus and a
    /// leading plus.
    fn string_int_quirk(json: &str) -> bool {
        let text = json
            .strip_prefix('"')
            .and_then(|inner| inner.strip_suffix('"'))
            .unwrap_or(json);
        text == "-" || text.starts_with('+')
    }

    /// Go's `stringIntItem`.
    #[derive(Deserialize)]
    struct StringIntItem {
        #[serde(default)]
        item_id: StringInt,
    }

    /// One case of Go's `stringIntCorpus`: the integer read, `error`, or the quirk, whose Rust
    /// reading is checked here (a lone minus fails, a plus sign reads its digits).
    fn string_int_case(rng: &mut SplitMix64) -> TestResult<(String, String)> {
        let json = match rng.below(4) {
            0 => int_number(rng),
            1 => match rng.below(6) {
                0 => {
                    let n = rng.below(22);
                    format!("\"{}\"", digits(rng, n, true))
                }
                1 => r#""""#.to_owned(),
                2 => r#""-""#.to_owned(),
                3 => {
                    let n = 1 + rng.below(5);
                    format!("\"+{}\"", digits(rng, n, true))
                }
                4 => format!(
                    "\"{}\"",
                    rng.pick(&[" 5", "0x1", "1.5", "abc", "--1", "-12"])
                ),
                _ => format!("\"{BACKSLASH_U}0033\""),
            },
            2 => rng.pick(&["true", "false", "null"]).to_owned(),
            _ => rng.pick(&["[]", "{}", "[1]"]).to_owned(),
        };
        let doc = format!(r#"{{"item_id":{json}}}"#);
        let read = unmarshal_valid::<StringIntItem>(doc.as_bytes()).map(|item| item.item_id.0);
        if string_int_quirk(&json) {
            let ledgered = json.trim_matches('"').strip_prefix('+').map_or_else(
                || read.is_err(),
                |number| read.as_ref().ok().copied() == number.parse::<i64>().ok(),
            );
            if !ledgered {
                return Err(
                    format!("{doc}: {read:?} is not json.string-int-parse's reading").into(),
                );
            }
            return Ok((doc, "quirk string-int-parse".to_owned()));
        }
        let outcome = read.map_or_else(|_| "error".to_owned(), |n| n.to_string());
        Ok((doc, outcome))
    }

    /// Go's `istExponents`.
    const EXPONENTS: [&str; 8] = ["", "", "e2", "E+2", "e-7", "e308", "e-320", "e400"];

    /// Go's `istNumber`: an integer of up to 30 digits, or a decimal with an exponent, up to
    /// beyond `f64`'s range.
    fn any_number(rng: &mut SplitMix64) -> String {
        let sign = if rng.below(3) == 1 { "-" } else { "" };
        if rng.below(2) == 0 {
            let n = rng.below(31);
            return format!("{sign}{}", digits(rng, n, false));
        }
        let n = rng.below(4);
        let mut text = format!("{sign}{}", digits(rng, n, false));
        if rng.below(2) == 0 {
            let fraction = 1 + rng.below(4);
            text.push('.');
            text.push_str(&digits(rng, fraction, true));
        }
        text.push_str(rng.pick(&EXPONENTS));
        text
    }

    /// Go's `outOfRange`: a number jsoniter's skipper rejects (`json.out-of-range-numbers`), one
    /// that starts with 0 beyond `f32`'s range or one with an exponent beyond `f64`'s.
    fn out_of_range(json: &str) -> bool {
        if json.starts_with('0') {
            !json.parse::<f32>().is_ok_and(f32::is_finite)
        } else {
            json.starts_with(|c: char| c == '-' || c.is_ascii_digit())
                && json.contains(['e', 'E'])
                && !json.parse::<f64>().is_ok_and(f64::is_finite)
        }
    }

    /// Go's `intStringItem`.
    #[derive(Deserialize)]
    struct IntStringItem {
        #[serde(default)]
        item_id: IntString,
    }

    /// One case of Go's `intStringCorpus`: the text read, `error`, or the quirk, whose Rust
    /// reading is checked here: gjson's text, `+Inf` or `-Inf` beyond `f64`'s range, where
    /// jsoniter fails the document.
    fn int_string_case(rng: &mut SplitMix64) -> TestResult<(String, String)> {
        let json = match rng.below(5) {
            0 | 1 => any_number(rng),
            2 => format!("\"{}\"", content(rng, false, true)),
            3 => rng.pick(&["true", "false", "null"]).to_owned(),
            _ => rng.pick(&["[]", "{}", "[1]"]).to_owned(),
        };
        let doc = format!(r#"{{"item_id":{json}}}"#);
        let read = unmarshal_valid::<IntStringItem>(doc.as_bytes()).map(|item| item.item_id.0);
        if out_of_range(&json) {
            let infinity = if json.starts_with('-') {
                "-Inf"
            } else {
                "+Inf"
            };
            let ledgered = read.as_deref().is_ok_and(|text| {
                if json.parse::<f64>().is_ok_and(f64::is_finite) {
                    !text.ends_with("Inf")
                } else {
                    text == infinity
                }
            });
            if !ledgered {
                return Err(
                    format!("{doc}: {read:?} is not json.out-of-range-numbers' reading").into(),
                );
            }
            return Ok((doc, "quirk out-of-range-numbers".to_owned()));
        }
        let outcome = read.map_or_else(|_| "error".to_owned(), |text| hex_of(&[text]));
        Ok((doc, outcome))
    }

    /// Go's `pisSpaces`.
    const SPACES: [&str; 5] = ["", "", " ", "\n", "\t "];

    /// Go's `pisValue`: strings without lone surrogates or escapes that are not JSON, numbers,
    /// literals, objects and arrays, or nothing.
    fn loose_value(rng: &mut SplitMix64) -> String {
        match rng.below(6) {
            0 => format!("\"{}\"", content(rng, false, false)),
            1 => int_number(rng),
            2 => any_number(rng),
            3 => rng.pick(&["true", "false", "null"]).to_owned(),
            4 => rng
                .pick(&[
                    r#"{"a": 1}"#,
                    "[1, 2]",
                    "{}",
                    "[]",
                    r#"{"a":[1,{"b":null}]}"#,
                ])
                .to_owned(),
            _ => String::new(),
        }
    }

    /// One case of Go's `parseIntoCorpus`: the text set, `nil`, or `error`.
    fn parse_into_case(rng: &mut SplitMix64) -> (String, String) {
        let before = rng.pick(&SPACES);
        let json = loose_value(rng);
        let after = rng.pick(&SPACES);
        let input = format!("{before}{json}{after}");
        let mut target = None;
        let outcome = match parse_into_string(input.as_bytes(), Some(&mut target)) {
            Err(_) => "error".to_owned(),
            Ok(()) => target.map_or_else(|| "nil".to_owned(), |text| hex_of(&[text])),
        };
        (input, outcome)
    }

    /// The golden records the six corpora, 50 batches of 1,000 cases each.
    // Go: crates/jsonutil/testdata/go/gen/readers.go readersGolden
    #[test]
    fn corpora_are_recorded() -> TestResult {
        let doc = golden("readers.json")?;
        let sets: Vec<&str> = doc
            .pointer("/seeded")
            .and_then(Value::as_object)
            .ok_or("the golden has no seeded member")?
            .keys()
            .map(String::as_str)
            .collect();
        ensure_eq(sets.as_slice(), SETS.as_slice(), "corpora")?;
        for set in SETS {
            ensure_eq(
                &list(&doc, &format!("/seeded/{set}/batches"))?.len(),
                &50,
                set,
            )?;
        }
        Ok(())
    }

    /// 50,000 strings of plain characters, escapes, lone surrogates, pairs and escapes that are
    /// not JSON, as a member, as a key and in arrays: `unmarshal` reads the bytes
    /// `jsonutil.Unmarshal` reads and fails where it fails, each lone surrogate a U+FFFD as
    /// jsoniter pairs them (`json.lone-surrogates`, restored).
    // Go: util/jsonutil/jsonutil.go:136-146 (Unmarshal); gen/readers.go stringsCorpus
    #[test]
    fn seeded_strings_match_go() -> TestResult {
        let doc = golden("readers.json")?;
        check_digests(&doc, "/seeded/strings", "strings", |rng| {
            Ok(strings_case(rng))
        })
    }

    /// 50,000 raw values with whitespace between their tokens, repeated keys and every number
    /// and string spelling kept: `raw_compact` writes what Prebid's `RawMessageExtension` writes
    /// (`json.raw-not-compacted`, restored).
    // Go: util/jsonutil/jsonutil.go:223-255 (RawMessageExtension); gen/readers.go compactCorpus
    #[test]
    fn seeded_raw_values_compact_like_go() -> TestResult {
        let doc = golden("readers.json")?;
        check_digests(&doc, "/seeded/compact", "compact", compact_case)
    }

    /// 50,000 messages in jsoniter's shape: `try_extract_error_message` cuts and prefixes each as
    /// `tryExtractErrorMessage` does.
    // Go: util/jsonutil/jsonutil.go:172-220 (tryExtractErrorMessage); gen/readers.go extractCorpus
    #[test]
    fn seeded_messages_extract_like_go() -> TestResult {
        let doc = golden("readers.json")?;
        check_digests(&doc, "/seeded/extract", "extract", |rng| {
            let msg = message(rng);
            let extracted = try_extract_error_message(&msg);
            Ok((msg, extracted))
        })
    }

    /// 50,000 `StringInt` values (numbers, strings of digits and others, literals, arrays and
    /// objects) read or fail as Go's do, except the spellings of `json.string-int-parse`.
    // Go: util/jsonutil/stringInt.go:9-29; gen/readers.go stringIntCorpus
    #[test]
    fn seeded_string_ints_match_go() -> TestResult {
        let doc = golden("readers.json")?;
        check_digests(&doc, "/seeded/string_int", "string_int", string_int_case)
    }

    /// 50,000 `IntString` values (integers of up to 30 digits, decimals with exponents, strings,
    /// literals, arrays and objects) read gjson's text or fail as Go's do, except a number beyond
    /// `f64`'s range (`json.out-of-range-numbers`).
    // Go: util/jsonutil/intstring.go:11-19; gen/readers.go intStringCorpus
    #[test]
    fn seeded_int_strings_match_go() -> TestResult {
        let doc = golden("readers.json")?;
        check_digests(&doc, "/seeded/int_string", "int_string", int_string_case)
    }

    /// 50,000 `ParseIntoString` inputs with space around the value: the text gjson gives, an
    /// object's or array's bytes to the end of the input, `+Inf` and `-Inf` beyond `f64`'s range,
    /// and nothing for null and blank inputs.
    // Go: util/jsonutil/forcestring.go:11-20; gen/readers.go parseIntoCorpus
    #[test]
    fn seeded_parse_into_string_matches_go() -> TestResult {
        let doc = golden("readers.json")?;
        check_digests(
            &doc,
            "/seeded/parse_into_string",
            "parse_into_string",
            |rng| Ok(parse_into_case(rng)),
        )
    }
}
