//! The `metrics` configuration structs (Go: `config/config.go`, `config/config_test.go`).

use std::error::Error;

use super::{DisabledMetric, DisabledMetrics, InfluxMetrics, Metrics, PrometheusMetrics};
use crate::metrics::GoDuration;

type TestResult = Result<(), Box<dyn Error>>;

/// Go: `TestValidateConfig`'s Prometheus case and `(*PrometheusMetrics).validate`.
#[test]
fn prometheus_validate_requires_a_positive_timeout_for_a_positive_port() -> TestResult {
    let cases = [
        (8080, 10_000, None),
        (8080, 0, Some("Got timeout=0 and port=8080")),
        (8080, -1, Some("Got timeout=-1 and port=8080")),
        (0, 0, None),
        (-1, 0, None),
    ];
    for (port, timeout_millis_raw, expected) in cases {
        let cfg = Metrics {
            prometheus: PrometheusMetrics {
                port,
                timeout_millis_raw,
                ..PrometheusMetrics::default()
            },
            ..Metrics::default()
        };
        let mut errs = Vec::new();
        cfg.validate(&mut errs);
        let expected: Vec<String> = expected
            .into_iter()
            .map(|tail| {
                format!(
                    "metrics.prometheus.timeout_ms must be positive if metrics.prometheus.port is \
                     defined. {tail}"
                )
            })
            .collect();
        ensure_eq!(errs, expected, "port={port} timeout={timeout_millis_raw}");
    }
    Ok(())
}

/// Go: `(*PrometheusMetrics).Timeout`.
#[test]
fn prometheus_timeout_is_in_milliseconds() -> TestResult {
    let cfg = PrometheusMetrics {
        timeout_millis_raw: 10_000,
        ..PrometheusMetrics::default()
    };
    ensure_eq!(
        cfg.timeout(),
        GoDuration::times(10, GoDuration::SECOND),
        "10000ms"
    );
    Ok(())
}

/// Go: `config.SetupViper` defaults for `metrics.disabled_metrics`.
#[test]
fn disabled_metrics_viper_defaults() -> TestResult {
    let defaults = DisabledMetrics::viper_defaults();
    let expected = [
        ("account_adapter_details", false),
        ("account_debug", true),
        ("account_stored_responses", true),
        ("adapter_connections_metrics", true),
        ("adapter_connections_dial_metrics", true),
        ("adapter_buyeruid_scrubbed", true),
        ("adapter_gdpr_request_blocked", false),
        ("account_modules_metrics", false),
    ];
    let actual: Vec<(&str, bool)> = DisabledMetric::ALL
        .into_iter()
        .map(|f| (f.config_key(), defaults.is(f)))
        .collect();
    ensure_eq!(actual, expected, "viper defaults");
    ensure_eq!(
        DisabledMetrics::default(),
        DisabledMetrics::NONE,
        "zero value"
    );
    Ok(())
}

#[test]
fn disabled_metrics_flags_are_independent() -> TestResult {
    for flag in DisabledMetric::ALL {
        let mut disabled = DisabledMetrics::NONE.with(flag, true);
        let others_unset = DisabledMetric::ALL
            .into_iter()
            .filter(|f| *f != flag)
            .all(|f| !disabled.is(f));
        ensure!(disabled.is(flag) && others_unset, "{flag:?}");
        disabled.set(flag, false);
        ensure_eq!(disabled, DisabledMetrics::NONE, "{flag:?} cleared");
        ensure_eq!(
            DisabledMetric::from_config_key(flag.config_key()),
            Some(flag),
            "{flag:?} key"
        );
    }
    ensure_eq!(
        DisabledMetric::from_config_key("unknown"),
        None,
        "unknown key"
    );
    Ok(())
}

#[test]
fn influx_debug_redacts_the_password() -> TestResult {
    let cfg = InfluxMetrics {
        password: "secret".to_owned(),
        ..InfluxMetrics::default()
    };
    let debug = format!("{cfg:?}");
    ensure!(!debug.contains("secret"), "{debug}");
    Ok(())
}
