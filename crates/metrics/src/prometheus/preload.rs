//! Creation of the label value combinations that exist from startup.
//!
//! Go: `metrics/prometheus/preload.go`. Every family listed here is scraped with zero values for
//! every combination of the listed label values before anything is recorded.

use std::collections::{BTreeMap, HashMap};

use ::prometheus::{Error, HistogramVec, IntCounterVec};

use super::{
    ADAPTER_ERROR_LABEL, ADAPTER_LABEL, CACHE_RESULT_LABEL, CONNECTION_ACCEPT_ERROR,
    CONNECTION_CLOSE_ERROR, CONNECTION_ERROR_LABEL, COOKIE_LABEL, HAS_BIDS_LABEL, IS_AUDIO_LABEL,
    IS_BANNER_LABEL, IS_NATIVE_LABEL, IS_VIDEO_LABEL, MARKUP_DELIVERY_ADM, MARKUP_DELIVERY_LABEL,
    MARKUP_DELIVERY_NURL, Metrics, OPT_OUT_LABEL, OVERHEAD_TYPE_LABEL, REQUEST_REJECT_LABEL,
    REQUEST_STATUS_LABEL, REQUEST_SUCCESS_LABEL, REQUEST_TYPE_LABEL, SOURCE_LABEL, SOURCE_REQUEST,
    STAGE_LABEL, STATUS_LABEL, STORED_DATA_ERROR_LABEL, STORED_DATA_FETCH_TYPE_LABEL,
    SUCCESS_LABEL, SYNCER_LABEL, VERSION_LABEL,
    type_conversion::{bool_values_as_string, enum_as_lower_case_string, enum_as_string},
};
use crate::{metrics, options::DisabledMetric};

/// Label names with the values to preload for each (Go: `map[string][]string`).
type LabelValues<'a> = [(&'a str, &'a [String])];

/// One combination of label values, by label name (Go: `prometheus.Labels`).
pub(super) type PromLabels<'a> = HashMap<&'a str, &'a str>;

/// The label value lists of `preloadLabelValues`.
struct Values {
    adapter_error: Vec<String>,
    adapter: Vec<String>,
    bid_type: Vec<String>,
    bool: Vec<String>,
    cache_result: Vec<String>,
    connection_error: Vec<String>,
    cookie_sync_status: Vec<String>,
    cookie: Vec<String>,
    overhead_types: Vec<String>,
    request_status: Vec<String>,
    request_type: Vec<String>,
    set_uid_status: Vec<String>,
    source: Vec<String>,
    stored_data_error: Vec<String>,
    stored_data_fetch_type: Vec<String>,
    syncer_request_status: Vec<String>,
    syncer_sets_status: Vec<String>,
    tcf_version: Vec<String>,
}

impl Values {
    /// The adapter values are `core_bidder_names` lowercased (Go:
    /// `enumAsLowerCaseString(openrtb_ext.CoreBidderNames())`).
    fn new(core_bidder_names: &[String]) -> Self {
        Self {
            adapter_error: enum_as_string(metrics::adapter_errors()),
            adapter: enum_as_lower_case_string(core_bidder_names),
            bid_type: vec![
                MARKUP_DELIVERY_ADM.to_owned(),
                MARKUP_DELIVERY_NURL.to_owned(),
            ],
            bool: bool_values_as_string(),
            cache_result: enum_as_string(metrics::cache_results()),
            connection_error: vec![
                CONNECTION_ACCEPT_ERROR.to_owned(),
                CONNECTION_CLOSE_ERROR.to_owned(),
            ],
            cookie_sync_status: enum_as_string(metrics::cookie_sync_statuses()),
            cookie: enum_as_string(metrics::cookie_types()),
            overhead_types: enum_as_string(metrics::overhead_types()),
            request_status: enum_as_string(metrics::request_statuses()),
            request_type: enum_as_string(metrics::request_types()),
            set_uid_status: enum_as_string(metrics::set_uid_statuses()),
            source: vec![SOURCE_REQUEST.to_owned()],
            stored_data_error: enum_as_string(metrics::stored_data_errors()),
            stored_data_fetch_type: enum_as_string(metrics::stored_data_fetch_types()),
            syncer_request_status: enum_as_string(metrics::syncer_request_statuses()),
            syncer_sets_status: enum_as_string(metrics::syncer_set_uid_statuses()),
            tcf_version: enum_as_string(metrics::tcf_versions()),
        }
    }
}

