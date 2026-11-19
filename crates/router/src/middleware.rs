//! The wrappers around Prebid's public routes.
//!
//! `router.NoCache` (router/router.go:105-114) and `router.SupportCORS` (router/router.go:377-385),
//! which main.go nests as `NoCache{SupportCORS(router)}` (main.go:82-83).

use std::mem;

use axum::{
    Router,
    http::{HeaderName, HeaderValue, Method, header},
    middleware::map_response,
    response::Response,
};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// The headers `NoCache` adds to every answer, in the order it adds them
/// (router/router.go:110-112).
pub const NO_CACHE_HEADERS: [(HeaderName, HeaderValue); 3] = [
    (
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    ),
    (header::PRAGMA, HeaderValue::from_static("no-cache")),
    (header::EXPIRES, HeaderValue::from_static("0")),
];

/// `NoCache`'s headers on `response`.
///
/// Go adds them before the handler runs, so they come first, and a value the handler sets for one
/// of their names replaces `NoCache`'s, as `w.Header().Set` would. A [`map_response`] function:
/// `router.layer(map_response(no_cache))`.
pub async fn no_cache(mut response: Response) -> Response {
    let handler = mem::take(response.headers_mut());
    let headers = response.headers_mut();
    headers.extend(NO_CACHE_HEADERS);
    headers.extend(handler);
    response
}

/// `router.SupportCORS` on tower-http's `CorsLayer`.
///
/// rs/cors with credentials, every origin allowed (and so reflected), rs/cors' default methods
/// (GET, POST and HEAD) and the four request headers Prebid allows. The answers that differ from
/// rs/cors' are ledgered (`server.cors-preflight`, `server.cors-actual-requests`).
pub fn support_cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST, Method::HEAD])
        .allow_headers([
            header::ORIGIN,
            HeaderName::from_static("x-requested-with"),
            header::CONTENT_TYPE,
            header::ACCEPT,
        ])
}

/// `router` as main.go serves Prebid's router: `NoCache` around `SupportCORS` around it.
///
/// Like every [`Router::layer`], it wraps the routes and the fallback `router` already has, so it
/// comes after the last route.
pub fn public<S: Clone + Send + Sync + 'static>(router: Router<S>) -> Router<S> {
    router.layer(support_cors()).layer(map_response(no_cache))
}
