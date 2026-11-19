//! `FindElement` and `DropElement` against Go (`testdata/golden/element.json`):
//! `TestDropElement`'s cases and the decoder-state edges (errors, EOF, the index panics), and
//! 50,000 seeded documents and paths compared through their batches' digests.

#[cfg(test)]
pub mod seeded;
#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use pbs_jsonutil::{ElementError, drop_element, find_element};
    use serde_json::Value;

    use super::{
        seeded::{SplitMix64, check_digests},
        support::{TestResult, ensure_eq, golden, list, text},
    };

    /// Go's `elementRawKeys` (the last one an escaped `a`).
    const RAW_KEYS: [&str; 6] = ["a", "b", "ext", "test", "x:y", "\\u0061"];

    /// Go's `elementNames`.
    const NAMES: [&str; 5] = ["a", "b", "ext", "test", "x:y"];

    /// Go's `findOutcome`.
    fn find_outcome(doc: &[u8], names: &[&str]) -> String {
        match find_element(doc, names) {
            Ok(span) => format!("{} {} {}", span.found, span.start, span.end),
            Err(error) => failure(&error),
        }
    }

    /// Go's `dropOutcome`.
    fn drop_outcome(doc: &[u8], names: &[&str]) -> String {
        match drop_element(doc, names) {
            Ok(out) => format!("ok {}", String::from_utf8_lossy(&out)),
            Err(error) => failure(&error),
        }
    }

    /// An error or a panic as the goldens write them.
    fn failure(error: &ElementError) -> String {
        match error {
            ElementError::Decode(message) => format!("error: {message}"),
            ElementError::Panic(message) => format!("panic: {message}"),
        }
    }

    /// Go's `space`.
    fn space(rng: &mut SplitMix64) -> &'static str {
        match rng.below(6) {
            0 => " ",
            1 => "\n\t",
            _ => "",
        }
    }

    /// Go's `elementValue`.
    fn value(rng: &mut SplitMix64, depth: usize) -> String {
        let mut n = rng.below(10);
        if depth >= 3 && n >= 6 {
            n = rng.below(6);
        }
        match n {
            0 => "null".to_owned(),
            1 => "true".to_owned(),
            2 => rng.below(1000).to_string(),
            3 => "-1.5e3".to_owned(),
            4 | 5 => format!("\"{}\"", rng.pick(&RAW_KEYS)),
            6 | 7 => object(rng, depth + 1),
            _ => array(rng, depth + 1),
        }
    }

    /// Go's `elementObject`.
    fn object(rng: &mut SplitMix64, depth: usize) -> String {
        let mut out = String::from("{");
        for i in 0..rng.below(4) {
            if i > 0 {
                out.push_str(space(rng));
                out.push(',');
            }
            out.push_str(space(rng));
            out.push('"');
            out.push_str(rng.pick(&RAW_KEYS));
            out.push('"');
            out.push_str(space(rng));
            out.push(':');
            out.push_str(space(rng));
            out.push_str(&value(rng, depth));
        }
        out.push_str(space(rng));
        out.push('}');
        out
    }

    /// Go's `elementArray`.
    fn array(rng: &mut SplitMix64, depth: usize) -> String {
        let mut out = String::from("[");
        for i in 0..rng.below(4) {
            if i > 0 {
                out.push_str(space(rng));
                out.push(',');
            }
            out.push_str(space(rng));
            out.push_str(&value(rng, depth));
        }
        out.push_str(space(rng));
        out.push(']');
        out
    }

    /// Go's `elementCase`: a document and the path searched in it.
    fn case(rng: &mut SplitMix64) -> (String, Vec<&'static str>) {
        let doc = match rng.below(20) {
            0 => array(rng, 1),
            1 => value(rng, 1),
            _ => object(rng, 1),
        };
        let k = 1 + rng.below(3);
        let names = (0..k).map(|_| rng.pick(&NAMES)).collect();
        (doc, names)
    }

    /// A curated case's path.
    fn names(case: &Value) -> TestResult<Vec<&str>> {
        list(case, "/names")?
            .iter()
            .map(|name| name.as_str().ok_or_else(|| "a name is not a string".into()))
            .collect()
    }

    /// `TestDropElement`'s cases and the decoder's edges give Go's offsets, output bytes, error
    /// texts and panics, wherever Go's text comes from the token walk (`texts` is `go`).
    // Go: util/jsonutil/jsonutil.go:26-134 (FindElement, DropElement), jsonutil_test.go:16-191
    #[test]
    fn drop_element_cases_match_go() -> TestResult {
        let doc = golden("element.json")?;
        let mut compared = 0;
        for case in list(&doc, "/curated")? {
            if text(case, "/texts")? != "go" {
                continue;
            }
            let what = text(case, "/description")?;
            let input = text(case, "/input")?.as_bytes();
            let path = names(case)?;
            ensure_eq(
                &find_outcome(input, &path).as_str(),
                &text(case, "/find")?,
                what,
            )?;
            ensure_eq(
                &drop_outcome(input, &path).as_str(),
                &text(case, "/drop")?,
                what,
            )?;
            compared += 1;
        }
        ensure_eq(&(compared >= 40), &true, "curated cases compared")
    }

    /// Where `serde_json` reads a malformed value, the walk fails with its message where Go's
    /// decoder fails with its own; a key that is not UTF-8 fails here where Go reads it as
    /// U+FFFD and finds the member after it (`json.invalid-utf8`).
    // Go: util/jsonutil/jsonutil.go:26-134 and encoding/json's scanner (stream.go readValue)
    #[test]
    fn malformed_values_fail_with_serde_texts() -> TestResult {
        let doc = golden("element.json")?;
        let mut compared = 0;
        for case in list(&doc, "/curated")? {
            if text(case, "/texts")? == "go" {
                continue;
            }
            let what = text(case, "/description")?;
            let path = names(case)?;
            let go_find = text(case, "/find")?;
            if what.contains("UTF-8") {
                // The golden writes the invalid byte as U+FFFD; the input held 0xff.
                let input = text(case, "/input")?.replace('\u{fffd}', "\u{0}");
                let input: Vec<u8> = input
                    .bytes()
                    .map(|b| if b == 0 { 0xFF } else { b })
                    .collect();
                ensure_eq(&go_find, &"true 6 12", what)?;
                let rust = find_outcome(&input, &path);
                ensure_eq(
                    &rust.starts_with("error: "),
                    &true,
                    &format!("{what}: {rust}"),
                )?;
            } else {
                let input = text(case, "/input")?.as_bytes();
                let rust = find_outcome(input, &path);
                ensure_eq(&go_find.starts_with("error: "), &true, what)?;
                ensure_eq(
                    &rust.starts_with("error: "),
                    &true,
                    &format!("{what}: {rust}"),
                )?;
                ensure_eq(&(rust.as_str() == go_find), &false, what)?;
            }
            compared += 1;
        }
        ensure_eq(&compared, &4, "library cases compared")
    }

    /// 50,000 seeded documents (objects, arrays and scalars with whitespace, escaped keys and
    /// keys holding a colon) and paths of one to three names give Go's outcomes byte for byte:
    /// offsets, output, error texts and panics.
    // Go: util/jsonutil/jsonutil.go:26-134; testdata/go/gen/element.go (elementCorpus)
    #[test]
    fn seeded_documents_match_go() -> TestResult {
        let doc = golden("element.json")?;
        check_digests(&doc, "/seeded", "element", |rng| {
            let (input, path) = case(rng);
            let line = format!("{input}\t{}", path.join(","));
            let outcome = format!(
                "{}\t{}",
                find_outcome(input.as_bytes(), &path),
                drop_outcome(input.as_bytes(), &path)
            );
            Ok((line, outcome))
        })
    }
}
