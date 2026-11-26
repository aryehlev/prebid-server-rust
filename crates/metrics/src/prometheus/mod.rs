//! The Prometheus metrics engine.
//!
//! Go: `metrics/prometheus/prometheus.go` (package `prometheusmetrics`).
//!
//! The families are the prometheus crate's counters and histograms on one [`Registry`], under
//! Go's names, help strings, label names, types, buckets, namespace and subsystem, registered in
//! Go's order and on Go's conditions, with Go's label values and preloading (Rule B (c): metric
//! names, labels and types are the operator interface). [`Metrics::text_exposition`] is the
//! crate's text format of what the registry gathers, served as [`TEXT_FORMAT`]; where it differs
//! from the `client_golang` v1.12.1 scrape, an entry of the ledger's `metrics` category says how.
//!
//! Go also registers `collectors.NewGoCollector()` (the `go_*` runtime families, prefixed with
//! `<namespace>_<subsystem>_`). Those report Go runtime internals and nothing stands in for them
//! (D-GOCOLL; ledger: `metrics.go-families`).

mod lower;
mod preload;
mod type_conversion;

use std::collections::BTreeMap;

pub use ::prometheus::{Error, Registry, TEXT_FORMAT, proto};
use ::prometheus::{
    Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, Opts, TextEncoder,
    core::Collector,
};

use self::lower::to_lower;
use crate::{
    float_from_usize,
    metrics::{
        AdapterBid, AdapterLabels, CacheResult, CookieFlag, CookieSyncStatus, FetcherBackendResult,
        FetcherOperation, FetcherResult, GoDuration, ImpLabels, Labels, MetricsEngine,
        ModuleLabels, OverheadType, PUBLISHER_UNKNOWN, PrivacyLabels, RequestStatus, RequestType,
        SetUidStatus, StoredDataLabels, StoredDataType, SyncerCookieSyncStatus, SyncerSetUidStatus,
        get_endpoint_from_request_type,
    },
    options::{DisabledMetric, DisabledMetrics, PrometheusMetrics},
};

const ACCOUNT_LABEL: &str = "account";
const ADAPTER_ERROR_LABEL: &str = "adapter_error";
const ADAPTER_LABEL: &str = "adapter";
const CACHE_RESULT_LABEL: &str = "cache_result";
const FETCHER_RESULT_LABEL: &str = "result";
const CONNECTION_ERROR_LABEL: &str = "connection_error";
const COOKIE_LABEL: &str = "cookie";
const HAS_BIDS_LABEL: &str = "has_bids";
const IS_AUDIO_LABEL: &str = "audio";
const IS_BANNER_LABEL: &str = "banner";
const IS_NATIVE_LABEL: &str = "native";
const OPERATION_LABEL: &str = "operation";
const IS_VIDEO_LABEL: &str = "video";
const MARKUP_DELIVERY_LABEL: &str = "delivery";
const OPT_OUT_LABEL: &str = "opt_out";
const OVERHEAD_TYPE_LABEL: &str = "overhead_type";
const REQUEST_STATUS_LABEL: &str = "request_status";
const REQUEST_TYPE_LABEL: &str = "request_type";
/// Go names the endpoint label `request_size` (`requestEndpointLabel`).
const REQUEST_ENDPOINT_LABEL: &str = "request_size";
const STAGE_LABEL: &str = "stage";
const STATUS_LABEL: &str = "status";
const SUCCESS_LABEL: &str = "success";
const SUBSYSTEM_LABEL: &str = "subsystem";
const SYNCER_LABEL: &str = "syncer";
const VERSION_LABEL: &str = "version";

const CONNECTION_ACCEPT_ERROR: &str = "accept";
const CONNECTION_CLOSE_ERROR: &str = "close";

const MARKUP_DELIVERY_ADM: &str = "adm";
const MARKUP_DELIVERY_NURL: &str = "nurl";

const REQUEST_SUCCESS_LABEL: &str = "requestAcceptedLabel";
const REQUEST_REJECT_LABEL: &str = "requestRejectedLabel";

const REQUEST_SUCCESSFUL: &str = "ok";
const REQUEST_FAILED: &str = "failed";

const SOURCE_LABEL: &str = "source";
const SOURCE_REQUEST: &str = "request";

const STORED_DATA_FETCH_TYPE_LABEL: &str = "stored_data_fetch_type";
const STORED_DATA_ERROR_LABEL: &str = "stored_data_error";

const STANDARD_TIME_BUCKETS: &[f64] = &[0.05, 0.1, 0.15, 0.20, 0.25, 0.3, 0.4, 0.5, 0.75, 1.0];
const CACHE_WRITE_TIME_BUCKETS: &[f64] = &[
    0.001, 0.002, 0.005, 0.01, 0.025, 0.05, 0.1, 0.2, 0.3, 0.4, 0.5, 1.0,
];
const PRICE_BUCKETS: &[f64] = &[
    250.0, 500.0, 750.0, 1000.0, 1500.0, 2000.0, 2500.0, 3000.0, 3500.0, 4000.0,
];
const QUEUED_REQUEST_TIME_BUCKETS: &[f64] =
    &[0.0, 1.0, 5.0, 30.0, 60.0, 120.0, 180.0, 240.0, 300.0];
const OVERHEAD_TIME_BUCKETS: &[f64] = &[
    0.05, 0.06, 0.07, 0.08, 0.09, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0,
];
const REQUEST_SIZE_BUCKETS: &[f64] = &[
    100.0, 500.0, 750.0, 1000.0, 2000.0, 4000.0, 7000.0, 10000.0, 15000.0, 20000.0, 50000.0,
    75000.0,
];
/// Go: `append(prometheus.DefBuckets, 15, 30)`.
const DIAL_TIME_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 15.0, 30.0,
];

/// One family of `NewMetrics`: its name (before the namespace and subsystem), help string, label
/// names and, for a histogram, buckets.
struct Spec {
    name: &'static str,
    help: &'static str,
    labels: &'static [&'static str],
    buckets: &'static [f64],
}

impl Spec {
    /// Go: `newCounter` and `newCounterWithoutLabels` (no labels).
    const fn counter(
        name: &'static str,
        help: &'static str,
        labels: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            help,
            labels,
            buckets: &[],
        }
    }

    /// Go: `newHistogramVec` and `newHistogram` (no labels).
    const fn histogram(
        name: &'static str,
        help: &'static str,
        labels: &'static [&'static str],
        buckets: &'static [f64],
    ) -> Self {
        Self {
            name,
            help,
            labels,
            buckets,
        }
    }
}

