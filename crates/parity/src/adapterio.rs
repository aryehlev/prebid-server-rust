//! The documents of the adapter endpoints and the introspection routes
//! (Go: `parity/adapterio.go`, `adapter_io.schema.json`, `introspection.schema.json`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::{
    body::Body,
    envelope::{FakeValues, HeaderList, TypedBid},
    ids::{DecisionId, StageId},
    tracedoc::{BuildInfo, ErrorRecord},
};

/// [`AdapterBuild::wrap`]: the adapter exactly as its builder returns it, which the
/// `adapterstest` JSON fixtures exercise (Go: `parity.WrapRaw`).
pub const WRAP_RAW: &str = "raw";
/// [`AdapterBuild::wrap`]: the adapter wrapped with `BuildInfoAwareBidder`, as the exchange uses
/// it in production (Go: `parity.WrapInfoAware`).
pub const WRAP_INFO_AWARE: &str = "info_aware";

/// Selects and configures the adapter under test (Go: `parity.AdapterBuild`). Omitted
/// `adapter_config`/`server` fall back to what the server derives from its own bidder info and
/// host config.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdapterBuild {
    /// The bidder.
    pub bidder: String,
    /// [`WRAP_RAW`] or [`WRAP_INFO_AWARE`].
    pub wrap: String,
    /// The builder's adapter config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_config: Option<AdapterConfig>,
    /// The builder's server config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerConfig>,
}

/// Mirrors `config.Adapter`, the builder's second argument (Go: `parity.AdapterConfig`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdapterConfig {
    /// The endpoint template.
    pub endpoint: String,
    /// The extra info.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub extra_info: String,
    /// The platform ID.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub platform_id: String,
    /// The app secret.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub app_secret: String,
    /// The XAPI settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xapi: Option<XapiConfig>,
}

/// Mirrors `config.AdapterXAPI` (Go: `parity.XAPIConfig`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct XapiConfig {
    /// The user name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// The password.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// The tracker.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tracker: String,
}

/// Mirrors `config.Server`, the builder's third argument (Go: `parity.ServerConfig`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// The external URL.
    pub external_url: String,
    /// The GVL ID.
    pub gvl_id: isize,
    /// The data center.
    #[serde(rename = "datacenter")]
    pub data_center: String,
}

/// Mirrors `adapters.ExtraRequestInfo` (Go: `parity.ExtraRequestInfo`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtraRequestInfo {
    /// The PBS entry point.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pbs_entry_point: String,
    /// The `Sec-GPC` header.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub global_privacy_control_header: String,
    /// The currency rates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency_rates: Option<CurrencyRates>,
    /// The preferred media type.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub preferred_media_type: String,
}

/// The conversions handed to the adapter, from currency to currency to rate
/// (Go: `parity.CurrencyRates`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CurrencyRates {
    /// The rates. A currency's map is nil-able like Go's `map[string]float64` (`None` is
    /// written as `null`).
    pub conversions: Option<BTreeMap<String, Option<BTreeMap<String, f64>>>>,
}

/// The body of `POST /__parity/adapter/{bidder}/requests` (Go: `parity.AdapterRequestsInput`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdapterRequestsInput {
    /// Always [`SCHEMA_ADAPTER_REQUESTS_INPUT`](crate::contracts::SCHEMA_ADAPTER_REQUESTS_INPUT).
    pub schema: String,
    /// The contract version.
    pub schema_version: String,
    /// The case ID.
    pub case_id: String,
    /// The fixture the case was generated from.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fixture_path: String,
    /// The adapter under test.
    pub build: AdapterBuild,
    /// The `openrtb2.BidRequest`.
    pub request: Option<Box<RawValue>>,
    /// The extra request info.
    pub extra_info: ExtraRequestInfo,
    /// The fake values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fakes: Option<FakeValues>,
}

/// Mirrors `adapters.RequestData` (Go: `parity.AdapterRequestData`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdapterRequestData {
    /// The method.
    pub method: String,
    /// The URI.
    pub uri: String,
    /// The headers.
    pub headers: HeaderList,
    /// The body.
    pub body: Option<Body>,
    /// The imp IDs.
    pub imp_ids: Option<Vec<String>>,
}

/// The response of the requests endpoint (Go: `parity.AdapterRequestsOutput`). `requests` and
/// `errors` are always present (empty arrays, never omitted).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdapterRequestsOutput {
    /// Always [`SCHEMA_ADAPTER_REQUESTS_OUTPUT`](crate::contracts::SCHEMA_ADAPTER_REQUESTS_OUTPUT).
    pub schema: String,
    /// The contract version.
    pub schema_version: String,
    /// The implementation.
    #[serde(rename = "impl")]
    pub implementation: String,
    /// The case ID.
    pub case_id: String,
    /// The bidder.
    pub bidder: String,
    /// The requests `MakeRequests` returned.
    pub requests: Option<Vec<AdapterRequestData>>,
    /// The errors `MakeRequests` returned.
    pub errors: Option<Vec<ErrorRecord>>,
}

