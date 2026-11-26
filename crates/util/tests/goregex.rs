//! Go's `regexp` against `pbs_util::goregex` on the goldens of `testdata/go/golden/goregex.go`:
//! the 162 patterns of Prebid's code and bidder parameter schemas (GOC-REGEXP's `sites.txt`)
//! over 1,000 seeded subjects each, and edge patterns over a fixed list of subjects.

#[cfg(test)]
mod tests {
    use std::{error::Error, fmt::Write as _, fs, path::PathBuf};

    use pbs_util::goregex::{self, CompileError};
    use regex::Regex;
    use serde::Deserialize;
    use sha2::{Digest as _, Sha256};

    type TestResult = Result<(), Box<dyn Error>>;

    #[derive(Deserialize)]
    struct Golden {
        count: usize,
        max_tokens: usize,
        patterns: Vec<Site>,
        edge_subjects: Vec<String>,
        edges: Vec<Edge>,
    }

    #[derive(Deserialize)]
    struct Site {
        pattern: String,
        seed: u64,
        num_subexp: usize,
        names: Vec<String>,
        tokens: Vec<String>,
        matching: usize,
        inputs_sha256: String,
        outputs_sha256: String,
    }

    #[derive(Deserialize)]
    struct Edge {
        pattern: String,
        error: String,
        matches: Vec<String>,
    }

    fn golden() -> Result<Golden, Box<dyn Error>> {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "testdata",
            "golden",
            "goregex.json",
        ]
        .iter()
        .collect();
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn check(ok: bool, what: &str) -> TestResult {
        if ok { Ok(()) } else { Err(what.into()) }
    }

    /// The splitmix64 generator of the Go golden program (Go: `splitmix64`).
    struct SplitMix64(u64);

    impl SplitMix64 {
        const fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn intn(&mut self, n: usize) -> usize {
            let n = u64::try_from(n).unwrap_or(u64::MAX).max(1);
            usize::try_from(self.next() % n).unwrap_or(0)
        }
    }

    /// Go: `manifest`, SHA-256 over `len:input\n` and over `output\n`.
    #[derive(Default)]
    struct Manifest {
        inputs: Sha256,
        outputs: Sha256,
    }

    impl Manifest {
        fn add(&mut self, input: &str, output: &str) {
            self.inputs
                .update(format!("{}:{input}\n", input.len()).as_bytes());
            self.outputs.update(format!("{output}\n").as_bytes());
        }

        fn sums(self) -> (String, String) {
            let hex = |d: &[u8]| {
                d.iter().fold(String::new(), |mut s, b| {
                    let _ok = write!(s, "{b:02x}");
                    s
                })
            };
            (hex(&self.inputs.finalize()), hex(&self.outputs.finalize()))
        }
    }

    /// Go: `regexMatches`, `FindAllStringSubmatchIndex(s, -1)` with matches joined by `;` and the
    /// offsets of each joined by `,`, -1 for a group that did not take part.
    fn matches(re: &Regex, s: &str) -> String {
        let mut out = String::new();
        for caps in re.captures_iter(s) {
            if !out.is_empty() {
                out.push(';');
            }
            for i in 0..caps.len() {
                if i > 0 {
                    out.push(',');
                }
                let (start, end) = caps.get(i).map_or((-1, -1), |m| {
                    (
                        i64::try_from(m.start()).unwrap_or(-1),
                        i64::try_from(m.end()).unwrap_or(-1),
                    )
                });
                let _ok = write!(out, "{start},{end}");
            }
        }
        out
    }

    #[test]
    fn sites_match_go() -> TestResult {
        // Go: regexp.MustCompile(p).FindAllStringSubmatchIndex for every pattern of Prebid's code
        // and bidder parameter schemas, over subjects of its own literals and class bounds, strings
        // its syntax tree generates, and the runes on which Go's ASCII classes and the regex
        // crate's Unicode classes disagree.
        let doc = golden()?;
        check(doc.patterns.len() == 162, "the 162 site patterns")?;
        for site in &doc.patterns {
            let pattern = &site.pattern;
            let re = goregex::compile(pattern).map_err(|e| format!("{pattern}: {e}"))?;
            check(re.captures_len() == site.num_subexp + 1, pattern)?;
            let names: Vec<String> = re
                .capture_names()
                .map(|name| name.unwrap_or_default().to_owned())
                .collect();
            check(names == site.names, pattern)?;
            let mut rng = SplitMix64(site.seed);
            let mut manifest = Manifest::default();
            let mut matching = 0_usize;
            for _ in 0..doc.count {
                let tokens = rng.intn(doc.max_tokens + 1);
                let mut subject = String::new();
                for _ in 0..tokens {
                    let at = rng.intn(site.tokens.len());
                    subject.push_str(site.tokens.get(at).ok_or("token index")?);
                }
                let got = matches(&re, &subject);
                if !got.is_empty() {
                    matching += 1;
                }
                manifest.add(&subject, &got);
            }
            let (inputs, outputs) = manifest.sums();
            check(
                inputs == site.inputs_sha256,
                &format!("{pattern}: the subjects"),
            )?;
            check(
                matching == site.matching,
                &format!("{pattern}: subjects that match"),
            )?;
            check(
                outputs == site.outputs_sha256,
                &format!("{pattern}: the matches"),
            )?;
        }
        Ok(())
    }

