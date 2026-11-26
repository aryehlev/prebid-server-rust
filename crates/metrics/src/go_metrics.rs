//! The go-metrics engine, which the `InfluxDB` reporter sends.
//!
//! Go: `metrics/go_metrics.go`. The metrics live in a [`Registry`] under Go's names
//! (`requests.ok.openrtb2-web`, `adapter.appnexus.request_time`,
//! `account.<id>.adapter.<bidder>.prices`, ...); the metrics of an account are registered the
//! first time the account is recorded.
//!
//! The metric handles are private to this crate. Go code outside the package reads the exported
//! fields only in tests; Rust callers read the metrics by name through
//! [`Metrics::metrics_registry`].
//!
//! # Differences from Go
//!
//! Where Go indexes a map with a label outside the values [`new_metrics`] registers (an unknown
//! request type or status, cache result, adapter error, stored data type, overhead type, or an
//! adapter missing from the list for the dial metrics), it calls a method on a nil metric and
//! panics; the port returns there, keeping what the call recorded before. Go ranges over the
//! adapter error set in random order; the port marks the errors in ascending order (and stops at
//! the first unknown one). Go leaves the `tmax_timeout` counter of [`new_blank_metrics`] nil, so
//! recording into it panics; the port's records nothing.

use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, PoisonError, RwLock},
};

use crate::{
    gometrics::{
        Counter, Histogram, Meter, Registry, Timer, new_exp_decay_sample, new_uniform_sample,
    },
    i64_from_float,
    metrics::{
        self, AdapterBid, AdapterError, AdapterLabels, CacheResult, CookieFlag, CookieSyncStatus,
        DemandSource, EndpointType, FetcherBackendResult, FetcherOperation, FetcherResult,
        GoDuration, ImpLabels, ImpMediaType, Labels, MetricsEngine, ModuleLabels, OverheadType,
        PUBLISHER_UNKNOWN, PrivacyLabels, RequestStatus, RequestType, SetUidStatus,
        StoredDataError, StoredDataFetchType, StoredDataLabels, StoredDataType,
        SyncerCookieSyncStatus, SyncerSetUidStatus, TcfVersionValue,
        get_endpoint_from_request_type,
    },
    options::{DisabledMetric, DisabledMetrics},
    prometheus::to_lower,
};

/// Go: `openrtb_ext.BidTypeBanner`, `BidTypeVideo`, `BidTypeAudio` and `BidTypeNative`, the keys
/// of the markup delivery metrics.
const BID_TYPES: [&str; 4] = ["banner", "video", "audio", "native"];

/// Go: `NewExpDecaySample(1028, 0.015)`, the sample of the price histograms.
const PRICE_RESERVOIR_SIZE: usize = 1028;
/// Go: the alpha of the price histograms' sample.
const PRICE_ALPHA: f64 = 0.015;

/// Go: `int64(n)` for a Go `int` the port holds as `usize`. A Go `int` cannot exceed
/// `i64::MAX`; a larger `usize` saturates.
fn go_int64(n: usize) -> i64 {
    u64::try_from(n).map_or(i64::MAX, |v| i64::try_from(v).unwrap_or(i64::MAX))
}

/// Go: `metrics.Metrics`, the go-metrics implementation of [`MetricsEngine`].
///
/// [`Default`] is Go's zero `Metrics{}` with every metric blank (Go's would be nil, and panic
/// when recorded into).
#[derive(Default)]
pub struct Metrics {
    /// Go: `MetricsRegistry`.
    pub(crate) registry: Registry,
    /// Go: `ConnectionCounter`.
    pub(crate) connection_counter: Counter,
    /// Go: `TMaxTimeoutCounter`.
    pub(crate) tmax_timeout_counter: Counter,
    /// Go: `ConnectionAcceptErrorMeter`.
    pub(crate) connection_accept_error_meter: Meter,
    /// Go: `ConnectionCloseErrorMeter`.
    pub(crate) connection_close_error_meter: Meter,
    /// Go: `ImpMeter`.
    pub(crate) imp_meter: Meter,
    /// Go: `AppRequestMeter`.
    pub(crate) app_request_meter: Meter,
    /// Go: `NoCookieMeter`.
    pub(crate) no_cookie_meter: Meter,
    /// Go: `DebugRequestMeter`.
    pub(crate) debug_request_meter: Meter,
    /// Go: `RequestTimer`.
    pub(crate) request_timer: Timer,
    /// Go: `RequestsQueueTimer`.
    pub(crate) requests_queue_timer: BTreeMap<RequestType, BTreeMap<bool, Timer>>,
    /// Go: `PrebidCacheRequestTimerSuccess`.
    pub(crate) prebid_cache_request_timer_success: Timer,
    /// Go: `PrebidCacheRequestTimerError`.
    pub(crate) prebid_cache_request_timer_error: Timer,
    /// Go: `StoredDataFetchTimer`.
    pub(crate) stored_data_fetch_timer:
        BTreeMap<StoredDataType, BTreeMap<StoredDataFetchType, Timer>>,
    /// Go: `StoredDataErrorMeter`.
    pub(crate) stored_data_error_meter: BTreeMap<StoredDataType, BTreeMap<StoredDataError, Meter>>,
    /// Go: `StoredReqCacheMeter`.
    pub(crate) stored_req_cache_meter: BTreeMap<CacheResult, Meter>,
    /// Go: `StoredImpCacheMeter`.
    pub(crate) stored_imp_cache_meter: BTreeMap<CacheResult, Meter>,
    /// Go: `AccountCacheMeter`.
    pub(crate) account_cache_meter: BTreeMap<CacheResult, Meter>,
    /// Go: `DNSLookupTimer`.
    pub(crate) dns_lookup_timer: Timer,
    /// Go: `TLSHandshakeTimer`.
    pub(crate) tls_handshake_timer: Timer,
    /// Go: `BidderServerResponseTimer`.
    pub(crate) bidder_server_response_timer: Timer,
    /// Go: `StoredResponsesMeter`.
    pub(crate) stored_responses_meter: Meter,
    /// Go: `GvlListRequestsMeter`.
    pub(crate) gvl_list_requests_meter: Meter,
    /// Go: `LiveGVLFetchSuccess`.
    pub(crate) live_gvl_fetch_success: Meter,
    /// Go: `LiveGVLFetchFailure`.
    pub(crate) live_gvl_fetch_failure: Meter,

    /// Go: `RequestStatuses`, the request metrics by request type and status.
    pub(crate) request_statuses: BTreeMap<RequestType, BTreeMap<RequestStatus, Meter>>,
    /// Go: `RequestSizeByEndpoint`.
    pub(crate) request_size_by_endpoint: BTreeMap<EndpointType, Histogram>,
    /// Go: `CookieSyncMeter`.
    pub(crate) cookie_sync_meter: Meter,
    /// Go: `CookieSyncStatusMeter`.
    pub(crate) cookie_sync_status_meter: BTreeMap<CookieSyncStatus, Meter>,
    /// Go: `SyncerRequestsMeter`.
    pub(crate) syncer_requests_meter: BTreeMap<String, BTreeMap<SyncerCookieSyncStatus, Meter>>,
    /// Go: `SetUidMeter`.
    pub(crate) set_uid_meter: Meter,
    /// Go: `SetUidStatusMeter`.
    pub(crate) set_uid_status_meter: BTreeMap<SetUidStatus, Meter>,
    /// Go: `SyncerSetsMeter`.
    pub(crate) syncer_sets_meter: BTreeMap<String, BTreeMap<SyncerSetUidStatus, Meter>>,

