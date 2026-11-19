//! Decoding against Go (`testdata/golden/decode.json`): null into value and `json.RawMessage`
//! fields, numbers into `interface{}`, malformed raw values, lone surrogates, numbers beyond
//! `f64`'s range, Prebid's lenient readers (`StringInt`, `IntString`, `ParseIntoString`), Go's
//! typed integers, `-0` read into integers, raw control characters in strings, null as a whole
//! document and as a slice element or map value, a NUL byte after the document, floats written
//! with a leading `+`, what a failed decode leaves, and a decode into a value that already holds a
//! slice.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fmt::Write as _,
        sync::Arc,
    };

    use pbs_errortypes::GoError;
    use pbs_jsonutil::{
        IntString, MergeClone, MergeCloneError, StringInt, marshal, merge_clone, merge_field,
        merge_struct, null_default, parse_into_string, raw_compact, raw_keep_null, unmarshal,
        unmarshal_valid,
    };
    use serde::{Deserialize, Serialize, de::DeserializeOwned};
    use serde_json::{Value, value::RawValue};

    use super::support::{TestResult, ensure_eq, golden, list, text};

    /// One recorded call of a `decode.json` section.
    struct Row<'a> {
        name: &'a str,
        input: &'a str,
        result: &'a str,
        err: &'a str,
    }

    /// The rows of the section `name`.
    fn rows<'a>(doc: &'a Value, name: &str) -> TestResult<Vec<Row<'a>>> {
        list(doc, name)?
            .iter()
            .map(|row| {
                Ok(Row {
                    name: text(row, "/name")?,
                    input: text(row, "/input")?,
                    result: text(row, "/result")?,
                    err: text(row, "/err")?,
                })
            })
            .collect()
    }

    /// `unmarshal` or `unmarshal_valid`, as the row's Go decoder is `Unmarshal` or
    /// `UnmarshalValid`.
    fn decode<T: DeserializeOwned>(row: &Row<'_>) -> Result<T, String> {
        let decoded = if row.name.starts_with("jsonutil.UnmarshalValid") {
            unmarshal_valid(row.input.as_bytes())
        } else {
            unmarshal(row.input.as_bytes())
        };
        decoded.map_err(GoError::into_message)
    }

    /// Lower-case hex of `text`'s bytes, as Go's `hex.EncodeToString` writes them.
    fn hex(text: &str) -> String {
        text.bytes().fold(String::new(), |mut out, byte| {
            let _written = write!(out, "{byte:02x}");
            out
        })
    }

    /// Go's `idTarget`, as typegen writes a struct.
    #[derive(Debug, Default, PartialEq, Deserialize)]
    #[serde(default)]
    struct IdTarget {
        #[serde(deserialize_with = "null_default")]
        id: String,
        #[serde(deserialize_with = "null_default")]
        n: i64,
    }

    /// A derived struct reads a JSON array as its fields in order, and `[]` as its defaults; Go
    /// rejects an array where it reads a struct (`expect { or n, but found [`), so the request or
    /// response fails to decode there (`json.struct-from-array`).
    // Go: jsoniter reflect_struct_decoder.go (readObjectStart); gen/decode.go errorTexts
    #[test]
    fn struct_reads_array() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/error_texts")?;
        let mut arrays = 0;
        for row in rows.iter().filter(|row| row.input == "[]") {
            ensure_eq(&row.err, &"expect { or n, but found [", row.name)?;
            ensure_eq(&decode::<IdTarget>(row), &Ok(IdTarget::default()), row.name)?;
            arrays += 1;
        }
        ensure_eq(&arrays, &2, "decoders")?;
        let positional: IdTarget = unmarshal(br#"["x",5]"#)?;
        let want = IdTarget {
            id: "x".to_owned(),
            n: 5,
        };
        ensure_eq(&positional, &want, "an array of the fields")
    }

    /// Go's `nullTarget`: a value field of each kind and a pointer.
    #[derive(Debug, Default, PartialEq, Deserialize)]
    #[serde(default)]
    struct NullTarget {
        #[serde(deserialize_with = "null_default")]
        s: String,
        #[serde(deserialize_with = "null_default")]
        i: i64,
        #[serde(rename = "F", deserialize_with = "null_default")]
        f: f64,
        #[serde(deserialize_with = "null_default")]
        b: bool,
        #[serde(deserialize_with = "null_default")]
        st: Nested,
        l: Option<Vec<i64>>,
        m: Option<BTreeMap<String, i64>>,
        p: Option<i64>,
    }

    /// Go's `nullTarget.St`.
    #[derive(Debug, Default, PartialEq, Deserialize)]
    #[serde(default)]
    struct Nested {
        #[serde(deserialize_with = "null_default")]
        a: i64,
    }

    /// A string field without `null_default`, as a bare derive declares it.
    #[derive(Debug, Deserialize)]
    struct Bare {
        #[serde(rename = "s")]
        _s: String,
    }

    /// Null into each value field leaves its zero value through `unmarshal` and
    /// `unmarshal_valid`, the values Go's three decoders give; a bare derive rejects the document
    /// (`json.null-into-value`, restored).
    // Go: util/jsonutil/jsonutil.go:136-157 (Unmarshal, UnmarshalValid); gen/decode.go
    // nullIntoValue
    #[test]
    fn null_leaves_zero_value() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/null_into_value")?;
        for row in &rows {
            let go: NullTarget = unmarshal(row.result.as_bytes())?;
            ensure_eq(&decode::<NullTarget>(row), &Ok(go), row.name)?;
        }
        ensure_eq(&rows.len(), &3, "decoders")?;
        let bare = serde_json::from_str::<Bare>(r#"{"s":null}"#).map_err(|e| e.to_string());
        ensure_eq(
            &bare.err().as_deref(),
            &Some("invalid type: null, expected a string at line 1 column 9"),
            "a bare derive",
        )
    }

    /// Go's `rawOmit` and `rawPlain` as `jsonutil.Unmarshal` decodes them.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct RawField {
        #[serde(serialize_with = "raw_compact")]
        ext: Option<Box<RawValue>>,
    }

    /// Go's `rawOmit`.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct RawOmit {
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "raw_compact"
        )]
        ext: Option<Box<RawValue>>,
    }

    /// Go's `rawOmit` and `rawPlain` as `json.Unmarshal` decodes them: null is kept.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct RawKept {
        #[serde(
            skip_serializing_if = "Option::is_none",
            deserialize_with = "raw_keep_null",
            serialize_with = "raw_compact"
        )]
        ext: Option<Box<RawValue>>,
    }

    /// Go's `rawState`: nil, or the bytes quoted.
    fn raw_state(ext: Option<&RawValue>) -> String {
        ext.map_or_else(|| "nil".to_owned(), |raw| format!("{:?}", raw.get()))
    }

    /// A row of `null_raw` decoded and written by the Rust type for its Go decoder and field.
    fn null_raw(row: &Row<'_>) -> TestResult<String> {
        Ok(match row.name {
            "json.Unmarshal" | "json.Unmarshal omitempty" => {
                let kept: RawKept = decode(row)?;
                format!(
                    "{} {}",
                    raw_state(kept.ext.as_deref()),
                    String::from_utf8(marshal(&kept)?)?
                )
            }
            "jsonutil.Unmarshal" => {
                let plain: RawField = decode(row)?;
                format!(
                    "{} {}",
                    raw_state(plain.ext.as_deref()),
                    String::from_utf8(marshal(&plain)?)?
                )
            }
            _ => {
                let omit: RawOmit = decode(row)?;
                format!(
                    "{} {}",
                    raw_state(omit.ext.as_deref()),
                    String::from_utf8(marshal(&omit)?)?
                )
            }
        })
    }

    /// A `json.RawMessage` given null: through `raw_keep_null` it holds `null` and is written
    /// back as `null`, as `encoding/json` keeps it (`json.null-raw`, restored); a plain `Option`
    /// reads it as nothing, as `jsonutil.Unmarshal` (jsoniter) does, and writes nothing under
    /// `omitempty`. Without `omitempty` Go writes that nil as no value at all and this `null`
    /// (`json.nil-raw-written`).
    // Go: util/jsonutil/jsonutil.go:136-157; encoding/json RawMessage.UnmarshalJSON; gen/decode.go
    // nullRaw
    #[test]
    fn null_raw_kept() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/null_raw")?;
        for row in &rows {
            let rust = null_raw(row)?;
            let want = if row.name == "jsonutil.Unmarshal" {
                row.result.replace(r#""ext":}"#, r#""ext":null}"#)
            } else {
                row.result.to_owned()
            };
            ensure_eq(&rust, &want, row.name)?;
        }
        ensure_eq(&rows.len(), &5, "decoders")
    }

    /// Go's `anyTarget`.
    #[derive(Serialize, Deserialize)]
    struct AnyTarget {
        #[serde(default)]
        v: Value,
    }

    /// The number of each `interface_numbers` input as a `Value` writes it.
    const NUMBERS: [(&str, &str); 6] = [
        (r#"{"v":9007199254740993}"#, r#"{"v":9007199254740993}"#),
        (r#"{"v":1.0}"#, r#"{"v":1.0}"#),
        (r#"{"v":1e2}"#, r#"{"v":100.0}"#),
        (r#"{"v":1.5}"#, r#"{"v":1.5}"#),
        (
            r#"{"v":12345678901234567890}"#,
            r#"{"v":12345678901234567890}"#,
        ),
        (r#"{"v":[1,{"a":2.0}]}"#, r#"{"v":[1,{"a":2.0}]}"#),
    ];

    /// A number read into a `Value` keeps an integer exact, above 2^53 and up to `u64::MAX`, and
    /// a float as the `f64` it reads to; Go reads every number into `interface{}` as a float64,
    /// so 9007199254740993 comes back 9007199254740992 (`json.interface-numbers`; the spellings of
    /// floats are `json.float-format`).
    // Go: util/jsonutil/jsonutil.go:136-147; jsoniter's interface{} decoder; gen/decode.go
    // interfaceNumbers
    #[test]
    fn interface_numbers_kept_exact() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/interface_numbers")?;
        let mut same = 0;
        for (row, (input, want)) in rows.iter().zip(NUMBERS) {
            ensure_eq(&row.input, &input, "input")?;
            let any: AnyTarget = decode(row)?;
            let rust = String::from_utf8(marshal(&any)?)?;
            ensure_eq(&rust.as_str(), &want, input)?;
            if rust == row.result {
                same += 1;
            }
        }
        ensure_eq(
            &(rows.len(), same),
            &(6, 1),
            "inputs, and those Go writes alike",
        )
    }

    /// `json.RawMessage` fields: `jsonutil.Unmarshal` and `UnmarshalValid` (jsoniter) keep a
    /// number with a leading zero after its minus (`-01`) as it was written, where `serde_json`,
    /// like `encoding/json`, rejects the document (`json.unvalidated-raw`); every other malformed
    /// value fails on both sides.
    // Go: util/jsonutil/jsonutil.go:136-157 (jsoniter's RawMessage skipper); gen/decode.go
    // unvalidatedRaw
    #[test]
    fn malformed_raw_rejected() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/unvalidated_raw")?;
        let mut accepted = Vec::new();
        for row in &rows {
            let rust = if row.name == "json.Unmarshal" {
                decode::<RawKept>(row).map(|kept| raw_state(kept.ext.as_deref()))
            } else {
                decode::<RawOmit>(row).map(|omit| raw_state(omit.ext.as_deref()))
            };
            ensure_eq(
                &rust.is_err(),
                &true,
                &format!("{} {}", row.name, row.input),
            )?;
            if row.err.is_empty() {
                accepted.push((row.input, row.result));
            }
        }
        ensure_eq(
            &accepted,
            &vec![
                (r#"{"ext":-01}"#, r#""-01""#),
                (r#"{"ext":-01}"#, r#""-01""#),
                (r#"{"ext":[-01]}"#, r#""[-01]""#),
                (r#"{"ext":[-01]}"#, r#""[-01]""#),
            ],
            "what Go's jsonutil decoders accept",
        )
    }

    /// Go's `stringTarget`.
    #[derive(Deserialize)]
    struct StringTarget {
        s: String,
    }

    /// A lone surrogate escape decodes to U+FFFD, in values and keys, as both Go decoders give
    /// it, where `serde_json` alone rejects the document (`json.lone-surrogates`, restored by
    /// `replace_lone_surrogates`); pairs and escaped backslashes read as they always did.
    // Go: util/jsonutil/jsonutil.go:136-147; encoding/json decode.go unquoteBytes; gen/decode.go
    // loneSurrogates
    #[test]
    fn lone_surrogates_decode_to_replacement() -> TestResult {
        let doc = golden("decode.json")?;
        let mut compared = 0;
        let mut lone = 0;
        for row in rows(&doc, "/lone_surrogates")? {
            let rust = match row.name {
                "jsonutil.Unmarshal" => decode::<StringTarget>(&row)?.s,
                "jsonutil.Unmarshal key" => {
                    let map: BTreeMap<String, i64> = decode(&row)?;
                    map.into_keys().collect()
                }
                _ => continue,
            };
            ensure_eq(&hex(&rust).as_str(), &row.result, row.input)?;
            let replaced = rust.contains('\u{fffd}');
            ensure_eq(
                &serde_json::from_str::<Value>(row.input).is_err(),
                &replaced,
                row.input,
            )?;
            compared += 1;
            lone += usize::from(replaced);
        }
        ensure_eq(
            &(compared, lone),
            &(13, 10),
            "inputs, and those with a lone surrogate",
        )
    }

    /// A surrogate escape followed by another `\u` escape it does not pair with reads as jsoniter
    /// (`jsonutil.Unmarshal`) reads it: U+FFFD for the surrogate and the second escape read on
    /// its own, not paired with what follows it. `encoding/json` pairs that second escape with
    /// the next, and gjson (`IntString`, `ParseIntoString`) turns the two escapes into one U+FFFD,
    /// dropping the character the second one spells (`json.surrogate-pairing`).
    // Go: jsoniter iter_str.go readEscapedChar; encoding/json decode.go unquoteBytes; gjson.go
    // unescape; gen/decode.go loneSurrogates, intStrings, parseIntoStrings
    #[test]
    fn surrogate_pairing_follows_jsoniter() -> TestResult {
        let doc = golden("decode.json")?;
        let lone = rows(&doc, "/lone_surrogates")?;
        // What jsoniter reads of each string, by the string's JSON text.
        let jsoniter_reads: BTreeMap<&str, &str> = lone
            .iter()
            .filter(|row| row.name == "jsonutil.Unmarshal")
            .filter_map(|row| {
                let string = row.input.strip_prefix(r#"{"s":"#)?.strip_suffix('}')?;
                Some((string, row.result))
            })
            .collect();
        let mut gjson = Vec::new();
        for row in rows(&doc, "/int_string")? {
            let Some(string) = surrogate_string(row.input, r#"{"item_id": "#) else {
                continue;
            };
            let rust = decode::<IntStringItem>(&row)?.item_id.0;
            let jsoniter = jsoniter_reads
                .get(string)
                .ok_or_else(|| format!("no jsoniter row for {string}"))?;
            ensure_eq(&hex(&rust).as_str(), jsoniter, row.input)?;
            gjson.push(("IntString", hex(row.result)));
        }
        for row in rows(&doc, "/parse_into_string")? {
            let Some(string) = surrogate_string(row.input, "") else {
                continue;
            };
            let mut target = None;
            parse_into_string(row.input.as_bytes(), Some(&mut target))?;
            let rust = target.ok_or_else(|| format!("{}: nothing read", row.input))?;
            let jsoniter = jsoniter_reads
                .get(string)
                .ok_or_else(|| format!("no jsoniter row for {string}"))?;
            ensure_eq(&hex(&rust).as_str(), jsoniter, row.input)?;
            let go = row
                .result
                .strip_prefix('"')
                .and_then(|inner| inner.strip_suffix('"'));
            gjson.push((
                "ParseIntoString",
                hex(go.ok_or("Go's result is not quoted")?),
            ));
        }
        let gjson_reads = [
            ("IntString", "efbfbd".to_owned()),
            ("IntString", "efbfbdefbfbd".to_owned()),
            ("ParseIntoString", "efbfbd".to_owned()),
            ("ParseIntoString", "efbfbdefbfbd".to_owned()),
        ];
        ensure_eq(gjson.as_slice(), gjson_reads.as_slice(), "what gjson reads")?;
        let mut differ = Vec::new();
        for [jsoniter, standard] in lone.as_chunks::<2>().0 {
            ensure_eq(&standard.name, &"json.Unmarshal", standard.input)?;
            let rust = hex(&decode::<StringTarget>(standard)?.s);
            ensure_eq(&rust.as_str(), &jsoniter.result, standard.input)?;
            if standard.result != jsoniter.result {
                differ.push((standard.input, standard.result));
            }
        }
        ensure_eq(
            &differ,
            &vec![
                (r#"{"s":"\ud800\ud800\udc00"}"#, "efbfbdf0908080"),
                (r#"{"s":"\udc00\ud800\udc00"}"#, "efbfbdf0908080"),
            ],
            "inputs encoding/json reads otherwise",
        )
    }

    /// The JSON string in `input` after `prefix` (and before a closing brace, if `prefix` opens
    /// an object), when it holds a lone surrogate escape.
    fn surrogate_string<'a>(input: &'a str, prefix: &str) -> Option<&'a str> {
        let rest = input.strip_prefix(prefix)?;
        let string = if prefix.is_empty() {
            rest
        } else {
            rest.strip_suffix('}')?
        };
        string.contains("\\ud800").then_some(string)
    }

    /// A lone surrogate escape inside a `json.RawMessage` is written back as `\uFFFD`, the
    /// rewrite that lets `serde_json` read the document; Go keeps the bytes as they came
    /// (`json.lone-surrogates-raw`). Both read back to the same value.
    // Go: util/jsonutil/jsonutil.go:229-255 (RawMessageExtension); gen/decode.go rawCompact
    #[test]
    fn lone_surrogates_in_raw_rewritten() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/raw_compact")?;
        let row = rows
            .iter()
            .find(|row| row.input.contains("\\ud800"))
            .ok_or("no raw row with a lone surrogate")?;
        let omit: RawOmit = decode(row)?;
        let rust = String::from_utf8(marshal(&omit)?)?;
        ensure_eq(
            &rust.as_str(),
            &"{\"ext\":[\"\\uFFFD\",\"x\\uFFFD\"]}",
            row.input,
        )?;
        let go: Value = unmarshal(row.result.as_bytes())?;
        ensure_eq(&serde_json::from_str::<Value>(&rust)?, &go, "the values")
    }

    /// Go's `stringIntItem`.
    #[derive(Deserialize)]
    struct StringIntItem {
        #[serde(default)]
        item_id: StringInt,
    }

    /// A `StringInt` row decoded: the value, or that it failed.
    fn string_int(row: &Row<'_>) -> Result<String, String> {
        decode::<StringIntItem>(row).map(|item| item.item_id.0.to_string())
    }

    /// `TestStringIntUnmarshalJSON` and the edges: a number or a string holding one reads, the
    /// empty string reads 0, and anything else fails, null included, as Go's decoder gives them
    /// (the texts are `json.error-texts`), except the two spellings `json.string-int-parse` names.
    // Go: util/jsonutil/stringInt.go:9-29, stringInt_test.go:10-41; gen/decode.go stringInts
    #[test]
    fn string_int_rows_match_go() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/string_int")?;
        for row in &rows {
            if row.input.contains(r#""-""#) || row.input.contains(r#""+5""#) {
                continue;
            }
            let rust = string_int(row);
            if row.err.is_empty() {
                ensure_eq(&rust, &Ok(row.result.to_owned()), row.input)?;
            } else {
                ensure_eq(&rust.is_err(), &true, row.input)?;
            }
        }
        ensure_eq(&rows.len(), &18, "inputs")
    }

    /// `StringInt` parses with `i64::from_str`: `"+5"` reads 5, where Go's `jsonparser.ParseInt`
    /// fails, and `"-"` fails, where Go reads 0 (`json.string-int-parse`).
    // Go: util/jsonutil/stringInt.go:9-29 (jsonparser.ParseInt); gen/decode.go stringInts
    #[test]
    fn string_int_parse_differs() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/string_int")?;
        let find = |input: &str| {
            rows.iter()
                .find(|row| row.input == input)
                .ok_or_else(|| format!("no row {input}"))
        };
        let minus = find(r#"{"item_id":"-"}"#)?;
        ensure_eq(
            &(minus.result, minus.err),
            &("0", ""),
            "Go reads a lone minus",
        )?;
        ensure_eq(&string_int(minus).is_err(), &true, "a lone minus")?;
        let plus = find(r#"{"item_id":"+5"}"#)?;
        ensure_eq(&plus.err.is_empty(), &false, "Go fails on a plus sign")?;
        ensure_eq(&string_int(plus), &Ok("5".to_owned()), "a plus sign")
    }

    /// Go's `intStringItem`.
    #[derive(Deserialize)]
    struct IntStringItem {
        #[serde(default)]
        item_id: IntString,
    }

    /// `TestIntStringUnmarshalJSON` and the edges: a string reads as its text and a number as
    /// gjson writes it (an integer as written, else the shortest `f64` form); null, booleans and
    /// arrays fail, with Prebid's `invalid type`. The strings with a lone surrogate are
    /// `json.surrogate-pairing`'s.
    // Go: util/jsonutil/intstring.go:11-19, intstring_test.go:10-58; gen/decode.go intStrings
    #[test]
    fn int_string_rows_match_go() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/int_string")?;
        for row in &rows {
            if surrogate_string(row.input, r#"{"item_id": "#).is_some() {
                continue;
            }
            let rust = decode::<IntStringItem>(row).map(|item| item.item_id.0);
            if row.err.is_empty() {
                ensure_eq(&rust, &Ok(row.result.to_owned()), row.input)?;
            } else {
                let rust = rust
                    .err()
                    .ok_or_else(|| format!("{}: read where Go failed", row.input))?;
                ensure_eq(
                    &rust.starts_with("invalid type"),
                    &row.err.ends_with(": invalid type"),
                    row.input,
                )?;
            }
        }
        ensure_eq(&rows.len(), &12, "inputs")
    }

    /// `ParseIntoString` on one input: the string it sets, as Go's `%q` would show it, or nil.
    fn parse_into(input: &str) -> TestResult<String> {
        let mut target = None;
        parse_into_string(input.as_bytes(), Some(&mut target))?;
        Ok(target.map_or_else(|| "nil".to_owned(), |text| format!("{text:?}")))
    }

    /// Inputs gjson reads a value out of although they are not JSON.
    const LENIENT: [&str; 5] = [r#""abc"#, "1x", "+5", "nan", "tru"];

    /// `Test_ParseIntoString`, `Test_ParseIntoNilStringError` and the edges: JSON values give
    /// Go's strings (an object or array its bytes to the end of the input, a number beyond
    /// `f64`'s range `+Inf`), null and nothing leave the target unset, and a nil target fails
    /// with Go's text. The strings with a lone surrogate are `json.surrogate-pairing`'s.
    // Go: util/jsonutil/forcestring.go:11-20, forcestring_test.go:10-61; gen/decode.go
    // parseIntoStrings
    #[test]
    fn parse_into_string_rows_match_go() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/parse_into_string")?;
        for row in &rows {
            if LENIENT.contains(&row.input) || surrogate_string(row.input, "").is_some() {
                continue;
            }
            if row.name == "jsonutil.ParseIntoString nil" {
                let error =
                    parse_into_string(row.input.as_bytes(), None).map_err(GoError::into_message);
                ensure_eq(&error, &Err(row.err.to_owned()), row.name)?;
                continue;
            }
            ensure_eq(&parse_into(row.input)?.as_str(), &row.result, row.input)?;
        }
        ensure_eq(&rows.len(), &22, "inputs")
    }

    /// Bytes that are not JSON leave the target unset, where gjson reads a value out of them:
    /// an unterminated string, a number with bytes after it, a plus sign, `nan` and a literal cut
    /// short (`json.parse-into-string-lenient`).
    // Go: util/jsonutil/forcestring.go:11-20 (gjson.ParseBytes); gen/decode.go parseIntoStrings
    #[test]
    fn parse_into_string_lenient_inputs_unset() -> TestResult {
        let doc = golden("decode.json")?;
        let mut seen = 0;
        for row in rows(&doc, "/parse_into_string")? {
            if !LENIENT.contains(&row.input) {
                continue;
            }
            ensure_eq(&(row.result == "nil"), &false, row.input)?;
            ensure_eq(&parse_into(row.input)?.as_str(), &"nil", row.input)?;
            seen += 1;
        }
        ensure_eq(&seen, &LENIENT.len(), "lenient inputs")
    }

    /// A number beyond `f64`'s range where `serde_json` does not parse it as a number (in a
    /// `json.RawMessage`, an unknown member and the bytes `IntString` reads) reads, as
    /// `encoding/json` reads it, where jsoniter (`jsonutil.Unmarshal`, `UnmarshalValid`) fails the
    /// document with `invalid number`, and so does one that starts with 0 beyond `f32`'s range
    /// (`0.5e39`), which jsoniter's skipper parses as a float32 (`value out of range`); `f64`'s
    /// largest value, a number that underflows and an integer of digits only read on every side
    /// (`json.out-of-range-numbers`).
    // Go: jsoniter iter_skip.go:69-104 (Skip), iter_skip_strict.go:10-22 (skipNumber) and
    // iter_float.go:159-185 (readNumberAsString); gen/decode.go outOfRangeNumbers
    #[test]
    fn out_of_range_numbers_read() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/out_of_range_numbers")?;
        let mut failed = Vec::new();
        for row in &rows {
            let rust = if row.name.ends_with("unknown member") {
                decode::<StringTarget>(row).map(|target| target.s)
            } else if row.name.ends_with("IntString") {
                decode::<IntStringItem>(row).map(|item| item.item_id.0)
            } else if row.name == "json.Unmarshal" {
                decode::<RawKept>(row).map(|kept| raw_state(kept.ext.as_deref()))
            } else {
                decode::<RawField>(row).map(|plain| raw_state(plain.ext.as_deref()))
            };
            let what = format!("{} {}", row.name, row.input);
            ensure_eq(&rust, &Ok(row.result.to_owned()), &what)?;
            if !row.err.is_empty() {
                let range =
                    row.err.ends_with("invalid number") || row.err.ends_with("value out of range");
                ensure_eq(&range, &true, &what)?;
                failed.push((row.name, row.input));
            }
        }
        ensure_eq(&rows.len(), &34, "rows")?;
        ensure_eq(
            &failed,
            &vec![
                ("jsonutil.Unmarshal", r#"{"ext":1e400}"#),
                ("jsonutil.UnmarshalValid", r#"{"ext":1e400}"#),
                ("jsonutil.Unmarshal", r#"{"ext":[-1e309]}"#),
                ("jsonutil.UnmarshalValid", r#"{"ext":[-1e309]}"#),
                ("jsonutil.Unmarshal", r#"{"ext":{"a":1.8e308}}"#),
                ("jsonutil.UnmarshalValid", r#"{"ext":{"a":1.8e308}}"#),
                ("jsonutil.Unmarshal", r#"{"ext":0.5e39}"#),
                ("jsonutil.UnmarshalValid", r#"{"ext":0.5e39}"#),
                (
                    "jsonutil.Unmarshal unknown member",
                    r#"{"zz":1e400,"s":"x"}"#,
                ),
                (
                    "jsonutil.Unmarshal unknown member",
                    r#"{"s":"x","zz":[1,{"a":-1e400}]}"#,
                ),
                ("jsonutil.UnmarshalValid IntString", r#"{"item_id":1e400}"#),
                ("jsonutil.UnmarshalValid IntString", r#"{"item_id":-1e400}"#),
                (
                    "jsonutil.UnmarshalValid IntString",
                    r#"{"item_id":0.165e308}"#,
                ),
            ],
            "what jsoniter fails on",
        )
    }

    /// Go's `typedInt8` (`type typedInt8 int8`), as typegen writes a Go typed integer: a
    /// transparent newtype, never a Rust enum, so any `int8` reads, as in Go.
    #[derive(Debug, Default, PartialEq, Eq, Deserialize)]
    #[serde(transparent)]
    struct TypedInt8(i8);

    /// Go's `typedTarget`.
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct TypedTarget {
        #[serde(deserialize_with = "null_default")]
        v: TypedInt8,
    }

    /// A Go typed integer as a transparent newtype reads any integer of its width, rejects one
    /// beyond it, a float and a string as Go does, and reads null as zero.
    // Go: jsoniter's int8 decoder (ReadInt8); gen/decode.go typedInts
    #[test]
    fn typed_ints_accept_any_integer() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/typed_ints")?;
        for row in &rows {
            let rust = decode::<TypedTarget>(row).map(|target| target.v.0.to_string());
            if row.err.is_empty() {
                ensure_eq(&rust, &Ok(row.result.to_owned()), row.input)?;
            } else {
                ensure_eq(&rust.is_err(), &true, row.input)?;
            }
        }
        ensure_eq(&rows.len(), &6, "inputs")
    }

    /// Go's `intTarget`: an `int`, an `int8`, a `uint` and a typed `int8` (as openrtb2's enums
    /// are), a pointer to an `int` and a `float64`, as typegen writes them.
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct IntTarget {
        #[serde(deserialize_with = "null_default")]
        i: i64,
        #[serde(rename = "i8", deserialize_with = "null_default")]
        small: i8,
        #[serde(deserialize_with = "null_default")]
        u: u64,
        #[serde(deserialize_with = "null_default")]
        t: TypedInt8,
        p: Option<i64>,
        #[serde(deserialize_with = "null_default")]
        f: f64,
    }

    impl MergeClone for IntTarget {
        fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
            merge_struct(patch, |key, value| match key {
                "i" => merge_field(&mut self.i, value, "main.intTarget", "I"),
                "i8" => merge_field(&mut self.small, value, "main.intTarget", "I8"),
                "u" => merge_field(&mut self.u, value, "main.intTarget", "U"),
                "t" => merge_field(&mut self.t, value, "main.intTarget", "T"),
                "p" => merge_field(&mut self.p, value, "main.intTarget", "P"),
                "f" => merge_field(&mut self.f, value, "main.intTarget", "F"),
                _ => Ok(()),
            })
        }
    }

    /// `-0` read into an integer field fails the document: `serde_json` parses it as the float
    /// -0.0, which every integer type rejects, where Go's three decoders read 0 into a signed
    /// integer, a typed one and a pointer to one, and `MergeClone` writes that 0
    /// (`json.negative-zero-int`). Both sides reject `-0` for an unsigned integer and read it as
    /// -0 into a float.
    // Go: jsoniter iter_int.go:41 (ReadInt8), :221-237 (ReadInt64), :126 and :248 (readUint32 and
    // readUint64 read a single zero); encoding/json decode.go:1003 (strconv.ParseInt), :1011
    // (strconv.ParseUint); gen/decode.go negativeZeros
    #[test]
    fn negative_zero_rejected_for_integers() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/negative_zero")?;
        let mut go_reads = Vec::new();
        for row in &rows {
            let what = format!("{} {}", row.name, row.input);
            let rust = if row.name == "jsonutil.MergeClone" {
                let mut target = IntTarget {
                    i: 5,
                    small: 5,
                    u: 5,
                    t: TypedInt8(5),
                    p: None,
                    f: 5.0,
                };
                merge_clone(&mut target, row.input.as_bytes())
                    .map(|()| target)
                    .map_err(GoError::into_message)
            } else {
                decode::<IntTarget>(row)
            };
            if row.input == r#"{"f":-0}"# {
                ensure_eq(
                    &(row.err, row.result.ends_with(r#""f":-0}"#)),
                    &("", true),
                    &what,
                )?;
                ensure_eq(&rust?.f.is_sign_negative(), &true, &what)?;
                continue;
            }
            let error = rust
                .err()
                .ok_or_else(|| format!("{what}: read where Go reads 0 or fails"))?;
            ensure_eq(
                &error.contains("invalid type: floating point `-0.0`, expected"),
                &true,
                &what,
            )?;
            if row.err.is_empty() {
                go_reads.push((row.name, row.input));
            }
        }
        ensure_eq(&rows.len(), &24, "rows")?;
        let signed = [r#"{"i":-0}"#, r#"{"i8":-0}"#, r#"{"t":-0}"#, r#"{"p":-0}"#];
        let decoders = [
            "jsonutil.Unmarshal",
            "jsonutil.UnmarshalValid",
            "json.Unmarshal",
            "jsonutil.MergeClone",
        ];
        let want: Vec<(&str, &str)> = signed
            .iter()
            .flat_map(|input| decoders.iter().map(move |name| (*name, *input)))
            .collect();
        ensure_eq(&go_reads, &want, "what Go reads as 0")
    }

    /// Go's `controlTarget`: a string, a `json.RawMessage` and a map's keys.
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct ControlTarget {
        #[serde(deserialize_with = "null_default")]
        s: String,
        ext: Option<Box<RawValue>>,
        m: Option<BTreeMap<String, i64>>,
    }

    impl MergeClone for ControlTarget {
        fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
            merge_struct(patch, |key, value| match key {
                "s" => merge_field(&mut self.s, value, "main.controlTarget", "S"),
                "ext" => merge_field(&mut self.ext, value, "main.controlTarget", "Ext"),
                "m" => merge_field(&mut self.m, value, "main.controlTarget", "M"),
                _ => Ok(()),
            })
        }
    }

    /// `merge_clone` into a `ControlTarget`, or the row's decoder, failing: the error, as the
    /// control-character rows expect it.
    fn control_error(row: &Row<'_>) -> TestResult {
        let what = format!("{} {:?}", row.name, row.input);
        let error = if row.name.starts_with("jsonutil.MergeClone") {
            merge_clone(&mut ControlTarget::default(), row.input.as_bytes())
                .err()
                .map(GoError::into_message)
        } else {
            decode::<ControlTarget>(row).err()
        };
        let error = error.ok_or_else(|| format!("{what}: read"))?;
        ensure_eq(
            &error.starts_with("control character (\\u0000-\\u001F) found while parsing"),
            &true,
            &what,
        )
    }

    /// A raw control character (U+0000 to U+001F) inside a string fails the document wherever
    /// it is, as `encoding/json` fails it, where jsoniter (`jsonutil.Unmarshal`, `UnmarshalValid`
    /// and `MergeClone`) reads one that follows an escape in its string: in a field, an unknown
    /// member, a `json.RawMessage` (kept as written) and a map key, and one anywhere in a member
    /// name of a struct of ten fields or fewer (`json.control-characters`). One before every
    /// escape of a value, a map key or a larger struct's member name fails on both sides. The Rust
    /// type is the same for every row: `serde_json` rejects the byte whatever it reads into.
    // Go: jsoniter iter_str.go:9-33 (ReadString checks only up to the first backslash), :35-52
    // (readStringSlowPath), iter_skip_strict.go:61-83 (skipString falls back to ReadString),
    // reflect_struct_decoder.go:47 (createStructDecoder) and iter_object.go:49-96 (readFieldHash
    // checks no byte); encoding/json scanner.go:339-351 (stateInString); gen/decode.go
    // controlCharacters, controlMemberNames
    #[test]
    fn raw_control_characters_rejected() -> TestResult {
        let doc = golden("decode.json")?;
        let names = rows(&doc, "/control_member_names")?;
        let mut small_reads = 0;
        for row in &names {
            control_error(row)?;
            let small = row.name.starts_with("jsonutil.") && !row.name.ends_with(" big");
            ensure_eq(&row.err.is_empty(), &small, row.input)?;
            small_reads += usize::from(small);
        }
        ensure_eq(
            &(names.len(), small_reads),
            &(21, 9),
            "member-name rows, and those jsoniter reads",
        )?;
        let rows = rows(&doc, "/control_characters")?;
        let mut go_reads = Vec::new();
        for row in &rows {
            control_error(row)?;
            let what = format!("{} {:?}", row.name, row.input);
            if row.name == "json.Unmarshal" {
                ensure_eq(&row.err.is_empty(), &false, &what)?;
            } else if row.err.is_empty() {
                go_reads.push(row.input);
            }
        }
        ensure_eq(&rows.len(), &36, "rows")?;
        let before_escape = ["{\"s\":\"a\tb\"}", "{\"zz\":\"\tq\",\"s\":\"x\"}"];
        let after_escape: BTreeSet<&str> = go_reads.iter().copied().collect();
        ensure_eq(
            &(go_reads.len(), after_escape.len()),
            &(21, 7),
            "rows and inputs jsoniter reads",
        )?;
        let inputs: BTreeSet<&str> = rows.iter().map(|row| row.input).collect();
        let rejected: Vec<&str> = inputs.difference(&after_escape).copied().collect();
        ensure_eq(
            rejected.as_slice(),
            before_escape.as_slice(),
            "inputs jsoniter rejects",
        )
    }

    impl MergeClone for IdTarget {
        fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
            merge_struct(patch, |key, value| match key {
                "id" => merge_field(&mut self.id, value, "main.idTarget", "ID"),
                "n" => merge_field(&mut self.n, value, "main.idTarget", "N"),
                _ => Ok(()),
            })
        }
    }

    /// A document that is JSON null, whitespace around it or not, fails to decode into a
    /// struct, a map, a `Vec`, a `String` or a number (`invalid type: null`) and reads `None`
    /// into an `Option`, where Go's three decoders return no error and leave the target's zero
    /// value: a struct and a number as they were, a map and a slice nil, and a string empty
    /// (jsoniter) or as it was (`encoding/json`) (`json.null-document`). `merge_clone` leaves the
    /// value as it was, as `MergeClone` does.
    // Go: jsoniter config.go:345-357 (Unmarshal), reflect.go:62-80 (ReadVal) and its decoders'
    // null branches; encoding/json decode.go:897-907 (literalStore's null); gen/decode.go
    // nullDocuments
    #[test]
    fn null_document_rejected() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/null_documents")?;
        for row in &rows {
            let what = format!("{} {:?}", row.name, row.input);
            ensure_eq(&row.err, &"", &what)?;
            if row.name == "jsonutil.MergeClone struct" {
                let mut target = IdTarget {
                    id: "keep".to_owned(),
                    n: 7,
                };
                merge_clone(&mut target, row.input.as_bytes())?;
                let go: IdTarget = unmarshal(row.result.as_bytes())?;
                ensure_eq(&target, &go, &what)?;
                continue;
            }
            let kind = row.name.rsplit(' ').next().unwrap_or_default();
            let error = match kind {
                "struct" => decode::<IdTarget>(row).err(),
                "map" => decode::<BTreeMap<String, i64>>(row).err(),
                "slice" => decode::<Vec<i64>>(row).err(),
                "string" => decode::<String>(row).err(),
                "int" => decode::<i64>(row).err(),
                _ => return Err(format!("{what}: no Rust target").into()),
            };
            let error = error.ok_or_else(|| format!("{what}: read"))?;
            ensure_eq(&error.starts_with("invalid type: null"), &true, &what)?;
            ensure_eq(&decode::<Option<IdTarget>>(row), &Ok(None), &what)?;
        }
        ensure_eq(&rows.len(), &32, "rows")
    }

    /// Go's `elementFormat`.
    #[derive(Debug, Default, PartialEq, Eq, Deserialize)]
    #[serde(default)]
    struct ElementFormat {
        #[serde(deserialize_with = "null_default")]
        w: i64,
    }

    impl MergeClone for ElementFormat {
        fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
            merge_struct(patch, |key, value| match key {
                "w" => merge_field(&mut self.w, value, "main.elementFormat", "W"),
                _ => Ok(()),
            })
        }
    }

    impl MergeClone for TypedInt8 {
        fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
            self.0.merge_clone(patch)
        }
    }

    /// Go's `elementTarget` as typegen writes it: its slices and maps stay `Option`, so nil and
    /// empty differ.
    #[derive(Debug, Default, PartialEq, Eq, Deserialize)]
    #[serde(default)]
    struct ElementTarget {
        l: Option<Vec<i64>>,
        s: Option<Vec<String>>,
        f: Option<Vec<ElementFormat>>,
        t: Option<Vec<TypedInt8>>,
        p: Option<Vec<Option<i64>>>,
        m: Option<BTreeMap<String, i64>>,
        ms: Option<BTreeMap<String, String>>,
    }

    impl MergeClone for ElementTarget {
        fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
            const GO: &str = "main.elementTarget";
            merge_struct(patch, |key, value| match key {
                "l" => merge_field(&mut self.l, value, GO, "L"),
                "s" => merge_field(&mut self.s, value, GO, "S"),
                "f" => merge_field(&mut self.f, value, GO, "F"),
                "t" => merge_field(&mut self.t, value, GO, "T"),
                "p" => merge_field(&mut self.p, value, GO, "P"),
                "m" => merge_field(&mut self.m, value, GO, "M"),
                "ms" => merge_field(&mut self.ms, value, GO, "MS"),
                _ => Ok(()),
            })
        }
    }

    /// null as an element of a slice, or a value of a map, whose type is no `Option` fails the
    /// document (`invalid type: null`): `null_default` covers fields only. Go's three decoders
    /// leave the element's zero value there, as they do for a field (`json.null-elements`). An
    /// `Option` element reads `None`, Go's nil, and `merge_clone` reads every row as `MergeClone`
    /// does.
    // Go: jsoniter reflect_slice.go (sliceDecoder.doDecode), reflect_map.go (mapDecoder.Decode)
    // and the element decoders' null branches; encoding/json decode.go:897-907 (literalStore's
    // null); gen/decode.go nullElements
    #[test]
    fn null_elements_rejected() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/null_elements")?;
        let mut rejected = BTreeSet::new();
        for row in &rows {
            let what = format!("{} {}", row.name, row.input);
            ensure_eq(&row.err, &"", &what)?;
            let go: ElementTarget = unmarshal(row.result.as_bytes())?;
            if row.name == "jsonutil.MergeClone" {
                let mut rust = ElementTarget::default();
                merge_clone(&mut rust, row.input.as_bytes())?;
                ensure_eq(&rust, &go, &what)?;
                continue;
            }
            match decode::<ElementTarget>(row) {
                Ok(rust) => ensure_eq(&rust, &go, &what)?,
                Err(error) => {
                    ensure_eq(&error.starts_with("invalid type: null"), &true, &what)?;
                    let _new = rejected.insert(row.input);
                }
            }
        }
        ensure_eq(&rows.len(), &28, "rows")?;
        ensure_eq(
            &rejected.into_iter().collect::<Vec<_>>(),
            &vec![
                r#"{"f":[null,{"w":1}]}"#,
                r#"{"l":[1,null,2]}"#,
                r#"{"m":{"a":null,"b":1}}"#,
                r#"{"ms":{"a":null}}"#,
                r#"{"s":["a",null]}"#,
                r#"{"t":[null,5]}"#,
            ],
            "inputs serde rejects",
        )
    }

    /// A NUL byte after the document fails it (`trailing characters`), as `encoding/json` fails
    /// it, where jsoniter (`jsonutil.Unmarshal`, `UnmarshalValid` and `MergeClone`) takes the NUL
    /// for the end of the input and ignores every byte after it (`json.trailing-nul`). A NUL
    /// before or inside the value fails on both sides.
    // Go: jsoniter config.go:345-357 (Unmarshal ends at the 0 nextToken returns), iter.go:181-197
    // (nextToken returns a NUL byte as it returns the end); util/jsonutil/merge.go:31-39
    // (MergeClone); gen/decode.go trailingNuls
    #[test]
    fn trailing_nul_rejected() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/trailing_nuls")?;
        let mut go_reads = BTreeSet::new();
        for row in &rows {
            let what = format!("{} {:?}", row.name, row.input);
            let error = if row.name == "jsonutil.MergeClone" {
                merge_clone(&mut IdTarget::default(), row.input.as_bytes())
                    .err()
                    .map(GoError::into_message)
            } else {
                decode::<IdTarget>(row).err()
            };
            let error = error.ok_or_else(|| format!("{what}: read"))?;
            if row.err.is_empty() {
                ensure_eq(&error.starts_with("trailing characters"), &true, &what)?;
                let _new = go_reads.insert(row.input);
            }
        }
        ensure_eq(&rows.len(), &16, "rows")?;
        ensure_eq(
            &go_reads.into_iter().collect::<Vec<_>>(),
            &vec!["{\"id\":\"x\"}\u{0}", "{\"id\":\"x\"} \u{0} not JSON"],
            "inputs jsoniter reads",
        )
    }

    /// Go's `floatTarget`.
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct FloatTarget {
        #[serde(deserialize_with = "null_default")]
        f: f64,
        #[serde(deserialize_with = "null_default")]
        f32: f32,
        #[serde(deserialize_with = "null_default")]
        i: i64,
    }

    impl MergeClone for FloatTarget {
        fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
            merge_struct(patch, |key, value| match key {
                "f" => merge_field(&mut self.f, value, "main.floatTarget", "F"),
                "f32" => merge_field(&mut self.f32, value, "main.floatTarget", "F32"),
                "i" => merge_field(&mut self.i, value, "main.floatTarget", "I"),
                _ => Ok(()),
            })
        }
    }

    /// A number written with a leading `+` fails the document, as `encoding/json` fails it,
    /// where jsoniter (`jsonutil.Unmarshal`, `UnmarshalValid` and `MergeClone`) reads it into a
    /// float field through `strconv.ParseFloat`: `+1.5`, `+.5`, `+1e2`, and `-+1` as -1
    /// (`json.plus-float`). An integer field and a skipped member reject it on both sides.
    // Go: jsoniter iter_float.go:207-214 (ReadFloat64), :299-337 (readFloat64SlowPath and
    // validateFloat), iter_skip.go:72-96 (Skip); gen/decode.go plusFloats
    #[test]
    fn plus_floats_rejected() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/plus_floats")?;
        let mut go_reads = BTreeSet::new();
        for row in &rows {
            let what = format!("{} {}", row.name, row.input);
            let error = if row.name == "jsonutil.MergeClone" {
                merge_clone(&mut FloatTarget::default(), row.input.as_bytes())
                    .err()
                    .map(GoError::into_message)
            } else {
                decode::<FloatTarget>(row).err()
            };
            let error = error.ok_or_else(|| format!("{what}: read"))?;
            let syntax = error.starts_with("expected value") || error.starts_with("invalid number");
            ensure_eq(&syntax, &true, &what)?;
            if row.err.is_empty() {
                let _new = go_reads.insert(row.input);
            }
        }
        ensure_eq(&rows.len(), &32, "rows")?;
        ensure_eq(
            &go_reads.into_iter().collect::<Vec<_>>(),
            &vec![
                r#"{"f":+.5}"#,
                r#"{"f":+1.5}"#,
                r#"{"f":+1e2}"#,
                r#"{"f":-+1}"#,
                r#"{"f32":+2}"#,
            ],
            "inputs jsoniter reads",
        )
    }

    /// Go's `partialTarget`.
    #[derive(Debug, Default, Serialize, Deserialize)]
    #[serde(default)]
    struct PartialTarget {
        #[serde(deserialize_with = "null_default")]
        id: String,
        #[serde(deserialize_with = "null_default")]
        n: i64,
        #[serde(
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "null_default"
        )]
        l: Vec<i64>,
    }

    impl MergeClone for PartialTarget {
        fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
            merge_struct(patch, |key, value| match key {
                "id" => merge_field(&mut self.id, value, "main.partialTarget", "ID"),
                "n" => merge_field(&mut self.n, value, "main.partialTarget", "N"),
                "l" => merge_field(&mut self.l, value, "main.partialTarget", "L"),
                _ => Ok(()),
            })
        }
    }

    /// A document that fails to decode gives `unmarshal`'s error and no value, where jsoniter
    /// (`jsonutil.Unmarshal`, `UnmarshalValid` and `MergeClone`) leaves in the target the members
    /// it read before the failure, a slice element that fails as its zero value, and the whole
    /// value when other bytes follow the document (`json.partial-results`). `merge_clone` into the
    /// default leaves Go's value where a member fails, but drops the failing element of a slice and
    /// leaves nothing where the document is not JSON or other bytes follow it.
    // Go: jsoniter config.go:345-358 (Unmarshal), reflect_struct_decoder.go:641-667,
    // reflect_slice.go:66-99 (sliceDecoder.doDecode); gen/decode.go partialResults
    #[test]
    fn partial_results_dropped() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/partial_results")?;
        let mut outcomes = Vec::new();
        for row in &rows {
            let what = format!("{} {}", row.name, row.input);
            ensure_eq(&row.err.is_empty(), &false, &what)?;
            let same = rows
                .iter()
                .filter(|other| other.input == row.input)
                .all(|other| other.result == row.result);
            ensure_eq(&same, &true, &what)?;
            if row.name == "jsonutil.MergeClone" {
                let mut target = PartialTarget::default();
                let failed = merge_clone(&mut target, row.input.as_bytes()).is_err();
                ensure_eq(&failed, &true, &what)?;
                let merged = String::from_utf8(marshal(&target)?)?;
                outcomes.push((row.input, row.result, merged));
            } else {
                ensure_eq(&decode::<PartialTarget>(row).is_err(), &true, &what)?;
            }
        }
        ensure_eq(&rows.len(), &12, "rows")?;
        let fresh = r#"{"id":"","n":0}"#;
        let want = [
            (
                r#"{"id":"x","n":"bad","l":[1]}"#,
                r#"{"id":"x","n":0}"#,
                r#"{"id":"x","n":0}"#,
            ),
            (
                r#"{"id":"x","l":[1,"y",3]}"#,
                r#"{"id":"x","n":0,"l":[1,0]}"#,
                r#"{"id":"x","n":0,"l":[1]}"#,
            ),
            (
                r#"{"id":"x","n":1,"l":[2"#,
                r#"{"id":"x","n":1,"l":[2]}"#,
                fresh,
            ),
            (
                r#"{"id":"x","n":1,"l":[2]} x"#,
                r#"{"id":"x","n":1,"l":[2]}"#,
                fresh,
            ),
        ];
        let want: Vec<(&str, &str, String)> = want
            .into_iter()
            .map(|(input, go, rust)| (input, go, rust.to_owned()))
            .collect();
        ensure_eq(
            &outcomes,
            &want,
            "Go's value after the error, then merge_clone's",
        )
    }

    /// `openrtb_ext.ExtBidDSATransparency`, as `gen/decode.go`'s `dsaTransparency`.
    #[derive(Debug, Clone, Default, Serialize)]
    struct DsaTransparency {
        #[serde(skip_serializing_if = "String::is_empty")]
        domain: String,
        #[serde(rename = "dsaparams", skip_serializing_if = "Vec::is_empty")]
        params: Vec<i64>,
    }

    impl MergeClone for DsaTransparency {
        fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
            merge_struct(patch, |key, value| match key {
                "domain" => merge_field(&mut self.domain, value, "main.dsaTransparency", "Domain"),
                "dsaparams" => {
                    merge_field(&mut self.params, value, "main.dsaTransparency", "Params")
                }
                _ => Ok(()),
            })
        }
    }

    /// `openrtb_ext.ExtRegsDSA`, the fields `gen/decode.go`'s `regsDSA` has.
    #[derive(Debug, Clone, Default, Serialize)]
    struct RegsDsa {
        #[serde(rename = "dsarequired", skip_serializing_if = "Option::is_none")]
        required: Option<i8>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        transparency: Vec<DsaTransparency>,
    }

    impl MergeClone for RegsDsa {
        fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
            merge_struct(patch, |key, value| match key {
                "dsarequired" => merge_field(&mut self.required, value, "main.regsDSA", "Required"),
                "transparency" => merge_field(
                    &mut self.transparency,
                    value,
                    "main.regsDSA",
                    "Transparency",
                ),
                _ => Ok(()),
            })
        }
    }

    /// The `DefaultUnpacked` the account defaults leave (Go: `hostDSA`), a shared pointer to a
    /// struct as D-COW holds one.
    fn host_dsa() -> Arc<RegsDsa> {
        Arc::new(RegsDsa {
            required: Some(1),
            transparency: vec![DsaTransparency {
                domain: "host.example".to_owned(),
                params: vec![1],
            }],
        })
    }

    /// A decode into a value that already holds a slice: Go's `jsonutil.Unmarshal` decodes each
    /// element into the element at its index, so the members an element leaves out keep the old
    /// element's, which `config.UnpackDSADefault` does with the `DefaultUnpacked` the account
    /// defaults left; `merge_clone`, the port's decode into a value that holds data, builds new
    /// elements, as Go's `MergeClone` does, and merges the rest alike
    /// (`json.decode-reuses-elements`).
    // Go: jsoniter reflect_slice.go:66-99 (sliceDecoder.doDecode), reflect2 unsafe_slice.go:150-162
    // (UnsafeGrow); config/config.go:907-912 (UnpackDSADefault); util/jsonutil/merge.go:99-109
    // (sliceCloneDecoder); gen/decode.go reusedElements
    #[test]
    fn reused_elements_rebuilt() -> TestResult {
        let doc = golden("decode.json")?;
        let mut outcomes = Vec::new();
        for row in rows(&doc, "/reused_elements")? {
            ensure_eq(&row.err, &"", row.input)?;
            let mut unpacked = Some(host_dsa());
            merge_clone(&mut unpacked, row.input.as_bytes())?;
            let merged = String::from_utf8(marshal(&unpacked.as_deref())?)?;
            outcomes.push((row.name, row.input, row.result, merged));
        }
        ensure_eq(&outcomes.len(), &6, "rows")?;
        let rebuilt = outcomes
            .iter()
            .filter(|(name, ..)| *name == "jsonutil.MergeClone")
            .all(|(_, _, go, rust)| *go == rust.as_str());
        ensure_eq(&rebuilt, &true, "merge_clone writes Go's MergeClone value")?;
        let reused: Vec<(&str, &str, &str)> = outcomes
            .iter()
            .filter(|(name, ..)| *name == "jsonutil.Unmarshal")
            .map(|(_, input, go, rust)| (*input, *go, rust.as_str()))
            .collect();
        let first = r#"{"dsarequired":3,"transparency":[{"domain":"pub.example"}]}"#;
        let second = r#"{"transparency":[{"dsaparams":[2]},{"domain":"second.example"}]}"#;
        let empty = r#"{"transparency":[]}"#;
        let first_reused = concat!(
            r#"{"dsarequired":3,"transparency":"#,
            r#"[{"domain":"pub.example","dsaparams":[1]}]}"#,
        );
        let second_reused = concat!(
            r#"{"dsarequired":1,"transparency":"#,
            r#"[{"domain":"host.example","dsaparams":[2]},{"domain":"second.example"}]}"#,
        );
        let second_rebuilt = concat!(
            r#"{"dsarequired":1,"transparency":"#,
            r#"[{"dsaparams":[2]},{"domain":"second.example"}]}"#,
        );
        let want = vec![
            (first, first_reused, first),
            (second, second_reused, second_rebuilt),
            (empty, r#"{"dsarequired":1}"#, r#"{"dsarequired":1}"#),
        ];
        ensure_eq(
            &reused,
            &want,
            "Go's Unmarshal keeps the old elements' members, merge_clone does not",
        )
    }
}
