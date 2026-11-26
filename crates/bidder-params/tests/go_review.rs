//! Break-it differential tests of the review: replays `testdata/golden/review_sweeps.jsonl`,
//! which `testdata/go/review` (a Go program running gojsonschema v1.2.0) wrote. Each line is a
//! sweep of 50,000 cases from a `SplitMix64` generator seeded as the line says: schemas an
//! operator's bidder-params file could hold, which no Prebid schema spells, and documents
//! against them. Go hashed the cases and their outcomes, and the port must give the same two
//! digests.
//!
//! - `limits`: numbers of at most 15 significant digits, and integers `i64` or `u64` holds, as
//!   limits and as documents: the texts that print a limit (`Must be less than or equal to 1e+06`),
//!   the type a number is given as, and the verdicts that compare two numbers.
//! - `strings`: enums and property names made of HTML characters, controls, line separators, astral
//!   characters and lone surrogate escapes, each spelled raw or escaped: the texts that print an
//!   enum's values, a property name and a context.
//!
//! Neither sweep reaches a ledgered difference: no number needs more than `f64`'s precision
//! (jsonschema.number-precision), every document is JSON (jsonschema.decode-errors), no lone
//! surrogate escape precedes another escape (json.surrogate-pairing), and every document has one
//! member, so Go's map order does not change the errors.

#[cfg(test)]
mod tests {
    use std::fs;

    use pbs_bidder_params::gojsonschema::{Schema, ValidateError};
    use serde_json::Value;
    use sha2::{Digest as _, Sha256};

    /// Go: `outside`: a document no generated enum holds (the pool has no `x`).
    const OUTSIDE: &str = r#""xxxxxxxxxxxxxxxxxxxx""#;

    /// Go: `wideNumbers`.
    const WIDE_NUMBERS: [&str; 31] = [
        "0",
        "-0",
        "0.0",
        "-0.0",
        "0e5",
        "1",
        "-1",
        "1e0",
        "1E+2",
        "1e-2",
        "100",
        "1e5",
        "1e6",
        "999999",
        "1000000",
        "0.0001",
        "0.00001",
        "1.5e-5",
        "123456789012345",
        "-123456789012345",
        "9007199254740992",
        "-9007199254740992",
        "9223372036854775807",
        "-9223372036854775808",
        "18446744073709551615",
        "1e20",
        "1e21",
        "1e-7",
        "1.0",
        "10.50",
        "2.5e+3",
    ];

    /// Go: `exponents`.
    const EXPONENTS: [&str; 6] = ["e", "E", "e+", "e-", "E+", "E-"];

    /// Go: `loneHigh` and `loneLow`, the lone surrogates of the pool, written as escapes.
    const LONE_HIGH: u32 = 0xD800;
    const LONE_LOW: u32 = 0xDC00;

    /// Go: `pool`.
    const POOL: [u32; 35] = [
        0x61, 0x5A, 0x30, 0x20, 0x2E, 0x3C, 0x3E, 0x26, 0x22, 0x5C, 0x2F, 0x27, 0x7B, 0x7D, 0x00,
        0x01, 0x08, 0x09, 0x0A, 0x0C, 0x0D, 0x1F, 0x7F, 0x80, 0x9F, 0xE9, 0x0301, 0x200B, 0x2028,
        0x2029, 0xFEFF, 0xFFFD, 0x1_F600, LONE_HIGH, LONE_LOW,
    ];

    /// Go: `splitmix64`, the generator both sides run.
    struct SplitMix64(u64);

    impl SplitMix64 {
        const fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Go: `below(n)`: `next() % n`.
        fn below(&mut self, n: usize) -> usize {
            let n = u64::try_from(n).unwrap_or(u64::MAX);
            usize::try_from(self.next() % n).unwrap_or(usize::MAX)
        }

        /// Go: `digits`: `n` random decimal digits, the first one not 0 when `non_zero_first`.
        fn digits(&mut self, n: usize, non_zero_first: bool) -> String {
            let mut out = String::with_capacity(n);
            for i in 0..n {
                let digit = if i == 0 && non_zero_first {
                    1 + self.below(9)
                } else {
                    self.below(10)
                };
                let digit = u32::try_from(digit).unwrap_or_default();
                out.push(char::from_digit(digit, 10).unwrap_or('0'));
            }
            out
        }

