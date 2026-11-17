//! The documents of the adapter endpoints and the introspection routes
//! (Go: `parity/adapterio.go`, `adapter_io.schema.json`, `introspection.schema.json`).

use std::collections::BTreeMap;

use pbs_gojson::{GoJson, RawJson};

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
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.AdapterBuild")]
pub struct AdapterBuild {
    /// The bidder.
    #[gojson(json = "bidder")]
    pub bidder: String,
    /// [`WRAP_RAW`] or [`WRAP_INFO_AWARE`].
    #[gojson(json = "wrap")]
    pub wrap: String,
    /// The builder's adapter config.
    #[gojson(json = "adapter_config,omitempty")]
    pub adapter_config: Option<AdapterConfig>,
    /// The builder's server config.
    #[gojson(json = "server,omitempty")]
    pub server: Option<ServerConfig>,
}

/// Mirrors `config.Adapter`, the builder's second argument (Go: `parity.AdapterConfig`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.AdapterConfig")]
pub struct AdapterConfig {
    /// The endpoint template.
    #[gojson(json = "endpoint")]
    pub endpoint: String,
    /// The extra info.
    #[gojson(json = "extra_info,omitempty")]
    pub extra_info: String,
    /// The platform ID.
    #[gojson(go = "PlatformID", json = "platform_id,omitempty")]
    pub platform_id: String,
    /// The app secret.
    #[gojson(json = "app_secret,omitempty")]
    pub app_secret: String,
    /// The XAPI settings.
    #[gojson(go = "XAPI", json = "xapi,omitempty")]
    pub xapi: Option<XapiConfig>,
}

/// Mirrors `config.AdapterXAPI` (Go: `parity.XAPIConfig`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.XAPIConfig")]
pub struct XapiConfig {
    /// The user name.
    #[gojson(json = "username,omitempty")]
    pub username: String,
    /// The password.
    #[gojson(json = "password,omitempty")]
    pub password: String,
    /// The tracker.
    #[gojson(json = "tracker,omitempty")]
    pub tracker: String,
}

/// Mirrors `config.Server`, the builder's third argument (Go: `parity.ServerConfig`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.ServerConfig")]
pub struct ServerConfig {
    /// The external URL.
    #[gojson(go = "ExternalURL", json = "external_url")]
    pub external_url: String,
    /// The GVL ID.
    #[gojson(go = "GvlID", json = "gvl_id")]
    pub gvl_id: isize,
    /// The data center.
    #[gojson(json = "datacenter")]
    pub data_center: String,
}

/// Mirrors `adapters.ExtraRequestInfo` (Go: `parity.ExtraRequestInfo`).
#[derive(Clone, Debug, Default, PartialEq, GoJson)]
#[gojson(go_type = "parity.ExtraRequestInfo")]
pub struct ExtraRequestInfo {
    /// The PBS entry point.
    #[gojson(json = "pbs_entry_point,omitempty")]
    pub pbs_entry_point: String,
    /// The `Sec-GPC` header.
    #[gojson(json = "global_privacy_control_header,omitempty")]
    pub global_privacy_control_header: String,
    /// The currency rates.
    #[gojson(json = "currency_rates,omitempty")]
    pub currency_rates: Option<CurrencyRates>,
    /// The preferred media type.
    #[gojson(json = "preferred_media_type,omitempty")]
    pub preferred_media_type: String,
}

/// The conversions handed to the adapter, from currency to currency to rate
/// (Go: `parity.CurrencyRates`).
#[derive(Clone, Debug, Default, PartialEq, GoJson)]
#[gojson(go_type = "parity.CurrencyRates")]
pub struct CurrencyRates {
    /// The rates.
    #[gojson(json = "conversions")]
    pub conversions: Option<BTreeMap<String, BTreeMap<String, f64>>>,
}

/// The body of `POST /__parity/adapter/{bidder}/requests` (Go: `parity.AdapterRequestsInput`).
#[derive(Clone, Debug, Default, PartialEq, GoJson)]
#[gojson(go_type = "parity.AdapterRequestsInput")]
pub struct AdapterRequestsInput {
    /// Always [`SCHEMA_ADAPTER_REQUESTS_INPUT`](crate::contracts::SCHEMA_ADAPTER_REQUESTS_INPUT).
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
    /// The adapter under test.
    #[gojson(json = "build")]
    pub build: AdapterBuild,
    /// The `openrtb2.BidRequest`.
    #[gojson(json = "request")]
    pub request: RawJson,
    /// The extra request info.
    #[gojson(json = "extra_info")]
    pub extra_info: ExtraRequestInfo,
    /// The fake values.
    #[gojson(json = "fakes,omitempty")]
    pub fakes: Option<FakeValues>,
}

