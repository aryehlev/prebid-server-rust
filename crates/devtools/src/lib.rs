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
//! cargo run -p pbs-devtools -- typegen                   # Rust types for */typegen.json
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
//! Generation also refuses what it cannot follow: a call outside the statements of a function
//! `go test` runs (methods, function literals, helpers, `Testlower`), a call guarded by anything
//! but `err == nil` on the error its init statement binds, more than one call per test, a file
//! with a build constraint, and a construction that reads a package variable some function or
//! initializer of the package assigns or takes the address of. A `return` before the call is
//! setup. `testdata/manifest_synth.json` is the evidence: `testdata/go/manifestsynth` generates a
//! few hundred adapter tests, runs them under `go test` against a fake `adapterstest`, and records
//! what Go passed; the crate's tests require an exact match or a refusal for every one.
//! Regenerate it with `go run ./rust/crates/devtools/testdata/go/manifestsynth` from the
//! repository root.
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
//!   default, Go type and verified `PBS_*` variable (plus the un-prefixed fallback, such as
//!   `GDPR_DEFAULT_VALUE`, that a `BindEnv` made before `SetEnvPrefix` adds), and the `config.New`
//!   result.
//! - `bidderinfos`: `static/bidder-info` through `config.LoadBidderInfoFromDisk` and `config.New`.
//! - `biddernames`: the `openrtb_ext` bidder name constants, `CoreBidderNames()`, reserved-name
//!   predicates and the `NormalizeBidderName` table.
//!
//! The programs clear `PBS_*` variables (`configdefaults` also clears those un-prefixed
//! fallbacks), run viper from an empty directory, and route glog to stderr, which is shown only
//! when a dump fails.
//!
//! # `typegen`
//!
//! [`typegen`] generates the Rust types of Go packages (every struct, named type, constant and
//! alias, with `#[derive(GoJson)]` attributes carrying the Go names and JSON tags verbatim, and
//! Go's doc comments) from each crate's `typegen.json`, using a go/types describer under
//! `rust/crates/devtools/go/typegen`. It also writes the crate's Rust and Go type indexes for
//! its parity tests. See the module documentation for the type mapping and the overrides file.

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
pub mod typegen;
