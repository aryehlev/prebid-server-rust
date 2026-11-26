//! Engine selection, the fan-out engine and the no-op engine.
//!
//! Go: `metrics/config/metrics.go` (package `config` of the metrics tree).

use std::{collections::BTreeMap, error::Error, fmt, sync::Arc};

use crate::{
    metrics::{
        AdapterLabels, CacheResult, CookieSyncStatus, FetcherBackendResult, FetcherOperation,
        FetcherResult, GoDuration, ImpLabels, Labels, MetricsEngine, ModuleLabels, OverheadType,
        PrivacyLabels, RequestType, SetUidStatus, StoredDataLabels, SyncerCookieSyncStatus,
        SyncerSetUidStatus,
    },
    options,
    prometheus::{self, Error as RegistrationError},
};

/// Why [`new_metrics_engine`] could not build the configured engines.
#[derive(Debug)]
pub enum MetricsEngineError {
    /// `metrics.influxdb.host` is set. Go then builds the go-metrics engine and an
    /// `InfluxDB` reporter (`metrics/go_metrics.go`), which this crate does not port.
    InfluxNotSupported,
    /// The Prometheus engine could not register a family: the prometheus crate's error for the
    /// family Go's `MustRegister` panics on during startup (ledger:
    /// `metrics.registration-errors`).
    Registration(RegistrationError),
}

impl fmt::Display for MetricsEngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InfluxNotSupported => f.write_str(
                "metrics.influxdb.host is set, but the InfluxDB (go-metrics) engine is not ported",
            ),
            Self::Registration(err) => fmt::Display::fmt(err, f),
        }
    }
}

impl Error for MetricsEngineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InfluxNotSupported => None,
            Self::Registration(err) => Some(err),
        }
    }
}

impl From<RegistrationError> for MetricsEngineError {
    fn from(err: RegistrationError) -> Self {
        Self::Registration(err)
    }
}

/// Reads the configuration and returns the metrics engine for this instance.
///
/// Go: `config.NewMetricsEngine(cfg, adapterList, syncerKeys, moduleStageNames)`. Go's only
/// caller (`router/router.go`) passes `openrtb_ext.CoreBidderNames()` as `adapterList`, which is
/// also the list the Prometheus engine preloads; `core_bidder_names` is that list.
///
/// The Prometheus engine exists when `metrics.prometheus.port` is not 0. With no engine the result
/// records nothing ([`NilMetricsEngine`]); with one engine it is that engine; with several it is a
/// [`MultiMetricsEngine`] in Go's order (go-metrics first, then Prometheus).
///
/// # Errors
///
/// [`MetricsEngineError::InfluxNotSupported`] when `metrics.influxdb.host` is set, and
/// [`MetricsEngineError::Registration`] when Go would panic while registering the Prometheus
/// families.
pub fn new_metrics_engine(
    cfg: &options::Metrics,
    core_bidder_names: &[String],
    syncer_keys: &[String],
    module_stage_names: &BTreeMap<String, Vec<String>>,
) -> Result<DetailedMetricsEngine, MetricsEngineError> {
    let mut engine_list: Vec<Arc<dyn MetricsEngine>> = Vec::with_capacity(2);
    let mut prometheus_metrics = None;

    if !cfg.influxdb.host.is_empty() {
        return Err(MetricsEngineError::InfluxNotSupported);
    }
    if cfg.prometheus.port != 0 {
        let engine = Arc::new(prometheus::Metrics::new(
            &cfg.prometheus,
            cfg.disabled,
            syncer_keys,
            module_stage_names,
            core_bidder_names,
        )?);
        let as_engine: Arc<dyn MetricsEngine> = Arc::<prometheus::Metrics>::clone(&engine);
        engine_list.push(as_engine);
        prometheus_metrics = Some(engine);
    }

    let metrics_engine: Arc<dyn MetricsEngine> = if engine_list.len() > 1 {
        Arc::new(MultiMetricsEngine(engine_list))
    } else if let Some(engine) = engine_list.pop() {
        engine
    } else {
        Arc::new(NilMetricsEngine)
    };

    Ok(DetailedMetricsEngine {
        metrics_engine,
        prometheus_metrics,
    })
}

/// The selected engine plus links to the underlying engines.
///
/// Go: `config.DetailedMetricsEngine` (its `GoMetrics` field belongs to the go-metrics engine,
/// which is not ported). It implements [`MetricsEngine`] by delegating to `metrics_engine`, as the
/// embedded Go interface does.
#[derive(Clone, Debug)]
pub struct DetailedMetricsEngine {
    /// Go: the embedded `metrics.MetricsEngine`.
    pub metrics_engine: Arc<dyn MetricsEngine>,
    /// Go: `PrometheusMetrics`, `None` (Go: nil) when the Prometheus engine is not configured.
    pub prometheus_metrics: Option<Arc<prometheus::Metrics>>,
}

/// Records metrics into several engines, in order. This can be useful in transitioning an
/// instance from one engine to another: both run in parallel to verify the stats match up.
///
/// Go: `config.MultiMetricsEngine`.
#[derive(Clone, Debug, Default)]
pub struct MultiMetricsEngine(pub Vec<Arc<dyn MetricsEngine>>);

