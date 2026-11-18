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
//! - the seams product code calls ([`trace`], [`fakes`]): stage records, decision points, goroutine
//!   tracking, the deterministic clock, random and UUID sources, and the map orders.
//!
//! # The `trace` feature
//!
//! Parity mode is the Cargo feature `trace`, off by default. Product crates call the seams
//! unconditionally; a production build (without the feature) carries no parity code paths:
//!
//! - [`trace::current`] is a `const fn` returning `None`, and [`trace::Session`] is uninhabited, so
//!   `if let Some(s) = trace::current() { s.record(..) }` and [`trace::with`] compile to nothing
//!   and the recorded value is never built;
//! - the decision seams ([`trace::decide`], [`trace::intn`], ...) call their default closure
//!   directly;
//! - [`fakes::active`] is `None`, so [`fakes::now`], [`fakes::int`], [`fakes::map_keys`] and
//!   [`fakes::new_v4`] are the production clock, random source, map order and UUIDs.
//!
//! With the feature, the parity bootstrap installs the fakes (`fakes::install`) and the exec
//! handler runs each case inside `trace::begin_case`: a process-wide lock admits one case at a
//! time, publishes its [`trace::Session`], applies the envelope's fake values, and on
//! `trace::Case::end` unpublishes the session and resets the fakes, exactly like Go's
//! `trace.Begin`. (These items exist only with the feature.) The session records a trace the way
//! Go's does: stage values as the bytes `jsonutil.Marshal` writes, error lists that are `null`
//! until recorded and `[]` when recorded empty, outbound calls, decisions, analytics objects and
//! notes, with every registered stage reported `captured`, `not_reached` or as explicitly marked.
//!
//! # Observer-effect rule
//!
//! Hooks only snapshot values the code already built. They never rebuild or re-marshal live
//! request state for tracing purposes and never mutate state (Go: `parity/trace/session.go`).
//!
//! # Testing against Go
//!
//! The unit tests run in both modes (`cargo test -p pbs-parity` and
//! `cargo test -p pbs-parity --features trace`). The goldens under `testdata/golden` are written by
//! the Go program under `testdata/go`, built against the real `parity` packages of the
//! `parity-harness` branch (`testdata/go/run.sh` builds it against a temporary worktree): fake
//! streams, decision records, scripted sessions encoded as traces, envelope and body edge cases,
//! document round trips through the schema examples, error records and metric deltas, and the
//! Go schema validator's verdict on every document this crate produces
//! (`testdata/documents`).

pub mod adapterio;
pub mod body;
pub mod contracts;
pub mod envelope;
pub mod errorcodes;
pub mod fakes;
pub mod ids;
pub mod trace;
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
#[cfg(test)]
#[path = "golden_test.rs"]
mod golden_tests;
