//! Label types, label values and the `MetricsEngine` interface.
//!
//! Go: `metrics/metrics.go`.
//!
//! Every Go label type is a `string` newtype with package-level constants. They are ported as
//! `Copy` newtypes over `&'static str` with associated constants, so the Go zero value (the empty
//! string, which Go code produces whenever a label field is left unset) is
//! [`Default::default`] and is recorded exactly like Go records it.

use std::{collections::BTreeSet, fmt, time};

use crate::float_from_i64;

/// Declares a Go `string`-typed label with its constants and, when Go has one, the function that
/// lists its values.
macro_rules! label_type {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[$cmeta:meta])* $konst:ident = $value:literal, )+
        }
        $(#[$fmeta:meta])*
        fn $values_fn:ident;
    ) => {
        label_type! {
            $(#[$meta])*
            $name {
                $( $(#[$cmeta])* $konst = $value, )+
            }
        }

        $(#[$fmeta])*
        #[must_use]
        pub const fn $values_fn() -> &'static [$name] {
            &[$( $name::$konst, )+]
        }
    };
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[$cmeta:meta])* $konst:ident = $value:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(&'static str);

        impl $name {
            $( $(#[$cmeta])* pub const $konst: Self = Self($value); )+

            /// The label value Go records for this value.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.0)
            }
        }
    };
}

/// Labels attached to the request metrics.
///
/// Go: `metrics.Labels`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Labels {
    /// Go: `Source`.
    pub source: DemandSource,
    /// Go: `RType`.
    pub r_type: RequestType,
    /// Go: `PubID`, an exchange-specific ID. [`PUBLISHER_UNKNOWN`] when not known.
    pub pub_id: String,
    /// Go: `CookieFlag`.
    pub cookie_flag: CookieFlag,
    /// Go: `RequestStatus`.
    pub request_status: RequestStatus,
    /// Go: `RequestSize` (an `int` holding the body length in bytes).
    pub request_size: usize,
}

/// Labels attached to the adapter metrics.
///
/// Go: `metrics.AdapterLabels`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdapterLabels {
    /// Go: `Source`.
    pub source: DemandSource,
    /// Go: `RType`.
    pub r_type: RequestType,
    /// Go: `Adapter` (an `openrtb_ext.BidderName`).
    pub adapter: String,
    /// Go: `PubID`.
    pub pub_id: String,
    /// Go: `CookieFlag`.
    pub cookie_flag: CookieFlag,
    /// Go: `AdapterBids`.
    pub adapter_bids: AdapterBid,
    /// Go: `AdapterErrors map[AdapterError]struct{}`. A nil and an empty Go map both mean "no
    /// errors".
    pub adapter_errors: BTreeSet<AdapterError>,
}

label_type! {
    /// Overhead type enumeration.
    ///
    /// Go: `metrics.OverheadType`.
    OverheadType {
        /// Go: `PreBidder` — time to run the adapter's `MakeRequests()`, build the Prebid
        /// headers and apply `GZip` compression if needed.
        PRE_BIDDER = "pre-bidder",
        /// Go: `MakeAuctionResponse` — time spent in all the `MakeBids()` calls and in preparing
        /// the PBS response.
        MAKE_AUCTION_RESPONSE = "make-auction-response",
        /// Go: `MakeBidderRequests` — time to fetch stored requests, parse, unmarshal and
        /// validate the request, interpret its privacy policies and split it per bidder.
        MAKE_BIDDER_REQUESTS = "make-bidder-requests",
    }
    /// Go: `metrics.OverheadTypes()`.
    fn overhead_types;
}

/// Labels describing the impression types of a request.
///
/// Go: `metrics.ImpLabels`, four booleans (`BannerImps`, `VideoImps`, `AudioImps`,
/// `NativeImps`), held here as the set of [`ImpMediaType`]s that are `true`.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ImpLabels {
    bits: u8,
}

impl ImpLabels {
    /// No impression type (the Go zero value).
    pub const NONE: Self = Self { bits: 0 };

