//! Port of Go `currency/rate_converter.go`.

use std::{
    error::Error,
    fmt::{self, Write as _},
    future::{self, Future},
    pin::Pin,
    sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::{Duration, SystemTime},
};

use tokio::time::{Instant, timeout_at};

use self::rates_json::unmarshal_valid;
use crate::{
    constant_rates::ConstantRates, converter_info::ConverterInfo, errors::RateError, rates::Rates,
};

mod rates_json;

/// Conversion rates between currencies (Go: `currency.Conversions`).
pub trait Conversions: Send + Sync + fmt::Debug {
    /// The rate to multiply an amount in `from` by to get the amount in `to`.
    ///
    /// # Errors
    ///
    /// A [`RateError`] when a code is not a valid ISO 4217 code or no rate is known.
    fn get_rate(&self, from: &str, to: &str) -> Result<f64, RateError>;

    /// The rates table behind these conversions, if there is one (Go: `GetRates`, `None` is a
    /// nil pointer).
    fn get_rates(&self) -> Option<&Rates>;
}

/// An error from an [`HttpClient`].
pub type BoxError = Box<dyn Error + Send + Sync>;

/// The rest of a response body, read to the end.
pub type BodyFuture = Pin<Box<dyn Future<Output = Result<Vec<u8>, BoxError>> + Send>>;

/// A pending response (status line and headers) from [`HttpClient::get`].
pub type ResponseFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HttpResponse, BoxError>> + Send + 'a>>;

/// The HTTP client the converter fetches the currency file with (Go: the `httpClient`
/// interface, `Do(req *http.Request) (*http.Response, error)`).
///
/// The converter applies the fetch timeout to both futures, like Go's request context does,
/// so an implementation needs no timeout of its own. Go returns the `*url.Error` of
/// `http.Client.Do` unchanged (for example `Get "invalidurl": unsupported protocol scheme ""`,
/// with the URL as `url.URL.String()` prints it and any password shown as `***`), and a URL
/// that does not parse fails earlier, in `http.NewRequestWithContext`, as
/// `parse "http://[::1": missing ']' in host`. An implementation that should match Go's log
/// lines formats its errors the same way; checks that Go makes before waiting (scheme, host)
/// should fail without waiting, because Go reports them even when the timeout is zero.
pub trait HttpClient: Send + Sync + fmt::Debug {
    /// Sends `GET url` and resolves once the status line and headers have arrived.
    fn get<'a>(&'a self, url: &'a str) -> ResponseFuture<'a>;
}

/// A response from an [`HttpClient`] (Go: `*http.Response`).
pub struct HttpResponse {
    /// Go: `StatusCode`.
    pub status_code: u16,
    /// Go: `Body`, read with `io.ReadAll`.
    pub body: BodyFuture,
}

impl HttpResponse {
    /// A response whose body arrives when `body` resolves.
    #[must_use]
    pub fn new<F>(status_code: u16, body: F) -> Self
    where
        F: Future<Output = Result<Vec<u8>, BoxError>> + Send + 'static,
    {
        Self {
            status_code,
            body: Box::pin(body),
        }
    }

    /// A response whose body is already complete.
    #[must_use]
    pub fn from_bytes(status_code: u16, body: Vec<u8>) -> Self {
        Self::new(status_code, future::ready(Ok(body)))
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status_code", &self.status_code)
            .finish_non_exhaustive()
    }
}