/// Mirrors `adapters.RequestData` (Go: `parity.AdapterRequestData`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.AdapterRequestData")]
pub struct AdapterRequestData {
    /// The method.
    #[gojson(json = "method")]
    pub method: String,
    /// The URI.
    #[gojson(go = "URI", json = "uri")]
    pub uri: String,
    /// The headers.
    #[gojson(json = "headers")]
    pub headers: HeaderList,
    /// The body.
    #[gojson(json = "body")]
    pub body: Option<Body>,
    /// The imp IDs.
    #[gojson(go = "ImpIDs", json = "imp_ids")]
    pub imp_ids: Option<Vec<String>>,
}

/// The response of the requests endpoint (Go: `parity.AdapterRequestsOutput`). `requests` and
/// `errors` are always present (empty arrays, never omitted).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.AdapterRequestsOutput")]
pub struct AdapterRequestsOutput {
    /// Always [`SCHEMA_ADAPTER_REQUESTS_OUTPUT`](crate::contracts::SCHEMA_ADAPTER_REQUESTS_OUTPUT).
    #[gojson(json = "schema")]
    pub schema: String,
    /// The contract version.
    #[gojson(json = "schema_version")]
    pub schema_version: String,
    /// The implementation.
    #[gojson(go = "Impl", json = "impl")]
    pub implementation: String,
    /// The case ID.
    #[gojson(go = "CaseID", json = "case_id")]
    pub case_id: String,
    /// The bidder.
    #[gojson(json = "bidder")]
    pub bidder: String,
    /// The requests `MakeRequests` returned.
    #[gojson(json = "requests")]
    pub requests: Option<Vec<AdapterRequestData>>,
    /// The errors `MakeRequests` returned.
    #[gojson(json = "errors")]
    pub errors: Option<Vec<ErrorRecord>>,
}

