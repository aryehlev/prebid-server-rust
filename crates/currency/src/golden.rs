//! Test support: reading the goldens written by `testdata/go` and checking without panics.

use std::{collections::BTreeMap, error::Error, fmt::Debug, fs, path::PathBuf, time::Duration};

use tokio::runtime;

use crate::{
    currency_mock::MockCurrencyRatesHttpClient,
    errors::RateError,
    rate_converter::RateConverter,
    rates::{ConversionRates, Rates},
};

/// The URL the tests fetch from. Go's tests use `currency.fake.com`, which `url.Parse` reads
/// as a path and Go's mock client never looks at; the converter here parses the URL first.
pub const FAKE_URL: &str = "http://currency.fake.com/latest.json";

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
    let runtime = runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;
    runtime.block_on(fetch_rates(json))
}

/// [`rates_from_json`] for async tests.
///
/// # Errors
///
/// When the document does not decode.
pub async fn fetch_rates(json: &str) -> Result<Rates, Box<dyn Error>> {
    let client = MockCurrencyRatesHttpClient {
        response_body: json.to_owned(),
    }
    .into_client();
    let converter = RateConverter::new(
        client,
        Duration::from_mins(1),
        FAKE_URL.to_owned(),
        Duration::ZERO,
    );
    converter.run().await?;
    Ok(converter
        .get_info()
        .rates()
        .cloned()
        .ok_or("the fetched rates are missing")?)
}

/// splitmix64, the generator `testdata/go/random.go` shares with these tests so that both
/// build the same random corpora.
#[derive(Debug, Clone, Copy)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Go: `&splitmix64{state: seed}`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Go: `next`.
    pub const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Go: `below(n)`, `next() % n`.
    pub const fn below(&mut self, n: u64) -> u64 {
        match self.next_u64().checked_rem(n) {
            Some(value) => value,
            None => 0,
        }
    }

    /// Go: `pick`.
    pub fn pick(&mut self, options: &[&'static str]) -> &'static str {
        let index = self.below(u64::try_from(options.len()).unwrap_or(u64::MAX));
        usize::try_from(index)
            .ok()
            .and_then(|index| options.get(index))
            .copied()
            .unwrap_or_default()
    }

    /// Go: `digits`.
    pub fn digits(&mut self, count: u64) -> String {
        (0..count)
            .map(|_| char::from(b'0' + u8::try_from(self.below(10)).unwrap_or_default()))
            .collect()
    }
}

/// Go: `randomTableCodes`.
const RANDOM_TABLE_CODES: [&str; 17] = [
    "USD", "GBP", "EUR", "MXN", "JPY", "SEK", "NOK", "CHF", "XXX", "XTS", "XAU", "ADP", "DEM",
    "usd", "gbp", "Eur", "xts",
];

/// Go: `randomQueryCodes`.
const RANDOM_QUERY_CODES: [&str; 26] = [
    "USD", "GBP", "EUR", "MXN", "JPY", "SEK", "NOK", "CHF", "XXX", "XTS", "XAU", "ADP", "DEM",
    "usd", "gbp", "eUR", "xxx", "xTs", "dem", "chf", "VES", "MRU", "FOO", "", "US", "USDX",
];

/// Go: `randomPivotCodes`.
const RANDOM_PIVOT_CODES: [&str; 5] = ["USD", "GBP", "EUR", "SEK", "usd"];

/// Go: `randomPivotQueryCodes`.
const RANDOM_PIVOT_QUERY_CODES: [&str; 6] = ["USD", "GBP", "EUR", "SEK", "usd", "gbp"];

const RANDOM_VALID_CODES: [&str; 8] = ["USD", "GBP", "EUR", "usd", "Mxn", "XXX", "xts", "ADP"];

const RANDOM_INVALID_CODES: [&str; 9] = [
    "VES", "MRU", "FOO", "", "US", "USDX", "U$D", "123", "\u{20ac}",
];

const RANDOM_RATE_VALUES: [&str; 20] = [
    "0",
    "-0",
    "1",
    "-1",
    "0.5",
    "3",
    "7",
    "0.1",
    "0.3",
    "1e-5",
    "5e-324",
    "1e-310",
    "2.2250738585072014e-308",
    "1e308",
    "1.7976931348623157e308",
    "0.77208",
    "1.2952",
    "10.23842",
    "123456789.123456789",
    "0.333333333333333314829616256247",
];

/// Where the keys of a random table come from (Go: the `code` function of `randomTable`).
#[derive(Debug, Clone, Copy)]
pub enum CodeSource {
    /// Go: `pickFrom(s, codes)`.
    From(&'static [&'static str]),
    /// Go: `validationCode(s)`.
    Validation,
}

