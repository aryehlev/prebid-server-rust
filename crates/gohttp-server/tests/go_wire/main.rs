//! Differential test of the wire bytes: replays every raw request of
//! `testdata/golden/wire.golden` against pbs-gohttp-server and compares the reply, byte for byte,
//! with what Go's `net/http` server answered with `julienschmidt/httprouter` and `rs/cors`
//! wrapped the way Prebid Server wraps them.
//!
//! `testdata/go/wire` (run with `GOTOOLCHAIN=go1.26.2`) wrote the golden from the real Go code:
//! Prebid's `router.NoCache`, `router.SupportCORS`, `endpoints.NewStatusEndpoint` and
//! `endpoints.NewVersionEndpoint`, a route table copied from `router/router.go` and
//! `router/admin.go`, and the listener settings of `server/server.go`. `handlers.rs` mirrors its
//! handlers one for one. `Date` values and multipart boundaries are normalized on both sides;
//! everything else must match, including how the connection ended.
//!
//! Both goldens record the GOOS the generator ran on (`goos`). A case whose bytes depend on the
//! GOOS (the `RemoteAddr` of a Unix-socket client: `""` on darwin, `"@"` on linux) has one
//! `expect` block per GOOS: the generator observed the block of the GOOS it ran on and derived
//! the others from Go's source (`testdata/go/wire/goos.go`), and the wire module's Go test checks
//! the block of the GOOS it runs on against real Go. The test takes the block of the OS it runs
//! on and fails on an OS such a case has no block for.
//!
//! `testdata/golden/shutdown.golden`, from the same program, holds the shutdown scenarios: what
//! idle, active, pipelined, half-read and new connections see while `Server.Shutdown` drains
//! the server (including connections that have been silent for over 5 seconds), whether later
//! dials are refused, what `Serve` and `Shutdown` return (also when the shutdown deadline
//! passes), whether a Unix socket file is removed, and the connection metrics Prebid's
//! `monitorableListener` records.

#[cfg(test)]
mod tests;
