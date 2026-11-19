//! The library pins of the port.
//!
//! The Rust server builds on crates where the Go server builds on Go libraries: `serde_json` for
//! `encoding/json` and jsoniter, `serde-saphyr` for yaml.v3, `config` for Viper, `reqwest` for the
//! `net/http` client, `axum` and `tower-http` for the server and `rs/cors`, `prometheus` for
//! `client_golang`, `moka` for freecache, `regex` for `regexp`, `url` for `net/url`, `cookie` for
//! `http.Cookie`, `time` for Go's time layouts, `flate2` for `compress/gzip`, `tracing` for glog
//! and `psl` for `x/net/publicsuffix`. Each file under `tests/` pins one library: next to a
//! golden of what the Go library does with an input (`testdata/golden`, written by the Go
//! program in `testdata/go` from the real code at the pinned upstream commit), it shows what the
//! crate does with the same input. A `go_difference_*` test pins an observable difference, which
//! the differences ledger records (ledgered as accepted, or restored by the unit that keeps Go's
//! behaviour); a `same_as_go_*` test pins a case where the crate answers as Go does.
//!
//! This library is the test support the pins share: reading a golden, checks that fail a test by
//! returning an error instead of panicking, the YAML options the pins parse with, and a runtime
//! for the pins that talk over loopback.

use std::{error::Error, fmt::Debug, fs, future::Future, path::Path};

use serde_json::Value;
use serde_saphyr::{DuplicateKeyPolicy, Options};
use tokio::runtime;

/// The error of a failed pin.
pub type BoxError = Box<dyn Error + Send + Sync>;

/// What every pin returns.
pub type TestResult = Result<(), BoxError>;

/// The directory the Go program writes the goldens to.
pub const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden");

/// The generator every golden's header names.
pub const GENERATOR: &str = "crates/differences/testdata/go";

/// The architecture the goldens are generated on (D-ARCH: amd64, under Rosetta on arm64 hosts).
pub const GOLDEN_ARCH: &str = "amd64";

/// Reads `testdata/golden/<name>` and checks its header: written by [`GENERATOR`] on
/// [`GOLDEN_ARCH`].
///
/// # Errors
///
/// When the file cannot be read, is not JSON, or its header names another generator or
/// architecture.
pub fn golden(name: &str) -> Result<Value, BoxError> {
    let path = Path::new(GOLDEN_DIR).join(name);
    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: Value = serde_json::from_slice(&bytes)?;
    check_eq(
        text(&doc, "/header/generator")?,
        GENERATOR,
        "golden generator",
    )?;
    check_eq(
        text(&doc, "/header/goarch")?,
        GOLDEN_ARCH,
        "golden architecture",
    )?;
    Ok(doc)
}

/// The value at `pointer` (RFC 6901) in `doc`.
///
/// # Errors
///
/// When `doc` has no value there.
pub fn at<'v>(doc: &'v Value, pointer: &str) -> Result<&'v Value, BoxError> {
    doc.pointer(pointer)
        .ok_or_else(|| format!("the golden has no {pointer}").into())
}

/// The string at `pointer` in `doc`.
///
/// # Errors
///
/// When `doc` has no string there.
pub fn text<'v>(doc: &'v Value, pointer: &str) -> Result<&'v str, BoxError> {
    at(doc, pointer)?
        .as_str()
        .ok_or_else(|| format!("{pointer} is not a string").into())
}

/// The array at `pointer` in `doc`.
///
/// # Errors
///
/// When `doc` has no array there.
pub fn list<'v>(doc: &'v Value, pointer: &str) -> Result<&'v [Value], BoxError> {
    at(doc, pointer)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{pointer} is not an array").into())
}

/// The unsigned integer at `pointer` in `doc`.
///
/// # Errors
///
/// When `doc` has no unsigned integer there.
pub fn number(doc: &Value, pointer: &str) -> Result<u64, BoxError> {
    at(doc, pointer)?
        .as_u64()
        .ok_or_else(|| format!("{pointer} is not an unsigned integer").into())
}

