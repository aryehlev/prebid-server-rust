//! Differential tests: replays the goldens that `testdata/go/golden` (a Go program running the
//! real `openrtb_ext.NewBidderParamsValidator` and `gojsonschema`) wrote to `testdata/golden`.
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
            Document, Node, ResultError, Schema, ValidateError, decode_json_using_number,
        },
        new_bidder_params_validator,
    };
    use pbs_devtools::repo::go_root;

    const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden");

    /// `static/bidder-params` of the Go root, the upstream checkout the goldens were made from.
    fn schema_dir() -> Result<String, String> {
        let dir = go_root()
            .map_err(|e| e.to_string())?
            .join("static")
            .join("bidder-params");
        dir.to_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("{} is not UTF-8", dir.display()))
    }

    /// One decoded golden record.
    struct Record {
        doc: Document,
    }

    impl Record {
        fn parse(text: &str) -> Result<Self, String> {
            let doc = decode_json_using_number(text.as_bytes())?;
            Ok(Self { doc })
        }

        fn field(&self, key: &str) -> Option<&Node> {
            self.doc
                .get(self.doc.root(), key)
                .map(|id| self.doc.node(id))
        }

        fn text(&self, key: &str) -> Option<&str> {
            if let Some(Node::String(s)) = self.field(key) {
                Some(s)
            } else {
                None
            }
        }

        fn required(&self, key: &str) -> Result<&str, String> {
            self.text(key)
                .ok_or_else(|| format!("golden record without {key}"))
        }

        fn strings(&self, key: &str) -> Vec<String> {
            let Some(Node::Array(items)) = self.field(key) else {
                return Vec::new();
            };
            items
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

        /// A byte string stored as text (`key`) or, when it is not UTF-8, as hex (`hex_key`).
        fn bytes(&self, key: &str, hex_key: &str) -> Result<Option<Vec<u8>>, String> {
            if let Some(text) = self.text(key) {
                return Ok(Some(text.as_bytes().to_vec()));
            }
            self.text(hex_key).map(decode_hex).transpose()
        }

        /// `details`: the Go `ResultError`s as (type, field, context, description).
        fn details(&self) -> Vec<[String; 4]> {
            let Some(Node::Array(items)) = self.field("details") else {
                return Vec::new();
            };
            let mut out: Vec<[String; 4]> = items
                .iter()
                .map(|&id| {
                    ["type", "field", "context", "description"].map(|key| {
                        match self.doc.get(id, key).map(|v| self.doc.node(v)) {
                            Some(Node::String(s)) => s.clone(),
                            Some(
                                Node::Null
                                | Node::Bool(_)
                                | Node::Number(_)
                                | Node::Array(_)
                                | Node::Object(_),
                            )
                            | None => String::new(),
                        }
                    })
                })
                .collect();
            out.sort();
            out
        }
    }

    fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
        let digits = text.as_bytes();
        digits
            .chunks(2)
            .map(|pair| {
                str::from_utf8(pair)
                    .ok()
                    .and_then(|p| u8::from_str_radix(p, 16).ok())
                    .ok_or_else(|| format!("bad hex in golden: {text}"))
            })
            .collect()
    }

    fn read_golden(name: &str) -> Result<String, String> {
        fs::read_to_string(format!("{GOLDEN_DIR}/{name}")).map_err(|e| format!("{name}: {e}"))
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

    /// The validator built like Go builds it, plus the golden that describes it.
    struct Fixture {
        validator: StandardBidderParamValidator,
        schemas: BTreeMap<String, String>,
        /// The schema directory the validator was loaded from.
        schema_dir: String,
    }

    fn load_fixture() -> Result<Fixture, String> {
        let golden = Record::parse(&read_golden("validator.json")?)?;
        let bidders: BTreeSet<String> = golden.strings("bidders").into_iter().collect();
        let mut aliases = BTreeMap::new();
        let mut schemas = BTreeMap::new();
        for (key, target) in [("aliases", &mut aliases), ("schemas", &mut schemas)] {
            let Some(Node::Object(entries)) = golden.field(key) else {
                return Err(format!("validator.json without {key}"));
            };
            for (name, value) in entries {
                if let Node::String(text) = golden.doc.node(*value) {
                    let _previous = target.insert(name.clone(), text.clone());
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
        let schema_dir = schema_dir()?;
        let validator = new_bidder_params_validator(&schema_dir, &bidders, &aliases)
            .map_err(|e| e.to_string())?;
        Ok(Fixture {
            validator,
            schemas,
            schema_dir,
        })
    }

    #[test]
    fn schema_accessor_matches_go() -> Result<(), String> {
        let fixture = load_fixture()?;
        for (name, file) in &fixture.schemas {
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

    /// Checks one bidder case; `Ok(false)` is a mismatch.
    fn bidder_case_matches(
        fixture: &Fixture,
        parsed: &mut BTreeMap<String, Schema>,
        record: &Record,
    ) -> Result<bool, String> {
        let bidder = record.required("bidder")?;
        let ext = record.bytes("ext", "ext_hex")?.unwrap_or_default();
        let kind = record.required("kind")?;
        let outputs = record.strings("outputs");
        let text = match (kind, fixture.validator.validate(bidder, &ext)) {
            ("ok", Ok(())) | ("panic", Err(ParamsError::Panic(_))) => return Ok(true),
            ("decode", Err(ParamsError::Decode(text))) => return Ok(outputs.contains(&text)),
            ("invalid", Err(ParamsError::Invalid(text))) => text,
            (
                _,
                Ok(())
                | Err(ParamsError::Panic(_) | ParamsError::Decode(_) | ParamsError::Invalid(_)),
            ) => {
                return Ok(false);
            }
        };
        if outputs.contains(&text) {
            return Ok(true);
        }
        if outputs.len() < 2 {
            return Ok(false);
        }
        // Go reported these errors in several orders: compare them as a multiset.
        let file = fixture.schemas.get(bidder).cloned().unwrap_or_default();
        if !parsed.contains_key(&file) {
            let bytes =
                fs::read(format!("{}/{file}", fixture.schema_dir)).map_err(|e| e.to_string())?;
            let _previous = parsed.insert(file.clone(), Schema::new(&bytes)?);
        }
        let schema = parsed.get(&file).ok_or("schema cache")?;
        let result = schema.validate(&ext).map_err(|e| e.to_string())?;
        let mut got: Vec<String> = result.errors().iter().map(ToString::to_string).collect();
        got.sort();
        Ok(got == record.strings("errors"))
    }

    #[test]
    fn bidder_params_match_go() -> Result<(), String> {
        let fixture = load_fixture()?;
        let mut parsed = BTreeMap::new();
        let mut failures = Vec::new();
        let mut cases = 0usize;
        for line in read_golden("bidder_cases.jsonl")?.lines() {
            let record = Record::parse(line)?;
            cases += 1;
            if !bidder_case_matches(&fixture, &mut parsed, &record)? {
                let got = record.bytes("ext", "ext_hex")?.unwrap_or_default();
                let bidder = record.required("bidder")?;
                failures.push(format!(
                    "{bidder} {:?} (from {}): got {:?}, Go {}",
                    String::from_utf8_lossy(&got),
                    record.text("src").unwrap_or_default(),
                    fixture.validator.validate(bidder, &got),
                    line
                ));
            }
        }
        if cases < 16_000 {
            return Err(format!("only {cases} bidder cases"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            let shown: Vec<&String> = failures.iter().take(25).collect();
            Err(format!(
                "{} of {cases} bidder cases differ from Go:\n{shown:#?}",
                failures.len()
            ))
        }
    }

    /// Checks one engine case; `Ok(false)` is a mismatch.
    fn engine_case_matches(record: &Record) -> Result<bool, String> {
        let schema_bytes = record.bytes("schema", "schema_hex")?.unwrap_or_default();
        let kind = record.required("kind")?;
        let outputs = record.strings("outputs");
        let schema = match Schema::new(&schema_bytes) {
            Ok(schema) => schema,
            Err(text) => {
                return Ok(match kind {
                    "load_error" => outputs.contains(&text),
                    "load_panic" => text.starts_with("panic: "),
                    _ => false,
                });
            }
        };
        let Some(doc) = record.bytes("doc", "doc_hex")? else {
            return Ok(kind == "loaded");
        };
        let result = match schema.validate(&doc) {
            Ok(result) => result,
            Err(ValidateError::Load(text)) => {
                return Ok(kind == "decode" && outputs.contains(&text));
            }
            Err(ValidateError::Panic(_)) => return Ok(kind == "panic"),
        };
        if result.valid() {
            return Ok(kind == "ok");
        }
        let joined: String = result.errors().iter().map(ToString::to_string).collect();
        let details_match = rust_details(result.errors()) == record.details();
        Ok(kind == "invalid" && details_match && (outputs.contains(&joined) || outputs.len() > 1))
    }

    #[test]
    fn engine_matches_go() -> Result<(), String> {
        let mut failures = Vec::new();
        let mut cases = 0usize;
        for line in read_golden("engine_cases.jsonl")?.lines() {
            let record = Record::parse(line)?;
            cases += 1;
            if !engine_case_matches(&record)? {
                let schema = record.bytes("schema", "schema_hex")?.unwrap_or_default();
                let doc = record.bytes("doc", "doc_hex")?;
                let got = Schema::new(&schema).map(|s| {
                    doc.map(|d| {
                        s.validate(&d).map(|r| {
                            r.errors()
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                        })
                    })
                });
                let name = record.text("name").unwrap_or_default();
                let line_start: String = line.chars().take(600).collect();
                failures.push(format!("{name}: got {got:?}\n    Go {line_start}"));
            }
        }
        if cases < 1_500 {
            return Err(format!("only {cases} engine cases"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            let shown: Vec<&String> = failures.iter().take(40).collect();
            Err(format!(
                "{} of {cases} engine cases differ from Go:\n{shown:#?}",
                failures.len()
            ))
        }
    }
}