/// Go: `preloadLabelValues`.
pub(super) fn preload_label_values(
    m: &Metrics,
    syncer_keys: &[String],
    module_stage_names: &BTreeMap<String, Vec<String>>,
    core_bidder_names: &[String],
) -> Result<(), Error> {
    let v = Values::new(core_bidder_names);
    preload_request_families(m, &v)?;
    preload_stored_families(m, &v)?;
    preload_adapter_families(m, &v)?;
    preload_syncer_and_privacy_families(m, &v, syncer_keys)?;
    preload_module_families(m, module_stage_names)
}

fn preload_request_families(m: &Metrics, v: &Values) -> Result<(), Error> {
    counter(
        &m.connections_error,
        &[(CONNECTION_ERROR_LABEL, &v.connection_error)],
    )?;
    counter(&m.cookie_sync, &[(STATUS_LABEL, &v.cookie_sync_status)])?;
    counter(&m.set_uid, &[(STATUS_LABEL, &v.set_uid_status)])?;
    counter(
        &m.impressions,
        &[
            (IS_BANNER_LABEL, &v.bool),
            (IS_VIDEO_LABEL, &v.bool),
            (IS_AUDIO_LABEL, &v.bool),
            (IS_NATIVE_LABEL, &v.bool),
        ],
    )?;
    histogram(&m.prebid_cache_write_timer, &[(SUCCESS_LABEL, &v.bool)])?;
    counter(
        &m.requests,
        &[
            (REQUEST_TYPE_LABEL, &v.request_type),
            (REQUEST_STATUS_LABEL, &v.request_status),
        ],
    )?;
    histogram(&m.requests_timer, &[(REQUEST_TYPE_LABEL, &v.request_type)])
}

fn preload_stored_families(m: &Metrics, v: &Values) -> Result<(), Error> {
    let fetch_types: &LabelValues<'_> =
        &[(STORED_DATA_FETCH_TYPE_LABEL, &v.stored_data_fetch_type)];
    histogram(&m.stored_account_fetch_timer, fetch_types)?;
    histogram(&m.stored_amp_fetch_timer, fetch_types)?;
    histogram(&m.stored_category_fetch_timer, fetch_types)?;
    histogram(&m.stored_request_fetch_timer, fetch_types)?;
    histogram(&m.stored_video_fetch_timer, fetch_types)?;
    histogram(&m.stored_responses_fetch_timer, fetch_types)?;

    let errors: &LabelValues<'_> = &[(STORED_DATA_ERROR_LABEL, &v.stored_data_error)];
    counter(&m.stored_account_errors, errors)?;
    counter(&m.stored_amp_errors, errors)?;
    counter(&m.stored_category_errors, errors)?;
    counter(&m.stored_request_errors, errors)?;
    counter(&m.stored_video_errors, errors)?;
    counter(&m.stored_responses_errors, errors)?;

    counter(
        &m.requests_without_cookie,
        &[(REQUEST_TYPE_LABEL, &v.request_type)],
    )?;

    let cache_results: &LabelValues<'_> = &[(CACHE_RESULT_LABEL, &v.cache_result)];
    counter(&m.stored_impressions_cache_result, cache_results)?;
    counter(&m.stored_request_cache_result, cache_results)?;
    counter(&m.account_cache_result, cache_results)
}

