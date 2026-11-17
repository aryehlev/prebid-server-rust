//! A recording mock of [`MetricsEngine`] for tests in other crates.
//!
//! Go: `metrics/metrics_mock.go` (`MetricsEngineMock`, a `testify/mock.Mock`). Expectations follow
//! testify: an expectation set with [`MetricsEngineMock::on`] matches calls until its
//! repeatability is used up ([`Expectation::once`], [`Expectation::times`]; unlimited by
//! default), and [`MetricsEngineMock::assert_expectations`] fails for every non-optional
//! expectation that was not met. Where testify panics on an unexpected call, the mock records the
//! failure and reports it from `assert_expectations`.

use std::{
    fmt,
    sync::{Mutex, MutexGuard, PoisonError},
};

use crate::metrics::{
    AdapterLabels, CacheResult, CookieSyncStatus, FetcherBackendResult, FetcherOperation,
    FetcherResult, GoDuration, ImpLabels, Labels, MetricsEngine, ModuleLabels, OverheadType,
    PrivacyLabels, RequestType, SetUidStatus, StoredDataLabels, SyncerCookieSyncStatus,
    SyncerSetUidStatus,
};

/// One call of a [`MetricsEngine`] method with its arguments, named after the Go method without
/// its `Record` prefix.
#[derive(Clone, Debug, PartialEq)]
pub enum MetricsCall {
    /// Go: `RecordConnectionAccept(success)`.
    ConnectionAccept(bool),
    /// Go: `RecordTMaxTimeout()`.
    TMaxTimeout,
    /// Go: `RecordConnectionClose(success)`.
    ConnectionClose(bool),
    /// Go: `RecordRequest(labels)`.
    Request(Labels),
    /// Go: `RecordImps(labels)`.
    Imps(ImpLabels),
    /// Go: `RecordRequestTime(labels, length)`.
    RequestTime(Labels, GoDuration),
    /// Go: `RecordOverheadTime(overhead, length)`.
    OverheadTime(OverheadType, GoDuration),
    /// Go: `RecordAdapterRequest(labels)`.
    AdapterRequest(AdapterLabels),
    /// Go: `RecordAdapterConnections(bidderName, connWasReused, connWaitTime)`.
    AdapterConnections(String, bool, GoDuration),
    /// Go: `RecordDNSTime(dnsLookupTime)`.
    DnsTime(GoDuration),
    /// Go: `RecordTLSHandshakeTime(tlsHandshakeTime)`.
    TlsHandshakeTime(GoDuration),
    /// Go: `RecordBidderServerResponseTime(bidderServerResponseTime)`.
    BidderServerResponseTime(GoDuration),
    /// Go: `RecordAdapterPanic(labels)`.
    AdapterPanic(AdapterLabels),
    /// Go: `RecordAdapterBidReceived(labels, bidType, hasAdm)`.
    AdapterBidReceived(AdapterLabels, String, bool),
    /// Go: `RecordAdapterPrice(labels, cpm)`.
    AdapterPrice(AdapterLabels, f64),
    /// Go: `RecordAdapterTime(labels, length)`.
    AdapterTime(AdapterLabels, GoDuration),
    /// Go: `RecordCookieSync(status)`.
    CookieSync(CookieSyncStatus),
    /// Go: `RecordSyncerRequest(key, status)`.
    SyncerRequest(String, SyncerCookieSyncStatus),
    /// Go: `RecordSetUid(status)`.
    SetUid(SetUidStatus),
    /// Go: `RecordSyncerSet(key, status)`.
    SyncerSet(String, SyncerSetUidStatus),
    /// Go: `RecordStoredReqCacheResult(cacheResult, inc)`.
    StoredReqCacheResult(CacheResult, usize),
    /// Go: `RecordStoredImpCacheResult(cacheResult, inc)`.
    StoredImpCacheResult(CacheResult, usize),
    /// Go: `RecordAccountCacheResult(cacheResult, inc)`.
    AccountCacheResult(CacheResult, usize),
    /// Go: `RecordStoredDataFetchTime(labels, length)`.
    StoredDataFetchTime(StoredDataLabels, GoDuration),
    /// Go: `RecordStoredDataError(labels)`.
    StoredDataError(StoredDataLabels),
    /// Go: `RecordFetcherResult(subsystem, result)`.
    FetcherResult(String, FetcherResult),
    /// Go: `RecordFetcherBackendFetch(subsystem, operation, result, length)`.
    FetcherBackendFetch(String, FetcherOperation, FetcherBackendResult, GoDuration),
    /// Go: `RecordPrebidCacheRequestTime(success, length)`.
    PrebidCacheRequestTime(bool, GoDuration),
    /// Go: `RecordRequestQueueTime(success, requestType, length)`.
    RequestQueueTime(bool, RequestType, GoDuration),
    /// Go: `RecordTimeoutNotice(success)`.
    TimeoutNotice(bool),
    /// Go: `RecordRequestPrivacy(privacy)`.
    RequestPrivacy(PrivacyLabels),
    /// Go: `RecordAdapterBuyerUIDScrubbed(adapterName)`.
    AdapterBuyerUidScrubbed(String),
    /// Go: `RecordAdapterGDPRRequestBlocked(adapterName)`.
    AdapterGdprRequestBlocked(String),
    /// Go: `RecordDebugRequest(debugEnabled, pubId)`.
    DebugRequest(bool, String),
    /// Go: `RecordStoredResponse(pubId)`.
    StoredResponse(String),
    /// Go: `RecordGvlListRequest()`.
    GvlListRequest,
    /// Go: `RecordLiveGVLFetch(success)`.
    LiveGvlFetch(bool),
    /// Go: `RecordAdsCertReq(success)`.
    AdsCertReq(bool),
    /// Go: `RecordAdsCertSignTime(adsCertSignTime)`.
    AdsCertSignTime(GoDuration),
    /// Go: `RecordBidValidationCreativeSizeError(adapter, account)`.
    BidValidationCreativeSizeError(String, String),
    /// Go: `RecordBidValidationCreativeSizeWarn(adapter, account)`.
    BidValidationCreativeSizeWarn(String, String),
    /// Go: `RecordBidValidationSecureMarkupError(adapter, account)`.
    BidValidationSecureMarkupError(String, String),
    /// Go: `RecordBidValidationSecureMarkupWarn(adapter, account)`.
    BidValidationSecureMarkupWarn(String, String),
    /// Go: `RecordModuleCalled(labels, duration)`.
    ModuleCalled(ModuleLabels, GoDuration),
    /// Go: `RecordModuleFailed(labels)`.
    ModuleFailed(ModuleLabels),
    /// Go: `RecordModuleSuccessNooped(labels)`.
    ModuleSuccessNooped(ModuleLabels),
    /// Go: `RecordModuleSuccessUpdated(labels)`.
    ModuleSuccessUpdated(ModuleLabels),
    /// Go: `RecordModuleSuccessRejected(labels)`.
    ModuleSuccessRejected(ModuleLabels),
    /// Go: `RecordModuleExecutionError(labels)`.
    ModuleExecutionError(ModuleLabels),
    /// Go: `RecordModuleTimeout(labels)`.
    ModuleTimeout(ModuleLabels),
    /// Go: `RecordAdapterThrottled(adapterName)`.
    AdapterThrottled(String),
    /// Go: `RecordAdapterConnectionDialError(adapterName)`. The Go mock records this call
    /// without its argument (`me.Called()`), so expectations cannot match on the adapter.
    AdapterConnectionDialError,
    /// Go: `RecordAdapterConnectionDialTime(adapterName, dialStartTime)`.
    AdapterConnectionDialTime(String, GoDuration),
}