impl CodeSource {
    fn draw(self, s: &mut SplitMix64) -> &'static str {
        match self {
            Self::From(codes) => s.pick(codes),
            Self::Validation => {
                if s.below(16) == 0 {
                    s.pick(&RANDOM_INVALID_CODES)
                } else {
                    s.pick(&RANDOM_VALID_CODES)
                }
            }
        }
    }
}

/// Go: `randomDecimal`.
fn random_decimal(s: &mut SplitMix64) -> String {
    match s.below(4) {
        0 => s.pick(&RANDOM_RATE_VALUES).to_owned(),
        1 => {
            let lead = 1 + s.below(9);
            let count = s.below(6);
            let rest = s.digits(count);
            format!("{lead}{rest}")
        }
        2 => {
            let whole = s.below(10_000);
            let count = 1 + s.below(17);
            let fraction = s.digits(count);
            format!("{whole}.{fraction}")
        }
        _ => {
            let lead = 1 + s.below(9);
            let count = 1 + s.below(12);
            let fraction = s.digits(count);
            let sign = s.pick(&["", "-"]);
            let exponent = s.below(300);
            format!("{lead}.{fraction}e{sign}{exponent}")
        }
    }
}

/// Go: `randomTable`.
#[must_use]
pub fn random_table(s: &mut SplitMix64, codes: CodeSource) -> String {
    match s.below(16) {
        0 => return "{}".to_owned(),
        1 => return r#"{"conversions":null}"#.to_owned(),
        2 => return r#"{"conversions":{}}"#.to_owned(),
        _ => {}
    }
    let mut entries = Vec::new();
    // Go: `for i := 1 + s.below(5); i > 0; i--`.
    for _ in 0..=s.below(5) {
        let from = codes.draw(s);
        if s.below(10) == 0 {
            entries.push(format!("\"{from}\":null"));
            continue;
        }
        let mut inner = Vec::new();
        for _ in 0..s.below(5) {
            let to = codes.draw(s);
            let rate = random_decimal(s);
            inner.push(format!("\"{to}\":{rate}"));
        }
        entries.push(format!("\"{from}\":{{{}}}", inner.join(",")));
    }
    format!("{{\"conversions\":{{{}}}}}", entries.join(","))
}

/// One case of Go's `writeRandomRates`.
#[derive(Debug, Clone)]
pub struct RandomRateCase {
    /// The line the Go input digest covers.
    pub input: String,
    /// The request (custom) rates file.
    pub custom: String,
    /// The server rates file.
    pub server: String,
    /// `GetRate`'s from-currency.
    pub from: &'static str,
    /// `GetRate`'s to-currency.
    pub to: &'static str,
}

/// One `writeRandomRates` call of the Go generator.
#[derive(Debug, Clone, Copy)]
pub struct RateCorpus {
    /// The prefix of its `random.golden` lines.
    pub name: &'static str,
    /// The generator's seed.
    pub seed: u64,
    /// The codes the tables use as keys.
    pub table_codes: &'static [&'static str],
    /// The codes passed to `GetRate`.
    pub query_codes: &'static [&'static str],
    /// The number of cases.
    pub cases: usize,
}

