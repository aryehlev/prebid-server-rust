//! The exec envelope: one parity case as the runner posts it to both servers
//! (Go: `parity/envelope.go`, `envelope.schema.json`).

use core::{error::Error, fmt};
use std::collections::{BTreeMap, BTreeSet};

use pbs_gocompat::{strconv::quote, strings::equal_fold};
use pbs_gojson::{
    Encoder, GoEncode, GoJson, GoKind, GoType, JsonError, JsoniterDecode, RawJson, StdDecode,
    jsoniter::{self, Iter},
    stdjson::{self, Class, StdDecoder, Token, literal_store, std_decode_slice},
};

use crate::{
    body::Body,
    ids::{DecisionId, MOCK_LEVEL_HTTP, MOCK_MODE_ORACLE},
};

/// One parity case (Go: `parity.Envelope`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.Envelope")]
pub struct Envelope {
    /// Always [`SCHEMA_ENVELOPE`](crate::contracts::SCHEMA_ENVELOPE).
    #[gojson(json = "schema")]
    pub schema: String,
    /// The contract version.
    #[gojson(json = "schema_version")]
    pub schema_version: String,
    /// The case ID.
    #[gojson(go = "CaseID", json = "case_id")]
    pub case_id: String,
    /// The fixture the case was generated from.
    #[gojson(json = "fixture_path,omitempty")]
    pub fixture_path: String,
    /// The server profile the case needs.
    #[gojson(json = "profile,omitempty")]
    pub profile: String,
    /// The inbound request to dispatch through the full handler chain.
    #[gojson(json = "request")]
    pub request: ExecRequest,
    /// How outbound calls are answered.
    #[gojson(json = "mock")]
    pub mock: MockSpec,
    /// Pinned decision points, by decision ID.
    #[gojson(json = "decisions,omitempty")]
    pub decisions: Option<BTreeMap<String, RawJson>>,
    /// The fake clock, random and UUID values for the case.
    #[gojson(json = "fakes,omitempty")]
    pub fakes: Option<FakeValues>,
    /// What the server records.
    #[gojson(json = "capture")]
    pub capture: CaptureSpec,
}

impl Envelope {
    /// The integer override for `point`, if the envelope has one that `json.Unmarshal` accepts
    /// into an `int64` (`null` reads as 0) (Go: `Envelope.DecisionInt`).
    #[must_use]
    pub fn decision_int(&self, point: DecisionId) -> Option<i64> {
        let raw = self.decision(point)?;
        let mut n = 0_i64;
        stdjson::unmarshal(raw.as_bytes(), &mut n).ok()?;
        Some(n)
    }

    /// The string override for `point`, if the envelope has one that `json.Unmarshal` accepts
    /// into a `string` (`null` reads as `""`) (Go: `Envelope.DecisionString`).
    #[must_use]
    pub fn decision_string(&self, point: DecisionId) -> Option<String> {
        let raw = self.decision(point)?;
        let mut s = String::new();
        stdjson::unmarshal(raw.as_bytes(), &mut s).ok()?;
        Some(s)
    }

    /// The raw envelope value of `point`, whatever its type.
    #[must_use]
    pub fn decision(&self, point: DecisionId) -> Option<&RawJson> {
        self.decisions.as_ref()?.get(point.as_str())
    }
}

/// The inbound HTTP request the server dispatches through its outer handler chain
/// (Go: `parity.ExecRequest`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.ExecRequest")]
pub struct ExecRequest {
    /// The method.
    #[gojson(json = "method")]
    pub method: String,
    /// The path.
    #[gojson(json = "path")]
    pub path: String,
    /// The raw query, without `?`.
    #[gojson(json = "query")]
    pub query: String,
    /// The `Host`, when it differs from the server's.
    #[gojson(json = "host,omitempty")]
    pub host: String,
    /// The headers in wire order.
    #[gojson(json = "headers")]
    pub headers: HeaderList,
    /// The client address.
    #[gojson(json = "remote_addr,omitempty")]
    pub remote_addr: String,
    /// The body.
    #[gojson(json = "body,omitempty")]
    pub body: Option<Body>,
}

