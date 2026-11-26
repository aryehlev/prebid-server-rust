//! Parity of `pbs_openrtb_ext` with Go's `openrtb_ext` package.
//!
//! * `golden` replays, byte for byte, the differential goldens that `testdata/go` writes with the
//!   Go types: every Prebid extension object of prebid-server's test corpora (request, imp, bidder
//!   params, site, app, publisher, device, user, regs, source, response and bid ext, and video
//!   endpoint payloads) and generated mutations of every ported type, each decoded with
//!   `jsonutil.Unmarshal` and `json.Unmarshal` and re-encoded with `jsonutil.Marshal`,
//!   `json.Marshal` and an `Encoder` without HTML escaping.
//! * `serde_goldens` and `shapes` compare the serde code of the types with Go's on those goldens
//!   and on each field's shapes, every difference a ledger entry.
//! * `request_wrapper` replays the goldens `testdata/go/request_wrapper` writes with Go's
//!   `RequestWrapper` and the convert functions (`request_wrapper.go`, `convert_up.go`,
//!   `convert_down.go`) through the crate's public API, every difference a ledger entry.
//! * `functions` replays the results of the package's functions and methods over generated inputs.
//! * `inventory` checks every Go type, field, JSON tag, constant and function port against the Go
//!   build's metadata (`testdata/go_struct_meta.json`), typegen's Go type index and `typegen.json`.
//!
//! `index` lists the ported types, constants and functions. Regenerate the goldens with
//! `GOTOOLCHAIN=go1.26.2 go run .` in `crates/openrtb-ext/testdata/go` (and `go run
//! ./request_wrapper` there for the wrapper's); the corpora are read from the Go root
//! (`$PBS_GO_ROOT`, else `.upstream`, which `scripts/fetch-upstream.sh` makes). `go_quote` reads
//! the fields the generators quote.

#[cfg(test)]
pub mod functions;
#[cfg(test)]
#[path = "../common/go_quote.rs"]
pub mod go_quote;
#[cfg(test)]
pub mod golden;
#[cfg(test)]
pub mod index;
#[cfg(test)]
pub mod inventory;
#[cfg(test)]
pub mod request_wrapper;
#[cfg(test)]
pub mod serde_goldens;
#[cfg(test)]
pub mod shapes;
#[cfg(test)]
pub mod support;
