//! The YAML reader against yaml.v3 v3.0.1 as Viper's YAML codec runs it
//! (`yaml.Unmarshal(b, &map[string]interface{})`): scalar resolution, whole documents and their
//! errors (`testdata/golden/scalars.json`), and every bidder-info file of the pinned upstream
//! (`testdata/golden/bidder_info.json`).

pub mod support;

#[cfg(test)]
mod tests {
    use std::fs;

    use pbs_config::loader::{GoValue, read_yaml};
    use pbs_devtools::repo;
    use serde_json::Value as Json;

    use super::support::{TestResult, at, check_eq, diff, golden, list, report, text};

    /// The reader's tree in the compact form, or its error.
    fn read(bytes: &[u8]) -> Result<Json, String> {
        read_yaml(bytes).map(|tree| GoValue::Map(tree).compact())
    }

    /// A golden result's error, empty when it has none.
    fn error_of(result: &Json) -> &str {
        result.get("error").and_then(Json::as_str).unwrap_or("")
    }

    /// The document's bytes: `bytes` when the golden records them (the text is not UTF-8).
    fn document(case: &Json) -> TestResult<Vec<u8>> {
        match case.get("bytes").and_then(Json::as_array) {
            Some(bytes) => bytes
                .iter()
                .map(|b| {
                    b.as_u64()
                        .and_then(|b| u8::try_from(b).ok())
                        .ok_or_else(|| format!("{b} is not a byte"))
                })
                .collect(),
            None => Ok(text(case, "/text")?.as_bytes().to_vec()),
        }
    }