/// One `[name, value]` header pair (Go: `[2]string`).
///
/// It decodes like a Go array: missing elements become `""` and extra ones are dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

impl GoType for HeaderPair {
    // Go arrays are neither nil-able nor `,string`-quotable, like structs.
    const KIND: GoKind = GoKind::Struct;

    fn go_type_name(out: &mut String) {
        out.push_str("[2]string");
    }
}

impl GoEncode for HeaderPair {
    fn go_encode(&self, enc: &mut Encoder) {
        // Go: encoding/json arrayEncoder, json-iterator arrayEncoder.
        enc.write_byte(b'[');
        self.name.go_encode(enc);
        enc.write_byte(b',');
        self.value.go_encode(enc);
        enc.write_byte(b']');
    }

    fn go_is_empty(&self) -> bool {
        // Go: isEmptyValue is len(v) == 0 for arrays.
        false
    }
}

impl JsoniterDecode for HeaderPair {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        // Go: json-iterator/go reflect_array.go arrayDecoder.Decode and doDecode (elements past
        // the array length are skipped, missing ones keep their values).
        decode_pair_jsoniter(self, it);
        it.wrap_error_with_type::<Self>(": ");
    }
}

/// jsoniter's `arrayDecoder.doDecode` for `[2]string`.
fn decode_pair_jsoniter(pair: &mut HeaderPair, it: &mut Iter<'_>) {
    if it.read_nil() {
        return;
    }
    let mut c = it.next_token();
    if c != b'[' {
        it.report_error(
            "decode array",
            &[b"expect [ or n, but found ".as_slice(), &[c]].concat(),
        );
        return;
    }
    c = it.next_token();
    if c == b']' {
        return;
    }
    it.unread_byte();
    pair.name.jsoniter_decode(it);
    let mut length = 1_usize;
    loop {
        c = it.next_token();
        if c != b',' {
            break;
        }
        if length >= 2 {
            it.skip();
            continue;
        }
        length += 1;
        pair.value.jsoniter_decode(it);
    }
    if c != b']' {
        it.report_error(
            "decode array",
            &[b"expect ], but found ".as_slice(), &[c]].concat(),
        );
    }
}

impl StdDecode for HeaderPair {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        // Go: encoding/json decode.go array (fixed arrays are zero-filled past the input and
        // drop extra elements) and object (an object into an array is a type error).
        match d.peek() {
            Token::Literal => {
                let item = d.read_literal();
                self.std_literal_as::<N>(item, false, d)
            }
            Token::Object => {
                d.type_error::<N>("object");
                d.skip();
                Ok(())
            }
            Token::Array => {
                let mut i = 0_usize;
                if d.array_begin() {
                    loop {
                        match i {
                            0 => self.name.std_decode(d)?,
                            1 => self.value.std_decode(d)?,
                            _ => d.skip(),
                        }
                        i += 1;
                        if !d.array_next() {
                            break;
                        }
                    }
                }
                if i < 1 {
                    self.name.clear();
                }
                if i < 2 {
                    self.value.clear();
                }
                Ok(())
            }
        }
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        // Go: literalStore into an array: null is ignored, anything else is a type error.
        let _stored = literal_store::<N>(item, from_quoted, d, Class::Other)?;
        Ok(())
    }
}

/// An ordered list of `[name, value]` pairs (Go: `parity.HeaderList`, `[][2]string`). Names keep
/// their original case and repeated names keep their relative order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeaderList(pub Option<Vec<HeaderPair>>);

impl GoType for HeaderList {
    const KIND: GoKind = <Option<Vec<HeaderPair>> as GoType>::KIND;

    fn go_type_name(out: &mut String) {
        out.push_str("parity.HeaderList");
    }
}

impl GoEncode for HeaderList {
    fn go_encode(&self, enc: &mut Encoder) {
        self.0.go_encode(enc);
    }

    fn go_is_empty(&self) -> bool {
        self.0.go_is_empty()
    }
}

// The decoders name the named slice type in their errors, as Go's do (`cannot unmarshal object
// into Go struct field ExecRequest.request.headers of type parity.HeaderList`).
impl JsoniterDecode for HeaderList {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        // Go: json-iterator/go reflect_slice.go sliceDecoder of the named type.
        jsoniter::decode_slice::<Self, HeaderPair>(&mut self.0, it);
    }
}