impl MetricsCall {
    /// The Go method name of the call, as testify's `On` names it.
    #[must_use]
    pub const fn method_name(&self) -> &'static str {
        match self {
            Self::ConnectionAccept(..) => "RecordConnectionAccept",
            Self::TMaxTimeout => "RecordTMaxTimeout",
            Self::ConnectionClose(..) => "RecordConnectionClose",
            Self::Request(..) => "RecordRequest",
            Self::Imps(..) => "RecordImps",
            Self::RequestTime(..) => "RecordRequestTime",
            Self::OverheadTime(..) => "RecordOverheadTime",
            Self::AdapterRequest(..) => "RecordAdapterRequest",
            Self::AdapterConnections(..) => "RecordAdapterConnections",
            Self::DnsTime(..) => "RecordDNSTime",
            Self::TlsHandshakeTime(..) => "RecordTLSHandshakeTime",
            Self::BidderServerResponseTime(..) => "RecordBidderServerResponseTime",
            Self::AdapterPanic(..) => "RecordAdapterPanic",
            Self::AdapterBidReceived(..) => "RecordAdapterBidReceived",
            Self::AdapterPrice(..) => "RecordAdapterPrice",
            Self::AdapterTime(..) => "RecordAdapterTime",
            Self::CookieSync(..) => "RecordCookieSync",
            Self::SyncerRequest(..) => "RecordSyncerRequest",
            Self::SetUid(..) => "RecordSetUid",
            Self::SyncerSet(..) => "RecordSyncerSet",
            Self::StoredReqCacheResult(..) => "RecordStoredReqCacheResult",
            Self::StoredImpCacheResult(..) => "RecordStoredImpCacheResult",
            Self::AccountCacheResult(..) => "RecordAccountCacheResult",
            Self::StoredDataFetchTime(..) => "RecordStoredDataFetchTime",
            Self::StoredDataError(..) => "RecordStoredDataError",
            Self::FetcherResult(..) => "RecordFetcherResult",
            Self::FetcherBackendFetch(..) => "RecordFetcherBackendFetch",
            Self::PrebidCacheRequestTime(..) => "RecordPrebidCacheRequestTime",
            Self::RequestQueueTime(..) => "RecordRequestQueueTime",
            Self::TimeoutNotice(..) => "RecordTimeoutNotice",
            Self::RequestPrivacy(..) => "RecordRequestPrivacy",
            Self::AdapterBuyerUidScrubbed(..) => "RecordAdapterBuyerUIDScrubbed",
            Self::AdapterGdprRequestBlocked(..) => "RecordAdapterGDPRRequestBlocked",
            Self::DebugRequest(..) => "RecordDebugRequest",
            Self::StoredResponse(..) => "RecordStoredResponse",
            Self::GvlListRequest => "RecordGvlListRequest",
            Self::LiveGvlFetch(..) => "RecordLiveGVLFetch",
            Self::AdsCertReq(..) => "RecordAdsCertReq",
            Self::AdsCertSignTime(..) => "RecordAdsCertSignTime",
            Self::BidValidationCreativeSizeError(..) => "RecordBidValidationCreativeSizeError",
            Self::BidValidationCreativeSizeWarn(..) => "RecordBidValidationCreativeSizeWarn",
            Self::BidValidationSecureMarkupError(..) => "RecordBidValidationSecureMarkupError",
            Self::BidValidationSecureMarkupWarn(..) => "RecordBidValidationSecureMarkupWarn",
            Self::ModuleCalled(..) => "RecordModuleCalled",
            Self::ModuleFailed(..) => "RecordModuleFailed",
            Self::ModuleSuccessNooped(..) => "RecordModuleSuccessNooped",
            Self::ModuleSuccessUpdated(..) => "RecordModuleSuccessUpdated",
            Self::ModuleSuccessRejected(..) => "RecordModuleSuccessRejected",
            Self::ModuleExecutionError(..) => "RecordModuleExecutionError",
            Self::ModuleTimeout(..) => "RecordModuleTimeout",
            Self::AdapterThrottled(..) => "RecordAdapterThrottled",
            Self::AdapterConnectionDialError => "RecordAdapterConnectionDialError",
            Self::AdapterConnectionDialTime(..) => "RecordAdapterConnectionDialTime",
        }
    }
}