    fn bit(media_type: ImpMediaType) -> u8 {
        if media_type == ImpMediaType::BANNER {
            1
        } else if media_type == ImpMediaType::VIDEO {
            1 << 1
        } else if media_type == ImpMediaType::AUDIO {
            1 << 2
        } else if media_type == ImpMediaType::NATIVE {
            1 << 3
        } else {
            0
        }
    }

    /// A copy with the flag of `media_type` set to `present`.
    #[must_use]
    pub fn with(self, media_type: ImpMediaType, present: bool) -> Self {
        let bit = Self::bit(media_type);
        Self {
            bits: if present {
                self.bits | bit
            } else {
                self.bits & !bit
            },
        }
    }

    /// Sets the flag of `media_type` to `present`.
    pub fn set(&mut self, media_type: ImpMediaType, present: bool) {
        *self = self.with(media_type, present);
    }

    /// Whether the flag of `media_type` is set. Always `false` for the zero `ImpMediaType`.
    #[must_use]
    pub fn has(self, media_type: ImpMediaType) -> bool {
        let bit = Self::bit(media_type);
        bit != 0 && self.bits & bit != 0
    }

    /// Go: `BannerImps`.
    #[must_use]
    pub fn banner_imps(self) -> bool {
        self.has(ImpMediaType::BANNER)
    }

    /// Go: `VideoImps`.
    #[must_use]
    pub fn video_imps(self) -> bool {
        self.has(ImpMediaType::VIDEO)
    }

    /// Go: `AudioImps`.
    #[must_use]
    pub fn audio_imps(self) -> bool {
        self.has(ImpMediaType::AUDIO)
    }

    /// Go: `NativeImps`.
    #[must_use]
    pub fn native_imps(self) -> bool {
        self.has(ImpMediaType::NATIVE)
    }
}

impl fmt::Debug for ImpLabels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImpLabels")
            .field("banner_imps", &self.banner_imps())
            .field("video_imps", &self.video_imps())
            .field("audio_imps", &self.audio_imps())
            .field("native_imps", &self.native_imps())
            .finish()
    }
}

/// Labels describing the result of a network request.
///
/// Go: `metrics.RequestLabels`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RequestLabels {
    /// Go: `RequestStatus`.
    pub request_status: RequestStatus,
}

/// Labels describing the result of privacy enforcement.
///
/// Go: `metrics.PrivacyLabels`; the flat Go fields are grouped by privacy regime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PrivacyLabels {
    /// Go: `CCPAEnforced` and `CCPAProvided`.
    pub ccpa: CcpaLabels,
    /// Go: `COPPAEnforced`.
    pub coppa_enforced: bool,
    /// Go: `GDPREnforced` and `GDPRTCFVersion`.
    pub gdpr: GdprLabels,
    /// Go: `LMTEnforced`.
    pub lmt_enforced: bool,
}

/// The CCPA part of [`PrivacyLabels`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CcpaLabels {
    /// Go: `PrivacyLabels.CCPAEnforced`.
    pub enforced: bool,
    /// Go: `PrivacyLabels.CCPAProvided`.
    pub provided: bool,
}

/// The GDPR part of [`PrivacyLabels`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GdprLabels {
    /// Go: `PrivacyLabels.GDPREnforced`.
    pub enforced: bool,
    /// Go: `PrivacyLabels.GDPRTCFVersion`.
    pub tcf_version: TcfVersionValue,
}

/// Labels of the module (hook) metrics.
///
/// Go: `metrics.ModuleLabels`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModuleLabels {
    /// Go: `Module`, the module code with `.` and `-` replaced by `_` (the key of the
    /// `moduleStageNames` map given to the engines).
    pub module: String,
    /// Go: `Stage`, the hook stage name.
    pub stage: String,
    /// Go: `AccountID`.
    pub account_id: String,
}

