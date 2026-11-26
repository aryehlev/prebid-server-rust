//! Glue (ruling 10 (b), D-FASTGZIP): response compression as server/server.go sets it up,
//! deciding as gziphandler decides and compressing with fastgzip.
//!
//! `getCompressionEnabledHandler` (server/server.go:120-125) wraps the main and Unix-socket
//! handlers in `gziphandler.GzipHandler` when `compression.response.enable_gzip` is set. Here that
//! is a layer of pbs-server's own over fastgzip in place of tower-http's, which decides as
//! gziphandler v1.1.1 decides:
//!
//! - every answer gets `Vary: Accept-Encoding`, ahead of the Vary values the handler added, as
//!   gziphandler adds it before the handler runs (`server.vary`);
//! - a request gets gzip when its first Accept-Encoding value gives gzip a q above 0, read as
//!   gziphandler reads it (`server.gzip-negotiation`), its q values by std's float parser
//!   (`server.gzip-qvalue-syntax`);
//! - an answer is sent as it is when the handler set a Content-Encoding, when its Content-Length
//!   (as `strconv.Atoi` reads it, 0 where it does not parse) is neither 0 nor at least
//!   [`MIN_SIZE`], when its body is empty, or when it has no such Content-Length and its body is
//!   shorter than [`MIN_SIZE`];
//! - otherwise it is gzipped whatever its type and status: Content-Encoding is set to `gzip`,
//!   Content-Length is dropped and comes back as the compressed length, and Accept-Ranges and
//!   Content-Range stay, so a range answer is the range gzipped (`server.gzip-ranges`).
//!
//! gziphandler gzips as the handler writes; this layer buffers the body and compresses it in one
//! call. A body in memory (axum's bodies of an exact size, which is what Prebid's handlers
//! answer with) is compressed whole. A streamed body (`ServeDir`'s files under /static) is read
//! up to [`STREAM_LIMIT`] bytes; one that goes past it is sent on uncompressed, the bytes already
//! read first, so no answer is held in memory past that size (`server.gzip-streams`). A body the
//! layer reads loses its HTTP trailers, which no Prebid handler sends (Go's set none).
//!
//! The bytes are fastgzip's level 0, its default: consecutive answers of one kind share their
//! Huffman codes, so the output depends on the answers compressed before it, and it differs from
//! compress/gzip's (`server.gzip-bytes`). Each worker thread keeps one compressor per kind of
//! payload (JSON, other text, the rest), used on the worker inside the request's task and never
//! on a blocking pool; a payload past [`SHARED_LIMIT`] bytes gets a compressor of its own, so a
//! thread's compressors keep at most three output buffers of about twice that size. Each call
//! compresses and copies the bytes out inside one `with_borrow_mut`, with no `.await`, so a task
//! never holds a compressor across a yield and never meets another task's borrow.

mod negotiation;

use std::{cell::RefCell, thread::LocalKey};

use axum::{
    Router,
    body::{Body, Bytes, HttpBody as _},
    extract::Request,
    http::{HeaderMap, HeaderValue, header},
    middleware::{self, Next},
    response::Response,
};
use fastgzip::Gzip;
use futures_util::{StreamExt as _, stream};

/// gziphandler's `DefaultMinSize` (gzip.go v1.1.1), which Prebid keeps: an answer of fewer bytes
/// is sent as it is.
pub const MIN_SIZE: u16 = 1400;

/// How much of a streamed body is read to be compressed. One that goes past it is sent on
/// uncompressed; the largest file under Prebid's static/ is 19,572 bytes.
pub const STREAM_LIMIT: usize = 1 << 20;

/// The longest payload a worker thread's own compressors are given.
///
/// A longer one gets a compressor of its own, so no output buffer a thread keeps grows past
/// about twice this size (the buffer's capacity doubles as it grows).
pub const SHARED_LIMIT: usize = 1 << 20;

/// `getCompressionEnabledHandler`: `router` behind gziphandler's compression when `enable_gzip`
/// is set.
///
/// `enable_gzip` is the value of `compression.response.enable_gzip`; without it `router` is
/// served as it is.
// Go: getCompressionEnabledHandler (server/server.go:120-125) and gziphandler.GzipHandler
pub fn compression_enabled(router: Router, enable_gzip: bool) -> Router {
    if enable_gzip {
        router.layer(middleware::from_fn(gzip))
    } else {
        router
    }
}

/// gziphandler's handler: Vary on every answer, and gzip for the answers it compresses.
// Go: GzipHandlerWithOpts's handler (gziphandler v1.1.1 gzip.go:320-344)
async fn gzip(request: Request, next: Next) -> Response {
    let accepts = negotiation::accepts_gzip(
        request
            .headers()
            .get(header::ACCEPT_ENCODING)
            .map(HeaderValue::as_bytes),
    );
    answer(accepts, next.run(request).await).await
}

/// The handler's `response` as gziphandler sends it to a request that does or does not accept
/// gzip.
async fn answer(accepts: bool, mut response: Response) -> Response {
    vary_first(response.headers_mut());
    if accepts {
        compress(response).await
    } else {
        response
    }
}

