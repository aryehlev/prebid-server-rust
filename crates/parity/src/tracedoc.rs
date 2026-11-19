//! The trace document and its records (Go: `parity/trace.go`, `trace.schema.json`), and the
//! `X-Parity-Meta` header payload (Go: `parity/transport/meta.go`).

use core::{error::Error, fmt};
use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::value::RawValue;

use crate::{
    body::Body,
    contracts::META_HEADER,
    envelope::{HeaderList, is_zero, nil_or_empty},
    ids::{DecisionId, StageId, StageStatus},
};

/// The response of `POST /__parity/exec`: the real HTTP response plus everything the server
/// observed while producing it (Go: `parity.Trace`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Trace {
    /// Always [`SCHEMA_TRACE`](crate::contracts::SCHEMA_TRACE).
    pub schema: String,
    /// The contract version.
    pub schema_version: String,
    /// The implementation that produced the trace (`go` or `rust`).
    #[serde(rename = "impl")]
    pub implementation: String,
    /// The binary that produced the trace.
    pub build: BuildInfo,
    /// The case ID.
    pub case_id: String,
    /// The envelope's request path.
    pub endpoint: String,
    /// The inbound request as the handler chain received it.
    pub http_in: Option<HttpRequest>,
    /// The response the handler chain wrote.
    pub http_response: Option<HttpResponse>,
    /// The recorded stage entries, by stage and key. A stage's map is nil-able like Go's
    /// `map[string]StageEntry` (`None` is written as `null`); a session never writes one.
    #[serde(serialize_with = "by_stage_name")]
    pub stages: Option<BTreeMap<StageId, Option<BTreeMap<String, StageEntry>>>>,
    /// The status of every registered stage.
    #[serde(serialize_with = "by_stage_name")]
    pub stage_status: Option<BTreeMap<StageId, StageStatus>>,
    /// The outbound calls.
    pub http_out: Option<Vec<OutboundCall>>,
    /// The consulted decision points, in consultation order.
    pub decisions: Option<Vec<Decision>>,
    /// The metric series that changed during the case.
    pub metrics_delta: Option<Vec<MetricDelta>>,
    /// The analytics objects logged during the case.
    pub analytics: Option<Vec<AnalyticsRecord>>,
    /// Free-form notes.
    pub notes: Option<Vec<String>>,
}

/// Writes a map keyed by stage, or `null`, with its members sorted by stage ID as Go writes a map
/// (the map itself iterates in registry order). `map` is the member's `Option`, which iterates
/// over the one map it holds, or none.
fn by_stage_name<M, V, S>(map: &M, serializer: S) -> Result<S::Ok, S::Error>
where
    for<'a> &'a M: IntoIterator<Item = &'a BTreeMap<StageId, V>>,
    V: Serialize,
    S: Serializer,
{
    let Some(map) = map.into_iter().next() else {
        return serializer.serialize_none();
    };
    let mut members: Vec<(&str, &V)> = map.iter().map(|(k, v)| (k.as_str(), v)).collect();
    members.sort_unstable_by_key(|(name, _)| *name);
    serializer.collect_map(members)
}

/// The binary that produced a trace (Go: `parity.BuildInfo`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildInfo {
    /// The commit.
    pub git_sha: String,
    /// The version.
    pub version: String,
}

/// The inbound request as the handler chain received it (Go: `parity.HTTPRequest`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpRequest {
    /// The method.
    pub method: String,
    /// The path.
    pub path: String,
    /// The raw query.
    pub query: String,
    /// The headers.
    pub headers: HeaderList,
    /// The body.
    pub body: Option<Body>,
}

/// The response the handler chain wrote (Go: `parity.HTTPResponse`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpResponse {
    /// The status.
    pub status: isize,
    /// The headers.
    pub headers: HeaderList,
    /// The body.
    pub body: Option<Body>,
}

/// What one implementation recorded for one (stage, key) (Go: `parity.StageEntry`).
///
/// `json` is the semantic value and `raw` the exact bytes the implementation marshaled, so
/// lexical differences survive the outer document's re-encoding. `errors` and `warnings` are
/// `None` (`null`) when the stage recorded no error list and empty (`[]`) when it recorded an
/// empty one.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StageEntry {
    /// The session-wide sequence number of the last write.
    pub seq: isize,
    /// How often the value was written.
    pub writes: isize,
    /// The value, when it is valid JSON.
    pub json: Option<Box<RawValue>>,
    /// The exact bytes, when they are valid UTF-8.
    pub raw: Option<String>,
    /// The exact bytes in standard base64, when they are not valid UTF-8.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_b64: Option<String>,
    /// The fatal and unknown errors.
    pub errors: Option<Vec<ErrorRecord>>,
    /// The warnings.
    pub warnings: Option<Vec<ErrorRecord>>,
    /// Notes about the entry.
    #[serde(skip_serializing_if = "nil_or_empty")]
    pub notes: Option<Vec<String>>,
}