/// The bytes a hex string spells.
///
/// # Errors
///
/// When `hex` has an odd length or a character that is not a hex digit.
pub fn unhex(hex: &str) -> Result<Vec<u8>, BoxError> {
    let digits: Vec<u32> = hex
        .chars()
        .map(|c| {
            c.to_digit(16)
                .ok_or_else(|| format!("{c:?} is not a hex digit"))
        })
        .collect::<Result<_, _>>()?;
    digits
        .chunks(2)
        .map(|pair| match *pair {
            [high, low] => u8::try_from(high * 16 + low).map_err(BoxError::from),
            _ => Err(format!("{hex:?} has an odd length").into()),
        })
        .collect()
}

/// `bytes` as lower-case hex.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|b| [b >> 4, b & 0x0F])
        .filter_map(|nibble| char::from_digit(u32::from(nibble), 16))
        .collect()
}

/// Fails the pin with `message` unless `condition` holds.
///
/// # Errors
///
/// `message`, when `condition` is false.
pub fn check(condition: bool, message: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

/// Fails the pin unless `actual == expected`.
///
/// # Errors
///
/// A message with both values, when they differ.
pub fn check_eq<T: PartialEq + Debug + ?Sized>(actual: &T, expected: &T, what: &str) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{what}:\n  got:  {actual:?}\n  want: {expected:?}").into())
    }
}

/// The `serde-saphyr` options closest to yaml.v3, as the stack spike settled them: duplicate
/// keys are errors, `0777` is octal, and the YAML 1.1 booleans (`yes`, `on`, ...) decode into
/// `bool` fields.
#[must_use]
pub fn yaml_options() -> Options {
    serde_saphyr::options! {
        duplicate_keys: DuplicateKeyPolicy::Error,
        legacy_octal_numbers: true,
        strict_booleans: false,
    }
}

/// Runs `pin` to its result on a fresh multi-threaded runtime with two workers.
///
/// # Errors
///
/// The pin's error, or the runtime's when it cannot start.
pub fn run<T, F: Future<Output = Result<T, BoxError>>>(pin: F) -> Result<T, BoxError> {
    runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(pin)
}

/// Fails the pin unless the rows of `actual` are `expected`: a table whose columns put what Go
/// does next to what the crate does.
///
/// # Errors
///
/// A message with both tables, when they differ.
pub fn check_table(actual: &[Vec<String>], expected: &[&[&str]], what: &str) -> TestResult {
    let want: Vec<Vec<String>> = expected
        .iter()
        .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
        .collect();
    check_eq(actual, want.as_slice(), what)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{TestResult, check, check_eq, check_table, golden, hex, list, number, text, unhex};

    #[test]
    fn hex_decodes() -> TestResult {
        check_eq(
            unhex("1f8bFF00")?.as_slice(),
            &[0x1F, 0x8B, 0xFF, 0x00][..],
            "bytes",
        )?;
        check(unhex("abc").is_err(), "odd length")?;
        check(unhex("zz").is_err(), "not hex")?;
        check_eq(hex(&[0x00, 0xAB, 0x7F]).as_str(), "00ab7f", "hex")
    }

    #[test]
    fn tables() -> TestResult {
        let rows = vec![vec!["a".to_owned(), "b".to_owned()]];
        check_table(&rows, &[&["a", "b"]], "equal")?;
        check(
            check_table(&rows, &[&["a", "c"]], "unequal").is_err(),
            "a different cell",
        )
    }

    #[test]
    fn pointers() -> TestResult {
        let doc = json!({"a": {"b": "c", "n": 7, "l": [1, 2]}});
        check_eq(text(&doc, "/a/b")?, "c", "string")?;
        check_eq(&number(&doc, "/a/n")?, &7, "number")?;
        check_eq(&list(&doc, "/a/l")?.len(), &2, "array")?;
        check(text(&doc, "/a/n").is_err(), "a number is not a string")?;
        check(list(&doc, "/missing").is_err(), "missing")
    }

    #[test]
    fn every_golden_has_the_header() -> TestResult {
        for name in [
            "cache.json",
            "config.json",
            "cookie.json",
            "float_parse.json",
            "gzip.json",
            "http_client.json",
            "json.json",
            "logging.json",
            "metrics.json",
            "psl.json",
            "regex.json",
            "server.json",
            "time.json",
            "url.json",
            "yaml.json",
        ] {
            let doc = golden(name)?;
            check(
                text(&doc, "/header/go_version")?.starts_with("go1."),
                "go_version",
            )?;
            check(
                text(&doc, "/header/upstream")?.starts_with("v4."),
                "the upstream module's version",
            )?;
        }
        Ok(())
    }
}
