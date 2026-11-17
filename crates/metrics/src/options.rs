//! The host configuration the engines read.
//!
//! Go: the `Metrics`, `InfluxMetrics`, `PrometheusMetrics` and `DisabledMetrics` structs of
//! `config/config.go`. These are plain option structs so this crate does not depend on the
//! config crate; `Default` is the Go zero value, not the viper defaults (see
//! [`DisabledMetrics::viper_defaults`]; viper also defaults `metric_send_interval` to 20 and
//! `timeout_ms` to 10000).

use std::fmt;

use crate::metrics::GoDuration;

/// Go: `config.Metrics` (`metrics`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metrics {
    /// Go: `Influxdb` (`metrics.influxdb`).
    pub influxdb: InfluxMetrics,
    /// Go: `Prometheus` (`metrics.prometheus`).
    pub prometheus: PrometheusMetrics,
    /// Go: `Disabled` (`metrics.disabled_metrics`).
    pub disabled: DisabledMetrics,
}

impl Metrics {
    /// Go: `(*Metrics).validate`, which validates the Prometheus settings only.
    pub fn validate(&self, errs: &mut Vec<String>) {
        self.prometheus.validate(errs);
    }
}

/// One flag of `metrics.disabled_metrics`.
///
/// Go: a `bool` field of `config.DisabledMetrics`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DisabledMetric {
    /// Go: `AccountAdapterDetails` — stop collecting account-to-adapter metrics.
    AccountAdapterDetails,
    /// Go: `AccountDebug` — stop collecting account debug request metrics.
    AccountDebug,
    /// Go: `AccountStoredResponses` — stop collecting account stored responses metrics.
    AccountStoredResponses,
    /// Go: `AdapterConnectionMetrics` — stop collecting the metrics about the connections to
    /// bidder servers (created or reused connections, connection wait time).
    AdapterConnectionMetrics,
    /// Go: `AdapterConnectionDialMetrics` — stop collecting dial time and dial errors.
    AdapterConnectionDialMetrics,
    /// Go: `AdapterBuyerUIDScrubbed` — stop collecting the per adapter buyer UID scrubbed
    /// metric.
    AdapterBuyerUidScrubbed,
    /// Go: `AdapterGDPRRequestBlocked` — stop collecting the per adapter GDPR request blocked
    /// metric.
    AdapterGdprRequestBlocked,
    /// Go: `AccountModulesMetrics` — stop collecting account modules metrics (read only by
    /// the go-metrics engine).
    AccountModulesMetrics,
}

impl DisabledMetric {
    /// Every flag, in the Go struct's field order.
    pub const ALL: [Self; 8] = [
        Self::AccountAdapterDetails,
        Self::AccountDebug,
        Self::AccountStoredResponses,
        Self::AdapterConnectionMetrics,
        Self::AdapterConnectionDialMetrics,
        Self::AdapterBuyerUidScrubbed,
        Self::AdapterGdprRequestBlocked,
        Self::AccountModulesMetrics,
    ];

    /// The configuration key under `metrics.disabled_metrics` (the Go `mapstructure` tag).
    #[must_use]
    pub const fn config_key(self) -> &'static str {
        match self {
            Self::AccountAdapterDetails => "account_adapter_details",
            Self::AccountDebug => "account_debug",
            Self::AccountStoredResponses => "account_stored_responses",
            Self::AdapterConnectionMetrics => "adapter_connections_metrics",
            Self::AdapterConnectionDialMetrics => "adapter_connections_dial_metrics",
            Self::AdapterBuyerUidScrubbed => "adapter_buyeruid_scrubbed",
            Self::AdapterGdprRequestBlocked => "adapter_gdpr_request_blocked",
            Self::AccountModulesMetrics => "account_modules_metrics",
        }
    }

    /// The flag whose configuration key is `key`.
    #[must_use]
    pub fn from_config_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|flag| flag.config_key() == key)
    }

    const fn bit(self) -> u8 {
        match self {
            Self::AccountAdapterDetails => 1,
            Self::AccountDebug => 1 << 1,
            Self::AccountStoredResponses => 1 << 2,
            Self::AdapterConnectionMetrics => 1 << 3,
            Self::AdapterConnectionDialMetrics => 1 << 4,
            Self::AdapterBuyerUidScrubbed => 1 << 5,
            Self::AdapterGdprRequestBlocked => 1 << 6,
            Self::AccountModulesMetrics => 1 << 7,
        }
    }
}