impl StdDecode for HeaderList {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        std_decode_slice::<N, HeaderPair>(&mut self.0, d)
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        HeaderPair::std_slice_literal::<N>(&mut self.0, item, from_quoted, d)
    }
}

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

    /// Every value of `name` (compared with `strings.EqualFold`) in list order
    /// (Go: `HeaderList.Values`).
    #[must_use]
    pub fn values(&self, name: &str) -> Vec<&str> {
        self.pairs()
            .iter()
            .filter(|kv| equal_fold(&kv.name, name))
            .map(|kv| kv.value.as_str())
            .collect()
    }
}

/// How outbound calls are answered during the case (Go: `parity.MockSpec`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.MockSpec")]
pub struct MockSpec {
    /// `oracle` (the default) or `inline`.
    #[gojson(json = "mode,omitempty")]
    pub mode: String,
    /// `http` (the default) or `bidder_response`.
    #[gojson(json = "level,omitempty")]
    pub level: String,
    /// The in-process answers of `inline` mode.
    #[gojson(json = "inline,omitempty")]
    pub inline: Option<InlineMocks>,
    /// The `MakeBids` replacements of the `bidder_response` level, by bidder.
    #[gojson(json = "bidder_responses,omitempty")]
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
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.InlineMocks")]
pub struct InlineMocks {
    /// The rules, in match order.
    #[gojson(json = "rules,omitempty")]
    pub rules: Option<Vec<InlineRule>>,
    /// Answers to bidder calls, by bidder.
    #[gojson(json = "bidders,omitempty")]
    pub bidders: Option<BTreeMap<String, InlineResponse>>,
    /// The answer to bidder calls without their own.
    #[gojson(json = "default_bidder,omitempty")]
    pub default_bidder: Option<InlineResponse>,
    /// The answer to cache calls.
    #[gojson(json = "cache,omitempty")]
    pub cache: Option<InlineResponse>,
    /// Answers to other calls, by outbound kind.
    #[gojson(json = "kinds,omitempty")]
    pub kinds: Option<BTreeMap<String, InlineResponse>>,
}

/// An inline matcher; every non-empty matcher must match (Go: `parity.InlineRule`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.InlineRule")]
pub struct InlineRule {
    /// The rule ID reported in `mock.id`.
    #[gojson(go = "ID", json = "id,omitempty")]
    pub id: String,
    /// The outbound kind to match.
    #[gojson(json = "kind,omitempty")]
    pub kind: String,
    /// The bidder to match.
    #[gojson(json = "bidder,omitempty")]
    pub bidder: String,
    /// A regular expression the request URI must match.
    #[gojson(go = "URIRegex", json = "uri_regex,omitempty")]
    pub uri_regex: String,
    /// The answer.
    #[gojson(json = "response")]
    pub response: InlineResponse,
}

/// A canned outbound response or failure (Go: `parity.InlineResponse`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.InlineResponse")]
pub struct InlineResponse {
    /// The status.
    #[gojson(json = "status,omitempty")]
    pub status: isize,
    /// The headers.
    #[gojson(json = "headers,omitempty")]
    pub headers: HeaderList,
    /// The body.
    #[gojson(json = "body,omitempty")]
    pub body: Option<Body>,
    /// A delay before answering, in milliseconds.
    #[gojson(go = "DelayMS", json = "delay_ms,omitempty")]
    pub delay_ms: isize,
    /// When set, the call fails instead of answering: `timeout`, `refused`, `reset` or `error`.
    #[gojson(json = "fail,omitempty")]
    pub fail: Option<String>,
}

/// A replacement for one bidder's `MakeBids` output (Go: `parity.BidderResponseMock`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.BidderResponseMock")]
pub struct BidderResponseMock {
    /// `MakeBids` returns a nil `*BidderResponse` (with the errors).
    #[gojson(json = "nil_response,omitempty")]
    pub nil_response: bool,
    /// The response currency.
    #[gojson(json = "currency")]
    pub currency: String,
    /// The bids.
    #[gojson(json = "bids")]
    pub bids: Option<Vec<TypedBid>>,
    /// The fledge auction configs.
    #[gojson(json = "fledge_auction_configs,omitempty")]
    pub fledge_auction_configs: RawJson,
    /// The errors `MakeBids` returns.
    #[gojson(json = "errors,omitempty")]
    pub errors: Option<Vec<ErrorSpec>>,
}

/// A typed bid (Go: `parity.TypedBid`, mirroring `adapters.TypedBid`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.TypedBid")]
pub struct TypedBid {
    /// The `openrtb2.Bid`.
    #[gojson(json = "bid")]
    pub bid: RawJson,
    /// The bid type.
    #[gojson(go = "Type", json = "type")]
    pub bid_type: String,
    /// The seat.
    #[gojson(json = "seat,omitempty")]
    pub seat: String,
    /// The bid video.
    #[gojson(json = "video,omitempty")]
    pub video: RawJson,
    /// The bid meta.
    #[gojson(json = "meta,omitempty")]
    pub meta: RawJson,
    /// The deal priority.
    #[gojson(json = "deal_priority,omitempty")]
    pub deal_priority: isize,
}

/// An error to inject, with a Go `errortypes` code (Go: `parity.ErrorSpec`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.ErrorSpec")]
pub struct ErrorSpec {
    /// The `errortypes` code (`error_codes.json`).
    #[gojson(json = "code")]
    pub code: isize,
    /// The message.
    #[gojson(json = "message")]
    pub message: String,
}

/// What the server records (Go: `parity.CaptureSpec`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.CaptureSpec")]
pub struct CaptureSpec {
    /// `"*"` or a list of stage IDs; omitted or `null` means every stage.
    #[gojson(json = "stages,omitempty")]
    pub stages: RawJson,
    /// Whether `metrics_delta` is captured (default true).
    #[gojson(json = "metrics,omitempty")]
    pub metrics: Option<bool>,
    /// Whether analytics objects are captured (default true).
    #[gojson(json = "analytics,omitempty")]
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

/// `parity.StageID` as `encoding/json` decodes it (any string).
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, GoJson)]
#[gojson(go_type = "parity.StageID")]
struct StageName(String);