    /// Go: `ImpsTypeBanner`, a media type found in the `imp` JSON object.
    pub(crate) imps_type_banner: Meter,
    /// Go: `ImpsTypeVideo`.
    pub(crate) imps_type_video: Meter,
    /// Go: `ImpsTypeAudio`.
    pub(crate) imps_type_audio: Meter,
    /// Go: `ImpsTypeNative`.
    pub(crate) imps_type_native: Meter,

    /// Go: `TimeoutNotificationSuccess`.
    pub(crate) timeout_notification_success: Meter,
    /// Go: `TimeoutNotificationFailure`.
    pub(crate) timeout_notification_failure: Meter,

    /// Go: `PrivacyCCPARequest`.
    pub(crate) privacy_ccpa_request: Meter,
    /// Go: `PrivacyCCPARequestOptOut`.
    pub(crate) privacy_ccpa_request_opt_out: Meter,
    /// Go: `PrivacyCOPPARequest`.
    pub(crate) privacy_coppa_request: Meter,
    /// Go: `PrivacyLMTRequest`.
    pub(crate) privacy_lmt_request: Meter,
    /// Go: `PrivacyTCFRequestVersion`.
    pub(crate) privacy_tcf_request_version: BTreeMap<TcfVersionValue, Meter>,

    /// Go: `AdapterMetrics`, by lower-cased adapter name.
    pub(crate) adapters: BTreeMap<String, AdapterMetrics>,
    /// Go: `accountMetrics`, behind `accountMetricsRWMutex`.
    pub(crate) accounts: RwLock<BTreeMap<String, Arc<AccountMetrics>>>,

    /// Go: `exchanges`, the (lower-cased) adapter names.
    pub(crate) exchanges: Vec<String>,
    /// Go: `modules`, the module names.
    pub(crate) modules: Vec<String>,
    /// Go: `MetricsDisabled`.
    pub(crate) disabled: DisabledMetrics,

    /// Go: `AdsCertRequestsSuccess`.
    pub(crate) ads_cert_requests_success: Meter,
    /// Go: `AdsCertRequestsFailure`.
    pub(crate) ads_cert_requests_failure: Meter,
    /// Go: `adsCertSignTimer`.
    pub(crate) ads_cert_sign_timer: Timer,

    /// Go: `ModuleMetrics`, by module and stage.
    pub(crate) module_stages: BTreeMap<String, BTreeMap<String, ModuleMetrics>>,

    /// Go: `OverheadTimer`.
    pub(crate) overhead_timer: BTreeMap<OverheadType, Timer>,
}

impl fmt::Debug for Metrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Metrics")
            .field("registry", &self.registry)
            .field("exchanges", &self.exchanges)
            .field("modules", &self.modules)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

/// Go: `metrics.AdapterMetrics`, the metrics of one adapter (or of one adapter for one account).
#[derive(Clone, Debug, Default)]
pub(crate) struct AdapterMetrics {
    /// Go: `NoCookieMeter`.
    pub(crate) no_cookie_meter: Meter,
    /// Go: `ErrorMeters`.
    pub(crate) error_meters: BTreeMap<AdapterError, Meter>,
    /// Go: `NoBidMeter`.
    pub(crate) no_bid_meter: Meter,
    /// Go: `GotBidsMeter`.
    pub(crate) got_bids_meter: Meter,
    /// Go: `RequestTimer`.
    pub(crate) request_timer: Timer,
    /// Go: `PriceHistogram`.
    pub(crate) price_histogram: Histogram,
    /// Go: `BidsReceivedMeter` (blank for the adapter itself, registered per account).
    pub(crate) bids_received_meter: Meter,
    /// Go: `PanicMeter`.
    pub(crate) panic_meter: Meter,
    /// Go: `MarkupMetrics`, by bid type.
    pub(crate) markup_metrics: BTreeMap<String, MarkupDeliveryMetrics>,
    /// Go: `ConnCreated`.
    pub(crate) conn_created: Counter,
    /// Go: `ConnReused`.
    pub(crate) conn_reused: Counter,
    /// Go: `ConnWaitTime`.
    pub(crate) conn_wait_time: Timer,
    /// Go: `ConnDialErrors`.
    pub(crate) conn_dial_errors: Counter,
    /// Go: `ConnDialTime`.
    pub(crate) conn_dial_time: Timer,
    /// Go: `BuyerUIDScrubbed`.
    pub(crate) buyer_uid_scrubbed: Meter,
    /// Go: `GDPRRequestBlocked`.
    pub(crate) gdpr_request_blocked: Meter,
    /// Go: `ThrottledMeter`.
    pub(crate) throttled_meter: Meter,
    /// Go: `BidValidationCreativeSizeErrorMeter`.
    pub(crate) bid_validation_creative_size_error_meter: Meter,
    /// Go: `BidValidationCreativeSizeWarnMeter`.
    pub(crate) bid_validation_creative_size_warn_meter: Meter,
    /// Go: `BidValidationSecureMarkupErrorMeter`.
    pub(crate) bid_validation_secure_markup_error_meter: Meter,
    /// Go: `BidValidationSecureMarkupWarnMeter`.
    pub(crate) bid_validation_secure_markup_warn_meter: Meter,
}

/// Go: `metrics.MarkupDeliveryMetrics`.
#[derive(Clone, Debug, Default)]
pub(crate) struct MarkupDeliveryMetrics {
    /// Go: `AdmMeter`.
    pub(crate) adm_meter: Meter,
    /// Go: `NurlMeter`.
    pub(crate) nurl_meter: Meter,
}

/// Go: `accountMetrics`. Go also holds `bidsReceivedMeter` and `priceHistogram`, which it
/// registers and never records into; the port registers them only.
#[derive(Debug, Default)]
pub(crate) struct AccountMetrics {
    /// Go: `requestMeter`.
    pub(crate) request_meter: Meter,
    /// Go: `debugRequestMeter`.
    pub(crate) debug_request_meter: Meter,
    /// Go: `adapterMetrics`, by lower-cased adapter name.
    pub(crate) adapters: BTreeMap<String, AdapterMetrics>,
    /// Go: `moduleMetrics`, by module name.
    pub(crate) modules: BTreeMap<String, ModuleMetrics>,
    /// Go: `storedResponsesMeter`.
    pub(crate) stored_responses_meter: Meter,
    /// Go: `bidValidationCreativeSizeMeter`.
    pub(crate) bid_validation_creative_size_meter: Meter,
    /// Go: `bidValidationCreativeSizeWarnMeter`.
    pub(crate) bid_validation_creative_size_warn_meter: Meter,
    /// Go: `bidValidationSecureMarkupMeter`.
    pub(crate) bid_validation_secure_markup_meter: Meter,
    /// Go: `bidValidationSecureMarkupWarnMeter`.
    pub(crate) bid_validation_secure_markup_warn_meter: Meter,
}

/// Go: `metrics.ModuleMetrics`.
#[derive(Clone, Debug, Default)]
pub(crate) struct ModuleMetrics {
    /// Go: `DurationTimer`.
    pub(crate) duration_timer: Timer,
    /// Go: `CallCounter`.
    pub(crate) call_counter: Counter,
    /// Go: `FailureCounter`.
    pub(crate) failure_counter: Counter,
    /// Go: `SuccessNoopCounter`.
    pub(crate) success_noop_counter: Counter,
    /// Go: `SuccessUpdateCounter`.
    pub(crate) success_update_counter: Counter,
    /// Go: `SuccessRejectCounter`.
    pub(crate) success_reject_counter: Counter,
    /// Go: `ExecutionErrorCounter`.
    pub(crate) execution_error_counter: Counter,
    /// Go: `TimeoutCounter`.
    pub(crate) timeout_counter: Counter,
}

