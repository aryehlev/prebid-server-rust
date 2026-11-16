//! The Prometheus metrics engine.
//!
//! Go: `metrics/prometheus/prometheus.go` (package `prometheusmetrics`).
//!
//! Metric names, help strings, label names and values, buckets, registration conditions and
//! preloading are Go's. The scrape is produced by [`Metrics::gatherer`] and
//! [`expfmt::encode_text`], which reproduce `client_golang` v1.12.1 byte for byte.
//!
//! Go also registers `collectors.NewGoCollector()` (the `go_*` runtime families, prefixed with
//! `<namespace>_<subsystem>_`). Those report Go runtime internals and have no Rust equivalent,
//! so they are not produced; no other runtime collector is registered either.

pub mod expfmt;
mod preload;
pub mod registry;
mod type_conversion;

use std::{collections::BTreeMap, sync::Arc};

use self::registry::{CounterVec, Desc, HistogramVec, RegistrationError, Registry, build_fq_name};
use crate::{
    goconv,
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
    duration: BTreeMap<String, Arc<HistogramVec>>,
    calls: BTreeMap<String, Arc<CounterVec>>,
    failures: BTreeMap<String, Arc<CounterVec>>,
    success_noops: BTreeMap<String, Arc<CounterVec>>,
    success_updates: BTreeMap<String, Arc<CounterVec>>,
    success_rejects: BTreeMap<String, Arc<CounterVec>>,
    execution_errors: BTreeMap<String, Arc<CounterVec>>,
    timeouts: BTreeMap<String, Arc<CounterVec>>,
}

/// The Prometheus metrics backing the [`MetricsEngine`] implementation.
///
/// Go: `prometheusmetrics.Metrics`. A family that Go leaves `nil` because its disabled flag is
/// set at construction is `None`; recording into it is a no-op (Go would dereference nil, which
/// no Go caller does).
#[derive(Debug)]
pub struct Metrics {
    gatherer: Registry,

    // General metrics.
    connections_closed: Arc<CounterVec>,
    connections_error: Arc<CounterVec>,
    connections_opened: Arc<CounterVec>,
    tmax_timeout: Arc<CounterVec>,
    cookie_sync: Arc<CounterVec>,
    set_uid: Arc<CounterVec>,
    impressions: Arc<CounterVec>,
    prebid_cache_write_timer: Arc<HistogramVec>,
    requests: Arc<CounterVec>,
    requests_size: Arc<HistogramVec>,
    debug_requests: Arc<CounterVec>,
    requests_timer: Arc<HistogramVec>,
    requests_queue_timer: Arc<HistogramVec>,
    requests_without_cookie: Arc<CounterVec>,
    stored_impressions_cache_result: Arc<CounterVec>,
    stored_request_cache_result: Arc<CounterVec>,
    account_cache_result: Arc<CounterVec>,
    stored_account_fetch_timer: Arc<HistogramVec>,
    stored_account_errors: Arc<CounterVec>,
    stored_amp_fetch_timer: Arc<HistogramVec>,
    stored_amp_errors: Arc<CounterVec>,
    stored_category_fetch_timer: Arc<HistogramVec>,
    stored_category_errors: Arc<CounterVec>,
    stored_request_fetch_timer: Arc<HistogramVec>,
    stored_request_errors: Arc<CounterVec>,
    stored_video_fetch_timer: Arc<HistogramVec>,
    stored_video_errors: Arc<CounterVec>,
    timeout_notifications: Arc<CounterVec>,
    dns_lookup_timer: Arc<HistogramVec>,
    tls_handshake_timer: Arc<HistogramVec>,
    privacy_ccpa: Arc<CounterVec>,
    privacy_coppa: Arc<CounterVec>,
    privacy_lmt: Arc<CounterVec>,
    privacy_tcf: Arc<CounterVec>,
    stored_responses: Arc<CounterVec>,
    gvl_list_requests: Arc<CounterVec>,
    live_gvl_fetch: Arc<CounterVec>,
    stored_responses_fetch_timer: Arc<HistogramVec>,
    stored_responses_errors: Arc<CounterVec>,
    ads_cert_requests: Arc<CounterVec>,
    ads_cert_sign_timer: Arc<HistogramVec>,
    bidder_server_response_timer: Arc<HistogramVec>,