/// The language-neutral form of a Go error (Go: `parity.ErrorRecord`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ErrorRecord {
    /// The Go `errortypes` code (`error_codes.json`).
    pub code: isize,
    /// The canonical name of the code.
    pub kind: String,
    /// `err.Error()`.
    pub message: String,
    /// [`SCOPE_GENERAL`] or [`SCOPE_DEBUG`].
    pub scope: String,
    /// [`SEVERITY_FATAL`], [`SEVERITY_WARNING`] or [`SEVERITY_UNKNOWN`].
    pub severity: String,
    /// Whether `errortypes.FatalOnly` keeps the error.
    pub fatal: bool,
}

/// [`ErrorRecord::scope`] of an error outside the debug scope (Go: `parity.ScopeGeneral`).
pub const SCOPE_GENERAL: &str = "general";
/// [`ErrorRecord::scope`] of a debug-only warning (Go: `parity.ScopeDebug`).
pub const SCOPE_DEBUG: &str = "debug";
/// [`ErrorRecord::severity`] of a fatal `Coder` (Go: `parity.SeverityFatal`).
pub const SEVERITY_FATAL: &str = "fatal";
/// [`ErrorRecord::severity`] of a warning `Coder` (Go: `parity.SeverityWarning`).
pub const SEVERITY_WARNING: &str = "warning";
/// [`ErrorRecord::severity`] of an error that is not a `Coder` (Go: `parity.SeverityUnknown`).
pub const SEVERITY_UNKNOWN: &str = "unknown";
/// [`ErrorRecord::kind`] of a code outside the registry (Go: `parity.ErrorKindUnknown`).
pub const ERROR_KIND_UNKNOWN: &str = "Unknown";

/// One outbound HTTP exchange (Go: `parity.OutboundCall`). Inline mode records the client-side
/// view (`wire` false); in oracle mode the oracle records the wire-exact view.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutboundCall {
    /// The session sequence number.
    pub seq: isize,
    /// The implementation, set by the runner when merging.
    #[serde(rename = "impl", skip_serializing_if = "String::is_empty")]
    pub implementation: String,
    /// The outbound kind.
    pub kind: String,
    /// The bidder, for bidder calls.
    pub bidder: String,
    /// The URL the server meant to call.
    pub orig_url: String,
    /// The request.
    pub request: OutboundRequest,
    /// Whether this is the wire-exact view.
    pub wire: bool,
    /// The mock that answered.
    pub mock: Option<MockRef>,
    /// The response.
    pub response: Option<OutboundResponse>,
    /// The transport error.
    pub error: Option<String>,
    /// Notes about the call.
    #[serde(skip_serializing_if = "nil_or_empty")]
    pub notes: Option<Vec<String>>,
}

/// The request half of an [`OutboundCall`] (Go: `parity.OutboundRequest`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutboundRequest {
    /// The method.
    pub method: String,
    /// The URL.
    pub url: String,
    /// The protocol, on the wire-exact view.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub proto: String,
    /// The headers.
    pub headers: HeaderList,
    /// The body.
    pub body: Option<Body>,
}

/// The response half of an [`OutboundCall`] (Go: `parity.OutboundResponse`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutboundResponse {
    /// The status.
    pub status: isize,
    /// The headers.
    pub headers: HeaderList,
    /// The body.
    pub body: Option<Body>,
    /// The mock's delay, in milliseconds.
    #[serde(skip_serializing_if = "is_zero")]
    pub delay_ms: isize,
}

/// Which mock answered an outbound call (Go: `parity.MockRef`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MockRef {
    /// The mock source (`inline`, `rule`, `fixture_replay`, ...).
    pub source: String,
    /// The mock ID.
    pub id: String,
}

/// The `X-Parity-Meta` payload version (Go: `transport.MetaVersion`).
pub const META_VERSION: isize = 1;

/// The decoded `X-Parity-Meta` header (Go: `parity.OutboundMeta`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutboundMeta {
    /// The payload version ([`META_VERSION`]).
    pub v: isize,
    /// The case ID.
    pub case: String,
    /// The implementation.
    #[serde(rename = "impl")]
    pub implementation: String,
    /// The outbound kind.
    pub kind: String,
    /// The bidder.
    pub bidder: String,
    /// The URL the server meant to call.
    pub orig_url: String,
    /// The session sequence number of the call.
    pub seq: isize,
}

/// An `X-Parity-Meta` payload that could not be encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaError(String);

impl fmt::Display for MetaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for MetaError {}

impl OutboundMeta {
    /// The `X-Parity-Meta` header value: standard base64 (with padding) of the payload as
    /// `serde_json` writes it (Go: `transport.EncodeMeta`, which writes `json.Marshal(meta)`, so
    /// `<`, `>` and `&` in its strings are `\u` escapes there; the oracle decodes either).
    ///
    /// # Errors
    ///
    /// `parity: encoding X-Parity-Meta: <error>` (unreachable for these field types).
    pub fn encode_header(&self) -> Result<String, MetaError> {
        serde_json::to_vec(self)
            .map(|raw| STANDARD.encode(raw))
            .map_err(|err| MetaError(format!("parity: encoding {META_HEADER}: {err}")))
    }