fn preload_adapter_families(m: &Metrics, v: &Values) -> Result<(), Error> {
    let adapters: &LabelValues<'_> = &[(ADAPTER_LABEL, &v.adapter)];
    counter(
        &m.adapter_bids,
        &[
            (ADAPTER_LABEL, &v.adapter),
            (MARKUP_DELIVERY_LABEL, &v.bid_type),
        ],
    )?;
    counter(
        &m.adapter_errors,
        &[
            (ADAPTER_LABEL, &v.adapter),
            (ADAPTER_ERROR_LABEL, &v.adapter_error),
        ],
    )?;
    counter(&m.adapter_panics, adapters)?;

    let adapter_success: &LabelValues<'_> =
        &[(ADAPTER_LABEL, &v.adapter), (SUCCESS_LABEL, &v.bool)];
    counter(&m.adapter_bid_response_secure_markup_error, adapter_success)?;
    counter(&m.adapter_bid_response_secure_markup_warn, adapter_success)?;
    counter(
        &m.adapter_bid_response_validation_size_error,
        adapter_success,
    )?;
    counter(
        &m.adapter_bid_response_validation_size_warn,
        adapter_success,
    )?;

    histogram(&m.adapter_prices, adapters)?;
    counter(
        &m.adapter_requests,
        &[
            (ADAPTER_LABEL, &v.adapter),
            (COOKIE_LABEL, &v.cookie),
            (HAS_BIDS_LABEL, &v.bool),
        ],
    )?;
    counter(&m.ads_cert_requests, &[(SUCCESS_LABEL, &v.bool)])?;

    if !m.disabled(DisabledMetric::AdapterConnectionMetrics) {
        optional_counter(m.adapter_created_connections.as_ref(), adapters)?;
        optional_counter(m.adapter_reused_connections.as_ref(), adapters)?;
        optional_histogram(m.adapter_connection_wait_time.as_ref(), adapters)?;

        if !m.disabled(DisabledMetric::AdapterConnectionDialMetrics) {
            optional_counter(m.adapter_connection_dial_errors.as_ref(), adapters)?;
            optional_histogram(m.adapter_connection_dial_time.as_ref(), adapters)?;
        }
    }

    histogram(&m.adapter_requests_timer, adapters)?;
    histogram(
        &m.overhead_timer,
        &[(OVERHEAD_TYPE_LABEL, &v.overhead_types)],
    )
}

fn preload_syncer_and_privacy_families(
    m: &Metrics,
    v: &Values,
    syncer_keys: &[String],
) -> Result<(), Error> {
    counter(
        &m.syncer_requests,
        &[
            (SYNCER_LABEL, syncer_keys),
            (STATUS_LABEL, &v.syncer_request_status),
        ],
    )?;
    counter(
        &m.syncer_sets,
        &[
            (SYNCER_LABEL, syncer_keys),
            (STATUS_LABEL, &v.syncer_sets_status),
        ],
    )?;

    // To minimize memory usage, the queue time metric is preloaded for the video endpoint only;
    // the two values are the general request statuses accepted and rejected.
    histogram(
        &m.requests_queue_timer,
        &[
            (
                REQUEST_TYPE_LABEL,
                &[metrics::RequestType::VIDEO.as_str().to_owned()],
            ),
            (
                REQUEST_STATUS_LABEL,
                &[
                    REQUEST_SUCCESS_LABEL.to_owned(),
                    REQUEST_REJECT_LABEL.to_owned(),
                ],
            ),
        ],
    )?;

    counter(
        &m.privacy_ccpa,
        &[(SOURCE_LABEL, &v.source), (OPT_OUT_LABEL, &v.bool)],
    )?;
    counter(&m.privacy_coppa, &[(SOURCE_LABEL, &v.source)])?;
    counter(&m.privacy_lmt, &[(SOURCE_LABEL, &v.source)])?;
    counter(
        &m.privacy_tcf,
        &[(SOURCE_LABEL, &v.source), (VERSION_LABEL, &v.tcf_version)],
    )?;

    let adapters: &LabelValues<'_> = &[(ADAPTER_LABEL, &v.adapter)];
    if !m.disabled(DisabledMetric::AdapterBuyerUidScrubbed) {
        optional_counter(m.adapter_scrubbed_buyer_uids.as_ref(), adapters)?;
    }
    if !m.disabled(DisabledMetric::AdapterGdprRequestBlocked) {
        optional_counter(m.adapter_gdpr_blocked_requests.as_ref(), adapters)?;
    }
    Ok(())
}