/// Creates the engine with every metric blank (Go's `Nil*` metrics), so that nothing is recorded
/// anywhere.
///
/// Go: `metrics.NewBlankMetrics(registry, exchanges, disabledMetrics, moduleStageNames)`.
/// `exchanges` are used as given ([`new_metrics`] lower-cases them first); an adapter listed twice
/// has one entry.
#[must_use]
pub fn new_blank_metrics(
    registry: Registry,
    exchanges: &[String],
    disabled_metrics: DisabledMetrics,
    module_stage_names: &BTreeMap<String, Vec<String>>,
) -> Metrics {
    // Go: go_metrics.go:158
    let mut new_metrics = Metrics {
        registry,
        disabled: disabled_metrics,
        exchanges: exchanges.to_vec(),
        modules: get_module_names(module_stage_names),
        overhead_timer: make_blank_overhead_timer_metrics(),
        ..Metrics::default()
    };

    for a in exchanges {
        drop(
            new_metrics
                .adapters
                .insert(a.clone(), make_blank_adapter_metrics()),
        );
    }

    for (module, stages) in module_stage_names {
        drop(
            new_metrics
                .module_stages
                .insert(module.clone(), make_blank_module_stage_metrics(stages)),
        );
    }

    for &t in metrics::request_types() {
        let statuses = metrics::request_statuses()
            .iter()
            .map(|&s| (s, Meter::NIL))
            .collect();
        drop(new_metrics.request_statuses.insert(t, statuses));
    }

    for &t in metrics::endpoint_types() {
        if t != EndpointType::AMP {
            drop(
                new_metrics
                    .request_size_by_endpoint
                    .insert(t, Histogram::NIL),
            );
        }
    }

    for &c in metrics::cache_results() {
        drop(new_metrics.stored_req_cache_meter.insert(c, Meter::NIL));
        drop(new_metrics.stored_imp_cache_meter.insert(c, Meter::NIL));
        drop(new_metrics.account_cache_meter.insert(c, Meter::NIL));
    }

    for &v in metrics::tcf_versions() {
        drop(
            new_metrics
                .privacy_tcf_request_version
                .insert(v, Meter::NIL),
        );
    }

    for &dt in metrics::stored_data_types() {
        let fetch_timers = metrics::stored_data_fetch_types()
            .iter()
            .map(|&ft| (ft, Timer::NIL))
            .collect();
        drop(new_metrics.stored_data_fetch_timer.insert(dt, fetch_timers));
        let error_meters = metrics::stored_data_errors()
            .iter()
            .map(|&e| (e, Meter::NIL))
            .collect();
        drop(new_metrics.stored_data_error_meter.insert(dt, error_meters));
    }

    // To minimize memory usage, the queued request metric is supported for the video endpoint
    // only; the boolean is the two general request statuses, accepted and rejected.
    drop(new_metrics.requests_queue_timer.insert(
        RequestType::VIDEO,
        BTreeMap::from([(true, Timer::NIL), (false, Timer::NIL)]),
    ));
    new_metrics
}

/// Go: `getModuleNames`, which lists the names in the (random) order of the map; the port's are
/// ascending.
fn get_module_names(module_stage_names: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    // Go: go_metrics.go:277
    module_stage_names.keys().cloned().collect()
}

/// Creates the engine with every metric registered in `registry`.
///
/// Go: `metrics.NewMetrics(registry, exchanges, disableAccountMetrics, syncerKeys,
/// moduleStageNames)`. The adapter names are lower-cased (Go: `strings.ToLower`).
#[must_use]
pub fn new_metrics(
    registry: Registry,
    exchanges: &[String],
    disable_account_metrics: DisabledMetrics,
    syncer_keys: &[String],
    module_stage_names: &BTreeMap<String, Vec<String>>,
) -> Metrics {
    // Go: go_metrics.go:294
    let lower_case_exchanges: Vec<String> = exchanges.iter().map(|e| to_lower(e)).collect();
    let mut new_metrics = new_blank_metrics(
        registry,
        &lower_case_exchanges,
        disable_account_metrics,
        module_stage_names,
    );
    let r = new_metrics.registry.clone();
    new_metrics.register_server_metrics(&r);
    new_metrics.register_stored_data_metrics(&r);
    // Go: go_metrics.go:337 (AmpNoCookieMeter, which nothing records into).
    let _amp_no_cookie_meter = r.get_or_register_meter("amp_no_cookie_requests");
    new_metrics.register_sync_metrics(&r, syncer_keys);

    for a in &lower_case_exchanges {
        if let Some(am) = new_metrics.adapters.get_mut(a) {
            register_adapter_metrics(&r, "adapter", a, am);
        }
    }

    new_metrics.register_request_metrics(&r);
    new_metrics.register_privacy_metrics(&r);

    for (module, stages) in module_stage_names {
        if let Some(mm) = new_metrics.module_stages.get_mut(module) {
            register_module_metrics(&r, module, stages, mm);
        }
    }

    new_metrics
}

/// Go: `makeBlankOverheadTimerMetrics`.
fn make_blank_overhead_timer_metrics() -> BTreeMap<OverheadType, Timer> {
    metrics::overhead_types()
        .iter()
        .map(|&overhead| (overhead, Timer::NIL))
        .collect()
}

/// Go: `makeBlankAdapterMetrics(disabledMetrics)`, the blank metrics of one adapter.
///
/// Go leaves the bid validation meters nil, and the connection, buyer UID and GDPR metrics nil
/// when they are disabled (blank otherwise); recording into a nil one panics in Go. The port
/// makes all of them blank, which record nothing.
fn make_blank_adapter_metrics() -> AdapterMetrics {
    // Go: go_metrics.go:416
    AdapterMetrics {
        markup_metrics: make_blank_bid_markup_metrics(),
        error_meters: metrics::adapter_errors()
            .iter()
            .map(|&err| (err, Meter::NIL))
            .collect(),
        ..AdapterMetrics::default()
    }
}

/// Go: `makeBlankModuleStageMetrics`.
fn make_blank_module_stage_metrics(stages: &[String]) -> BTreeMap<String, ModuleMetrics> {
    // Go: go_metrics.go:449
    stages
        .iter()
        .map(|stage| (stage.clone(), ModuleMetrics::default()))
        .collect()
}

/// Go: `makeBlankBidMarkupMetrics`.
fn make_blank_bid_markup_metrics() -> BTreeMap<String, MarkupDeliveryMetrics> {
    // Go: go_metrics.go:471
    BID_TYPES
        .iter()
        .map(|&bid_type| (bid_type.to_owned(), MarkupDeliveryMetrics::default()))
        .collect()
}

/// Go: `makeOverheadTimerMetrics`.
fn make_overhead_timer_metrics(registry: &Registry) -> BTreeMap<OverheadType, Timer> {
    // Go: go_metrics.go:487
    metrics::overhead_types()
        .iter()
        .map(|&overhead| {
            let name = format!("request_over_head_time.{overhead}");
            (overhead, registry.get_or_register_timer(&name))
        })
        .collect()
}

