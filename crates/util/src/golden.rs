//! Test support: the goldens written by `testdata/go/golden` and `testdata/go/tickergolden`,
//! and checks that return errors instead of panicking.
//!
//! Each golden line is a tab-separated list of `strconv.Quote`d fields, so a field may carry
//! any bytes. Sweeps are FNV-1a 64 checksums over newline-terminated outputs, computed over
//! inputs drawn from the same splitmix64 generator as the Go program.

use std::{
    error::Error,
    fmt::{Debug, Display},
    fs,
    path::PathBuf,
    str,
};

/// The result type of every test in this crate.
pub type TestResult = Result<(), Box<dyn Error>>;

/// The fields of one golden line, as bytes.
pub type Row = Vec<Vec<u8>>;

/// Reads `testdata/golden/<name>` into rows of unquoted fields.
///
/// # Errors
///
/// When the file cannot be read or a field is not a Go string literal.
pub fn read_golden(name: &str) -> Result<Vec<Row>, Box<dyn Error>> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "testdata", "golden", name]
        .iter()
        .collect();
    let text = fs::read_to_string(&path)?;
    let mut rows = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let mut row = Vec::new();
        for field in line.split('\t') {
            let value =
                unquote_bytes(field).map_err(|e| format!("{name}:{}: {e}: {field}", n + 1))?;
            row.push(value);
        }
        rows.push(row);
    }
    if rows.is_empty() {
        return Err(format!("{name} has no rows").into());
    }
    Ok(rows)
}

/// The first `N` fields of `row`.
///
/// # Errors
///
/// When the row is shorter.
pub fn fields<const N: usize>(row: &[Vec<u8>]) -> Result<[&[u8]; N], Box<dyn Error>> {
    let got: Vec<&[u8]> = row.iter().take(N).map(Vec::as_slice).collect();
    got.try_into().map_err(|short: Vec<&[u8]>| {
        format!("row has {} of {N} fields: {row:?}", short.len()).into()
    })
}

/// A field that must be UTF-8.
///
/// # Errors
///
/// When it is not.
pub fn text(field: &[u8]) -> Result<&str, Box<dyn Error>> {
    Ok(str::from_utf8(field)?)
}

/// Lowercase hex without punctuation (Go: `hex.EncodeToString`).
#[must_use]
pub fn hex(b: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        for nibble in [x >> 4, x & 0x0F] {
            s.push(char::from(
                DIGITS.get(usize::from(nibble)).copied().unwrap_or(b'0'),
            ));
        }
    }
    s
}

/// Go: `strconv.Unquote` of a double-quoted literal, as bytes: the golden programs write every
/// field with `strconv.Quote`, whose `\x` escapes carry bytes that are not UTF-8.
///
/// # Errors
///
/// When `field` is not such a literal.
pub fn unquote_bytes(field: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let body = field
        .strip_prefix('"')
        .and_then(|f| f.strip_suffix('"'))
        .ok_or("not a quoted string")?;
    let mut out = Vec::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            if c == '"' {
                return Err("unescaped quote".into());
            }
            let mut buf = [0_u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        let e = chars.next().ok_or("trailing backslash")?;
        let simple = match e {
            'a' => Some(0x07),
            'b' => Some(0x08),
            'f' => Some(0x0C),
            'n' => Some(b'\n'),
            'r' => Some(b'\r'),
            't' => Some(b'\t'),
            'v' => Some(0x0B),
            '\\' => Some(b'\\'),
            '"' => Some(b'"'),
            _ => None,
        };
        if let Some(b) = simple {
            out.push(b);
            continue;
        }
        let mut digits = |n: usize, radix: u32| -> Result<u32, Box<dyn Error>> {
            let text: String = chars.by_ref().take(n).collect();
            if text.chars().count() != n {
                return Err("short escape".into());
            }
            Ok(u32::from_str_radix(&text, radix)?)
        };
        match e {
            'x' => out.push(u8::try_from(digits(2, 16)?)?),
            'u' | 'U' => {
                let rune = char::from_u32(digits(if e == 'u' { 4 } else { 8 }, 16)?)
                    .ok_or("invalid rune")?;
                let mut buf = [0_u8; 4];
                out.extend_from_slice(rune.encode_utf8(&mut buf).as_bytes());
            }
            '0'..='7' => {
                let rest: String = chars.by_ref().take(2).collect();
                out.push(u8::try_from(u32::from_str_radix(
                    &format!("{e}{rest}"),
                    8,
                )?)?);
            }
            _ => return Err(format!("unknown escape \\{e}").into()),
        }
    }
    Ok(out)
}

/// Go: `errText`: `""` for success, otherwise `"error: "` and the error text.
pub fn err_text<T, E: Display>(r: &Result<T, E>) -> String {
    r.as_ref()
        .err()
        .map_or_else(String::new, |e| format!("error: {e}"))
}

/// Compares `got` with `want`.
///
/// # Errors
///
/// A message naming `what` and both values when they differ.
pub fn check_eq<T: PartialEq + Debug + ?Sized>(got: &T, want: &T, what: &str) -> TestResult {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}").into())
    }
}

/// Checks a condition.
///
/// # Errors
///
/// `what` when the condition is false.
pub fn check(ok: bool, what: &str) -> TestResult {
    if ok { Ok(()) } else { Err(what.into()) }
}

/// The splitmix64 generator of the Go golden program.
#[derive(Debug, Clone, Copy)]
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    /// Go: `next`.
    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Go: `intn`: `next() % n`.
    pub fn intn(&mut self, n: usize) -> usize {
        let n = u64::try_from(n).unwrap_or(u64::MAX).max(1);
        usize::try_from(self.next_u64() % n).unwrap_or(0)
    }

    /// Go: `sweepString`: up to `max_len` bytes drawn from `alphabet`.
    pub fn sweep_string(&mut self, alphabet: &[u8], max_len: usize) -> Vec<u8> {
        let n = self.intn(max_len + 1);
        (0..n)
            .map(|_| {
                let i = self.intn(alphabet.len());
                alphabet.get(i).copied().unwrap_or(0)
            })
            .collect()
    }
}

/// FNV-1a 64 over newline-terminated outputs (Go: `sweep`).
#[derive(Debug, Clone, Copy)]
pub struct Sweep(u64);

impl Default for Sweep {
    fn default() -> Self {
        Self(0xCBF2_9CE4_8422_2325)
    }
}

impl Sweep {
    /// Adds one output.
    pub fn add(&mut self, v: &[u8]) {
        for &b in v.iter().chain(b"\n") {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    /// The checksum as 16 hex digits.
    #[must_use]
    pub fn sum(self) -> String {
        format!("{:016x}", self.0)
    }
}