label_type! {
    /// Stored data type.
    ///
    /// Go: `metrics.StoredDataType`.
    StoredDataType {
        /// Go: `AccountDataType`.
        ACCOUNT = "account",
        /// Go: `AMPDataType`.
        AMP = "amp",
        /// Go: `CategoryDataType`.
        CATEGORY = "category",
        /// Go: `RequestDataType`.
        REQUEST = "request",
        /// Go: `VideoDataType`.
        VIDEO = "video",
        /// Go: `ResponseDataType`.
        RESPONSE = "response",
    }
    /// Go: `metrics.StoredDataTypes()`.
    fn stored_data_types;
}

label_type! {
    /// Stored data fetch type.
    ///
    /// Go: `metrics.StoredDataFetchType`.
    StoredDataFetchType {
        /// Go: `FetchAll`.
        ALL = "all",
        /// Go: `FetchDelta`.
        DELTA = "delta",
    }
    /// Go: `metrics.StoredDataFetchTypes()`.
    fn stored_data_fetch_types;
}

/// Labels of the stored data metrics.
///
/// Go: `metrics.StoredDataLabels`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct StoredDataLabels {
    /// Go: `DataType`.
    pub data_type: StoredDataType,
    /// Go: `DataFetchType`.
    pub data_fetch_type: StoredDataFetchType,
    /// Go: `Error`.
    pub error: StoredDataError,
}

label_type! {
    /// Stored data error.
    ///
    /// Go: `metrics.StoredDataError`.
    StoredDataError {
        /// Go: `StoredDataErrorNetwork`.
        NETWORK = "network",
        /// Go: `StoredDataErrorUndefined`.
        UNDEFINED = "undefined",
    }
    /// Go: `metrics.StoredDataErrors()`.
    fn stored_data_errors;
}

label_type! {
    /// Demand source enumeration.
    ///
    /// Go: `metrics.DemandSource`.
    DemandSource {
        /// Go: `DemandWeb`.
        WEB = "web",
        /// Go: `DemandApp`.
        APP = "app",
        /// Go: `DemandDOOH`.
        DOOH = "dooh",
        /// Go: `DemandUnknown`.
        UNKNOWN = "unknown",
    }
    /// Go: `metrics.DemandTypes()`.
    fn demand_types;
}

label_type! {
    /// Media type of an `imp` object.
    ///
    /// Go: `metrics.ImpMediaType`.
    ImpMediaType {
        /// Go: `ImpTypeBanner`.
        BANNER = "banner",
        /// Go: `ImpTypeVideo`.
        VIDEO = "video",
        /// Go: `ImpTypeAudio`.
        AUDIO = "audio",
        /// Go: `ImpTypeNative`.
        NATIVE = "native",
    }
    /// Go: `metrics.ImpTypes()`.
    fn imp_types;
}

label_type! {
    /// Endpoint type.
    ///
    /// Go: `metrics.EndpointType`.
    EndpointType {
        /// Go: `EndpointAuction`.
        AUCTION = "auction",
        /// Go: `EndpointVideo`.
        VIDEO = "video",
        /// Go: `EndpointAmp`.
        AMP = "amp",
    }
    /// Go: `metrics.EndpointTypes()`.
    fn endpoint_types;
}

label_type! {
    /// Request type (endpoint) enumeration.
    ///
    /// Go: `metrics.RequestType`.
    RequestType {
        /// Go: `ReqTypeORTB2Web`.
        ORTB2_WEB = "openrtb2-web",
        /// Go: `ReqTypeORTB2App`.
        ORTB2_APP = "openrtb2-app",
        /// Go: `ReqTypeORTB2DOOH`.
        ORTB2_DOOH = "openrtb2-dooh",
        /// Go: `ReqTypeAMP`.
        AMP = "amp",
        /// Go: `ReqTypeVideo`.
        VIDEO = "video",
    }
    /// Go: `metrics.RequestTypes()`.
    fn request_types;
}