/// Go: `registerAdapterMetrics`: registers the metrics of `exchange` under
/// `<adapter_or_account>.<exchange>.` into `am`.
fn register_adapter_metrics(
    registry: &Registry,
    adapter_or_account: &str,
    exchange: &str,
    am: &mut AdapterMetrics,
) {
    // Go: go_metrics.go:496
    let prefix = format!("{adapter_or_account}.{exchange}");
    let meter = |suffix: &str| registry.get_or_register_meter(&format!("{prefix}.{suffix}"));
    let timer = |suffix: &str| registry.get_or_register_timer(&format!("{prefix}.{suffix}"));
    let counter = |suffix: &str| registry.get_or_register_counter(&format!("{prefix}.{suffix}"));

    am.no_cookie_meter = meter("no_cookie_requests");
    am.no_bid_meter = meter("requests.nobid");
    am.got_bids_meter = meter("requests.gotbids");
    am.request_timer = timer("request_time");
    am.price_histogram = registry.get_or_register_histogram(
        &format!("{prefix}.prices"),
        new_exp_decay_sample(PRICE_RESERVOIR_SIZE, PRICE_ALPHA),
    );
    am.markup_metrics = BID_TYPES
        .iter()
        .map(|&bid_type| {
            let delivery = make_delivery_metrics(registry, &prefix, bid_type);
            (bid_type.to_owned(), delivery)
        })
        .collect();
    am.conn_created = counter("connections_created");
    am.conn_reused = counter("connections_reused");
    am.conn_wait_time = timer("connection_wait_time");
    am.conn_dial_errors = counter("connection_dial_err");
    am.conn_dial_time = timer("connection_dial_time");

    for (err, error_meter) in &mut am.error_meters {
        *error_meter = meter(&format!("requests.{err}"));
    }
    if adapter_or_account != "adapter" {
        am.bids_received_meter = meter("bids_received");
    }
    am.panic_meter = meter("requests.panic");
    am.buyer_uid_scrubbed = meter("buyeruid_scrubbed");
    am.gdpr_request_blocked = meter("gdpr_request_blocked");
    am.throttled_meter = meter("requests.throttled");

    am.bid_validation_creative_size_error_meter = meter("response.validation.size.err");
    am.bid_validation_creative_size_warn_meter = meter("response.validation.size.warn");

    am.bid_validation_secure_markup_error_meter = meter("response.validation.secure.err");
    am.bid_validation_secure_markup_warn_meter = meter("response.validation.secure.warn");
}

/// Go: `registerModuleMetrics`.
fn register_module_metrics(
    registry: &Registry,
    module: &str,
    stages: &[String],
    mm: &mut BTreeMap<String, ModuleMetrics>,
) {
    // Go: go_metrics.go:532
    for stage in stages {
        if let Some(metrics) = mm.get_mut(stage) {
            let prefix = format!("modules.module.{module}.stage.{stage}");
            fill_module_metrics(registry, &prefix, metrics);
        }
    }
}

/// Go: `registerAccountModuleMetrics`.
fn register_account_module_metrics(
    registry: &Registry,
    id: &str,
    module: &str,
    mm: &mut ModuleMetrics,
) {
    // Go: go_metrics.go:545
    let prefix = format!("account.{id}.modules.module.{module}");
    fill_module_metrics(registry, &prefix, mm);
}

/// The eight module metrics under `<prefix>.` (Go: the bodies of `registerModuleMetrics` and
/// `registerAccountModuleMetrics`).
fn fill_module_metrics(registry: &Registry, prefix: &str, mm: &mut ModuleMetrics) {
    let counter = |suffix: &str| registry.get_or_register_counter(&format!("{prefix}.{suffix}"));
    mm.duration_timer = registry.get_or_register_timer(&format!("{prefix}.duration"));
    mm.call_counter = counter("call");
    mm.failure_counter = counter("failure");
    mm.success_noop_counter = counter("success.noop");
    mm.success_update_counter = counter("success.update");
    mm.success_reject_counter = counter("success.reject");
    mm.execution_error_counter = counter("execution_error");
    mm.timeout_counter = counter("timeout");
}

/// Go: `makeDeliveryMetrics`.
fn make_delivery_metrics(
    registry: &Registry,
    prefix: &str,
    bid_type: &str,
) -> MarkupDeliveryMetrics {
    // Go: go_metrics.go:556
    let meter =
        |suffix: &str| registry.get_or_register_meter(&format!("{prefix}.{bid_type}.{suffix}"));
    MarkupDeliveryMetrics {
        adm_meter: meter("adm_bids_received"),
        nurl_meter: meter("nurl_bids_received"),
    }
}

impl Metrics {
    /// Go: `MetricsRegistry`, the registry the metrics are registered in.
    #[must_use]
    pub const fn metrics_registry(&self) -> &Registry {
        &self.registry
    }

    /// The server, request time and cache timing metrics of `NewMetrics`.
    fn register_server_metrics(&mut self, r: &Registry) {
        // Go: go_metrics.go:300
        self.connection_counter = r.get_or_register_counter("active_connections");
        self.tmax_timeout_counter = r.get_or_register_counter("tmax_timeout");
        self.connection_accept_error_meter = r.get_or_register_meter("connection_accept_errors");
        self.connection_close_error_meter = r.get_or_register_meter("connection_close_errors");
        self.imp_meter = r.get_or_register_meter("imps_requested");

        self.imps_type_banner = r.get_or_register_meter("imp_banner");
        self.imps_type_video = r.get_or_register_meter("imp_video");
        self.imps_type_audio = r.get_or_register_meter("imp_audio");
        self.imps_type_native = r.get_or_register_meter("imp_native");

        self.no_cookie_meter = r.get_or_register_meter("no_cookie_requests");
        self.app_request_meter = r.get_or_register_meter("app_requests");
        self.debug_request_meter = r.get_or_register_meter("debug_requests");
        self.request_timer = r.get_or_register_timer("request_time");
        self.dns_lookup_timer = r.get_or_register_timer("dns_lookup_time");
        self.tls_handshake_timer = r.get_or_register_timer("tls_handshake_time");
        self.prebid_cache_request_timer_success =
            r.get_or_register_timer("prebid_cache_request_time.ok");
        self.prebid_cache_request_timer_error =
            r.get_or_register_timer("prebid_cache_request_time.err");
        self.stored_responses_meter = r.get_or_register_meter("stored_responses");
        self.gvl_list_requests_meter = r.get_or_register_meter("gvl_requests");
        self.live_gvl_fetch_success = r.get_or_register_meter("live_gvl_fetch.ok");
        self.live_gvl_fetch_failure = r.get_or_register_meter("live_gvl_fetch.failed");
        self.overhead_timer = make_overhead_timer_metrics(r);
        self.bidder_server_response_timer =
            r.get_or_register_timer("bidder_server_response_time_seconds");
    }

    /// The stored data timers and meters of `NewMetrics`.
    fn register_stored_data_metrics(&mut self, r: &Registry) {
        // Go: go_metrics.go:326
        for &dt in metrics::stored_data_types() {
            let timers = self.stored_data_fetch_timer.entry(dt).or_default();
            for &ft in metrics::stored_data_fetch_types() {
                let timer_name = format!("stored_{dt}_fetch_time.{ft}");
                drop(timers.insert(ft, r.get_or_register_timer(&timer_name)));
            }
            let meters = self.stored_data_error_meter.entry(dt).or_default();
            for &e in metrics::stored_data_errors() {
                let meter_name = format!("stored_{dt}_error.{e}");
                drop(meters.insert(e, r.get_or_register_meter(&meter_name)));
            }
        }
    }