/// Mirrors `adapters.ResponseData` (Go: `parity.AdapterHTTPResponse`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.AdapterHTTPResponse")]
pub struct AdapterHttpResponse {
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

/// The body of `POST /__parity/adapter/{bidder}/bids` (Go: `parity.AdapterBidsInput`).
#[derive(Clone, Debug, Default, PartialEq, GoJson)]
#[gojson(go_type = "parity.AdapterBidsInput")]
pub struct AdapterBidsInput {
    /// Always [`SCHEMA_ADAPTER_BIDS_INPUT`](crate::contracts::SCHEMA_ADAPTER_BIDS_INPUT).
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
    /// The adapter under test.
    #[gojson(json = "build")]
    pub build: AdapterBuild,
    /// The `openrtb2.BidRequest`.
    #[gojson(json = "request")]
    pub request: RawJson,
    /// The request `MakeBids` answers.
    #[gojson(json = "request_data")]
    pub request_data: AdapterRequestData,
    /// The bidder's response.
    #[gojson(json = "response")]
    pub response: AdapterHttpResponse,
    /// The extra request info.
    #[gojson(json = "extra_info")]
    pub extra_info: ExtraRequestInfo,
    /// The fake values.
    #[gojson(json = "fakes,omitempty")]
    pub fakes: Option<FakeValues>,
}

/// Mirrors `adapters.BidderResponse` (Go: `parity.AdapterBidderResponse`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.AdapterBidderResponse")]
pub struct AdapterBidderResponse {
    /// The currency.
    #[gojson(json = "currency")]
    pub currency: String,
    /// The bids.
    #[gojson(json = "bids")]
    pub bids: Option<Vec<TypedBid>>,
    /// The fledge auction configs.
    #[gojson(json = "fledge_auction_configs,omitempty")]
    pub fledge_auction_configs: RawJson,
}

/// The response of the bids endpoint (Go: `parity.AdapterBidsOutput`). `bidder_response` and
/// `errors` are always both present: Go's `MakeBids` returns bids and errors together.
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.AdapterBidsOutput")]
pub struct AdapterBidsOutput {
    /// Always [`SCHEMA_ADAPTER_BIDS_OUTPUT`](crate::contracts::SCHEMA_ADAPTER_BIDS_OUTPUT).
    #[gojson(json = "schema")]
    pub schema: String,
    /// The contract version.
    #[gojson(json = "schema_version")]
    pub schema_version: String,
    /// The implementation.
    #[gojson(go = "Impl", json = "impl")]
    pub implementation: String,
    /// The case ID.
    #[gojson(go = "CaseID", json = "case_id")]
    pub case_id: String,
    /// The bidder.
    #[gojson(json = "bidder")]
    pub bidder: String,
    /// The response `MakeBids` returned.
    #[gojson(json = "bidder_response")]
    pub bidder_response: Option<AdapterBidderResponse>,
    /// The errors `MakeBids` returned.
    #[gojson(json = "errors")]
    pub errors: Option<Vec<ErrorRecord>>,
}

/// The response of `GET /__parity/health` (Go: `parity.Health`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.Health")]
pub struct Health {
    /// Always [`SCHEMA_HEALTH`](crate::contracts::SCHEMA_HEALTH).
    #[gojson(json = "schema")]
    pub schema: String,
    /// The contract version.
    #[gojson(json = "schema_version")]
    pub schema_version: String,
    /// The implementation.
    #[gojson(go = "Impl", json = "impl")]
    pub implementation: String,
    /// The binary.
    #[gojson(json = "build")]
    pub build: BuildInfo,
    /// The stages the implementation can capture.
    #[gojson(json = "stages_supported")]
    pub stages_supported: Option<Vec<StageId>>,
    /// The decision points the implementation reads.
    #[gojson(json = "decisions_supported")]
    pub decisions_supported: Option<Vec<DecisionId>>,
    /// The endpoints the exec route dispatches.
    #[gojson(json = "endpoints")]
    pub endpoints: Option<Vec<String>>,
    /// The mock modes.
    #[gojson(json = "mock_modes")]
    pub mock_modes: Option<Vec<String>>,
    /// The mock levels.
    #[gojson(json = "mock_levels")]
    pub mock_levels: Option<Vec<String>>,
    /// Whether the deterministic fakes are installed.
    #[gojson(json = "fakes")]
    pub fakes: bool,
}

/// The response of `GET /__parity/config` (Go: `parity.ConfigDump`). `config` is keyed by the Go
/// mapstructure config keys, without the adapters and parity subtrees.
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.ConfigDump")]
pub struct ConfigDump {
    /// Always [`SCHEMA_CONFIG_DUMP`](crate::contracts::SCHEMA_CONFIG_DUMP).
    #[gojson(json = "schema")]
    pub schema: String,
    /// The contract version.
    #[gojson(json = "schema_version")]
    pub schema_version: String,
    /// The implementation.
    #[gojson(go = "Impl", json = "impl")]
    pub implementation: String,
    /// The resolved configuration.
    #[gojson(json = "config")]
    pub config: RawJson,
}

/// The response of `GET /__parity/bidders` (Go: `parity.BiddersDump`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.BiddersDump")]
pub struct BiddersDump {
    /// Always [`SCHEMA_BIDDERS_DUMP`](crate::contracts::SCHEMA_BIDDERS_DUMP).
    #[gojson(json = "schema")]
    pub schema: String,
    /// The contract version.
    #[gojson(json = "schema_version")]
    pub schema_version: String,
    /// The implementation.
    #[gojson(go = "Impl", json = "impl")]
    pub implementation: String,
    /// The resolved bidder infos.
    #[gojson(json = "bidders")]
    pub bidders: RawJson,
    /// The active bidders.
    #[gojson(json = "active")]
    pub active: Option<Vec<String>>,
    /// The disabled bidders and their messages.
    #[gojson(json = "disabled")]
    pub disabled: Option<BTreeMap<String, String>>,
}

/// The response of `POST /__parity/reset` (Go: `parity.ResetResult`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.ResetResult")]
pub struct ResetResult {
    /// Always [`SCHEMA_RESET`](crate::contracts::SCHEMA_RESET).
    #[gojson(json = "schema")]
    pub schema: String,
    /// The contract version.
    #[gojson(json = "schema_version")]
    pub schema_version: String,
    /// The implementation.
    #[gojson(go = "Impl", json = "impl")]
    pub implementation: String,
    /// The caches that were reset.
    #[gojson(json = "reset")]
    pub reset: Option<Vec<String>>,
    /// The caches that could not be reset.
    #[gojson(json = "not_reset")]
    pub not_reset: Option<Vec<NotResetRow>>,
}

/// A cache the server could not reset and why (Go: `parity.NotResetRow`).
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.NotResetRow")]
pub struct NotResetRow {
    /// The cache.
    #[gojson(json = "name")]
    pub name: String,
    /// Why it was not reset.
    #[gojson(json = "reason")]
    pub reason: String,
}