label_type! {
    /// Whether the user ID cookie exists.
    ///
    /// Go: `metrics.CookieFlag`.
    CookieFlag {
        /// Go: `CookieFlagYes`.
        YES = "exists",
        /// Go: `CookieFlagNo`.
        NO = "no",
        /// Go: `CookieFlagUnknown`.
        UNKNOWN = "unknown",
    }
    /// Go: `metrics.CookieTypes()`.
    fn cookie_types;
}

label_type! {
    /// The request return status.
    ///
    /// Go: `metrics.RequestStatus`.
    RequestStatus {
        /// Go: `RequestStatusOK`.
        OK = "ok",
        /// Go: `RequestStatusBadInput`.
        BAD_INPUT = "badinput",
        /// Go: `RequestStatusErr`.
        ERR = "err",
        /// Go: `RequestStatusNetworkErr`.
        NETWORK_ERR = "networkerr",
        /// Go: `RequestStatusBlockedApp`.
        BLOCKED_APP = "blockedapp",
        /// Go: `RequestStatusQueueTimeout`.
        QUEUE_TIMEOUT = "queuetimeout",
        /// Go: `RequestStatusAccountConfigErr`.
        ACCOUNT_CONFIG_ERR = "acctconfigerr",
    }
    /// Go: `metrics.RequestStatuses()`.
    fn request_statuses;
}

label_type! {
    /// Whether the adapter returned bids.
    ///
    /// Go: `metrics.AdapterBid`.
    AdapterBid {
        /// Go: `AdapterBidPresent`.
        PRESENT = "bid",
        /// Go: `AdapterBidNone`.
        NONE = "nobid",
    }
    /// Go: `metrics.AdapterBids()`.
    fn adapter_bids;
}

label_type! {
    /// An error that occurred during the adapter's execution.
    ///
    /// Go: `metrics.AdapterError`.
    AdapterError {
        /// Go: `AdapterErrorBadInput`.
        BAD_INPUT = "badinput",
        /// Go: `AdapterErrorBadServerResponse`.
        BAD_SERVER_RESPONSE = "badserverresponse",
        /// Go: `AdapterErrorTimeout`.
        TIMEOUT = "timeout",
        /// Go: `AdapterErrorFailedToRequestBids`.
        FAILED_TO_REQUEST_BIDS = "failedtorequestbid",
        /// Go: `AdapterErrorValidation`.
        VALIDATION = "validation",
        /// Go: `AdapterErrorTmaxTimeout`.
        TMAX_TIMEOUT = "tmaxtimeout",
        /// Go: `AdapterErrorUnknown`.
        UNKNOWN = "unknown_error",
    }
    /// Go: `metrics.AdapterErrors()`.
    fn adapter_errors;
}

label_type! {
    /// Cache hit or miss.
    ///
    /// Go: `metrics.CacheResult`.
    CacheResult {
        /// Go: `CacheHit` — the key was found in the cache.
        HIT = "hit",
        /// Go: `CacheMiss` — the key was not in the cache and was fetched from the backend.
        MISS = "miss",
    }
    /// Go: `metrics.CacheResults()`.
    fn cache_results;
}

label_type! {
    /// Outcome of a Fetchers 2.0 cache lookup.
    ///
    /// Go: `metrics.FetcherResult`.
    FetcherResult {
        /// Go: `FetcherResultHit` — served from the positive cache without a backend call.
        HIT = "hit",
        /// Go: `FetcherResultMiss` — the lookup had to go to the backend source.
        MISS = "miss",
        /// Go: `FetcherResultNegative` — a not-found served from the negative cache.
        NEGATIVE = "negative",
    }
    /// Go: `metrics.FetcherResults()`.
    fn fetcher_results;
}

label_type! {
    /// Outcome of a Fetchers 2.0 upstream (source) fetch.
    ///
    /// Go: `metrics.FetcherBackendResult`.
    FetcherBackendResult {
        /// Go: `FetcherBackendOK` — a successful fetch that returned a value.
        OK = "ok",
        /// Go: `FetcherBackendNotFound` — a definitive per-key not-found.
        NOT_FOUND = "notfound",
        /// Go: `FetcherBackendError` — a systemic backend failure (never cached).
        ERROR = "error",
    }
    /// Go: `metrics.FetcherBackendResults()`.
    fn fetcher_backend_results;
}

