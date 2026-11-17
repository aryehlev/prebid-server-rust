//! The trace document and its records (Go: `parity/trace.go`, `trace.schema.json`), and the
//! `X-Parity-Meta` header payload (Go: `parity/transport/meta.go`).

use core::{error::Error, fmt};
use std::collections::BTreeMap;

use pbs_gocompat::base64::STD_ENCODING;
use pbs_gojson::{GoJson, GoString, RawJson, stdjson};

use crate::{
    body::Body,
    contracts::META_HEADER,
    envelope::HeaderList,
    ids::{DecisionId, StageId, StageStatus},
};

/// The response of `POST /__parity/exec`: the real HTTP response plus everything the server
/// observed while producing it (Go: `parity.Trace`).
#[derive(Clone, Debug, Default, PartialEq, GoJson)]
#[gojson(go_type = "parity.Trace")]
pub struct Trace {
    /// Always [`SCHEMA_TRACE`](crate::contracts::SCHEMA_TRACE).
    #[gojson(json = "schema")]
    pub schema: String,
    /// The contract version.
    #[gojson(json = "schema_version")]
    pub schema_version: String,
    /// The implementation that produced the trace (`go` or `rust`).
    #[gojson(go = "Impl", json = "impl")]
    pub implementation: String,
    /// The binary that produced the trace.
    #[gojson(json = "build")]
    pub build: BuildInfo,
    /// The case ID.
    #[gojson(go = "CaseID", json = "case_id")]
    pub case_id: String,
    /// The envelope's request path.
    #[gojson(json = "endpoint")]
    pub endpoint: String,
    /// The inbound request as the handler chain received it.
    #[gojson(go = "HTTPIn", json = "http_in")]
    pub http_in: Option<HttpRequest>,
    /// The response the handler chain wrote.
    #[gojson(go = "HTTPResponse", json = "http_response")]
    pub http_response: Option<HttpResponse>,
    /// The recorded stage entries, by stage and key. A stage's map is nil-able like Go's
    /// `map[string]StageEntry` (`None` is written as `null`); a session never writes one.
    #[gojson(json = "stages")]
    pub stages: Option<BTreeMap<StageId, Option<BTreeMap<String, StageEntry>>>>,
    /// The status of every registered stage.
    #[gojson(json = "stage_status")]
    pub stage_status: Option<BTreeMap<StageId, StageStatus>>,
    /// The outbound calls.
    #[gojson(go = "HTTPOut", json = "http_out")]
    pub http_out: Option<Vec<OutboundCall>>,
    /// The consulted decision points, in consultation order.
    #[gojson(json = "decisions")]
    pub decisions: Option<Vec<Decision>>,
    /// The metric series that changed during the case.
    #[gojson(json = "metrics_delta")]
    pub metrics_delta: Option<Vec<MetricDelta>>,
    /// The analytics objects logged during the case.
    #[gojson(json = "analytics")]
    pub analytics: Option<Vec<AnalyticsRecord>>,
    /// Free-form notes.
    #[gojson(json = "notes")]
    pub notes: Option<Vec<GoString>>,
}

/// The binary that produced a trace (Go: `parity.BuildInfo`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.BuildInfo")]
pub struct BuildInfo {
    /// The commit.
    #[gojson(go = "GitSHA", json = "git_sha")]
    pub git_sha: String,
    /// The version.
    #[gojson(json = "version")]
    pub version: String,
}

/// The inbound request as the handler chain received it (Go: `parity.HTTPRequest`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.HTTPRequest")]
pub struct HttpRequest {
    /// The method.
    #[gojson(json = "method")]
    pub method: String,
    /// The path.
    #[gojson(json = "path")]
    pub path: String,
    /// The raw query.
    #[gojson(json = "query")]
    pub query: String,
    /// The headers.
    #[gojson(json = "headers")]
    pub headers: HeaderList,
    /// The body.
    #[gojson(json = "body")]
    pub body: Option<Body>,
}

/// The response the handler chain wrote (Go: `parity.HTTPResponse`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.HTTPResponse")]
pub struct HttpResponse {
    /// The status.
    #[gojson(json = "status")]
    pub status: isize,
    /// The headers.
    #[gojson(json = "headers")]
    pub headers: HeaderList,
    /// The body.
    #[gojson(json = "body")]
    pub body: Option<Body>,
}