const CONNECTIONS_CLOSED: Spec = Spec::counter(
    "connections_closed",
    "Count of successful connections closed to Prebid Server.",
    &[],
);
const CONNECTIONS_ERROR: Spec = Spec::counter(
    "connections_error",
    "Count of errors for connection open and close attempts to Prebid Server labeled by type.",
    &[CONNECTION_ERROR_LABEL],
);
const CONNECTIONS_OPENED: Spec = Spec::counter(
    "connections_opened",
    "Count of successful connections opened to Prebid Server.",
    &[],
);
const TMAX_TIMEOUT: Spec = Spec::counter(
    "tmax_timeout",
    "Count of requests rejected due to Tmax timeout exceed.",
    &[],
);
const COOKIE_SYNC: Spec = Spec::counter(
    "cookie_sync_requests",
    "Count of cookie sync requests to Prebid Server.",
    &[STATUS_LABEL],
);
const SET_UID: Spec = Spec::counter(
    "setuid_requests",
    "Count of set uid requests to Prebid Server.",
    &[STATUS_LABEL],
);
const IMPRESSIONS: Spec = Spec::counter(
    "impressions_requests",
    "Count of requested impressions to Prebid Server labeled by type.",
    &[
        IS_BANNER_LABEL,
        IS_VIDEO_LABEL,
        IS_AUDIO_LABEL,
        IS_NATIVE_LABEL,
    ],
);
const PREBID_CACHE_WRITE_TIMER: Spec = Spec::histogram(
    "prebidcache_write_time_seconds",
    "Seconds to write to Prebid Cache labeled by success or failure. Failure timing is limited by \
     Prebid Server enforced timeouts.",
    &[SUCCESS_LABEL],
    CACHE_WRITE_TIME_BUCKETS,
);
const REQUESTS: Spec = Spec::counter(
    "requests",
    "Count of total requests to Prebid Server labeled by type and status.",
    &[REQUEST_TYPE_LABEL, REQUEST_STATUS_LABEL],
);
const REQUESTS_SIZE: Spec = Spec::histogram(
    "request_size_bytes",
    "Count that keeps track of incoming request size in bytes labeled by endpoint.",
    &[REQUEST_ENDPOINT_LABEL],
    REQUEST_SIZE_BUCKETS,
);
const DEBUG_REQUESTS: Spec = Spec::counter(
    "debug_requests",
    "Count of total requests to Prebid Server that have debug enabled",
    &[],
);
const REQUESTS_TIMER: Spec = Spec::histogram(
    "request_time_seconds",
    "Seconds to resolve successful Prebid Server requests labeled by type.",
    &[REQUEST_TYPE_LABEL],
    STANDARD_TIME_BUCKETS,
);
const REQUESTS_WITHOUT_COOKIE: Spec = Spec::counter(
    "requests_without_cookie",
    "Count of total requests to Prebid Server without a cookie labeled by type.",
    &[REQUEST_TYPE_LABEL],
);
const STORED_IMPRESSIONS_CACHE_RESULT: Spec = Spec::counter(
    "stored_impressions_cache_performance",
    "Count of stored impression cache requests attempts by hits or miss.",
    &[CACHE_RESULT_LABEL],
);
const STORED_REQUEST_CACHE_RESULT: Spec = Spec::counter(
    "stored_request_cache_performance",
    "Count of stored request cache requests attempts by hits or miss.",
    &[CACHE_RESULT_LABEL],
);
const ACCOUNT_CACHE_RESULT: Spec = Spec::counter(
    "account_cache_performance",
    "Count of account cache lookups by hits or miss.",
    &[CACHE_RESULT_LABEL],
);
const STORED_ACCOUNT_FETCH_TIMER: Spec = Spec::histogram(
    "stored_account_fetch_time_seconds",
    "Seconds to fetch stored accounts labeled by fetch type",
    &[STORED_DATA_FETCH_TYPE_LABEL],
    STANDARD_TIME_BUCKETS,
);
const STORED_ACCOUNT_ERRORS: Spec = Spec::counter(
    "stored_account_errors",
    "Count of stored account errors by error type",
    &[STORED_DATA_ERROR_LABEL],
);
const FETCHER_RESULT: Spec = Spec::counter(
    "fetcher_cache_result",
    "Count of Fetchers 2.0 cache lookups labeled by subsystem and result (hit, miss, negative).",
    &[SUBSYSTEM_LABEL, FETCHER_RESULT_LABEL],
);
const FETCHER_BACKEND_FETCH: Spec = Spec::counter(
    "fetcher_backend_fetch",
    "Count of Fetchers 2.0 backend fetches labeled by subsystem, operation and result (ok, \
     notfound, error).",
    &[SUBSYSTEM_LABEL, OPERATION_LABEL, FETCHER_RESULT_LABEL],
);
const FETCHER_BACKEND_FETCH_TIMER: Spec = Spec::histogram(
    "fetcher_backend_fetch_duration_seconds",
    "Seconds to fetch a value from a Fetchers 2.0 backend source, labeled by subsystem and \
     operation.",
    &[SUBSYSTEM_LABEL, OPERATION_LABEL],
    STANDARD_TIME_BUCKETS,
);
const STORED_AMP_FETCH_TIMER: Spec = Spec::histogram(
    "stored_amp_fetch_time_seconds",
    "Seconds to fetch stored AMP requests labeled by fetch type",
    &[STORED_DATA_FETCH_TYPE_LABEL],
    STANDARD_TIME_BUCKETS,
);
const STORED_AMP_ERRORS: Spec = Spec::counter(
    "stored_amp_errors",
    "Count of stored AMP errors by error type",
    &[STORED_DATA_ERROR_LABEL],
);
const STORED_CATEGORY_FETCH_TIMER: Spec = Spec::histogram(
    "stored_category_fetch_time_seconds",
    "Seconds to fetch stored categories labeled by fetch type",
    &[STORED_DATA_FETCH_TYPE_LABEL],
    STANDARD_TIME_BUCKETS,
);
const STORED_CATEGORY_ERRORS: Spec = Spec::counter(
    "stored_category_errors",
    "Count of stored category errors by error type",
    &[STORED_DATA_ERROR_LABEL],
);
const STORED_REQUEST_FETCH_TIMER: Spec = Spec::histogram(
    "stored_request_fetch_time_seconds",
    "Seconds to fetch stored requests labeled by fetch type",
    &[STORED_DATA_FETCH_TYPE_LABEL],
    STANDARD_TIME_BUCKETS,
);
const STORED_REQUEST_ERRORS: Spec = Spec::counter(
    "stored_request_errors",
    "Count of stored request errors by error type",
    &[STORED_DATA_ERROR_LABEL],
);
const STORED_VIDEO_FETCH_TIMER: Spec = Spec::histogram(
    "stored_video_fetch_time_seconds",
    "Seconds to fetch stored video labeled by fetch type",
    &[STORED_DATA_FETCH_TYPE_LABEL],
    STANDARD_TIME_BUCKETS,
);
const STORED_VIDEO_ERRORS: Spec = Spec::counter(
    "stored_video_errors",
    "Count of stored video errors by error type",
    &[STORED_DATA_ERROR_LABEL],
);
const TIMEOUT_NOTIFICATIONS: Spec = Spec::counter(
    "timeout_notification",
    "Count of timeout notifications triggered, and if they were successfully sent.",
    &[SUCCESS_LABEL],
);
const DNS_LOOKUP_TIMER: Spec = Spec::histogram(
    "dns_lookup_time",
    "Seconds to resolve DNS",
    &[],
    STANDARD_TIME_BUCKETS,
);
const TLS_HANDSHAKE_TIMER: Spec = Spec::histogram(
    "tls_handshake_time",
    "Seconds to perform TLS Handshake",
    &[],
    STANDARD_TIME_BUCKETS,
);
const PRIVACY_CCPA: Spec = Spec::counter(
    "privacy_ccpa",
    "Count of total requests to Prebid Server where CCPA was provided by source and opt-out .",
    &[SOURCE_LABEL, OPT_OUT_LABEL],
);
const PRIVACY_COPPA: Spec = Spec::counter(
    "privacy_coppa",
    "Count of total requests to Prebid Server where the COPPA flag was set by source",
    &[SOURCE_LABEL],
);
const PRIVACY_TCF: Spec = Spec::counter(
    "privacy_tcf",
    "Count of TCF versions for requests where GDPR was enforced by source and version.",
    &[VERSION_LABEL, SOURCE_LABEL],
);
const PRIVACY_LMT: Spec = Spec::counter(
    "privacy_lmt",
    "Count of total requests to Prebid Server where the LMT flag was set by source",
    &[SOURCE_LABEL],
);
const ADAPTER_SCRUBBED_BUYER_UIDS: Spec = Spec::counter(
    "adapter_buyeruids_scrubbed",
    "Count of total bidder requests with a scrubbed buyeruid due to a privacy policy",
    &[ADAPTER_LABEL],
);
const ADAPTER_GDPR_BLOCKED_REQUESTS: Spec = Spec::counter(
    "adapter_gdpr_requests_blocked",
    "Count of total bidder requests blocked due to unsatisfied GDPR purpose 2 legal basis",
    &[ADAPTER_LABEL],
);
const STORED_RESPONSES_FETCH_TIMER: Spec = Spec::histogram(
    "stored_response_fetch_time_seconds",
    "Seconds to fetch stored responses labeled by fetch type",
    &[STORED_DATA_FETCH_TYPE_LABEL],
    STANDARD_TIME_BUCKETS,
);
const STORED_RESPONSES_ERRORS: Spec = Spec::counter(
    "stored_response_errors",
    "Count of stored video errors by error type",
    &[STORED_DATA_ERROR_LABEL],
);
const STORED_RESPONSES: Spec = Spec::counter(
    "stored_responses",
    "Count of total requests to Prebid Server that have stored responses",
    &[],
);
const GVL_LIST_REQUESTS: Spec = Spec::counter(
    "gvl_requests",
    "Count number of times GVL list is fetched",
    &[],
);
const LIVE_GVL_FETCH: Spec = Spec::counter(
    "live_gvl_fetch",
    "Count of live GVL vendor ID fetches labeled by success or failure.",
    &[SUCCESS_LABEL],
);
const ADAPTER_BIDS: Spec = Spec::counter(
    "adapter_bids",
    "Count of bids labeled by adapter and markup delivery type (adm or nurl).",
    &[ADAPTER_LABEL, MARKUP_DELIVERY_LABEL],
);
const ADAPTER_ERRORS: Spec = Spec::counter(
    "adapter_errors",
    "Count of errors labeled by adapter and error type.",
    &[ADAPTER_LABEL, ADAPTER_ERROR_LABEL],
);
const ADAPTER_PANICS: Spec = Spec::counter(
    "adapter_panics",
    "Count of panics labeled by adapter.",
    &[ADAPTER_LABEL],
);
const ADAPTER_PRICES: Spec = Spec::histogram(
    "adapter_prices",
    "Monetary value of the bids labeled by adapter.",
    &[ADAPTER_LABEL],
    PRICE_BUCKETS,
);
const ADAPTER_REQUESTS: Spec = Spec::counter(
    "adapter_requests",
    "Count of requests labeled by adapter, if has a cookie, and if it resulted in bids.",
    &[ADAPTER_LABEL, COOKIE_LABEL, HAS_BIDS_LABEL],
);
const ADAPTER_CREATED_CONNECTIONS: Spec = Spec::counter(
    "adapter_connection_created",
    "Count that keeps track of new connections when contacting adapter bidder endpoints.",
    &[ADAPTER_LABEL],
);
const ADAPTER_REUSED_CONNECTIONS: Spec = Spec::counter(
    "adapter_connection_reused",
    "Count that keeps track of reused connections when contacting adapter bidder endpoints.",
    &[ADAPTER_LABEL],
);
const ADAPTER_CONNECTION_WAIT_TIME: Spec = Spec::histogram(
    "adapter_connection_wait",
    "Seconds from when the connection was requested until it is either created or reused",
    &[ADAPTER_LABEL],
    STANDARD_TIME_BUCKETS,
);
const ADAPTER_CONNECTION_DIAL_ERRORS: Spec = Spec::counter(
    "adapter_connection_dial_errors",
    "Count when a connection dial returns an error.",
    &[ADAPTER_LABEL],
);
const ADAPTER_CONNECTION_DIAL_TIME: Spec = Spec::histogram(
    "adapter_connection_dial_time",
    "Seconds adapter bidder connection dial lasted",
    &[ADAPTER_LABEL],
    DIAL_TIME_BUCKETS,
);
const ADAPTER_BID_RESPONSE_VALIDATION_SIZE_ERROR: Spec = Spec::counter(
    "adapter_response_validation_size_err",
    "Count that tracks number of bids removed from bid response that had a creative size greater \
     than maxWidth/maxHeight",
    &[ADAPTER_LABEL, SUCCESS_LABEL],
);
const ADAPTER_BID_RESPONSE_VALIDATION_SIZE_WARN: Spec = Spec::counter(
    "adapter_response_validation_size_warn",
    "Count that tracks number of bids removed from bid response that had a creative size greater \
     than maxWidth/maxHeight (warn)",
    &[ADAPTER_LABEL, SUCCESS_LABEL],
);
const ADAPTER_BID_RESPONSE_SECURE_MARKUP_ERROR: Spec = Spec::counter(
    "adapter_response_validation_secure_err",
    "Count that tracks number of bids removed from bid response that had a invalid bidAdm",
    &[ADAPTER_LABEL, SUCCESS_LABEL],
);
const ADAPTER_BID_RESPONSE_SECURE_MARKUP_WARN: Spec = Spec::counter(
    "adapter_response_validation_secure_warn",
    "Count that tracks number of bids removed from bid response that had a invalid bidAdm (warn)",
    &[ADAPTER_LABEL, SUCCESS_LABEL],
);
const ADAPTER_THROTTLED: Spec = Spec::counter(
    "adapter_throttled",
    "Count of requests throttled labeled by adapter.",
    &[ADAPTER_LABEL],
);
const OVERHEAD_TIMER: Spec = Spec::histogram(
    "overhead_time_seconds",
    "Seconds to prepare adapter request or resolve adapter response",
    &[OVERHEAD_TYPE_LABEL],
    OVERHEAD_TIME_BUCKETS,
);
const ADAPTER_REQUESTS_TIMER: Spec = Spec::histogram(
    "adapter_request_time_seconds",
    "Seconds to resolve each successful request labeled by adapter.",
    &[ADAPTER_LABEL],
    STANDARD_TIME_BUCKETS,
);
const BIDDER_SERVER_RESPONSE_TIMER: Spec = Spec::histogram(
    "bidder_server_response_time_seconds",
    "Duration needed to send HTTP request and receive response back from bidder server.",
    &[],
    STANDARD_TIME_BUCKETS,
);
const SYNCER_REQUESTS: Spec = Spec::counter(
    "syncer_requests",
    "Count of cookie sync requests where a syncer is a candidate to be synced labeled by syncer \
     key and status.",
    &[SYNCER_LABEL, STATUS_LABEL],
);
const SYNCER_SETS: Spec = Spec::counter(
    "syncer_sets",
    "Count of setuid set requests for a syncer labeled by syncer key and status.",
    &[SYNCER_LABEL, STATUS_LABEL],
);
const ACCOUNT_REQUESTS: Spec = Spec::counter(
    "account_requests",
    "Count of total requests to Prebid Server labeled by account.",
    &[ACCOUNT_LABEL],
);
const ACCOUNT_DEBUG_REQUESTS: Spec = Spec::counter(
    "account_debug_requests",
    "Count of total requests to Prebid Server that have debug enabled labled by account",
    &[ACCOUNT_LABEL],
);
const ACCOUNT_BID_RESPONSE_VALIDATION_SIZE_ERROR: Spec = Spec::counter(
    "account_response_validation_size_err",
    "Count that tracks number of bids removed from bid response that had a creative size greater \
     than maxWidth/maxHeight labeled by account (enforce) ",
    &[ACCOUNT_LABEL, SUCCESS_LABEL],
);
const ACCOUNT_BID_RESPONSE_VALIDATION_SIZE_WARN: Spec = Spec::counter(
    "account_response_validation_size_warn",
    "Count that tracks number of bids removed from bid response that had a creative size greater \
     than maxWidth/maxHeight labeled by account (warn)",
    &[ACCOUNT_LABEL, SUCCESS_LABEL],
);
const ACCOUNT_BID_RESPONSE_SECURE_MARKUP_ERROR: Spec = Spec::counter(
    "account_response_validation_secure_err",
    "Count that tracks number of bids removed from bid response that had a invalid bidAdm labeled \
     by account (enforce) ",
    &[ACCOUNT_LABEL, SUCCESS_LABEL],
);
const ACCOUNT_BID_RESPONSE_SECURE_MARKUP_WARN: Spec = Spec::counter(
    "account_response_validation_secure_warn",
    "Count that tracks number of bids removed from bid response that had a invalid bidAdm labeled \
     by account (warn)",
    &[ACCOUNT_LABEL, SUCCESS_LABEL],
);
const REQUESTS_QUEUE_TIMER: Spec = Spec::histogram(
    "request_queue_time",
    "Seconds request was waiting in queue",
    &[REQUEST_TYPE_LABEL, REQUEST_STATUS_LABEL],
    QUEUED_REQUEST_TIME_BUCKETS,
);
const ACCOUNT_STORED_RESPONSES: Spec = Spec::counter(
    "account_stored_responses",
    "Count of total requests to Prebid Server that have stored responses labled by account",
    &[ACCOUNT_LABEL],
);
const ADS_CERT_SIGN_TIMER: Spec = Spec::histogram(
    "ads_cert_sign_time",
    "Seconds to generate an AdsCert header",
    &[],
    STANDARD_TIME_BUCKETS,
);
const ADS_CERT_REQUESTS: Spec = Spec::counter(
    "ads_cert_requests",
    "Count of AdsCert request, and if they were successfully sent.",
    &[SUCCESS_LABEL],
);