    // Fetchers 2.0 metrics.
    fetcher_result: Arc<CounterVec>,
    fetcher_backend_fetch: Arc<CounterVec>,
    fetcher_backend_fetch_timer: Arc<HistogramVec>,

    // Adapter metrics.
    adapter_bids: Arc<CounterVec>,
    adapter_errors: Arc<CounterVec>,
    adapter_panics: Arc<CounterVec>,
    adapter_prices: Arc<HistogramVec>,
    adapter_requests: Arc<CounterVec>,
    overhead_timer: Arc<HistogramVec>,
    adapter_requests_timer: Arc<HistogramVec>,
    adapter_reused_connections: Option<Arc<CounterVec>>,
    adapter_created_connections: Option<Arc<CounterVec>>,
    adapter_connection_wait_time: Option<Arc<HistogramVec>>,
    adapter_scrubbed_buyer_uids: Option<Arc<CounterVec>>,
    adapter_gdpr_blocked_requests: Option<Arc<CounterVec>>,
    adapter_bid_response_validation_size_error: Arc<CounterVec>,
    adapter_bid_response_validation_size_warn: Arc<CounterVec>,
    adapter_bid_response_secure_markup_error: Arc<CounterVec>,
    adapter_bid_response_secure_markup_warn: Arc<CounterVec>,
    adapter_throttled: Arc<CounterVec>,
    adapter_connection_dial_errors: Option<Arc<CounterVec>>,
    adapter_connection_dial_time: Option<Arc<HistogramVec>>,

    // Syncer metrics.
    syncer_requests: Arc<CounterVec>,
    syncer_sets: Arc<CounterVec>,

    // Account metrics.
    account_requests: Arc<CounterVec>,
    account_debug_requests: Arc<CounterVec>,
    account_stored_responses: Arc<CounterVec>,
    account_bid_response_validation_size_error: Arc<CounterVec>,
    account_bid_response_validation_size_warn: Arc<CounterVec>,
    account_bid_response_secure_markup_error: Arc<CounterVec>,
    account_bid_response_secure_markup_warn: Arc<CounterVec>,

    // Module metrics.
    modules: ModuleFamilies,

    /// Go: `metricsDisabled`.
    disabled_flags: DisabledMetrics,
}