    /// The cookie sync and set UID metrics of `NewMetrics`.
    fn register_sync_metrics(&mut self, r: &Registry, syncer_keys: &[String]) {
        // Go: go_metrics.go:339
        self.cookie_sync_meter = r.get_or_register_meter("cookie_sync_requests");
        for &s in metrics::cookie_sync_statuses() {
            let meter = r.get_or_register_meter(&format!("cookie_sync_requests.{s}"));
            drop(self.cookie_sync_status_meter.insert(s, meter));
        }

        self.set_uid_meter = r.get_or_register_meter("setuid_requests");
        for &s in metrics::set_uid_statuses() {
            let meter = r.get_or_register_meter(&format!("setuid_requests.{s}"));
            drop(self.set_uid_status_meter.insert(s, meter));
        }

        for syncer_key in syncer_keys {
            let requests = metrics::syncer_request_statuses()
                .iter()
                .map(|&status| {
                    let name = format!("syncer.{syncer_key}.request.{status}");
                    (status, r.get_or_register_meter(&name))
                })
                .collect();
            drop(
                self.syncer_requests_meter
                    .insert(syncer_key.clone(), requests),
            );

            let sets = metrics::syncer_set_uid_statuses()
                .iter()
                .map(|&status| {
                    let name = format!("syncer.{syncer_key}.set.{status}");
                    (status, r.get_or_register_meter(&name))
                })
                .collect();
            drop(self.syncer_sets_meter.insert(syncer_key.clone(), sets));
        }
    }

    /// The request status, size, cache and queue metrics of `NewMetrics`.
    fn register_request_metrics(&mut self, r: &Registry) {
        // Go: go_metrics.go:365
        for (typ, status_map) in &mut self.request_statuses {
            for (stat, meter) in status_map.iter_mut() {
                *meter = r.get_or_register_meter(&format!("requests.{stat}.{typ}"));
            }
        }

        for &endpoint in metrics::endpoint_types() {
            let histogram = r.get_or_register_histogram(
                &format!("requests.size.{endpoint}"),
                new_uniform_sample(1024),
            );
            drop(self.request_size_by_endpoint.insert(endpoint, histogram));
        }

        for &cache_res in metrics::cache_results() {
            let stored_req = r.get_or_register_meter(&format!("stored_request_cache_{cache_res}"));
            drop(self.stored_req_cache_meter.insert(cache_res, stored_req));
            let stored_imp = r.get_or_register_meter(&format!("stored_imp_cache_{cache_res}"));
            drop(self.stored_imp_cache_meter.insert(cache_res, stored_imp));
            let account = r.get_or_register_meter(&format!("account_cache_{cache_res}"));
            drop(self.account_cache_meter.insert(cache_res, account));
        }

        let video = self
            .requests_queue_timer
            .entry(RequestType::VIDEO)
            .or_default();
        drop(video.insert(
            true,
            r.get_or_register_timer("queued_requests.video.accepted"),
        ));
        drop(video.insert(
            false,
            r.get_or_register_timer("queued_requests.video.rejected"),
        ));
    }

    /// The timeout notification, privacy and ads.cert metrics of `NewMetrics`.
    fn register_privacy_metrics(&mut self, r: &Registry) {
        // Go: go_metrics.go:384
        self.timeout_notification_success = r.get_or_register_meter("timeout_notification.ok");
        self.timeout_notification_failure = r.get_or_register_meter("timeout_notification.failed");

        self.privacy_ccpa_request = r.get_or_register_meter("privacy.request.ccpa.specified");
        self.privacy_ccpa_request_opt_out = r.get_or_register_meter("privacy.request.ccpa.opt-out");
        self.privacy_coppa_request = r.get_or_register_meter("privacy.request.coppa");
        self.privacy_lmt_request = r.get_or_register_meter("privacy.request.lmt");
        for &version in metrics::tcf_versions() {
            let meter = r.get_or_register_meter(&format!("privacy.request.tcf.{version}"));
            drop(self.privacy_tcf_request_version.insert(version, meter));
        }

        self.ads_cert_requests_success = r.get_or_register_meter("ads_cert_requests.ok");
        self.ads_cert_requests_failure = r.get_or_register_meter("ads_cert_requests.failed");
        self.ads_cert_sign_timer = r.get_or_register_timer("ads_cert_sign_time");
    }

    /// Go: `getAccountMetrics`: the metrics of account `id`, registered on first use.
    pub(crate) fn get_account_metrics(&self, id: &str) -> Arc<AccountMetrics> {
        // Go: go_metrics.go:565
        if let Some(am) = self
            .accounts
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
        {
            return Arc::clone(am);
        }

        let mut accounts = self
            .accounts
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(am) = accounts.get(id) {
            return Arc::clone(am);
        }
        let r = &self.registry;
        let meter = |suffix: &str| r.get_or_register_meter(&format!("account.{id}.{suffix}"));
        let request_meter = meter("requests");
        let debug_request_meter = meter("debug_requests");
        // Go: bidsReceivedMeter and priceHistogram, which nothing records into.
        let _bids_received_meter = meter("bids_received");
        let _price_histogram = r.get_or_register_histogram(
            &format!("account.{id}.prices"),
            new_exp_decay_sample(PRICE_RESERVOIR_SIZE, PRICE_ALPHA),
        );
        let mut am = AccountMetrics {
            request_meter,
            debug_request_meter,
            stored_responses_meter: meter("stored_responses"),
            ..AccountMetrics::default()
        };
        if !self.disabled.is(DisabledMetric::AccountAdapterDetails) {
            let prefix = format!("account.{id}");
            for a in &self.exchanges {
                let mut adapter_metrics = make_blank_adapter_metrics();
                register_adapter_metrics(r, &prefix, a, &mut adapter_metrics);
                drop(am.adapters.insert(a.clone(), adapter_metrics));
            }
        }
        am.bid_validation_creative_size_meter = meter("response.validation.size.err");
        am.bid_validation_creative_size_warn_meter = meter("response.validation.size.warn");

        am.bid_validation_secure_markup_meter = meter("response.validation.secure.err");
        am.bid_validation_secure_markup_warn_meter = meter("response.validation.secure.warn");

        if !self.disabled.is(DisabledMetric::AccountModulesMetrics) {
            for module in &self.modules {
                let mut module_metrics = ModuleMetrics::default();
                register_account_module_metrics(r, id, module, &mut module_metrics);
                drop(am.modules.insert(module.clone(), module_metrics));
            }
        }

        let am = Arc::new(am);
        drop(accounts.insert(id.to_owned(), Arc::clone(&am)));
        drop(accounts);
        am
    }

    /// Go: `getModuleMetric`: the metrics of the module's stage, or `None` with the error
    /// logged.
    fn get_module_metric(&self, labels: &ModuleLabels) -> Option<&ModuleMetrics> {
        // Go: go_metrics.go:1221
        let mm = self
            .module_stages
            .get(&labels.module)
            .and_then(|stages| stages.get(&labels.stage));
        if mm.is_none() {
            // Go: logger.Errorf of the fmt.Errorf error, which prints the same text.
            pbs_logger::errorf!(
                "Trying to run module {} metrics for stage {}: module metrics not found",
                labels.module,
                labels.stage
            );
        }
        mm
    }