/// The per-module families, keyed by module name.
///
/// Go: the `module*` maps of `prometheusmetrics.Metrics`, filled by `createModulesMetrics`.
#[derive(Debug, Default)]
struct ModuleFamilies {
    duration: BTreeMap<String, HistogramVec>,
    calls: BTreeMap<String, IntCounterVec>,
    failures: BTreeMap<String, IntCounterVec>,
    success_noops: BTreeMap<String, IntCounterVec>,
    success_updates: BTreeMap<String, IntCounterVec>,
    success_rejects: BTreeMap<String, IntCounterVec>,
    execution_errors: BTreeMap<String, IntCounterVec>,
    timeouts: BTreeMap<String, IntCounterVec>,
}

/// The Prometheus metrics backing the [`MetricsEngine`] implementation.
///
/// Go: `prometheusmetrics.Metrics`. A family Go creates with `prometheus.NewCounter` or
/// `NewHistogram` (no labels) is an [`IntCounter`] or a [`Histogram`], one with labels an
/// [`IntCounterVec`] or a [`HistogramVec`]. A family that Go leaves `nil` because its disabled
/// flag is set at construction is `None`; recording into it is a no-op (Go would dereference
/// nil, which no Go caller does).
#[derive(Debug)]
pub struct Metrics {
    gatherer: Registry,