/// Go: the `writeRandomRates` calls of `writeRandom`.
pub const RATE_CORPORA: [RateCorpus; 2] = [
    RateCorpus {
        name: "mixed",
        seed: 20_240_901,
        table_codes: &RANDOM_TABLE_CODES,
        query_codes: &RANDOM_QUERY_CODES,
        cases: 20 * RANDOM_BATCH,
    },
    RateCorpus {
        name: "pivot",
        seed: 20_240_904,
        table_codes: &RANDOM_PIVOT_CODES,
        query_codes: &RANDOM_PIVOT_QUERY_CODES,
        cases: 10 * RANDOM_BATCH,
    },
];

/// Go: the inputs of `writeRandomRates`.
#[must_use]
pub fn random_rate_cases(corpus: RateCorpus) -> Vec<RandomRateCase> {
    let mut s = SplitMix64::new(corpus.seed);
    (0..corpus.cases)
        .map(|_| {
            let custom = random_table(&mut s, CodeSource::From(corpus.table_codes));
            let server = random_table(&mut s, CodeSource::From(corpus.table_codes));
            let from = s.pick(corpus.query_codes);
            let to = s.pick(corpus.query_codes);
            RandomRateCase {
                input: format!("{custom}\t{server}\t{from}\t{to}"),
                custom,
                server,
                from,
                to,
            }
        })
        .collect()
}

/// Go: `randomBatch`.
const RANDOM_BATCH: usize = 500;

/// Checks one random corpus of `random.golden` (written by `testdata/go/random.go`).
///
/// `cases` holds each case's input line and Rust outcome, in order. The `<kind>-batch`
/// lines hold the digests of the inputs and of Go's outcomes per batch; a case listed on a
/// `<kind>-multi` line has several possible Go outcomes, and the Rust one must be one of them.
///
/// # Errors
///
/// When the inputs differ from Go's (the generators drifted) or an outcome differs.
pub fn check_random_corpus(kind: &str, cases: &[(String, String)]) -> TestResult {
    let batch_kind = format!("{kind}-batch");
    let multi_kind = format!("{kind}-multi");
    let mut batches = Vec::new();
    let mut multi = BTreeMap::new();
    for fields in read_golden("random.golden")? {
        match fields.as_slice() {
            [line_kind, _batch, inputs, outcomes] if *line_kind == batch_kind => {
                batches.push((inputs.clone(), outcomes.clone()));
            }
            [line_kind, index, options] if *line_kind == multi_kind => {
                drop(multi.insert(index.parse::<usize>()?, options.clone()));
            }
            _ => {}
        }
    }
    check(
        !batches.is_empty() && batches.len() * RANDOM_BATCH == cases.len(),
        &format!(
            "{kind}: {} cases for {} batches",
            cases.len(),
            batches.len()
        ),
    )?;
    for (batch, (chunk, (go_inputs, go_outcomes))) in
        cases.chunks(RANDOM_BATCH).zip(&batches).enumerate()
    {
        let mut inputs = Vec::new();
        let mut outcomes = Vec::new();
        for (offset, (input, outcome)) in chunk.iter().enumerate() {
            let index = batch * RANDOM_BATCH + offset;
            inputs.extend_from_slice(input.as_bytes());
            inputs.push(b'\n');
            if let Some(options) = multi.get(&index) {
                check(
                    options.split('|').any(|option| option == outcome),
                    &format!("{kind} case {index} ({input}): got {outcome}, want one of {options}"),
                )?;
                outcomes.extend_from_slice(b"multi\n");
            } else {
                outcomes.extend_from_slice(outcome.as_bytes());
                outcomes.push(b'\n');
            }
        }
        check_eq(
            &fnv64a_hex(&inputs),
            go_inputs,
            &format!("{kind} batch {batch}: the Rust generator drifted from Go's"),
        )?;
        check_eq(
            &fnv64a_hex(&outcomes),
            go_outcomes,
            &format!("{kind} batch {batch}: the outcomes differ from Go's"),
        )?;
    }
    Ok(())
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