/// What one implementation recorded for one (stage, key) (Go: `parity.StageEntry`).
///
/// `json` is the semantic value and `raw` the exact bytes the implementation marshaled, so
/// lexical differences survive the outer document's re-encoding. `errors` and `warnings` are
/// `None` (`null`) when the stage recorded no error list and empty (`[]`) when it recorded an
/// empty one.
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.StageEntry")]
pub struct StageEntry {
    /// The session-wide sequence number of the last write.
    #[gojson(json = "seq")]
    pub seq: isize,
    /// How often the value was written.
    #[gojson(json = "writes")]
    pub writes: isize,
    /// The value, when it is valid JSON.
    #[gojson(go = "JSON", json = "json")]
    pub json: RawJson,
    /// The exact bytes, when they are valid UTF-8.
    #[gojson(json = "raw")]
    pub raw: Option<String>,
    /// The exact bytes in standard base64, when they are not valid UTF-8.
    #[gojson(go = "RawB64", json = "raw_b64,omitempty")]
    pub raw_b64: Option<String>,
    /// The fatal and unknown errors.
    #[gojson(json = "errors")]
    pub errors: Option<Vec<ErrorRecord>>,
    /// The warnings.
    #[gojson(json = "warnings")]
    pub warnings: Option<Vec<ErrorRecord>>,
    /// Notes about the entry.
    #[gojson(json = "notes,omitempty")]
    pub notes: Option<Vec<GoString>>,
}

/// The language-neutral form of a Go error (Go: `parity.ErrorRecord`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.ErrorRecord")]
pub struct ErrorRecord {
    /// The Go `errortypes` code (`error_codes.json`).
    #[gojson(json = "code")]
    pub code: isize,
    /// The canonical name of the code.
    #[gojson(json = "kind")]
    pub kind: String,
    /// `err.Error()`.
    #[gojson(json = "message")]
    pub message: String,
    /// [`SCOPE_GENERAL`] or [`SCOPE_DEBUG`].
    #[gojson(json = "scope")]
    pub scope: String,
    /// [`SEVERITY_FATAL`], [`SEVERITY_WARNING`] or [`SEVERITY_UNKNOWN`].
    #[gojson(json = "severity")]
    pub severity: String,
    /// Whether `errortypes.FatalOnly` keeps the error.
    #[gojson(json = "fatal")]
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
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.OutboundCall")]
pub struct OutboundCall {
    /// The session sequence number.
    #[gojson(json = "seq")]
    pub seq: isize,
    /// The implementation, set by the runner when merging.
    #[gojson(go = "Impl", json = "impl,omitempty")]
    pub implementation: String,
    /// The outbound kind.
    #[gojson(json = "kind")]
    pub kind: String,
    /// The bidder, for bidder calls.
    #[gojson(json = "bidder")]
    pub bidder: String,
    /// The URL the server meant to call.
    #[gojson(go = "OrigURL", json = "orig_url")]
    pub orig_url: String,
    /// The request.
    #[gojson(json = "request")]
    pub request: OutboundRequest,
    /// Whether this is the wire-exact view.
    #[gojson(json = "wire")]
    pub wire: bool,
    /// The mock that answered.
    #[gojson(json = "mock")]
    pub mock: Option<MockRef>,
    /// The response.
    #[gojson(json = "response")]
    pub response: Option<OutboundResponse>,
    /// The transport error.
    #[gojson(json = "error")]
    pub error: Option<String>,
    /// Notes about the call.
    #[gojson(json = "notes,omitempty")]
    pub notes: Option<Vec<GoString>>,
}

/// The request half of an [`OutboundCall`] (Go: `parity.OutboundRequest`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.OutboundRequest")]
pub struct OutboundRequest {
    /// The method.
    #[gojson(json = "method")]
    pub method: String,
    /// The URL.
    #[gojson(go = "URL", json = "url")]
    pub url: String,
    /// The protocol, on the wire-exact view.
    #[gojson(json = "proto,omitempty")]
    pub proto: String,
    /// The headers.
    #[gojson(go = "Header", json = "headers")]
    pub headers: HeaderList,
    /// The body.
    #[gojson(json = "body")]
    pub body: Option<Body>,
}

/// The response half of an [`OutboundCall`] (Go: `parity.OutboundResponse`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.OutboundResponse")]
pub struct OutboundResponse {
    /// The status.
    #[gojson(json = "status")]
    pub status: isize,
    /// The headers.
    #[gojson(json = "headers")]
    pub headers: HeaderList,
    /// The body.
    #[gojson(json = "body")]
    pub body: Option<Body>,
    /// The mock's delay, in milliseconds.
    #[gojson(go = "DelayMS", json = "delay_ms,omitempty")]
    pub delay_ms: isize,
}

/// Which mock answered an outbound call (Go: `parity.MockRef`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.MockRef")]
pub struct MockRef {
    /// The mock source (`inline`, `rule`, `fixture_replay`, ...).
    #[gojson(json = "source")]
    pub source: String,
    /// The mock ID.
    #[gojson(go = "ID", json = "id")]
    pub id: String,
}

/// The `X-Parity-Meta` payload version (Go: `transport.MetaVersion`).
pub const META_VERSION: isize = 1;

/// The decoded `X-Parity-Meta` header (Go: `parity.OutboundMeta`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.OutboundMeta")]
pub struct OutboundMeta {
    /// The payload version ([`META_VERSION`]).
    #[gojson(go = "V", json = "v")]
    pub v: isize,
    /// The case ID.
    #[gojson(json = "case")]
    pub case: String,
    /// The implementation.
    #[gojson(go = "Impl", json = "impl")]
    pub implementation: String,
    /// The outbound kind.
    #[gojson(json = "kind")]
    pub kind: String,
    /// The bidder.
    #[gojson(json = "bidder")]
    pub bidder: String,
    /// The URL the server meant to call.
    #[gojson(go = "OrigURL", json = "orig_url")]
    pub orig_url: String,
    /// The session sequence number of the call.
    #[gojson(json = "seq")]
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
    /// The `X-Parity-Meta` header value: standard base64 (with padding) of `json.Marshal(meta)`
    /// (Go: `transport.EncodeMeta`).
    ///
    /// # Errors
    ///
    /// Go's `parity: encoding X-Parity-Meta: <error>` (unreachable for these field types).
    pub fn encode_header(&self) -> Result<String, MetaError> {
        stdjson::marshal(self)
            .map(|raw| STD_ENCODING.encode_to_string(&raw))
            .map_err(|err| {
                MetaError(format!(
                    "parity: encoding {META_HEADER}: {}",
                    String::from_utf8_lossy(&err.go_text())
                ))
            })
    }

