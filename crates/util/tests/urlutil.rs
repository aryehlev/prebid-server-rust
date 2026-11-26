//! Go's `net/url` escapes against `pbs_util::urlutil` on the golden of
//! `testdata/go/golden/urlescape.go`: `QueryEscape` and `PathEscape` of 50,000 seeded strings,
//! of every rune the strings are made of and of hand-picked strings, `Values.Encode` of 50,000
//! seeded `url.Values` and of hand-picked pairs, and `ParseQuery` and `QueryUnescape` beside
//! `form_urlencoded`, whose differences the url.query-parse ledger entry records.

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, error::Error, fmt::Write as _, fs, path::PathBuf};

    use pbs_util::urlutil::{path_escape, query_escape, values_encode};
    use serde::Deserialize;
    use sha2::{Digest as _, Sha256};
    use url::form_urlencoded;

    type TestResult = Result<(), Box<dyn Error>>;

    #[derive(Deserialize)]
    struct Golden {
        seed: u64,
        count: usize,
        max_runes: usize,
        runes: Vec<String>,
        inputs_sha256: String,
        outputs_sha256: String,
        values_seed: u64,
        values_count: usize,
        values_keys: Vec<String>,
        values_inputs_sha256: String,
        values_outputs_sha256: String,
        edges: Vec<EscapeEdge>,
        values_edges: Vec<ValuesEdge>,
        parse_query: Vec<ParsedQuery>,
    }

    #[derive(Deserialize)]
    struct EscapeEdge {
        input: String,
        query: String,
        path: String,
        query_unescape: String,
        query_unescape_error: String,
    }

    #[derive(Deserialize)]
    struct ValuesEdge {
        pairs: Vec<String>,
        encode: String,
    }

    #[derive(Deserialize)]
    struct ParsedQuery {
        query: String,
        values: BTreeMap<String, Vec<String>>,
        error: String,
        get: BTreeMap<String, String>,
    }

    fn golden() -> Result<Golden, Box<dyn Error>> {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "testdata",
            "golden",
            "urlutil_escape.json",
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

        /// Go: `intn(n)`, `next() % n`.
        fn intn(&mut self, n: usize) -> usize {
            let n = u64::try_from(n).unwrap_or(u64::MAX).max(1);
            usize::try_from(self.next() % n).unwrap_or_default()
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

    /// Go: `urlEscapeRunes`: every ASCII character, then runes of two, three and four bytes,
    /// white space outside ASCII and a byte-order mark.
    fn runes() -> Vec<String> {
        let mut runes: Vec<String> = (0_u8..0x80).map(|b| char::from(b).to_string()).collect();
        for extra in [
            "\u{E9}",
            "\u{A0}",
            "\u{65E5}",
            "\u{2028}",
            "\u{FEFF}",
            "\u{1D11E}",
            "\u{130}",
            "\u{FFFD}",
        ] {
            runes.push(extra.to_owned());
        }
        runes
    }

    /// Go: `valuesKeys`.
    const VALUES_KEYS: [&str; 12] = [
        "", "a", "b", "B", "a b", "a+b", "~", "*", "id", "id[]", "\u{E9}", "z",
    ];

    /// Go: `seededText`: up to `max` runes, the count `intn(max+1)`, then each rune
    /// `runes[intn(len)]`.
    fn seeded_text(r: &mut SplitMix64, runes: &[String], max: usize) -> String {
        let n = r.intn(max + 1);
        let mut s = String::new();
        for _ in 0..n {
            let at = r.intn(runes.len());
            s.push_str(runes.get(at).map_or("", String::as_str));
        }
        s
    }

    /// Go: `seededValues`: up to 6 `Add` calls, each key from [`VALUES_KEYS`] or seeded, and
    /// the calls as key and value pairs.
    fn seeded_values(
        r: &mut SplitMix64,
        runes: &[String],
    ) -> (BTreeMap<String, Vec<String>>, Vec<String>) {
        let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut calls = Vec::new();
        for _ in 0..r.intn(7) {
            let key = if r.intn(3) == 0 {
                seeded_text(r, runes, 4)
            } else {
                let at = r.intn(VALUES_KEYS.len());
                VALUES_KEYS.get(at).copied().unwrap_or_default().to_owned()
            };
            let value = seeded_text(r, runes, 6);
            values.entry(key.clone()).or_default().push(value.clone());
            calls.push(key);
            calls.push(value);
        }
        (values, calls)
    }

    #[test]
    fn escape_like_go() -> TestResult {
        // Go: url.QueryEscape and url.PathEscape, with which Prebid's macros and user sync
        // URLs escape values, over 50,000 seeded strings of ASCII and multibyte runes, every
        // rune alone and hand-picked strings.
        let g = golden()?;
        let runes = runes();
        check(runes == g.runes, "the runes of the seeded strings")?;
        let mut r = SplitMix64(g.seed);
        let mut manifest = Manifest::default();
        for _ in 0..g.count {
            let s = seeded_text(&mut r, &runes, g.max_runes);
            let escaped = format!("{}\t{}", query_escape(&s), path_escape(&s));
            manifest.add(&s, &escaped);
        }
        let (inputs, outputs) = manifest.sums();
        check(g.count == 50_000, "50,000 seeded strings")?;
        check(inputs == g.inputs_sha256, "the seeded strings")?;
        check(outputs == g.outputs_sha256, "their escapes")?;
        check(g.edges.len() == runes.len() + 17, "the edge strings")?;
        for edge in &g.edges {
            let s = &edge.input;
            check(
                query_escape(s) == edge.query,
                &format!("QueryEscape({s:?})"),
            )?;
            check(path_escape(s) == edge.path, &format!("PathEscape({s:?})"))?;
        }
        Ok(())
    }

    #[test]
    fn values_sorted() -> TestResult {
        // Go: url.Values.Encode, which writes the keys in sorted order and the values of a key
        // in the order they were added, over 50,000 seeded url.Values and hand-picked pairs.
        // form_urlencoded's serializer escapes `~` and keeps `*`, so the pairs go through
        // QueryEscape's set instead.
        let g = golden()?;
        check(
            g.values_keys == VALUES_KEYS,
            "the keys of the seeded values",
        )?;
        let runes = runes();
        let mut r = SplitMix64(g.values_seed);
        let mut manifest = Manifest::default();
        for _ in 0..g.values_count {
            let (values, calls) = seeded_values(&mut r, &runes);
            manifest.add(&calls.join("\t"), &values_encode(&values));
        }
        let (inputs, outputs) = manifest.sums();
        check(g.values_count == 50_000, "50,000 seeded values")?;
        check(inputs == g.values_inputs_sha256, "the seeded values")?;
        check(outputs == g.values_outputs_sha256, "their encodings")?;
        check(g.values_edges.len() == 8, "the edge pairs")?;
        for edge in &g.values_edges {
            let mut values: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for [key, value] in edge.pairs.as_chunks::<2>().0 {
                values.entry(key).or_default().push(value);
            }
            let got = values_encode(&values);
            check(got == edge.encode, &format!("{:?}: got {got}", edge.pairs))?;
        }
        let crate_bytes: String = form_urlencoded::byte_serialize(b"~*").collect();
        check(
            crate_bytes == "%7E*" && query_escape("~*") == "~%2A",
            "form_urlencoded escapes ~ and keeps *, Go the other way round",
        )
    }

    /// `form_urlencoded`'s pairs of `query`, as a key's values in order.
    fn crate_pairs(query: &str) -> BTreeMap<String, Vec<String>> {
        let mut pairs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (key, value) in form_urlencoded::parse(query.as_bytes()) {
            pairs
                .entry(key.into_owned())
                .or_default()
                .push(value.into_owned());
        }
        pairs
    }

    #[test]
    fn query_parse_differs() -> TestResult {
        // Go: url.ParseQuery drops a pair that holds ';' or an invalid escape and reports an
        // error, Values.Get gives a key's first value, and url.QueryUnescape fails on an
        // invalid escape. form_urlencoded, which axum's Query and Form extractors read with
        // (serde_urlencoded), keeps such pairs, an invalid escape as written.
        let g = golden()?;
        check(g.parse_query.len() == 12, "the queries")?;
        let mut dropped = 0_usize;
        for row in &g.parse_query {
            let q = &row.query;
            let got = crate_pairs(q);
            if row.error.is_empty() {
                check(got == row.values, &format!("{q}: the pairs"))?;
                for (key, want) in &row.get {
                    let first = got
                        .get(key)
                        .and_then(|values| values.first())
                        .map_or("", String::as_str);
                    check(first == want, &format!("{q}: Get({key:?})"))?;
                }
            } else {
                // Every pair Go keeps is kept, and the pairs Go drops are kept too.
                check(got != row.values, &format!("{q}: Go drops a pair"))?;
                for (key, values) in &row.values {
                    check(got.get(key) == Some(values), &format!("{q}: {key}"))?;
                }
                dropped += 1;
            }
        }
        check(
            dropped == 3,
            "Go drops pairs of a=1;b=2, a=%zz&b=2 and a=%2",
        )?;
        let mut kept_escapes = 0_usize;
        for edge in &g.edges {
            let s = &edge.input;
            if s.is_empty() || s.contains(['&', '=']) {
                continue;
            }
            let got = crate_pairs(s);
            let key = got.keys().next().map_or("", String::as_str);
            if edge.query_unescape_error.is_empty() {
                check(key == edge.query_unescape, &format!("QueryUnescape({s:?})"))?;
            } else {
                check(
                    key == s.replace('+', " "),
                    &format!("{s:?} decodes as written"),
                )?;
                kept_escapes += 1;
            }
        }
        check(kept_escapes == 3, "the invalid escapes %, 100% and %zz")
    }
}
