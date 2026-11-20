//! The exec envelope: one parity case as the runner posts it to both servers
//! (Go: `parity/envelope.go`, `envelope.schema.json`).

use core::{error::Error, fmt};
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::{
    body::Body,
    ids::{DecisionId, MOCK_LEVEL_HTTP, MOCK_MODE_ORACLE},
};

/// The `omitempty` test of a number or bool member of the documents' Go structs, for
/// `skip_serializing_if`: `true` for its zero value.
pub(crate) fn is_zero<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// The `omitempty` test of a slice or map member, an `Option` of a collection: `true` when it is
/// nil (`None`) or holds nothing (the `Option` iterates over the one collection it holds, or none).
pub(crate) fn nil_or_empty<C, I>(value: &C) -> bool
where
    for<'a> &'a C: IntoIterator<Item = &'a I>,
    for<'a> &'a I: IntoIterator,
{
    value
        .into_iter()
        .all(|inner| inner.into_iter().next().is_none())
}

/// One parity case (Go: `parity.Envelope`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Envelope {
    /// Always [`SCHEMA_ENVELOPE`](crate::contracts::SCHEMA_ENVELOPE).
    pub schema: String,
    /// The contract version.
    pub schema_version: String,
    /// The case ID.
    pub case_id: String,
    /// The fixture the case was generated from.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fixture_path: String,
    /// The server profile the case needs.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub profile: String,
    /// The inbound request to dispatch through the full handler chain.
    pub request: ExecRequest,
    /// How outbound calls are answered.
    pub mock: MockSpec,
    /// Pinned decision points, by decision ID.
    #[serde(skip_serializing_if = "nil_or_empty")]
    pub decisions: Option<BTreeMap<String, Box<RawValue>>>,
    /// The fake clock, random and UUID values for the case.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fakes: Option<FakeValues>,
    /// What the server records.
    pub capture: CaptureSpec,
}

impl Envelope {
    /// The integer override for `point`, if the envelope has one that reads as an `int64` (`null`
    /// reads as 0, as in Go's `json.Unmarshal`) (Go: `Envelope.DecisionInt`).
    #[must_use]
    pub fn decision_int(&self, point: DecisionId) -> Option<i64> {
        let raw = self.decision(point)?;
        serde_json::from_str::<Option<i64>>(raw.get())
            .ok()
            .map(Option::unwrap_or_default)
    }

    /// The string override for `point`, if the envelope has one that reads as a `string` (`null`
    /// reads as `""`) (Go: `Envelope.DecisionString`).
    #[must_use]
    pub fn decision_string(&self, point: DecisionId) -> Option<String> {
        let raw = self.decision(point)?;
        serde_json::from_str::<Option<String>>(raw.get())
            .ok()
            .map(Option::unwrap_or_default)
    }

    /// The raw envelope value of `point`, whatever its type.
    #[must_use]
    pub fn decision(&self, point: DecisionId) -> Option<&RawValue> {
        self.decisions
            .as_ref()?
            .get(point.as_str())
            .map(AsRef::as_ref)
    }
}

/// The inbound HTTP request the server dispatches through its outer handler chain
/// (Go: `parity.ExecRequest`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecRequest {
    /// The method.
    pub method: String,
    /// The path.
    pub path: String,
    /// The raw query, without `?`.
    pub query: String,
    /// The `Host`, when it differs from the server's.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub host: String,
    /// The headers in wire order.
    pub headers: HeaderList,
    /// The client address.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote_addr: String,
    /// The body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Body>,
}

/// One `[name, value]` header pair (Go: `[2]string`), written and read as a two-element array.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(from = "(String, String)", into = "(String, String)")]
pub struct HeaderPair {
    /// The name, in its original case.
    pub name: String,
    /// The value.
    pub value: String,
}

