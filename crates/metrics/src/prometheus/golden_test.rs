//! The differential test against Go: replays `testdata/scenarios.txt` and the generated
//! `testdata/scenarios_generated.txt` (written by `testdata/go -gen`) through the Rust engine and
//! compares the scrape with the output of the Go engine written by `testdata/go` to
//! `testdata/golden/`.
//!
//! The committed `<scenario>.prom` files hold the scrape reduced to a few sample core bidders (the
//! full scrape repeats every adapter family for all core bidders); `digests.txt` pins the length
//! and FNV-1a 64 hash of every complete scrape, so the full Rust scrape is compared too.
//! `digest_only` scenarios have no `.prom` file. Where Go's `NewMetrics` panics, the scrape is the
//! line `panic: <message>`, compared with the Rust registration error.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
};

use super::{Metrics, expfmt::FMT_TEXT, tests::core_bidder_names};
use crate::{
    goconv::to_lower,
    metrics::{
        self, AdapterLabels, CcpaLabels, FetcherOperation, GdprLabels, GoDuration, ImpLabels,
        ImpMediaType, Labels, MetricsEngine as _, ModuleLabels, PrivacyLabels, StoredDataLabels,
    },
    options::{DisabledMetric, DisabledMetrics, PrometheusMetrics},
};

type TestResult = Result<(), Box<dyn Error>>;

const TESTDATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata");

/// Go: `sampleAdapters` in `testdata/go/main.go`.
const SAMPLE_ADAPTERS: [&str; 6] = [
    "33across",
    "adkerneladn",
    "appnexus",
    "pubmatic",
    "rubicon",
    "zmaticoo",
];

#[derive(Debug, Default)]
struct Scenario {
    name: String,
    /// Go: `digestOnly` — only the digest of the full scrape is pinned.
    digest_only: bool,
    cfg: PrometheusMetrics,
    disabled: DisabledMetrics,
    syncers: Vec<String>,
    modules: BTreeMap<String, Vec<String>>,
    calls: Vec<(usize, Vec<String>)>,
}

/// Go: `url.PathUnescape`.
fn path_unescape(s: &str) -> Result<String, Box<dyn Error>> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = s
                .get(i + 1..i + 3)
                .ok_or_else(|| format!("invalid URL escape in {s:?}"))?;
            out.push(u8::from_str_radix(hex, 16)?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    Ok(String::from_utf8(out)?)
}

/// Go: `strconv.ParseBool`.
fn parse_bool(s: &str) -> Result<bool, Box<dyn Error>> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Ok(false),
        _ => Err(format!("strconv.ParseBool: parsing {s:?}: invalid syntax").into()),
    }
}

/// Go: `parseScript`.
fn parse_script(text: &str) -> Result<Vec<Scenario>, Box<dyn Error>> {
    let mut scenarios: Vec<Scenario> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let raw: Vec<&str> = line.split(' ').collect();
        let directive = raw.first().copied().unwrap_or_default();
        let is_call = directive.starts_with("Record");
        let mut tokens = vec![directive.to_owned()];
        for token in raw.iter().skip(1) {
            tokens.push(if is_call {
                (*token).to_owned()
            } else {
                path_unescape(token)?
            });
        }
        let args = tokens.get(1..).unwrap_or_default();
        if directive == "scenario" {
            let [name] = args else {
                return Err(format!("line {line_no}: scenario takes one name").into());
            };
            scenarios.push(Scenario {
                name: name.clone(),
                ..Scenario::default()
            });
            continue;
        }
        let cur = scenarios
            .last_mut()
            .ok_or_else(|| format!("line {line_no}: directive before scenario"))?;
        let first = args.first().cloned().unwrap_or_default();
        match directive {
            "digest_only" => cur.digest_only = true,
            "namespace" => cur.cfg.namespace = first,
            "subsystem" => cur.cfg.subsystem = first,
            "disabled" => {
                for kv in args {
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| format!("line {line_no}: bad flag"))?;
                    let flag = DisabledMetric::from_config_key(k)
                        .ok_or_else(|| format!("line {line_no}: unknown disabled metric {k:?}"))?;
                    cur.disabled.set(flag, parse_bool(v)?);
                }
            }
            "syncer" => cur.syncers.extend_from_slice(args),
            "module" => {
                let stages = args
                    .get(1..)
                    .ok_or_else(|| format!("line {line_no}: module name"))?;
                drop(cur.modules.insert(first, stages.to_vec()));
            }
            _ if is_call => cur.calls.push((line_no, tokens)),
            _ => return Err(format!("line {line_no}: unknown directive {directive:?}").into()),
        }
    }
    Ok(scenarios)
}

