//! Parity of `pbs_openrtb_ext` with Go's `openrtb_ext` package (but `request_wrapper.go` and the
//! convert files, which a separate port covers).
//!
//! * `golden` replays, byte for byte, the differential goldens that `testdata/go` writes with the
//!   Go types: every Prebid extension object of prebid-server's test corpora (request, imp, bidder
//!   params, site, app, publisher, device, user, regs, source, response and bid ext, and video
//!   endpoint payloads) and generated mutations of every ported type, each decoded with
//!   `jsonutil.Unmarshal` and `json.Unmarshal` and re-encoded with `jsonutil.Marshal`,
//!   `json.Marshal` and an `Encoder` without HTML escaping.
//! * `functions` replays the results of the package's functions and methods over generated inputs.
//! * `inventory` checks every Go type, field, JSON tag, constant and function port against the Go
//!   build's metadata (`testdata/go_struct_meta.json`), typegen's Go type index and `typegen.json`.
//!
//! `index` lists the ported types, constants and functions. Regenerate the goldens with
//! `GOTOOLCHAIN=go1.26.2 go run .` in `crates/openrtb-ext/testdata/go`; the corpora are read from
//! the Go root (`$PBS_GO_ROOT`, else `.upstream`, which `scripts/fetch-upstream.sh` makes).

#[cfg(test)]
pub mod functions;
#[cfg(test)]
pub mod golden;
#[cfg(test)]
pub mod index;
#[cfg(test)]
pub mod inventory;
#[cfg(test)]
pub mod serde_goldens;
#[cfg(test)]
pub mod shapes;
#[cfg(test)]
pub mod support;