/// Why fetching the currency file failed (Go: the errors of `RateConverter.fetch`).
#[derive(Debug)]
pub enum FetchError {
    /// Go: the error of `httpClient.Do`, returned unwrapped.
    Request(BoxError),
    /// Go: `httpClient.Do` failing on the expired request context, whose `*url.Error` reads
    /// `Get "<url>": context deadline exceeded`.
    Timeout {
        /// The fetched URL.
        url: String,
    },
    /// Go: `errortypes.BadServerResponse` (error code 4) for a status code of 400 or more.
    BadServerResponse {
        /// The response status code.
        status_code: u16,
    },
    /// Go: `fmt.Errorf("the currency rates request failed: %v", err)` when reading the body
    /// fails, including on the fetch deadline.
    ReadBody(BoxError),
    /// Go: `fmt.Errorf("the currency rates request failed to parse json: %v", err)`; the message
    /// is the raw bytes of Go's `jsonutil` error, which can quote any input bytes.
    ParseJson(Vec<u8>),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(err) => err.fmt(f),
            Self::Timeout { url } => {
                f.write_str("Get ")?;
                write_go_quoted(f, url)?;
                f.write_str(": context deadline exceeded")
            }
            Self::BadServerResponse { status_code } => {
                write!(
                    f,
                    "the currency rates request failed with status code {status_code}"
                )
            }
            Self::ReadBody(err) => write!(f, "the currency rates request failed: {err}"),
            Self::ParseJson(message) => write!(
                f,
                "the currency rates request failed to parse json: {}",
                String::from_utf8_lossy(message)
            ),
        }
    }
}

impl Error for FetchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Request(err) | Self::ReadBody(err) => Some(err.as_ref()),
            Self::Timeout { .. } | Self::BadServerResponse { .. } | Self::ParseJson(_) => None,
        }
    }
}

/// Go: `context.DeadlineExceeded`, the body read error once the fetch timeout expires.
#[derive(Debug, Clone, Copy)]
struct DeadlineExceeded;

impl fmt::Display for DeadlineExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("context deadline exceeded")
    }
}

impl Error for DeadlineExceeded {}

/// Go's `%q` for a string (`strconv.Quote`).
fn write_go_quoted(f: &mut fmt::Formatter<'_>, text: &str) -> fmt::Result {
    f.write_char('"')?;
    for ch in text.chars() {
        match ch {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\u{7}' => f.write_str("\\a")?,
            '\u{8}' => f.write_str("\\b")?,
            '\u{c}' => f.write_str("\\f")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            '\u{b}' => f.write_str("\\v")?,
            _ if u32::from(ch) < 0x20 || ch == '\u{7f}' => write!(f, "\\x{:02x}", u32::from(ch))?,
            _ if ch.is_control() => write!(f, "\\u{:04x}", u32::from(ch))?,
            _ => f.write_char(ch)?,
        }
    }
    f.write_char('"')
}

/// Go: `timeutil.Time`, replaced by a fake clock in tests.
trait Clock: Send + Sync + fmt::Debug {
    fn now(&self) -> SystemTime;
}

/// Go: `timeutil.RealTime`.
#[derive(Debug, Clone, Copy)]
struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Fetches the currency file and serves its rates (Go: `currency.RateConverter`).
///
/// Until the first successful fetch, and after a failed fetch once the fetched rates are older
/// than the stale threshold, [`rates`](Self::rates) serves [`ConstantRates`].
#[derive(Debug)]
pub struct RateConverter {
    http_client: Arc<dyn HttpClient>,
    http_timeout: Duration,
    stale_rates_threshold: Duration,
    sync_source_url: String,
    rates: RwLock<Option<Arc<Rates>>>,
    last_updated: RwLock<Option<SystemTime>>,
    constant_rates: Arc<ConstantRates>,
    clock: Arc<dyn Clock>,
}

impl RateConverter {
    /// Go: `NewRateConverter(httpClient, httpTimeout, syncSourceURL, staleRatesThreshold)`.
    ///
    /// Go's durations are signed: a non-positive `httpTimeout` makes every real request fail
    /// with the expired deadline and a non-positive `staleRatesThreshold` means the rates never
    /// go stale. Callers pass [`Duration::ZERO`] for both. Go's server builds them from
    /// `currency_converter.fetch_timeout_ms` and `currency_converter.stale_rates_seconds`.
    #[must_use]
    pub fn new(
        http_client: Arc<dyn HttpClient>,
        http_timeout: Duration,
        sync_source_url: String,
        stale_rates_threshold: Duration,
    ) -> Self {
        Self {
            http_client,
            http_timeout,
            stale_rates_threshold,
            sync_source_url,
            rates: RwLock::new(None),
            last_updated: RwLock::new(None),
            constant_rates: Arc::new(ConstantRates::new()),
            clock: Arc::new(RealClock),
        }
    }