/// Go: the `args` helper of `testdata/go/main.go`.
struct Args {
    values: BTreeMap<String, String>,
    used: BTreeSet<String>,
}

impl Args {
    fn parse(tokens: &[String]) -> Result<Self, Box<dyn Error>> {
        let mut values = BTreeMap::new();
        for token in tokens {
            let (k, v) = token
                .split_once('=')
                .ok_or_else(|| format!("argument {token:?} is not key=value"))?;
            if values.insert(k.to_owned(), path_unescape(v)?).is_some() {
                return Err(format!("duplicate argument {k:?}").into());
            }
        }
        Ok(Self {
            values,
            used: BTreeSet::new(),
        })
    }

    fn str(&mut self, k: &str) -> String {
        let _new = self.used.insert(k.to_owned());
        self.values.get(k).cloned().unwrap_or_default()
    }

    fn boolean(&mut self, k: &str) -> Result<bool, Box<dyn Error>> {
        let v = self.str(k);
        if v.is_empty() {
            Ok(false)
        } else {
            parse_bool(&v)
        }
    }

    fn integer(&mut self, k: &str) -> Result<usize, Box<dyn Error>> {
        let v = self.str(k);
        if v.is_empty() { Ok(0) } else { Ok(v.parse()?) }
    }

    fn float(&mut self, k: &str) -> Result<f64, Box<dyn Error>> {
        let v = self.str(k);
        if v.is_empty() {
            Ok(0.0)
        } else {
            Ok(v.parse()?)
        }
    }

    fn duration(&mut self, k: &str) -> Result<GoDuration, Box<dyn Error>> {
        let v = self.str(k);
        if v.is_empty() {
            Ok(GoDuration::default())
        } else {
            Ok(GoDuration::from_nanos(v.parse()?))
        }
    }

    /// A Go string-typed label: the zero value for "" and otherwise one of Go's constants.
    fn label<T: Copy + Default + fmt::Display>(
        &mut self,
        k: &str,
        values: &[T],
    ) -> Result<T, Box<dyn Error>> {
        let v = self.str(k);
        if v.is_empty() {
            return Ok(T::default());
        }
        values
            .iter()
            .copied()
            .find(|value| value.to_string() == v)
            .ok_or_else(|| format!("{k}={v:?} is not a Go constant").into())
    }

    fn finish(&self) -> TestResult {
        self.values
            .keys()
            .find(|k| !self.used.contains(*k))
            .map_or(Ok(()), |k| Err(format!("unknown argument {k:?}").into()))
    }

    fn labels(&mut self) -> Result<Labels, Box<dyn Error>> {
        Ok(Labels {
            source: self.label("source", metrics::demand_types())?,
            r_type: self.label("rtype", metrics::request_types())?,
            pub_id: self.str("pub"),
            cookie_flag: self.label("cookie", metrics::cookie_types())?,
            request_status: self.label("status", metrics::request_statuses())?,
            request_size: self.integer("size")?,
        })
    }

    fn adapter_labels(&mut self) -> Result<AdapterLabels, Box<dyn Error>> {
        let mut labels = AdapterLabels {
            source: self.label("source", metrics::demand_types())?,
            r_type: self.label("rtype", metrics::request_types())?,
            adapter: self.str("adapter"),
            pub_id: self.str("pub"),
            cookie_flag: self.label("cookie", metrics::cookie_types())?,
            adapter_bids: self.label("bids", metrics::adapter_bids())?,
            ..AdapterLabels::default()
        };
        let errors = self.str("errors");
        if !errors.is_empty() {
            for e in errors.split(',') {
                let error = metrics::adapter_errors()
                    .iter()
                    .copied()
                    .find(|v| v.as_str() == e)
                    .ok_or_else(|| format!("unknown adapter error {e:?}"))?;
                let _new = labels.adapter_errors.insert(error);
            }
        }
        Ok(labels)
    }