impl CaptureSpec {
    /// The stage IDs to capture, or `None` for every stage (Go: `CaptureSpec.StageFilter`). The
    /// IDs are not checked against the registry.
    ///
    /// # Errors
    ///
    /// Go's error for a string other than `"*"` (`capture.stages: string value must be "*", got
    /// "all"`) or for a value that is not a list of strings (`capture.stages: json: cannot
    /// unmarshal object into Go value of type []parity.StageID`).
    pub fn stage_filter(&self) -> Result<Option<BTreeSet<String>>, CaptureError> {
        let raw = self.stages.as_bytes();
        if raw.is_empty() || raw == b"null" {
            return Ok(None);
        }
        let mut all = String::new();
        if stdjson::unmarshal(raw, &mut all).is_ok() {
            if all == "*" {
                return Ok(None);
            }
            return Err(CaptureError(format!(
                "capture.stages: string value must be \"*\", got {}",
                quote(&all)
            )));
        }
        let mut list: Option<Vec<StageName>> = None;
        if let Err(err) = stdjson::unmarshal(raw, &mut list) {
            return Err(CaptureError(format!(
                "capture.stages: {}",
                String::from_utf8_lossy(&err.go_text())
            )));
        }
        Ok(Some(
            list.unwrap_or_default()
                .into_iter()
                .map(|id| id.0)
                .collect(),
        ))
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, GoJson)]
#[gojson(go_type = "parity.FakeValues")]
pub struct FakeValues {
    /// The fixed instant, in Unix milliseconds.
    #[gojson(go = "NowUnixMS", json = "now_unix_ms,omitempty")]
    pub now_unix_ms: i64,
    /// The fixed 63-bit random value.
    #[gojson(json = "int63,omitempty")]
    pub int63: i64,
    /// The fixed UUID, also the seed of the generated-UUID stream.
    #[gojson(go = "UUID", json = "uuid,omitempty")]
    pub uuid: String,
}

#[cfg(test)]
#[path = "envelope_test.rs"]
mod tests;