impl HeaderPair {
    /// The pair `[name, value]`.
    #[must_use]
    pub fn new<N: Into<String>, V: Into<String>>(name: N, value: V) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

impl From<(String, String)> for HeaderPair {
    fn from((name, value): (String, String)) -> Self {
        Self { name, value }
    }
}

impl From<HeaderPair> for (String, String) {
    fn from(pair: HeaderPair) -> Self {
        (pair.name, pair.value)
    }
}

/// An ordered list of `[name, value]` pairs (Go: `parity.HeaderList`, `[][2]string`). Names keep
/// their original case and repeated names keep their relative order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HeaderList(pub Option<Vec<HeaderPair>>);

impl HeaderList {
    /// A non-nil list of `pairs`.
    #[must_use]
    pub const fn from_pairs(pairs: Vec<HeaderPair>) -> Self {
        Self(Some(pairs))
    }

    /// The pairs (none for a nil list).
    #[must_use]
    pub fn pairs(&self) -> &[HeaderPair] {
        self.0.as_deref().unwrap_or_default()
    }

    /// Whether the list is nil or holds no pair (Go's `omitempty` test).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pairs().is_empty()
    }

    /// Every value of `name`, compared ignoring case, in list order (Go: `HeaderList.Values`).
    #[must_use]
    pub fn values(&self, name: &str) -> Vec<&str> {
        let name = name.to_lowercase();
        self.pairs()
            .iter()
            .filter(|kv| kv.name.to_lowercase() == name)
            .map(|kv| kv.value.as_str())
            .collect()
    }
}

/// How outbound calls are answered during the case (Go: `parity.MockSpec`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MockSpec {
    /// `oracle` (the default) or `inline`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// `http` (the default) or `bidder_response`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub level: String,
    /// The in-process answers of `inline` mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline: Option<InlineMocks>,
    /// The `MakeBids` replacements of the `bidder_response` level, by bidder.
    #[serde(skip_serializing_if = "nil_or_empty")]
    pub bidder_responses: Option<BTreeMap<String, BidderResponseMock>>,
}

impl MockSpec {
    /// The mode, defaulting to oracle (Go: `MockSpec.EffectiveMode`).
    #[must_use]
    pub fn effective_mode(&self) -> &str {
        if self.mode.is_empty() {
            MOCK_MODE_ORACLE
        } else {
            &self.mode
        }
    }

    /// The level, defaulting to http (Go: `MockSpec.EffectiveLevel`).
    #[must_use]
    pub fn effective_level(&self) -> &str {
        if self.level.is_empty() {
            MOCK_LEVEL_HTTP
        } else {
            &self.level
        }
    }
}

/// In-process answers to outbound calls (Go: `parity.InlineMocks`).
///
/// Resolution order: `rules` (first match), then `bidders[bidder]` / `default_bidder` for bidder
/// calls, `cache` for cache calls, `kinds[kind]` for any other kind; unresolved calls get the
/// built-in defaults.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InlineMocks {
    /// The rules, in match order.
    #[serde(skip_serializing_if = "nil_or_empty")]
    pub rules: Option<Vec<InlineRule>>,
    /// Answers to bidder calls, by bidder.
    #[serde(skip_serializing_if = "nil_or_empty")]
    pub bidders: Option<BTreeMap<String, InlineResponse>>,
    /// The answer to bidder calls without their own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_bidder: Option<InlineResponse>,
    /// The answer to cache calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<InlineResponse>,
    /// Answers to other calls, by outbound kind.
    #[serde(skip_serializing_if = "nil_or_empty")]
    pub kinds: Option<BTreeMap<String, InlineResponse>>,
}

/// An inline matcher; every non-empty matcher must match (Go: `parity.InlineRule`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InlineRule {
    /// The rule ID reported in `mock.id`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The outbound kind to match.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The bidder to match.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bidder: String,
    /// A regular expression the request URI must match.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uri_regex: String,
    /// The answer.
    pub response: InlineResponse,
}

/// A canned outbound response or failure (Go: `parity.InlineResponse`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InlineResponse {
    /// The status.
    #[serde(skip_serializing_if = "is_zero")]
    pub status: isize,
    /// The headers.
    #[serde(skip_serializing_if = "HeaderList::is_empty")]
    pub headers: HeaderList,
    /// The body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Body>,
    /// A delay before answering, in milliseconds.
    #[serde(skip_serializing_if = "is_zero")]
    pub delay_ms: isize,
    /// When set, the call fails instead of answering: `timeout`, `refused`, `reset` or `error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fail: Option<String>,
}

