//! Helpers of the exposition tests.
//!
//! The scenario scripts `testdata/go` replays through Go's engine, the goldens it writes from
//! them, and a scrape read back into families and samples the way the generator reads Go's
//! (`readExposition` of `testdata/go/main.go`).

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
};

use pbs_metrics::{
    MetricsEngine,
    metrics::{
        self, AdapterError, AdapterLabels, CcpaLabels, FetcherOperation, GdprLabels, GoDuration,
        ImpLabels, ImpMediaType, Labels, ModuleLabels, PrivacyLabels, StoredDataLabels,
    },
    options::{DisabledMetric, DisabledMetrics, PrometheusMetrics},
    prometheus::Metrics,
};

/// A test result whose error is the failure message.
pub type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// The directory of the scripts and goldens.
pub const TESTDATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata");

/// Go: `sampleAdapters` in `testdata/go/main.go`, the core bidders whose series the committed
/// scrapes keep.
pub const SAMPLE_ADAPTERS: [&str; 6] = [
    "33across",
    "adkerneladn",
    "appnexus",
    "pubmatic",
    "rubicon",
    "zmaticoo",
];

/// Fails with `message` unless `condition` holds.
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

/// Fails, showing both values, unless they are equal.
///
/// # Errors
///
/// Both values and `what`, when they differ.
pub fn check_eq<T: PartialEq + fmt::Debug + ?Sized>(
    actual: &T,
    expected: &T,
    what: &str,
) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{what}\n  actual: {actual:?}\nexpected: {expected:?}").into())
    }
}

/// The text of `testdata/golden/<name>`.
///
/// # Errors
///
/// When the file cannot be read.
pub fn golden(name: &str) -> TestResult<String> {
    let path = format!("{TESTDATA}/golden/{name}");
    fs::read_to_string(&path).map_err(|e| format!("{path}: {e}").into())
}

/// `openrtb_ext.CoreBidderNames()` as Go's generator wrote it.
///
/// # Errors
///
/// When the golden cannot be read.
pub fn core_bidder_names() -> TestResult<Vec<String>> {
    Ok(golden("core_bidder_names.txt")?
        .lines()
        .map(str::to_owned)
        .collect())
}

/// The core bidder names as Go's `keepSampleAdapters` lowercases them. They are ASCII, where
/// `strings.ToLower` is ASCII lowercasing.
///
/// # Errors
///
/// When the golden cannot be read or a name is not ASCII.
pub fn core_adapter_labels() -> TestResult<BTreeSet<String>> {
    let names = core_bidder_names()?;
    check(
        names.iter().all(|name| name.is_ascii()),
        "every core bidder name is ASCII",
    )?;
    Ok(names.iter().map(|name| name.to_ascii_lowercase()).collect())
}

/// One scenario of a script.
#[derive(Debug, Default)]
pub struct Scenario {
    /// Its name, the stem of its golden.
    pub name: String,
    /// Go: `digestOnly`: only the digests of the scrape are pinned, there is no `.prom`.
    pub digest_only: bool,
    /// The `namespace` and `subsystem` directives.
    pub cfg: PrometheusMetrics,
    /// The `disabled` directives.
    pub disabled: DisabledMetrics,
    /// The `syncer` directives.
    pub syncers: Vec<String>,
    /// The `module` directives.
    pub modules: BTreeMap<String, Vec<String>>,
    /// The calls with their line numbers: the method, then its `key=value` tokens.
    pub calls: Vec<(usize, Vec<String>)>,
}

impl Scenario {
    /// Every text the scenario passes to the engine: the namespace, subsystem, syncers, module
    /// names and stages, and the decoded values of its calls.
    ///
    /// # Errors
    ///
    /// When a call's token is not `key=value` with a valid escape.
    pub fn inputs(&self) -> TestResult<Vec<String>> {
        let mut texts = vec![self.cfg.namespace.clone(), self.cfg.subsystem.clone()];
        texts.extend(self.syncers.iter().cloned());
        for (module, stages) in &self.modules {
            texts.push(module.clone());
            texts.extend(stages.iter().cloned());
        }
        for (_, tokens) in &self.calls {
            texts.extend(
                Args::parse(tokens.get(1..).unwrap_or_default())?
                    .values
                    .into_values(),
            );
        }
        Ok(texts)
    }
}

