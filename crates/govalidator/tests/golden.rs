//! Differential tests: replays the goldens that `testdata/go/golden` (a Go program running
//! govalidator at the version Prebid requires, under Go 1.26.2) wrote to `testdata/golden`.
//!
//! - `validator.txt`: `IsURL`, `IsRequestURL`, `IsRequestURI`, `IsIP` and `net.ParseIP` of 2,844
//!   inputs from govalidator's tests, Go's `net/url`, `net` and `net/netip` tests and the URLs of
//!   Prebid's bidder-info files and configs, one `strconv.Quote`d input and its outcome per line;
//! - `pattern.txt`: govalidator's `URL` pattern, quoted;
//! - `sweep.txt`: two sweeps of 50,000 seeded URL-like and IP-like inputs, with the digests of the
//!   inputs and of their outcomes.

#[cfg(test)]
mod tests {
    use std::{fs, net::IpAddr, path::Path, str::Chars, thread};

    use pbs_govalidator::{URL, is_ip, is_request_uri, is_request_url, is_url};
    use pbs_util::{goregex, iputil::net::parse_ip};
    use sha2::{Digest as _, Sha256};

    /// Go: `maxTokens` in `sweep.go`.
    const MAX_TOKENS: u64 = 12;

    /// The failures shown before the rest are only counted.
    const SHOWN: usize = 20;

    /// A golden file of `testdata/golden`.
    fn read(name: &str) -> Result<String, String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/golden")
            .join(name);
        fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The `n` characters after an escape's letter, as a number in `radix`.
    fn escaped(chars: &mut Chars<'_>, n: usize, radix: u32) -> Result<u32, String> {
        let digits: String = chars.by_ref().take(n).collect();
        u32::from_str_radix(&digits, radix).map_err(|e| format!("escape {digits:?}: {e}"))
    }