/// Go: `config.DisabledMetrics` (`metrics.disabled_metrics`): the set of flags that are `true`,
/// each meaning the metric is *not* collected. The default is the Go zero value (nothing
/// disabled).
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct DisabledMetrics {
    bits: u8,
}

impl DisabledMetrics {
    /// Every flag `false` (Go: `config.DisabledMetrics{}`).
    pub const NONE: Self = Self { bits: 0 };

    /// The values viper assigns when the configuration sets nothing.
    ///
    /// Go: `config.SetupViper` (`metrics.disabled_metrics.*` defaults: `account_debug`,
    /// `account_stored_responses`, `adapter_connections_metrics`,
    /// `adapter_connections_dial_metrics` and `adapter_buyeruid_scrubbed` are `true`;
    /// `account_modules_metrics` has no default).
    #[must_use]
    pub const fn viper_defaults() -> Self {
        Self::NONE
            .with(DisabledMetric::AccountDebug, true)
            .with(DisabledMetric::AccountStoredResponses, true)
            .with(DisabledMetric::AdapterConnectionMetrics, true)
            .with(DisabledMetric::AdapterConnectionDialMetrics, true)
            .with(DisabledMetric::AdapterBuyerUidScrubbed, true)
    }

    /// Whether `flag` is `true` (the metric is disabled).
    #[must_use]
    pub const fn is(self, flag: DisabledMetric) -> bool {
        self.bits & flag.bit() != 0
    }

    /// A copy with `flag` set to `disabled`.
    #[must_use]
    pub const fn with(self, flag: DisabledMetric, disabled: bool) -> Self {
        let bits = if disabled {
            self.bits | flag.bit()
        } else {
            self.bits & !flag.bit()
        };
        Self { bits }
    }

    /// Sets `flag` to `disabled`.
    pub const fn set(&mut self, flag: DisabledMetric, disabled: bool) {
        *self = self.with(flag, disabled);
    }
}

impl fmt::Debug for DisabledMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("DisabledMetrics");
        for flag in DisabledMetric::ALL {
            let _builder = s.field(flag.config_key(), &self.is(flag));
        }
        s.finish()
    }
}

/// Go: `config.InfluxMetrics` (`metrics.influxdb`).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct InfluxMetrics {
    /// Go: `Host` (`host`). The go-metrics engine exists only when it is not empty.
    pub host: String,
    /// Go: `Database` (`database`).
    pub database: String,
    /// Go: `Measurement` (`measurement`).
    pub measurement: String,
    /// Go: `Username` (`username`).
    pub username: String,
    /// Go: `Password` (`password`).
    pub password: String,
    /// Go: `AlignTimestamps` (`align_timestamps`).
    pub align_timestamps: bool,
    /// Go: `MetricSendInterval` (`metric_send_interval`), in seconds.
    pub metric_send_interval: i64,
}

impl fmt::Debug for InfluxMetrics {
    /// Redacts the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InfluxMetrics")
            .field("host", &self.host)
            .field("database", &self.database)
            .field("measurement", &self.measurement)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("align_timestamps", &self.align_timestamps)
            .field("metric_send_interval", &self.metric_send_interval)
            .finish()
    }
}

/// Go: `config.PrometheusMetrics` (`metrics.prometheus`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PrometheusMetrics {
    /// Go: `Port` (`port`). The Prometheus engine and listener exist only when it is not 0.
    pub port: i64,
    /// Go: `Namespace` (`namespace`).
    pub namespace: String,
    /// Go: `Subsystem` (`subsystem`).
    pub subsystem: String,
    /// Go: `TimeoutMillisRaw` (`timeout_ms`).
    pub timeout_millis_raw: i64,
}

impl PrometheusMetrics {
    /// Appends the configuration error Go reports for a positive port without a positive
    /// timeout.
    ///
    /// Go: `(*PrometheusMetrics).validate`.
    pub fn validate(&self, errs: &mut Vec<String>) {
        if self.port > 0 && self.timeout_millis_raw <= 0 {
            errs.push(format!(
                "metrics.prometheus.timeout_ms must be positive if metrics.prometheus.port is \
                 defined. Got timeout={} and port={}",
                self.timeout_millis_raw, self.port
            ));
        }
    }

    /// The scrape timeout.
    ///
    /// Go: `(*PrometheusMetrics).Timeout` (`time.Duration(TimeoutMillisRaw) * time.Millisecond`).
    #[must_use]
    pub const fn timeout(&self) -> GoDuration {
        GoDuration::times(self.timeout_millis_raw, GoDuration::MILLISECOND)
    }
}

#[cfg(test)]
#[path = "options_test.rs"]
mod tests;
