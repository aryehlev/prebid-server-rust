//! The `InfluxDB` reporter of the go-metrics engine.
//!
//! Go: `github.com/vrischmann/go-metrics-influxdb` v0.1.1 (`influxdb.go`), which
//! `metrics/config/metrics.go` starts when `metrics.influxdb.host` is set, over
//! `github.com/influxdata/influxdb1-client` 8bf82d3c094d ([`Client`]). At every interval it writes
//! every metric of the registry as points of the measurement, and every five seconds it pings the
//! server and makes a new client when the ping fails. Failures are logged and it carries on.
//!
//! The points of a send all carry its time, truncated to the interval with `align` ([`truncate`]):
//!
//! - a counter: the integer field `<name>.count`, without tags;
//! - a meter: the float field `<name>.meter`, one point per `bucket` tag: `count`, `m1`, `m5`,
//!   `m15` and `mean`;
//! - a histogram: `<name>.histogram` with the buckets `count`, `max`, `mean`, `min`, `stddev`,
//!   `variance`, `p50`, `p75`, `p95`, `p99`, `p999` and `p9999`, read from one snapshot whose
//!   percentiles are taken first;
//! - a timer: `<name>.timer` with the histogram's buckets and `m1`, `m5`, `m15` and `meanrate`.
//!
//! The names, tags and field types are go-metrics-influxdb's (Rule B (c), D-INFLUX); influxdb
//! 0.8.0 spells the points and writes them. The ledger's `influx` entries hold what differs: the
//! write request (`influx.write-request`), the statuses a write succeeds on
//! (`influx.write-status`), the credentials of a redirect (`influx.redirect-auth`), the error
//! texts (`influx.error-texts`), host URLs (`influx.host-url`), escapes (`influx.key-escapes`),
//! the ticks (`influx.cadence`) and the values the samples keep (`influx.sampled-values`). The
//! lines go through pbs-logger at ERROR, where Go's go to its standard logger
//! (`logging.std-log-lines`).

mod client;

use std::{
    fmt, future, io,
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use influxdb::{Timestamp, Type, WriteQuery};
use reqwest::Url;
use tokio::{
    runtime,
    time::{Instant, Interval, MissedTickBehavior, interval_at},
};

pub use self::client::{Client, USER_AGENT};
use crate::{
    float_from_i64,
    gometrics::{Metric, Registry, WeakRegistry},
    metrics::GoDuration,
};

/// Go: `time.Second * 5`, the interval of the pings.
const PING_INTERVAL: Duration = Duration::from_secs(5);

/// Go: the percentiles `send` reads from a histogram or timer snapshot.
const PERCENTILES: [f64; 6] = [0.5, 0.75, 0.95, 0.99, 0.999, 0.9999];

/// Go: the buckets of [`PERCENTILES`].
const PERCENTILE_BUCKETS: [&str; 6] = ["p50", "p75", "p95", "p99", "p999", "p9999"];

/// Go: `unixToInternal`, the seconds from January 1 of year 1 to the Unix epoch, where
/// `Time.Truncate` counts from.
const UNIX_TO_INTERNAL_NANOS: i128 = 62_135_596_800 * 1_000_000_000;

/// What the reporter sends and where (Go: the arguments of `influxdb.InfluxDB`).
#[derive(Clone, Default)]
pub struct Settings {
    /// Go: `d`, the interval of the sends (`metrics.influxdb.metric_send_interval` seconds); one
    /// that is not positive never sends (Go: `time.Tick` returns a nil channel).
    pub interval: GoDuration,
    /// Go: `url`, the server (`metrics.influxdb.host`).
    pub url: String,
    /// Go: `database`.
    pub database: String,
    /// Go: `measurement`, the measurement of every point.
    pub measurement: String,
    /// Go: `username`; when it is not empty the requests carry basic auth.
    pub username: String,
    /// Go: `password`.
    pub password: String,
    /// Go: `align`: the points' time is truncated to a multiple of the interval.
    pub align: bool,
}

impl fmt::Debug for Settings {
    /// Redacts the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("interval", &self.interval)
            .field("url", &self.url)
            .field("database", &self.database)
            .field("measurement", &self.measurement)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("align", &self.align)
            .finish()
    }
}

/// Go: `go influxdb.InfluxDB(registry, d, url, ...)`: [`run`] on a thread of its own.
///
/// The thread is `influxdb-reporter`, with a single-threaded tokio runtime. It ends at the first
/// tick after the registry is dropped.
///
/// # Errors
///
/// When the runtime or the thread cannot start (Go starts a goroutine, which cannot fail).
pub fn influx_db(registry: &Registry, settings: Settings) -> io::Result<JoinHandle<()>> {
    let registry = registry.downgrade();
    let runtime = runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    thread::Builder::new()
        .name("influxdb-reporter".to_owned())
        .spawn(move || runtime.block_on(run(&registry, &settings)))
}