    fn module_labels(&mut self) -> ModuleLabels {
        ModuleLabels {
            module: self.str("module"),
            stage: self.str("stage"),
            account_id: self.str("account"),
        }
    }

    fn stored_data_labels(&mut self) -> Result<StoredDataLabels, Box<dyn Error>> {
        Ok(StoredDataLabels {
            data_type: self.label("type", metrics::stored_data_types())?,
            data_fetch_type: self.label("fetch", metrics::stored_data_fetch_types())?,
            error: self.label("error", metrics::stored_data_errors())?,
        })
    }

    fn imp_labels(&mut self) -> Result<ImpLabels, Box<dyn Error>> {
        let mut labels = ImpLabels::NONE;
        for (k, media_type) in [
            ("banner", ImpMediaType::BANNER),
            ("video", ImpMediaType::VIDEO),
            ("audio", ImpMediaType::AUDIO),
            ("native", ImpMediaType::NATIVE),
        ] {
            labels.set(media_type, self.boolean(k)?);
        }
        Ok(labels)
    }

    fn privacy_labels(&mut self) -> Result<PrivacyLabels, Box<dyn Error>> {
        Ok(PrivacyLabels {
            ccpa: CcpaLabels {
                enforced: self.boolean("ccpa_enforced")?,
                provided: self.boolean("ccpa_provided")?,
            },
            coppa_enforced: self.boolean("coppa")?,
            gdpr: GdprLabels {
                enforced: self.boolean("gdpr")?,
                tcf_version: self.label("tcf", metrics::tcf_versions())?,
            },
            lmt_enforced: self.boolean("lmt")?,
        })
    }
}

const FETCHER_OPERATIONS: [FetcherOperation; 3] = [
    FetcherOperation::GET,
    FetcherOperation::START,
    FetcherOperation::BACKGROUND_REFRESH,
];

/// Go: `replay`.
fn replay(m: &Metrics, tokens: &[String]) -> TestResult {
    let method = tokens.first().map_or("", String::as_str);
    let mut a = Args::parse(tokens.get(1..).unwrap_or_default())?;
    if replay_request_call(m, method, &mut a)?
        || replay_adapter_call(m, method, &mut a)?
        || replay_other_call(m, method, &mut a)?
    {
        a.finish()
    } else {
        Err(format!("unknown method {method:?}").into())
    }
}

