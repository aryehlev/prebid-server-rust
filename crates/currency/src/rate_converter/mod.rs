//! Port of Go `currency/rate_converter.go`.

use std::{
    error::Error,
    fmt,
    sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::{Duration, SystemTime},
};

use pbs_errortypes::GoError;
use pbs_http_client::{
    Builder, Client, HttpClientSettings, ProxyEnv, RequestError,
    reqwest::{Method, Request},
};
use pbs_jsonutil::unmarshal_valid;
use pbs_logger::errorf;
use tokio::time::Instant;
use url::{ParseError, Url};

use crate::{
    constant_rates::ConstantRates, converter_info::ConverterInfo, errors::RateError, rates::Rates,
};

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

/// The client `main.go:72` hands the converter: Go's `&http.Client{}`.
///
/// That is `http.DefaultTransport`'s settings, the proxies `proxy` names (the server passes
/// [`ProxyEnv::from_process`]) and no timeout of its own; pbs-http-client sends
/// `User-Agent: Go-http-client/1.1` and asks for gzip, as Go's Transport does.
///
/// # Errors
///
/// When reqwest cannot build the client.
pub fn default_http_client(proxy: ProxyEnv) -> Result<Client, RequestError> {
    Builder::new(HttpClientSettings::DEFAULT_TRANSPORT, proxy).build()
}

/// Why fetching the currency file failed (Go: the errors of `RateConverter.fetch`).
///
/// Prebid's texts are Go's; the words of the client, the body reader and the URL parser are
/// reqwest's and the url crate's (`currency.fetch-error-texts`), and those of the JSON decoder
/// `serde_json`'s (`json.error-texts`).
#[derive(Debug)]
pub enum FetchError {
    /// Go: the `*url.Error` of `http.NewRequestWithContext` for a URL that does not parse,
    /// `parse "<url>": <reason>`, the URL quoted as Rust's `{:?}` quotes it where Go's `%q`
    /// does. Go parses a URL without a scheme (`invalidurl`) and fails in `client.Do` instead
    /// (`Get "invalidurl": unsupported protocol scheme ""`); the url crate refuses it here.
    Url {
        /// The configured URL.
        url: String,
        /// Why it does not parse.
        reason: ParseError,
    },
    /// Go: the error of `client.Do`, returned as it is. It reads as [`RequestError::do_text`]:
    /// `Get "<url>": context deadline exceeded` once the fetch timeout has passed.
    Request(RequestError),
    /// Go: `errortypes.BadServerResponse` (error code 4) for a status code of 400 or more.
    BadServerResponse {
        /// The response status code.
        status_code: u16,
    },
    /// Go: `fmt.Errorf("the currency rates request failed: %v", err)` when reading the body
    /// fails, including on the fetch deadline.
    ReadBody(RequestError),
    /// Go: `fmt.Errorf("the currency rates request failed to parse json: %v", err)`, `err` the
    /// `FailedToUnmarshal` of `jsonutil.UnmarshalValid`.
    ParseJson(GoError),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url { url, reason } => write!(f, "parse {url:?}: {reason}"),
            Self::Request(err) => f.write_str(&err.do_text()),
            Self::BadServerResponse { status_code } => {
                write!(
                    f,
                    "the currency rates request failed with status code {status_code}"
                )
            }
            Self::ReadBody(err) => write!(f, "the currency rates request failed: {err}"),
            Self::ParseJson(err) => {
                write!(f, "the currency rates request failed to parse json: {err}")
            }
        }
    }
}