    /// Calls `f` with the module's metrics for the labels' account, unless the account is empty
    /// or unknown or has no metrics for the module (Go: the `labels.AccountID` checks of the
    /// `RecordModule*` methods).
    fn record_account_module<F: FnOnce(&ModuleMetrics)>(&self, labels: &ModuleLabels, f: F) {
        if !labels.account_id.is_empty() && labels.account_id != PUBLISHER_UNKNOWN {
            let am = self.get_account_metrics(&labels.account_id);
            if let Some(aam) = am.modules.get(&labels.module) {
                f(aam);
            }
        }
    }

    /// The lower-cased adapter name and its metrics (Go:
    /// `me.AdapterMetrics[strings.ToLower(name)]`).
    fn lookup_adapter(&self, adapter: &str) -> (String, Option<&AdapterMetrics>) {
        let lower_case_adapter = to_lower(adapter);
        let am = self.adapters.get(&lower_case_adapter);
        (lower_case_adapter, am)
    }

    /// Go: the body of the `RecordBidValidation*` methods: the adapter's meter, then the
    /// account's (registered on first use) unless account-adapter details are disabled.
    fn record_bid_validation<A, C>(
        &self,
        adapter: &str,
        pub_id: &str,
        adapter_meter: A,
        account_meter: C,
    ) where
        A: FnOnce(&AdapterMetrics) -> &Meter,
        C: FnOnce(&AccountMetrics) -> &Meter,
    {
        let (_, am) = self.lookup_adapter(adapter);
        let Some(am) = am else {
            pbs_logger::errorf!(
                "Trying to run adapter metrics on {}: adapter metrics not found",
                adapter
            );
            return;
        };
        adapter_meter(am).mark(1);

        let aam = self.get_account_metrics(pub_id);
        if !self.disabled.is(DisabledMetric::AccountAdapterDetails) {
            account_meter(&aam).mark(1);
        }
    }
}

impl MetricsEngine for Metrics {
    fn record_connection_accept(&self, success: bool) {
        // Go: go_metrics.go:691
        if success {
            self.connection_counter.inc(1);
        } else {
            self.connection_accept_error_meter.mark(1);
        }
    }

    fn record_tmax_timeout(&self) {
        // Go: go_metrics.go:699
        self.tmax_timeout_counter.inc(1);
    }

    fn record_connection_close(&self, success: bool) {
        // Go: go_metrics.go:703
        if success {
            self.connection_counter.dec(1);
        } else {
            self.connection_close_error_meter.mark(1);
        }
    }

    fn record_request(&self, labels: &Labels) {
        // Go: go_metrics.go:621
        let Some(meter) = self
            .request_statuses
            .get(&labels.r_type)
            .and_then(|statuses| statuses.get(&labels.request_status))
        else {
            return;
        };
        meter.mark(1);
        if labels.source == DemandSource::APP {
            self.app_request_meter.mark(1);
        } else if labels.cookie_flag == CookieFlag::NO {
            // NOTE: Old behavior was log me.AMPNoCookieMeter here for AMP requests. AMP is
            // still new and OpenRTB does not do this, so changing to match OpenRTB endpoint.
            self.no_cookie_meter.mark(1);
        }

        // Request size by endpoint.
        if labels.request_size > 0 && labels.r_type != RequestType::AMP {
            let endpoint = get_endpoint_from_request_type(labels.r_type);
            let Some(histogram) = self.request_size_by_endpoint.get(&endpoint) else {
                return;
            };
            histogram.update(go_int64(labels.request_size));
        }

        // Handle the account metrics now.
        let am = self.get_account_metrics(&labels.pub_id);
        am.request_meter.mark(1);
    }

    fn record_imps(&self, labels: ImpLabels) {
        // Go: go_metrics.go:675
        self.imp_meter.mark(1);
        if labels.has(ImpMediaType::BANNER) {
            self.imps_type_banner.mark(1);
        }
        if labels.has(ImpMediaType::VIDEO) {
            self.imps_type_video.mark(1);
        }
        if labels.has(ImpMediaType::AUDIO) {
            self.imps_type_audio.mark(1);
        }
        if labels.has(ImpMediaType::NATIVE) {
            self.imps_type_native.mark(1);
        }
    }

    fn record_request_time(&self, labels: &Labels, length: GoDuration) {
        // Go: go_metrics.go:713. Only successful requests are timed, as there are no labels to
        // screen out bad requests.
        if labels.request_status == RequestStatus::OK {
            self.request_timer.update(length);
        }
    }

    fn record_overhead_time(&self, overhead: OverheadType, length: GoDuration) {
        // Go: go_metrics.go:884
        if let Some(timer) = self.overhead_timer.get(&overhead) {
            timer.update(length);
        }
    }

    fn record_adapter_request(&self, labels: &AdapterLabels) {
        // Go: go_metrics.go:743
        let (lower_case_adapter, am) = self.lookup_adapter(&labels.adapter);
        let Some(am) = am else {
            pbs_logger::errorf!(
                "Trying to run adapter metrics on {}: adapter metrics not found",
                labels.adapter.as_str()
            );
            return;
        };

        let account = self.get_account_metrics(&labels.pub_id);
        let aam = account.adapters.get(&lower_case_adapter);
        if labels.adapter_bids == AdapterBid::NONE {
            am.no_bid_meter.mark(1);
            if let Some(aam) = aam {
                aam.no_bid_meter.mark(1);
            }
        } else if labels.adapter_bids == AdapterBid::PRESENT {
            am.got_bids_meter.mark(1);
            if let Some(aam) = aam {
                aam.got_bids_meter.mark(1);
            }
        } else {
            pbs_logger::warnf!(
                "No go-metrics logged for AdapterBids value: {}",
                labels.adapter_bids.as_str()
            );
        }
        for err_type in &labels.adapter_errors {
            let Some(meter) = am.error_meters.get(err_type) else {
                return;
            };
            meter.mark(1);
        }

        if labels.cookie_flag == CookieFlag::NO {
            am.no_cookie_meter.mark(1);
        }
    }

    fn record_adapter_connections(
        &self,
        adapter_name: &str,
        conn_was_reused: bool,
        conn_wait_time: GoDuration,
    ) {
        // Go: go_metrics.go:778
        if self.disabled.is(DisabledMetric::AdapterConnectionMetrics) {
            return;
        }
        let (_, am) = self.lookup_adapter(adapter_name);
        let Some(am) = am else {
            pbs_logger::errorf!(
                "Trying to log adapter connection metrics for {}: adapter not found",
                adapter_name
            );
            return;
        };

        if conn_was_reused {
            am.conn_reused.inc(1);
        } else {
            am.conn_created.inc(1);
        }
        am.conn_wait_time.update(conn_wait_time);
    }

    fn record_dns_time(&self, dns_lookup_time: GoDuration) {
        // Go: go_metrics.go:808
        self.dns_lookup_timer.update(dns_lookup_time);
    }

    fn record_tls_handshake_time(&self, tls_handshake_time: GoDuration) {
        // Go: go_metrics.go:812
        self.tls_handshake_timer.update(tls_handshake_time);
    }

    fn record_bidder_server_response_time(&self, bidder_server_response_time: GoDuration) {
        // Go: go_metrics.go:816
        self.bidder_server_response_timer
            .update(bidder_server_response_time);
    }

    fn record_adapter_panic(&self, labels: &AdapterLabels) {
        // Go: go_metrics.go:731
        let (_, am) = self.lookup_adapter(&labels.adapter);
        let Some(am) = am else {
            pbs_logger::errorf!(
                "Trying to run adapter metrics on {}: adapter metrics not found",
                labels.adapter.as_str()
            );
            return;
        };
        am.panic_meter.mark(1);
    }

