//! Developer tools for the Prebid Server Rust port: they extract ground truth from the Go code at
//! the repository root into JSON goldens under `rust/testdata/`, which the Rust ports replay.
//!
//! # Commands
//!
//! Run from anywhere inside the repository (`--root DIR` or `PBS_GO_ROOT` override discovery):
//!
//! ```text
//! cargo run -p pbs-devtools -- adapter-manifest          # rust/testdata/go_fixture_manifest.json
//! cargo run -p pbs-devtools -- go-dump struct-meta       # rust/testdata/go_struct_meta.json
//! cargo run -p pbs-devtools -- go-dump config-defaults   # rust/testdata/go_config_defaults.json
//! cargo run -p pbs-devtools -- go-dump bidder-infos      # rust/testdata/go_bidder_infos.json
//! cargo run -p pbs-devtools -- go-dump bidder-names      # rust/testdata/go_bidder_names.json
//! cargo run -p pbs-devtools -- go-dump all               # all four Go dumps
//! cargo run -p pbs-devtools -- all                       # everything
//! cargo run -p pbs-devtools -- all --check               # fail if any committed file is stale
//! ```
//!
//! Every file records `go_commit`: `--go-commit`, else `PBS_GO_COMMIT`, else the last commit that
//! touched `*.go`, `go.mod`, `go.sum`, `static/` or `adapters/` outside `rust/` (a shallow clone is
//! rejected rather than guessed). `--check` regenerates in memory and fails, naming each stale
//! file, when the committed copy differs; the daily Go-sync job runs `all --check`.
//!
//! # `adapter-manifest`
//!
//! [`manifest`] reads `adapters/*/*_test.go` with a Go tokenizer ([`golex`]) and a small parser
//! ([`goparse`]) and records, for each `adapterstest.RunJSONBidderTest(t, dir, bidder)` call, the
//! fixture root, the bidder name and the `config.Adapter`/`config.Server` passed to `Builder`
//! (following locals, `if bidder, err := Builder(...); err == nil { ... }` init statements, helper
//! functions, package constants and `+` concatenation), or the struct literal for adapters built
//! directly. Statements that alter the bidder after construction must be matched by an entry in
//! the hand-maintained overlay `rust/testdata/go_fixture_hooks.json` ([`hooks`]), which also pins
//! the expected totals (282 runs and the per-kind fixture counts at the time of writing). An
//! unresolved run, an unmatched statement, a changed test file or a changed total fails
//! generation with a message saying what to review.
//!
//! The typed [`manifest::Manifest`] can be deserialized by other crates' test harnesses.
//!
//! # `go-dump`
//!
//! [`godump`] runs Go programs that live in the repository's Go module (so they can import
//! prebid-server packages) under `rust/crates/devtools/go/`:
//!
//! - `structmeta`: reflect metadata for every named type in openrtb2, openrtb3, native1 (and its
//!   request/response packages), adcom1, `openrtb_ext`, adapters (and every adapter package,
//!   unexported types included) and config, plus exported typed constants. Go cannot enumerate
//!   types at run time, so a `-plan` step writes `go build -overlay` files that register each
//!   declaration from inside its own package.
//! - `configdefaults`: `config.SetupViper` exactly as `main.go` calls it, every key with its
//!   default, Go type and verified `PBS_*` variable, and the `config.New` result.
//! - `bidderinfos`: `static/bidder-info` through `config.LoadBidderInfoFromDisk` and `config.New`.
//! - `biddernames`: the `openrtb_ext` bidder name constants, `CoreBidderNames()`, reserved-name
//!   predicates and the `NormalizeBidderName` table.
//!
//! The programs clear `PBS_*` variables, run viper from an empty directory, and route glog to
//! stderr, which is shown only when a dump fails.

pub mod cli;
pub mod error;
pub mod fixtures;
pub mod godump;
#[cfg(test)]
mod goldens;
pub mod golex;
pub mod goparse;
pub mod hooks;
pub mod manifest;
pub mod repo;
#[cfg(test)]
pub mod testutil;