        /// Go: `number`: a JSON number of at most 15 significant digits or, when `wide`, one of
        /// [`WIDE_NUMBERS`].
        fn number(&mut self, wide: bool) -> String {
            match self.below(if wide { 4 } else { 3 }) {
                0 => {
                    let sign = if self.below(2) == 0 { "-" } else { "" };
                    let n = 1 + self.below(15);
                    format!("{sign}{}", self.digits(n, true))
                }
                1 => {
                    let n = i64::try_from(self.below(2001)).unwrap_or_default();
                    (n - 1000).to_string()
                }
                2 => {
                    let sign = if self.below(2) == 0 { "-" } else { "" };
                    let total = 1 + self.below(15);
                    let point = self.below(total + 1);
                    let mut text = String::from(sign);
                    if point == 0 {
                        text.push_str("0.");
                        text.push_str(&self.digits(total, false));
                    } else {
                        text.push_str(&self.digits(point, true));
                        if point < total {
                            text.push('.');
                            text.push_str(&self.digits(total - point, false));
                        }
                    }
                    if self.below(2) == 0 {
                        let exponent = EXPONENTS.get(self.below(EXPONENTS.len()));
                        text.push_str(exponent.copied().unwrap_or("e"));
                        text.push_str(&self.below(21).to_string());
                    }
                    text
                }
                _ => {
                    let number = WIDE_NUMBERS.get(self.below(WIDE_NUMBERS.len()));
                    number.copied().unwrap_or("0").to_owned()
                }
            }
        }

        /// Go: `escape`: `\u` and the four hex digits of `unit`, each in a random case.
        fn escape(&mut self, unit: u32, out: &mut String) {
            out.push_str("\\u");
            for shift in [12, 8, 4, 0] {
                let digit = char::from_digit((unit >> shift) & 0xF, 16).unwrap_or('0');
                out.push(if self.below(2) == 0 {
                    digit
                } else {
                    digit.to_ascii_uppercase()
                });
            }
        }

        /// Go: `char`: one character of the pool, raw or escaped.
        fn char(&mut self, out: &mut String) {
            let unit = POOL.get(self.below(POOL.len())).copied().unwrap_or(0x61);
            if unit == LONE_HIGH || unit == LONE_LOW {
                self.escape(unit, out);
                out.push('q');
                return;
            }
            let Some(c) = char::from_u32(unit) else {
                return;
            };
            if unit > 0xFFFF {
                if self.below(2) == 0 {
                    out.push(c);
                } else {
                    let mut buf = [0; 2];
                    for half in c.encode_utf16(&mut buf) {
                        self.escape(u32::from(*half), out);
                    }
                }
            } else if c == '"' || c == '\\' || unit < 0x20 {
                let short = short(c);
                if !short.is_empty() && self.below(2) == 0 {
                    out.push_str(short);
                } else {
                    self.escape(unit, out);
                }
            } else if c == '/' {
                match self.below(3) {
                    0 => out.push('/'),
                    1 => out.push_str("\\/"),
                    _ => self.escape(unit, out),
                }
            } else if self.below(2) == 0 {
                out.push(c);
            } else {
                self.escape(unit, out);
            }
        }

        /// Go: `str`: a JSON string of 0 to 6 characters of the pool.
        fn str(&mut self) -> String {
            let mut out = String::from('"');
            for _ in 0..self.below(7) {
                self.char(&mut out);
            }
            out.push('"');
            out
        }
    }