    /// Checks one edge pattern against Go: the same error text, or the same matches of every
    /// edge subject.
    fn check_edge(edge: &Edge, subjects: &[String]) -> TestResult {
        let p = &edge.pattern;
        match goregex::compile(p) {
            Ok(re) => {
                check(
                    edge.error.is_empty(),
                    &format!("{p}: Go fails with {}", edge.error),
                )?;
                check(edge.matches.len() == subjects.len(), p)?;
                for (s, want) in subjects.iter().zip(&edge.matches) {
                    let got = matches(&re, s);
                    check(
                        &got == want,
                        &format!("{p} on {s:?}: got {got:?}, want {want:?}"),
                    )?;
                }
                Ok(())
            }
            Err(e @ CompileError::Syntax { .. }) => check(
                e.to_string() == edge.error,
                &format!("{p}: got {e}, want {}", edge.error),
            ),
            Err(e) => Err(format!("{p}: Go compiles it, the translation fails with {e}").into()),
        }
    }

    #[test]
    fn edges_like_go() -> TestResult {
        // Go: regexp.Compile and FindAllStringSubmatchIndex for each construct the translation
        // handles and the forms Go rejects: ASCII classes and boundaries, case folding, braces,
        // the characters the regex crate reads as set operators, escapes, flags and repetitions.
        let g = golden()?;
        check(g.edges.len() > 190, "the edge patterns")?;
        for edge in &g.edges {
            check_edge(edge, &g.edge_subjects)?;
        }
        Ok(())
    }

    #[test]
    fn quoted_literals() -> TestResult {
        // Go: \Q...\E quotes its text up to \E or the end of the pattern, outside classes only;
        // the regex crate has no \Q.
        let g = golden()?;
        let quoted: Vec<&Edge> = g
            .edges
            .iter()
            .filter(|e| e.pattern.contains(r"\Q") || e.pattern.contains(r"\E"))
            .collect();
        check(quoted.len() == 14, "the \\Q edge patterns")?;
        for edge in &quoted {
            check_edge(edge, &g.edge_subjects)?;
        }
        check(
            quoted.iter().all(|edge| Regex::new(&edge.pattern).is_err()),
            "the regex crate rejects each of them as written",
        )
    }

    #[test]
    fn untranslated_forms() -> TestResult {
        // Go: regexp.Compile accepts a repetition right after a flag group (a(?i)* repeats a),
        // escapes of surrogate code points, which match nothing, and counted repetitions up to
        // 1000 of any class; no Prebid pattern has one of these forms (regex.syntax-edges).
        for p in ["a(?i)*", r"\x{D800}", r"[\x{D800}]", r"[a-\x{DFFF}]"] {
            check(
                matches!(goregex::compile(p), Err(CompileError::Unsupported { .. })),
                &format!("{p}: not translated"),
            )?;
        }
        for p in [r"\pL{1000}", r"(?:\pL{100}){10}"] {
            check(
                matches!(goregex::compile(p), Err(CompileError::Crate(_))),
                &format!("{p}: over the regex crate's compiled size limit"),
            )?;
        }
        for p in [r"\w{1000}", "[^a]{1000}", ".{1000}"] {
            let _re = goregex::compile(p).map_err(|e| format!("{p}: {e}"))?;
        }
        Ok(())
    }

    #[test]
    fn unicode_16_letters() -> TestResult {
        // Go: regexp's Unicode classes and case folding are unicode.Version's tables, 15.0.0 in
        // Go 1.26, where U+1C89 (CYRILLIC CAPITAL LETTER TJE, new in Unicode 16.0) is
        // unassigned: \pL does not match it and (?i) folds it to nothing. The regex crate's
        // tables are Unicode 16.0's.
        let letter = goregex::compile(r"^\pL$")?;
        check(
            letter.is_match("\u{1C89}"),
            "\\pL matches a letter of Unicode 16.0",
        )?;
        let unassigned = goregex::compile(r"^\p{Cn}$")?;
        check(!unassigned.is_match("\u{1C89}"), "the letter is assigned")?;
        let folded = goregex::compile("(?i)^\u{1C89}$")?;
        check(
            folded.is_match("\u{1C8A}"),
            "(?i) folds it to its small letter",
        )
    }
}