fn replay_request_call(m: &Metrics, method: &str, a: &mut Args) -> Result<bool, Box<dyn Error>> {
    match method {
        "RecordConnectionAccept" => m.record_connection_accept(a.boolean("success")?),
        "RecordTMaxTimeout" => m.record_tmax_timeout(),
        "RecordConnectionClose" => m.record_connection_close(a.boolean("success")?),
        "RecordRequest" => m.record_request(&a.labels()?),
        "RecordImps" => m.record_imps(a.imp_labels()?),
        "RecordRequestTime" => m.record_request_time(&a.labels()?, a.duration("d")?),
        "RecordOverheadTime" => {
            m.record_overhead_time(
                a.label("type", metrics::overhead_types())?,
                a.duration("d")?,
            );
        }
        "RecordDNSTime" => m.record_dns_time(a.duration("d")?),
        "RecordTLSHandshakeTime" => m.record_tls_handshake_time(a.duration("d")?),
        "RecordBidderServerResponseTime" => {
            m.record_bidder_server_response_time(a.duration("d")?);
        }
        "RecordCookieSync" => {
            m.record_cookie_sync(a.label("status", metrics::cookie_sync_statuses())?);
        }
        "RecordSyncerRequest" => {
            let key = a.str("key");
            m.record_syncer_request(&key, a.label("status", metrics::syncer_request_statuses())?);
        }
        "RecordSetUid" => m.record_set_uid(a.label("status", metrics::set_uid_statuses())?),
        "RecordSyncerSet" => {
            let key = a.str("key");
            m.record_syncer_set(&key, a.label("status", metrics::syncer_set_uid_statuses())?);
        }
        "RecordPrebidCacheRequestTime" => {
            m.record_prebid_cache_request_time(a.boolean("success")?, a.duration("d")?);
        }
        "RecordRequestQueueTime" => m.record_request_queue_time(
            a.boolean("success")?,
            a.label("rtype", metrics::request_types())?,
            a.duration("d")?,
        ),
        "RecordTimeoutNotice" => m.record_timeout_notice(a.boolean("success")?),
        "RecordRequestPrivacy" => m.record_request_privacy(a.privacy_labels()?),
        "RecordDebugRequest" => {
            let enabled = a.boolean("enabled")?;
            m.record_debug_request(enabled, &a.str("pub"));
        }
        "RecordStoredResponse" => m.record_stored_response(&a.str("pub")),
        "RecordGvlListRequest" => m.record_gvl_list_request(),
        "RecordLiveGVLFetch" => m.record_live_gvl_fetch(a.boolean("success")?),
        "RecordAdsCertReq" => m.record_ads_cert_req(a.boolean("success")?),
        "RecordAdsCertSignTime" => m.record_ads_cert_sign_time(a.duration("d")?),
        _ => return Ok(false),
    }
    Ok(true)
}