    /// Fetches and decodes the currency file (Go: `fetch`).
    async fn fetch(&self) -> Result<Rates, FetchError> {
        // Go: context.WithTimeout covers the request and the body read.
        let deadline = Instant::now().checked_add(self.http_timeout);
        let response =
            match with_deadline(deadline, self.http_client.get(&self.sync_source_url)).await {
                Some(Ok(response)) => response,
                Some(Err(err)) => return Err(FetchError::Request(err)),
                None => {
                    return Err(FetchError::Timeout {
                        url: self.sync_source_url.clone(),
                    });
                }
            };
        // Go reads the body once: with io.ReadAll below a 400 status, and in the deferred drain
        // (which logs its error before the caller sees the result) for every status.
        let body = with_deadline(deadline, response.body)
            .await
            .unwrap_or_else(|| Err(Box::new(DeadlineExceeded)));
        if let Err(err) = &body {
            log::error!("error draining conversion rates response body: {err}");
        }
        if response.status_code >= 400 {
            return Err(FetchError::BadServerResponse {
                status_code: response.status_code,
            });
        }
        let bytes = body.map_err(FetchError::ReadBody)?;
        unmarshal_valid(&bytes).map_err(FetchError::ParseJson)
    }

    /// Go: `update`.
    async fn update(&self) -> Result<(), FetchError> {
        match self.fetch().await {
            Ok(rates) => {
                *write(&self.rates) = Some(Arc::new(rates));
                *write(&self.last_updated) = Some(self.clock.now());
                Ok(())
            }
            Err(err) => {
                if self.check_stale_rates() {
                    self.clear_rates();
                    log::error!(
                        "Error updating conversion rates, falling back to constant rates: {err}"
                    );
                } else {
                    log::error!("Error updating conversion rates: {err}");
                }
                Err(err)
            }
        }
    }

    /// Fetches the currency file once and stores its rates (Go: `Run`).
    ///
    /// # Errors
    ///
    /// The [`FetchError`], which has already been logged; the previous rates stay in use unless
    /// they are stale.
    pub async fn run(&self) -> Result<(), FetchError> {
        self.update().await
    }

    /// When the rates were last fetched successfully (Go: `LastUpdated`; `None` is Go's zero
    /// `time.Time`).
    #[must_use]
    pub fn last_updated(&self) -> Option<SystemTime> {
        *read(&self.last_updated)
    }

    /// The current rates: the fetched ones, or [`ConstantRates`] (Go: `Rates`).
    #[must_use]
    pub fn rates(&self) -> Arc<dyn Conversions> {
        match self.fetched_rates() {
            Some(rates) => rates,
            None => Arc::<ConstantRates>::clone(&self.constant_rates),
        }
    }

    fn fetched_rates(&self) -> Option<Arc<Rates>> {
        read(&self.rates).as_ref().map(Arc::clone)
    }

    /// Go: `clearRates`.
    fn clear_rates(&self) {
        *write(&self.rates) = None;
    }

    /// Go: `checkStaleRates`.
    fn check_stale_rates(&self) -> bool {
        if self.stale_rates_threshold.is_zero() {
            return false;
        }
        let current_time = self.clock.now();
        let Some(last_updated) = self.last_updated() else {
            return false;
        };
        // Go: delta.Seconds() > rc.staleRatesThreshold.Seconds(), compared as float64.
        let delta = go_sub(current_time, last_updated);
        go_seconds(delta) > go_seconds(go_duration(self.stale_rates_threshold))
    }

    /// The converter's source, last update and rates (Go: `GetInfo`).
    #[must_use]
    pub fn get_info(&self) -> ConverterInfo {
        let rates = self.fetched_rates();
        ConverterInfo::new(self.sync_source_url.clone(), self.last_updated(), rates)
    }
}

/// Awaits `future` until `deadline` (`None` is a deadline too far away to represent).
async fn with_deadline<F: Future>(deadline: Option<Instant>, future: F) -> Option<F::Output> {
    match deadline {
        Some(deadline) => timeout_at(deadline, future).await.ok(),
        None => Some(future.await),
    }
}

fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// Go: `time.Duration(d)`, nanoseconds saturated to `int64`.
fn go_duration(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)
}

