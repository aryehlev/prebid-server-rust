//! Pins of the GPP base64 glue (`base64.leniency` in the differences ledger) against go-gpp
//! v0.2.0's `util.NewBitStreamFromBase64`: the 50,000 seeded strings of
//! `testdata/golden/base64_seeded.txt` and the 960,800 enumerated ones of
//! `testdata/golden/base64_exhaustive.txt` (written by `testdata/go` with go1.26.2), the seeded
//! ones beside what the base64 crate's `URL_SAFE_NO_PAD` does alone with the same padded input.

#[cfg(test)]
mod tests {
    use std::{error::Error, fmt::Debug, fs, path::Path};

    use base64::{DecodeError, Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use pbs_gpp::util::BitStream;
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

    /// `bytes` as lower-case hex, Go's `hex.EncodeToString`.
    fn hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .flat_map(|b| [b >> 4, b & 0x0F])
            .filter_map(|nibble| char::from_digit(u32::from(nibble), 16))
            .collect()
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

    /// seeded.go's `seededAlphabet`, the alphabet of `base64.RawURLEncoding`.
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    /// seeded.go's `seededSpecials`.
    const SPECIALS: [&str; 14] = [
        "\n", "\r", "\r\n", "=", "==", "+", "/", ".", "~", " ", "\t", "\x00", "\x7f", "é",
    ];

    /// seeded.go's `b64Input`.
    fn seeded_input(generator: &mut SplitMix64) -> String {
        let n = match generator.below(8) {
            0 => generator.below(301),
            1 => generator.below(5),
            _ => generator.below(41),
        };
        let noise = [0, 0, 64, 16, 4]
            .get(generator.below(5))
            .copied()
            .unwrap_or_default();
        let mut out = String::new();
        for _ in 0..n {
            if noise > 0 && generator.below(noise) == 0 {
                out.push_str(SPECIALS.get(generator.below(SPECIALS.len())).unwrap_or(&""));
            } else if let Some(&digit) = ALPHABET.get(generator.below(ALPHABET.len())) {
                out.push(char::from(digit));
            }
        }
        out
    }

    /// The input go-gpp hands `RawURLEncoding.Decode`: `padLastQuantum` appends an `A` when the
    /// length, line breaks included, is not a multiple of 4.
    // Go: util/bitstream.go:35-41.
    fn padded(input: &str) -> String {
        if input.len().is_multiple_of(4) {
            input.to_owned()
        } else {
            format!("{input}A")
        }
    }

    /// How the crate's `URL_SAFE_NO_PAD`, alone, answers the padded strings Go decodes or
    /// refuses.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct CrateAlone {
        /// Strings both decode to the same bytes.
        same_bytes: u64,
        /// Strings Go decodes and the crate refuses (all of them hold `\r` or `\n`).
        refused: u64,
        /// Strings both refuse, the crate at Go's offset.
        same_offset: u64,
        /// Strings both refuse, the crate at another offset or with none.
        other_offset: u64,
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

    /// Each of 50,000 seeded strings (base64url with line breaks, `=`, the standard alphabet's
    /// digits, GPP's separators, control bytes and a two-byte character mixed in) decodes through
    /// `BitStream::from_base64` to Go's bytes or fails with Go's text, batch by batch.
    // Go: testdata/go/seeded.go (seededGolden: util.NewBitStreamFromBase64,
    // util/bitstream.go:20-30, over RawURLEncoding.Decode).
    #[test]
    fn go_seeded() -> TestResult {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/golden/base64_seeded.txt");
        let golden = fs::read_to_string(&path)?;
        let mut lines = golden.lines();
        check_eq(
            &lines.next(),
            &Some("seed\t0x4750504236343031\tcount\t50000\tbatch\t1000"),
            "header",
        )?;
        let mut generator = SplitMix64(0x4750_5042_3634_3031);
        let mut alone = CrateAlone::default();
        let mut totals = [0_u64; 2];
        let mut batches = 0_usize;
        for (n, line) in lines.enumerate() {
            let mut inputs = Sha256::new();
            let mut results = Sha256::new();
            let mut counts = [0_u64; 2];
            for _ in 0..1000 {
                let input = seeded_input(&mut generator);
                inputs.update(format!("{}\n", hex(input.as_bytes())));
                match BitStream::from_base64(&input) {
                    Ok(stream) => {
                        results.update(format!("={}\n", hex(stream.bytes())));
                        let line_breaks = input.contains(['\r', '\n']);
                        counts[0] += 1;
                        counts[1] += u64::from(line_breaks);
                        match URL_SAFE_NO_PAD.decode(padded(&input)) {
                            Ok(bytes) => {
                                check_eq(&bytes.as_slice(), &stream.bytes(), &input)?;
                                alone.same_bytes += 1;
                            }
                            Err(_) if line_breaks => alone.refused += 1,
                            Err(err) => return Err(format!("{input:?}: the crate {err}").into()),
                        }
                    }
                    Err(err) => {
                        results.update(format!("!{err}\n"));
                        let go_offset = err
                            .message()
                            .strip_prefix("illegal base64 data at input byte ")
                            .map(str::parse::<usize>)
                            .transpose()?;
                        match URL_SAFE_NO_PAD.decode(padded(&input)) {
                            Ok(_) => return Err(format!("{input:?}: the crate decodes").into()),
                            Err(crate_err) if crate_offset(&crate_err) == go_offset => {
                                alone.same_offset += 1;
                            }
                            Err(_) => alone.other_offset += 1,
                        }
                    }
                }
            }
            let got = format!(
                "{n}\t{}\t{}\t{}\t{}",
                hex(&inputs.finalize()),
                hex(&results.finalize()),
                counts[0],
                counts[1]
            );
            check_eq(&got.as_str(), &line, &format!("batch {n}"))?;
            for (total, count) in totals.iter_mut().zip(counts) {
                *total += count;
            }
            batches += 1;
        }
        check_eq(&batches, &50, "batches")?;
        check_eq(&totals, &[33_274, 997], "accepted, with line breaks")?;
        check_eq(
            &alone,
            &CrateAlone {
                same_bytes: 32_277,
                refused: 997,
                same_offset: 13_807,
                other_offset: 2_919,
            },
            "the crate alone beside Go",
        )
    }

    /// exhaustive.go's `exhaustiveSymbols`: a digit whose low bits are zero, one whose low bits
    /// are all set, the two line breaks, `=`, GPP's segment separator and a two-byte character.
    const EXHAUSTIVE_SYMBOLS: [&str; 7] = ["A", "_", "\n", "\r", "=", ".", "é"];

    /// Moves `digits` to the next number in base `base`, the last digit the least significant, as
    /// exhaustive.go counts; false once `digits` held the greatest.
    fn count_up(digits: &mut [usize], base: usize) -> bool {
        let Some(position) = digits.iter().rposition(|&digit| digit + 1 < base) else {
            return false;
        };
        for (index, digit) in digits.iter_mut().enumerate().skip(position) {
            *digit = if index == position { *digit + 1 } else { 0 };
        }
        true
    }

    /// Every string of up to 7 symbols of [`EXHAUSTIVE_SYMBOLS`] (960,800 strings: each
    /// arrangement of line breaks, padding, unused bits and illegal bytes over a quantum, a lone
    /// digit and two more symbols, which go-gpp's padding counts in bytes) decodes through
    /// `BitStream::from_base64` to Go's bytes or fails with Go's text, length by length.
    // Go: testdata/go/exhaustive.go (exhaustiveGolden: util.NewBitStreamFromBase64,
    // util/bitstream.go:20-41, over RawURLEncoding.Decode).
    #[test]
    fn go_exhaustive() -> TestResult {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/golden/base64_exhaustive.txt");
        let golden = fs::read_to_string(&path)?;
        let mut lines = golden.lines();
        check_eq(&lines.next(), &Some("max\t7\tsymbols\t7"), "header")?;
        let mut strings = 0_u64;
        let mut lengths = 0_usize;
        for (len, line) in lines.enumerate() {
            let mut inputs = Sha256::new();
            let mut results = Sha256::new();
            let (mut count, mut accepted) = (0_u64, 0_u64);
            let mut digits = vec![0_usize; len];
            loop {
                let input: String = digits
                    .iter()
                    .filter_map(|&digit| EXHAUSTIVE_SYMBOLS.get(digit).copied())
                    .collect();
                inputs.update(format!("{}\n", hex(input.as_bytes())));
                count += 1;
                match BitStream::from_base64(&input) {
                    Ok(stream) => {
                        results.update(format!("={}\n", hex(stream.bytes())));
                        accepted += 1;
                    }
                    Err(err) => results.update(format!("!{err}\n")),
                }
                if !count_up(&mut digits, EXHAUSTIVE_SYMBOLS.len()) {
                    break;
                }
            }
            let got = format!(
                "{len}\t{count}\t{accepted}\t{}\t{}",
                hex(&inputs.finalize()),
                hex(&results.finalize())
            );
            check_eq(&got.as_str(), &line, &format!("length {len}"))?;
            strings += count;
            lengths += 1;
        }
        check_eq(&lengths, &8, "lengths")?;
        check_eq(&strings, &960_800, "strings")
    }
}