    /// Go: `short`: the two-character escape JSON has for `c`, if any.
    const fn short(c: char) -> &'static str {
        match c {
            '"' => "\\\"",
            '\\' => "\\\\",
            '\u{8}' => "\\b",
            '\u{c}' => "\\f",
            '\n' => "\\n",
            '\r' => "\\r",
            '\t' => "\\t",
            _ => "",
        }
    }

    /// Go: `outcome`: one validation, as both sides spell it.
    fn outcome(schema: &Schema, doc: &str) -> String {
        match schema.validate(doc.as_bytes()) {
            Ok(result) if result.valid() => "ok".to_owned(),
            Ok(result) => {
                let mut fields = vec!["invalid".to_owned()];
                fields.extend(result.errors().iter().map(ToString::to_string));
                fields.join("\t")
            }
            Err(ValidateError::Load(text)) => format!("decode {text}"),
            Err(ValidateError::Panic(panic)) => format!("panic {panic}"),
        }
    }

    /// Go: `outcomes`: loads `schema` and validates each document, or gives the load error.
    fn outcomes(schema: &str, docs: &[&str]) -> Vec<String> {
        match Schema::new(schema.as_bytes()) {
            Ok(loaded) => docs.iter().map(|doc| outcome(&loaded, doc)).collect(),
            Err(text) => vec![format!("load_error {text}")],
        }
    }

    /// Go: `limitsCase`: a limit, a second limit and a document, and their outcomes.
    fn limits_case(rng: &mut SplitMix64) -> (Vec<String>, Vec<String>) {
        let limit = rng.number(true);
        let other = rng.number(false);
        let doc = rng.number(false);
        let mut out = outcomes(
            &format!(r#"{{"maximum":{limit}}}"#),
            &["1e300", &limit, &doc],
        );
        out.extend(outcomes(
            &format!(r#"{{"minimum":{limit}}}"#),
            &["-1e300", &limit, &doc],
        ));
        out.extend(outcomes(r#"{"type":"string"}"#, &[&limit]));
        out.extend(outcomes(r#"{"type":"integer"}"#, &[&limit]));
        out.extend(outcomes(
            &format!(r#"{{"maximum":{limit},"minimum":{other}}}"#),
            &[&doc],
        ));
        out.extend(outcomes(
            &format!(r#"{{"properties":{{"n":{{"minimum":{other}}}}}}}"#),
            &[&format!(r#"{{"n":{doc}}}"#)],
        ));
        (vec![limit, other, doc], out)
    }

    /// Go: `stringsCase`: an enum of 1 to 4 strings and two property names, and their outcomes.
    fn strings_case(rng: &mut SplitMix64) -> (Vec<String>, Vec<String>) {
        let count = 1 + rng.below(4);
        let values: Vec<String> = (0..count).map(|_| rng.str()).collect();
        let key = rng.str();
        let other = rng.str();
        let joined = values.join(",");
        let first = values.first().map_or("", String::as_str);
        let last = values.last().map_or("", String::as_str);
        let enumeration = format!(r#"{{"enum":[{joined}]}}"#);
        let mut out = outcomes(&enumeration, &[OUTSIDE, first, last]);
        out.extend(outcomes(
            &format!(r#"{{"properties":{{{key}:{enumeration}}},"additionalProperties":false}}"#),
            &[
                &format!("{{{key}:{OUTSIDE}}}"),
                &format!("{{{other}:{first}}}"),
                &format!("{{{key}:{last}}}"),
            ],
        ));
        out.extend(outcomes(
            &format!(r#"{{"required":[{key}],"properties":{{{other}:{{"type":"integer"}}}}}}"#),
            &["{}", &format!("{{{other}:{first}}}")],
        ));
        (vec![joined, key, other], out)
    }

    fn hex(digest: &[u8]) -> String {
        digest
            .iter()
            .flat_map(|b| [b >> 4, b & 0xF])
            .filter_map(|nibble| char::from_digit(u32::from(nibble), 16))
            .collect()
    }

    /// The golden line of `sweep`.
    fn golden(sweep: &str) -> Result<Value, String> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/golden/review_sweeps.jsonl"
        );
        let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        for line in text.lines() {
            let value: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
            if value.get("sweep").and_then(Value::as_str) == Some(sweep) {
                return Ok(value);
            }
        }
        Err(format!("{path} has no {sweep} sweep"))
    }

    /// A generator of one case: its inputs and the outcomes of its validations.
    type Case = fn(&mut SplitMix64) -> (Vec<String>, Vec<String>);

    /// Go: `sweep`: runs the golden's count of cases from its seed and compares the digests.
    fn replay(name: &str, case: Case) -> TestResult {
        let line = golden(name)?;
        let field = |key: &str| line.get(key).ok_or_else(|| format!("{name}: no {key}"));
        let seed = field("seed")?.as_u64().ok_or("seed")?;
        let count = field("count")?.as_u64().ok_or("count")?;
        let sample: Vec<&str> = field("sample")?
            .as_array()
            .map_or_default(|items| items.iter().filter_map(Value::as_str).collect());
        let mut rng = SplitMix64(seed);
        let (mut inputs, mut results) = (Sha256::new(), Sha256::new());
        let mut first_difference = None;
        for i in 0..count {
            let (input, output) = case(&mut rng);
            inputs.update(format!("{}\n", input.join("\t")));
            let mut fields = input;
            fields.extend(output);
            let text = format!("{}\n", fields.join("\t"));
            results.update(&text);
            let want = usize::try_from(i).ok().and_then(|index| sample.get(index));
            if first_difference.is_none()
                && let Some(want) = want.filter(|want| **want != text)
            {
                first_difference = Some(format!("case {i}:\nGo   {want:?}\nRust {text:?}"));
            }
        }
        let want = |key: &str| field(key).map(|v| v.as_str().unwrap_or_default().to_owned());
        if hex(&inputs.finalize()) != want("inputs")? {
            return Err(format!("{name}: the cases differ from Go's"));
        }
        if hex(&results.finalize()) != want("outcomes")? {
            return Err(format!(
                "{name}: the outcomes differ from Go's; {}",
                first_difference.unwrap_or_else(|| "the sampled cases agree".to_owned())
            ));
        }
        Ok(())
    }

    type TestResult = Result<(), String>;

    /// Go: `limitsCase` in `testdata/go/review/main.go`: gojsonschema's `validateNumber` and
    /// type checks, and its texts of a limit (`new(big.Float).SetRat(limit)` printed with
    /// `%v`), over 50,000 seeded limits and documents.
    #[test]
    fn limits_match_go() -> TestResult {
        replay("limits", limits_case)
    }

    /// Go: `stringsCase` in `testdata/go/review/main.go`: gojsonschema's `enum` (its values as
    /// `json.Marshal` writes them, and its duplicates), `required`, `additionalProperties` and
    /// context texts, over 50,000 seeded enums and property names.
    #[test]
    fn strings_match_go() -> TestResult {
        replay("strings", strings_case)
    }
}