/// Puts `Vary: Accept-Encoding` ahead of the Vary values the handler added, where gziphandler's
/// `Header().Add` before the handler runs leaves it.
// Go: w.Header().Add(vary, acceptEncoding) (gziphandler v1.1.1 gzip.go:324)
fn vary_first(headers: &mut HeaderMap) {
    let added: Vec<HeaderValue> = headers.get_all(header::VARY).iter().cloned().collect();
    drop(headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding")));
    for value in added {
        let _: bool = headers.append(header::VARY, value);
    }
}

/// `GzipResponseWriter`'s decision, and its gzip, on the whole body.
// Go: GzipResponseWriter.Write, startGzip, startPlain and Close (gziphandler v1.1.1
// gzip.go:100-206, 226-245)
async fn compress(response: Response) -> Response {
    let headers = response.headers();
    let encoded = headers
        .get(header::CONTENT_ENCODING)
        .is_some_and(|value| !value.as_bytes().is_empty());
    let length = headers
        .get(header::CONTENT_LENGTH)
        .map_or(0, |value| atoi(value.as_bytes()));
    if encoded || (length != 0 && length < i64::from(MIN_SIZE)) {
        return response;
    }
    let size = response.body().size_hint();
    // Go: an answer the handler writes nothing to is sent as it is, and one with no
    // Content-Length waits for MIN_SIZE bytes.
    if size.upper() == Some(0)
        || (length == 0 && size.upper().is_some_and(|n| n < u64::from(MIN_SIZE)))
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let payload = match read(body, size.exact().is_some()).await {
        Ok(payload) => payload,
        Err(body) => return Response::from_parts(parts, body),
    };
    if payload.is_empty() || (length == 0 && payload.len() < usize::from(MIN_SIZE)) {
        return Response::from_parts(parts, Body::from(payload));
    }
    let gzipped = gzip_payload(Kind::of(&parts.headers), &payload);
    drop(
        parts
            .headers
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip")),
    );
    // Go: the handler's length is not the gzip's. The new body's exact size gives axum the
    // compressed one.
    drop(parts.headers.remove(header::CONTENT_LENGTH));
    Response::from_parts(parts, Body::from(gzipped))
}

/// The body as one payload: all of it when `whole` (a body in memory), up to [`STREAM_LIMIT`]
/// bytes otherwise. A body that goes past the limit, or fails, comes back as a body that sends
/// what was read and then the rest, or the error.
async fn read(body: Body, whole: bool) -> Result<Bytes, Body> {
    let mut rest = body.into_data_stream();
    let mut chunks: Vec<Bytes> = Vec::new();
    let mut total = 0_usize;
    while let Some(next) = rest.next().await {
        match next {
            Ok(chunk) => {
                total = total.saturating_add(chunk.len());
                chunks.push(chunk);
                if !whole && total > STREAM_LIMIT {
                    let read = stream::iter(chunks.into_iter().map(Ok));
                    return Err(Body::from_stream(read.chain(rest)));
                }
            }
            Err(error) => {
                let read = stream::iter(chunks.into_iter().map(Ok));
                let failed = stream::once(async { Err::<Bytes, _>(error) });
                return Err(Body::from_stream(read.chain(failed)));
            }
        }
    }
    Ok(match <[Bytes; 1]>::try_from(chunks) {
        Ok([chunk]) => chunk,
        Err(chunks) => Bytes::from(chunks.concat()),
    })
}

/// The kinds of payload a worker thread keeps a compressor for: answers of one kind share their
/// Huffman codes, which fit another kind poorly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `application/json` and `+json` types: the auction, AMP, video, cookie-sync and info
    /// answers.
    Json,
    /// Other text: `text/*`, JavaScript, XML and YAML, as under /static.
    Text,
    /// Everything else, and answers with no Content-Type.
    Other,
}

impl Kind {
    /// The kind of an answer with `headers`, by its Content-Type's media type.
    fn of(headers: &HeaderMap) -> Self {
        let media = headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if media == "application/json" || media.ends_with("+json") {
            Self::Json
        } else if media.starts_with("text/")
            || media.ends_with("javascript")
            || media.ends_with("xml")
            || media.ends_with("yaml")
        {
            Self::Text
        } else {
            Self::Other
        }
    }

    /// This worker thread's compressor for the kind.
    const fn local(self) -> &'static LocalKey<RefCell<Gzip>> {
        match self {
            Self::Json => &JSON,
            Self::Text => &TEXT,
            Self::Other => &OTHER,
        }
    }
}

thread_local! {
    /// The worker thread's compressor for [`Kind::Json`].
    static JSON: RefCell<Gzip> = RefCell::new(Gzip::new());
    /// The worker thread's compressor for [`Kind::Text`].
    static TEXT: RefCell<Gzip> = RefCell::new(Gzip::new());
    /// The worker thread's compressor for [`Kind::Other`].
    static OTHER: RefCell<Gzip> = RefCell::new(Gzip::new());
}

/// `payload` gzipped by this thread's compressor for `kind`, or by one of its own past
/// [`SHARED_LIMIT`] bytes.
fn gzip_payload(kind: Kind, payload: &[u8]) -> Bytes {
    if payload.len() > SHARED_LIMIT {
        return Bytes::copy_from_slice(Gzip::new().compress(payload));
    }
    kind.local()
        .with_borrow_mut(|gz| Bytes::copy_from_slice(gz.compress(payload)))
}

/// Go: `strconv.Atoi(s)` with its error dropped, as gziphandler reads Content-Length: an
/// optional sign and decimal digits, 0 for anything else, and the nearest `int64` past its
/// range.
// Go: Atoi and ParseInt (internal/strconv/atoi.go)
fn atoi(s: &[u8]) -> i64 {
    let (negative, digits) = match s {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return 0;
    }
    digits.iter().fold(0_i64, |n, c| {
        let digit = i64::from(c - b'0');
        if negative {
            n.saturating_mul(10).saturating_sub(digit)
        } else {
            n.saturating_mul(10).saturating_add(digit)
        }
    })
}

#[cfg(test)]
mod tests;
