//! Pins of `serde_json` and the `serde` derives against `encoding/json` and jsoniter as
//! `util/jsonutil` configures it: writing strings, floats and raw messages, decoding keys,
//! duplicates, invalid UTF-8 and nulls, and reading numbers (`testdata/golden/json.json` and
//! `testdata/golden/float_parse.json`).

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pbs_differences::{
        BoxError, TestResult, at, check, check_eq, check_table, golden, hex, list, number, text,
        unhex,
    };
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, value::RawValue};
    use sha2::{Digest as _, Sha256};

    /// What `jsonutil.Marshal`, `json.Marshal` and the endpoints' Encoder (its newline removed)
    /// wrote for one golden value, each as its text or `error: <text>`.
    fn go_writers(writers: &Value) -> Result<Vec<String>, BoxError> {
        ["jsonutil_marshal", "json_marshal", "encoder_no_html"]
            .iter()
            .map(|name| {
                let err = text(writers, &format!("/{name}/err"))?;
                if err.is_empty() {
                    let out = text(writers, &format!("/{name}/out"))?;
                    Ok(out.strip_suffix('\n').unwrap_or(out).to_owned())
                } else {
                    Ok(format!("error: {err}"))
                }
            })
            .collect()
    }

    /// `serde_json`'s text for `value`, or `error: <text>`.
    fn written<T: Serialize + ?Sized>(value: &T) -> String {
        serde_json::to_string(value).unwrap_or_else(|e| format!("error: {e}"))
    }

    /// The golden's hex bits field as an integer.
    fn bits_of(item: &Value, pointer: &str) -> Result<u64, BoxError> {
        Ok(u64::from_str_radix(text(item, pointer)?, 16)?)
    }

    /// `jsonutil.Marshal` and `json.Marshal` escape `<`, `>` and `&` in strings, and U+2028 and
    /// U+2029, as `\u` sequences; the endpoints' Encoders turn HTML escaping off and still escape
    /// U+2028 and U+2029. `serde_json` writes all five characters as they are.
    // Go: util/jsonutil/jsonutil.go:159-167 (Marshal on ConfigCompatibleWithStandardLibrary, line
    // 125) and endpoints/openrtb2/auction.go:383-384 (an Encoder with SetEscapeHTML(false))
    #[test]
    fn go_difference_no_html_escaping() -> TestResult {
        let doc = golden("json.json")?;
        let input = text(&doc, "/html_escape/input")?;
        let mut row = go_writers(at(&doc, "/html_escape/writers")?)?;
        row.push(written(input));
        check_table(
            &[row],
            &[&[
                "\"\\u003ca href=\\\"x\\\"\\u003e\\u0026\\u003c/a\\u003e\\u2028\\u2029\"",
                "\"\\u003ca href=\\\"x\\\"\\u003e\\u0026\\u003c/a\\u003e\\u2028\\u2029\"",
                "\"<a href=\\\"x\\\">&</a>\\u2028\\u2029\"",
                "\"<a href=\\\"x\\\">&</a>\u{2028}\u{2029}\"",
            ]],
            "jsonutil.Marshal, json.Marshal, the endpoints' Encoder, serde_json",
        )
    }

    /// Go writes floats in strconv's shortest form, plain from 1e-6 up to 1e21 and with an
    /// exponent outside that range, which jsoniter spells `1e-07` and `encoding/json` `1e-7`; an
    /// integral value has no fraction, and a float32 is written at 32 bits (overflowing to an
    /// error when the value does not fit). `serde_json` writes `1.0`, switches to exponents at
    /// other magnitudes (`1e+20`, and `1e-6` for an f64) and writes `null` for an f32 that
    /// overflowed. A `Value` number keeps the form it was read in (`300` stays `300`, `300.0`
    /// stays a float), and an integer up to `u64::MAX` stays exact where Go reads a float64.
    // Go: util/jsonutil/jsonutil.go:159-167 (jsoniter's WriteFloat64 and WriteFloat32) and
    // encoding/json's floatEncoder (json.Marshal in the adapters); interface{} numbers through
    // util/jsonutil/jsonutil.go:136-144
    #[test]
    fn go_difference_float_formatting() -> TestResult {
        let doc = golden("json.json")?;
        let mut f64_rows = Vec::new();
        let mut f32_rows = Vec::new();
        for case in list(&doc, "/float_format")? {
            let mut row = vec![text(case, "/bits")?.to_owned()];
            row.extend(go_writers(at(case, "/f64")?)?);
            row.push(written(&f64::from_bits(bits_of(case, "/bits")?)));
            f64_rows.push(row);
            let mut row32 = vec![text(case, "/bits32")?.to_owned()];
            row32.extend(go_writers(at(case, "/f32")?)?);
            row32.push(written(&f32::from_bits(u32::try_from(bits_of(
                case, "/bits32",
            )?)?)));
            f32_rows.push(row32);
        }
        check_table(
            &f64_rows,
            F64_TABLE,
            "f64: bits, jsonutil, json, Encoder, serde_json",
        )?;
        check_table(
            &f32_rows,
            F32_TABLE,
            "f32: bits, jsonutil, json, Encoder, serde_json",
        )?;
        let mut value_rows = Vec::new();
        for case in list(&doc, "/value_numbers")? {
            let input = text(case, "/input")?;
            value_rows.push(vec![
                input.to_owned(),
                text(case, "/jsonutil/out")?.to_owned(),
                text(case, "/json/out")?.to_owned(),
                serde_json::from_str::<Value>(input)?.to_string(),
            ]);
        }
        check_table(
            &value_rows,
            VALUE_TABLE,
            "a number read into interface{} or Value and written back: input, jsonutil, json, \
             serde_json",
        )
    }

    const F64_TABLE: &[&[&str]] = &[
        &["0", "0", "0", "0", "0.0"],
        &["8000000000000000", "-0", "-0", "-0", "-0.0"],
        &["3ff0000000000000", "1", "1", "1", "1.0"],
        &["4059000000000000", "100", "100", "100", "100.0"],
        &["4072c00000000000", "300", "300", "300", "300.0"],
        &["3ff8000000000000", "1.5", "1.5", "1.5", "1.5"],
        &["3fb999999999999a", "0.1", "0.1", "0.1", "0.1"],
        &["3fd3333333333333", "0.3", "0.3", "0.3", "0.3"],
        &[
            "400921f9f01b866e",
            "3.14159",
            "3.14159",
            "3.14159",
            "3.14159",
        ],
        &[
            "4132d68720000000",
            "1234567.125",
            "1234567.125",
            "1234567.125",
            "1234567.125",
        ],
        &[
            "419d6f3454000000",
            "123456789",
            "123456789",
            "123456789",
            "123456789.0",
        ],
        &[
            "3eb0c6f7a0b5ed8d",
            "0.000001",
            "0.000001",
            "0.000001",
            "1e-6",
        ],
        &["3e7ad7f29abcaf48", "1e-07", "1e-7", "1e-7", "1e-7"],
        &["3e5ad7f29abcaf48", "2.5e-08", "2.5e-8", "2.5e-8", "2.5e-8"],
        &[
            "4415af1d78b58c40",
            "100000000000000000000",
            "100000000000000000000",
            "100000000000000000000",
            "1e+20",
        ],
        &["444b1ae4d6e2ef50", "1e+21", "1e+21", "1e+21", "1e+21"],
        &[
            "7e41eb2d66005835",
            "1.5e+300",
            "1.5e+300",
            "1.5e+300",
            "1.5e+300",
        ],
        &[
            "7fefffffffffffff",
            "1.7976931348623157e+308",
            "1.7976931348623157e+308",
            "1.7976931348623157e+308",
            "1.7976931348623157e+308",
        ],
        &["1", "5e-324", "5e-324", "5e-324", "5e-324"],
    ];

    const F32_TABLE: &[&[&str]] = &[
        &["0", "0", "0", "0", "0.0"],
        &["80000000", "-0", "-0", "-0", "-0.0"],
        &["3f800000", "1", "1", "1", "1.0"],
        &["42c80000", "100", "100", "100", "100.0"],
        &["43960000", "300", "300", "300", "300.0"],
        &["3fc00000", "1.5", "1.5", "1.5", "1.5"],
        &["3dcccccd", "0.1", "0.1", "0.1", "0.1"],
        &["3e99999a", "0.3", "0.3", "0.3", "0.3"],
        &["40490fd0", "3.14159", "3.14159", "3.14159", "3.14159"],
        &[
            "4996b439",
            "1234567.1",
            "1234567.1",
            "1234567.1",
            "1234567.1",
        ],
        &[
            "4ceb79a3",
            "123456790",
            "123456790",
            "123456790",
            "123456790.0",
        ],
        &["358637bd", "0.000001", "0.000001", "0.000001", "0.000001"],
        &["33d6bf95", "1e-07", "1e-7", "1e-7", "1e-7"],
        &["32d6bf95", "2.5e-08", "2.5e-8", "2.5e-8", "2.5e-8"],
        &[
            "60ad78ec",
            "100000000000000000000",
            "100000000000000000000",
            "100000000000000000000",
            "1e+20",
        ],
        &["6258d727", "1e+21", "1e+21", "1e+21", "1e+21"],
        &[
            "7f800000",
            "error: unsupported value: +Inf",
            "error: json: unsupported value: +Inf",
            "error: json: unsupported value: +Inf",
            "null",
        ],
        &[
            "7f800000",
            "error: unsupported value: +Inf",
            "error: json: unsupported value: +Inf",
            "error: json: unsupported value: +Inf",
            "null",
        ],
        &["0", "0", "0", "0", "0.0"],
    ];

    const VALUE_TABLE: &[&[&str]] = &[
        &["300", "300", "300", "300"],
        &["300.0", "300", "300", "300.0"],
        &["1E2", "100", "100", "100.0"],
        &["1.5", "1.5", "1.5", "1.5"],
        &["0.000001", "0.000001", "0.000001", "1e-6"],
        &["1e-7", "1e-07", "1e-7", "1e-7"],
        &["-0", "-0", "-0", "-0.0"],
        &[
            "12345678901234567890",
            "12345678901234567000",
            "12345678901234567000",
            "12345678901234567890",
        ],
        &["1e21", "1e+21", "1e+21", "1e+21"],
    ];

    /// Go fails to write NaN and the infinities: `jsonutil.Marshal` with jsoniter's `unsupported
    /// value` after the field's path, `encoding/json` and the endpoints' Encoder with `json:
    /// unsupported value`, so the response is not written. `serde_json` writes `null` in the
    /// value's place, and the document goes out.
    // Go: util/jsonutil/jsonutil.go:159-167 (Marshal wraps jsoniter's error in FailedToMarshal) and
    // endpoints/openrtb2/auction.go:383-394 (the response Encoder)
    #[test]
    fn go_difference_non_finite_floats() -> TestResult {
        #[derive(Serialize)]
        struct PricedBid {
            price: f64,
        }
        let doc = golden("json.json")?;
        let mut rows = Vec::new();
        for (case, value) in
            list(&doc, "/non_finite")?
                .iter()
                .zip([f64::NAN, f64::INFINITY, f64::NEG_INFINITY])
        {
            let mut row = vec![text(case, "/name")?.to_owned()];
            row.extend(go_writers(at(case, "/bare")?)?);
            row.extend(go_writers(at(case, "/field")?)?);
            row.push(written(&value));
            row.push(written(&PricedBid { price: value }));
            rows.push(row);
        }
        check_table(
            &rows,
            &[
                &[
                    "NaN",
                    "error: unsupported value: NaN",
                    "error: json: unsupported value: NaN",
                    "error: json: unsupported value: NaN",
                    "error: main.pricedBid.Price: unsupported value: NaN",
                    "error: json: unsupported value: NaN",
                    "error: json: unsupported value: NaN",
                    "null",
                    "{\"price\":null}",
                ],
                &[
                    "+Inf",
                    "error: unsupported value: +Inf",
                    "error: json: unsupported value: +Inf",
                    "error: json: unsupported value: +Inf",
                    "error: main.pricedBid.Price: unsupported value: +Inf",
                    "error: json: unsupported value: +Inf",
                    "error: json: unsupported value: +Inf",
                    "null",
                    "{\"price\":null}",
                ],
                &[
                    "-Inf",
                    "error: unsupported value: -Inf",
                    "error: json: unsupported value: -Inf",
                    "error: json: unsupported value: -Inf",
                    "error: main.pricedBid.Price: unsupported value: -Inf",
                    "error: json: unsupported value: -Inf",
                    "error: json: unsupported value: -Inf",
                    "null",
                    "{\"price\":null}",
                ],
            ],
            "value; Go bare (jsonutil, json, Encoder); Go in a field (jsonutil, json, Encoder); \
             serde_json bare and in a field",
        )
    }

    /// A bid as the decode pins read it. `#[serde(default)]`, as the `OpenRTB` types will have
    /// it, lets fields be missing.
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Bid {
        id: String,
        impid: String,
        price: f64,
    }

    /// The same bid without defaults: every field must be present.
    #[derive(Debug, Deserialize)]
    struct StrictBid {
        id: String,
        impid: String,
        price: f64,
    }

    /// What Go's decoders made of one golden document: per decoder, the id as hex, the impid
    /// and the price, or `error: <text>`.
    fn go_decodes(doc: &Value, case: &str) -> Result<Vec<Vec<String>>, BoxError> {
        [
            "jsonutil_unmarshal",
            "jsonutil_unmarshal_valid",
            "json_unmarshal",
        ]
        .iter()
        .map(|decoder| {
            let base = format!("/{case}/{decoder}");
            let err = text(doc, &format!("{base}/err"))?;
            Ok(if err.is_empty() {
                vec![
                    (*decoder).to_owned(),
                    text(doc, &format!("{base}/id_hex"))?.to_owned(),
                    text(doc, &format!("{base}/impid"))?.to_owned(),
                    text(doc, &format!("{base}/price"))?.to_owned(),
                ]
            } else {
                vec![(*decoder).to_owned(), format!("error: {err}")]
            })
        })
        .collect()
    }

    /// What the `serde` derives made of the same document, in the same columns.
    fn rust_decodes(input: &[u8]) -> Vec<Vec<String>> {
        let lenient = match serde_json::from_slice::<Bid>(input) {
            Ok(bid) => vec![
                "serde(default)".to_owned(),
                hex(bid.id.as_bytes()),
                bid.impid,
                bid.price.to_string(),
            ],
            Err(e) => vec!["serde(default)".to_owned(), format!("error: {e}")],
        };
        let strict = match serde_json::from_slice::<StrictBid>(input) {
            Ok(bid) => vec![
                "serde".to_owned(),
                hex(bid.id.as_bytes()),
                bid.impid,
                bid.price.to_string(),
            ],
            Err(e) => vec!["serde".to_owned(), format!("error: {e}")],
        };
        vec![lenient, strict]
    }

    /// Go matches object keys to struct fields case-insensitively (`ID` fills `id`); `serde`
    /// matches exactly, so other spellings are unknown keys: ignored, then defaulted or a missing
    /// field.
    // Go: util/jsonutil/jsonutil.go:136-155 (Unmarshal and UnmarshalValid on the jsoniter configs
    // of lines 125-132, which leave CaseSensitive off)
    #[test]
    fn go_difference_case_sensitive_keys() -> TestResult {
        let doc = golden("json.json")?;
        let mut rows = go_decodes(&doc, "key_case")?;
        rows.extend(rust_decodes(&unhex(text(&doc, "/key_case/input_hex")?)?));
        check_table(
            &rows,
            &[
                &["jsonutil_unmarshal", "78", "y", "1.5"],
                &["jsonutil_unmarshal_valid", "78", "y", "1.5"],
                &["json_unmarshal", "78", "y", "1.5"],
                &["serde(default)", "", "", "0"],
                &["serde", "error: missing field `id` at line 1 column 34"],
            ],
            "decoder, id (hex), impid, price",
        )
    }

    /// Go keeps the last of duplicate keys; the `serde` derives reject the document, and only a
    /// `Value` keeps the last one.
    // Go: util/jsonutil/jsonutil.go:136-155 (Unmarshal and UnmarshalValid) and encoding/json's
    // object decoding
    #[test]
    fn go_difference_duplicate_struct_keys() -> TestResult {
        let doc = golden("json.json")?;
        let input = unhex(text(&doc, "/duplicate_keys/input_hex")?)?;
        let mut rows = go_decodes(&doc, "duplicate_keys")?;
        rows.extend(rust_decodes(&input));
        let value: Value = serde_json::from_slice(&input)?;
        rows.push(vec![
            "Value".to_owned(),
            value.get("id").map_or_default(Value::to_string),
        ]);
        check_table(
            &rows,
            &[
                &["jsonutil_unmarshal", "62", "y", "1"],
                &["jsonutil_unmarshal_valid", "62", "y", "1"],
                &["json_unmarshal", "62", "y", "1"],
                &[
                    "serde(default)",
                    "error: duplicate field `id` at line 1 column 14",
                ],
                &["serde", "error: duplicate field `id` at line 1 column 14"],
                &["Value", "\"b\""],
            ],
            "decoder, id (hex), impid, price",
        )
    }

    /// jsoniter keeps invalid UTF-8 in a string as it came and `encoding/json` replaces it with
    /// U+FFFD; `serde_json` rejects the document.
    // Go: util/jsonutil/jsonutil.go:136-155 (jsoniter's ReadString) and encoding/json's unquote
    #[test]
    fn go_difference_invalid_utf8() -> TestResult {
        let doc = golden("json.json")?;
        let mut rows = go_decodes(&doc, "invalid_utf8")?;
        rows.extend(rust_decodes(&unhex(text(
            &doc,
            "/invalid_utf8/input_hex",
        )?)?));
        check_table(
            &rows,
            &[
                &["jsonutil_unmarshal", "61ff62", "y", "1"],
                &["jsonutil_unmarshal_valid", "61ff62", "y", "1"],
                &["json_unmarshal", "61efbfbd62", "y", "1"],
                &[
                    "serde(default)",
                    "error: invalid unicode code point at line 1 column 9",
                ],
                &[
                    "serde",
                    "error: invalid unicode code point at line 1 column 9",
                ],
            ],
            "decoder, id (hex), impid, price",
        )
    }

    /// For Go, `null` has no effect on a field that is not a pointer, slice, map or interface:
    /// the field keeps its zero value and the document decodes. `serde` rejects it.
    // Go: util/jsonutil/jsonutil.go:136-155 (Unmarshal and UnmarshalValid) and encoding/json's
    // literalStore for null
    #[test]
    fn go_difference_null_into_value_field() -> TestResult {
        #[derive(Debug, Deserialize)]
        struct Sub {
            #[serde(rename = "a")]
            _a: String,
        }
        #[derive(Debug, Deserialize)]
        struct NullTarget {
            #[serde(rename = "id")]
            _id: String,
            #[serde(rename = "price")]
            _price: f64,
            #[serde(rename = "w")]
            _w: i64,
            #[serde(rename = "ok")]
            _ok: bool,
            #[serde(rename = "imp")]
            _imp: Vec<String>,
            #[serde(rename = "ext")]
            _ext: BTreeMap<String, String>,
            #[serde(rename = "sub")]
            _sub: Sub,
        }
        let doc = golden("json.json")?;
        let input = text(&doc, "/null_into_value/input")?;
        let mut rows = Vec::new();
        for decoder in [
            "jsonutil_unmarshal",
            "jsonutil_unmarshal_valid",
            "json_unmarshal",
        ] {
            let base = format!("/null_into_value/{decoder}");
            rows.push(vec![
                decoder.to_owned(),
                text(&doc, &format!("{base}/err"))?.to_owned(),
                text(&doc, &format!("{base}/value"))?.to_owned(),
            ]);
        }
        rows.push(vec![
            "serde".to_owned(),
            serde_json::from_str::<NullTarget>(input)
                .err()
                .map_or_default(|e| e.to_string()),
            String::new(),
        ]);
        let zero = "{\"id\":\"\",\"price\":0,\"w\":0,\"ok\":false,\"imp\":null,\"ext\":null,\
                    \"sub\":{\"a\":\"\"}}";
        check_table(
            &rows,
            &[
                &["jsonutil_unmarshal", "", zero],
                &["jsonutil_unmarshal_valid", "", zero],
                &["json_unmarshal", "", zero],
                &[
                    "serde",
                    "invalid type: null, expected a string at line 1 column 10",
                    "",
                ],
            ],
            "decoder, error, decoded struct",
        )
    }

    /// A raw message as the `OpenRTB` ext fields hold it.
    #[derive(Serialize)]
    struct RawBid {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        ext: Option<Box<RawValue>>,
    }

    /// `jsonutil.Marshal` writes a `json.RawMessage` through Prebid's encoder, which runs
    /// `json.Compact`: whitespace goes, nothing is escaped, and nothing at all is written for
    /// bytes that are not JSON; `encoding/json` compacts and HTML-escapes it and fails on invalid
    /// bytes. A `RawValue` is written as it was read, and cannot hold invalid bytes.
    // Go: main.go:23-25 (registers jsonutil.RawMessageExtension) and
    // util/jsonutil/jsonutil.go:223-251 (rawMessageCodec.Encode)
    #[test]
    fn go_difference_raw_message_not_compacted() -> TestResult {
        let doc = golden("json.json")?;
        let mut rows = Vec::new();
        for case in list(&doc, "/raw_message")? {
            let ext = text(case, "/ext")?;
            let mut row = vec![ext.to_owned()];
            row.extend(go_writers(at(case, "/writers")?)?);
            row.push(match RawValue::from_string(ext.to_owned()) {
                Ok(raw) => written(&RawBid {
                    id: "1".to_owned(),
                    ext: Some(raw),
                }),
                Err(e) => format!("error: {e}"),
            });
            rows.push(row);
        }
        check_table(
            &rows,
            &[
                &[
                    "{ \"a\" : \"<\" }",
                    "{\"id\":\"1\",\"ext\":{\"a\":\"<\"}}",
                    "{\"id\":\"1\",\"ext\":{\"a\":\"\\u003c\"}}",
                    "{\"id\":\"1\",\"ext\":{\"a\":\"<\"}}",
                    "{\"id\":\"1\",\"ext\":{ \"a\" : \"<\" }}",
                ],
                &[
                    "{\"a\":1}",
                    "{\"id\":\"1\",\"ext\":{\"a\":1}}",
                    "{\"id\":\"1\",\"ext\":{\"a\":1}}",
                    "{\"id\":\"1\",\"ext\":{\"a\":1}}",
                    "{\"id\":\"1\",\"ext\":{\"a\":1}}",
                ],
                &[
                    "{\"a\":",
                    "{\"id\":\"1\",\"ext\":}",
                    "error: json: error calling MarshalJSON for type json.RawMessage: unexpected \
                     end of JSON input",
                    "error: json: error calling MarshalJSON for type json.RawMessage: unexpected \
                     end of JSON input",
                    "error: EOF while parsing a value at line 1 column 5",
                ],
            ],
            "ext bytes, jsonutil.Marshal, json.Marshal, the endpoints' Encoder, serde_json",
        )
    }

    /// The generator of `testdata/go/floatparse.go`, call for call.
    struct SplitMix64 {
        state: u64,
    }

    impl SplitMix64 {
        const fn next(&mut self) -> u64 {
            self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// A number in `[0, n)`.
        const fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        /// A number in `[lo, hi]`.
        const fn between(&mut self, lo: i64, hi: i64) -> i64 {
            lo.wrapping_add_unsigned(self.below(hi.abs_diff(lo) + 1))
        }

        /// `n` random digits, the first one not zero when `lead` is set.
        fn digits(&mut self, n: i64, lead: bool) -> String {
            let mut out = String::new();
            for i in 0..n {
                let digit = if i == 0 && lead {
                    1 + self.below(9)
                } else {
                    self.below(10)
                };
                out.extend(char::from_digit(
                    u32::try_from(digit).unwrap_or_default(),
                    10,
                ));
            }
            out
        }

        /// An integer part of up to `max_digits` digits, `0` for none.
        fn integer(&mut self, max_digits: u64) -> String {
            match self.below(max_digits + 1) {
                0 => "0".to_owned(),
                n => self.digits(i64::try_from(n).unwrap_or_default(), true),
            }
        }

        /// An exponent in `[lo, hi]`, its marker and sign spelled at random.
        fn exponent(&mut self, lo: i64, hi: i64) -> String {
            let e = self.between(lo, hi);
            let marker = if self.below(2) == 1 { "E" } else { "e" };
            if e < 0 {
                format!("{marker}-{}", e.unsigned_abs())
            } else if self.below(2) == 1 {
                format!("{marker}+{e}")
            } else {
                format!("{marker}{e}")
            }
        }

        /// One JSON number: prices, integers, long fractions, exponents near both ends of the
        /// range, subnormals and small fractions, a quarter of them negative.
        fn number(&mut self) -> String {
            let body = match self.below(7) {
                0 => {
                    let int = self.integer(4);
                    let n = self.between(1, 6);
                    format!("{int}.{}", self.digits(n, false))
                }
                1 => {
                    let n = self.between(1, 25);
                    self.digits(n, true)
                }
                2 => {
                    let int = self.integer(3);
                    let n = self.between(10, 30);
                    format!("{int}.{}", self.digits(n, false))
                }
                3 => self.scientific(19, -330, 307),
                4 => {
                    let n = self.between(16, 19);
                    let mantissa = self.digits(n, true);
                    format!("{mantissa}{}", self.exponent(-340, 288))
                }
                5 => self.scientific(20, -324, -307),
                _ => {
                    let zeros = "0".repeat(usize::try_from(self.below(11)).unwrap_or_default());
                    let n = self.between(1, 20);
                    format!("0.{zeros}{}", self.digits(n, true))
                }
            };
            if self.below(4) == 0 {
                format!("-{body}")
            } else {
                body
            }
        }

        /// `d.ddd` with up to `max_decimals` decimals and an exponent in `[lo, hi]`.
        fn scientific(&mut self, max_decimals: i64, lo: i64, hi: i64) -> String {
            let lead = self.digits(1, true);
            let n = self.between(1, max_decimals);
            let decimals = self.digits(n, false);
            format!("{lead}.{decimals}{}", self.exponent(lo, hi))
        }
    }

    /// With the `float_roundtrip` feature `serde_json` reads every decimal number as the f64
    /// `strconv.ParseFloat` gives (jsoniter and `encoding/json` read the same, which the
    /// generator checks), over 50,000 seeded numbers and the edge list. Without the feature it
    /// may be one ulp off, which can move a bid across a price bucket.
    // Go: util/jsonutil/jsonutil.go:136-144 (Unmarshal; jsoniter's ReadFloat64 falls back to
    // strconv.ParseFloat) and encoding/json's literalStore (strconv.ParseFloat)
    #[test]
    fn same_as_go_float_parse() -> TestResult {
        let doc = golden("float_parse.json")?;
        let seed = text(&doc, "/seed")?;
        let mut generator = SplitMix64 {
            state: u64::from_str_radix(seed.trim_start_matches("0x"), 16)?,
        };
        let batch = number(&doc, "/batch")?;
        let batches = list(&doc, "/batches")?;
        let count = batch * u64::try_from(batches.len())?;
        check_eq(
            &count,
            &number(&doc, "/count")?,
            "the batches cover the corpus",
        )?;
        check(count >= 50_000, "50,000 inputs")?;
        for (index, digests) in batches.iter().enumerate() {
            let mut inputs = Sha256::new();
            let mut bits = Sha256::new();
            for _ in 0..batch {
                let input = generator.number();
                inputs.update(input.as_bytes());
                inputs.update(b"\n");
                let parsed: f64 = serde_json::from_str(&input)?;
                bits.update(format!("{:016x}\n", parsed.to_bits()).as_bytes());
            }
            check_eq(
                hex(&inputs.finalize()).as_str(),
                text(digests, "/inputs_sha256")?,
                &format!("batch {index}: the inputs Go generated"),
            )?;
            check_eq(
                hex(&bits.finalize()).as_str(),
                text(digests, "/bits_sha256")?,
                &format!("batch {index}: the bits Go read"),
            )?;
        }
        let edges = list(&doc, "/edges")?;
        check(edges.len() >= 40, "the edge list")?;
        for edge in edges {
            let input = text(edge, "/input")?;
            let parsed: f64 = serde_json::from_str(input)?;
            check_eq(
                format!("{:016x}", parsed.to_bits()).as_str(),
                text(edge, "/bits")?,
                &format!("the bits of {input}"),
            )?;
        }
        Ok(())
    }
}