    /// Go: `strconv.Unquote` of what `strconv.Quote` writes, for a string that is UTF-8.
    fn unquote(field: &str) -> Result<String, String> {
        let inner = field
            .strip_prefix('"')
            .and_then(|f| f.strip_suffix('"'))
            .ok_or_else(|| format!("{field} is not quoted"))?;
        let mut out = Vec::new();
        let mut chars = inner.chars();
        let mut buf = [0; 4];
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                continue;
            }
            let rune = match chars.next() {
                Some('a') => '\u{7}',
                Some('b') => '\u{8}',
                Some('f') => '\u{c}',
                Some('n') => '\n',
                Some('r') => '\r',
                Some('t') => '\t',
                Some('v') => '\u{b}',
                Some(quoted @ ('\\' | '"' | '\'')) => quoted,
                Some('x') => {
                    let byte =
                        u8::try_from(escaped(&mut chars, 2, 16)?).map_err(|e| e.to_string())?;
                    out.push(byte);
                    continue;
                }
                Some('u') => char::from_u32(escaped(&mut chars, 4, 16)?).ok_or("bad \\u")?,
                Some('U') => char::from_u32(escaped(&mut chars, 8, 16)?).ok_or("bad \\U")?,
                other => return Err(format!("{field}: unknown escape {other:?}")),
            };
            out.extend_from_slice(rune.encode_utf8(&mut buf).as_bytes());
        }
        String::from_utf8(out).map_err(|e| format!("{field}: {e}"))
    }

    /// Bytes in lower-case hex, as Go's `hex.EncodeToString` writes them.
    fn hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .flat_map(|&b| [b >> 4, b & 0xF])
            .filter_map(|d| char::from_digit(u32::from(d), 16))
            .collect()
    }

    /// Go: `outcome` in `testdata/go/golden/main.go`: `IsURL`, `IsRequestURL`, `IsRequestURI`,
    /// `IsIP` and `net.ParseIP` (its 16 bytes in hex, or `nil`).
    fn outcome(s: &str) -> String {
        let ip = parse_ip(s).map_or_else(|| "nil".to_owned(), |ip| hex(&net_ip(ip)));
        format!(
            "{}\t{}\t{}\t{}\t{ip}",
            is_url(s),
            is_request_url(s),
            is_request_uri(s),
            is_ip(s)
        )
    }

    /// The 16 bytes Go's `net.IP` holds for an address: an IPv4 one in its mapped form.
    fn net_ip(ip: IpAddr) -> [u8; 16] {
        match ip {
            IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
            IpAddr::V6(v6) => v6.octets(),
        }
    }

    /// Failures, of which the first [`SHOWN`] are kept.
    #[derive(Debug, Default)]
    struct Failures {
        shown: Vec<String>,
        count: usize,
    }

    impl Failures {
        fn add(&mut self, failure: String) {
            self.count += 1;
            if self.shown.len() < SHOWN {
                self.shown.push(failure);
            }
        }

        fn finish(self, what: &str) -> Result<(), String> {
            if self.count == 0 {
                return Ok(());
            }
            Err(format!(
                "{what}: {} failures:\n{}",
                self.count,
                self.shown.join("\n")
            ))
        }
    }

    /// url.govalidator-checks: the four checks and `net.ParseIP` give Go's results on every
    /// input of the corpus.
    #[test]
    fn validator_matches_go() -> Result<(), String> {
        let text = read("validator.txt")?;
        let mut failures = Failures::default();
        let mut cases = 0usize;
        for (n, line) in text.lines().enumerate() {
            let (quoted, want) = line
                .split_once('\t')
                .ok_or_else(|| format!("validator.txt:{}: no outcome", n + 1))?;
            let got = outcome(&unquote(quoted)?);
            if got != want {
                failures.add(format!(
                    "validator.txt:{}: {quoted}: {got}, want {want}",
                    n + 1
                ));
            }
            cases += 1;
        }
        if cases < 2_800 {
            failures.add(format!("validator.txt has {cases} cases"));
        }
        failures.finish("validator.txt")
    }

    /// Go: `patterns.go` assembles `URL` from its parts, and `rxURL = regexp.MustCompile(URL)`.
    #[test]
    fn url_pattern_is_govalidators() -> Result<(), String> {
        let want = unquote(read("pattern.txt")?.trim_end())?;
        if URL != want {
            return Err(format!("URL pattern {URL:?}, want {want:?}"));
        }
        goregex::compile(URL)
            .map(|_rx| ())
            .map_err(|e| e.to_string())
    }

    /// Go: `splitmix64` in `sweep.go`.
    struct SplitMix64(u64);

    impl SplitMix64 {
        const fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Go: `below(n)`: `next() % n`.
        fn below(&mut self, n: u64) -> usize {
            usize::try_from(self.next() % n.max(1)).unwrap_or(usize::MAX)
        }
    }

    /// One sweep.txt line: its name, seed and count, the two digests, then the tokens, each
    /// quoted. Go quotes rune by rune, so the quoted form of an input is its tokens' quoted
    /// forms joined.
    fn run_sweep(line: &str) -> Result<(), String> {
        let fields: Vec<&str> = line.split('\t').collect();
        let [name, seed, count, want_in, want_out, quoted @ ..] = fields.as_slice() else {
            return Err(format!("short sweep line {line:?}"));
        };
        let seed: u64 = seed.parse().map_err(|e| format!("{name}: seed: {e}"))?;
        let count: usize = count.parse().map_err(|e| format!("{name}: count: {e}"))?;
        let tokens = quoted
            .iter()
            .map(|t| unquote(t))
            .collect::<Result<Vec<_>, _>>()?;
        let bodies: Vec<&str> = quoted
            .iter()
            .map(|t| t.get(1..t.len().saturating_sub(1)).unwrap_or_default())
            .collect();
        let len = u64::try_from(tokens.len()).map_err(|e| e.to_string())?;
        let mut rng = SplitMix64(seed);
        let (mut inputs, mut outcomes) = (Sha256::new(), Sha256::new());
        for _ in 0..count {
            let (mut input, mut quoted_input) = (String::new(), String::from("\""));
            for _ in 0..rng.below(MAX_TOKENS + 1) {
                let i = rng.below(len);
                input.push_str(tokens.get(i).map_or("", String::as_str));
                quoted_input.push_str(bodies.get(i).copied().unwrap_or_default());
            }
            quoted_input.push('"');
            inputs.update(format!("{quoted_input}\n"));
            outcomes.update(format!("{quoted_input}\t{}\n", outcome(&input)));
        }
        if hex(&inputs.finalize()) != *want_in {
            return Err(format!(
                "{name}: the inputs differ from Go's: the generator drifted"
            ));
        }
        if hex(&outcomes.finalize()) != *want_out {
            return Err(format!("{name}: the outcomes differ from Go's"));
        }
        Ok(())
    }

    /// url.govalidator-checks over 50,000 seeded URL-like and 50,000 seeded IP-like inputs:
    /// schemes, separators, user info, IP literals, zones, ports, escapes valid and not,
    /// spaces, multibyte text and template actions.
    #[test]
    fn sweeps_match_go() -> Result<(), String> {
        let text = read("sweep.txt")?;
        let results: Vec<Result<(), String>> = thread::scope(|scope| {
            // Every sweep gets a thread of its own before any is joined.
            let mut handles = Vec::new();
            for line in text.lines() {
                handles.push(scope.spawn(move || run_sweep(line)));
            }
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err("a sweep panicked".to_owned()))
                })
                .collect()
        });
        let mut failures = Failures::default();
        if results.len() != 2 {
            failures.add(format!("sweep.txt has {} sweeps", results.len()));
        }
        for result in results {
            if let Err(e) = result {
                failures.add(e);
            }
        }
        failures.finish("sweep.txt")
    }
}
