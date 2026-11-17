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
//! `testdata/golden/shutdown.golden`, from the same program, holds the shutdown scenarios: what
//! idle, active, pipelined, half-read and new connections see while `Server.Shutdown` drains
//! the server, whether later dials are refused, what `Serve` and `Shutdown` return (also when
//! the shutdown deadline passes), and whether a Unix socket file is removed.

#[cfg(test)]
mod tests;