/// How an expectation selects calls.
enum Matcher {
    /// Go: `On(method, args...)` with concrete arguments.
    Exact(MetricsCall),
    /// Go: `On(method, mock.Anything, ...)`.
    AnyArgs(&'static str),
    /// Go: `On(method, mock.MatchedBy(fn), ...)`.
    MatchedBy(
        &'static str,
        Box<dyn Fn(&MetricsCall) -> bool + Send + Sync>,
    ),
}

impl Matcher {
    fn matches(&self, call: &MetricsCall) -> bool {
        match self {
            Self::Exact(expected) => expected == call,
            Self::AnyArgs(method) => *method == call.method_name(),
            Self::MatchedBy(method, f) => *method == call.method_name() && f(call),
        }
    }
}

impl fmt::Debug for Matcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact(call) => write!(f, "{call:?}"),
            Self::AnyArgs(method) => write!(f, "{method}(mock.Anything...)"),
            Self::MatchedBy(method, _) => write!(f, "{method}(mock.MatchedBy(...))"),
        }
    }
}

/// An expected call.
///
/// Go: `*mock.Call`.
#[derive(Debug)]
pub struct Expectation {
    matcher: Matcher,
    /// Go: `Repeatability` — `None` is unlimited (Go: 0), `Some(n)` is the number of calls left
    /// (Go uses -1 once a limited expectation is used up).
    remaining: Option<usize>,
    /// Go: `optional`, set by `Maybe()`.
    optional: bool,
    /// Go: `totalCalls`.
    total_calls: usize,
}