label_type! {
    /// The Fetchers 2.0 operation that caused an upstream source call. Go has no function that
    /// lists these values.
    ///
    /// Go: `metrics.FetcherOperation`.
    FetcherOperation {
        /// Go: `FetcherOperationGet` — an on-demand read for one key.
        GET = "get",
        /// Go: `FetcherOperationStart` — the startup preload.
        START = "start",
        /// Go: `FetcherOperationBackgroundRefresh` — an async refresh of a stale key.
        BACKGROUND_REFRESH = "background_refresh",
    }
}

label_type! {
    /// The possible values for TCF versions.
    ///
    /// Go: `metrics.TCFVersionValue`.
    TcfVersionValue {
        /// Go: `TCFVersionErr`.
        ERR = "err",
        /// Go: `TCFVersionV2`.
        V2 = "v2",
    }
    /// Go: `metrics.TCFVersions()`.
    fn tcf_versions;
}

label_type! {
    /// Status of a call to the `/cookie_sync` endpoint.
    ///
    /// Go: `metrics.CookieSyncStatus`.
    CookieSyncStatus {
        /// Go: `CookieSyncOK`.
        OK = "ok",
        /// Go: `CookieSyncBadRequest`.
        BAD_REQUEST = "bad_request",
        /// Go: `CookieSyncOptOut`.
        OPT_OUT = "opt_out",
        /// Go: `CookieSyncGDPRHostCookieBlocked`.
        GDPR_HOST_COOKIE_BLOCKED = "gdpr_blocked_host_cookie",
        /// Go: `CookieSyncAccountBlocked`.
        ACCOUNT_BLOCKED = "acct_blocked",
        /// Go: `CookieSyncAccountConfigMalformed`.
        ACCOUNT_CONFIG_MALFORMED = "acct_config_malformed",
        /// Go: `CookieSyncAccountInvalid`.
        ACCOUNT_INVALID = "acct_invalid",
    }
    /// Go: `metrics.CookieSyncStatuses()`.
    fn cookie_sync_statuses;
}

label_type! {
    /// Status of one syncer during a call to the `/cookie_sync` endpoint.
    ///
    /// Go: `metrics.SyncerCookieSyncStatus`.
    SyncerCookieSyncStatus {
        /// Go: `SyncerCookieSyncOK`.
        OK = "ok",
        /// Go: `SyncerCookieSyncPrivacyBlocked`.
        PRIVACY_BLOCKED = "privacy_blocked",
        /// Go: `SyncerCookieSyncAlreadySynced`.
        ALREADY_SYNCED = "already_synced",
        /// Go: `SyncerCookieSyncRejectedByFilter`.
        REJECTED_BY_FILTER = "rejected_by_filter",
    }
    /// Go: `metrics.SyncerRequestStatuses()`.
    fn syncer_request_statuses;
}

label_type! {
    /// Status of a call to the `/setuid` endpoint.
    ///
    /// Go: `metrics.SetUidStatus`.
    SetUidStatus {
        /// Go: `SetUidOK`.
        OK = "ok",
        /// Go: `SetUidBadRequest`.
        BAD_REQUEST = "bad_request",
        /// Go: `SetUidOptOut`.
        OPT_OUT = "opt_out",
        /// Go: `SetUidGDPRHostCookieBlocked`.
        GDPR_HOST_COOKIE_BLOCKED = "gdpr_blocked_host_cookie",
        /// Go: `SetUidAccountBlocked`.
        ACCOUNT_BLOCKED = "acct_blocked",
        /// Go: `SetUidAccountConfigMalformed`.
        ACCOUNT_CONFIG_MALFORMED = "acct_config_malformed",
        /// Go: `SetUidAccountInvalid`.
        ACCOUNT_INVALID = "acct_invalid",
        /// Go: `SetUidSyncerUnknown`.
        SYNCER_UNKNOWN = "syncer_unknown",
    }
    /// Go: `metrics.SetUidStatuses()`.
    fn set_uid_statuses;
}