/// Go: `url.PathUnescape`.
///
/// # Errors
///
/// When an escape is cut short or not hexadecimal, or the result is not UTF-8.
pub fn path_unescape(s: &str) -> TestResult<String> {
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
fn parse_bool(s: &str) -> TestResult<bool> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Ok(false),
        _ => Err(format!("strconv.ParseBool: parsing {s:?}: invalid syntax").into()),
    }
}

/// Go: `parseScript`.
///
/// # Errors
///
/// When a line is not a directive the generator reads.
pub fn parse_script(text: &str) -> TestResult<Vec<Scenario>> {
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

/// The hand-written script followed by the one `testdata/go -gen` writes (Go: the `-script`
/// list of the generator).
///
/// # Errors
///
/// When a script cannot be read or parsed.
pub fn all_scenarios() -> TestResult<Vec<Scenario>> {
    read_scenarios(&["scenarios.txt", "scenarios_generated.txt"])
}

/// The scenarios of the `scripts` under `testdata`, in order.
///
/// # Errors
///
/// When a script cannot be read or parsed.
pub fn read_scenarios(scripts: &[&str]) -> TestResult<Vec<Scenario>> {
    let mut scenarios = Vec::new();
    for script in scripts {
        let path = format!("{TESTDATA}/{script}");
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        scenarios.extend(parse_script(&text)?);
    }
    Ok(scenarios)
}

/// Go: the `args` helper of `testdata/go/main.go`.
struct Args {
    values: BTreeMap<String, String>,
    used: BTreeSet<String>,
}

impl Args {
    fn parse(tokens: &[String]) -> TestResult<Self> {
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

    fn boolean(&mut self, k: &str) -> TestResult<bool> {
        let v = self.str(k);
        if v.is_empty() {
            Ok(false)
        } else {
            parse_bool(&v)
        }
    }

    fn integer(&mut self, k: &str) -> TestResult<usize> {
        let v = self.str(k);
        if v.is_empty() { Ok(0) } else { Ok(v.parse()?) }
    }

    fn float(&mut self, k: &str) -> TestResult<f64> {
        let v = self.str(k);
        if v.is_empty() {
            Ok(0.0)
        } else {
            Ok(v.parse()?)
        }
    }

    fn duration(&mut self, k: &str) -> TestResult<GoDuration> {
        let v = self.str(k);
        if v.is_empty() {
            Ok(GoDuration::default())
        } else {
            Ok(GoDuration::from_nanos(v.parse()?))
        }
    }

    /// A Go string-typed label: the zero value for "" and otherwise one of Go's constants.
    fn label<T: Copy + Default + fmt::Display>(&mut self, k: &str, values: &[T]) -> TestResult<T> {
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

    fn labels(&mut self) -> TestResult<Labels> {
        Ok(Labels {
            source: self.label("source", metrics::demand_types())?,
            r_type: self.label("rtype", metrics::request_types())?,
            pub_id: self.str("pub"),
            cookie_flag: self.label("cookie", metrics::cookie_types())?,
            request_status: self.label("status", metrics::request_statuses())?,
            request_size: self.integer("size")?,
        })
    }

    fn adapter_labels(&mut self) -> TestResult<AdapterLabels> {
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
                // Go: an empty element is the zero AdapterError, as for the other labels.
                let error = if e.is_empty() {
                    AdapterError::default()
                } else {
                    metrics::adapter_errors()
                        .iter()
                        .copied()
                        .find(|v| v.as_str() == e)
                        .ok_or_else(|| format!("unknown adapter error {e:?}"))?
                };
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

    fn stored_data_labels(&mut self) -> TestResult<StoredDataLabels> {
        Ok(StoredDataLabels {
            data_type: self.label("type", metrics::stored_data_types())?,
            data_fetch_type: self.label("fetch", metrics::stored_data_fetch_types())?,
            error: self.label("error", metrics::stored_data_errors())?,
        })
    }

    fn imp_labels(&mut self) -> TestResult<ImpLabels> {
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

    fn privacy_labels(&mut self) -> TestResult<PrivacyLabels> {
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

/// The fetcher operations Go's generator names.
const FETCHER_OPERATIONS: [FetcherOperation; 3] = [
    FetcherOperation::GET,
    FetcherOperation::START,
    FetcherOperation::BACKGROUND_REFRESH,
];

/// Go: `replay`: one scripted call, through either engine.
///
/// # Errors
///
/// When the call names an unknown method, argument or label value.
pub fn replay(m: &dyn MetricsEngine, tokens: &[String]) -> TestResult {
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

fn replay_request_call(m: &dyn MetricsEngine, method: &str, a: &mut Args) -> TestResult<bool> {
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

fn replay_adapter_call(m: &dyn MetricsEngine, method: &str, a: &mut Args) -> TestResult<bool> {
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

fn replay_other_call(m: &dyn MetricsEngine, method: &str, a: &mut Args) -> TestResult<bool> {
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

/// What the Rust engine makes of a scenario.
#[derive(Debug)]
pub enum Scrape {
    /// The text exposition after the scenario's calls.
    Text(String),
    /// The registration error, where Go's `NewMetrics` panics; no call is replayed.
    Refused(String),
}

/// Builds the engine for `s` as Go's generator does (port 8080), replays its calls and scrapes
/// it.
///
/// # Errors
///
/// When a call cannot be replayed or the scrape cannot be encoded.
pub fn scrape(s: &Scenario, core_bidder_names: &[String]) -> TestResult<Scrape> {
    let cfg = PrometheusMetrics {
        port: 8080,
        ..s.cfg.clone()
    };
    let m = match Metrics::new(&cfg, s.disabled, &s.syncers, &s.modules, core_bidder_names) {
        Ok(m) => m,
        Err(err) => return Ok(Scrape::Refused(err.to_string())),
    };
    for (line_no, tokens) in &s.calls {
        replay(&m, tokens).map_err(|e| format!("{}: line {line_no}: {e}", s.name))?;
    }
    Ok(Scrape::Text(m.text_exposition()?))
}

/// Go: `adapterLabel`: the value of the adapter label of a sample line.
#[must_use]
pub fn adapter_label(line: &str) -> Option<&str> {
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

/// Go: `keepSampleAdapters`: the scrape without the series of core bidders other than the
/// sample ones.
#[must_use]
pub fn keep_sample_adapters(text: &str, core: &BTreeSet<String>) -> String {
    text.split_inclusive('\n')
        .filter(|line| {
            adapter_label(line).is_none_or(|a| !core.contains(a) || SAMPLE_ADAPTERS.contains(&a))
        })
        .collect()
}

/// Go: `hash/fnv` `New64a`.
#[must_use]
pub fn fnv1a64(data: &[u8]) -> u64 {
    data.iter().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// Go: `hexOf`.
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|b| [b >> 4, b & 0x0F])
        .filter_map(|nibble| char::from_digit(u32::from(nibble), 16))
        .collect()
}

/// Go: `unescape`: the text format's escapes `\\` and `\n`, and `\"` in a label value (`quote`).
fn unescape(s: &[u8], quote: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while let Some(&b) = s.get(i) {
        let escaped = if b == b'\\' {
            s.get(i + 1).and_then(|next| match next {
                b'\\' => Some(b'\\'),
                b'n' => Some(b'\n'),
                b'"' if quote => Some(b'"'),
                _ => None,
            })
        } else {
            None
        };
        if let Some(e) = escaped {
            out.push(e);
            i += 2;
        } else {
            out.push(b);
            i += 1;
        }
    }
    out
}

/// Go: `canonicalValue`: a sample value as both sides compare it, `NaN`, `0` for either zero,
/// otherwise the IEEE-754 bits in hex.
///
/// # Errors
///
/// When `token` is not a number.
pub fn canonical_value(token: &str) -> TestResult<String> {
    let f: f64 = token
        .parse()
        .map_err(|e| format!("sample value {token:?}: {e}"))?;
    Ok(if f.is_nan() {
        "NaN".to_owned()
    } else if f == 0.0 {
        "0".to_owned()
    } else {
        format!("{:016x}", f.to_bits())
    })
}

/// A sample line split as Go's `parseSample` splits it: the metric name, the label pairs as
/// `<hex name>=<hex unescaped value>`, and the value token.
type Sample<'a> = (&'a str, Vec<String>, &'a str);

/// Go: `parseSample`.
fn parse_sample(line: &str) -> TestResult<Sample<'_>> {
    let (head, value) = line.rsplit_once(' ').ok_or("no value")?;
    let Some(open) = head.find('{') else {
        return Ok((head, Vec::new(), value));
    };
    let name = head.get(..open).ok_or("bad name")?;
    let inner = head
        .strip_suffix('}')
        .and_then(|h| h.get(open + 1..))
        .ok_or("unterminated labels")?;
    let mut rest = inner.as_bytes();
    let mut pairs = Vec::new();
    while !rest.is_empty() {
        let eq = rest
            .iter()
            .position(|&b| b == b'=')
            .filter(|&eq| rest.get(eq + 1) == Some(&b'"'))
            .ok_or_else(|| format!("bad label at {:?}", String::from_utf8_lossy(rest)))?;
        let mut end = eq + 2;
        while let Some(&b) = rest.get(end) {
            if b == b'"' {
                break;
            }
            if b == b'\\' {
                end += 1;
            }
            end += 1;
        }
        let (Some(label), Some(escaped), Some(after)) =
            (rest.get(..eq), rest.get(eq + 2..end), rest.get(end + 1..))
        else {
            return Err(format!(
                "unterminated label value at {:?}",
                String::from_utf8_lossy(rest)
            )
            .into());
        };
        pairs.push(format!("{}={}", hex(label), hex(&unescape(escaped, true))));
        rest = match after.strip_prefix(b",") {
            Some(next) => next,
            None if after.is_empty() => after,
            None => {
                return Err(format!(
                    "bad label separator at {:?}",
                    String::from_utf8_lossy(after)
                )
                .into());
            }
        };
    }
    Ok((name, pairs, value))
}

/// The counts and digests `semantics.txt` records for a complete scrape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Digests {
    /// The number of families (HELP or TYPE names).
    pub families: usize,
    /// The number of sample lines.
    pub samples: usize,
    /// The FNV-1a 64 digest of the families (name, TYPE and unescaped HELP) and samples (name,
    /// unescaped label set and canonical value), sorted: what the scrape says, whatever its
    /// order and spelling.
    pub semantic: u64,
    /// The FNV-1a 64 digest of the lines in their order, each sample value in canonical form:
    /// everything but the spelling of the values.
    pub values: u64,
}

/// A scrape read back as Go's generator reads one (`readExposition`).
#[derive(Debug, Default)]
pub struct Exposition {
    help: BTreeMap<String, String>,
    typ: BTreeMap<String, String>,
    samples: Vec<String>,
    lines: Vec<String>,
}

impl Exposition {
    /// Go: `readExposition`.
    ///
    /// # Errors
    ///
    /// When a sample line cannot be parsed.
    pub fn read(text: &str) -> TestResult<Self> {
        let mut e = Self::default();
        for piece in text.split_inclusive('\n') {
            let line = piece.strip_suffix('\n').unwrap_or(piece);
            if let Some(rest) = line.strip_prefix("# HELP ") {
                let (name, help) = rest.split_once(' ').unwrap_or((rest, ""));
                let help = String::from_utf8(unescape(help.as_bytes(), false))?;
                drop(e.help.insert(name.to_owned(), help));
                e.lines.push(line.to_owned());
            } else if let Some(rest) = line.strip_prefix("# TYPE ") {
                let (name, typ) = rest.split_once(' ').unwrap_or((rest, ""));
                drop(e.typ.insert(name.to_owned(), typ.to_owned()));
                e.lines.push(line.to_owned());
            } else {
                let (name, mut pairs, value) =
                    parse_sample(line).map_err(|err| format!("{line:?}: {err}"))?;
                let canonical = canonical_value(value).map_err(|err| format!("{line:?}: {err}"))?;
                pairs.sort();
                e.samples.push(format!(
                    "S {} {} {canonical}",
                    hex(name.as_bytes()),
                    pairs.join(",")
                ));
                let head = line.rsplit_once(' ').map_or(line, |(head, _)| head);
                e.lines.push(format!("{head} {canonical}"));
            }
        }
        Ok(e)
    }

    /// Go: `digests`.
    #[must_use]
    pub fn digests(&self) -> Digests {
        let names: BTreeSet<&String> = self.help.keys().chain(self.typ.keys()).collect();
        let mut records: Vec<String> = names
            .iter()
            .map(|name| {
                format!(
                    "F {} {} {}",
                    hex(name.as_bytes()),
                    self.typ.get(*name).map_or("", String::as_str),
                    hex(self.help.get(*name).map_or("", String::as_str).as_bytes())
                )
            })
            .collect();
        records.sort();
        let mut samples = self.samples.clone();
        samples.sort();
        records.extend(samples);
        Digests {
            families: names.len(),
            samples: self.samples.len(),
            semantic: fnv1a64(format!("{}\n", records.join("\n")).as_bytes()),
            values: fnv1a64(format!("{}\n", self.lines.join("\n")).as_bytes()),
        }
    }
}

/// `semantics.txt`: the digests of each scenario's complete scrape, or `None` where Go's
/// `NewMetrics` panicked.
///
/// # Errors
///
/// When the golden cannot be read or a line is malformed.
pub fn semantics() -> TestResult<BTreeMap<String, Option<Digests>>> {
    let mut out = BTreeMap::new();
    for line in golden("semantics.txt")?.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let (name, digests) = match fields.as_slice() {
            [name, "panic"] => (*name, None),
            [name, families, samples, semantic, values] => (
                *name,
                Some(Digests {
                    families: families.parse()?,
                    samples: samples.parse()?,
                    semantic: u64::from_str_radix(semantic, 16)?,
                    values: u64::from_str_radix(values, 16)?,
                }),
            ),
            _ => return Err(format!("malformed semantics line {line:?}").into()),
        };
        if out.insert(name.to_owned(), digests).is_some() {
            return Err(format!("{name} is listed twice").into());
        }
    }
    Ok(out)
}

/// `digests.txt`: the length and FNV-1a 64 digest of each scenario's complete scrape.
///
/// # Errors
///
/// When the golden cannot be read or a line is malformed.
pub fn full_digests() -> TestResult<BTreeMap<String, (usize, u64)>> {
    let mut out = BTreeMap::new();
    for line in golden("digests.txt")?.lines() {
        let [name, len, hash] = line.split(' ').collect::<Vec<_>>()[..] else {
            return Err(format!("malformed digest line {line:?}").into());
        };
        let digest = (len.parse::<usize>()?, u64::from_str_radix(hash, 16)?);
        if out.insert(name.to_owned(), digest).is_some() {
            return Err(format!("{name} is listed twice").into());
        }
    }
    Ok(out)
}

/// The lines where a Rust scrape spells a sample value otherwise than Go's, as (Go's value, the
/// Rust value).
///
/// # Errors
///
/// When the scrapes differ in anything else: the number of lines, a HELP or TYPE line, a name or
/// label set, or a value.
pub fn value_spellings(go: &str, rust: &str) -> TestResult<Vec<(String, String)>> {
    check_eq(
        &rust.ends_with('\n'),
        &go.ends_with('\n'),
        "the scrape ends with a newline",
    )?;
    let go_lines: Vec<&str> = go.lines().collect();
    let rust_lines: Vec<&str> = rust.lines().collect();
    check_eq(&rust_lines.len(), &go_lines.len(), "the number of lines")?;
    let mut spellings = Vec::new();
    for (n, (go_line, rust_line)) in go_lines.iter().zip(&rust_lines).enumerate() {
        if go_line == rust_line {
            continue;
        }
        let differs = || format!("line {}: Go {go_line:?}, Rust {rust_line:?}", n + 1);
        let (Some((go_head, go_value)), Some((rust_head, rust_value))) =
            (go_line.rsplit_once(' '), rust_line.rsplit_once(' '))
        else {
            return Err(differs().into());
        };
        if go_line.starts_with('#')
            || go_head != rust_head
            || canonical_value(go_value)? != canonical_value(rust_value)?
        {
            return Err(differs().into());
        }
        spellings.push((go_value.to_owned(), rust_value.to_owned()));
    }
    Ok(spellings)
}

/// Go's `strings.ToLower` of every rune it changes (`to_lower.txt`).
///
/// # Errors
///
/// When the golden cannot be read or a line is malformed.
pub fn go_to_lower() -> TestResult<BTreeMap<char, char>> {
    let rune = |hex: &str| -> TestResult<char> {
        char::from_u32(u32::from_str_radix(hex, 16)?)
            .ok_or_else(|| format!("{hex} is not a rune").into())
    };
    let mut table = BTreeMap::new();
    for line in golden("to_lower.txt")?.lines() {
        let (from, to) = line.split_once(' ').ok_or("malformed line")?;
        if table.insert(rune(from)?, rune(to)?).is_some() {
            return Err(format!("{from} is listed twice").into());
        }
    }
    Ok(table)
}

/// The ledgered `metrics.label-unicode` difference, undone: for each letter Go's Unicode 15.0.0
/// tables keep and std's later ones lower, std's lower case mapped back to the letter.
#[derive(Debug)]
pub struct LabelUnicode {
    back: BTreeMap<char, char>,
}

impl LabelUnicode {
    /// The letters of std's tables that Go's `to_lower.txt` does not lower.
    ///
    /// # Errors
    ///
    /// When the golden cannot be read, std lowers such a letter to more than one character, or
    /// two letters share a lower case.
    pub fn new() -> TestResult<Self> {
        let go = go_to_lower()?;
        let mut back = BTreeMap::new();
        for c in (0..=0x10_FFFF_u32).filter_map(char::from_u32) {
            if go.contains_key(&c) {
                continue;
            }
            let mut lower = c.to_lowercase();
            let (Some(first), rest) = (lower.next(), lower.next()) else {
                continue;
            };
            if first == c {
                continue;
            }
            check(
                rest.is_none(),
                &format!("std lowers U+{:04X} to one character", u32::from(c)),
            )?;
            if let Some(other) = back.insert(first, c) {
                return Err(format!(
                    "U+{:04X} and U+{:04X} share a lower case",
                    u32::from(other),
                    u32::from(c)
                )
                .into());
            }
        }
        Ok(Self { back })
    }

    /// The pairs (std's lower case, Go's unchanged letter).
    #[must_use]
    pub const fn pairs(&self) -> &BTreeMap<char, char> {
        &self.back
    }

    /// Whether `text` holds a character std's tables give as a lower case Go has none for.
    #[must_use]
    pub fn touches(&self, text: &str) -> bool {
        text.chars().any(|c| self.back.contains_key(&c))
    }

    /// `text` with each such lower case mapped back to Go's letter. Only an adapter label
    /// lowered by the engine can hold one when no input does.
    #[must_use]
    pub fn undo(&self, text: &str) -> String {
        text.chars()
            .map(|c| self.back.get(&c).copied().unwrap_or(c))
            .collect()
    }
}

/// Go's `strconv.Unquote` of the quoted string `s` starts with, as `%q` writes it, with the rest
/// of `s` after the closing quote.
///
/// # Errors
///
/// When `s` does not start with a quoted string `%q` writes.
pub fn go_unquote(text: &str) -> TestResult<(String, &str)> {
    let body = text.strip_prefix('"').ok_or("no opening quote")?;
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut pos = 0;
    loop {
        let &byte = bytes.get(pos).ok_or("no closing quote")?;
        if byte == b'"' {
            let rest = body.get(pos + 1..).ok_or("no rest")?;
            return Ok((String::from_utf8(out)?, rest));
        }
        if byte != b'\\' {
            out.push(byte);
            pos += 1;
            continue;
        }
        let &escape = bytes.get(pos + 1).ok_or("a cut escape")?;
        let simple = match escape {
            b'a' => Some(0x07),
            b'b' => Some(0x08),
            b'f' => Some(0x0C),
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b't' => Some(b'\t'),
            b'v' => Some(0x0B),
            b'\\' => Some(b'\\'),
            b'"' => Some(b'"'),
            _ => None,
        };
        if let Some(unescaped) = simple {
            out.push(unescaped);
            pos += 2;
            continue;
        }
        let digits = match escape {
            b'x' => 2,
            b'u' => 4,
            b'U' => 8,
            _ => return Err(format!("unknown escape {:?}", char::from(escape)).into()),
        };
        let hex = body.get(pos + 2..pos + 2 + digits).ok_or("a cut escape")?;
        let value = u32::from_str_radix(hex, 16)?;
        if escape == b'x' {
            out.push(u8::try_from(value)?);
        } else {
            let rune = char::from_u32(value).ok_or("an escape of no rune")?;
            out.extend_from_slice(rune.encode_utf8(&mut [0; 4]).as_bytes());
        }
        pos += 2 + digits;
    }
}