impl Expectation {
    const fn new(matcher: Matcher) -> Self {
        Self {
            matcher,
            remaining: None,
            optional: false,
            total_calls: 0,
        }
    }

    /// An expectation for exactly `call` (Go: `On(method, args...)`).
    #[must_use]
    pub const fn call(call: MetricsCall) -> Self {
        Self::new(Matcher::Exact(call))
    }

    /// An expectation for any call of the Go method `method` (Go: `On(method, mock.Anything...)`).
    #[must_use]
    pub const fn any_args(method: &'static str) -> Self {
        Self::new(Matcher::AnyArgs(method))
    }

    /// An expectation for calls of the Go method `method` that satisfy `matches` (Go:
    /// `On(method, mock.MatchedBy(fn))`).
    #[must_use]
    pub fn matched_by<F>(method: &'static str, matches: F) -> Self
    where
        F: Fn(&MetricsCall) -> bool + Send + Sync + 'static,
    {
        Self::new(Matcher::MatchedBy(method, Box::new(matches)))
    }

    /// Go: `Once()`.
    #[must_use]
    pub const fn once(self) -> Self {
        self.times(1)
    }

    /// Go: `Times(n)`.
    #[must_use]
    pub const fn times(mut self, n: usize) -> Self {
        self.remaining = Some(n);
        self
    }

    /// Go: `Maybe()` — the expectation may go unmet.
    #[must_use]
    pub const fn maybe(mut self) -> Self {
        self.optional = true;
        self
    }

    const fn is_available(&self) -> bool {
        !matches!(self.remaining, Some(0))
    }
}

#[derive(Debug, Default)]
struct MockState {
    expectations: Vec<Expectation>,
    calls: Vec<MetricsCall>,
    failures: Vec<String>,
}

/// A mock for the [`MetricsEngine`] interface.
///
/// Go: `metrics.MetricsEngineMock`.
#[derive(Debug, Default)]
pub struct MetricsEngineMock {
    state: Mutex<MockState>,
}

impl MetricsEngineMock {
    /// A mock without expectations.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, MockState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds an expectation (Go: `On`).
    pub fn on(&self, expectation: Expectation) {
        self.state().expectations.push(expectation);
    }