/// Mirrors `adapters.ResponseData` (Go: `parity.AdapterHTTPResponse`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdapterHttpResponse {
    /// The status.
    pub status: isize,
    /// The headers.
    pub headers: HeaderList,
    /// The body.
    pub body: Option<Body>,
}

/// The body of `POST /__parity/adapter/{bidder}/bids` (Go: `parity.AdapterBidsInput`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdapterBidsInput {
    /// Always [`SCHEMA_ADAPTER_BIDS_INPUT`](crate::contracts::SCHEMA_ADAPTER_BIDS_INPUT).
    pub schema: String,
    /// The contract version.
    pub schema_version: String,
    /// The case ID.
    pub case_id: String,
    /// The fixture the case was generated from.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fixture_path: String,
    /// The adapter under test.
    pub build: AdapterBuild,
    /// The `openrtb2.BidRequest`.
    pub request: Option<Box<RawValue>>,
    /// The request `MakeBids` answers.
    pub request_data: AdapterRequestData,
    /// The bidder's response.
    pub response: AdapterHttpResponse,
    /// The extra request info.
    pub extra_info: ExtraRequestInfo,
    /// The fake values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fakes: Option<FakeValues>,
}

/// Mirrors `adapters.BidderResponse` (Go: `parity.AdapterBidderResponse`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdapterBidderResponse {
    /// The currency.
    pub currency: String,
    /// The bids.
    pub bids: Option<Vec<TypedBid>>,
    /// The fledge auction configs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fledge_auction_configs: Option<Box<RawValue>>,
}

/// The response of the bids endpoint (Go: `parity.AdapterBidsOutput`). `bidder_response` and
/// `errors` are always both present: Go's `MakeBids` returns bids and errors together.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdapterBidsOutput {
    /// Always [`SCHEMA_ADAPTER_BIDS_OUTPUT`](crate::contracts::SCHEMA_ADAPTER_BIDS_OUTPUT).
    pub schema: String,
    /// The contract version.
    pub schema_version: String,
    /// The implementation.
    #[serde(rename = "impl")]
    pub implementation: String,
    /// The case ID.
    pub case_id: String,
    /// The bidder.
    pub bidder: String,
    /// The response `MakeBids` returned.
    pub bidder_response: Option<AdapterBidderResponse>,
    /// The errors `MakeBids` returned.
    pub errors: Option<Vec<ErrorRecord>>,
}

/// The response of `GET /__parity/health` (Go: `parity.Health`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Health {
    /// Always [`SCHEMA_HEALTH`](crate::contracts::SCHEMA_HEALTH).
    pub schema: String,
    /// The contract version.
    pub schema_version: String,
    /// The implementation.
    #[serde(rename = "impl")]
    pub implementation: String,
    /// The binary.
    pub build: BuildInfo,
    /// The stages the implementation can capture.
    pub stages_supported: Option<Vec<StageId>>,
    /// The decision points the implementation reads.
    pub decisions_supported: Option<Vec<DecisionId>>,
    /// The endpoints the exec route dispatches.
    pub endpoints: Option<Vec<String>>,
    /// The mock modes.
    pub mock_modes: Option<Vec<String>>,
    /// The mock levels.
    pub mock_levels: Option<Vec<String>>,
    /// Whether the deterministic fakes are installed.
    pub fakes: bool,
}

/// The response of `GET /__parity/config` (Go: `parity.ConfigDump`). `config` is keyed by the Go
/// mapstructure config keys, without the adapters and parity subtrees.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigDump {
    /// Always [`SCHEMA_CONFIG_DUMP`](crate::contracts::SCHEMA_CONFIG_DUMP).
    pub schema: String,
    /// The contract version.
    pub schema_version: String,
    /// The implementation.
    #[serde(rename = "impl")]
    pub implementation: String,
    /// The resolved configuration.
    pub config: Option<Box<RawValue>>,
}

/// The response of `GET /__parity/bidders` (Go: `parity.BiddersDump`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BiddersDump {
    /// Always [`SCHEMA_BIDDERS_DUMP`](crate::contracts::SCHEMA_BIDDERS_DUMP).
    pub schema: String,
    /// The contract version.
    pub schema_version: String,
    /// The implementation.
    #[serde(rename = "impl")]
    pub implementation: String,
    /// The resolved bidder infos.
    pub bidders: Option<Box<RawValue>>,
    /// The active bidders.
    pub active: Option<Vec<String>>,
    /// The disabled bidders and their messages.
    pub disabled: Option<BTreeMap<String, String>>,
}

/// The response of `POST /__parity/reset` (Go: `parity.ResetResult`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResetResult {
    /// Always [`SCHEMA_RESET`](crate::contracts::SCHEMA_RESET).
    pub schema: String,
    /// The contract version.
    pub schema_version: String,
    /// The implementation.
    #[serde(rename = "impl")]
    pub implementation: String,
    /// The caches that were reset.
    pub reset: Option<Vec<String>>,
    /// The caches that could not be reset.
    pub not_reset: Option<Vec<NotResetRow>>,
}

/// A cache the server could not reset and why (Go: `parity.NotResetRow`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotResetRow {
    /// The cache.
    pub name: String,
    /// Why it was not reset.
    pub reason: String,
}