    fn record_adapter_bid_received(&self, labels: &AdapterLabels, bid_type: &str, has_adm: bool) {
        // Go: go_metrics.go:822. This tracks how many bids from each bidder use `adm` vs. `nurl`.
        let (lower_case_adapter_name, am) = self.lookup_adapter(&labels.adapter);
        let Some(am) = am else {
            pbs_logger::errorf!(
                "Trying to run adapter bid metrics on {}: adapter metrics not found",
                labels.adapter.as_str()
            );
            return;
        };

        // Adapter metrics.
        am.bids_received_meter.mark(1);
        // Account-adapter metrics.
        if let Some(aam) = self
            .get_account_metrics(&labels.pub_id)
            .adapters
            .get(&lower_case_adapter_name)
        {
            aam.bids_received_meter.mark(1);
        }

        if let Some(metrics_for_type) = am.markup_metrics.get(bid_type) {
            if has_adm {
                metrics_for_type.adm_meter.mark(1);
            } else {
                metrics_for_type.nurl_meter.mark(1);
            }
        } else {
            pbs_logger::errorf!(
                "bid/adm metrics map entry does not exist for type {}. This is a bug, and should \
                 be reported.",
                bid_type
            );
        }
    }

    fn record_adapter_price(&self, labels: &AdapterLabels, cpm: f64) {
        // Go: go_metrics.go:850. A histogram of winning bid prices.
        let (lowercase_adapter, am) = self.lookup_adapter(&labels.adapter);
        let Some(am) = am else {
            pbs_logger::errorf!(
                "Trying to run adapter price metrics on {}: adapter metrics not found",
                labels.adapter.as_str()
            );
            return;
        };
        // Adapter metrics.
        am.price_histogram.update(i64_from_float(cpm));
        // Account-adapter metrics.
        if let Some(aam) = self
            .get_account_metrics(&labels.pub_id)
            .adapters
            .get(&lowercase_adapter)
        {
            aam.price_histogram.update(i64_from_float(cpm));
        }
    }

    fn record_adapter_time(&self, labels: &AdapterLabels, length: GoDuration) {
        // Go: go_metrics.go:867. The adapter response time.
        let (lowercase_adapter, am) = self.lookup_adapter(&labels.adapter);
        let Some(am) = am else {
            pbs_logger::errorf!(
                "Trying to run adapter latency metrics on {}: adapter metrics not found",
                labels.adapter.as_str()
            );
            return;
        };
        // Adapter metrics.
        am.request_timer.update(length);
        // Account-adapter metrics.
        if let Some(aam) = self
            .get_account_metrics(&labels.pub_id)
            .adapters
            .get(&lowercase_adapter)
        {
            aam.request_timer.update(length);
        }
    }

    fn record_cookie_sync(&self, status: CookieSyncStatus) {
        // Go: go_metrics.go:889
        self.cookie_sync_meter.mark(1);
        if let Some(meter) = self.cookie_sync_status_meter.get(&status) {
            meter.mark(1);
        }
    }

    fn record_syncer_request(&self, key: &str, status: SyncerCookieSyncStatus) {
        // Go: go_metrics.go:897
        if let Some(status_meter) = self
            .syncer_requests_meter
            .get(key)
            .and_then(|key_meter| key_meter.get(&status))
        {
            status_meter.mark(1);
        }
    }

    fn record_set_uid(&self, status: SetUidStatus) {
        // Go: go_metrics.go:906
        self.set_uid_meter.mark(1);
        if let Some(meter) = self.set_uid_status_meter.get(&status) {
            meter.mark(1);
        }
    }

    fn record_syncer_set(&self, key: &str, status: SyncerSetUidStatus) {
        // Go: go_metrics.go:914
        if let Some(status_meter) = self
            .syncer_sets_meter
            .get(key)
            .and_then(|key_meter| key_meter.get(&status))
        {
            status_meter.mark(1);
        }
    }

    fn record_stored_req_cache_result(&self, cache_result: CacheResult, inc: usize) {
        // Go: go_metrics.go:924
        if let Some(meter) = self.stored_req_cache_meter.get(&cache_result) {
            meter.mark(go_int64(inc));
        }
    }

    fn record_stored_imp_cache_result(&self, cache_result: CacheResult, inc: usize) {
        // Go: go_metrics.go:930
        if let Some(meter) = self.stored_imp_cache_meter.get(&cache_result) {
            meter.mark(go_int64(inc));
        }
    }

    fn record_account_cache_result(&self, cache_result: CacheResult, inc: usize) {
        // Go: go_metrics.go:936
        if let Some(meter) = self.account_cache_meter.get(&cache_result) {
            meter.mark(go_int64(inc));
        }
    }

    fn record_stored_data_fetch_time(&self, labels: StoredDataLabels, length: GoDuration) {
        // Go: go_metrics.go:721
        if let Some(timer) = self
            .stored_data_fetch_timer
            .get(&labels.data_type)
            .and_then(|timers| timers.get(&labels.data_fetch_type))
        {
            timer.update(length);
        }
    }

    fn record_stored_data_error(&self, labels: StoredDataLabels) {
        // Go: go_metrics.go:726
        if let Some(meter) = self
            .stored_data_error_meter
            .get(&labels.data_type)
            .and_then(|meters| meters.get(&labels.error))
        {
            meter.mark(1);
        }
    }

    fn record_fetcher_result(&self, _subsystem: &str, _result: FetcherResult) {
        // Go: go_metrics.go:941. Not tracked by the go-metrics engine.
    }

    fn record_fetcher_backend_fetch(
        &self,
        _subsystem: &str,
        _operation: FetcherOperation,
        _result: FetcherBackendResult,
        _length: GoDuration,
    ) {
        // Go: go_metrics.go:945. Not tracked by the go-metrics engine.
    }

    fn record_prebid_cache_request_time(&self, success: bool, length: GoDuration) {
        // Go: go_metrics.go:950. The time taken to store the auction result in Prebid Cache.
        if success {
            self.prebid_cache_request_timer_success.update(length);
        } else {
            self.prebid_cache_request_timer_error.update(length);
        }
    }

    fn record_request_queue_time(
        &self,
        success: bool,
        request_type: RequestType,
        length: GoDuration,
    ) {
        // Go: go_metrics.go:958. Remove the check when other request types are supported.
        if request_type == RequestType::VIDEO
            && let Some(timer) = self
                .requests_queue_timer
                .get(&request_type)
                .and_then(|timers| timers.get(&success))
        {
            timer.update(length);
        }
    }

    fn record_timeout_notice(&self, success: bool) {
        // Go: go_metrics.go:965
        if success {
            self.timeout_notification_success.mark(1);
        } else {
            self.timeout_notification_failure.mark(1);
        }
    }

    fn record_request_privacy(&self, privacy: PrivacyLabels) {
        // Go: go_metrics.go:973
        if privacy.ccpa.provided {
            self.privacy_ccpa_request.mark(1);
            if privacy.ccpa.enforced {
                self.privacy_ccpa_request_opt_out.mark(1);
            }
        }

        if privacy.coppa_enforced {
            self.privacy_coppa_request.mark(1);
        }

        if privacy.gdpr.enforced {
            let version = self
                .privacy_tcf_request_version
                .get(&privacy.gdpr.tcf_version)
                .or_else(|| self.privacy_tcf_request_version.get(&TcfVersionValue::ERR));
            if let Some(metric) = version {
                metric.mark(1);
            }
        }

        if privacy.lmt_enforced {
            self.privacy_lmt_request.mark(1);
        }
    }

