//! Pins of the consent-string base64 glue (`pbs_tcf2::base64`, `base64.leniency` in the differences
//! ledger) against Go's `base64.RawURLEncoding`, as go-gdpr's `ParseString` calls it: the decodes
//! `testdata/base64_goldens.json` and `testdata/base64_seeded_goldens.json` record from go1.26.2
//! (written by `testdata/go`), beside what the base64 crate's `URL_SAFE_NO_PAD` does alone.

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fmt::{Debug, Display},
        fs,
        path::Path,
    };

    use base64::{DecodeError, Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use pbs_tcf2::base64::raw_url_decode;
    use serde_json::Value;
    use sha2::{Digest as _, Sha256};

    type TestResult = Result<(), Box<dyn Error>>;

    /// Fails the test with `message` unless `left == right`.
    fn check_eq<T: PartialEq + Debug>(left: &T, right: &T, message: &str) -> TestResult {
        if left == right {
            Ok(())
        } else {
            Err(format!("{message}: {left:?} != {right:?}").into())
        }
    }

    /// Reads `testdata/<name>`, written by go-gdpr v1.12.0's generator.
    fn golden(name: &str) -> Result<Value, Box<dyn Error>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(name);
        let doc: Value = serde_json::from_slice(&fs::read(&path)?)?;
        check_eq(
            &doc.pointer("/header/go_gdpr_version")
                .and_then(Value::as_str),
            &Some("v1.12.0"),
            name,
        )?;
        check_eq(
            &doc.pointer("/header/go_version").and_then(Value::as_str),
            &Some("go1.26.2"),
            name,
        )?;
        Ok(doc)
    }

    /// `bytes` as lower-case hex, Go's `hex.EncodeToString`.
    fn hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .flat_map(|b| [b >> 4, b & 0x0F])
            .filter_map(|nibble| char::from_digit(u32::from(nibble), 16))
            .collect()
    }

    /// The bytes a lower-case hex string spells.
    fn unhex(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
        let (pairs, rest) = text.as_bytes().as_chunks::<2>();
        if !rest.is_empty() {
            return Err(format!("odd-length hex {text:?}").into());
        }
        pairs
            .iter()
            .map(|pair| Ok(u8::from_str_radix(str::from_utf8(pair)?, 16)?))
            .collect()
    }

    /// The input of a golden case: `input` (valid UTF-8) or `input_hex`.
    fn input(case: &Value) -> Result<Vec<u8>, Box<dyn Error>> {
        match (case.get("input"), case.get("input_hex")) {
            (Some(Value::String(text)), _) => Ok(text.as_bytes().to_vec()),
            (_, Some(Value::String(hex))) => unhex(hex),
            _ => Err(format!("case without input: {case}").into()),
        }
    }

    /// A decode as the generator records it: `=` and the hex of the bytes, or `!` and the error.
    fn render<E: Display>(result: Result<Vec<u8>, E>) -> String {
        match result {
            Ok(bytes) => format!("={}", hex(&bytes)),
            Err(err) => format!("!{err}"),
        }
    }

    /// The Go decode `key` of a `base64_goldens.json` case, as [`render`] writes it.
    fn go_decode(case: &Value, key: &str) -> Result<String, Box<dyn Error>> {
        let result = case
            .get(key)
            .ok_or_else(|| format!("case without {key}: {case}"))?;
        match (result.get("err"), result.get("hex")) {
            (Some(Value::String(err)), _) => Ok(format!("!{err}")),
            (_, Some(Value::String(hex))) => Ok(format!("={hex}")),
            (_, None) if result.get("len").and_then(Value::as_u64) == Some(0) => Ok("=".to_owned()),
            _ => Err(format!("unreadable {key} result: {case}").into()),
        }
    }

    /// How the crate's `URL_SAFE_NO_PAD`, alone, answers an input Go decodes or refuses.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct CrateAlone {
        /// Inputs both decode to the same bytes.
        same_bytes: usize,
        /// Inputs Go decodes and the crate refuses because they hold `\r` or `\n`.
        refused_line_breaks: usize,
        /// Inputs Go decodes and the crate refuses for the unused bits of their last digit.
        refused_trailing_bits: usize,
        /// Inputs both refuse, the crate at Go's offset.
        same_offset: usize,
        /// Inputs both refuse, the crate at another offset or with none.
        other_offset: usize,
    }

    /// The offset the crate's error names, if any.
    const fn crate_offset(err: &DecodeError) -> Option<usize> {
        match *err {
            DecodeError::InvalidByte(offset, _)
            | DecodeError::InvalidLastSymbol { offset, .. }
            | DecodeError::InvalidLength(offset) => Some(offset),
            DecodeError::InvalidPadding => None,
        }
    }

    /// Go's `RawURLEncoding` skips `\r` and `\n` and accepts a last digit with unused bits set,
    /// which the base64 crate's `URL_SAFE_NO_PAD` refuses, and it reports some errors at another
    /// offset than the crate; `raw_url_decode` gives Go's bytes and Go's error text for every
    /// input of `base64_goldens.json`, whose `DecodeString` and in-place `Decode` results agree.
    // Go: vendorconsent/tcf2/consent.go:27-33 and vendorconsent/tcf1/consent.go:16-22 (go-gdpr
    // v1.12.0), adapters/flipp/flipp.go:291, and $GOROOT/src/encoding/base64/base64.go:312-407;
    // testdata/go/b64.go writes the golden.
    #[test]
    fn go_leniency() -> TestResult {
        let doc = golden("base64_goldens.json")?;
        let cases = doc
            .get("cases")
            .and_then(Value::as_array)
            .ok_or("no cases")?;
        check_eq(&cases.len(), &1341, "cases")?;
        let mut alone = CrateAlone::default();
        for case in cases {
            let input = input(case)?;
            let go = go_decode(case, "raw_url")?;
            check_eq(&go_decode(case, "raw_url_in_place")?, &go, "in place")?;
            check_eq(&render(raw_url_decode(&input)), &go, &format!("{input:?}"))?;

            let crate_result = URL_SAFE_NO_PAD.decode(&input);
            match (go.strip_prefix('!'), &crate_result) {
                (None, Ok(bytes)) => {
                    check_eq(&format!("={}", hex(bytes)), &go, "bytes")?;
                    alone.same_bytes += 1;
                }
                (None, Err(_)) if input.iter().any(|&b| b == b'\r' || b == b'\n') => {
                    alone.refused_line_breaks += 1;
                }
                (None, Err(DecodeError::InvalidLastSymbol { .. })) => {
                    alone.refused_trailing_bits += 1;
                }
                (Some(go_err), Err(err)) => {
                    let go_offset = go_err.strip_prefix("illegal base64 data at input byte ");
                    if go_offset.map(str::parse::<usize>).transpose()? == crate_offset(err) {
                        alone.same_offset += 1;
                    } else {
                        alone.other_offset += 1;
                    }
                }
                (Some(_), Ok(_)) | (None, Err(_)) => {
                    return Err(format!(
                        "{input:?}: Go {go}, the crate {}",
                        render(crate_result.clone())
                    )
                    .into());
                }
            }
        }
        check_eq(
            &alone,
            &CrateAlone {
                same_bytes: 159,
                refused_line_breaks: 32,
                refused_trailing_bits: 160,
                same_offset: 527,
                other_offset: 463,
            },
            "the crate alone beside Go",
        )?;

        // Side by side for one input of each kind: Go's decode (the golden's, which the glue
        // gives, as checked above) and the crate's alone.
        for (text, go, crate_alone) in [
            ("A\nA\nA\nA", "=000000", "!Invalid symbol 10, offset 1."),
            (
                "QR",
                "=41",
                "!Invalid last symbol 0x52 ('R') at offset 1, decoded as 0b00010001.",
            ),
            (
                "w=+/+",
                "!illegal base64 data at input byte 1",
                "!Invalid symbol 43, offset 4.",
            ),
            (
                "AA==",
                "!illegal base64 data at input byte 2",
                "!Invalid padding",
            ),
        ] {
            let case = cases
                .iter()
                .find(|case| input(case).is_ok_and(|bytes| bytes == text.as_bytes()))
                .ok_or_else(|| format!("no case {text:?}"))?;
            check_eq(&go_decode(case, "raw_url")?, &go.to_owned(), text)?;
            check_eq(
                &render(URL_SAFE_NO_PAD.decode(text)),
                &crate_alone.to_owned(),
                text,
            )?;
        }
        Ok(())
    }

    /// Go's splitmix64, which `testdata/go/seeded.go` and this test share.
    struct SplitMix64(u64);

    impl SplitMix64 {
        const fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// A number in `[0, n)`.
        fn below(&mut self, n: usize) -> usize {
            let n = u64::try_from(n).unwrap_or(u64::MAX);
            usize::try_from(self.next() % n).unwrap_or_default()
        }
    }

    /// The alphabet of `base64.RawURLEncoding`.
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    /// seeded.go's `seededSpecials`.
    const SPECIALS: [&[u8]; 16] = [
        b"\n",
        b"\r",
        b"\r\n",
        b"=",
        b"==",
        b"+",
        b"/",
        b".",
        b"~",
        b" ",
        b"\t",
        b"\x00",
        b"\x7f",
        b"\x80",
        b"\xff",
        b"\xc3\xa9",
    ];

    /// seeded.go's `b64Input`.
    fn seeded_input(generator: &mut SplitMix64) -> Vec<u8> {
        let n = match generator.below(8) {
            0 => generator.below(301),
            1 => generator.below(5),
            _ => generator.below(41),
        };
        let noise = [0, 0, 64, 16, 4]
            .get(generator.below(5))
            .copied()
            .unwrap_or_default();
        let mut out = Vec::new();
        for _ in 0..n {
            if noise > 0 && generator.below(noise) == 0 {
                let special = SPECIALS.get(generator.below(SPECIALS.len()));
                out.extend_from_slice(special.copied().unwrap_or_default());
            } else if let Some(&digit) = ALPHABET.get(generator.below(ALPHABET.len())) {
                out.push(digit);
            }
        }
        out
    }

    /// seeded.go's `trailingBits`: whether `input`, which decoded, ends in a digit whose unused
    /// low bits are not zero.
    fn trailing_bits(input: &[u8]) -> bool {
        let digits: Vec<u8> = input
            .iter()
            .copied()
            .filter(|&b| b != b'\r' && b != b'\n')
            .collect();
        let unused = match digits.len() % 4 {
            2 => 4,
            3 => 2,
            _ => return false,
        };
        digits
            .last()
            .and_then(|last| ALPHABET.iter().position(|digit| digit == last))
            .is_some_and(|value| value & ((1 << unused) - 1) != 0)
    }

    /// One batch of the golden, as seeded.go's `seededBatchDigest` records it.
    #[derive(Debug, PartialEq, Eq)]
    struct Batch {
        inputs_sha256: String,
        results_sha256: String,
        accepted: u64,
        accepted_newlines: u64,
        accepted_trailing_bits: u64,
    }

    impl Batch {
        fn from_golden(batch: &Value) -> Result<Self, Box<dyn Error>> {
            let text = |key: &str| {
                batch
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| format!("batch without {key}"))
            };
            let count = |key: &str| {
                batch
                    .get(key)
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("batch without {key}"))
            };
            Ok(Self {
                inputs_sha256: text("inputs_sha256")?,
                results_sha256: text("results_sha256")?,
                accepted: count("accepted")?,
                accepted_newlines: count("accepted_newlines")?,
                accepted_trailing_bits: count("accepted_trailing_bits")?,
            })
        }
    }

    /// Each of 50,000 seeded strings (base64url with line breaks, `=`, the standard alphabet's
    /// digits, separators, control and non-ASCII bytes mixed in) decodes to Go's bytes or fails
    /// with Go's text (`illegal base64 data at input byte N`), batch by batch.
    // Go: testdata/go/seeded.go (seededResult: go-gdpr's in-place RawURLEncoding.Decode,
    // vendorconsent/tcf2/consent.go:27-33, checked there against DecodeString).
    #[test]
    fn go_seeded() -> TestResult {
        let doc = golden("base64_seeded_goldens.json")?;
        check_eq(
            &doc.get("seed").and_then(Value::as_str),
            &Some("0x4236344c454e3031"),
            "seed",
        )?;
        check_eq(
            &doc.get("count").and_then(Value::as_u64),
            &Some(50_000),
            "count",
        )?;
        let per_batch = doc
            .get("batch")
            .and_then(Value::as_u64)
            .ok_or("no batch size")?;
        let batches = doc
            .get("batches")
            .and_then(Value::as_array)
            .ok_or("no batches")?;
        check_eq(
            &(per_batch * u64::try_from(batches.len())?),
            &50_000,
            "strings",
        )?;
        let mut generator = SplitMix64(0x4236_344C_454E_3031);
        let mut totals = [0_u64; 3];
        for (n, golden_batch) in batches.iter().enumerate() {
            let mut inputs = Sha256::new();
            let mut results = Sha256::new();
            let mut counts = [0_u64; 3];
            for _ in 0..per_batch {
                let input = seeded_input(&mut generator);
                inputs.update(format!("{}\n", hex(&input)));
                let result = raw_url_decode(&input);
                if result.is_ok() {
                    counts[0] += 1;
                    counts[1] += u64::from(input.iter().any(|&b| b == b'\r' || b == b'\n'));
                    counts[2] += u64::from(trailing_bits(&input));
                }
                results.update(format!("{}\n", render(result)));
            }
            let batch = Batch {
                inputs_sha256: hex(&inputs.finalize()),
                results_sha256: hex(&results.finalize()),
                accepted: counts[0],
                accepted_newlines: counts[1],
                accepted_trailing_bits: counts[2],
            };
            check_eq(
                &batch,
                &Batch::from_golden(golden_batch)?,
                &format!("batch {n}"),
            )?;
            for (total, count) in totals.iter_mut().zip(counts) {
                *total += count;
            }
        }
        // What the corpus covers: decoded strings, among them ones with line breaks and ones
        // whose last digit has unused bits set (both refused by the crate's URL_SAFE_NO_PAD).
        check_eq(
            &totals,
            &[25_513, 821, 13_128],
            "accepted, with line breaks, with trailing bits",
        )
    }
}