/// Go: `InfluxDBWithTags(..., map[string]string{}, align)` and `reporter.run`: parses the URL,
/// makes the client, then sends at every interval and pings every five seconds until the
/// registry is dropped.
pub async fn run(registry: &WeakRegistry, settings: &Settings) {
    // Go: influxdb.go:35
    let url = match Url::parse(&settings.url) {
        Ok(url) => url,
        Err(err) => {
            pbs_logger::errorf!("unable to parse InfluxDB url {}. err={err}", settings.url);
            return;
        }
    };
    // Go: influxdb.go:52 (makeClient)
    let mut client = match new_client(&url, settings) {
        Ok(client) => client,
        Err(err) => {
            pbs_logger::errorf!("unable to make InfluxDB client. err={err}");
            return;
        }
    };
    // Go: influxdb.go:71-72
    let mut sends = u64::try_from(settings.interval.as_nanos())
        .ok()
        .filter(|&nanos| nanos > 0)
        .and_then(|nanos| ticker(Duration::from_nanos(nanos)));
    let mut pings = ticker(PING_INTERVAL);
    loop {
        tokio::select! {
            _ = tick(sends.as_mut()) => {
                let Some(registry) = registry.upgrade() else {
                    return;
                };
                if let Err(err) = send(&registry, settings, &client).await {
                    // Go: influxdb.go:78
                    pbs_logger::errorf!("unable to send metrics to InfluxDB. err={err}");
                }
            }
            _ = tick(pings.as_mut()) => {
                if registry.upgrade().is_none() {
                    return;
                }
                if let Err(err) = client.ping().await {
                    // Go: influxdb.go:83
                    pbs_logger::errorf!(
                        "got error while sending a ping to InfluxDB, trying to recreate client. \
                         err={err}"
                    );
                    match new_client(&url, settings) {
                        Ok(remade) => client = remade,
                        Err(make_err) => {
                            pbs_logger::errorf!("unable to make InfluxDB client. err={make_err}");
                        }
                    }
                }
            }
        }
    }
}

/// Go: `makeClient`.
fn new_client(url: &Url, settings: &Settings) -> io::Result<Client> {
    Client::new(
        url,
        &settings.username,
        &settings.password,
        &settings.database,
    )
}

/// Go: `time.Tick(period)`: the first tick one period from now, and a tick the reporter is too
/// busy to take fires as soon as it can, the next one coming back on the grid (`influx.cadence`).
/// `None` for a period too long for the clock, which never ticks.
fn ticker(period: Duration) -> Option<Interval> {
    let start = Instant::now().checked_add(period)?;
    let mut ticker = interval_at(start, period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    Some(ticker)
}

/// The next tick of `ticker`; never without one.
async fn tick(ticker: Option<&mut Interval>) -> Instant {
    match ticker {
        Some(ticker) => ticker.tick().await,
        None => future::pending().await,
    }
}

/// Go: `send`: one batch with the points of every metric, at the time of the call.
async fn send(
    registry: &Registry,
    settings: &Settings,
    client: &Client,
) -> Result<(), influxdb::Error> {
    // Go: influxdb.go:96
    let mut now = unix_nanos(SystemTime::now());
    if settings.align {
        now = truncate(now, settings.interval);
    }
    let points = points(registry, &settings.measurement, now);
    client.write(points).await
}

/// The nanoseconds of `t` since the Unix epoch, negative before it.
fn unix_nanos(t: SystemTime) -> i128 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(after) => i128::try_from(after.as_nanos()).unwrap_or(i128::MAX),
        Err(before) => i128::try_from(before.duration().as_nanos()).map_or(i128::MIN, |n| -n),
    }
}

/// Go: `t.Truncate(d)`, for the time `unix_nanos` nanoseconds after the Unix epoch.
///
/// The time rounded down to a multiple of `d` since January 1 of year 1, Go's zero time
/// (`time/time.go` `Truncate` and `div`), or as it is when `d` is not positive.
#[must_use]
pub fn truncate(unix_nanos: i128, d: GoDuration) -> i128 {
    // Go: time/time.go:1782
    let d = i128::from(d.as_nanos());
    if d <= 0 {
        return unix_nanos;
    }
    // Go: div's remainder, which is never negative.
    unix_nanos - (unix_nanos + UNIX_TO_INTERNAL_NANOS).rem_euclid(d)
}