    /// Parses an `X-Parity-Meta` header value (Go: `transport.DecodeMeta`); the oracle uses it
    /// to attribute a captured request.
    ///
    /// # Errors
    ///
    /// Go's `parity: decoding X-Parity-Meta base64: <error>`, `parity: decoding X-Parity-Meta
    /// JSON: <error>` or `parity: unsupported X-Parity-Meta version <v>`.
    pub fn decode_header(value: &str) -> Result<Self, MetaError> {
        let raw = STD_ENCODING
            .decode_string(value)
            .map_err(|err| MetaError(format!("parity: decoding {META_HEADER} base64: {err}")))?;
        let mut meta = Self::default();
        stdjson::unmarshal(&raw, &mut meta).map_err(|err| {
            MetaError(format!(
                "parity: decoding {META_HEADER} JSON: {}",
                String::from_utf8_lossy(&err.go_text())
            ))
        })?;
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
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.Decision")]
pub struct Decision {
    /// The decision point.
    #[gojson(json = "point")]
    pub point: DecisionId,
    /// The value used.
    #[gojson(json = "value")]
    pub value: RawJson,
    /// [`DECISION_SOURCE_ENVELOPE`](crate::ids::DECISION_SOURCE_ENVELOPE) or
    /// [`DECISION_SOURCE_RNG`](crate::ids::DECISION_SOURCE_RNG).
    #[gojson(json = "source")]
    pub source: String,
    /// Why an envelope value was not used.
    #[gojson(json = "notes,omitempty")]
    pub notes: Option<Vec<GoString>>,
}

/// The change of one Prometheus series across a case (Go: `parity.MetricDelta`).
///
/// Counters, gauges and untyped metrics report the value change in `delta`; histograms and
/// summaries report the sum change there and the observation-count changes in `count_delta` and
/// `bucket_deltas`.
#[derive(Clone, Debug, Default, PartialEq, GoJson)]
#[gojson(go_type = "parity.MetricDelta")]
pub struct MetricDelta {
    /// The family name.
    #[gojson(json = "name")]
    pub name: String,
    /// The metric type ([`METRIC_COUNTER`], [`METRIC_HISTOGRAM`], ...).
    #[gojson(go = "Type", json = "type")]
    pub metric_type: String,
    /// The series labels.
    #[gojson(json = "labels")]
    pub labels: Option<BTreeMap<String, String>>,
    /// The value (or sum) change.
    #[gojson(json = "delta")]
    pub delta: f64,
    /// The observation-count change of a histogram or summary.
    #[gojson(json = "count_delta")]
    pub count_delta: Option<i64>,
    /// The changed cumulative bucket counts of a histogram, by upper bound.
    #[gojson(json = "bucket_deltas")]
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
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.AnalyticsRecord")]
pub struct AnalyticsRecord {
    /// The record kind ([`ANALYTICS_AUCTION`], ...).
    #[gojson(json = "kind")]
    pub kind: String,
    /// The object as `jsonutil.Marshal` wrote it.
    #[gojson(json = "object")]
    pub object: RawJson,
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
