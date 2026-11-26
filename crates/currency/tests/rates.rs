//! Pins of the currency file decode against Go's: `RateConverter.fetch` decodes the body with
//! `jsonutil.UnmarshalValid(body, &currency.Rates{})` (jsoniter), the port with
//! `pbs_jsonutil::unmarshal_valid` into serde's `Rates`.
//!
//! Go's outcomes are in `testdata/golden/rates_json.golden` (jsoniter's, with its texts, of which
//! the verdicts and the rates count here) and `testdata/golden/decode.golden` (fixtures and a
//! seeded corpus, without texts), written by `testdata/go`. Where serde reads a currency file
//! otherwise is `currency.rates-json`.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use pbs_currency::Rates;
    use pbs_jsonutil::unmarshal_valid;

    use crate::support::{SplitMix64, TestResult, check, check_eq, decode, fnv64a, golden, unhex};

    /// A document of the goldens: one written out, or a nested case of `rates_json.golden` by
    /// its shape, depth and whether it is closed.
    #[derive(Debug, Clone, Copy)]
    enum Case {
        Document(&'static [u8]),
        Nested(&'static str),
    }

    /// The documents serde reads otherwise than jsoniter: the ledger entry each falls under, the
    /// document, and serde's outcome. Each is a difference `currency.rates-json` lists; every
    /// other document of the goldens decodes as in Go.
    const DIFFERENCES: &[(&str, Case, &str)] = &[
        // The document `null`, which jsoniter reads as nil rates.
        ("json.null-document", Case::Document(b"null"), "err"),
        ("json.null-document", Case::Document(b" null "), "err"),
        // A NUL after the document, where jsoniter stops.
        ("json.trailing-nul", Case::Document(b"{}\0garbage"), "err"),
        ("json.trailing-nul", Case::Document(b"{}\0"), "err"),
        (
            "json.trailing-nul",
            Case::Document(b"{\"conversions\":{\"USD\":{\"GBP\":1}}}\0"),
            "err",
        ),
        (
            "json.trailing-nul",
            Case::Document(b"{\"conversions\":{\"USD\":{\"GBP\":1}}} \0 x"),
            "err",
        ),
        // An array where the struct belongs, which serde reads as its fields.
        ("json.struct-from-array", Case::Document(b"[{}]"), "ok:{}"),
        (
            "json.struct-from-array",
            Case::Document(b"[null]"),
            "ok:nil",
        ),
        (
            "json.struct-from-array",
            Case::Document(b"[{\"USD\":{\"GBP\":1}}]"),
            "ok:{555344:{474250=3ff0000000000000,};}",
        ),
        // Another case of `conversions`, which jsoniter reads as the member.
        (
            "json.key-case",
            Case::Document(b"{\"Conversions\":{\"USD\":{\"GBP\":1}}}"),
            "ok:nil",
        ),
        (
            "json.key-case",
            Case::Document(b"{\"CONVERSIONS\":{\"USD\":{\"GBP\":1}}}"),
            "ok:nil",
        ),
        (
            "json.key-case",
            Case::Document(b"{\"conversionS\":{\"USD\":{\"GBP\":1}}}"),
            "ok:nil",
        ),
        (
            "json.key-case",
            Case::Document(b"{\"CoNvErSiOnS\":{\"USD\":{\"GBP\":1}}}"),
            "ok:nil",
        ),
        (
            "json.key-case",
            Case::Document(b"{\"\\u0043onversions\":{\"USD\":{\"GBP\":1}}}"),
            "ok:nil",
        ),
        (
            "json.key-case",
            Case::Document(
                b"{\"conversions\":{\"USD\":{\"GBP\":1}},\"Conversions\":{\"EUR\":{\"GBP\":2}}}",
            ),
            "ok:{555344:{474250=3ff0000000000000,};}",
        ),
        // A second `conversions` member, which jsoniter decodes into the first one's map, or null.
        (
            "json.duplicate-keys",
            Case::Document(
                b"{\"conversions\":{\"USD\":{\"GBP\":1}},\"conversions\":{\"EUR\":{\"GBP\":2}}}",
            ),
            "err",
        ),
        (
            "json.duplicate-keys",
            Case::Document(b"{\"conversions\":{\"USD\":{\"GBP\":1}},\"conversions\":null}"),
            "err",
        ),
        (
            "json.duplicate-keys",
            Case::Document(
                b"{\"conversions\":{\"USD\":{\"GBP\":1}},\"conversions\":{\"USD\":null}}",
            ),
            "err",
        ),
        // `null` where a key belongs, which jsoniter reads as the key `""`.
        (
            "currency.rates-json",
            Case::Document(b"{\"conversions\":{null:{\"GBP\":null}}}"),
            "err",
        ),
        (
            "currency.rates-json",
            Case::Document(b"{\"conversions\":{\"USD\":{null:1}}}"),
            "err",
        ),
        // A null rate, which jsoniter reads as 0.
        (
            "json.null-elements",
            Case::Document(b"{\"conversions\":{\"USD\":{\"GBP\":null}}}"),
            "err",
        ),
        (
            "json.null-elements",
            Case::Document(b"{\"conversions\":{\"USD\":{\"GBP\":null,\"EUR\":2}}}"),
            "err",
        ),
        (
            "json.null-elements",
            Case::Document(b"{\"conversions\":{\"USD\":{\"GBP\":2,\"EUR\":null}}}"),
            "err",
        ),
        // A rate with a leading plus.
        (
            "json.plus-float",
            Case::Document(b"{\"conversions\":{\"USD\":{\"GBP\":+5}}}"),
            "err",
        ),
        (
            "json.plus-float",
            Case::Document(b"{\"conversions\":{\"USD\":{\"GBP\":-+5}}}"),
            "err",
        ),
        (
            "json.plus-float",
            Case::Document(b"{\"conversions\":{\"USD\":{\"GBP\":+.5}}}"),
            "err",
        ),
        // A malformed number that starts with a minus, in a member Rates skips, which jsoniter's
        // skipper reads.
        (
            "currency.rates-json",
            Case::Document(b"{\"x\":-.5,\"conversions\":{\"USD\":{\"GBP\":1}}}"),
            "err",
        ),
        (
            "currency.rates-json",
            Case::Document(b"{\"x\":-01e5,\"conversions\":{\"USD\":{\"GBP\":1}}}"),
            "err",
        ),
        (
            "currency.rates-json",
            Case::Document(b"{\"x\":-01e2147483646,\"conversions\":{\"USD\":{\"GBP\":1}}}"),
            "err",
        ),
        (
            "currency.rates-json",
            Case::Document(b"{\"x\":-.1e-2147483647,\"conversions\":{\"USD\":{\"GBP\":1}}}"),
            "err",
        ),
        (
            "currency.rates-json",
            Case::Document(b"{\"x\":-00e5,\"conversions\":{\"USD\":{\"GBP\":1}}}"),
            "err",
        ),
        (
            "currency.rates-json",
            Case::Document(b"{\"x\":-00e999999999999,\"conversions\":{\"USD\":{\"GBP\":1}}}"),
            "err",
        ),
        (
            "currency.rates-json",
            Case::Document(b"{\"x\":-01.5e3,\"conversions\":{\"USD\":{\"GBP\":1}}}"),
            "err",
        ),
        (
            "currency.rates-json",
            Case::Document(b"{\"x\":[-01],\"conversions\":{}}"),
            "err",
        ),
        (
            "currency.rates-json",
            Case::Document(b"{\"x\":{\"y\":-.5},\"conversions\":{}}"),
            "err",
        ),
        // A skipped number past float64's range, or past float32's where it starts with 0, on
        // which jsoniter fails.
        (
            "json.out-of-range-numbers",
            Case::Document(b"{\"x\":-1e400,\"conversions\":{\"USD\":{\"GBP\":1}}}"),
            "ok:{555344:{474250=3ff0000000000000,};}",
        ),
        (
            "json.out-of-range-numbers",
            Case::Document(b"{\"x\":1e400,\"conversions\":{\"USD\":{\"GBP\":1}}}"),
            "ok:{555344:{474250=3ff0000000000000,};}",
        ),
        (
            "json.out-of-range-numbers",
            Case::Document(b"{\"x\":0.5e39,\"conversions\":{\"USD\":{\"GBP\":1}}}"),
            "ok:{555344:{474250=3ff0000000000000,};}",
        ),
        (
            "json.out-of-range-numbers",
            Case::Document(b"{\"x\":1e309,\"conversions\":{}}"),
            "ok:{}",
        ),
        (
            "json.out-of-range-numbers",
            Case::Document(b"{\"x\":-1e309,\"conversions\":{}}"),
            "ok:{}",
        ),
        (
            "json.out-of-range-numbers",
            Case::Document(b"{\"x\":1.8e308,\"conversions\":{}}"),
            "ok:{}",
        ),
        (
            "json.out-of-range-numbers",
            Case::Document(b"{\"x\":0.4e39,\"conversions\":{}}"),
            "ok:{}",
        ),
        // A raw control character after an escape, or in a member name.
        (
            "json.control-characters",
            Case::Document(b"{\"x\":\"a\\nb\x01\",\"conversions\":{\"USD\":{\"GBP\":1}}}"),
            "err",
        ),
        (
            "json.control-characters",
            Case::Document(b"{\"x\":\"a\\n\tb\",\"conversions\":{}}"),
            "err",
        ),
        (
            "json.control-characters",
            Case::Document(b"{\"conversions\":{\"a\\n\tb\":{\"GBP\":1}}}"),
            "err",
        ),
        (
            "json.control-characters",
            Case::Document(b"{\"conver\tsions\":{\"USD\":{\"GBP\":1}}}"),
            "err",
        ),
        (
            "json.control-characters",
            Case::Document(b"{\"x\\n\t\":1,\"conversions\":{}}"),
            "err",
        ),
        // Bytes that are not UTF-8 in a key.
        (
            "json.invalid-utf8",
            Case::Document(b"{\"conversions\":{\"\xff\":{\"GBP\":1}}}"),
            "err",
        ),
        (
            "json.invalid-utf8",
            Case::Document(b"{\"conversions\":{\"US\xe2\x82\":{\"GBP\":1}}}"),
            "err",
        ),
        // Nesting past jsoniter's 10000 levels in a skipped member.
        (
            "json.nesting-depth",
            Case::Nested("array 10000 true"),
            "ok:nil",
        ),
        (
            "json.nesting-depth",
            Case::Nested("array 200000 true"),
            "ok:nil",
        ),
        (
            "json.nesting-depth",
            Case::Nested("object 10000 true"),
            "ok:nil",
        ),
        (
            "json.nesting-depth",
            Case::Nested("object 200000 true"),
            "ok:nil",
        ),
    ];

    /// One document of the goldens, with Go's outcome: `err`, or `ok:` and the dump.
    struct GoCase {
        name: String,
        input: Vec<u8>,
        outcome: String,
    }

    /// Go's verdict and rates in a `rates_json.golden` outcome, without jsoniter's texts.
    fn verdict(outcome: &str) -> TestResult<String> {
        let outcome = String::from_utf8(unhex(outcome)?)?;
        Ok(if outcome.starts_with("ok:") {
            outcome
        } else {
            "err".to_owned()
        })
    }

    /// Every document of `rates_json.golden`'s explicit and nested cases and of
    /// `decode.golden`'s fixtures, with Go's outcome.
    fn go_cases() -> TestResult<Vec<GoCase>> {
        let mut cases = Vec::new();
        for fields in golden("rates_json.golden")? {
            match fields.as_slice() {
                [kind, input, outcome] if kind == "json" => cases.push(GoCase {
                    name: format!("json {input}"),
                    input: unhex(input)?,
                    outcome: verdict(outcome)?,
                }),
                [kind, shape, depth, closed, outcome] if kind == "deep" => {
                    let (open, close): (&[u8], &[u8]) = if shape == "array" {
                        (b"[", b"]")
                    } else {
                        (b"{\"a\":", b"}")
                    };
                    let levels: usize = depth.parse()?;
                    let mut input = b"{\"x\":".to_vec();
                    input.extend(open.repeat(levels));
                    if closed == "true" {
                        input.push(b'1');
                        input.extend(close.repeat(levels));
                        input.push(b'}');
                    }
                    cases.push(GoCase {
                        name: format!("{shape} {depth} {closed}"),
                        input,
                        outcome: verdict(outcome)?,
                    });
                }
                _ => {}
            }
        }
        for fields in golden("decode.golden")? {
            if let [kind, input, outcome] = fields.as_slice()
                && kind == "fixture"
            {
                cases.push(GoCase {
                    name: format!("fixture {input}"),
                    input: unhex(input)?,
                    outcome: String::from_utf8(unhex(outcome)?)?,
                });
            }
        }
        Ok(cases)
    }

    #[test]
    fn decode_like_go() -> TestResult {
        // Go: currency/rate_converter.go:81-85 (fetch decodes the body with
        // jsonutil.UnmarshalValid into a currency.Rates)
        let cases = go_cases()?;
        check(cases.len() > 200, "the goldens hold the documents")?;
        let mut used = vec![false; DIFFERENCES.len()];
        for case in &cases {
            let rust = decode(&case.input);
            let what = format!(
                "{} ({:?})",
                case.name,
                String::from_utf8_lossy(case.input.get(..200).unwrap_or(case.input.as_slice()))
            );
            let listed =
                DIFFERENCES
                    .iter()
                    .enumerate()
                    .find(|(_, (_, document, _))| match document {
                        Case::Document(bytes) => *bytes == case.input.as_slice(),
                        Case::Nested(name) => *name == case.name,
                    });
            match listed {
                Some((at, (entry, _, outcome))) => {
                    check_eq(rust.as_str(), *outcome, &format!("{entry}: {what}"))?;
                    check(
                        rust != case.outcome,
                        &format!("{entry}: {what} decodes as in Go"),
                    )?;
                    if let Some(flag) = used.get_mut(at) {
                        *flag = true;
                    }
                }
                None => check_eq(&rust, &case.outcome, &what)?,
            }
        }
        let unused: Vec<_> = DIFFERENCES
            .iter()
            .zip(&used)
            .filter(|(_, flag)| !**flag)
            .map(|(difference, _)| difference)
            .collect();
        check(
            unused.is_empty(),
            &format!("differences no golden holds: {unused:?}"),
        )
    }

    /// [`Rates`] of `pairs`: each currency with its rates.
    fn rates(pairs: &[(&str, &[(&str, f64)])]) -> Rates {
        Rates {
            conversions: Some(
                pairs
                    .iter()
                    .map(|(from, to)| {
                        let to = to.iter().map(|(to, rate)| ((*to).to_owned(), *rate));
                        ((*from).to_owned(), Some(to.collect()))
                    })
                    .collect(),
            ),
        }
    }

    #[test]
    fn unmarshal_rates() -> TestResult {
        // Go: TestUnMarshallRates (jsoniter's `expect { or n, but found m` for the malformed
        // document is serde_json's text here, json.error-texts)
        check(
            unmarshal_valid::<Rates>(b"malformed").is_err(),
            "malformed JSON object",
        )?;
        let valid = br#"{
				"conversions":{
					"USD":{
						"GBP":0.7662523901
					},
					"GBP":{
						"USD":1.3050530256
					}
				}
			}"#;
        check_eq(
            &unmarshal_valid::<Rates>(valid).ok(),
            &Some(rates(&[
                ("USD", &[("GBP", 0.766_252_390_1)]),
                ("GBP", &[("USD", 1.305_053_025_6)]),
            ])),
            "valid conversions",
        )?;
        let repeated = br#"{
				"conversions":{
					"USD":{
						"GBP":0.7662523901,
						"MXN":20.00
					},
					"USD":{
						"GBP":0.4815162342
					}
				}
			}"#;
        check_eq(
            &unmarshal_valid::<Rates>(repeated).ok(),
            &Some(rates(&[("USD", &[("GBP", 0.481_516_234_2)])])),
            "repeated entries, last one wins",
        )
    }

    /// The lists the corpus draws from (Go: `decodeLists`), as `decode.golden` holds them.
    #[derive(Debug, Default)]
    struct Lists {
        outer_space: Vec<Vec<u8>>,
        non_objects: Vec<Vec<u8>>,
        conversions_names: Vec<Vec<u8>>,
        other_names: Vec<Vec<u8>>,
        nested_names: Vec<Vec<u8>>,
        code_keys: Vec<Vec<u8>>,
        skipped_strings: Vec<Vec<u8>>,
        literals: Vec<Vec<u8>>,
        skipped_numbers: Vec<Vec<u8>>,
        rate_values: Vec<Vec<u8>>,
        bad_rates: Vec<Vec<u8>>,
        signs: Vec<Vec<u8>>,
        colons: Vec<Vec<u8>>,
        commas: Vec<Vec<u8>>,
        exponents: Vec<Vec<u8>>,
        exponent_signs: Vec<Vec<u8>>,
    }

    impl Lists {
        /// The `list` lines of `decode.golden`.
        fn from_golden(lines: &[Vec<String>]) -> TestResult<Self> {
            let mut lists = Self::default();
            let mut read = 0_usize;
            for fields in lines {
                let [kind, name, items @ ..] = fields.as_slice() else {
                    continue;
                };
                if kind != "list" {
                    continue;
                }
                let target = match name.as_str() {
                    "outer_space" => &mut lists.outer_space,
                    "non_objects" => &mut lists.non_objects,
                    "conversions_names" => &mut lists.conversions_names,
                    "other_names" => &mut lists.other_names,
                    "nested_names" => &mut lists.nested_names,
                    "code_keys" => &mut lists.code_keys,
                    "skipped_strings" => &mut lists.skipped_strings,
                    "literals" => &mut lists.literals,
                    "skipped_numbers" => &mut lists.skipped_numbers,
                    "rate_values" => &mut lists.rate_values,
                    "bad_rates" => &mut lists.bad_rates,
                    "signs" => &mut lists.signs,
                    "colons" => &mut lists.colons,
                    "commas" => &mut lists.commas,
                    "exponents" => &mut lists.exponents,
                    "exponent_signs" => &mut lists.exponent_signs,
                    _ => return Err(format!("unknown list {name}").into()),
                };
                *target = items
                    .iter()
                    .map(|item| unhex(item))
                    .collect::<TestResult<_>>()?;
                check(!target.is_empty(), &format!("the list {name} is empty"))?;
                read += 1;
            }
            check_eq(&read, &16, "the lists")?;
            Ok(lists)
        }
    }

    /// Go: `joinItems`: `items` joined by drawn commas.
    fn join_items(s: &mut SplitMix64, l: &Lists, items: Vec<Vec<u8>>) -> Vec<u8> {
        let mut out = Vec::new();
        for (n, item) in items.into_iter().enumerate() {
            if n > 0 {
                out.extend(s.pick(&l.commas));
            }
            out.extend(item);
        }
        out
    }

    /// `inside` between `open` and `close`.
    fn wrap(open: u8, inside: Vec<u8>, close: u8) -> Vec<u8> {
        [vec![open], inside, vec![close]].concat()
    }

    /// The decimal digits of `n` (Go: `strconv.Itoa`).
    fn decimal(n: usize) -> Vec<u8> {
        n.to_string().into_bytes()
    }

    /// Go: `randomWhole`.
    fn random_whole(s: &mut SplitMix64, max_rest: usize) -> Vec<u8> {
        if s.below(3) == 0 {
            return b"0".to_vec();
        }
        let lead = 1 + s.below(9);
        let count = s.below(max_rest);
        [decimal(lead), s.digits(count)].concat()
    }

    /// Go: `randomRateNumber`.
    fn random_rate_number(s: &mut SplitMix64, l: &Lists) -> Vec<u8> {
        match s.below(5) {
            0 => s.pick(&l.rate_values),
            1 => {
                let sign = s.pick(&l.signs);
                let lead = 1 + s.below(9);
                let count = s.below(25);
                [sign, decimal(lead), s.digits(count)].concat()
            }
            2 => {
                let sign = s.pick(&l.signs);
                let whole = random_whole(s, 20);
                let count = 1 + s.below(20);
                [sign, whole, b".".to_vec(), s.digits(count)].concat()
            }
            _ => {
                let sign = s.pick(&l.signs);
                let mut mantissa = random_whole(s, 20);
                if s.below(2) == 0 {
                    let count = 1 + s.below(17);
                    mantissa.push(b'.');
                    mantissa.extend(s.digits(count));
                }
                let e = s.pick(&l.exponents);
                let exponent_sign = s.pick(&l.exponent_signs);
                let exponent = s.below(330);
                [sign, mantissa, e, exponent_sign, decimal(exponent)].concat()
            }
        }
    }

    /// Go: `randomRateValue`.
    fn random_rate_value(s: &mut SplitMix64, l: &Lists) -> Vec<u8> {
        if s.below(200) == 0 {
            return s.pick(&l.bad_rates);
        }
        random_rate_number(s, l)
    }

    /// Go: `randomSkippedNumber`.
    fn random_skipped_number(s: &mut SplitMix64, l: &Lists) -> Vec<u8> {
        let sign = s.pick(&l.signs);
        let number = match s.below(3) {
            0 => {
                let lead = 1 + s.below(9);
                let count = s.below(15);
                [decimal(lead), s.digits(count)].concat()
            }
            1 => {
                let whole = random_whole(s, 15);
                let count = 1 + s.below(15);
                [whole, b".".to_vec(), s.digits(count)].concat()
            }
            _ => {
                let lead = 1 + s.below(9);
                let mut mantissa = decimal(lead);
                let count = s.below(10);
                let fraction = s.digits(count);
                if !fraction.is_empty() {
                    mantissa.push(b'.');
                    mantissa.extend(fraction);
                }
                let e = s.pick(&l.exponents);
                let exponent_sign = s.pick(&l.exponent_signs);
                let exponent = s.below(21);
                [mantissa, e, exponent_sign, decimal(exponent)].concat()
            }
        };
        [sign, number].concat()
    }

    /// Go: `randomSkippedValue`.
    fn random_skipped_value(s: &mut SplitMix64, l: &Lists, depth: usize) -> Vec<u8> {
        let kinds = if depth >= 3 { 4 } else { 7 };
        match s.below(kinds) {
            0 => s.pick(&l.skipped_strings),
            1 => s.pick(&l.literals),
            2 => s.pick(&l.skipped_numbers),
            3 => random_skipped_number(s, l),
            4 | 5 => {
                let mut items = Vec::new();
                for _ in 0..s.below(4) {
                    items.push(random_skipped_value(s, l, depth + 1));
                }
                wrap(b'[', join_items(s, l, items), b']')
            }
            _ => {
                let mut members = Vec::new();
                for _ in 0..s.below(4) {
                    let name = s.pick(&l.nested_names);
                    let colon = s.pick(&l.colons);
                    let value = random_skipped_value(s, l, depth + 1);
                    members.push([name, colon, value].concat());
                }
                wrap(b'{', join_items(s, l, members), b'}')
            }
        }
    }

    /// Go: `randomOtherMember`.
    fn random_other_member(s: &mut SplitMix64, l: &Lists) -> Vec<u8> {
        let name = s.pick(&l.other_names);
        let colon = s.pick(&l.colons);
        let value = random_skipped_value(s, l, 0);
        [name, colon, value].concat()
    }

    /// Go: `randomConversions`.
    fn random_conversions(s: &mut SplitMix64, l: &Lists) -> Vec<u8> {
        if s.below(12) == 0 {
            return b"{}".to_vec();
        }
        let mut entries = Vec::new();
        for _ in 0..=s.below(6) {
            let key = s.pick(&l.code_keys);
            let colon = s.pick(&l.colons);
            if s.below(8) == 0 {
                entries.push([key, colon, b"null".to_vec()].concat());
                continue;
            }
            let mut inner = Vec::new();
            for _ in 0..s.below(6) {
                let to = s.pick(&l.code_keys);
                let inner_colon = s.pick(&l.colons);
                let rate = random_rate_value(s, l);
                inner.push([to, inner_colon, rate].concat());
            }
            let rates = wrap(b'{', join_items(s, l, inner), b'}');
            entries.push([key, colon, rates].concat());
        }
        wrap(b'{', join_items(s, l, entries), b'}')
    }

    /// Go: `randomRatesObject`.
    fn random_rates_object(s: &mut SplitMix64, l: &Lists) -> Vec<u8> {
        let mut members = Vec::new();
        for _ in 0..s.below(3) {
            members.push(random_other_member(s, l));
        }
        match s.below(8) {
            0 => {}
            1 => {
                let name = s.pick(&l.conversions_names);
                let colon = s.pick(&l.colons);
                members.push([name, colon, b"null".to_vec()].concat());
            }
            _ => {
                let name = s.pick(&l.conversions_names);
                let colon = s.pick(&l.colons);
                let value = random_conversions(s, l);
                members.push([name, colon, value].concat());
            }
        }
        for _ in 0..s.below(3) {
            members.push(random_other_member(s, l));
        }
        wrap(b'{', join_items(s, l, members), b'}')
    }

    /// Go: `randomDocument`.
    fn random_document(s: &mut SplitMix64, l: &Lists) -> Vec<u8> {
        let kind = s.below(32);
        if kind == 0 {
            return s.pick(&l.non_objects);
        }
        let mut document = random_rates_object(s, l);
        if kind == 1 {
            let cut = s.below(document.len());
            document.truncate(cut);
        }
        let lead = s.pick(&l.outer_space);
        let trail = s.pick(&l.outer_space);
        [lead, document, trail].concat()
    }

    /// Go: `randomBatch`.
    const BATCH: usize = 500;

    #[test]
    fn seeded_documents_like_go() -> TestResult {
        // Go: currency/rate_converter.go:81-85 (fetch's decode, over the 50,000 currency files
        // testdata/go/decode.go draws from splitmix64, in batch digests)
        let lines = golden("decode.golden")?;
        let lists = Lists::from_golden(&lines)?;
        let batches: Vec<(&str, &str)> = lines
            .iter()
            .filter_map(|fields| match fields.as_slice() {
                [kind, _batch, inputs, outcomes] if kind == "decode-batch" => {
                    Some((inputs.as_str(), outcomes.as_str()))
                }
                _ => None,
            })
            .collect();
        check_eq(&batches.len(), &100, "the batches")?;
        let mut s = SplitMix64::new(20_261_126);
        for (n, (go_inputs, go_outcomes)) in batches.into_iter().enumerate() {
            let mut inputs = Vec::new();
            let mut outcomes = Vec::new();
            for _ in 0..BATCH {
                let document = random_document(&mut s, &lists);
                outcomes.extend(decode(&document).into_bytes());
                outcomes.push(b'\n');
                inputs.extend(document);
                inputs.push(b'\n');
            }
            check_eq(
                fnv64a(&inputs).as_str(),
                go_inputs,
                &format!("batch {n}: the documents (the generators drifted)"),
            )?;
            check_eq(
                fnv64a(&outcomes).as_str(),
                go_outcomes,
                &format!("batch {n}: the outcomes"),
            )?;
        }
        Ok(())
    }
}