fn replay_adapter_call(m: &Metrics, method: &str, a: &mut Args) -> Result<bool, Box<dyn Error>> {
    match method {
        "RecordAdapterRequest" => m.record_adapter_request(&a.adapter_labels()?),
        "RecordAdapterConnections" => {
            let adapter = a.str("adapter");
            m.record_adapter_connections(&adapter, a.boolean("reused")?, a.duration("d")?);
        }
        "RecordAdapterPanic" => m.record_adapter_panic(&a.adapter_labels()?),
        "RecordAdapterBidReceived" => {
            let labels = a.adapter_labels()?;
            let bid_type = a.str("bidtype");
            m.record_adapter_bid_received(&labels, &bid_type, a.boolean("adm")?);
        }
        "RecordAdapterPrice" => {
            let labels = a.adapter_labels()?;
            m.record_adapter_price(&labels, a.float("cpm")?);
        }
        "RecordAdapterTime" => {
            let labels = a.adapter_labels()?;
            m.record_adapter_time(&labels, a.duration("d")?);
        }
        "RecordAdapterBuyerUIDScrubbed" => m.record_adapter_buyer_uid_scrubbed(&a.str("adapter")),
        "RecordAdapterGDPRRequestBlocked" => {
            m.record_adapter_gdpr_request_blocked(&a.str("adapter"));
        }
        "RecordBidValidationCreativeSizeError" => {
            let adapter = a.str("adapter");
            m.record_bid_validation_creative_size_error(&adapter, &a.str("account"));
        }
        "RecordBidValidationCreativeSizeWarn" => {
            let adapter = a.str("adapter");
            m.record_bid_validation_creative_size_warn(&adapter, &a.str("account"));
        }
        "RecordBidValidationSecureMarkupError" => {
            let adapter = a.str("adapter");
            m.record_bid_validation_secure_markup_error(&adapter, &a.str("account"));
        }
        "RecordBidValidationSecureMarkupWarn" => {
            let adapter = a.str("adapter");
            m.record_bid_validation_secure_markup_warn(&adapter, &a.str("account"));
        }
        "RecordAdapterThrottled" => m.record_adapter_throttled(&a.str("adapter")),
        "RecordAdapterConnectionDialError" => {
            m.record_adapter_connection_dial_error(&a.str("adapter"));
        }
        "RecordAdapterConnectionDialTime" => {
            let adapter = a.str("adapter");
            m.record_adapter_connection_dial_time(&adapter, a.duration("d")?);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn replay_other_call(m: &Metrics, method: &str, a: &mut Args) -> Result<bool, Box<dyn Error>> {
    match method {
        "RecordStoredReqCacheResult" => m.record_stored_req_cache_result(
            a.label("result", metrics::cache_results())?,
            a.integer("inc")?,
        ),
        "RecordStoredImpCacheResult" => m.record_stored_imp_cache_result(
            a.label("result", metrics::cache_results())?,
            a.integer("inc")?,
        ),
        "RecordAccountCacheResult" => m.record_account_cache_result(
            a.label("result", metrics::cache_results())?,
            a.integer("inc")?,
        ),
        "RecordStoredDataFetchTime" => {
            let labels = a.stored_data_labels()?;
            m.record_stored_data_fetch_time(labels, a.duration("d")?);
        }
        "RecordStoredDataError" => m.record_stored_data_error(a.stored_data_labels()?),
        "RecordFetcherResult" => {
            let subsystem = a.str("subsystem");
            m.record_fetcher_result(&subsystem, a.label("result", metrics::fetcher_results())?);
        }
        "RecordFetcherBackendFetch" => {
            let subsystem = a.str("subsystem");
            m.record_fetcher_backend_fetch(
                &subsystem,
                a.label("operation", &FETCHER_OPERATIONS)?,
                a.label("result", metrics::fetcher_backend_results())?,
                a.duration("d")?,
            );
        }
        "RecordModuleCalled" => {
            let labels = a.module_labels();
            m.record_module_called(&labels, a.duration("d")?);
        }
        "RecordModuleFailed" => m.record_module_failed(&a.module_labels()),
        "RecordModuleSuccessNooped" => m.record_module_success_nooped(&a.module_labels()),
        "RecordModuleSuccessUpdated" => m.record_module_success_updated(&a.module_labels()),
        "RecordModuleSuccessRejected" => m.record_module_success_rejected(&a.module_labels()),
        "RecordModuleExecutionError" => m.record_module_execution_error(&a.module_labels()),
        "RecordModuleTimeout" => m.record_module_timeout(&a.module_labels()),
        _ => return Ok(false),
    }
    Ok(true)
}

/// Go: `adapterLabel`.
fn adapter_label(line: &str) -> Option<&str> {
    if line.starts_with('#') {
        return None;
    }
    for marker in ["{adapter=\"", ",adapter=\""] {
        if let Some(i) = line.find(marker) {
            let rest = line.get(i + marker.len()..)?;
            if let Some(j) = rest.find('"') {
                return rest.get(..j);
            }
        }
    }
    None
}

/// Go: `keepSampleAdapters`.
fn keep_sample_adapters(text: &str, core: &BTreeSet<String>) -> String {
    text.split_inclusive('\n')
        .filter(|line| {
            adapter_label(line).is_none_or(|a| !core.contains(a) || SAMPLE_ADAPTERS.contains(&a))
        })
        .collect()
}

/// Go: `hash/fnv` `New64a`.
fn fnv1a64(data: &[u8]) -> u64 {
    data.iter().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// The first line that differs, for a readable failure.
fn first_difference(expected: &str, actual: &str) -> String {
    let mut expected_lines = expected.lines();
    let mut actual_lines = actual.lines();
    for line_no in 1.. {
        match (expected_lines.next(), actual_lines.next()) {
            (None, None) => break,
            (e, a) if e == a => {}
            (e, a) => return format!("line {line_no}: Go {e:?}, Rust {a:?}"),
        }
    }
    "no line differs".to_owned()
}

/// The scrape of one scenario, or `panic: <message>` when Go's `NewMetrics` panics (the Rust
/// engine returns the registration error instead) and no call is replayed.
fn scrape(s: &Scenario, core_bidder_names: &[String]) -> Result<String, Box<dyn Error>> {
    let cfg = PrometheusMetrics {
        port: 8080,
        ..s.cfg.clone()
    };
    let m = match Metrics::new(&cfg, s.disabled, &s.syncers, &s.modules, core_bidder_names) {
        Ok(m) => m,
        Err(err) => return Ok(format!("panic: {err}\n")),
    };
    for (line_no, tokens) in &s.calls {
        replay(&m, tokens).map_err(|e| format!("{}: line {line_no}: {e}", s.name))?;
    }
    Ok(m.text_exposition())
}

/// The hand-written script followed by the one `testdata/go -gen` writes (Go: the `-script`
/// list of the generator).
fn all_scenarios() -> Result<Vec<Scenario>, Box<dyn Error>> {
    let mut scenarios = Vec::new();
    for script in ["scenarios.txt", "scenarios_generated.txt"] {
        scenarios.extend(parse_script(&fs::read_to_string(format!(
            "{TESTDATA}/{script}"
        ))?)?);
    }
    Ok(scenarios)
}

#[test]
fn scrapes_match_go_byte_for_byte() -> TestResult {
    let scenarios = all_scenarios()?;
    let digests_text = fs::read_to_string(format!("{TESTDATA}/golden/digests.txt"))?;
    let mut digests = BTreeMap::new();
    for line in digests_text.lines() {
        let mut fields = line.split(' ');
        let (Some(name), Some(len), Some(hash), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(format!("malformed digest line {line:?}").into());
        };
        let digest = (len.parse::<usize>()?, u64::from_str_radix(hash, 16)?);
        if digests.insert(name.to_owned(), digest).is_some() {
            return Err(format!("duplicate digest for {name}").into());
        }
    }
    let core_bidder_names = core_bidder_names()?;
    let core: BTreeSet<String> = core_bidder_names.iter().map(|n| to_lower(n)).collect();

    ensure!(
        scenarios.len() >= 60,
        "the scripts have {} scenarios",
        scenarios.len()
    );
    ensure_eq!(digests.len(), scenarios.len(), "one digest per scenario");
    let mut failures = Vec::new();
    for s in &scenarios {
        let body = scrape(s, &core_bidder_names)?;

        if !s.digest_only {
            let golden = fs::read_to_string(format!("{TESTDATA}/golden/{}.prom", s.name))?;
            let filtered = keep_sample_adapters(&body, &core);
            if filtered != golden {
                failures.push(format!(
                    "{}: the sample scrape differs from Go: {}",
                    s.name,
                    first_difference(&golden, &filtered)
                ));
                continue;
            }
        }

        let (len, hash) = digests.get(&s.name).copied().ok_or("missing digest")?;
        let actual = (body.len(), fnv1a64(body.as_bytes()));
        if actual != (len, hash) {
            failures.push(format!(
                "{}: full scrape (length, FNV-1a) {actual:?}, Go ({len}, {hash})",
                s.name
            ));
        }
    }
    ensure!(
        failures.is_empty(),
        "{} of {} scenarios differ from Go:\n{}",
        failures.len(),
        scenarios.len(),
        failures.join("\n")
    );
    Ok(())
}

#[test]
fn content_type_matches_go() -> TestResult {
    let golden = fs::read_to_string(format!("{TESTDATA}/golden/content_type.txt"))?;
    ensure_eq!(golden.trim_end(), FMT_TEXT, "promhttp Content-Type");
    Ok(())
}

#[test]
fn script_helpers_match_go() -> TestResult {
    ensure_eq!(
        path_unescape("a%20b%22%5C%0A%C3%A9")?,
        "a b\"\\\né",
        "PathUnescape"
    );
    ensure!(path_unescape("bad%2").is_err(), "truncated escape");
    ensure_eq!(fnv1a64(b""), 0xCBF2_9CE4_8422_2325, "FNV offset basis");
    ensure_eq!(fnv1a64(b"a"), 0xAF63_DC4C_8601_EC8C, "FNV-1a of a");
    ensure_eq!(
        adapter_label("x{adapter=\"appnexus\",y=\"1\"} 0"),
        Some("appnexus"),
        "first"
    );
    ensure_eq!(
        adapter_label("x{a=\"1\",adapter=\"rubicon\"} 0"),
        Some("rubicon"),
        "later"
    );
    ensure_eq!(
        adapter_label("# HELP x{adapter=\"appnexus\"}"),
        None,
        "comment"
    );
    Ok(())
}