/// Go: `t.Sub(u)` on wall clock readings, saturating like Go.
fn go_sub(t: SystemTime, u: SystemTime) -> i64 {
    match t.duration_since(u) {
        Ok(elapsed) => go_duration(elapsed),
        Err(err) => i64::try_from(err.duration().as_nanos()).map_or(i64::MIN, |nanos| -nanos),
    }
}

/// Go: `Duration.Seconds()`, `float64(sec) + float64(nsec)/1e9`.
fn go_seconds(nanos: i64) -> f64 {
    const NANOS_PER_SECOND: i64 = 1_000_000_000;
    let seconds = nanos / NANOS_PER_SECOND;
    let fraction = i32::try_from(nanos % NANOS_PER_SECOND).unwrap_or_default();
    i64_to_f64(seconds) + f64::from(fraction) / 1e9
}

/// Go: `float64(v)` for an `int64` below 2^53 in magnitude (exact), without a lossy cast.
fn i64_to_f64(value: i64) -> f64 {
    let high = i32::try_from(value >> 32).unwrap_or_default();
    let low = u32::try_from(value & 0xFFFF_FFFF).unwrap_or_default();
    f64::from(high).mul_add(4_294_967_296.0, f64::from(low))
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        future,
        sync::{
            Arc, Mutex, PoisonError,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, SystemTime},
    };

    use tokio::time::{self, Instant};

    use super::{
        BoxError, Clock, FetchError, HttpClient, HttpResponse, RateConverter, ResponseFuture,
        go_seconds, go_sub, write,
    };
    use crate::{
        currency_mock::MockCurrencyRatesHttpClient,
        golden::{SplitMix64, TestResult, check, check_eq, check_random_corpus, conversion_rates},
        rates::Rates,
        ticker_task::TickerTask,
    };

    const MOCK_RATES: &str = r#"{
		"dataAsOf":"2018-09-12",
		"conversions":{
			"USD":{
				"GBP":0.77208
			},
			"GBP":{
				"USD":1.2952
			}
		}
	}"#;

    fn expected_rates() -> Rates {
        Rates::new(Some(conversion_rates(&[
            ("USD", &[("GBP", 0.77208)]),
            ("GBP", &[("USD", 1.2952)]),
        ])))
    }

    /// Go: `time.Date(2018, time.September, 12, 30, 0, 0, 0, time.UTC)`, which normalizes to
    /// 2018-09-13T06:00:00Z.
    fn initial_fake_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_hours(426_894)
    }

    /// Go: the test `FakeTime`.
    #[derive(Debug)]
    struct FakeClock(Mutex<SystemTime>);

    impl FakeClock {
        fn advance(&self, by: Duration) {
            let mut now = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            *now += by;
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> SystemTime {
            *self.0.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    /// Answers each call with the next scripted response (the last one repeats).
    #[derive(Debug)]
    struct ScriptedClient {
        responses: Vec<Scripted>,
        calls: AtomicUsize,
    }

    #[derive(Debug, Clone)]
    enum Scripted {
        Respond(u16, &'static str),
        Fail(&'static str),
        BodyFails(u16, &'static str),
        Hang,
        HangBody(u16),
    }

    impl ScriptedClient {
        fn new(responses: Vec<Scripted>) -> Self {
            Self {
                responses,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl HttpClient for ScriptedClient {
        fn get<'a>(&'a self, _url: &'a str) -> ResponseFuture<'a> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let scripted = self
                .responses
                .get(call)
                .or_else(|| self.responses.last())
                .cloned()
                .unwrap_or(Scripted::Fail("no response scripted"));
            Box::pin(async move {
                match scripted {
                    Scripted::Respond(status, body) => {
                        Ok(HttpResponse::from_bytes(status, body.as_bytes().to_vec()))
                    }
                    Scripted::Fail(message) => Err(BoxError::from(message)),
                    Scripted::BodyFails(status, message) => {
                        Ok(HttpResponse::new(status, async move {
                            Err(BoxError::from(message))
                        }))
                    }
                    Scripted::Hang => future::pending().await,
                    Scripted::HangBody(status) => Ok(HttpResponse::new(status, future::pending())),
                }
            })
        }
    }

    fn converter(
        client: Arc<dyn HttpClient>,
        stale: Duration,
        now: SystemTime,
    ) -> (RateConverter, Arc<FakeClock>) {
        let clock = Arc::new(FakeClock(Mutex::new(now)));
        let mut converter = RateConverter::new(
            client,
            Duration::from_secs(60),
            "currency.fake.com".to_owned(),
            stale,
        );
        converter.clock = Arc::<FakeClock>::clone(&clock);
        (converter, clock)
    }

    fn is_constant(converter: &RateConverter) -> bool {
        format!("{:?}", converter.rates()) == "ConstantRates"
    }

    fn fetched(converter: &RateConverter) -> Option<Rates> {
        converter.rates().get_rates().cloned()
    }

    #[tokio::test]
    async fn read_write_rates() -> TestResult {
        // Go: TestReadWriteRates (the URL case needs a real client and lives with the wiring).
        let fake_time = initial_fake_time();
        let cases = [
            (
                "Fetching currency rates successfully",
                Scripted::Respond(200, MOCK_RATES),
                Some(expected_rates()),
            ),
            (
                "Currency rates endpoint returns empty response",
                Scripted::Respond(200, "{}"),
                Some(Rates::new(None)),
            ),
            (
                "Currency rates endpoint returns nil response",
                Scripted::Respond(200, ""),
                None,
            ),
            (
                "Currency rates endpoint returns non-2xx status code",
                Scripted::Respond(404, r#"{"message": "Not Found"}"#),
                None,
            ),
            (
                "Currency rates endpoint returns invalid json response",
                Scripted::Respond(200, r#"{"message": Invalid-JSON-No-Surrounding-Quotes}"#),
                None,
            ),
            (
                "Currency rates endpoint url is invalid",
                Scripted::Fail(r#"Get "invalidurl": unsupported protocol scheme """#),
                None,
            ),
        ];
        for (description, response, want_rates) in cases {
            let client = Arc::new(ScriptedClient::new(vec![response]));
            let (converter, _clock) = converter(client, Duration::from_hours(24), fake_time);
            let result = converter.run().await;
            check_eq(&result.is_err(), &want_rates.is_none(), description)?;
            if let Some(rates) = want_rates {
                check_eq(&fetched(&converter), &Some(rates), description)?;
                check_eq(&converter.last_updated(), &Some(fake_time), description)?;
            } else {
                check(is_constant(&converter), description)?;
                check_eq(&converter.last_updated(), &None, description)?;
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn rate_staleness() -> TestResult {
        // Go: TestRateStaleness
        let not_found = Scripted::Respond(404, r#"{"message": "Not Found"}"#);
        let ok = Scripted::Respond(200, MOCK_RATES);
        let client = Arc::new(ScriptedClient::new(vec![
            not_found.clone(),
            ok.clone(),
            not_found.clone(),
            not_found,
            ok,
        ]));
        let initial = initial_fake_time();
        let (converter, clock) = converter(client, Duration::from_secs(30), initial);

        check(converter.run().await.is_err(), "first update fails")?;
        check(
            is_constant(&converter),
            "Rates should return constant rates",
        )?;
        check_eq(
            &converter.last_updated(),
            &None,
            "LastUpdated return is incorrect",
        )?;

        check(converter.run().await.is_ok(), "second update succeeds")?;
        check_eq(
            &fetched(&converter),
            &Some(expected_rates()),
            "rates after the second update",
        )?;
        check_eq(
            &converter.last_updated(),
            &Some(initial),
            "LastUpdated should be set",
        )?;

        clock.advance(Duration::from_secs(29));
        check(converter.run().await.is_err(), "third update fails")?;
        check_eq(
            &fetched(&converter),
            &Some(expected_rates()),
            "rates are not stale yet",
        )?;
        check_eq(
            &converter.last_updated(),
            &Some(initial),
            "LastUpdated is unchanged",
        )?;

        clock.advance(Duration::from_secs(2));
        check(converter.run().await.is_err(), "fourth update fails")?;
        check(
            is_constant(&converter),
            "stale rates fall back to constant rates",
        )?;
        check_eq(
            &converter.last_updated(),
            &Some(initial),
            "LastUpdated is unchanged",
        )?;

        check(converter.run().await.is_ok(), "fifth update succeeds")?;
        check_eq(
            &fetched(&converter),
            &Some(expected_rates()),
            "rates after the fifth update",
        )?;
        check_eq(
            &converter.last_updated(),
            &Some(initial + Duration::from_secs(31)),
            "LastUpdated should be set",
        )
    }

    #[tokio::test]
    async fn rates_are_never_considered_stale() -> TestResult {
        // Go: TestRatesAreNeverConsideredStale
        let client = Arc::new(ScriptedClient::new(vec![
            Scripted::Respond(200, MOCK_RATES),
            Scripted::Respond(404, r#"{"message": "Not Found"}"#),
        ]));
        let initial = initial_fake_time();
        let (converter, clock) = converter(client, Duration::ZERO, initial);

        check(converter.run().await.is_ok(), "first update succeeds")?;
        check_eq(
            &fetched(&converter),
            &Some(expected_rates()),
            "rates after the first update",
        )?;
        check_eq(
            &converter.last_updated(),
            &Some(initial),
            "LastUpdated should be set",
        )?;

        clock.advance(Duration::from_hours(24));
        check(converter.run().await.is_err(), "second update fails")?;
        check_eq(
            &fetched(&converter),
            &Some(expected_rates()),
            "rates from a day ago are kept",
        )?;
        check_eq(
            &converter.last_updated(),
            &Some(initial),
            "LastUpdated is unchanged",
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn race() -> TestResult {
        // Go: TestRace: clients update and read while the ticker updates every millisecond.
        let client = Arc::new(MockCurrencyRatesHttpClient {
            response_body: MOCK_RATES.to_owned(),
        });
        let converter = Arc::new(RateConverter::new(
            client,
            Duration::from_secs(60),
            "currency.fake.com".to_owned(),
            Duration::from_hours(24),
        ));
        let ticker = TickerTask::new(Duration::from_millis(1), Arc::clone(&converter));
        ticker.start().await;

        let mut clients = Vec::new();
        for client_num in 0..10_u64 {
            let converter = Arc::clone(&converter);
            clients.push(tokio::spawn(async move {
                let mut interval = time::interval(Duration::from_millis(client_num + 1));
                let started = Instant::now();
                while started.elapsed() < Duration::from_millis(100) {
                    let _tick = interval.tick().await;
                    if client_num < 5 {
                        converter.run().await.map_err(|err| err.to_string())?;
                    } else {
                        let rate = converter
                            .rates()
                            .get_rate("USD", "GBP")
                            .map_err(|err| err.to_string())?;
                        if rate.to_bits() != 0.77208_f64.to_bits() {
                            return Err(format!("rate {rate}"));
                        }
                    }
                }
                Ok::<(), String>(())
            }));
        }
        for handle in clients {
            handle.await??;
        }
        ticker.stop();
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn fetch_timeout_while_waiting_for_the_response() -> TestResult {
        let client = Arc::new(ScriptedClient::new(vec![Scripted::Hang]));
        let converter = RateConverter::new(
            client,
            Duration::from_millis(100),
            "https://cdn.example/latest.json".to_owned(),
            Duration::ZERO,
        );
        let err = converter
            .run()
            .await
            .err()
            .ok_or("the fetch must time out")?;
        check_eq(
            &err.to_string(),
            &r#"Get "https://cdn.example/latest.json": context deadline exceeded"#.to_owned(),
            "timeout message",
        )
    }

    #[tokio::test(start_paused = true)]
    async fn fetch_timeout_while_reading_the_body() -> TestResult {
        for (status, expected) in [
            (
                200,
                "the currency rates request failed: context deadline exceeded",
            ),
            (
                500,
                "the currency rates request failed with status code 500",
            ),
        ] {
            let client = Arc::new(ScriptedClient::new(vec![Scripted::HangBody(status)]));
            let converter = RateConverter::new(
                client,
                Duration::from_millis(100),
                "currency.fake.com".to_owned(),
                Duration::ZERO,
            );
            let err = converter
                .run()
                .await
                .err()
                .ok_or("the fetch must time out")?;
            check_eq(
                &err.to_string(),
                &expected.to_owned(),
                "body timeout message",
            )?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn fetch_error_messages_match_go() -> TestResult {
        let cases = [
            (
                Scripted::Respond(404, "{}"),
                "the currency rates request failed with status code 404",
            ),
            (
                Scripted::Respond(399, ""),
                "the currency rates request failed to parse json: expect { or n, but found \u{0}",
            ),
            (
                Scripted::Respond(200, ""),
                "the currency rates request failed to parse json: expect { or n, but found \u{0}",
            ),
            (
                Scripted::Respond(200, r#"{"message": Invalid-JSON-No-Surrounding-Quotes}"#),
                "the currency rates request failed to parse json: do not know how to skip: 73",
            ),
            (
                Scripted::BodyFails(200, "unexpected EOF"),
                "the currency rates request failed: unexpected EOF",
            ),
            (
                Scripted::BodyFails(503, "unexpected EOF"),
                "the currency rates request failed with status code 503",
            ),
            (
                Scripted::Fail("Get \"x\": dial tcp: lookup x: no such host"),
                "Get \"x\": dial tcp: lookup x: no such host",
            ),
        ];
        for (response, expected) in cases {
            let client = Arc::new(ScriptedClient::new(vec![response.clone()]));
            let converter = RateConverter::new(
                client,
                Duration::from_secs(60),
                "u".to_owned(),
                Duration::ZERO,
            );
            let err = converter.run().await.err().ok_or("the fetch must fail")?;
            check_eq(
                &err.to_string(),
                &expected.to_owned(),
                &format!("{response:?}"),
            )?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn status_399_with_valid_json_succeeds() -> TestResult {
        let client = Arc::new(ScriptedClient::new(vec![Scripted::Respond(
            399, MOCK_RATES,
        )]));
        let converter = RateConverter::new(
            client,
            Duration::from_secs(60),
            "u".to_owned(),
            Duration::ZERO,
        );
        check(
            converter.run().await.is_ok(),
            "Go only rejects status codes of 400 and above",
        )?;
        check_eq(&fetched(&converter), &Some(expected_rates()), "rates")
    }

    #[tokio::test]
    async fn get_info() -> TestResult {
        let client = Arc::new(ScriptedClient::new(vec![
            Scripted::Respond(200, MOCK_RATES),
            Scripted::Respond(200, "null"),
        ]));
        let initial = initial_fake_time();
        let (converter, _clock) = converter(client, Duration::ZERO, initial);

        let before = converter.get_info();
        check_eq(&before.source(), &"currency.fake.com", "source")?;
        check_eq(&before.last_updated(), &None, "never updated")?;
        check(before.rates().is_none(), "constant rates have no table")?;

        check(converter.run().await.is_ok(), "update")?;
        let after = converter.get_info();
        check_eq(&after.last_updated(), &Some(initial), "updated")?;
        check_eq(&after.rates().cloned(), &Some(expected_rates()), "rates")?;

        // Go: `null` decodes to a nil map, so the table is there but holds nothing.
        check(converter.run().await.is_ok(), "update with null")?;
        check_eq(
            &converter.get_info().rates().cloned(),
            &Some(Rates::new(None)),
            "null rates",
        )?;
        check_eq(
            &converter
                .rates()
                .get_rate("USD", "GBP")
                .map_err(|err| err.to_string()),
            &Err("rates are nil".to_owned()),
            "nil conversions",
        )
    }

    #[test]
    fn go_duration_seconds() -> TestResult {
        let base = initial_fake_time();
        check_eq(
            &go_seconds(go_sub(base + Duration::from_nanos(1_500_000_001), base)),
            &1.500_000_001,
            "positive",
        )?;
        check_eq(
            &go_seconds(go_sub(base, base + Duration::from_millis(2500))),
            &-2.5,
            "negative",
        )?;
        check_eq(
            &go_seconds(i64::MAX),
            &9_223_372_036.854_776,
            "max duration",
        )
    }

    /// Go: `time.Unix(sec, nsec)`: the normalized seconds and nanoseconds and the instant.
    fn go_unix(sec: i64, nsec: i64) -> Result<(i64, u32, SystemTime), Box<dyn Error>> {
        const NANOS: i64 = 1_000_000_000;
        let (mut whole, mut nanos) = (sec, nsec);
        if !(0..NANOS).contains(&nanos) {
            let carry = nanos / NANOS;
            whole += carry;
            nanos -= carry * NANOS;
            if nanos < 0 {
                nanos += NANOS;
                whole -= 1;
            }
        }
        let nanos = u32::try_from(nanos)?;
        let seconds = Duration::from_secs(whole.unsigned_abs());
        let at_second = if whole >= 0 {
            SystemTime::UNIX_EPOCH.checked_add(seconds)
        } else {
            SystemTime::UNIX_EPOCH.checked_sub(seconds)
        };
        let instant = at_second
            .and_then(|time| time.checked_add(Duration::from_nanos(u64::from(nanos))))
            .ok_or("time out of range")?;
        Ok((whole, nanos, instant))
    }

    #[test]
    fn go_random_staleness() -> TestResult {
        // Go: checkStaleRates on random last-update times, current times and thresholds
        // (testdata/go/random.go): nanosecond boundaries, whole seconds, Time.Sub saturating
        // beyond 292 years, and thresholds near Go's longest duration, where the float64
        // seconds comparison no longer sees nanoseconds.
        const NANOS: i64 = 1_000_000_000;
        let mut s = SplitMix64::new(20_240_903);
        let mut outcomes = Vec::new();
        for _ in 0..10_000 {
            // Go's time.Unix arguments: seconds and nanoseconds, the latter unnormalized.
            let updated_seconds = i64::try_from(s.next_u64() % (1 << 34))?;
            let updated_fraction = i64::try_from(s.below(1_000_000_000))?;
            let (threshold, current_seconds, current_fraction) = match s.below(4) {
                0 => {
                    let threshold = i64::try_from(s.next_u64() % (1 << 62))?;
                    let delta = threshold + i64::try_from(s.below(2001))? - 1000;
                    let seconds = updated_seconds + delta / NANOS;
                    (threshold, seconds, updated_fraction + delta % NANOS)
                }
                1 => {
                    let threshold = i64::try_from(s.below(1_000_000))? * NANOS;
                    let delta = threshold + i64::try_from(s.below(3))? - 1;
                    let seconds = updated_seconds + delta / NANOS;
                    (threshold, seconds, updated_fraction + delta % NANOS)
                }
                2 => {
                    let threshold = i64::try_from(s.next_u64() >> 1)?;
                    let seconds = i64::try_from(s.next_u64() % (1 << 34))?;
                    let fraction = i64::try_from(s.below(1_000_000_000))?;
                    (threshold, seconds, fraction)
                }
                _ => {
                    let threshold = i64::MAX - i64::try_from(s.next_u64() % (1 << 40))?;
                    let delta = i64::MAX - i64::try_from(s.next_u64() % (1 << 40))?;
                    let seconds = updated_seconds + delta / NANOS;
                    (threshold, seconds, updated_fraction + delta % NANOS)
                }
            };
            let (last_unix, last_nanos, last_updated) = go_unix(updated_seconds, updated_fraction)?;
            let (now_unix, now_nanos, now) = go_unix(current_seconds, current_fraction)?;
            let client = Arc::new(ScriptedClient::new(Vec::new()));
            let stale = Duration::from_nanos(u64::try_from(threshold)?);
            let (converter, _clock) = converter(client, stale, now);
            *write(&converter.last_updated) = Some(last_updated);
            outcomes.push((
                format!("{last_unix}.{last_nanos:09}\t{now_unix}.{now_nanos:09}\t{threshold}"),
                converter.check_stale_rates().to_string(),
            ));
        }
        check_random_corpus("stale", &outcomes)
    }

    #[test]
    fn fetch_error_sources() -> TestResult {
        let err = FetchError::ReadBody(BoxError::from("boom"));
        check_eq(
            &err.source().map(ToString::to_string),
            &Some("boom".to_owned()),
            "source",
        )?;
        check_eq(
            &FetchError::Timeout {
                url: "http://a/\"b\"\n".to_owned(),
            }
            .to_string(),
            &r#"Get "http://a/\"b\"\n": context deadline exceeded"#.to_owned(),
            "quoted URL",
        )
    }
}