/// Registers families with Go's `newCounter`/`newHistogramVec` options (namespace and subsystem
/// from the configuration) into one registry.
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

    fn desc(&self, name: &str, help: &str, labels: &[&str]) -> Result<Desc, RegistrationError> {
        Desc::new(
            build_fq_name(&self.cfg.namespace, &self.cfg.subsystem, name),
            help,
            labels,
        )
    }

    fn counter_named(
        &mut self,
        name: &str,
        help: &str,
        labels: &[&str],
    ) -> Result<Arc<CounterVec>, RegistrationError> {
        let desc = self.desc(name, help, labels)?;
        self.reg.register_counter(desc)
    }

    fn histogram_named(
        &mut self,
        name: &str,
        help: &str,
        labels: &[&str],
        buckets: &[f64],
    ) -> Result<Arc<HistogramVec>, RegistrationError> {
        let desc = self.desc(name, help, labels)?;
        self.reg.register_histogram(desc, buckets)
    }

    fn counter(&mut self, spec: &Spec) -> Result<Arc<CounterVec>, RegistrationError> {
        self.counter_named(spec.name, spec.help, spec.labels)
    }

    fn histogram(&mut self, spec: &Spec) -> Result<Arc<HistogramVec>, RegistrationError> {
        self.histogram_named(spec.name, spec.help, spec.labels, spec.buckets)
    }

    /// A family Go registers only behind an `if !metricsDisabled...` check.
    fn optional_counter(
        &mut self,
        enabled: bool,
        spec: &Spec,
    ) -> Result<Option<Arc<CounterVec>>, RegistrationError> {
        if enabled {
            self.counter(spec).map(Some)
        } else {
            Ok(None)
        }
    }

    fn optional_histogram(
        &mut self,
        enabled: bool,
        spec: &Spec,
    ) -> Result<Option<Arc<HistogramVec>>, RegistrationError> {
        if enabled {
            self.histogram(spec).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Go: `createModulesMetrics`. Go ranges over the module map in random order; the set of
    /// registered families, and therefore the scrape, does not depend on the order.
    fn modules(
        &mut self,
        module_stage_names: &BTreeMap<String, Vec<String>>,
    ) -> Result<ModuleFamilies, RegistrationError> {
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
    /// families are registered in Go's order, so the first registration error is Go's.
    ///
    /// # Errors
    ///
    /// The registration error Go's `MustRegister` panics with when a family name is invalid
    /// (for example a namespace that is not a valid metric name prefix).
    pub fn new(
        cfg: &PrometheusMetrics,
        disabled_metrics: DisabledMetrics,
        syncer_keys: &[String],
        module_stage_names: &BTreeMap<String, Vec<String>>,
        core_bidder_names: &[String],
    ) -> Result<Self, RegistrationError> {
        let mut b = Builder::new(cfg);
        let t = Toggles(disabled_metrics);

        // The fields are initialized, and so registered, in Go's order.
        let metrics = Self {
            connections_closed: b.counter(&CONNECTIONS_CLOSED)?,
            connections_error: b.counter(&CONNECTIONS_ERROR)?,
            connections_opened: b.counter(&CONNECTIONS_OPENED)?,
            tmax_timeout: b.counter(&TMAX_TIMEOUT)?,
            cookie_sync: b.counter(&COOKIE_SYNC)?,
            set_uid: b.counter(&SET_UID)?,
            impressions: b.counter(&IMPRESSIONS)?,
            prebid_cache_write_timer: b.histogram(&PREBID_CACHE_WRITE_TIMER)?,
            requests: b.counter(&REQUESTS)?,
            requests_size: b.histogram(&REQUESTS_SIZE)?,
            debug_requests: b.counter(&DEBUG_REQUESTS)?,
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
            dns_lookup_timer: b.histogram(&DNS_LOOKUP_TIMER)?,
            tls_handshake_timer: b.histogram(&TLS_HANDSHAKE_TIMER)?,
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
            stored_responses: b.counter(&STORED_RESPONSES)?,
            gvl_list_requests: b.counter(&GVL_LIST_REQUESTS)?,
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
            bidder_server_response_timer: b.histogram(&BIDDER_SERVER_RESPONSE_TIMER)?,
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
            ads_cert_sign_timer: b.histogram(&ADS_CERT_SIGN_TIMER)?,
            ads_cert_requests: b.counter(&ADS_CERT_REQUESTS)?,
            modules: b.modules(module_stage_names)?,
            disabled_flags: disabled_metrics,
            gatherer: b.reg,
        };

        preload::preload_label_values(&metrics, syncer_keys, module_stage_names, core_bidder_names);

        Ok(metrics)
    }

    /// The registry to scrape.
    ///
    /// Go: `Metrics.Gatherer`.
    #[must_use]
    pub const fn gatherer(&self) -> &Registry {
        &self.gatherer
    }

    /// The scrape body in the text exposition format: the body of Go's `promhttp` handler for a
    /// text scrape, without the Go collector families.
    #[must_use]
    pub fn text_exposition(&self) -> String {
        expfmt::encode_text(&self.gatherer.gather())
    }

    const fn disabled(&self, flag: DisabledMetric) -> bool {
        self.disabled_flags.is(flag)
    }
}

/// Go: `strconv.FormatBool`.
const fn format_bool(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

impl MetricsEngine for Metrics {
    fn record_connection_accept(&self, success: bool) {
        if success {
            self.connections_opened.inc(&[]);
        } else {
            self.connections_error.inc(&[CONNECTION_ACCEPT_ERROR]);
        }
    }

    fn record_tmax_timeout(&self) {
        self.tmax_timeout.inc(&[]);
    }

    fn record_connection_close(&self, success: bool) {
        if success {
            self.connections_closed.inc(&[]);
        } else {
            self.connections_error.inc(&[CONNECTION_CLOSE_ERROR]);
        }
    }

    fn record_request(&self, labels: &Labels) {
        self.requests
            .inc(&[labels.r_type.as_str(), labels.request_status.as_str()]);

        if labels.request_size > 0 && labels.r_type != RequestType::AMP {
            let endpoint = get_endpoint_from_request_type(labels.r_type);
            self.requests_size.observe(
                &[endpoint.as_str()],
                goconv::usize_to_f64(labels.request_size),
            );
        }

        if labels.cookie_flag == CookieFlag::NO {
            self.requests_without_cookie.inc(&[labels.r_type.as_str()]);
        }

        if labels.pub_id != PUBLISHER_UNKNOWN {
            self.account_requests.inc(&[&labels.pub_id]);
        }
    }

    fn record_imps(&self, labels: ImpLabels) {
        self.impressions.inc(&[
            format_bool(labels.banner_imps()),
            format_bool(labels.video_imps()),
            format_bool(labels.audio_imps()),
            format_bool(labels.native_imps()),
        ]);
    }

    fn record_request_time(&self, labels: &Labels, length: GoDuration) {
        if labels.request_status == RequestStatus::OK {
            self.requests_timer
                .observe(&[labels.r_type.as_str()], length.seconds());
        }
    }

    fn record_overhead_time(&self, overhead: OverheadType, length: GoDuration) {
        self.overhead_timer
            .observe(&[overhead.as_str()], length.seconds());
    }

    fn record_adapter_request(&self, labels: &AdapterLabels) {
        let lower_cased_adapter = goconv::to_lower(&labels.adapter);
        self.adapter_requests.inc(&[
            &lower_cased_adapter,
            labels.cookie_flag.as_str(),
            format_bool(labels.adapter_bids == AdapterBid::PRESENT),
        ]);

        for err in &labels.adapter_errors {
            self.adapter_errors
                .inc(&[&lower_cased_adapter, err.as_str()]);
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
        let lower_cased_adapter_name = goconv::to_lower(adapter_name);
        if self.disabled(DisabledMetric::AdapterConnectionMetrics) {
            return;
        }

        let connections = if conn_was_reused {
            &self.adapter_reused_connections
        } else {
            &self.adapter_created_connections
        };
        if let Some(connections) = connections {
            connections.inc(&[&lower_cased_adapter_name]);
        }

        if let Some(wait_time) = &self.adapter_connection_wait_time {
            wait_time.observe(&[&lower_cased_adapter_name], conn_wait_time.seconds());
        }
    }

    fn record_dns_time(&self, dns_lookup_time: GoDuration) {
        self.dns_lookup_timer
            .observe(&[], dns_lookup_time.seconds());
    }

    fn record_tls_handshake_time(&self, tls_handshake_time: GoDuration) {
        self.tls_handshake_timer
            .observe(&[], tls_handshake_time.seconds());
    }

    fn record_bidder_server_response_time(&self, bidder_server_response_time: GoDuration) {
        self.bidder_server_response_timer
            .observe(&[], bidder_server_response_time.seconds());
    }

    fn record_adapter_panic(&self, labels: &AdapterLabels) {
        self.adapter_panics
            .inc(&[&goconv::to_lower(&labels.adapter)]);
    }

    fn record_adapter_bid_received(&self, labels: &AdapterLabels, _bid_type: &str, has_adm: bool) {
        let markup_delivery = if has_adm {
            MARKUP_DELIVERY_ADM
        } else {
            MARKUP_DELIVERY_NURL
        };
        self.adapter_bids
            .inc(&[&goconv::to_lower(&labels.adapter), markup_delivery]);
    }

    fn record_adapter_price(&self, labels: &AdapterLabels, cpm: f64) {
        self.adapter_prices
            .observe(&[&goconv::to_lower(&labels.adapter)], cpm);
    }

    fn record_adapter_time(&self, labels: &AdapterLabels, length: GoDuration) {
        if labels.adapter_errors.is_empty() {
            self.adapter_requests_timer
                .observe(&[&goconv::to_lower(&labels.adapter)], length.seconds());
        }
    }

    fn record_cookie_sync(&self, status: CookieSyncStatus) {
        self.cookie_sync.inc(&[status.as_str()]);
    }

    fn record_syncer_request(&self, key: &str, status: SyncerCookieSyncStatus) {
        self.syncer_requests.inc(&[key, status.as_str()]);
    }

    fn record_set_uid(&self, status: SetUidStatus) {
        self.set_uid.inc(&[status.as_str()]);
    }

    fn record_syncer_set(&self, key: &str, status: SyncerSetUidStatus) {
        self.syncer_sets.inc(&[key, status.as_str()]);
    }

    fn record_stored_req_cache_result(&self, cache_result: CacheResult, inc: usize) {
        self.stored_request_cache_result
            .add(&[cache_result.as_str()], goconv::usize_to_f64(inc));
    }

    fn record_stored_imp_cache_result(&self, cache_result: CacheResult, inc: usize) {
        self.stored_impressions_cache_result
            .add(&[cache_result.as_str()], goconv::usize_to_f64(inc));
    }

    fn record_account_cache_result(&self, cache_result: CacheResult, inc: usize) {
        self.account_cache_result
            .add(&[cache_result.as_str()], goconv::usize_to_f64(inc));
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
        timer.observe(&[labels.data_fetch_type.as_str()], length.seconds());
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
        errors.inc(&[labels.error.as_str()]);
    }

    fn record_fetcher_result(&self, subsystem: &str, result: FetcherResult) {
        self.fetcher_result.inc(&[subsystem, result.as_str()]);
    }

    fn record_fetcher_backend_fetch(
        &self,
        subsystem: &str,
        operation: FetcherOperation,
        result: FetcherBackendResult,
        length: GoDuration,
    ) {
        self.fetcher_backend_fetch
            .inc(&[subsystem, operation.as_str(), result.as_str()]);
        self.fetcher_backend_fetch_timer
            .observe(&[subsystem, operation.as_str()], length.seconds());
    }

    fn record_prebid_cache_request_time(&self, success: bool, length: GoDuration) {
        self.prebid_cache_write_timer
            .observe(&[format_bool(success)], length.seconds());
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
        self.requests_queue_timer.observe(
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
        self.timeout_notifications.inc(&[label]);
    }

    fn record_request_privacy(&self, privacy: PrivacyLabels) {
        if privacy.ccpa.provided {
            self.privacy_ccpa
                .inc(&[SOURCE_REQUEST, format_bool(privacy.ccpa.enforced)]);
        }

        if privacy.coppa_enforced {
            self.privacy_coppa.inc(&[SOURCE_REQUEST]);
        }

        if privacy.gdpr.enforced {
            self.privacy_tcf
                .inc(&[privacy.gdpr.tcf_version.as_str(), SOURCE_REQUEST]);
        }

        if privacy.lmt_enforced {
            self.privacy_lmt.inc(&[SOURCE_REQUEST]);
        }
    }

    fn record_adapter_buyer_uid_scrubbed(&self, adapter_name: &str) {
        if self.disabled(DisabledMetric::AdapterBuyerUidScrubbed) {
            return;
        }
        if let Some(scrubbed) = &self.adapter_scrubbed_buyer_uids {
            scrubbed.inc(&[&goconv::to_lower(adapter_name)]);
        }
    }

    fn record_adapter_gdpr_request_blocked(&self, adapter_name: &str) {
        if self.disabled(DisabledMetric::AdapterGdprRequestBlocked) {
            return;
        }
        if let Some(blocked) = &self.adapter_gdpr_blocked_requests {
            blocked.inc(&[&goconv::to_lower(adapter_name)]);
        }
    }

    fn record_debug_request(&self, debug_enabled: bool, pub_id: &str) {
        if debug_enabled {
            self.debug_requests.inc(&[]);
            if !self.disabled(DisabledMetric::AccountDebug) && pub_id != PUBLISHER_UNKNOWN {
                self.account_debug_requests.inc(&[pub_id]);
            }
        }
    }

    fn record_stored_response(&self, pub_id: &str) {
        self.stored_responses.inc(&[]);
        if !self.disabled(DisabledMetric::AccountStoredResponses) && pub_id != PUBLISHER_UNKNOWN {
            self.account_stored_responses.inc(&[pub_id]);
        }
    }

    fn record_gvl_list_request(&self) {
        self.gvl_list_requests.inc(&[]);
    }

    fn record_live_gvl_fetch(&self, success: bool) {
        let label = if success {
            REQUEST_SUCCESSFUL
        } else {
            REQUEST_FAILED
        };
        self.live_gvl_fetch.inc(&[label]);
    }

    fn record_ads_cert_req(&self, success: bool) {
        let label = if success {
            REQUEST_SUCCESSFUL
        } else {
            REQUEST_FAILED
        };
        self.ads_cert_requests.inc(&[label]);
    }

    fn record_ads_cert_sign_time(&self, ads_cert_sign_time: GoDuration) {
        self.ads_cert_sign_timer
            .observe(&[], ads_cert_sign_time.seconds());
    }

    // Go labels the bid validation series with the literal value `success="success"` (the
    // label name, `successLabel`), while preloading uses "true"/"false".

    fn record_bid_validation_creative_size_error(&self, adapter: &str, account: &str) {
        self.adapter_bid_response_validation_size_error
            .inc(&[&goconv::to_lower(adapter), SUCCESS_LABEL]);
        if !self.disabled(DisabledMetric::AccountAdapterDetails) && account != PUBLISHER_UNKNOWN {
            self.account_bid_response_validation_size_error
                .inc(&[account, SUCCESS_LABEL]);
        }
    }

    fn record_bid_validation_creative_size_warn(&self, adapter: &str, account: &str) {
        self.adapter_bid_response_validation_size_warn
            .inc(&[&goconv::to_lower(adapter), SUCCESS_LABEL]);
        if !self.disabled(DisabledMetric::AccountAdapterDetails) && account != PUBLISHER_UNKNOWN {
            self.account_bid_response_validation_size_warn
                .inc(&[account, SUCCESS_LABEL]);
        }
    }

    fn record_bid_validation_secure_markup_error(&self, adapter: &str, account: &str) {
        self.adapter_bid_response_secure_markup_error
            .inc(&[&goconv::to_lower(adapter), SUCCESS_LABEL]);
        if !self.disabled(DisabledMetric::AccountAdapterDetails) && account != PUBLISHER_UNKNOWN {
            self.account_bid_response_secure_markup_error
                .inc(&[account, SUCCESS_LABEL]);
        }
    }

    fn record_bid_validation_secure_markup_warn(&self, adapter: &str, account: &str) {
        self.adapter_bid_response_secure_markup_warn
            .inc(&[&goconv::to_lower(adapter), SUCCESS_LABEL]);
        if !self.disabled(DisabledMetric::AccountAdapterDetails) && account != PUBLISHER_UNKNOWN {
            self.account_bid_response_secure_markup_warn
                .inc(&[account, SUCCESS_LABEL]);
        }
    }

    // Go indexes the module maps directly; a module that was not registered is a nil family and
    // Go panics. No Go caller records an unregistered module; Rust records nothing.

    fn record_module_called(&self, labels: &ModuleLabels, duration: GoDuration) {
        if let Some(calls) = self.modules.calls.get(&labels.module) {
            calls.inc(&[&labels.stage]);
        }
        if let Some(timer) = self.modules.duration.get(&labels.module) {
            timer.observe(&[&labels.stage], duration.seconds());
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
        self.adapter_throttled
            .inc(&[&goconv::to_lower(adapter_name)]);
    }

    fn record_adapter_connection_dial_error(&self, adapter_name: &str) {
        if let Some(dial_errors) = &self.adapter_connection_dial_errors {
            dial_errors.inc(&[&goconv::to_lower(adapter_name)]);
        }
    }

    fn record_adapter_connection_dial_time(&self, adapter_name: &str, dial_start_time: GoDuration) {
        if let Some(dial_time) = &self.adapter_connection_dial_time {
            dial_time.observe(
                &[&goconv::to_lower(adapter_name)],
                dial_start_time.seconds(),
            );
        }
    }
}

fn inc_module(families: &BTreeMap<String, Arc<CounterVec>>, labels: &ModuleLabels) {
    if let Some(family) = families.get(&labels.module) {
        family.inc(&[&labels.stage]);
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

#[cfg(test)]
#[path = "golden_test.rs"]
mod golden_tests;