    /// Parses an `X-Parity-Meta` header value (Go: `transport.DecodeMeta`); the oracle uses it
    /// to attribute a captured request.
    ///
    /// # Errors
    ///
    /// Go's `parity: decoding X-Parity-Meta base64: <error>`, `parity: decoding X-Parity-Meta
    /// JSON: <error>` or `parity: unsupported X-Parity-Meta version <v>`, with the base64 crate's
    /// and `serde_json`'s messages inside.
    pub fn decode_header(value: &str) -> Result<Self, MetaError> {
        let raw = STANDARD
            .decode(value)
            .map_err(|err| MetaError(format!("parity: decoding {META_HEADER} base64: {err}")))?;
        let meta: Self = serde_json::from_slice(&raw)
            .map_err(|err| MetaError(format!("parity: decoding {META_HEADER} JSON: {err}")))?;
        if meta.v != META_VERSION {
            return Err(MetaError(format!(
                "parity: unsupported {META_HEADER} version {}",
                meta.v
            )));
        }
        Ok(meta)
    }
}

/// One consulted decision point (Go: `parity.Decision`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Decision {
    /// The decision point.
    pub point: DecisionId,
    /// The value used.
    pub value: Option<Box<RawValue>>,
    /// [`DECISION_SOURCE_ENVELOPE`](crate::ids::DECISION_SOURCE_ENVELOPE) or
    /// [`DECISION_SOURCE_RNG`](crate::ids::DECISION_SOURCE_RNG).
    pub source: String,
    /// Why an envelope value was not used.
    #[serde(skip_serializing_if = "nil_or_empty")]
    pub notes: Option<Vec<String>>,
}

/// The change of one Prometheus series across a case (Go: `parity.MetricDelta`).
///
/// Counters, gauges and untyped metrics report the value change in `delta`; histograms and
/// summaries report the sum change there and the observation-count changes in `count_delta` and
/// `bucket_deltas`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricDelta {
    /// The family name.
    pub name: String,
    /// The metric type ([`METRIC_COUNTER`], [`METRIC_HISTOGRAM`], ...).
    #[serde(rename = "type")]
    pub metric_type: String,
    /// The series labels.
    pub labels: Option<BTreeMap<String, String>>,
    /// The value (or sum) change.
    pub delta: f64,
    /// The observation-count change of a histogram or summary.
    pub count_delta: Option<i64>,
    /// The changed cumulative bucket counts of a histogram, by upper bound.
    pub bucket_deltas: Option<BTreeMap<String, i64>>,
}

/// [`MetricDelta::metric_type`] of a counter (Go: `parity.MetricCounter`).
pub const METRIC_COUNTER: &str = "counter";
/// [`MetricDelta::metric_type`] of a gauge (Go: `parity.MetricGauge`).
pub const METRIC_GAUGE: &str = "gauge";
/// [`MetricDelta::metric_type`] of a histogram (Go: `parity.MetricHistogram`).
pub const METRIC_HISTOGRAM: &str = "histogram";
/// [`MetricDelta::metric_type`] of a summary (Go: `parity.MetricSummary`).
pub const METRIC_SUMMARY: &str = "summary";
/// [`MetricDelta::metric_type`] of an untyped metric (Go: `parity.MetricUntyped`).
pub const METRIC_UNTYPED: &str = "untyped";

/// One analytics `Log*Object` call observed during the case (Go: `parity.AnalyticsRecord`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalyticsRecord {
    /// The record kind ([`ANALYTICS_AUCTION`], ...).
    pub kind: String,
    /// The object as `jsonutil.Marshal` wrote it.
    pub object: Option<Box<RawValue>>,
}

/// [`AnalyticsRecord::kind`] of `LogAuctionObject` (Go: `parity.AnalyticsAuction`).
pub const ANALYTICS_AUCTION: &str = "auction";
/// [`AnalyticsRecord::kind`] of `LogAmpObject` (Go: `parity.AnalyticsAmp`).
pub const ANALYTICS_AMP: &str = "amp";
/// [`AnalyticsRecord::kind`] of `LogVideoObject` (Go: `parity.AnalyticsVideo`).
pub const ANALYTICS_VIDEO: &str = "video";
/// [`AnalyticsRecord::kind`] of `LogCookieSyncObject` (Go: `parity.AnalyticsCookieSync`).
pub const ANALYTICS_COOKIE_SYNC: &str = "cookie_sync";
/// [`AnalyticsRecord::kind`] of `LogSetUIDObject` (Go: `parity.AnalyticsSetUID`).
pub const ANALYTICS_SETUID: &str = "setuid";
/// [`AnalyticsRecord::kind`] of `LogNotificationEventObject` (Go:
/// `parity.AnalyticsNotification`).
pub const ANALYTICS_NOTIFICATION: &str = "notification";