    /// Go: `Called` — records the call and consumes the first available matching expectation.
    fn called(&self, call: MetricsCall) {
        let mut state = self.state();
        let matched = state
            .expectations
            .iter_mut()
            .find(|e| e.is_available() && e.matcher.matches(&call));
        if let Some(expectation) = matched {
            if let Some(remaining) = expectation.remaining.as_mut() {
                *remaining = remaining.saturating_sub(1);
            }
            expectation.total_calls = expectation.total_calls.saturating_add(1);
        } else {
            let exhausted = state.expectations.iter().any(|e| e.matcher.matches(&call));
            let failure = if exhausted {
                format!(
                    "mock: The method has been called over the expected number of times: {call:?}"
                )
            } else {
                format!(
                    "mock: I don't know what to return because the method call was unexpected: \
                     {call:?}"
                )
            };
            state.failures.push(failure);
        }
        state.calls.push(call);
    }

    /// Every call made so far, in order (Go: `Calls`).
    #[must_use]
    pub fn calls(&self) -> Vec<MetricsCall> {
        self.state().calls.clone()
    }

    /// The number of calls of the Go method `method` (Go: `AssertNumberOfCalls`' count).
    #[must_use]
    pub fn number_of_calls(&self, method: &str) -> usize {
        self.state()
            .calls
            .iter()
            .filter(|c| c.method_name() == method)
            .count()
    }

    /// Whether `call` was made (Go: `AssertCalled`).
    #[must_use]
    pub fn was_called(&self, call: &MetricsCall) -> bool {
        self.state().calls.contains(call)
    }

    /// Go: `AssertExpectations` — every unexpected call, every non-optional expectation that
    /// matched no call, and every limited expectation with calls left.
    ///
    /// # Errors
    ///
    /// The failure messages, one per line.
    pub fn assert_expectations(&self) -> Result<(), String> {
        let failures = Self::failures(&self.state());
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }

    fn failures(state: &MockState) -> Vec<String> {
        let mut failures = state.failures.clone();
        for expectation in &state.expectations {
            let called = state.calls.iter().any(|c| expectation.matcher.matches(c));
            if !expectation.optional && !called && expectation.total_calls == 0 {
                failures.push(format!("FAIL:\t{:?}", expectation.matcher));
            } else if let Some(left) = expectation.remaining.filter(|n| *n > 0) {
                failures.push(format!(
                    "FAIL:\t{:?}: expected {left} more call(s)",
                    expectation.matcher
                ));
            }
        }
        failures
    }
}

impl MetricsEngine for MetricsEngineMock {
    fn record_connection_accept(&self, success: bool) {
        self.called(MetricsCall::ConnectionAccept(success));
    }

    fn record_tmax_timeout(&self) {
        self.called(MetricsCall::TMaxTimeout);
    }

    fn record_connection_close(&self, success: bool) {
        self.called(MetricsCall::ConnectionClose(success));
    }

    fn record_request(&self, labels: &Labels) {
        self.called(MetricsCall::Request(labels.clone()));
    }

    fn record_imps(&self, labels: ImpLabels) {
        self.called(MetricsCall::Imps(labels));
    }

    fn record_request_time(&self, labels: &Labels, length: GoDuration) {
        self.called(MetricsCall::RequestTime(labels.clone(), length));
    }

    fn record_overhead_time(&self, overhead: OverheadType, length: GoDuration) {
        self.called(MetricsCall::OverheadTime(overhead, length));
    }

    fn record_adapter_request(&self, labels: &AdapterLabels) {
        self.called(MetricsCall::AdapterRequest(labels.clone()));
    }

    fn record_adapter_connections(
        &self,
        adapter_name: &str,
        conn_was_reused: bool,
        conn_wait_time: GoDuration,
    ) {
        self.called(MetricsCall::AdapterConnections(
            adapter_name.to_owned(),
            conn_was_reused,
            conn_wait_time,
        ));
    }

