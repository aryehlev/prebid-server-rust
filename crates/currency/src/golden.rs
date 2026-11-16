//! Test support: reading the goldens written by `testdata/go` and checking without panics.

use std::{
    collections::BTreeMap, error::Error, fmt::Debug, fs, path::PathBuf, sync::Arc, time::Duration,
};

use tokio::runtime;

use crate::{
    currency_mock::MockCurrencyRatesHttpClient,
    errors::RateError,
    rate_converter::RateConverter,
    rates::{ConversionRates, Rates},
};

/// The result type of every test in this crate.
pub type TestResult = Result<(), Box<dyn Error>>;

/// Builds a [`ConversionRates`] from literal entries.
#[must_use]
pub fn conversion_rates(entries: &[(&str, &[(&str, f64)])]) -> ConversionRates {
    entries
        .iter()
        .map(|(from, rates)| {
            let inner: BTreeMap<String, f64> = rates
                .iter()
                .map(|(to, rate)| ((*to).to_owned(), *rate))
                .collect();
            ((*from).to_owned(), Some(inner))
        })
        .collect()
}

/// Renders a `GetRate` outcome like the Go golden generator.
#[must_use]
pub fn render_rate(result: Result<f64, RateError>) -> String {
    match result {
        Ok(rate) => format!("ok:{:016x}", rate.to_bits()),
        Err(err) => format!("err:{err}"),
    }
}

/// Decodes a currency file by fetching it with a [`RateConverter`], as the server does.
///
/// Only for synchronous tests: it runs its own tokio runtime.
///
/// # Errors
///
/// When the runtime cannot start or the document does not decode.
pub fn rates_from_json(json: &str) -> Result<Rates, Box<dyn Error>> {
    let client = Arc::new(MockCurrencyRatesHttpClient {
        response_body: json.to_owned(),
    });
    let converter = RateConverter::new(
        client,
        Duration::from_mins(1),
        String::new(),
        Duration::ZERO,
    );
    let runtime = runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;
    runtime.block_on(converter.run())?;
    Ok(converter
        .get_info()
        .rates()
        .cloned()
        .ok_or("the fetched rates are missing")?)
}

/// Checks `actual` against a golden expectation, which may list several Go outcomes as
/// `any:<outcome>|<outcome>...` where Go's map iteration order picks one at random.
///
/// # Errors
///
/// When `actual` is not (one of) the expected outcome(s).
pub fn check_outcome(actual: &str, expected: &str, context: &str) -> TestResult {
    expected.strip_prefix("any:").map_or_else(
        || check_eq(actual, expected, context),
        |options| {
            check(
                options.split('|').any(|option| option == actual),
                &format!("{context}: got {actual}, want one of {options}"),
            )
        },
    )
}

/// Reads `testdata/golden/<name>` as tab-separated fields per line.
///
/// # Errors
///
/// When the file cannot be read.
pub fn read_golden(name: &str) -> Result<Vec<Vec<String>>, Box<dyn Error>> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "testdata", "golden", name]
        .iter()
        .collect();
    let text = fs::read_to_string(&path)?;
    Ok(text
        .lines()
        .map(|line| line.split('\t').map(str::to_owned).collect())
        .collect())
}

/// Decodes lower-case hex as written by Go's `hex.EncodeToString`.
///
/// # Errors
///
/// When `hex` is not hex.
pub fn hex_decode(hex: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let digits = hex.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return Err(format!("odd hex length in {hex:?}").into());
    }
    digits
        .chunks(2)
        .map(|pair| {
            let text = str::from_utf8(pair)?;
            Ok(u8::from_str_radix(text, 16)?)
        })
        .collect()
}

/// Encodes bytes as lower-case hex, like Go's `hex.EncodeToString`.
#[must_use]
pub fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            out.push(char::from(
                HEX_DIGITS
                    .get(usize::from(byte >> 4))
                    .copied()
                    .unwrap_or(b'0'),
            ));
            out.push(char::from(
                HEX_DIGITS
                    .get(usize::from(byte & 0x0F))
                    .copied()
                    .unwrap_or(b'0'),
            ));
            out
        })
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// 64-bit FNV-1a, like Go's `hash/fnv.New64a`, rendered as `%016x`.
#[must_use]
pub fn fnv64a_hex(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xCBF2_9CE4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3)
    });
    format!("{hash:016x}")
}

/// Fails the test with `message` unless `condition` holds.
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

/// Fails the test unless `actual == expected`.
///
/// # Errors
///
/// A message with both values, when they differ.
pub fn check_eq<T: PartialEq + Debug + ?Sized>(
    actual: &T,
    expected: &T,
    context: &str,
) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{context}: got {actual:?}, want {expected:?}").into())
    }
}
