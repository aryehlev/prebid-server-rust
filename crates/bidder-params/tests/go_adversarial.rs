//! Differential tests: replays the randomized goldens that `testdata/go/adversarial` (a Go
//! program running the real `regexp`, `gojsonschema` and `openrtb_ext.NewBidderParamsValidator`)
//! wrote to `testdata/golden/adversarial_*.jsonl`, and the numbers of `huge_numbers.jsonl`.
//!
//! Go walks maps in random order, so when a value has several errors Go may report them in
//! several orders; the goldens list every order observed. Rust must produce one of them, or, when
//! Go itself was not deterministic, the same errors in another order. A case may differ only on
//! an input a ledgered difference names (see `support::known_difference`), and a schema may
//! fail to load only where it uses a keyword the port does not read.

pub mod support;

#[cfg(test)]
mod tests {
    use pbs_bidder_params::gojsonschema::{Schema, ValidateError};
    use pbs_util::goregex::{self, CompileError};
    use regex::Regex;
    use serde_json::Value;

    use super::support::{
        Fixture, Tally, Verdict, bytes, joined, known_difference, read_lines, reordered, required,
        sorted_strings, strings, text, undecodable,
    };

    /// The words of every refusal of a keyword, format or form the port does not read.
    const UNSUPPORTED: &str = "not supported by the Rust port of gojsonschema";

    /// Whether `text` holds a character Unicode 16.0 assigned and Go 1.26's tables (Unicode
    /// 15.0) do not have: regex.unicode-version.
    fn unicode_16(added: &Regex, text: &str) -> bool {
        added.is_match(text)
    }

    /// Go's `regexp` and the patterns of the schemas: `pbs_util::goregex` refuses what Go
    /// refuses and matches as Go matches, but on the characters Unicode 16.0 added
    /// (regex.unicode-version). A pattern Go compiles that goregex does not translate
    /// (regex.syntax-edges) fails its schema loudly (jsonschema.unsupported-keywords).
    #[test]
    fn regexps_match_go() -> Result<(), String> {
        let added = Regex::new(r"[\p{age=16.0}--\p{age=15.0}]").map_err(|e| e.to_string())?;
        let mut failures = Vec::new();
        let (mut patterns, mut cases, mut unicode) = (0usize, 0usize, 0usize);
        let (mut untranslated, mut misread) = (0usize, 0usize);
        for record in read_lines("adversarial_regexp.jsonl")? {
            let pattern = required(&record, "pattern")?;
            let valid = record.get("valid").and_then(Value::as_bool) == Some(true);
            patterns += 1;
            cases += 1;
            let rx = match (goregex::compile(pattern), valid) {
                (Ok(rx), true) => rx,
                (Err(_), false) => continue,
                (Err(CompileError::Unsupported { .. } | CompileError::Crate(_)), true) => {
                    untranslated += 1;
                    continue;
                }
                // goregex reads a repetition after an empty `\Q\E` (`a{2}\Q\E{2}`) as a nested
                // repetition, where Go repeats the repetition (`(a{2}){2}`): reported to pbs-util.
                // The schema fails to load all the same.
                (
                    Err(CompileError::Syntax {
                        code: "invalid nested repetition operator",
                        ..
                    }),
                    true,
                ) if pattern.contains(r"\Q\E") => {
                    misread += 1;
                    continue;
                }
                (Err(e), true) => {
                    failures.push(format!("{pattern:?}: Go compiles it, Rust rejects it: {e}"));
                    continue;
                }
                (Ok(_), false) => {
                    failures.push(format!("{pattern:?}: Go rejects it, Rust compiles it"));
                    continue;
                }
            };
            let matches = text(&record, "matches").unwrap_or_default();
            for (input, want) in strings(&record, "inputs").iter().zip(matches.chars()) {
                cases += 1;
                let got = rx.is_match(input);
                if got == (want == '1') {
                    continue;
                }
                if unicode_16(&added, input) || unicode_16(&added, pattern) {
                    unicode += 1;
                } else {
                    failures.push(format!(
                        "{pattern:?} on {input:?}: Go {}, Rust {got}",
                        want == '1'
                    ));
                }
            }
        }
        if cases < 100_000 || untranslated > 1_000 || misread > 300 || unicode > cases / 1000 {
            return Err(format!(
                "{cases} regexp cases over {patterns} patterns: {untranslated} not translated, \
                 {misread} read as nested repetitions, {unicode} differ on Unicode 16.0 characters"
            ));
        }
        if failures.is_empty() {
            return Ok(());
        }
        let shown: Vec<&str> = failures.iter().take(40).map(String::as_str).collect();
        Err(format!(
            "{} of {cases} regexp cases differ from Go:\n{}",
            failures.len(),
            shown.join("\n")
        ))
    }