    fn record_dns_time(&self, dns_lookup_time: GoDuration) {
        self.called(MetricsCall::DnsTime(dns_lookup_time));
    }

    fn record_tls_handshake_time(&self, tls_handshake_time: GoDuration) {
        self.called(MetricsCall::TlsHandshakeTime(tls_handshake_time));
    }

    fn record_bidder_server_response_time(&self, bidder_server_response_time: GoDuration) {
        self.called(MetricsCall::BidderServerResponseTime(
            bidder_server_response_time,
        ));
    }

    fn record_adapter_panic(&self, labels: &AdapterLabels) {
        self.called(MetricsCall::AdapterPanic(labels.clone()));
    }

    fn record_adapter_bid_received(&self, labels: &AdapterLabels, bid_type: &str, has_adm: bool) {
        self.called(MetricsCall::AdapterBidReceived(
            labels.clone(),
            bid_type.to_owned(),
            has_adm,
        ));
    }

    fn record_adapter_price(&self, labels: &AdapterLabels, cpm: f64) {
        self.called(MetricsCall::AdapterPrice(labels.clone(), cpm));
    }

    fn record_adapter_time(&self, labels: &AdapterLabels, length: GoDuration) {
        self.called(MetricsCall::AdapterTime(labels.clone(), length));
    }

    fn record_cookie_sync(&self, status: CookieSyncStatus) {
        self.called(MetricsCall::CookieSync(status));
    }

    fn record_syncer_request(&self, key: &str, status: SyncerCookieSyncStatus) {
        self.called(MetricsCall::SyncerRequest(key.to_owned(), status));
    }

    fn record_set_uid(&self, status: SetUidStatus) {
        self.called(MetricsCall::SetUid(status));
    }

    fn record_syncer_set(&self, key: &str, status: SyncerSetUidStatus) {
        self.called(MetricsCall::SyncerSet(key.to_owned(), status));
    }

    fn record_stored_req_cache_result(&self, cache_result: CacheResult, inc: usize) {
        self.called(MetricsCall::StoredReqCacheResult(cache_result, inc));
    }

    fn record_stored_imp_cache_result(&self, cache_result: CacheResult, inc: usize) {
        self.called(MetricsCall::StoredImpCacheResult(cache_result, inc));
    }

    fn record_account_cache_result(&self, cache_result: CacheResult, inc: usize) {
        self.called(MetricsCall::AccountCacheResult(cache_result, inc));
    }

    fn record_stored_data_fetch_time(&self, labels: StoredDataLabels, length: GoDuration) {
        self.called(MetricsCall::StoredDataFetchTime(labels, length));
    }

    fn record_stored_data_error(&self, labels: StoredDataLabels) {
        self.called(MetricsCall::StoredDataError(labels));
    }

    fn record_fetcher_result(&self, subsystem: &str, result: FetcherResult) {
        self.called(MetricsCall::FetcherResult(subsystem.to_owned(), result));
    }

    fn record_fetcher_backend_fetch(
        &self,
        subsystem: &str,
        operation: FetcherOperation,
        result: FetcherBackendResult,
        length: GoDuration,
    ) {
        self.called(MetricsCall::FetcherBackendFetch(
            subsystem.to_owned(),
            operation,
            result,
            length,
        ));
    }

    fn record_prebid_cache_request_time(&self, success: bool, length: GoDuration) {
        self.called(MetricsCall::PrebidCacheRequestTime(success, length));
    }

    fn record_request_queue_time(
        &self,
        success: bool,
        request_type: RequestType,
        length: GoDuration,
    ) {
        self.called(MetricsCall::RequestQueueTime(success, request_type, length));
    }

    fn record_timeout_notice(&self, success: bool) {
        self.called(MetricsCall::TimeoutNotice(success));
    }

    fn record_request_privacy(&self, privacy: PrivacyLabels) {
        self.called(MetricsCall::RequestPrivacy(privacy));
    }