/// A replacement for one bidder's `MakeBids` output (Go: `parity.BidderResponseMock`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BidderResponseMock {
    /// `MakeBids` returns a nil `*BidderResponse` (with the errors).
    #[serde(skip_serializing_if = "is_zero")]
    pub nil_response: bool,
    /// The response currency.
    pub currency: String,
    /// The bids.
    pub bids: Option<Vec<TypedBid>>,
    /// The fledge auction configs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fledge_auction_configs: Option<Box<RawValue>>,
    /// The errors `MakeBids` returns.
    #[serde(skip_serializing_if = "nil_or_empty")]
    pub errors: Option<Vec<ErrorSpec>>,
}

/// A typed bid (Go: `parity.TypedBid`, mirroring `adapters.TypedBid`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TypedBid {
    /// The `openrtb2.Bid`.
    pub bid: Option<Box<RawValue>>,
    /// The bid type.
    #[serde(rename = "type")]
    pub bid_type: String,
    /// The seat.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub seat: String,
    /// The bid video.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<Box<RawValue>>,
    /// The bid meta.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Box<RawValue>>,
    /// The deal priority.
    #[serde(skip_serializing_if = "is_zero")]
    pub deal_priority: isize,
}

/// An error to inject, with a Go `errortypes` code (Go: `parity.ErrorSpec`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ErrorSpec {
    /// The `errortypes` code (`error_codes.json`).
    pub code: isize,
    /// The message.
    pub message: String,
}

/// What the server records (Go: `parity.CaptureSpec`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSpec {
    /// `"*"` or a list of stage IDs; omitted or `null` means every stage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stages: Option<Box<RawValue>>,
    /// Whether `metrics_delta` is captured (default true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<bool>,
    /// Whether analytics objects are captured (default true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analytics: Option<bool>,
}

/// A `capture.stages` value that is neither `"*"` nor a list of strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureError(String);

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for CaptureError {}

impl CaptureSpec {
    /// The stage IDs to capture, or `None` for every stage (Go: `CaptureSpec.StageFilter`). The
    /// IDs are not checked against the registry.
    ///
    /// # Errors
    ///
    /// Go's error for a string other than `"*"` (`capture.stages: string value must be "*", got
    /// "all"`), or `capture.stages:` and `serde_json`'s message for a value that is not a list of
    /// strings (Go: `json: cannot unmarshal object into Go value of type []parity.StageID`).
    pub fn stage_filter(&self) -> Result<Option<BTreeSet<String>>, CaptureError> {
        let Some(raw) = &self.stages else {
            return Ok(None);
        };
        let raw = raw.get();
        if raw == "null" {
            return Ok(None);
        }
        if let Ok(all) = serde_json::from_str::<String>(raw) {
            if all == "*" {
                return Ok(None);
            }
            return Err(CaptureError(format!(
                "capture.stages: string value must be \"*\", got {all:?}"
            )));
        }
        serde_json::from_str::<Vec<String>>(raw)
            .map(|ids| Some(ids.into_iter().collect()))
            .map_err(|err| CaptureError(format!("capture.stages: {err}")))
    }

    /// Whether `metrics_delta` is captured (Go: `CaptureSpec.WantsMetrics`).
    #[must_use]
    pub fn wants_metrics(&self) -> bool {
        self.metrics.is_none_or(|on| on)
    }

    /// Whether analytics objects are captured (Go: `CaptureSpec.WantsAnalytics`).
    #[must_use]
    pub fn wants_analytics(&self) -> bool {
        self.analytics.is_none_or(|on| on)
    }
}

/// Deterministic replacements for the wall clock and the random sources (Go:
/// `parity.FakeValues`, `fakes.json`). Zero fields take the defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct FakeValues {
    /// The fixed instant, in Unix milliseconds.
    #[serde(skip_serializing_if = "is_zero")]
    pub now_unix_ms: i64,
    /// The fixed 63-bit random value.
    #[serde(skip_serializing_if = "is_zero")]
    pub int63: i64,
    /// The fixed UUID, also the seed of the generated-UUID stream.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uuid: String,
}

#[cfg(test)]
#[path = "envelope_test.rs"]
mod tests;