    /// Compares one document of a loaded engine schema.
    fn engine_doc(schema: &Schema, schema_bytes: &[u8], record: &Value) -> Result<Verdict, String> {
        let doc = bytes(record, "doc", "doc_hex")?.unwrap_or_default();
        let kind = required(record, "kind")?;
        let outputs = strings(record, "outputs");
        let got = schema.validate(&doc);
        let same = match (kind, &got) {
            // The text is serde_json's (jsonschema.decode-errors).
            ("decode", Err(ValidateError::Load(_))) => true,
            ("ok", Ok(result)) => result.valid(),
            ("invalid", Ok(result)) if !result.valid() => {
                let mut types: Vec<String> = result
                    .errors()
                    .iter()
                    .map(|e| format!("{} {}", e.error_type().as_str(), e.context()))
                    .collect();
                types.sort();
                types == strings(record, "types")
                    && (outputs.contains(&joined(result.errors()))
                        || (outputs.len() > 1
                            && sorted_strings(result.errors()) == strings(record, "errors"))
                        || outputs.iter().any(|o| reordered(o, result.errors(), &doc)))
            }
            _ => false,
        };
        if same {
            return Ok(Verdict::Same);
        }
        if let Some(entry) = known_difference(&doc).or_else(|| known_difference(schema_bytes)) {
            return Ok(Verdict::Ledgered(entry));
        }
        let shown = got.map(|r| joined(r.errors()));
        Ok(Verdict::Differs(format!(
            "schema {} on {:?}: Go {kind} {outputs:?}, Rust {shown:?}",
            String::from_utf8_lossy(schema_bytes),
            String::from_utf8_lossy(&doc)
        )))
    }

    #[test]
    fn engine_matches_go() -> Result<(), String> {
        let mut tally = Tally::default();
        for record in read_lines("adversarial_engine.jsonl")? {
            let schema_bytes = bytes(&record, "schema", "schema_hex")?.unwrap_or_default();
            let kind = required(&record, "kind")?;
            let outputs = strings(&record, "outputs");
            let docs = record
                .get("docs")
                .and_then(Value::as_array)
                .map_or_default(Vec::as_slice);
            let schema = match (Schema::new(&schema_bytes), kind) {
                (Ok(schema), "loaded") => schema,
                (Err(text), _) if text.contains(UNSUPPORTED) => {
                    tally.add(Verdict::Ledgered("jsonschema.unsupported-keywords"));
                    continue;
                }
                (Err(text), "load_error") if outputs.contains(&text) => {
                    tally.add(Verdict::Same);
                    continue;
                }
                (Err(_), "load_error") if undecodable(&schema_bytes) => {
                    tally.add(Verdict::Ledgered("jsonschema.decode-errors"));
                    continue;
                }
                // Map order decides whether Go reaches the panic or an error first.
                (Err(text), "load_panic")
                    if text.starts_with("panic: ") || outputs.contains(&text) =>
                {
                    tally.add(Verdict::Same);
                    continue;
                }
                (got, _) => {
                    let got = got.map(|_| "loaded");
                    tally.add(known_difference(&schema_bytes).map_or_else(
                        || {
                            Verdict::Differs(format!(
                                "schema {}: Go {kind} {outputs:?}, Rust {got:?}",
                                String::from_utf8_lossy(&schema_bytes)
                            ))
                        },
                        Verdict::Ledgered,
                    ));
                    continue;
                }
            };
            tally.add(Verdict::Same);
            for doc in docs {
                tally.add(engine_doc(&schema, &schema_bytes, doc)?);
            }
        }
        tally.finish("engine", 5_000)
    }

    #[test]
    fn bidder_params_match_go() -> Result<(), String> {
        let mut fixture = Fixture::load()?;
        let mut tally = Tally::default();
        for record in read_lines("adversarial_bidders.jsonl")? {
            tally.add(fixture.compare(&record)?);
        }
        tally.finish("bidder", 8_000)
    }