    fn record_adapter_buyer_uid_scrubbed(&self, adapter_name: &str) {
        self.called(MetricsCall::AdapterBuyerUidScrubbed(
            adapter_name.to_owned(),
        ));
    }

    fn record_adapter_gdpr_request_blocked(&self, adapter_name: &str) {
        self.called(MetricsCall::AdapterGdprRequestBlocked(
            adapter_name.to_owned(),
        ));
    }

    fn record_debug_request(&self, debug_enabled: bool, pub_id: &str) {
        self.called(MetricsCall::DebugRequest(debug_enabled, pub_id.to_owned()));
    }

    fn record_stored_response(&self, pub_id: &str) {
        self.called(MetricsCall::StoredResponse(pub_id.to_owned()));
    }

    fn record_gvl_list_request(&self) {
        self.called(MetricsCall::GvlListRequest);
    }

    fn record_live_gvl_fetch(&self, success: bool) {
        self.called(MetricsCall::LiveGvlFetch(success));
    }

    fn record_ads_cert_req(&self, success: bool) {
        self.called(MetricsCall::AdsCertReq(success));
    }

    fn record_ads_cert_sign_time(&self, ads_cert_sign_time: GoDuration) {
        self.called(MetricsCall::AdsCertSignTime(ads_cert_sign_time));
    }

    fn record_bid_validation_creative_size_error(&self, adapter: &str, account: &str) {
        self.called(MetricsCall::BidValidationCreativeSizeError(
            adapter.to_owned(),
            account.to_owned(),
        ));
    }

    fn record_bid_validation_creative_size_warn(&self, adapter: &str, account: &str) {
        self.called(MetricsCall::BidValidationCreativeSizeWarn(
            adapter.to_owned(),
            account.to_owned(),
        ));
    }

    fn record_bid_validation_secure_markup_error(&self, adapter: &str, account: &str) {
        self.called(MetricsCall::BidValidationSecureMarkupError(
            adapter.to_owned(),
            account.to_owned(),
        ));
    }

    fn record_bid_validation_secure_markup_warn(&self, adapter: &str, account: &str) {
        self.called(MetricsCall::BidValidationSecureMarkupWarn(
            adapter.to_owned(),
            account.to_owned(),
        ));
    }

    fn record_module_called(&self, labels: &ModuleLabels, duration: GoDuration) {
        self.called(MetricsCall::ModuleCalled(labels.clone(), duration));
    }

    fn record_module_failed(&self, labels: &ModuleLabels) {
        self.called(MetricsCall::ModuleFailed(labels.clone()));
    }

    fn record_module_success_nooped(&self, labels: &ModuleLabels) {
        self.called(MetricsCall::ModuleSuccessNooped(labels.clone()));
    }

    fn record_module_success_updated(&self, labels: &ModuleLabels) {
        self.called(MetricsCall::ModuleSuccessUpdated(labels.clone()));
    }

    fn record_module_success_rejected(&self, labels: &ModuleLabels) {
        self.called(MetricsCall::ModuleSuccessRejected(labels.clone()));
    }

    fn record_module_execution_error(&self, labels: &ModuleLabels) {
        self.called(MetricsCall::ModuleExecutionError(labels.clone()));
    }

    fn record_module_timeout(&self, labels: &ModuleLabels) {
        self.called(MetricsCall::ModuleTimeout(labels.clone()));
    }

    fn record_adapter_throttled(&self, adapter_name: &str) {
        self.called(MetricsCall::AdapterThrottled(adapter_name.to_owned()));
    }

    fn record_adapter_connection_dial_error(&self, _adapter_name: &str) {
        self.called(MetricsCall::AdapterConnectionDialError);
    }

    fn record_adapter_connection_dial_time(&self, adapter_name: &str, dial_start_time: GoDuration) {
        self.called(MetricsCall::AdapterConnectionDialTime(
            adapter_name.to_owned(),
            dial_start_time,
        ));
    }
}

#[cfg(test)]
#[path = "metrics_mock_test.rs"]
mod tests;
