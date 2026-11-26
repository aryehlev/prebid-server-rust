//! Differential tests: replays the goldens that `testdata/go/golden` and `testdata/go/seeded`
//! (Go programs running the real `openrtb_ext.NewBidderParamsValidator` and `gojsonschema`)
//! wrote to `testdata/golden`.
//!
//! Go walks maps in random order, so when a value has several errors Go may report them in
//! several orders; the goldens list every order observed. Rust must produce one of them, or, when
//! Go itself was not deterministic, the same errors in another order. A case may differ only on
//! an input a ledgered difference names (see `support::known_difference`), and a schema may
//! fail to load only where it uses a keyword the port does not read.

pub mod support;

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
    };

    use pbs_bidder_params::{
        BidderParamValidator as _, ParamsError,
        gojsonschema::{ResultError, Schema, ValidateError},
    };
    use pbs_devtools::repo::go_root;
    use serde_json::Value;

    use super::support::{
        Fixture, Tally, Verdict, bytes, joined, known_difference, read_lines, reordered, required,
        strings, text, undecodable,
    };

    /// The words of every refusal of a keyword, format or form the port does not read.
    const UNSUPPORTED: &str = "not supported by the Rust port of gojsonschema";

    #[test]
    fn schema_accessor_matches_go() -> Result<(), String> {
        let fixture = Fixture::load()?;
        for (name, file) in &fixture.files {
            let want = if file.is_empty() {
                String::new()
            } else {
                fs::read_to_string(format!("{}/{file}", fixture.schema_dir))
                    .map_err(|e| e.to_string())?
            };
            if fixture.validator.schema(name) != want {
                return Err(format!(
                    "Schema({name}) differs from Go (want the contents of {file:?})"
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn bidder_params_match_go() -> Result<(), String> {
        let mut fixture = Fixture::load()?;
        let mut tally = Tally::default();
        for record in read_lines("bidder_cases.jsonl")? {
            tally.add(fixture.compare(&record)?);
        }
        tally.finish("bidder", 16_000)
    }

    /// The Go `ResultError`s of a record as (type, field, context, description), sorted.
    fn go_details(record: &Value) -> Vec<[String; 4]> {
        let items = record
            .get("details")
            .and_then(Value::as_array)
            .map_or_default(Vec::as_slice);
        let mut out: Vec<[String; 4]> = items
            .iter()
            .map(|item| {
                ["type", "field", "context", "description"]
                    .map(|key| text(item, key).unwrap_or_default().to_owned())
            })
            .collect();
        out.sort();
        out
    }

    fn rust_details(errors: &[ResultError]) -> Vec<[String; 4]> {
        let mut out: Vec<[String; 4]> = errors
            .iter()
            .map(|e| {
                [
                    e.error_type().as_str().to_owned(),
                    e.field().to_owned(),
                    e.context().to_owned(),
                    e.description().to_owned(),
                ]
            })
            .collect();
        out.sort();
        out
    }

    /// Compares one engine case: a schema, and a document unless the case is about loading.
    fn engine_case(record: &Value) -> Result<Verdict, String> {
        let schema_bytes = bytes(record, "schema", "schema_hex")?.unwrap_or_default();
        let kind = required(record, "kind")?;
        let outputs = strings(record, "outputs");
        let doc = bytes(record, "doc", "doc_hex")?;
        let differs = |what: String| {
            let known = known_difference(&schema_bytes)
                .or_else(|| doc.as_deref().and_then(known_difference));
            known.map_or_else(|| Verdict::Differs(what), Verdict::Ledgered)
        };
        let name = text(record, "name").unwrap_or_default();
        let schema = match Schema::new(&schema_bytes) {
            Ok(schema) => schema,
            Err(text) if text.contains(UNSUPPORTED) => {
                return Ok(Verdict::Ledgered("jsonschema.unsupported-keywords"));
            }
            Err(_) if kind == "load_error" && undecodable(&schema_bytes) => {
                return Ok(Verdict::Ledgered("jsonschema.decode-errors"));
            }
            Err(text) => {
                let same = match kind {
                    "load_error" => outputs.contains(&text),
                    "load_panic" => text.starts_with("panic: ") || outputs.contains(&text),
                    _ => false,
                };
                return Ok(if same {
                    Verdict::Same
                } else {
                    differs(format!("{name}: Go {kind} {outputs:?}, Rust {text:?}"))
                });
            }
        };
        let Some(doc) = doc.as_deref() else {
            return Ok(if kind == "loaded" {
                Verdict::Same
            } else {
                differs(format!("{name}: Go {kind} {outputs:?}, Rust loads"))
            });
        };
        let got = schema.validate(doc);
        let same = match (kind, &got) {
            // The text is serde_json's (jsonschema.decode-errors).
            ("decode", Err(ValidateError::Load(_))) => true,
            ("ok", Ok(result)) => result.valid(),
            ("invalid", Ok(result)) if !result.valid() => {
                rust_details(result.errors()) == go_details(record)
                    && (outputs.contains(&joined(result.errors()))
                        || outputs.len() > 1
                        || outputs.iter().any(|o| reordered(o, result.errors(), doc)))
            }
            _ => false,
        };
        if same {
            return Ok(Verdict::Same);
        }
        let shown = got.map(|r| joined(r.errors()));
        Ok(differs(format!(
            "{name} on {:?}: Go {kind} {outputs:?}, Rust {shown:?}",
            String::from_utf8_lossy(doc)
        )))
    }

    #[test]
    fn engine_matches_go() -> Result<(), String> {
        let mut tally = Tally::default();
        for record in read_lines("engine_cases.jsonl")? {
            tally.add(engine_case(&record)?);
        }
        tally.finish("engine", 1_500)
    }

    /// jsonschema.decode-errors: bytes that are not JSON fail on both sides, Go with
    /// `encoding/json`'s text and the port with `serde_json`'s.
    #[test]
    fn decode_texts_are_serde_jsons() -> Result<(), String> {
        let fixture = Fixture::load()?;
        let mut pairs = BTreeSet::new();
        for record in read_lines("bidder_cases.jsonl")? {
            if required(&record, "kind")? != "decode" {
                continue;
            }
            let ext = bytes(&record, "ext", "ext_hex")?.unwrap_or_default();
            if known_difference(&ext).is_some() {
                continue;
            }
            let bidder = required(&record, "bidder")?;
            let rust = match fixture.validator.validate(bidder, &ext) {
                Err(ParamsError::Decode(text)) => text,
                other => return Err(format!("{bidder} {ext:?}: Go decode, Rust {other:?}")),
            };
            for go in strings(&record, "outputs") {
                let _new = pairs.insert((go, rust.clone()));
            }
        }
        for record in read_lines("engine_cases.jsonl")? {
            let schema = bytes(&record, "schema", "schema_hex")?.unwrap_or_default();
            if required(&record, "kind")? != "load_error" || !undecodable(&schema) {
                continue;
            }
            let rust = Schema::new(&schema).err().unwrap_or_default();
            for go in strings(&record, "outputs") {
                let _new = pairs.insert((go, rust.clone()));
            }
        }
        let expected = [
            ("EOF", "EOF while parsing a value at line 1 column 0"),
            (
                "unexpected EOF",
                "EOF while parsing an object at line 1 column 1",
            ),
        ];
        let missing: Vec<_> = expected
            .iter()
            .filter(|(go, rust)| !pairs.contains(&((*go).to_owned(), (*rust).to_owned())))
            .collect();
        if pairs.len() < 10 || !missing.is_empty() || pairs.iter().any(|(go, rust)| go == rust) {
            return Err(format!("missing {missing:?} in {pairs:#?}"));
        }
        Ok(())
    }

    /// A schema Prebid loads, by its path under the Go root, compiled once.
    fn schema_at<'c>(
        cache: &'c mut BTreeMap<String, Schema>,
        file: &str,
    ) -> Result<&'c Schema, String> {
        if !cache.contains_key(file) {
            let path = go_root().map_err(|e| e.to_string())?.join(file);
            let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let _previous = cache.insert(file.to_owned(), Schema::new(&bytes)?);
        }
        cache.get(file).ok_or_else(|| "schema cache".to_owned())
    }

    /// jsonschema.error-texts: every schema Prebid loads, on documents whose objects hold one
    /// member at most (so that Go's order of errors is fixed), gives Go's joined error text
    /// byte for byte.
    #[test]
    fn texts_exact() -> Result<(), String> {
        let mut cache = BTreeMap::new();
        let mut failures = Vec::new();
        let mut cases = 0usize;
        let mut invalid = 0usize;
        for record in read_lines("texts.jsonl")? {
            let file = required(&record, "file")?;
            let doc = required(&record, "doc")?;
            let kind = required(&record, "kind")?;
            let want = text(&record, "text").unwrap_or_default();
            let result = schema_at(&mut cache, file)?
                .validate(doc.as_bytes())
                .map_err(|e| format!("{file} on {doc}: {e}"))?;
            let got = joined(result.errors());
            cases += 1;
            invalid += usize::from(kind == "invalid");
            let same = match kind {
                "ok" => result.valid(),
                "invalid" => !result.valid() && got == want,
                _ => false,
            };
            if !same {
                failures.push(format!(
                    "{file} on {doc}:\n  Go   {kind} {want}\n  Rust {got}"
                ));
            }
        }
        if cache.len() < 283 || cases < 16_000 || invalid < 14_000 {
            return Err(format!(
                "only {} schemas, {cases} cases, {invalid} invalid",
                cache.len()
            ));
        }
        if failures.is_empty() {
            return Ok(());
        }
        let shown: Vec<&str> = failures.iter().take(30).map(String::as_str).collect();
        Err(format!(
            "{} of {cases} texts differ from Go:\n{}",
            failures.len(),
            shown.join("\n")
        ))
    }

    /// The formats gojsonschema checks that the port does not.
    const UNPORTED_FORMATS: [&str; 15] = [
        "date",
        "time",
        "date-time",
        "email",
        "idn-email",
        "ipv4",
        "ipv6",
        "uri",
        "uri-reference",
        "iri",
        "iri-reference",
        "uri-template",
        "regex",
        "json-pointer",
        "relative-json-pointer",
    ];

    /// jsonschema.formats: the two formats the bidder schemas use (`uuid` and `hostname`)
    /// check as Go's checkers do, on strings and on other values, with Go's text; a name Go has
    /// no checker for (the empty name, `UUID`, `custom`) checks nothing, as in Go; the 15 other
    /// formats Go checks fail to load.
    #[test]
    fn formats() -> Result<(), String> {
        let mut failures = Vec::new();
        let mut documents = 0usize;
        let mut refused = Vec::new();
        for record in read_lines("formats.jsonl")? {
            let schema_text = required(&record, "schema")?;
            let kind = required(&record, "kind")?;
            let format: Value = serde_json::from_str(schema_text).map_err(|e| e.to_string())?;
            let format = text(&format, "format").unwrap_or_default().to_owned();
            let ported = !UNPORTED_FORMATS.contains(&format.as_str());
            let schema = match (Schema::new(schema_text.as_bytes()), ported) {
                (Ok(schema), true) => schema,
                (Err(e), false) if kind == "loaded" && e.contains(UNSUPPORTED) => {
                    refused.push(format);
                    continue;
                }
                (got, _) => {
                    let got = got.map(|_| "loaded");
                    failures.push(format!("{schema_text}: Go {kind}, Rust {got:?}"));
                    continue;
                }
            };
            let Some(doc) = text(&record, "doc") else {
                continue;
            };
            documents += 1;
            let result = schema.validate(doc.as_bytes()).map_err(|e| e.to_string())?;
            let got = joined(result.errors());
            let want = text(&record, "text").unwrap_or_default();
            let same = match kind {
                "ok" => result.valid(),
                "invalid" => got == want,
                _ => false,
            };
            if !same {
                failures.push(format!(
                    "{schema_text} on {doc}: Go {kind} {want:?}, Rust {got:?}"
                ));
            }
        }
        if documents < 400 || refused.len() != UNPORTED_FORMATS.len() {
            return Err(format!(
                "only {documents} documents and {} refused formats: {refused:?}",
                refused.len()
            ));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }
}