    // General metrics.
    connections_closed: IntCounter,
    connections_error: IntCounterVec,
    connections_opened: IntCounter,
    tmax_timeout: IntCounter,
    cookie_sync: IntCounterVec,
    set_uid: IntCounterVec,
    impressions: IntCounterVec,
    prebid_cache_write_timer: HistogramVec,
    requests: IntCounterVec,
    requests_size: HistogramVec,
    debug_requests: IntCounter,
    requests_timer: HistogramVec,
    requests_queue_timer: HistogramVec,
    requests_without_cookie: IntCounterVec,
    stored_impressions_cache_result: IntCounterVec,
    stored_request_cache_result: IntCounterVec,
    account_cache_result: IntCounterVec,
    stored_account_fetch_timer: HistogramVec,
    stored_account_errors: IntCounterVec,
    stored_amp_fetch_timer: HistogramVec,
    stored_amp_errors: IntCounterVec,
    stored_category_fetch_timer: HistogramVec,
    stored_category_errors: IntCounterVec,
    stored_request_fetch_timer: HistogramVec,
    stored_request_errors: IntCounterVec,
    stored_video_fetch_timer: HistogramVec,
    stored_video_errors: IntCounterVec,
    timeout_notifications: IntCounterVec,
    dns_lookup_timer: Histogram,
    tls_handshake_timer: Histogram,
    privacy_ccpa: IntCounterVec,
    privacy_coppa: IntCounterVec,
    privacy_lmt: IntCounterVec,
    privacy_tcf: IntCounterVec,
    stored_responses: IntCounter,
    gvl_list_requests: IntCounter,
    live_gvl_fetch: IntCounterVec,
    stored_responses_fetch_timer: HistogramVec,
    stored_responses_errors: IntCounterVec,
    ads_cert_requests: IntCounterVec,
    ads_cert_sign_timer: Histogram,
    bidder_server_response_timer: Histogram,

    // Fetchers 2.0 metrics.
    fetcher_result: IntCounterVec,
    fetcher_backend_fetch: IntCounterVec,
    fetcher_backend_fetch_timer: HistogramVec,

    // Adapter metrics.
    adapter_bids: IntCounterVec,
    adapter_errors: IntCounterVec,
    adapter_panics: IntCounterVec,
    adapter_prices: HistogramVec,
    adapter_requests: IntCounterVec,
    overhead_timer: HistogramVec,
    adapter_requests_timer: HistogramVec,
    adapter_reused_connections: Option<IntCounterVec>,
    adapter_created_connections: Option<IntCounterVec>,
    adapter_connection_wait_time: Option<HistogramVec>,
    adapter_scrubbed_buyer_uids: Option<IntCounterVec>,
    adapter_gdpr_blocked_requests: Option<IntCounterVec>,
    adapter_bid_response_validation_size_error: IntCounterVec,
    adapter_bid_response_validation_size_warn: IntCounterVec,
    adapter_bid_response_secure_markup_error: IntCounterVec,
    adapter_bid_response_secure_markup_warn: IntCounterVec,
    adapter_throttled: IntCounterVec,
    adapter_connection_dial_errors: Option<IntCounterVec>,
    adapter_connection_dial_time: Option<HistogramVec>,

    // Syncer metrics.
    syncer_requests: IntCounterVec,
    syncer_sets: IntCounterVec,

    // Account metrics.
    account_requests: IntCounterVec,
    account_debug_requests: IntCounterVec,
    account_stored_responses: IntCounterVec,
    account_bid_response_validation_size_error: IntCounterVec,
    account_bid_response_validation_size_warn: IntCounterVec,
    account_bid_response_secure_markup_error: IntCounterVec,
    account_bid_response_secure_markup_warn: IntCounterVec,

    // Module metrics.
    modules: ModuleFamilies,

    /// Go: `metricsDisabled`.
    disabled_flags: DisabledMetrics,
}

/// Registers families with Go's `newCounter`/`newHistogramVec` options (the namespace and
/// subsystem of the configuration) on one registry.
struct Builder<'a> {
    cfg: &'a PrometheusMetrics,
    reg: Registry,
}

/// The `DisabledMetrics` toggles Go's `NewMetrics` reads to decide which families exist.
#[derive(Clone, Copy, Debug)]
struct Toggles(DisabledMetrics);

impl Toggles {
    const fn connections(self) -> bool {
        !self.0.is(DisabledMetric::AdapterConnectionMetrics)
    }

    /// The dial families are registered only inside Go's connection-metrics block.
    const fn dial(self) -> bool {
        self.connections() && !self.0.is(DisabledMetric::AdapterConnectionDialMetrics)
    }

    const fn scrubbed(self) -> bool {
        !self.0.is(DisabledMetric::AdapterBuyerUidScrubbed)
    }

    const fn gdpr_blocked(self) -> bool {
        !self.0.is(DisabledMetric::AdapterGdprRequestBlocked)
    }
}

impl<'a> Builder<'a> {
    fn new(cfg: &'a PrometheusMetrics) -> Self {
        Self {
            cfg,
            reg: Registry::new(),
        }
    }

    /// Go: the `prometheus.CounterOpts` of `newCounter`.
    fn opts(&self, name: &str, help: &str) -> Opts {
        Opts::new(name, help)
            .namespace(self.cfg.namespace.as_str())
            .subsystem(self.cfg.subsystem.as_str())
    }

    /// Go: the `prometheus.HistogramOpts` of `newHistogramVec`.
    fn histogram_opts(&self, name: &str, help: &str, buckets: &[f64]) -> HistogramOpts {
        HistogramOpts::new(name, help)
            .namespace(self.cfg.namespace.as_str())
            .subsystem(self.cfg.subsystem.as_str())
            .buckets(buckets.to_vec())
    }