    /// The documents the reader gives another tree than yaml.v3 does, with the tree it gives
    /// (ledger: `yaml.scalars`): under a `%TAG` directive, yaml.v3 keeps the text of a scalar
    /// with a custom tag (`!e!x 1` is the string `1`), serde-saphyr resolves it untagged.
    const LEDGERED_DOCS: &[(&str, &str)] = &[("tag directive", r#"{"a": 1}"#)];

    /// The JSON text `text`.
    fn json(text: &str) -> TestResult<Json> {
        serde_json::from_str(text).map_err(|e| format!("{text}: {e}"))
    }

    /// The documents whose error texts the pin shows, in the golden's order, as yaml.v3 and the
    /// reader word them: the reader's own words for all but a `%YAML` directive of a version
    /// yaml.v3 refuses, whose text the reader writes as yaml.v3 does.
    const ERROR_TEXTS: &[(&str, &str, &str)] = &[
        (
            "scalar document",
            "yaml: unmarshal errors:\n  line 1: cannot unmarshal !!int `5` into \
             map[string]interface {}",
            "invalid type: integer `5`, expected a mapping",
        ),
        (
            "tab indent",
            "yaml: line 2: found character that cannot start any token",
            "tabs disallowed within this context (block indentation) at line 2, column 2",
        ),
        (
            "duplicate key",
            "yaml: unmarshal errors:\n  line 2: mapping key \"a\" already defined at line 1",
            "duplicate mapping key: a, set DuplicateKeyPolicy in Options if acceptable at line \
             2, column 1",
        ),
        (
            "yaml 2.0 directive",
            "yaml: found incompatible YAML document",
            "unsupported YAML major version at line 1, column 1",
        ),
        (
            "directive",
            "yaml: found incompatible YAML document",
            "yaml: found incompatible YAML document",
        ),
        (
            "undefined alias",
            "yaml: unknown anchor 'nope' referenced",
            "alias references unknown anchor at line 1, column 4",
        ),
    ];

    /// Every document yaml.v3 reads into `map[string]interface{}` reads into the same tree, but
    /// for the ledgered ones, and every one it rejects is rejected; the error texts are
    /// serde-saphyr's (ledger: `yaml.error-texts`).
    // Go: gopkg.in/yaml.v3 v3.0.1 yaml.go:88 (Unmarshal), as viper's internal/encoding/yaml codec
    // calls it
    #[test]
    fn errors_same_verdict() -> TestResult {
        let doc = golden("scalars.json")?;
        let mut failures = Vec::new();
        let mut texts = Vec::new();
        for case in list(&doc, "/yaml_docs")? {
            let name = text(case, "/name")?;
            let go = at(case, "/map")?;
            let go_error = error_of(go);
            let got = read(&document(case)?);
            if let Some((_, mine)) = LEDGERED_DOCS.iter().find(|(ledgered, _)| *ledgered == name) {
                let mine = json(mine)?;
                if got.as_ref() != Ok(&mine) || go.get("value") == Some(&mine) {
                    failures.push(format!(
                        "{name}: got {got:?}, the ledger says {mine}, Go gives {go}"
                    ));
                }
                continue;
            }
            match got {
                Ok(tree) if go_error.is_empty() => {
                    for d in diff(&tree, at(go, "/value")?) {
                        failures.push(format!("{name}: {d}"));
                    }
                }
                Ok(tree) => failures.push(format!("{name}: read {tree}, Go fails: {go_error}")),
                Err(e) if go_error.is_empty() => failures.push(format!(
                    "{name}: fails ({e}), Go reads {}",
                    at(go, "/value")?
                )),
                Err(e) => {
                    if ERROR_TEXTS.iter().any(|(shown, ..)| *shown == name) {
                        texts.push((name.to_owned(), go_error.to_owned(), e));
                    }
                }
            }
        }
        report("yaml_docs", &failures)?;
        check_eq(
            &texts,
            &ERROR_TEXTS
                .iter()
                .map(|(name, go, rust)| ((*name).to_owned(), (*go).to_owned(), (*rust).to_owned()))
                .collect::<Vec<_>>(),
            "error texts: document, yaml.v3, the reader",
        )
    }

    /// The plain scalars serde-saphyr resolves otherwise than yaml.v3, with the value the reader
    /// gives each (ledger: `yaml.scalars`). yaml.v3 parses integers with `strconv.ParseInt` base
    /// 0, which takes `0X`, `0O` and `0B` and underscores anywhere after the first digit
    /// (`1_` is 1), and floats after removing underscores; it keeps a float out of range a
    /// string, makes YAML 1.1 timestamps `time.Time`, and fails on `!!null` with a value.
    const LEDGERED_SCALARS: &[(&str, &str)] = &[
        ("0O17", r#""0O17""#),
        ("0X1F", r#""0X1F""#),
        ("0B101", r#""0B101""#),
        ("1_", r#""1_""#),
        ("1__0", r#""1__0""#),
        ("1_000.5", r#""1_000.5""#),
        ("685.230_15e+03", r#""685.230_15e+03""#),
        ("1e400", r#"{"$float64": "+Inf"}"#),
        ("2001-12-14", r#""2001-12-14""#),
        (
            "2001-12-14t21:59:43.10-05:00",
            r#""2001-12-14t21:59:43.10-05:00""#,
        ),
        ("!!null x", "null"),
    ];

    /// Every scalar resolves to the Go type and value yaml.v3's untyped resolution gives it, but
    /// for the ledgered ones, which resolve as the ledger says and not as in Go.
    // Go: gopkg.in/yaml.v3 v3.0.1 resolve.go:126-265 (resolve; :150 refuses a tagged value it
    // cannot convert)
    #[test]
    fn scalars_resolve_like_yaml_v3() -> TestResult {
        let doc = golden("scalars.json")?;
        let mut failures = Vec::new();
        let mut ledgered = 0_usize;
        for case in list(&doc, "/yaml_scalars")? {
            let scalar = text(case, "/text")?;
            let go_error = error_of(case);
            let got = read(format!("v: {scalar}\n").as_bytes())
                .map(|tree| tree.get("v").cloned().unwrap_or(Json::Null));
            if let Some((_, mine)) = LEDGERED_SCALARS.iter().find(|(text, _)| *text == scalar) {
                ledgered += 1;
                let mine = json(mine)?;
                if got.as_ref() != Ok(&mine)
                    || (go_error.is_empty() && case.get("value") == Some(&mine))
                {
                    failures.push(format!(
                        "{scalar:?}: got {got:?}, the ledger says {mine}, Go gives {}",
                        case.get("value").unwrap_or(&Json::Null)
                    ));
                }
                continue;
            }
            match got {
                Ok(value) if go_error.is_empty() => {
                    if &value != at(case, "/value")? {
                        failures.push(format!(
                            "{scalar:?}: got {value}, want {}",
                            at(case, "/value")?
                        ));
                    }
                }
                Ok(value) => {
                    failures.push(format!("{scalar:?}: got {value}, Go fails: {go_error}"));
                }
                Err(e) if go_error.is_empty() => failures.push(format!(
                    "{scalar:?}: fails ({e}), Go gives {}",
                    at(case, "/value")?
                )),
                Err(_) => {}
            }
        }
        check_eq(
            &ledgered,
            &LEDGERED_SCALARS.len(),
            "the ledgered scalars found",
        )?;
        report("yaml_scalars", &failures)
    }

    /// Every `static/bidder-info` file of the pinned upstream reads into the tree yaml.v3 gives.
    // Go: config/bidderinfo.go:323 (yaml.Unmarshal of each file)
    #[test]
    fn bidder_info_files_read_like_yaml_v3() -> TestResult {
        let doc = golden("bidder_info.json")?;
        let dir = repo::go_root()
            .map_err(|e| e.to_string())?
            .join("static")
            .join("bidder-info");
        let files = list(&doc, "/files")?;
        let mut failures = Vec::new();
        for file in files {
            let name = text(file, "/file")?;
            let path = dir.join(name);
            let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            match read(&bytes) {
                Ok(tree) => {
                    for d in diff(&tree, at(file, "/value")?) {
                        failures.push(format!("{name}: {d}"));
                    }
                }
                Err(e) => failures.push(format!("{name}: {e}")),
            }
        }
        check_eq(&files.len(), &398, "the bidder-info files")?;
        report("bidder_info.json", &failures)
    }
}