fn preload_module_families(
    m: &Metrics,
    module_stage_names: &BTreeMap<String, Vec<String>>,
) -> Result<(), Error> {
    let modules = &m.modules;
    for (module, stage_values) in module_stage_names {
        let stages: &LabelValues<'_> = &[(STAGE_LABEL, stage_values)];
        optional_histogram(modules.duration.get(module), stages)?;
        for families in [
            &modules.calls,
            &modules.failures,
            &modules.success_noops,
            &modules.success_updates,
            &modules.success_rejects,
            &modules.execution_errors,
            &modules.timeouts,
        ] {
            optional_counter(families.get(module), stages)?;
        }
    }
    Ok(())
}

/// Go: `preloadLabelValuesForCounter`: `counter.With(labels)` for every combination, which
/// creates the series at zero.
fn counter(counter: &IntCounterVec, labels_with_values: &LabelValues<'_>) -> Result<(), Error> {
    register_label_permutations(labels_with_values, |labels| {
        let _series = counter.get_metric_with(labels)?;
        Ok(())
    })
}

/// Go: `preloadLabelValuesForHistogram`.
fn histogram(histogram: &HistogramVec, labels_with_values: &LabelValues<'_>) -> Result<(), Error> {
    register_label_permutations(labels_with_values, |labels| {
        let _series = histogram.get_metric_with(labels)?;
        Ok(())
    })
}

fn optional_counter(
    family: Option<&IntCounterVec>,
    labels_with_values: &LabelValues<'_>,
) -> Result<(), Error> {
    family.map_or(Ok(()), |family| counter(family, labels_with_values))
}

fn optional_histogram(
    family: Option<&HistogramVec>,
    labels_with_values: &LabelValues<'_>,
) -> Result<(), Error> {
    family.map_or(Ok(()), |family| histogram(family, labels_with_values))
}

/// Go: `registerLabelPermutations` — calls `register` with every combination of one value per
/// label. Nothing is registered when there are no labels or when any label has no values.
///
/// # Errors
///
/// The first error `register` returns.
pub(super) fn register_label_permutations<'a, F>(
    labels_with_values: &LabelValues<'a>,
    mut register: F,
) -> Result<(), Error>
where
    F: FnMut(&PromLabels<'a>) -> Result<(), Error>,
{
    if labels_with_values.is_empty() {
        return Ok(());
    }
    let mut labels = PromLabels::new();
    register_label_permutations_recursive(0, labels_with_values, &mut labels, &mut register)
}

/// Go: `registerLabelPermutationsRecursive`.
fn register_label_permutations_recursive<'a, F>(
    depth: usize,
    labels_with_values: &LabelValues<'a>,
    labels: &mut PromLabels<'a>,
    register: &mut F,
) -> Result<(), Error>
where
    F: FnMut(&PromLabels<'a>) -> Result<(), Error>,
{
    let Some(&(label, values)) = labels_with_values.get(depth) else {
        return Ok(());
    };
    let is_leaf = depth + 1 == labels_with_values.len();
    for value in values {
        let _previous = labels.insert(label, value.as_str());
        if is_leaf {
            register(labels)?;
        } else {
            register_label_permutations_recursive(depth + 1, labels_with_values, labels, register)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "preload_test.rs"]
mod tests;
