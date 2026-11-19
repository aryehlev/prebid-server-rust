//! Pins of the `regex` crate against Go's `regexp` (RE2 syntax in both): the Perl classes on
//! non-ASCII text, `\Q...\E` quoting and the `(?-u)` flag (`testdata/golden/regex.json`).

#[cfg(test)]
mod tests {
    use pbs_differences::{BoxError, TestResult, check_table, golden, list, text};
    use regex::Regex;
    use serde_json::Value;

    /// Per golden case: the pattern, then per input the input, what `regexp` answered and what
    /// `regex` answers (`compile error` when a side rejects the pattern).
    fn rows(doc: &Value, patterns: &[&str]) -> Result<Vec<Vec<String>>, BoxError> {
        let mut rows = Vec::new();
        for case in list(doc, "/cases")? {
            let pattern = text(case, "/pattern")?;
            if !patterns.contains(&pattern) {
                continue;
            }
            let compiled = Regex::new(pattern);
            let go_compiles = text(case, "/compile_err")?.is_empty();
            let inputs = case
                .get("matches")
                .and_then(Value::as_array)
                .map_or_default(Vec::as_slice);
            if inputs.is_empty() {
                rows.push(vec![
                    pattern.to_owned(),
                    String::new(),
                    outcome(go_compiles, false),
                    compiled
                        .as_ref()
                        .map_or_else(|_| "compile error".to_owned(), |_| "compiles".to_owned()),
                ]);
            }
            for item in inputs {
                let input = text(item, "/input")?;
                let go = item
                    .get("match")
                    .and_then(Value::as_bool)
                    .unwrap_or_default();
                rows.push(vec![
                    pattern.to_owned(),
                    input.to_owned(),
                    outcome(go_compiles, go),
                    compiled.as_ref().map_or_else(
                        |_| "compile error".to_owned(),
                        |re| outcome(true, re.is_match(input)),
                    ),
                ]);
            }
        }
        Ok(rows)
    }

    /// `match`, `no match`, or `compile error` when the pattern did not compile.
    fn outcome(compiled: bool, matched: bool) -> String {
        match (compiled, matched) {
            (false, _) => "compile error",
            (true, true) => "match",
            (true, false) => "no match",
        }
        .to_owned()
    }

    /// Go's `\d`, `\w`, `\s` and `\b` are ASCII-only and Go has no `(?-u)` flag; the `regex`
    /// crate's classes are Unicode-aware, so Arabic-Indic digits, a Latin letter with an accent
    /// and CJK letters, a no-break space and an em space match where Go's do not, and `\b` sees
    /// no boundary between an accented letter and `foo`. `(?-u:\d)` gives the crate Go's class,
    /// and is a syntax error in Go.
    // Go: static/bidder-params/visx.json:9 ("^\\d+$", compiled by gojsonschema at
    // openrtb_ext/bidders.go:792 and checked at :859), adapters/silverpush/devicetype.go:11 (\b
    // over the User-Agent) and usersync/syncer.go:105-111 (\s in the sync URL macros)
    #[test]
    fn go_difference_unicode_classes() -> TestResult {
        let doc = golden("regex.json")?;
        let table = rows(
            &doc,
            &[r"^\d+$", r"^\w+$", r"^\s$", r"\bfoo\b", r"^(?-u:\d)+$"],
        )?;
        check_table(
            &table,
            &[
                &[r"^\d+$", "123", "match", "match"],
                &[r"^\d+$", "\u{663}\u{664}", "no match", "match"],
                &[r"^\w+$", "abc_1", "match", "match"],
                &[r"^\w+$", "\u{e9}", "no match", "match"],
                &[r"^\w+$", "\u{65e5}\u{672c}", "no match", "match"],
                &[r"^\s$", " ", "match", "match"],
                &[r"^\s$", "\t", "match", "match"],
                &[r"^\s$", "\u{a0}", "no match", "match"],
                &[r"^\s$", "\u{2003}", "no match", "match"],
                &[r"\bfoo\b", "a foo b", "match", "match"],
                &[r"\bfoo\b", "\u{e9}foo\u{e9}", "match", "no match"],
                &[r"^(?-u:\d)+$", "", "compile error", "compiles"],
            ],
            "pattern, input, regexp, regex",
        )
    }

    /// Go's `regexp` reads `\Q...\E` as a literal; the `regex` crate rejects the pattern. No
    /// Prebid pattern uses it today; every pattern Prebid compiles is read this way.
    // Go: regexp/syntax's \Q...\E literal mode, which applies to every pattern Prebid compiles: the
    // bidder-params schemas through gojsonschema (openrtb_ext/bidders.go:792) and the
    // regexp.MustCompile sites
    #[test]
    fn go_difference_quote_meta_escape() -> TestResult {
        let doc = golden("regex.json")?;
        let table = rows(&doc, &[r"\Qa.b\E"])?;
        check_table(
            &table,
            &[
                &[r"\Qa.b\E", "a.b", "match", "compile error"],
                &[r"\Qa.b\E", "axb", "no match", "compile error"],
            ],
            "pattern, input, regexp, regex",
        )
    }
}