label_type! {
    /// Status of one syncer during a call to the `/setuid` endpoint.
    ///
    /// Go: `metrics.SyncerSetUidStatus`.
    SyncerSetUidStatus {
        /// Go: `SyncerSetUidOK`.
        OK = "ok",
        /// Go: `SyncerSetUidCleared`.
        CLEARED = "cleared",
    }
    /// Go: `metrics.SyncerSetUidStatuses()`.
    fn syncer_set_uid_statuses;
}

/// Default value for [`Labels::pub_id`] when the publisher is not known.
///
/// Go: `metrics.PublisherUnknown`.
pub const PUBLISHER_UNKNOWN: &str = "unknown";

/// The endpoint a request type belongs to. Go returns the zero `EndpointType` (the empty string)
/// for any other value, including the zero `RequestType`.
///
/// Go: `metrics.GetEndpointFromRequestType`.
#[must_use]
pub fn get_endpoint_from_request_type(request_type: RequestType) -> EndpointType {
    if request_type == RequestType::ORTB2_WEB
        || request_type == RequestType::ORTB2_APP
        || request_type == RequestType::ORTB2_DOOH
    {
        EndpointType::AUCTION
    } else if request_type == RequestType::AMP {
        EndpointType::AMP
    } else if request_type == RequestType::VIDEO {
        EndpointType::VIDEO
    } else {
        EndpointType::default()
    }
}

/// The TCF version label for an integer TCF version.
///
/// Go: `metrics.TCFVersionToValue`.
#[must_use]
pub const fn tcf_version_to_value(version: i64) -> TcfVersionValue {
    if version == 2 {
        TcfVersionValue::V2
    } else {
        TcfVersionValue::ERR
    }
}

/// A Go `time.Duration`: a signed count of nanoseconds.
///
/// The engines take Go durations rather than [`std::time::Duration`] because Go call sites can
/// produce negative durations (for example the request queue time parsed from a request header
/// in `router/aspects/request_timeout_handler.go`), and Prometheus records them as negative
/// seconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GoDuration(i64);

impl GoDuration {
    /// Go: `time.Hour`.
    pub const HOUR: Self = Self(3_600_000_000_000);
    /// Go: `time.Microsecond`.
    pub const MICROSECOND: Self = Self(1_000);
    /// Go: `time.Millisecond`.
    pub const MILLISECOND: Self = Self(1_000_000);
    /// Go: `time.Minute`.
    pub const MINUTE: Self = Self(60_000_000_000);
    /// Go: `time.Nanosecond`.
    pub const NANOSECOND: Self = Self(1);
    /// Go: `time.Second`.
    pub const SECOND: Self = Self(1_000_000_000);

    /// A duration of `nanos` nanoseconds (Go: `time.Duration(nanos)`).
    #[must_use]
    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    /// `n` units of `unit`, wrapping on overflow like Go's `time.Duration(n) * unit`.
    #[must_use]
    pub const fn times(n: i64, unit: Self) -> Self {
        Self(n.wrapping_mul(unit.0))
    }

    /// The duration as a count of nanoseconds.
    #[must_use]
    pub const fn as_nanos(self) -> i64 {
        self.0
    }

    /// The duration in seconds, computed exactly like Go's `Duration.Seconds()`:
    /// `float64(d / Second) + float64(d % Second) / 1e9`.
    #[must_use]
    pub fn seconds(self) -> f64 {
        let sec = self.0 / Self::SECOND.0;
        let nsec = self.0 % Self::SECOND.0;
        float_from_i64(sec) + float_from_i64(nsec) / 1e9
    }
}