    fn record_adapter_buyer_uid_scrubbed(&self, adapter_name: &str) {
        // Go: go_metrics.go:998
        if self.disabled.is(DisabledMetric::AdapterBuyerUidScrubbed) {
            return;
        }

        let (_, am) = self.lookup_adapter(adapter_name);
        let Some(am) = am else {
            pbs_logger::errorf!(
                "Trying to log adapter buyeruid scrubbed metric for {}: adapter not found",
                adapter_name
            );
            return;
        };

        am.buyer_uid_scrubbed.mark(1);
    }

    fn record_adapter_gdpr_request_blocked(&self, adapter_name: &str) {
        // Go: go_metrics.go:1013
        if self.disabled.is(DisabledMetric::AdapterGdprRequestBlocked) {
            return;
        }

        let (_, am) = self.lookup_adapter(adapter_name);
        let Some(am) = am else {
            pbs_logger::errorf!(
                "Trying to log adapter GDPR request blocked metric for {}: adapter not found",
                adapter_name
            );
            return;
        };

        am.gdpr_request_blocked.mark(1);
    }

    fn record_debug_request(&self, debug_enabled: bool, pub_id: &str) {
        // Go: go_metrics.go:644
        if debug_enabled {
            self.debug_request_meter.mark(1);
            if pub_id != PUBLISHER_UNKNOWN {
                let am = self.get_account_metrics(pub_id);
                if !self.disabled.is(DisabledMetric::AccountDebug) {
                    am.debug_request_meter.mark(1);
                }
            }
        }
    }

    fn record_stored_response(&self, pub_id: &str) {
        // Go: go_metrics.go:656
        self.stored_responses_meter.mark(1);
        if pub_id != PUBLISHER_UNKNOWN && !self.disabled.is(DisabledMetric::AccountStoredResponses)
        {
            self.get_account_metrics(pub_id)
                .stored_responses_meter
                .mark(1);
        }
    }

    fn record_gvl_list_request(&self) {
        // Go: go_metrics.go:663
        self.gvl_list_requests_meter.mark(1);
    }

    fn record_live_gvl_fetch(&self, success: bool) {
        // Go: go_metrics.go:667
        if success {
            self.live_gvl_fetch_success.mark(1);
        } else {
            self.live_gvl_fetch_failure.mark(1);
        }
    }

    fn record_ads_cert_req(&self, success: bool) {
        // Go: go_metrics.go:1028
        if success {
            self.ads_cert_requests_success.mark(1);
        } else {
            self.ads_cert_requests_failure.mark(1);
        }
    }

    fn record_ads_cert_sign_time(&self, ads_cert_sign_time: GoDuration) {
        // Go: go_metrics.go:1036
        self.ads_cert_sign_timer.update(ads_cert_sign_time);
    }

    fn record_bid_validation_creative_size_error(&self, adapter: &str, account: &str) {
        // Go: go_metrics.go:1040
        self.record_bid_validation(
            adapter,
            account,
            |am| &am.bid_validation_creative_size_error_meter,
            |aam| &aam.bid_validation_creative_size_meter,
        );
    }

    fn record_bid_validation_creative_size_warn(&self, adapter: &str, account: &str) {
        // Go: go_metrics.go:1055
        self.record_bid_validation(
            adapter,
            account,
            |am| &am.bid_validation_creative_size_warn_meter,
            |aam| &aam.bid_validation_creative_size_warn_meter,
        );
    }

    fn record_bid_validation_secure_markup_error(&self, adapter: &str, account: &str) {
        // Go: go_metrics.go:1070
        self.record_bid_validation(
            adapter,
            account,
            |am| &am.bid_validation_secure_markup_error_meter,
            |aam| &aam.bid_validation_secure_markup_meter,
        );
    }

    fn record_bid_validation_secure_markup_warn(&self, adapter: &str, account: &str) {
        // Go: go_metrics.go:1085
        self.record_bid_validation(
            adapter,
            account,
            |am| &am.bid_validation_secure_markup_warn_meter,
            |aam| &aam.bid_validation_secure_markup_warn_meter,
        );
    }

    fn record_module_called(&self, labels: &ModuleLabels, duration: GoDuration) {
        // Go: go_metrics.go:1100
        let Some(mm) = self.get_module_metric(labels) else {
            return;
        };

        // Module metrics.
        mm.call_counter.inc(1);
        mm.duration_timer.update(duration);

        // Account-module metrics.
        self.record_account_module(labels, |aam| {
            aam.call_counter.inc(1);
            aam.duration_timer.update(duration);
        });
    }

    fn record_module_failed(&self, labels: &ModuleLabels) {
        // Go: go_metrics.go:1119
        let Some(mm) = self.get_module_metric(labels) else {
            return;
        };
        mm.failure_counter.inc(1);
        self.record_account_module(labels, |aam| aam.failure_counter.inc(1));
    }

    fn record_module_success_nooped(&self, labels: &ModuleLabels) {
        // Go: go_metrics.go:1136
        let Some(mm) = self.get_module_metric(labels) else {
            return;
        };
        mm.success_noop_counter.inc(1);
        self.record_account_module(labels, |aam| aam.success_noop_counter.inc(1));
    }

    fn record_module_success_updated(&self, labels: &ModuleLabels) {
        // Go: go_metrics.go:1153
        let Some(mm) = self.get_module_metric(labels) else {
            return;
        };
        mm.success_update_counter.inc(1);
        self.record_account_module(labels, |aam| aam.success_update_counter.inc(1));
    }

    fn record_module_success_rejected(&self, labels: &ModuleLabels) {
        // Go: go_metrics.go:1170
        let Some(mm) = self.get_module_metric(labels) else {
            return;
        };
        mm.success_reject_counter.inc(1);
        self.record_account_module(labels, |aam| aam.success_reject_counter.inc(1));
    }

    fn record_module_execution_error(&self, labels: &ModuleLabels) {
        // Go: go_metrics.go:1187
        let Some(mm) = self.get_module_metric(labels) else {
            return;
        };
        mm.execution_error_counter.inc(1);
        self.record_account_module(labels, |aam| aam.execution_error_counter.inc(1));
    }

    fn record_module_timeout(&self, labels: &ModuleLabels) {
        // Go: go_metrics.go:1204
        let Some(mm) = self.get_module_metric(labels) else {
            return;
        };
        mm.timeout_counter.inc(1);
        self.record_account_module(labels, |aam| aam.timeout_counter.inc(1));
    }

    fn record_adapter_throttled(&self, adapter_name: &str) {
        // Go: go_metrics.go:1232
        let (_, am) = self.lookup_adapter(adapter_name);
        let Some(am) = am else {
            pbs_logger::errorf!(
                "Trying to log adapter throttled metric for {}: adapter not found",
                adapter_name
            );
            return;
        };

        am.throttled_meter.mark(1);
    }

    fn record_adapter_connection_dial_error(&self, adapter_name: &str) {
        // Go: go_metrics.go:800
        if let (_, Some(am)) = self.lookup_adapter(adapter_name) {
            am.conn_dial_errors.inc(1);
        }
    }

    fn record_adapter_connection_dial_time(&self, adapter_name: &str, dial_start_time: GoDuration) {
        // Go: go_metrics.go:804
        if let (_, Some(am)) = self.lookup_adapter(adapter_name) {
            am.conn_dial_time.update(dial_start_time);
        }
    }
}

#[cfg(test)]
#[path = "go_metrics_test.rs"]
mod tests;