impl Error for FetchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Url { reason, .. } => Some(reason),
            Self::Request(err) | Self::ReadBody(err) => Some(err),
            Self::ParseJson(err) => Some(err),
            Self::BadServerResponse { .. } => None,
        }
    }
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
    http_client: Client,
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
    /// The server passes [`default_http_client`]; a test passes a [`Client`] over a
    /// [`Transport`](pbs_http_client::Transport) of its own, such as
    /// [`MockCurrencyRatesHttpClient`](crate::MockCurrencyRatesHttpClient). Go's test doubles
    /// stand in for the whole client and may be handed a URL without a scheme; here the
    /// converter parses the URL before the client sees it, so it must be absolute.
    ///
    /// Go's durations are signed: a non-positive `httpTimeout` makes every request over the
    /// network fail with the expired deadline and a non-positive `staleRatesThreshold` means the
    /// rates never go stale. Callers pass [`Duration::ZERO`] for both. Go's server builds them
    /// from `currency_converter.fetch_timeout_ms` and `currency_converter.stale_rates_seconds`.
    #[must_use]
    pub fn new(
        http_client: Client,
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
        // Go: http.NewRequestWithContext parses the URL.
        let url = Url::parse(&self.sync_source_url).map_err(|reason| FetchError::Url {
            url: self.sync_source_url.clone(),
            reason,
        })?;
        // Go reads the body once: with io.ReadAll below a 400 status, and in the deferred drain
        // (which logs its error before the caller sees the result) for every status.
        let request = Request::new(Method::GET, url);
        let (status_code, (body, read_error)) =
            match self.http_client.send(request, deadline, None).await {
                Ok(response) => (response.status().as_u16(), response.read_all().await),
                Err(err) => return Err(FetchError::Request(err)),
            };
        if let Some(err) = &read_error {
            errorf!("error draining conversion rates response body: {err}");
        }
        if status_code >= 400 {
            return Err(FetchError::BadServerResponse { status_code });
        }
        if let Some(err) = read_error {
            return Err(FetchError::ReadBody(err));
        }
        // Go: jsonutil.UnmarshalValid(bytesJSON, &Rates{}), here into serde's Rates
        // (currency.rates-json).
        unmarshal_valid(&body).map_err(FetchError::ParseJson)
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
                    errorf!(
                        "Error updating conversion rates, falling back to constant rates: {err}"
                    );
                } else {
                    errorf!("Error updating conversion rates: {err}");
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

    use bytes::Bytes;
    use futures_util::future::BoxFuture;
    use pbs_http_client::{
        Client, ErrorKind, RequestError, Response, Trace, Transport,
        reqwest::{Request, StatusCode, header::HeaderMap},
    };
    use tokio::time::{self, Instant};
    use url::Url;

    use super::{Clock, FetchError, RateConverter, go_seconds, go_sub, write};
    use crate::{
        currency_mock::MockCurrencyRatesHttpClient,
        golden::{
            FAKE_URL, SplitMix64, TestResult, check, check_eq, check_random_corpus,
            conversion_rates,
        },
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

    /// Answers each request with the next scripted answer (the last one repeats), as the Go
    /// tests' servers and mock clients do.
    #[derive(Debug)]
    struct ScriptedTransport {
        answers: Vec<Scripted>,
        calls: AtomicUsize,
    }

    #[derive(Debug, Clone)]
    enum Scripted {
        Respond(u16, &'static str),
        Fail(&'static str),
        BodyFails(u16, &'static str),
        Hang,
    }

    impl ScriptedTransport {
        fn client(answers: Vec<Scripted>) -> Client {
            Client::from_transport(Arc::new(Self {
                answers,
                calls: AtomicUsize::new(0),
            }))
        }
    }

    /// A response from `url` with `status` and `body`.
    fn response(url: Url, status: u16, body: &'static str) -> Result<Response, RequestError> {
        let status = StatusCode::from_u16(status)
            .map_err(|err| RequestError::new(ErrorKind::Request, err))?;
        Ok(Response::new(
            url,
            status,
            HeaderMap::new(),
            Bytes::from_static(body.as_bytes()),
        ))
    }

    impl Transport for ScriptedTransport {
        fn round_trip(
            &self,
            request: Request,
            _deadline: Option<Instant>,
            _trace: Option<Trace>,
        ) -> BoxFuture<'_, Result<Response, RequestError>> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let scripted = self
                .answers
                .get(call)
                .or_else(|| self.answers.last())
                .cloned()
                .unwrap_or(Scripted::Fail("no answer scripted"));
            let url = request.url().clone();
            Box::pin(async move {
                match scripted {
                    Scripted::Respond(status, body) => response(url, status, body),
                    Scripted::Fail(message) => Err(RequestError::new(ErrorKind::Request, message)),
                    Scripted::BodyFails(status, message) => response(url, status, "").map(|made| {
                        made.with_body_error(RequestError::new(ErrorKind::Body, message))
                    }),
                    Scripted::Hang => future::pending().await,
                }
            })
        }
    }

    fn converter(
        client: Client,
        stale: Duration,
        now: SystemTime,
    ) -> (RateConverter, Arc<FakeClock>) {
        let clock = Arc::new(FakeClock(Mutex::new(now)));
        let mut converter =
            RateConverter::new(client, Duration::from_secs(60), FAKE_URL.to_owned(), stale);
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
        // Go: TestReadWriteRates
        let fake_time = initial_fake_time();
        let cases = [
            (
                "Fetching currency rates successfully",
                FAKE_URL,
                Scripted::Respond(200, MOCK_RATES),
                Some(expected_rates()),
            ),
            (
                "Currency rates endpoint returns empty response",
                FAKE_URL,
                Scripted::Respond(200, "{}"),
                Some(Rates::new(None)),
            ),
            (
                "Currency rates endpoint returns nil response",
                FAKE_URL,
                Scripted::Respond(200, ""),
                None,
            ),
            (
                "Currency rates endpoint returns non-2xx status code",
                FAKE_URL,
                Scripted::Respond(404, r#"{"message": "Not Found"}"#),
                None,
            ),
            (
                "Currency rates endpoint returns invalid json response",
                FAKE_URL,
                Scripted::Respond(200, r#"{"message": Invalid-JSON-No-Surrounding-Quotes}"#),
                None,
            ),
            (
                "Currency rates endpoint url is invalid",
                "invalidurl",
                Scripted::Respond(200, MOCK_RATES),
                None,
            ),
        ];
        for (description, url, answer, want_rates) in cases {
            let mut converter = RateConverter::new(
                ScriptedTransport::client(vec![answer]),
                Duration::from_secs(60),
                url.to_owned(),
                Duration::from_hours(24),
            );
            converter.clock = Arc::new(FakeClock(Mutex::new(fake_time)));
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
        let client = ScriptedTransport::client(vec![
            not_found.clone(),
            ok.clone(),
            not_found.clone(),
            not_found,
            ok,
        ]);
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
        let client = ScriptedTransport::client(vec![
            Scripted::Respond(200, MOCK_RATES),
            Scripted::Respond(404, r#"{"message": "Not Found"}"#),
        ]);
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
        let client = MockCurrencyRatesHttpClient {
            response_body: MOCK_RATES.to_owned(),
        }
        .into_client();
        let converter = Arc::new(RateConverter::new(
            client,
            Duration::from_secs(60),
            FAKE_URL.to_owned(),
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
        // Go: currency/rate_converter.go:50-61 (the request runs under context.WithTimeout,
        // whose expiry client.Do reports with the URL)
        let converter = RateConverter::new(
            ScriptedTransport::client(vec![Scripted::Hang]),
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

    #[tokio::test]
    async fn fetch_error_messages_match_go() -> TestResult {
        // Go: currency/rate_converter.go:58-84 (client.Do's error as it is, then Prebid's texts
        // for the status, the body and the JSON)
        let cases = [
            (
                Scripted::Respond(404, "{}"),
                "the currency rates request failed with status code 404",
            ),
            // Go's words around serde_json's, where Go has jsoniter's `expect { or n, but found
            // \x00` and `do not know how to skip: 73` (json.error-texts).
            (
                Scripted::Respond(399, ""),
                "the currency rates request failed to parse json: EOF while parsing a value at \
                 line 1 column 0",
            ),
            (
                Scripted::Respond(200, ""),
                "the currency rates request failed to parse json: EOF while parsing a value at \
                 line 1 column 0",
            ),
            (
                Scripted::Respond(200, r#"{"message": Invalid-JSON-No-Surrounding-Quotes}"#),
                "the currency rates request failed to parse json: expected value at line 1 \
                 column 13",
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
                Scripted::Fail("dial tcp: lookup currency.fake.com: no such host"),
                "Get \"http://currency.fake.com/latest.json\": dial tcp: lookup currency.fake.com: \
                 no such host",
            ),
        ];
        for (answer, expected) in cases {
            let converter = RateConverter::new(
                ScriptedTransport::client(vec![answer.clone()]),
                Duration::from_secs(60),
                FAKE_URL.to_owned(),
                Duration::ZERO,
            );
            let err = converter.run().await.err().ok_or("the fetch must fail")?;
            check_eq(
                &err.to_string(),
                &expected.to_owned(),
                &format!("{answer:?}"),
            )?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn status_399_with_valid_json_succeeds() -> TestResult {
        // Go: currency/rate_converter.go:72 (only a status code of 400 or more fails)
        let converter = RateConverter::new(
            ScriptedTransport::client(vec![Scripted::Respond(399, MOCK_RATES)]),
            Duration::from_secs(60),
            FAKE_URL.to_owned(),
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
        // Go: currency/rate_converter.go:151-159 (GetInfo: the source, the last update and the
        // fetched rates, none for the constant rates)
        let client = ScriptedTransport::client(vec![
            Scripted::Respond(200, MOCK_RATES),
            Scripted::Respond(200, r#"{"conversions":null}"#),
        ]);
        let initial = initial_fake_time();
        let (converter, _clock) = converter(client, Duration::ZERO, initial);

        let before = converter.get_info();
        check_eq(&before.source(), &FAKE_URL, "source")?;
        check_eq(&before.last_updated(), &None, "never updated")?;
        check(before.rates().is_none(), "constant rates have no table")?;

        check(converter.run().await.is_ok(), "update")?;
        let after = converter.get_info();
        check_eq(&after.last_updated(), &Some(initial), "updated")?;
        check_eq(&after.rates().cloned(), &Some(expected_rates()), "rates")?;

        // Go: a null `conversions` decodes to a nil map, so the table is there but holds
        // nothing.
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
        // Go: currency/rate_converter.go:144 (checkStaleRates compares Duration.Seconds())
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
            let stale = Duration::from_nanos(u64::try_from(threshold)?);
            let (converter, _clock) = converter(ScriptedTransport::client(Vec::new()), stale, now);
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
        // Go: currency/rate_converter.go:53-56 (http.NewRequestWithContext's url.Error, which
        // quotes the URL with %q; `{:?}` writes these characters as %q does)
        let err = FetchError::ReadBody(RequestError::new(ErrorKind::Body, "boom"));
        check_eq(
            &err.source().map(ToString::to_string),
            &Some("boom".to_owned()),
            "source",
        )?;
        let url = "http://a/\"b\"\n[";
        let reason = Url::parse("http://[::1").err().ok_or("the URL parses")?;
        check_eq(
            &FetchError::Url {
                url: url.to_owned(),
                reason,
            }
            .to_string(),
            &r#"parse "http://a/\"b\"\n[": invalid IPv6 address"#.to_owned(),
            "quoted URL",
        )
    }
}