    /// Go: `registry.MustRegister(collector)`.
    fn register<C: Collector + Clone + 'static>(&self, collector: C) -> Result<C, Error> {
        self.reg.register(Box::new(collector.clone()))?;
        Ok(collector)
    }

    /// Go: `newCounterWithoutLabels`.
    fn plain_counter(&self, spec: &Spec) -> Result<IntCounter, Error> {
        self.register(IntCounter::with_opts(self.opts(spec.name, spec.help))?)
    }

    /// Go: `newHistogram`.
    fn plain_histogram(&self, spec: &Spec) -> Result<Histogram, Error> {
        self.register(Histogram::with_opts(self.histogram_opts(
            spec.name,
            spec.help,
            spec.buckets,
        ))?)
    }

    fn counter_named(
        &self,
        name: &str,
        help: &str,
        labels: &[&str],
    ) -> Result<IntCounterVec, Error> {
        self.register(IntCounterVec::new(self.opts(name, help), labels)?)
    }

    fn histogram_named(
        &self,
        name: &str,
        help: &str,
        labels: &[&str],
        buckets: &[f64],
    ) -> Result<HistogramVec, Error> {
        self.register(HistogramVec::new(
            self.histogram_opts(name, help, buckets),
            labels,
        )?)
    }

    /// Go: `newCounter`.
    fn counter(&self, spec: &Spec) -> Result<IntCounterVec, Error> {
        self.counter_named(spec.name, spec.help, spec.labels)
    }

    /// Go: `newHistogramVec`.
    fn histogram(&self, spec: &Spec) -> Result<HistogramVec, Error> {
        self.histogram_named(spec.name, spec.help, spec.labels, spec.buckets)
    }

    /// A family Go registers only behind an `if !metricsDisabled...` check.
    fn optional_counter(&self, enabled: bool, spec: &Spec) -> Result<Option<IntCounterVec>, Error> {
        if enabled {
            self.counter(spec).map(Some)
        } else {
            Ok(None)
        }
    }

    fn optional_histogram(
        &self,
        enabled: bool,
        spec: &Spec,
    ) -> Result<Option<HistogramVec>, Error> {
        if enabled {
            self.histogram(spec).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Go: `createModulesMetrics`. Go ranges over the module map in random order; the set of
    /// registered families, and therefore the scrape, does not depend on the order.
    fn modules(
        &self,
        module_stage_names: &BTreeMap<String, Vec<String>>,
    ) -> Result<ModuleFamilies, Error> {
        let mut m = ModuleFamilies::default();
        let stage = &[STAGE_LABEL];
        for module in module_stage_names.keys() {
            let name = |suffix: &str| format!("modules_{module}_{suffix}");
            let duration = self.histogram_named(
                &name("duration"),
                "Amount of seconds a module processed a hook labeled by stage name.",
                stage,
                STANDARD_TIME_BUCKETS,
            )?;
            drop(m.duration.insert(module.clone(), duration));
            for (suffix, help, families) in [
                (
                    "called",
                    "Count of module calls labeled by stage name.",
                    &mut m.calls,
                ),
                (
                    "failed",
                    "Count of module fails labeled by stage name.",
                    &mut m.failures,
                ),
                (
                    "success_noops",
                    "Count of module successful noops labeled by stage name.",
                    &mut m.success_noops,
                ),
                (
                    "success_updates",
                    "Count of module successful updates labeled by stage name.",
                    &mut m.success_updates,
                ),
                (
                    "success_rejects",
                    "Count of module successful rejects labeled by stage name.",
                    &mut m.success_rejects,
                ),
                (
                    "execution_errors",
                    "Count of module execution errors labeled by stage name.",
                    &mut m.execution_errors,
                ),
                (
                    "timeouts",
                    "Count of module timeouts labeled by stage name.",
                    &mut m.timeouts,
                ),
            ] {
                let family = self.counter_named(&name(suffix), help, stage)?;
                drop(families.insert(module.clone(), family));
            }
        }
        Ok(m)
    }
}

impl Metrics {
    /// Initializes the Prometheus metrics with preloaded label values.
    ///
    /// Go: `prometheusmetrics.NewMetrics(cfg, disabledMetrics, syncerKeys, moduleStageNames)`.
    /// Go reads the adapter label values to preload from `openrtb_ext.CoreBidderNames()`; they
    /// are passed here as `core_bidder_names` (the engine lowercases them, as Go does). The
    /// families are registered in Go's order and the crate accepts the same names Go does, so
    /// the family the crate refuses is the one Go's `MustRegister` panics on.
    ///
    /// # Errors
    ///
    /// The prometheus crate's error for the first family it refuses: one whose name is not a
    /// valid metric name, as under a namespace, a subsystem or a module name that is not a valid
    /// part of one. Go panics there instead, with its own words (ledger:
    /// `metrics.registration-errors`).
    pub fn new(
        cfg: &PrometheusMetrics,
        disabled_metrics: DisabledMetrics,
        syncer_keys: &[String],
        module_stage_names: &BTreeMap<String, Vec<String>>,
        core_bidder_names: &[String],
    ) -> Result<Self, Error> {
        let metrics = Self::register_families(cfg, disabled_metrics, module_stage_names)?;
        preload::preload_label_values(
            &metrics,
            syncer_keys,
            module_stage_names,
            core_bidder_names,
        )?;
        Ok(metrics)
    }

    /// Go: the registrations of `NewMetrics`, before `preloadLabelValues`. The fields are
    /// initialized, and so registered, in Go's order.
    fn register_families(
        cfg: &PrometheusMetrics,
        disabled_metrics: DisabledMetrics,
        module_stage_names: &BTreeMap<String, Vec<String>>,
    ) -> Result<Self, Error> {
        let b = Builder::new(cfg);
        let t = Toggles(disabled_metrics);
        Ok(Self {
            connections_closed: b.plain_counter(&CONNECTIONS_CLOSED)?,
            connections_error: b.counter(&CONNECTIONS_ERROR)?,
            connections_opened: b.plain_counter(&CONNECTIONS_OPENED)?,
            tmax_timeout: b.plain_counter(&TMAX_TIMEOUT)?,
            cookie_sync: b.counter(&COOKIE_SYNC)?,
            set_uid: b.counter(&SET_UID)?,
            impressions: b.counter(&IMPRESSIONS)?,
            prebid_cache_write_timer: b.histogram(&PREBID_CACHE_WRITE_TIMER)?,
            requests: b.counter(&REQUESTS)?,
            requests_size: b.histogram(&REQUESTS_SIZE)?,
            debug_requests: b.plain_counter(&DEBUG_REQUESTS)?,
            requests_timer: b.histogram(&REQUESTS_TIMER)?,
            requests_without_cookie: b.counter(&REQUESTS_WITHOUT_COOKIE)?,
            stored_impressions_cache_result: b.counter(&STORED_IMPRESSIONS_CACHE_RESULT)?,
            stored_request_cache_result: b.counter(&STORED_REQUEST_CACHE_RESULT)?,
            account_cache_result: b.counter(&ACCOUNT_CACHE_RESULT)?,
            stored_account_fetch_timer: b.histogram(&STORED_ACCOUNT_FETCH_TIMER)?,
            stored_account_errors: b.counter(&STORED_ACCOUNT_ERRORS)?,
            fetcher_result: b.counter(&FETCHER_RESULT)?,
            fetcher_backend_fetch: b.counter(&FETCHER_BACKEND_FETCH)?,
            fetcher_backend_fetch_timer: b.histogram(&FETCHER_BACKEND_FETCH_TIMER)?,
            stored_amp_fetch_timer: b.histogram(&STORED_AMP_FETCH_TIMER)?,
            stored_amp_errors: b.counter(&STORED_AMP_ERRORS)?,
            stored_category_fetch_timer: b.histogram(&STORED_CATEGORY_FETCH_TIMER)?,
            stored_category_errors: b.counter(&STORED_CATEGORY_ERRORS)?,
            stored_request_fetch_timer: b.histogram(&STORED_REQUEST_FETCH_TIMER)?,
            stored_request_errors: b.counter(&STORED_REQUEST_ERRORS)?,
            stored_video_fetch_timer: b.histogram(&STORED_VIDEO_FETCH_TIMER)?,
            stored_video_errors: b.counter(&STORED_VIDEO_ERRORS)?,
            timeout_notifications: b.counter(&TIMEOUT_NOTIFICATIONS)?,
            dns_lookup_timer: b.plain_histogram(&DNS_LOOKUP_TIMER)?,
            tls_handshake_timer: b.plain_histogram(&TLS_HANDSHAKE_TIMER)?,
            privacy_ccpa: b.counter(&PRIVACY_CCPA)?,
            privacy_coppa: b.counter(&PRIVACY_COPPA)?,
            privacy_tcf: b.counter(&PRIVACY_TCF)?,
            privacy_lmt: b.counter(&PRIVACY_LMT)?,
            adapter_scrubbed_buyer_uids: b
                .optional_counter(t.scrubbed(), &ADAPTER_SCRUBBED_BUYER_UIDS)?,
            adapter_gdpr_blocked_requests: b
                .optional_counter(t.gdpr_blocked(), &ADAPTER_GDPR_BLOCKED_REQUESTS)?,
            stored_responses_fetch_timer: b.histogram(&STORED_RESPONSES_FETCH_TIMER)?,
            stored_responses_errors: b.counter(&STORED_RESPONSES_ERRORS)?,
            stored_responses: b.plain_counter(&STORED_RESPONSES)?,
            gvl_list_requests: b.plain_counter(&GVL_LIST_REQUESTS)?,
            live_gvl_fetch: b.counter(&LIVE_GVL_FETCH)?,
            adapter_bids: b.counter(&ADAPTER_BIDS)?,
            adapter_errors: b.counter(&ADAPTER_ERRORS)?,
            adapter_panics: b.counter(&ADAPTER_PANICS)?,
            adapter_prices: b.histogram(&ADAPTER_PRICES)?,
            adapter_requests: b.counter(&ADAPTER_REQUESTS)?,
            adapter_created_connections: b
                .optional_counter(t.connections(), &ADAPTER_CREATED_CONNECTIONS)?,
            adapter_reused_connections: b
                .optional_counter(t.connections(), &ADAPTER_REUSED_CONNECTIONS)?,
            adapter_connection_wait_time: b
                .optional_histogram(t.connections(), &ADAPTER_CONNECTION_WAIT_TIME)?,
            adapter_connection_dial_errors: b
                .optional_counter(t.dial(), &ADAPTER_CONNECTION_DIAL_ERRORS)?,
            adapter_connection_dial_time: b
                .optional_histogram(t.dial(), &ADAPTER_CONNECTION_DIAL_TIME)?,
            adapter_bid_response_validation_size_error: b
                .counter(&ADAPTER_BID_RESPONSE_VALIDATION_SIZE_ERROR)?,
            adapter_bid_response_validation_size_warn: b
                .counter(&ADAPTER_BID_RESPONSE_VALIDATION_SIZE_WARN)?,
            adapter_bid_response_secure_markup_error: b
                .counter(&ADAPTER_BID_RESPONSE_SECURE_MARKUP_ERROR)?,
            adapter_bid_response_secure_markup_warn: b
                .counter(&ADAPTER_BID_RESPONSE_SECURE_MARKUP_WARN)?,
            adapter_throttled: b.counter(&ADAPTER_THROTTLED)?,
            overhead_timer: b.histogram(&OVERHEAD_TIMER)?,
            adapter_requests_timer: b.histogram(&ADAPTER_REQUESTS_TIMER)?,
            bidder_server_response_timer: b.plain_histogram(&BIDDER_SERVER_RESPONSE_TIMER)?,
            syncer_requests: b.counter(&SYNCER_REQUESTS)?,
            syncer_sets: b.counter(&SYNCER_SETS)?,
            account_requests: b.counter(&ACCOUNT_REQUESTS)?,
            account_debug_requests: b.counter(&ACCOUNT_DEBUG_REQUESTS)?,
            account_bid_response_validation_size_error: b
                .counter(&ACCOUNT_BID_RESPONSE_VALIDATION_SIZE_ERROR)?,
            account_bid_response_validation_size_warn: b
                .counter(&ACCOUNT_BID_RESPONSE_VALIDATION_SIZE_WARN)?,
            account_bid_response_secure_markup_error: b
                .counter(&ACCOUNT_BID_RESPONSE_SECURE_MARKUP_ERROR)?,
            account_bid_response_secure_markup_warn: b
                .counter(&ACCOUNT_BID_RESPONSE_SECURE_MARKUP_WARN)?,
            requests_queue_timer: b.histogram(&REQUESTS_QUEUE_TIMER)?,
            account_stored_responses: b.counter(&ACCOUNT_STORED_RESPONSES)?,
            ads_cert_sign_timer: b.plain_histogram(&ADS_CERT_SIGN_TIMER)?,
            ads_cert_requests: b.counter(&ADS_CERT_REQUESTS)?,
            modules: b.modules(module_stage_names)?,
            disabled_flags: disabled_metrics,
            gatherer: b.reg,
        })
    }

    /// The registry to scrape.
    ///
    /// Go: `Metrics.Gatherer`.
    #[must_use]
    pub const fn gatherer(&self) -> &Registry {
        &self.gatherer
    }

    /// The scrape body: the prometheus crate's text format 0.0.4 ([`TEXT_FORMAT`]) of what the
    /// registry gathers. Go's `promhttp` handler writes the same families, samples and values,
    /// beside the Go collector's families; the ledger's `metrics` entries say where the two
    /// differ.
    ///
    /// # Errors
    ///
    /// The crate's error when it cannot encode a family, which it never has to for a gathered
    /// family: the registry leaves out the families without samples.
    pub fn text_exposition(&self) -> Result<String, Error> {
        TextEncoder::new().encode_to_string(&self.gatherer.gather())
    }

    const fn disabled(&self, flag: DisabledMetric) -> bool {
        self.disabled_flags.is(flag)
    }
}

/// Go: `strconv.FormatBool`.
const fn format_bool(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// Go: `vec.With(labels).Inc()`. The values always match the family's label names, so the
/// crate never refuses them; if it did, nothing would be recorded.
fn inc(family: &IntCounterVec, values: &[&str]) {
    inc_by(family, values, 1);
}

/// Go: `vec.With(labels).Add(float64(v))`, which adds a whole number to the counter's integer
/// part.
fn inc_by(family: &IntCounterVec, values: &[&str], v: u64) {
    if let Ok(counter) = family.get_metric_with_label_values(values) {
        counter.inc_by(v);
    }
}

/// Go: `vec.With(labels).Observe(v)`.
fn observe(family: &HistogramVec, values: &[&str], v: f64) {
    if let Ok(histogram) = family.get_metric_with_label_values(values) {
        histogram.observe(v);
    }
}

/// The integer Go's `counter.Add(float64(n))` adds: `client_golang` adds an integer-valued
/// increment to the counter's `uint64` part, as [`IntCounter`] keeps its value, so the increment
/// is `uint64(float64(n))`, `n` rounded to the nearest `f64` (ties to even), which changes only
/// counts above 2^53.
fn count(n: usize) -> u64 {
    let n = u64::try_from(n).unwrap_or(u64::MAX);
    let dropped = n.bit_width().saturating_sub(f64::MANTISSA_DIGITS);
    if dropped == 0 {
        return n;
    }
    let unit = 1_u64 << dropped;
    let rest = n % unit;
    let down = n - rest;
    let half = unit / 2;
    if rest > half || (rest == half && (n / unit) % 2 == 1) {
        down.saturating_add(unit)
    } else {
        down
    }
}

impl MetricsEngine for Metrics {
    fn record_connection_accept(&self, success: bool) {
        if success {
            self.connections_opened.inc();
        } else {
            inc(&self.connections_error, &[CONNECTION_ACCEPT_ERROR]);
        }
    }

    fn record_tmax_timeout(&self) {
        self.tmax_timeout.inc();
    }

    fn record_connection_close(&self, success: bool) {
        if success {
            self.connections_closed.inc();
        } else {
            inc(&self.connections_error, &[CONNECTION_CLOSE_ERROR]);
        }
    }

    fn record_request(&self, labels: &Labels) {
        inc(
            &self.requests,
            &[labels.r_type.as_str(), labels.request_status.as_str()],
        );

        if labels.request_size > 0 && labels.r_type != RequestType::AMP {
            let endpoint = get_endpoint_from_request_type(labels.r_type);
            observe(
                &self.requests_size,
                &[endpoint.as_str()],
                float_from_usize(labels.request_size),
            );
        }

        if labels.cookie_flag == CookieFlag::NO {
            inc(&self.requests_without_cookie, &[labels.r_type.as_str()]);
        }

        if labels.pub_id != PUBLISHER_UNKNOWN {
            inc(&self.account_requests, &[&labels.pub_id]);
        }
    }

    fn record_imps(&self, labels: ImpLabels) {
        inc(
            &self.impressions,
            &[
                format_bool(labels.banner_imps()),
                format_bool(labels.video_imps()),
                format_bool(labels.audio_imps()),
                format_bool(labels.native_imps()),
            ],
        );
    }

    fn record_request_time(&self, labels: &Labels, length: GoDuration) {
        if labels.request_status == RequestStatus::OK {
            observe(
                &self.requests_timer,
                &[labels.r_type.as_str()],
                length.seconds(),
            );
        }
    }

    fn record_overhead_time(&self, overhead: OverheadType, length: GoDuration) {
        observe(&self.overhead_timer, &[overhead.as_str()], length.seconds());
    }

    fn record_adapter_request(&self, labels: &AdapterLabels) {
        let lower_cased_adapter = to_lower(&labels.adapter);
        inc(
            &self.adapter_requests,
            &[
                &lower_cased_adapter,
                labels.cookie_flag.as_str(),
                format_bool(labels.adapter_bids == AdapterBid::PRESENT),
            ],
        );

        for err in &labels.adapter_errors {
            inc(&self.adapter_errors, &[&lower_cased_adapter, err.as_str()]);
        }
    }

    /// Keeps track of created and reused connections to adapter bidders and the time from the
    /// connection request to the connection creation, or reuse from the pool.
    fn record_adapter_connections(
        &self,
        adapter_name: &str,
        conn_was_reused: bool,
        conn_wait_time: GoDuration,
    ) {
        let lower_cased_adapter_name = to_lower(adapter_name);
        if self.disabled(DisabledMetric::AdapterConnectionMetrics) {
            return;
        }

        let connections = if conn_was_reused {
            &self.adapter_reused_connections
        } else {
            &self.adapter_created_connections
        };
        if let Some(connections) = connections {
            inc(connections, &[&lower_cased_adapter_name]);
        }

        if let Some(wait_time) = &self.adapter_connection_wait_time {
            observe(
                wait_time,
                &[&lower_cased_adapter_name],
                conn_wait_time.seconds(),
            );
        }
    }

    fn record_dns_time(&self, dns_lookup_time: GoDuration) {
        self.dns_lookup_timer.observe(dns_lookup_time.seconds());
    }

    fn record_tls_handshake_time(&self, tls_handshake_time: GoDuration) {
        self.tls_handshake_timer
            .observe(tls_handshake_time.seconds());
    }

    fn record_bidder_server_response_time(&self, bidder_server_response_time: GoDuration) {
        self.bidder_server_response_timer
            .observe(bidder_server_response_time.seconds());
    }

    fn record_adapter_panic(&self, labels: &AdapterLabels) {
        inc(&self.adapter_panics, &[&to_lower(&labels.adapter)]);
    }

    fn record_adapter_bid_received(&self, labels: &AdapterLabels, _bid_type: &str, has_adm: bool) {
        let markup_delivery = if has_adm {
            MARKUP_DELIVERY_ADM
        } else {
            MARKUP_DELIVERY_NURL
        };
        inc(
            &self.adapter_bids,
            &[&to_lower(&labels.adapter), markup_delivery],
        );
    }

    fn record_adapter_price(&self, labels: &AdapterLabels, cpm: f64) {
        observe(&self.adapter_prices, &[&to_lower(&labels.adapter)], cpm);
    }

    fn record_adapter_time(&self, labels: &AdapterLabels, length: GoDuration) {
        if labels.adapter_errors.is_empty() {
            observe(
                &self.adapter_requests_timer,
                &[&to_lower(&labels.adapter)],
                length.seconds(),
            );
        }
    }

    fn record_cookie_sync(&self, status: CookieSyncStatus) {
        inc(&self.cookie_sync, &[status.as_str()]);
    }

    fn record_syncer_request(&self, key: &str, status: SyncerCookieSyncStatus) {
        inc(&self.syncer_requests, &[key, status.as_str()]);
    }

    fn record_set_uid(&self, status: SetUidStatus) {
        inc(&self.set_uid, &[status.as_str()]);
    }

    fn record_syncer_set(&self, key: &str, status: SyncerSetUidStatus) {
        inc(&self.syncer_sets, &[key, status.as_str()]);
    }

    fn record_stored_req_cache_result(&self, cache_result: CacheResult, inc: usize) {
        inc_by(
            &self.stored_request_cache_result,
            &[cache_result.as_str()],
            count(inc),
        );
    }

    fn record_stored_imp_cache_result(&self, cache_result: CacheResult, inc: usize) {
        inc_by(
            &self.stored_impressions_cache_result,
            &[cache_result.as_str()],
            count(inc),
        );
    }

    fn record_account_cache_result(&self, cache_result: CacheResult, inc: usize) {
        inc_by(
            &self.account_cache_result,
            &[cache_result.as_str()],
            count(inc),
        );
    }

    fn record_stored_data_fetch_time(&self, labels: StoredDataLabels, length: GoDuration) {
        let timer = match stored_data_index(labels.data_type) {
            Some(StoredDataIndex::Account) => &self.stored_account_fetch_timer,
            Some(StoredDataIndex::Amp) => &self.stored_amp_fetch_timer,
            Some(StoredDataIndex::Category) => &self.stored_category_fetch_timer,
            Some(StoredDataIndex::Request) => &self.stored_request_fetch_timer,
            Some(StoredDataIndex::Video) => &self.stored_video_fetch_timer,
            Some(StoredDataIndex::Response) => &self.stored_responses_fetch_timer,
            None => return,
        };
        observe(timer, &[labels.data_fetch_type.as_str()], length.seconds());
    }

    fn record_stored_data_error(&self, labels: StoredDataLabels) {
        let errors = match stored_data_index(labels.data_type) {
            Some(StoredDataIndex::Account) => &self.stored_account_errors,
            Some(StoredDataIndex::Amp) => &self.stored_amp_errors,
            Some(StoredDataIndex::Category) => &self.stored_category_errors,
            Some(StoredDataIndex::Request) => &self.stored_request_errors,
            Some(StoredDataIndex::Video) => &self.stored_video_errors,
            Some(StoredDataIndex::Response) => &self.stored_responses_errors,
            None => return,
        };
        inc(errors, &[labels.error.as_str()]);
    }

    fn record_fetcher_result(&self, subsystem: &str, result: FetcherResult) {
        inc(&self.fetcher_result, &[subsystem, result.as_str()]);
    }

    fn record_fetcher_backend_fetch(
        &self,
        subsystem: &str,
        operation: FetcherOperation,
        result: FetcherBackendResult,
        length: GoDuration,
    ) {
        inc(
            &self.fetcher_backend_fetch,
            &[subsystem, operation.as_str(), result.as_str()],
        );
        observe(
            &self.fetcher_backend_fetch_timer,
            &[subsystem, operation.as_str()],
            length.seconds(),
        );
    }

    fn record_prebid_cache_request_time(&self, success: bool, length: GoDuration) {
        observe(
            &self.prebid_cache_write_timer,
            &[format_bool(success)],
            length.seconds(),
        );
    }

    fn record_request_queue_time(
        &self,
        success: bool,
        request_type: RequestType,
        length: GoDuration,
    ) {
        let success_label_formatted = if success {
            REQUEST_SUCCESS_LABEL
        } else {
            REQUEST_REJECT_LABEL
        };
        observe(
            &self.requests_queue_timer,
            &[request_type.as_str(), success_label_formatted],
            length.seconds(),
        );
    }

    fn record_timeout_notice(&self, success: bool) {
        let label = if success {
            REQUEST_SUCCESSFUL
        } else {
            REQUEST_FAILED
        };
        inc(&self.timeout_notifications, &[label]);
    }

    fn record_request_privacy(&self, privacy: PrivacyLabels) {
        if privacy.ccpa.provided {
            inc(
                &self.privacy_ccpa,
                &[SOURCE_REQUEST, format_bool(privacy.ccpa.enforced)],
            );
        }

        if privacy.coppa_enforced {
            inc(&self.privacy_coppa, &[SOURCE_REQUEST]);
        }

        if privacy.gdpr.enforced {
            inc(
                &self.privacy_tcf,
                &[privacy.gdpr.tcf_version.as_str(), SOURCE_REQUEST],
            );
        }

        if privacy.lmt_enforced {
            inc(&self.privacy_lmt, &[SOURCE_REQUEST]);
        }
    }

    fn record_adapter_buyer_uid_scrubbed(&self, adapter_name: &str) {
        if self.disabled(DisabledMetric::AdapterBuyerUidScrubbed) {
            return;
        }
        if let Some(scrubbed) = &self.adapter_scrubbed_buyer_uids {
            inc(scrubbed, &[&to_lower(adapter_name)]);
        }
    }

    fn record_adapter_gdpr_request_blocked(&self, adapter_name: &str) {
        if self.disabled(DisabledMetric::AdapterGdprRequestBlocked) {
            return;
        }
        if let Some(blocked) = &self.adapter_gdpr_blocked_requests {
            inc(blocked, &[&to_lower(adapter_name)]);
        }
    }

    fn record_debug_request(&self, debug_enabled: bool, pub_id: &str) {
        if debug_enabled {
            self.debug_requests.inc();
            if !self.disabled(DisabledMetric::AccountDebug) && pub_id != PUBLISHER_UNKNOWN {
                inc(&self.account_debug_requests, &[pub_id]);
            }
        }
    }

    fn record_stored_response(&self, pub_id: &str) {
        self.stored_responses.inc();
        if !self.disabled(DisabledMetric::AccountStoredResponses) && pub_id != PUBLISHER_UNKNOWN {
            inc(&self.account_stored_responses, &[pub_id]);
        }
    }

    fn record_gvl_list_request(&self) {
        self.gvl_list_requests.inc();
    }

    fn record_live_gvl_fetch(&self, success: bool) {
        let label = if success {
            REQUEST_SUCCESSFUL
        } else {
            REQUEST_FAILED
        };
        inc(&self.live_gvl_fetch, &[label]);
    }

    fn record_ads_cert_req(&self, success: bool) {
        let label = if success {
            REQUEST_SUCCESSFUL
        } else {
            REQUEST_FAILED
        };
        inc(&self.ads_cert_requests, &[label]);
    }

    fn record_ads_cert_sign_time(&self, ads_cert_sign_time: GoDuration) {
        self.ads_cert_sign_timer
            .observe(ads_cert_sign_time.seconds());
    }

    // Go labels the bid validation series with the literal value `success="success"` (the
    // label name, `successLabel`), while preloading uses "true"/"false".

    fn record_bid_validation_creative_size_error(&self, adapter: &str, account: &str) {
        inc(
            &self.adapter_bid_response_validation_size_error,
            &[&to_lower(adapter), SUCCESS_LABEL],
        );
        if !self.disabled(DisabledMetric::AccountAdapterDetails) && account != PUBLISHER_UNKNOWN {
            inc(
                &self.account_bid_response_validation_size_error,
                &[account, SUCCESS_LABEL],
            );
        }
    }

    fn record_bid_validation_creative_size_warn(&self, adapter: &str, account: &str) {
        inc(
            &self.adapter_bid_response_validation_size_warn,
            &[&to_lower(adapter), SUCCESS_LABEL],
        );
        if !self.disabled(DisabledMetric::AccountAdapterDetails) && account != PUBLISHER_UNKNOWN {
            inc(
                &self.account_bid_response_validation_size_warn,
                &[account, SUCCESS_LABEL],
            );
        }
    }

    fn record_bid_validation_secure_markup_error(&self, adapter: &str, account: &str) {
        inc(
            &self.adapter_bid_response_secure_markup_error,
            &[&to_lower(adapter), SUCCESS_LABEL],
        );
        if !self.disabled(DisabledMetric::AccountAdapterDetails) && account != PUBLISHER_UNKNOWN {
            inc(
                &self.account_bid_response_secure_markup_error,
                &[account, SUCCESS_LABEL],
            );
        }
    }

    fn record_bid_validation_secure_markup_warn(&self, adapter: &str, account: &str) {
        inc(
            &self.adapter_bid_response_secure_markup_warn,
            &[&to_lower(adapter), SUCCESS_LABEL],
        );
        if !self.disabled(DisabledMetric::AccountAdapterDetails) && account != PUBLISHER_UNKNOWN {
            inc(
                &self.account_bid_response_secure_markup_warn,
                &[account, SUCCESS_LABEL],
            );
        }
    }

    // Go indexes the module maps directly; a module that was not registered is a nil family and
    // Go panics. No Go caller records an unregistered module; Rust records nothing.

    fn record_module_called(&self, labels: &ModuleLabels, duration: GoDuration) {
        if let Some(calls) = self.modules.calls.get(&labels.module) {
            inc(calls, &[&labels.stage]);
        }
        if let Some(timer) = self.modules.duration.get(&labels.module) {
            observe(timer, &[&labels.stage], duration.seconds());
        }
    }

    fn record_module_failed(&self, labels: &ModuleLabels) {
        inc_module(&self.modules.failures, labels);
    }

    fn record_module_success_nooped(&self, labels: &ModuleLabels) {
        inc_module(&self.modules.success_noops, labels);
    }

    fn record_module_success_updated(&self, labels: &ModuleLabels) {
        inc_module(&self.modules.success_updates, labels);
    }

    fn record_module_success_rejected(&self, labels: &ModuleLabels) {
        inc_module(&self.modules.success_rejects, labels);
    }

    fn record_module_execution_error(&self, labels: &ModuleLabels) {
        inc_module(&self.modules.execution_errors, labels);
    }

    fn record_module_timeout(&self, labels: &ModuleLabels) {
        inc_module(&self.modules.timeouts, labels);
    }

    fn record_adapter_throttled(&self, adapter_name: &str) {
        inc(&self.adapter_throttled, &[&to_lower(adapter_name)]);
    }

    fn record_adapter_connection_dial_error(&self, adapter_name: &str) {
        if let Some(dial_errors) = &self.adapter_connection_dial_errors {
            inc(dial_errors, &[&to_lower(adapter_name)]);
        }
    }

    fn record_adapter_connection_dial_time(&self, adapter_name: &str, dial_start_time: GoDuration) {
        if let Some(dial_time) = &self.adapter_connection_dial_time {
            observe(
                dial_time,
                &[&to_lower(adapter_name)],
                dial_start_time.seconds(),
            );
        }
    }
}

fn inc_module(families: &BTreeMap<String, IntCounterVec>, labels: &ModuleLabels) {
    if let Some(family) = families.get(&labels.module) {
        inc(family, &[&labels.stage]);
    }
}

/// The `switch labels.DataType` arms of `RecordStoredDataFetchTime`/`RecordStoredDataError`.
#[derive(Clone, Copy)]
enum StoredDataIndex {
    Account,
    Amp,
    Category,
    Request,
    Video,
    Response,
}

fn stored_data_index(data_type: StoredDataType) -> Option<StoredDataIndex> {
    [
        (StoredDataType::ACCOUNT, StoredDataIndex::Account),
        (StoredDataType::AMP, StoredDataIndex::Amp),
        (StoredDataType::CATEGORY, StoredDataIndex::Category),
        (StoredDataType::REQUEST, StoredDataIndex::Request),
        (StoredDataType::VIDEO, StoredDataIndex::Video),
        (StoredDataType::RESPONSE, StoredDataIndex::Response),
    ]
    .into_iter()
    .find_map(|(t, index)| (t == data_type).then_some(index))
}

#[cfg(test)]
#[path = "prometheus_test.rs"]
mod tests;