/// One point of `measurement` with the one field `key`, at `unix_nanos`.
///
/// Go: a `client.Point` with `Tags`, `Fields: {key: value}` and `Time`. The tags go in key order
/// without those whose value is empty, as Go writes them. A time before the Unix epoch is written
/// as 0 (`influx.write-request`).
#[must_use]
pub fn point(
    measurement: &str,
    tags: &[(&str, &str)],
    key: &str,
    value: Type,
    unix_nanos: i128,
) -> WriteQuery {
    let mut tags = tags.to_vec();
    tags.sort_by_key(|&(k, _)| k);
    let time = Timestamp::Nanoseconds(u128::try_from(unix_nanos).unwrap_or(0));
    let mut query = WriteQuery::new(time, measurement);
    for (k, v) in tags {
        if !v.is_empty() {
            query = query.add_tag(k, v);
        }
    }
    query.add_field(key, value)
}

/// Go: the points `send`'s callback appends for every metric of the registry, all at `time`.
fn points(registry: &Registry, measurement: &str, time: i128) -> Vec<WriteQuery> {
    let mut points = Vec::new();
    registry.each(|name, metric| match metric {
        Metric::Counter(c) => {
            // Go: influxdb.go:103
            let key = format!("{name}.count");
            let value = Type::SignedInteger(c.snapshot().count());
            points.push(point(measurement, &[], &key, value, time));
        }
        Metric::Histogram(h) => {
            // Go: influxdb.go:133
            let mut ms = h.snapshot();
            let percentiles = ms.percentiles(&PERCENTILES);
            let statistics = Statistics {
                count: ms.count(),
                max: ms.max(),
                mean: ms.mean(),
                min: ms.min(),
                std_dev: ms.std_dev(),
                variance: ms.variance(),
                percentiles,
            };
            let key = format!("{name}.histogram");
            buckets(&mut points, measurement, &key, statistics.buckets(), time);
        }
        Metric::Meter(m) => {
            // Go: influxdb.go:161
            let ms = m.snapshot();
            let fields = vec![
                ("count", float_from_i64(ms.count())),
                ("m1", ms.rate1()),
                ("m5", ms.rate5()),
                ("m15", ms.rate15()),
                ("mean", ms.rate_mean()),
            ];
            buckets(
                &mut points,
                measurement,
                &format!("{name}.meter"),
                fields,
                time,
            );
        }
        Metric::Timer(t) => {
            // Go: influxdb.go:181
            let mut ms = t.snapshot();
            let percentiles = ms.percentiles(&PERCENTILES);
            let statistics = Statistics {
                count: ms.count(),
                max: ms.max(),
                mean: ms.mean(),
                min: ms.min(),
                std_dev: ms.std_dev(),
                variance: ms.variance(),
                percentiles,
            };
            let mut fields = statistics.buckets();
            fields.extend([
                ("m1", ms.rate1()),
                ("m5", ms.rate5()),
                ("m15", ms.rate15()),
                ("meanrate", ms.rate_mean()),
            ]);
            buckets(
                &mut points,
                measurement,
                &format!("{name}.timer"),
                fields,
                time,
            );
        }
    });
    points
}

/// One point per bucket, tagged with it, its value in the float field `key` (Go: the
/// `for k, v := range fields` loops of `send`, which go in map order; these go by bucket).
fn buckets(
    points: &mut Vec<WriteQuery>,
    measurement: &str,
    key: &str,
    mut fields: Vec<(&str, f64)>,
    time: i128,
) {
    fields.sort_by_key(|&(bucket, _)| bucket);
    for (bucket, value) in fields {
        // Go: influxdb.go:224 (bucketTags)
        let tags = [("bucket", bucket)];
        points.push(point(measurement, &tags, key, Type::Float(value), time));
    }
}

/// What a histogram or timer snapshot gives after its percentiles were taken (Go: the `fields`
/// both cases of `send` start with).
struct Statistics {
    count: i64,
    max: i64,
    mean: f64,
    min: i64,
    std_dev: f64,
    variance: f64,
    percentiles: Vec<f64>,
}

impl Statistics {
    /// The buckets and their values.
    fn buckets(&self) -> Vec<(&'static str, f64)> {
        let mut buckets = vec![
            ("count", float_from_i64(self.count)),
            ("max", float_from_i64(self.max)),
            ("mean", self.mean),
            ("min", float_from_i64(self.min)),
            ("stddev", self.std_dev),
            ("variance", self.variance),
        ];
        buckets.extend(
            PERCENTILE_BUCKETS
                .into_iter()
                .zip(self.percentiles.iter().copied()),
        );
        buckets
    }
}
