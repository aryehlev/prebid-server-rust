//! Differential tests: replays the randomized goldens that `testdata/go/adversarial` (a Go
//! program running the real `regexp`, `gojsonschema` and `openrtb_ext.NewBidderParamsValidator`)
//! wrote to `testdata/golden/adversarial_*.jsonl`.
//!
//! Go walks maps in random order, so when a value has several errors Go may report them in
//! several orders; the goldens list every order observed. Rust must produce one of them, or, when
//! Go itself was not deterministic, the same errors in another order.

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
    };

    use pbs_bidder_params::{
        BidderParamValidator as _, ParamsError, StandardBidderParamValidator,
        gojsonschema::{
            Document, GoRegexp, Node, NodeId, ResultError, Schema, ValidateError,
            decode_json_using_number,
        },
        new_bidder_params_validator,
    };

    const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden");
    const SCHEMA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../static/bidder-params");

    /// A decoded golden object (or one of its nested objects).
    struct Record<'d> {
        doc: &'d Document,
        id: NodeId,
    }

    impl<'d> Record<'d> {
        fn field(&self, key: &str) -> Option<&'d Node> {
            self.doc.get(self.id, key).map(|id| self.doc.node(id))
        }

        fn text(&self, key: &str) -> Option<&'d str> {
            if let Some(Node::String(s)) = self.field(key) {
                Some(s)
            } else {
                None
            }
        }

        fn required(&self, key: &str) -> Result<&'d str, String> {
            self.text(key)
                .ok_or_else(|| format!("golden record without {key}"))
        }

        fn flag(&self, key: &str) -> bool {
            matches!(self.field(key), Some(Node::Bool(true)))
        }

        fn strings(&self, key: &str) -> Vec<String> {
            self.children(key)
                .iter()
                .filter_map(|&id| {
                    if let Node::String(s) = self.doc.node(id) {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
                .collect()
        }

        fn children(&self, key: &str) -> Vec<NodeId> {
            match self.field(key) {
                Some(Node::Array(items)) => items.clone(),
                Some(
                    Node::Null
                    | Node::Bool(_)
                    | Node::Number(_)
                    | Node::String(_)
                    | Node::Object(_),
                )
                | None => Vec::new(),
            }
        }

        /// A byte string stored as text (`key`) or, when it is not UTF-8, as hex (`hex_key`).
        fn bytes(&self, key: &str, hex_key: &str) -> Result<Option<Vec<u8>>, String> {
            if let Some(text) = self.text(key) {
                return Ok(Some(text.as_bytes().to_vec()));
            }
            self.text(hex_key).map(decode_hex).transpose()
        }
    }

    fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
        text.as_bytes()
            .chunks(2)
            .map(|pair| {
                str::from_utf8(pair)
                    .ok()
                    .and_then(|p| u8::from_str_radix(p, 16).ok())
                    .ok_or_else(|| format!("bad hex in golden: {text}"))
            })
            .collect()
    }

    /// Decodes every line of a golden file.
    fn read_lines(name: &str) -> Result<Vec<Document>, String> {
        let text = fs::read_to_string(format!("{GOLDEN_DIR}/{name}"))
            .map_err(|e| format!("{name}: {e}"))?;
        text.lines()
            .map(|line| decode_json_using_number(line.as_bytes()))
            .collect()
    }

    fn report(what: &str, cases: usize, minimum: usize, failures: &[String]) -> Result<(), String> {
        if cases < minimum {
            return Err(format!("only {cases} {what} cases"));
        }
        if failures.is_empty() {
            return Ok(());
        }
        let shown: Vec<&str> = failures.iter().take(40).map(String::as_str).collect();
        Err(format!(
            "{} of {cases} {what} cases differ from Go:\n{}",
            failures.len(),
            shown.join("\n")
        ))
    }

    #[test]
    fn regexps_match_go() -> Result<(), String> {
        let mut failures = Vec::new();
        let mut cases = 0usize;
        for doc in read_lines("adversarial_regexp.jsonl")? {
            let record = Record {
                doc: &doc,
                id: doc.root(),
            };
            let pattern = record.required("pattern")?;
            let valid = record.flag("valid");
            cases += 1;
            let compiled = GoRegexp::compile(pattern);
            let rx = match (&compiled, valid) {
                (Ok(rx), true) => rx,
                (Err(_), false) => continue,
                (Ok(_), false) => {
                    failures.push(format!("{pattern:?}: Go rejects it, Rust compiles it"));
                    continue;
                }
                (Err(e), true) => {
                    failures.push(format!("{pattern:?}: Go compiles it, Rust rejects it: {e}"));
                    continue;
                }
            };
            let matches = record.text("matches").unwrap_or_default();
            for (input, want) in record.strings("inputs").iter().zip(matches.chars()) {
                cases += 1;
                let got = rx.match_string(input);
                if got != (want == '1') {
                    failures.push(format!(
                        "{pattern:?} on {input:?}: Go {}, Rust {got}",
                        want == '1'
                    ));
                }
            }
        }
        report("regexp", cases, 100_000, &failures)
    }

    fn joined(errors: &[ResultError]) -> String {
        errors.iter().map(ToString::to_string).collect()
    }

    fn sorted_strings(errors: &[ResultError]) -> Vec<String> {
        let mut out: Vec<String> = errors.iter().map(ToString::to_string).collect();
        out.sort();
        out
    }

    /// Compares one validated engine document; `Some` describes a mismatch.
    fn engine_doc_mismatch(schema: &Schema, record: &Record<'_>) -> Result<Option<String>, String> {
        let doc = record.bytes("doc", "doc_hex")?.unwrap_or_default();
        let kind = record.required("kind")?;
        let outputs = record.strings("outputs");
        let got = schema.validate(&doc);
        let matched = match (kind, &got) {
            ("decode", Err(ValidateError::Load(text))) => outputs.contains(text),
            ("panic", Err(ValidateError::Panic(_))) => true,
            ("ok", Ok(result)) => result.valid(),
            ("invalid", Ok(result)) if !result.valid() => {
                let mut types: Vec<String> = result
                    .errors()
                    .iter()
                    .map(|e| format!("{} {}", e.error_type().as_str(), e.context()))
                    .collect();
                types.sort();
                let text = joined(result.errors());
                types == record.strings("types")
                    && (outputs.contains(&text)
                        || (outputs.len() > 1
                            && sorted_strings(result.errors()) == record.strings("errors")))
            }
            _ => false,
        };
        if matched {
            return Ok(None);
        }
        let shown = got.map(|r| joined(r.errors()));
        Ok(Some(format!(
            "doc {:?}: Go {kind} {outputs:?}, Rust {shown:?}",
            String::from_utf8_lossy(&doc)
        )))
    }

    #[test]
    fn engine_matches_go() -> Result<(), String> {
        let mut failures = Vec::new();
        let mut cases = 0usize;
        for line in read_lines("adversarial_engine.jsonl")? {
            let record = Record {
                doc: &line,
                id: line.root(),
            };
            let schema_bytes = record.bytes("schema", "schema_hex")?.unwrap_or_default();
            let schema_text = String::from_utf8_lossy(&schema_bytes).into_owned();
            let kind = record.required("kind")?;
            let outputs = record.strings("outputs");
            cases += 1;
            let schema = match (Schema::new(&schema_bytes), kind) {
                (Ok(schema), "loaded") => schema,
                (Err(text), "load_error") if outputs.contains(&text) => continue,
                // Map order decides whether Go reaches the panic or an error first.
                (Err(text), "load_panic")
                    if text.starts_with("panic: ") || outputs.contains(&text) =>
                {
                    continue;
                }
                (got, _) => {
                    let got = got.map(|_| "loaded");
                    failures.push(format!(
                        "schema {schema_text}: Go {kind} {outputs:?}, Rust {got:?}"
                    ));
                    continue;
                }
            };
            for id in record.children("docs") {
                cases += 1;
                let doc_record = Record { doc: &line, id };
                if let Some(mismatch) = engine_doc_mismatch(&schema, &doc_record)? {
                    failures.push(format!("schema {schema_text}: {mismatch}"));
                }
            }
        }
        report("engine", cases, 10_000, &failures)
    }

    /// The validator built like Go builds it (bidders and aliases from validator.json), and the
    /// schema file of every bidder.
    fn load_validator() -> Result<(StandardBidderParamValidator, BTreeMap<String, String>), String>
    {
        let text = fs::read_to_string(format!("{GOLDEN_DIR}/validator.json"))
            .map_err(|e| e.to_string())?;
        let golden = decode_json_using_number(text.as_bytes())?;
        let record = Record {
            doc: &golden,
            id: golden.root(),
        };
        let bidders: BTreeSet<String> = record.strings("bidders").into_iter().collect();
        let mut aliases = BTreeMap::new();
        let mut files = BTreeMap::new();
        for (key, target) in [("aliases", &mut aliases), ("schemas", &mut files)] {
            if let Some(Node::Object(entries)) = record.field(key) {
                for (name, value) in entries {
                    if let Node::String(value) = golden.node(*value) {
                        let _previous = target.insert(name.clone(), value.clone());
                    }
                }
            }
        }
        let validator = new_bidder_params_validator(SCHEMA_DIR, &bidders, &aliases)
            .map_err(|e| e.to_string())?;
        Ok((validator, files))
    }

    /// The bidder's schema, for comparing errors as a multiset.
    fn schema_of<'c>(
        cache: &'c mut BTreeMap<String, Schema>,
        file: &str,
    ) -> Result<&'c Schema, String> {
        if !cache.contains_key(file) {
            let bytes = fs::read(format!("{SCHEMA_DIR}/{file}")).map_err(|e| e.to_string())?;
            let _previous = cache.insert(file.to_owned(), Schema::new(&bytes)?);
        }
        cache.get(file).ok_or_else(|| "schema cache".to_owned())
    }

    #[test]
    fn bidder_params_match_go() -> Result<(), String> {
        let (validator, files) = load_validator()?;
        let mut cache = BTreeMap::new();
        let mut failures = Vec::new();
        let mut cases = 0usize;
        for line in read_lines("adversarial_bidders.jsonl")? {
            let record = Record {
                doc: &line,
                id: line.root(),
            };
            let bidder = record.required("bidder")?;
            let ext = record.bytes("ext", "ext_hex")?.unwrap_or_default();
            let kind = record.required("kind")?;
            let outputs = record.strings("outputs");
            cases += 1;
            let got = validator.validate(bidder, &ext);
            let matched = match (kind, &got) {
                ("ok", Ok(())) | ("panic", Err(ParamsError::Panic(_))) => true,
                ("decode", Err(ParamsError::Decode(text))) => outputs.contains(text),
                ("invalid", Err(ParamsError::Invalid(text))) if outputs.contains(text) => true,
                ("invalid", Err(ParamsError::Invalid(_))) if outputs.len() > 1 => {
                    // Go reported these errors in several orders: compare them as a multiset.
                    let file = files.get(bidder).cloned().unwrap_or_default();
                    let result = schema_of(&mut cache, &file)?
                        .validate(&ext)
                        .map_err(|e| e.to_string())?;
                    sorted_strings(result.errors()) == record.strings("errors")
                }
                _ => false,
            };
            if !matched {
                failures.push(format!(
                    "{bidder} {:?}: Go {kind} {outputs:?}, Rust {got:?}",
                    String::from_utf8_lossy(&ext)
                ));
            }
        }
        report("bidder", cases, 8_000, &failures)
    }
}