/// An engine that captures no metrics, used when no metrics backend is configured and in tests.
///
/// Go: `config.NilMetricsEngine`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NilMetricsEngine;

/// Implements [`MetricsEngine`] for the fan-out, no-op and delegating engines from one list of the
/// trait's methods.
macro_rules! engine_impls {
    ($( fn $name:ident(&self $(, $arg:ident: $ty:ty)*); )+) => {
        impl MetricsEngine for MultiMetricsEngine {
            $(
                fn $name(&self $(, $arg: $ty)*) {
                    for engine in &self.0 {
                        engine.$name($($arg),*);
                    }
                }
            )+
        }

        impl MetricsEngine for NilMetricsEngine {
            $( fn $name(&self $(, _: $ty)*) {} )+
        }

        impl MetricsEngine for DetailedMetricsEngine {
            $(
                fn $name(&self $(, $arg: $ty)*) {
                    self.metrics_engine.$name($($arg),*);
                }
            )+
        }
    };
}

engine_impls! {
    fn record_connection_accept(&self, success: bool);
    fn record_tmax_timeout(&self);
    fn record_connection_close(&self, success: bool);
    fn record_request(&self, labels: &Labels);
    fn record_imps(&self, labels: ImpLabels);
    fn record_request_time(&self, labels: &Labels, length: GoDuration);
    fn record_overhead_time(&self, overhead: OverheadType, length: GoDuration);
    fn record_adapter_request(&self, labels: &AdapterLabels);
    fn record_adapter_connections(
        &self,
        adapter_name: &str,
        conn_was_reused: bool,
        conn_wait_time: GoDuration
    );
    fn record_dns_time(&self, dns_lookup_time: GoDuration);
    fn record_tls_handshake_time(&self, tls_handshake_time: GoDuration);
    fn record_bidder_server_response_time(&self, bidder_server_response_time: GoDuration);
    fn record_adapter_panic(&self, labels: &AdapterLabels);
    fn record_adapter_bid_received(&self, labels: &AdapterLabels, bid_type: &str, has_adm: bool);
    fn record_adapter_price(&self, labels: &AdapterLabels, cpm: f64);
    fn record_adapter_time(&self, labels: &AdapterLabels, length: GoDuration);
    fn record_cookie_sync(&self, status: CookieSyncStatus);
    fn record_syncer_request(&self, key: &str, status: SyncerCookieSyncStatus);
    fn record_set_uid(&self, status: SetUidStatus);
    fn record_syncer_set(&self, key: &str, status: SyncerSetUidStatus);
    fn record_stored_req_cache_result(&self, cache_result: CacheResult, inc: usize);
    fn record_stored_imp_cache_result(&self, cache_result: CacheResult, inc: usize);
    fn record_account_cache_result(&self, cache_result: CacheResult, inc: usize);
    fn record_stored_data_fetch_time(&self, labels: StoredDataLabels, length: GoDuration);
    fn record_stored_data_error(&self, labels: StoredDataLabels);
    fn record_fetcher_result(&self, subsystem: &str, result: FetcherResult);
    fn record_fetcher_backend_fetch(
        &self,
        subsystem: &str,
        operation: FetcherOperation,
        result: FetcherBackendResult,
        length: GoDuration
    );
    fn record_prebid_cache_request_time(&self, success: bool, length: GoDuration);
    fn record_request_queue_time(
        &self,
        success: bool,
        request_type: RequestType,
        length: GoDuration
    );
    fn record_timeout_notice(&self, success: bool);
    fn record_request_privacy(&self, privacy: PrivacyLabels);
    fn record_adapter_buyer_uid_scrubbed(&self, adapter_name: &str);
    fn record_adapter_gdpr_request_blocked(&self, adapter_name: &str);
    fn record_debug_request(&self, debug_enabled: bool, pub_id: &str);
    fn record_stored_response(&self, pub_id: &str);
    fn record_gvl_list_request(&self);
    fn record_live_gvl_fetch(&self, success: bool);
    fn record_ads_cert_req(&self, success: bool);
    fn record_ads_cert_sign_time(&self, ads_cert_sign_time: GoDuration);
    fn record_bid_validation_creative_size_error(&self, adapter: &str, account: &str);
    fn record_bid_validation_creative_size_warn(&self, adapter: &str, account: &str);
    fn record_bid_validation_secure_markup_error(&self, adapter: &str, account: &str);
    fn record_bid_validation_secure_markup_warn(&self, adapter: &str, account: &str);
    fn record_module_called(&self, labels: &ModuleLabels, duration: GoDuration);
    fn record_module_failed(&self, labels: &ModuleLabels);
    fn record_module_success_nooped(&self, labels: &ModuleLabels);
    fn record_module_success_updated(&self, labels: &ModuleLabels);
    fn record_module_success_rejected(&self, labels: &ModuleLabels);
    fn record_module_execution_error(&self, labels: &ModuleLabels);
    fn record_module_timeout(&self, labels: &ModuleLabels);
    fn record_adapter_throttled(&self, adapter_name: &str);
    fn record_adapter_connection_dial_error(&self, adapter_name: &str);
    fn record_adapter_connection_dial_time(&self, adapter_name: &str, dial_start_time: GoDuration);
}

#[cfg(test)]
#[path = "config_test.rs"]
mod tests;