    /// The outcome of one huge-numbers case, as the golden spells it: the kind, and the text of
    /// an error.
    fn outcome(schema: &str, doc: Option<&str>) -> (&'static str, String) {
        let schema = match Schema::new(schema.as_bytes()) {
            Ok(schema) => schema,
            Err(e) => return ("load_error", e),
        };
        let Some(doc) = doc else {
            return ("loaded", String::new());
        };
        match schema.validate(doc.as_bytes()) {
            Ok(result) if result.valid() => ("ok", String::new()),
            Ok(result) => ("invalid", joined(result.errors())),
            Err(e) => ("decode", e.to_string()),
        }
    }

    /// jsonschema.number-precision: numbers `f64` holds exactly (every integer up to 2^53 in
    /// size, `u64` and `i64` integers against integer limits, decimals of 15 significant digits)
    /// validate as Go's `big.Rat` does; the others may not, as these cases show.
    #[test]
    fn huge_numbers() -> Result<(), String> {
        let mut same = 0usize;
        let mut differ = Vec::new();
        let mut failures = Vec::new();
        for record in read_lines("huge_numbers.jsonl")? {
            let schema = required(&record, "schema")?;
            let doc = text(&record, "doc");
            let kind = required(&record, "kind")?;
            let want = text(&record, "text").unwrap_or_default();
            let (got_kind, got) = outcome(schema, doc);
            let case = format!("{schema} on {doc:?}: Go {kind} {want:?}, Rust {got_kind} {got:?}");
            if got_kind == kind && (got == want || got_kind == "decode") {
                same += 1;
                continue;
            }
            let imprecise = known_difference(schema.as_bytes())
                .or_else(|| doc.and_then(|d| known_difference(d.as_bytes())));
            if imprecise == Some("jsonschema.number-precision") {
                differ.push(case);
            } else {
                failures.push(case);
            }
        }
        // The differences the ledger entry describes: the schema, the document, and Go's and
        // the port's outcomes, as a case spells them.
        let expected: [(&str, Option<&str>, &str, &str); 6] = [
            // 1 + 1e-16 is 1 as an f64, an integer.
            (
                r#"{"type":"integer"}"#,
                Some("1.0000000000000001"),
                r#"invalid "(root): Invalid type. Expected: integer, given: number""#,
                r#"ok """#,
            ),
            // Go reads 1e400 exactly; serde_json refuses it, in a schema too.
            (
                r#"{"type":"integer"}"#,
                Some("1e400"),
                r#"ok """#,
                r#"decode "number out of range at line 1 column 5""#,
            ),
            (
                r#"{"minimum":1e400}"#,
                None,
                r#"loaded """#,
                r#"load_error "number out of range at line 1 column 16""#,
            ),
            // 1e-400 is 0 as an f64, an integer.
            (
                r#"{"type":"string"}"#,
                Some("1e-400"),
                r#"invalid "(root): Invalid type. Expected: string, given: number""#,
                r#"invalid "(root): Invalid type. Expected: string, given: integer""#,
            ),
            // Go panics on a number whose exponent is beyond big.Rat's limit; serde_json reads
            // it as 0.
            (
                r#"{"type":"number","minimum":1}"#,
                Some("0.1e-1000005"),
                r#"panic "runtime error: invalid memory address or nil pointer dereference""#,
                r#"invalid "(root): Must be greater than or equal to 1""#,
            ),
            // A limit of 17 significant digits prints as the f64's shortest digits.
            (
                r#"{"minimum":0.30000000000000001}"#,
                Some("0"),
                r#"invalid "(root): Must be greater than or equal to 0.30000000000000001""#,
                r#"invalid "(root): Must be greater than or equal to 0.3""#,
            ),
        ];
        for (schema, doc, go, rust) in expected {
            let case = format!("{schema} on {doc:?}: Go {go}, Rust {rust}");
            if !differ.contains(&case) {
                failures.push(format!("expected the difference {case}"));
            }
        }
        // Integers beyond 2^53 that serde_json holds as u64 or i64 compare exactly.
        let exact = r#"{"type":"number","maximum":9007199254740992} on Some("9007199254740993")"#;
        if differ.iter().any(|d| d.starts_with(exact)) {
            failures.push(format!("{exact} differs from Go"));
        }
        if same < 200 || differ.len() < 40 {
            failures.push(format!("{same} cases as Go, {} differ", differ.len()));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "{}\n\nledgered differences:\n{}",
                failures.join("\n"),
                differ.join("\n")
            ))
        }
    }
}
