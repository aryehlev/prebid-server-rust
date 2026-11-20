//! Prebid Server's public router on axum.
//!
//! The Go server routes with httprouter v1.3.0 and serves its public routes inside two wrappers,
//! `router.NoCache` around `router.SupportCORS` (main.go:82-83, router/router.go:105-114 and
//! 377-385). Here the routes are axum's (D-ROUTER: no httprouter emulation; its 404, 405 and
//! `Allow`, OPTIONS, HEAD, redirect and decoded-path edges are ledgered and pinned against Go's
//! answers in `tests/routing.rs`), and the wrappers are tower layers in [`middleware`]:
//! [`middleware::public`] wraps a router as main.go wraps Prebid's.
//!
//! ROUTER-MAIN registers Prebid's routes and wraps them with [`middleware::public`]; pbs-server
//! serves the result.

pub mod middleware;