impl From<time::Duration> for GoDuration {
    /// Converts a non-negative std duration, saturating at the largest Go duration the way
    /// Go's `time.Since` and `Time.Sub` saturate.
    fn from(d: time::Duration) -> Self {
        Self(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
    }
}

/// A generic interface to record PBS metrics into the desired backend.
///
/// The first three request metrics fire once per incoming request, so their totals equal the
/// number of incoming requests. The adapter metrics fire per outgoing request to a bidder
/// adapter, so they record several hits per incoming request. The two groups are consistent
/// within themselves, but comparing numbers between groups is generally not useful.
///
/// Go: `metrics.MetricsEngine`. The methods are in Go's order; bidder names
/// (`openrtb_ext.BidderName`) and bid types (`openrtb_ext.BidType`) are passed as strings.
pub trait MetricsEngine: fmt::Debug + Send + Sync {
    /// Go: `RecordConnectionAccept`.
    fn record_connection_accept(&self, success: bool);
    /// Go: `RecordTMaxTimeout`.
    fn record_tmax_timeout(&self);
    /// Go: `RecordConnectionClose`.
    fn record_connection_close(&self, success: bool);
    /// Go: `RecordRequest`.
    fn record_request(&self, labels: &Labels);
    /// Go: `RecordImps`.
    fn record_imps(&self, labels: ImpLabels);
    /// Go: `RecordRequestTime`.
    fn record_request_time(&self, labels: &Labels, length: GoDuration);
    /// Go: `RecordOverheadTime`.
    fn record_overhead_time(&self, overhead: OverheadType, length: GoDuration);
    /// Go: `RecordAdapterRequest`.
    fn record_adapter_request(&self, labels: &AdapterLabels);
    /// Go: `RecordAdapterConnections`.
    fn record_adapter_connections(
        &self,
        adapter_name: &str,
        conn_was_reused: bool,
        conn_wait_time: GoDuration,
    );
    /// Go: `RecordDNSTime`.
    fn record_dns_time(&self, dns_lookup_time: GoDuration);
    /// Go: `RecordTLSHandshakeTime`.
    fn record_tls_handshake_time(&self, tls_handshake_time: GoDuration);
    /// Go: `RecordBidderServerResponseTime`.
    fn record_bidder_server_response_time(&self, bidder_server_response_time: GoDuration);
    /// Go: `RecordAdapterPanic`.
    fn record_adapter_panic(&self, labels: &AdapterLabels);
    /// Go: `RecordAdapterBidReceived`.
    fn record_adapter_bid_received(&self, labels: &AdapterLabels, bid_type: &str, has_adm: bool);
    /// Go: `RecordAdapterPrice`.
    fn record_adapter_price(&self, labels: &AdapterLabels, cpm: f64);
    /// Go: `RecordAdapterTime`.
    fn record_adapter_time(&self, labels: &AdapterLabels, length: GoDuration);
    /// Go: `RecordCookieSync`.
    fn record_cookie_sync(&self, status: CookieSyncStatus);
    /// Go: `RecordSyncerRequest`.
    fn record_syncer_request(&self, key: &str, status: SyncerCookieSyncStatus);
    /// Go: `RecordSetUid`.
    fn record_set_uid(&self, status: SetUidStatus);
    /// Go: `RecordSyncerSet`.
    fn record_syncer_set(&self, key: &str, status: SyncerSetUidStatus);
    /// Go: `RecordStoredReqCacheResult`. Go takes an `int`; every Go caller passes a length.
    fn record_stored_req_cache_result(&self, cache_result: CacheResult, inc: usize);
    /// Go: `RecordStoredImpCacheResult`.
    fn record_stored_imp_cache_result(&self, cache_result: CacheResult, inc: usize);
    /// Go: `RecordAccountCacheResult`.
    fn record_account_cache_result(&self, cache_result: CacheResult, inc: usize);
    /// Go: `RecordStoredDataFetchTime`.
    fn record_stored_data_fetch_time(&self, labels: StoredDataLabels, length: GoDuration);
    /// Go: `RecordStoredDataError`.
    fn record_stored_data_error(&self, labels: StoredDataLabels);
    /// Go: `RecordFetcherResult`.
    fn record_fetcher_result(&self, subsystem: &str, result: FetcherResult);
    /// Go: `RecordFetcherBackendFetch`.
    fn record_fetcher_backend_fetch(
        &self,
        subsystem: &str,
        operation: FetcherOperation,
        result: FetcherBackendResult,
        length: GoDuration,
    );
    /// Go: `RecordPrebidCacheRequestTime`.
    fn record_prebid_cache_request_time(&self, success: bool, length: GoDuration);
    /// Go: `RecordRequestQueueTime`.
    fn record_request_queue_time(
        &self,
        success: bool,
        request_type: RequestType,
        length: GoDuration,
    );
    /// Go: `RecordTimeoutNotice`.
    fn record_timeout_notice(&self, success: bool);
    /// Go: `RecordRequestPrivacy`.
    fn record_request_privacy(&self, privacy: PrivacyLabels);
    /// Go: `RecordAdapterBuyerUIDScrubbed`.
    fn record_adapter_buyer_uid_scrubbed(&self, adapter_name: &str);
    /// Go: `RecordAdapterGDPRRequestBlocked`.
    fn record_adapter_gdpr_request_blocked(&self, adapter_name: &str);
    /// Go: `RecordDebugRequest`.
    fn record_debug_request(&self, debug_enabled: bool, pub_id: &str);
    /// Go: `RecordStoredResponse`.
    fn record_stored_response(&self, pub_id: &str);
    /// Go: `RecordGvlListRequest`.
    fn record_gvl_list_request(&self);
    /// Go: `RecordLiveGVLFetch`.
    fn record_live_gvl_fetch(&self, success: bool);
    /// Go: `RecordAdsCertReq`.
    fn record_ads_cert_req(&self, success: bool);
    /// Go: `RecordAdsCertSignTime`.
    fn record_ads_cert_sign_time(&self, ads_cert_sign_time: GoDuration);
    /// Go: `RecordBidValidationCreativeSizeError`.
    fn record_bid_validation_creative_size_error(&self, adapter: &str, account: &str);
    /// Go: `RecordBidValidationCreativeSizeWarn`.
    fn record_bid_validation_creative_size_warn(&self, adapter: &str, account: &str);
    /// Go: `RecordBidValidationSecureMarkupError`.
    fn record_bid_validation_secure_markup_error(&self, adapter: &str, account: &str);
    /// Go: `RecordBidValidationSecureMarkupWarn`.
    fn record_bid_validation_secure_markup_warn(&self, adapter: &str, account: &str);
    /// Go: `RecordModuleCalled`.
    fn record_module_called(&self, labels: &ModuleLabels, duration: GoDuration);
    /// Go: `RecordModuleFailed`.
    fn record_module_failed(&self, labels: &ModuleLabels);
    /// Go: `RecordModuleSuccessNooped`.
    fn record_module_success_nooped(&self, labels: &ModuleLabels);
    /// Go: `RecordModuleSuccessUpdated`.
    fn record_module_success_updated(&self, labels: &ModuleLabels);
    /// Go: `RecordModuleSuccessRejected`.
    fn record_module_success_rejected(&self, labels: &ModuleLabels);
    /// Go: `RecordModuleExecutionError`.
    fn record_module_execution_error(&self, labels: &ModuleLabels);
    /// Go: `RecordModuleTimeout`.
    fn record_module_timeout(&self, labels: &ModuleLabels);
    /// Go: `RecordAdapterThrottled`.
    fn record_adapter_throttled(&self, adapter_name: &str);
    /// Go: `RecordAdapterConnectionDialError`.
    fn record_adapter_connection_dial_error(&self, adapter_name: &str);
    /// Go: `RecordAdapterConnectionDialTime`.
    fn record_adapter_connection_dial_time(&self, adapter_name: &str, dial_start_time: GoDuration);
}

#[cfg(test)]
#[path = "metrics_test.rs"]
mod tests;
