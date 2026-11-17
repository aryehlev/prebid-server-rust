//! Go `golang.org/x/net/context/ctxhttp`: requests bound to a context, returning the context's
//! error (not a `*url.Error`) when the context is done.

use crate::{
    client::{Client, default_client},
    context::Context,
    error::HttpError,
    request::{Body, Request},
    response::Response,
};

/// Go: `ctxhttp.Do(ctx, client, req)` with both results (see [`Client::do_full`]).
pub async fn do_full(
    ctx: &Context,
    client: Option<&Client>,
    req: Request,
) -> (Option<Response>, Option<HttpError>) {
    let client = client.unwrap_or_else(|| default_client());
    let (resp, err) = client.do_full(req.with_context(ctx.clone())).await;
    let err = err.map(|e| ctx.err().map_or(e, HttpError::Context));
    (resp, err)
}

/// Go: `ctxhttp.Do(ctx, client, req)`: `client` `None` is `http.DefaultClient`.
///
/// # Errors
///
/// `context.Canceled` / `context.DeadlineExceeded` when `ctx` is done, otherwise the
/// [`Client::do_request`] error.
pub async fn do_request(
    ctx: &Context,
    client: Option<&Client>,
    req: Request,
) -> Result<Response, HttpError> {
    match do_full(ctx, client, req).await {
        (Some(resp), None) => Ok(resp),
        (_, Some(e)) => Err(e),
        (None, None) => Err(HttpError::Static("http: nil Response")),
    }
}

/// Go: `ctxhttp.Get(ctx, client, url)`.
///
/// # Errors
///
/// The `NewRequest` error, or as [`do_request`].
pub async fn get(ctx: &Context, client: Option<&Client>, url: &str) -> Result<Response, HttpError> {
    let req = Request::new("GET", url, Body::Nil)?;
    do_request(ctx, client, req).await
}

/// Go: `ctxhttp.Head(ctx, client, url)`.
///
/// # Errors
///
/// The `NewRequest` error, or as [`do_request`].
pub async fn head(
    ctx: &Context,
    client: Option<&Client>,
    url: &str,
) -> Result<Response, HttpError> {
    let req = Request::new("HEAD", url, Body::Nil)?;
    do_request(ctx, client, req).await
}

/// Go: `ctxhttp.Post(ctx, client, url, bodyType, body)`.
///
/// # Errors
///
/// The `NewRequest` error, or as [`do_request`].
pub async fn post(
    ctx: &Context,
    client: Option<&Client>,
    url: &str,
    body_type: &str,
    body: Body,
) -> Result<Response, HttpError> {
    let mut req = Request::new("POST", url, body)?;
    req.header.set("Content-Type", body_type);
    do_request(ctx, client, req).await
}
