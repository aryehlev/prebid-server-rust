//! Parity-harness seams: stage traces, decision points, deterministic fakes and orders.
//!
//! The Go-vs-Rust parity harness (Go: the `parity` tree of the `parity-harness` branch) runs the
//! same case through both servers and compares everything each one observed. This crate is the
//! Rust side of its contract:
//!
//! - the identifiers and registries both servers share ([`ids`], [`contracts`]): the 45 stage IDs
//!   with their order, owning domain, keys and trace member, the 7 decision points, the outbound
//!   call kinds, the mock constants and the schema names, all checked against the registry JSON
//!   vendored under `contract/`;
//! - the documents the servers exchange with the runner, with Go's exact JSON ([`envelope`],
//!   [`tracedoc`], [`body`], [`adapterio`], [`errorcodes`]), encoded as the Go server writes them
//!   ([`encode_document`]);
//! - the deterministic clock, random and UUID sources and the map orders of parity mode
//!   ([`fakes`]).
//!
//! # The `trace` feature
//!
//! Parity mode is the Cargo feature `trace`, off by default. Product crates call the seams
//! unconditionally; without the feature [`fakes::active`] is a `const fn` returning `None`, so
//! [`fakes::now`], [`fakes::int`], [`fakes::map_keys`] and [`fakes::new_v4`] are the production
//! clock, random source, map order and UUIDs. With it, the parity bootstrap installs the fakes
//! (`fakes::install`, which exists only with the feature).

pub mod adapterio;
pub mod body;
pub mod contracts;
pub mod envelope;
pub mod errorcodes;
pub mod fakes;
pub mod ids;
pub mod tracedoc;

pub use self::{
    adapterio::{
        AdapterBidderResponse, AdapterBidsInput, AdapterBidsOutput, AdapterBuild, AdapterConfig,
        AdapterHttpResponse, AdapterRequestData, AdapterRequestsInput, AdapterRequestsOutput,
        BiddersDump, ConfigDump, CurrencyRates, ExtraRequestInfo, Health, NotResetRow, ResetResult,
        ServerConfig, XapiConfig,
    },
    body::{Body, BodyError},
    contracts::{Domain, encode_document, error_document},
    envelope::{
        BidderResponseMock, CaptureError, CaptureSpec, Envelope, ErrorSpec, ExecRequest,
        FakeValues, HeaderList, HeaderPair, InlineMocks, InlineResponse, InlineRule, MockSpec,
        TypedBid,
    },
    errorcodes::{error_codes, error_kind_for_code},
    ids::{DecisionDomain, DecisionId, KeyedBy, OutboundKind, StageId, StageStatus, TraceField},
    tracedoc::{
        AnalyticsRecord, BuildInfo, Decision, ErrorRecord, HttpRequest, HttpResponse, MetricDelta,
        MockRef, OutboundCall, OutboundMeta, OutboundRequest, OutboundResponse, StageEntry, Trace,
    },
};

// Public (in test builds only) so that its pub(crate) helpers are not flagged as redundant.
#[cfg(test)]
pub mod golden;
