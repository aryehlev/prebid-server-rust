//! One request on a pooled connection, as reqwest and hyper write it over their own HTTP/2: the
//! request's head (reqwest's default `Accept`, tower-http's `Accept-Encoding: gzip`, hyper's
//! `Content-Length` rule and the connection-specific fields it strips), and the response's body
//! (hyper's reading of the stream and tower-http's gzip decompression, with reqwest's error
//! texts).

use std::{
    error::Error,
    fmt,
    future::poll_fn,
    io,
    pin::Pin,
    task::{Context, Poll, ready},
};

use async_compression::tokio::bufread::GzipDecoder;
use bytes::{Bytes, BytesMut};
use futures_util::Stream;
use h2::{Reason, RecvStream};
use reqwest::{
    Method, Url, Version,
    header::{
        ACCEPT, ACCEPT_ENCODING, CONNECTION, CONTENT_ENCODING, CONTENT_LENGTH, HeaderMap,
        HeaderName, HeaderValue, TE, TRANSFER_ENCODING, UPGRADE,
    },
};
use tokio_util::io::{StreamReader, poll_read_buf};

use super::Slot;
use crate::error::{ErrorKind, RequestError};

/// The request of one hop: what reqwest hands its connection, the client's default fields added.
#[derive(Debug, Clone)]
pub(in super::super) struct Hop {
    pub(in super::super) method: Method,
    pub(in super::super) url: Url,
    pub(in super::super) headers: HeaderMap,
    /// The body, which the pool sends only when it is in memory, as every request body of the
    /// crate's callers is.
    pub(in super::super) body: Option<Bytes>,
}

impl Hop {
    /// The hop of a request with `method`, `url`, `headers` and `body`, with the fields reqwest's
    /// client adds when the caller set none: its default `Accept: */*`, and tower-http's
    /// `Accept-Encoding: gzip`, which reqwest's gzip asks for.
    pub(in super::super) fn new(
        method: Method,
        url: Url,
        mut headers: HeaderMap,
        body: Option<Bytes>,
    ) -> Self {
        for (name, value) in [(ACCEPT, "*/*"), (ACCEPT_ENCODING, "gzip")] {
            if !headers.contains_key(&name) {
                drop(headers.insert(name, HeaderValue::from_static(value)));
            }
        }
        Self {
            method,
            url,
            headers,
            body,
        }
    }

    /// The request's head as hyper's h2 client sends it, and whether it ends the stream (no
    /// body).
    ///
    /// # Errors
    ///
    /// The URL is not one HTTP/2 can name.
    pub(super) fn head(&self) -> Result<(http::Request<()>, bool), http::Error> {
        let mut target = self.url.clone();
        target.set_fragment(None);
        let uri: http::Uri = target.as_str().parse()?;
        let mut head = http::Request::new(());
        head.method_mut().clone_from(&self.method);
        *head.uri_mut() = uri;
        *head.version_mut() = Version::HTTP_2;
        let fields = head.headers_mut();
        fields.clone_from(&self.headers);
        strip_connection_fields(fields);
        let length = self.body.as_ref().map_or(0, Bytes::len);
        // hyper: a body of known length gets Content-Length when it is not empty or the method
        // gives a body a meaning.
        let defined = !matches!(
            self.method,
            Method::GET | Method::HEAD | Method::DELETE | Method::CONNECT
        );
        if (length != 0 || defined) && !fields.contains_key(CONTENT_LENGTH) {
            drop(fields.insert(CONTENT_LENGTH, HeaderValue::from(length)));
        }
        Ok((head, length == 0))
    }
}

/// hyper's `strip_connection_headers` for a request: the fields HTTP/2 forbids (RFC 9113 section
/// 8.2.2), `TE` but for `trailers`, and every field `Connection` names.
fn strip_connection_fields(fields: &mut HeaderMap) {
    let forbidden = [
        HeaderName::from_static("keep-alive"),
        HeaderName::from_static("proxy-connection"),
        TRANSFER_ENCODING,
        UPGRADE,
    ];
    for name in forbidden {
        drop(fields.remove(name));
    }
    if fields.get(TE).is_some_and(|te| te != "trailers") {
        drop(fields.remove(TE));
    }
    if let Some(connection) = fields.remove(CONNECTION)
        && let Ok(names) = connection.to_str()
    {
        for name in names.split(',') {
            drop(fields.remove(name.trim()));
        }
    }
}

/// A pooled response's body, read as reqwest reads its own HTTP/2 bodies: hyper's body of the
/// stream (each part's window given back; a reset with `NO_ERROR` ends it, as RFC 9113 section
/// 8.1 lets an early response end), through tower-http's gzip decompression when the response
/// says `Content-Encoding: gzip`; and the connection's stream slot, given back once the body
/// ends or fails.
pub(in super::super) struct Body {
    read: Read,
    slot: Option<Slot>,
    ended: bool,
}

/// How a body is read.
enum Read {
    Plain(Frames),
    Gzip(Box<Gzip>),
}

/// tower-http's `WrapBody` over async-compression's gzip decoder.
struct Gzip {
    decoder: GzipDecoder<StreamReader<Frames, Bytes>>,
    buf: BytesMut,
    /// The decoder has ended.
    read_all: bool,
}

/// The size tower-http reserves for each part it decodes.
const GZIP_BUF: usize = 4096;

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Body")
            .field("gzip", &matches!(self.read, Read::Gzip(_)))
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

impl Body {
    /// The body of a response whose fields are `headers`, on the stream `stream` in `slot`. A
    /// response whose first `Content-Encoding` is `gzip` is decompressed, and loses its
    /// `Content-Encoding` and `Content-Length`, as tower-http's decompression does.
    pub(super) fn new(stream: RecvStream, headers: &mut HeaderMap, slot: Slot) -> Self {
        let frames = Frames {
            stream,
            error: None,
            read_some: false,
            data_done: false,
            done: false,
        };
        let gzip = headers
            .get(CONTENT_ENCODING)
            .is_some_and(|encoding| encoding.as_bytes() == b"gzip");
        let read = if gzip {
            drop(headers.remove(CONTENT_ENCODING));
            drop(headers.remove(CONTENT_LENGTH));
            Read::Gzip(Box::new(Gzip {
                decoder: GzipDecoder::new(StreamReader::new(frames)),
                buf: BytesMut::new(),
                read_all: false,
            }))
        } else {
            Read::Plain(frames)
        };
        Self {
            read,
            slot: Some(slot),
            ended: false,
        }
    }

    /// The next part of the body, `None` at its end.
    ///
    /// # Errors
    ///
    /// The stream failed, or a gzip body did not decompress ([`ErrorKind::Body`], with reqwest's
    /// texts).
    pub(in super::super) async fn chunk(&mut self) -> Result<Option<Bytes>, RequestError> {
        loop {
            if self.ended {
                return Ok(None);
            }
            let next = poll_fn(|cx| match &mut self.read {
                Read::Plain(frames) => frames.poll_frame(cx),
                Read::Gzip(gzip) => gzip.poll_part(cx),
            })
            .await;
            match next {
                Some(Ok(part)) if part.is_empty() => {}
                Some(Ok(part)) => return Ok(Some(part)),
                Some(Err(error)) => {
                    self.end();
                    return Err(error);
                }
                None => {
                    self.end();
                    return Ok(None);
                }
            }
        }
    }

    /// Ends the body, giving its slot back.
    fn end(&mut self) {
        self.ended = true;
        self.slot = None;
    }
}

impl Gzip {
    /// tower-http's `WrapBody::poll_frame`: the next decompressed part; once the decoder has
    /// ended, more data on the stream is an error. A decoding error before the stream gave any
    /// data ends the body without one.
    fn poll_part(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Bytes, RequestError>>> {
        if !self.read_all {
            if self.buf.capacity() == 0 {
                self.buf.reserve(GZIP_BUF);
            }
            match ready!(poll_read_buf(
                Pin::new(&mut self.decoder),
                cx,
                &mut self.buf
            )) {
                Ok(0) => self.read_all = true,
                Ok(_) => return Poll::Ready(Some(Ok(self.buf.split().freeze()))),
                Err(error) => {
                    let frames = self.decoder.get_mut().get_mut();
                    if let Some(failed) = frames.error.take() {
                        return Poll::Ready(Some(Err(failed)));
                    }
                    if frames.read_some {
                        return Poll::Ready(Some(Err(RequestError::chained(
                            ErrorKind::Body,
                            DECODING,
                            error,
                        ))));
                    }
                }
            }
        }
        let frames = self.decoder.get_mut().get_mut();
        Poll::Ready(match ready!(frames.poll_frame(cx)) {
            Some(Ok(data)) if !data.is_empty() => Some(Err(RequestError::chained(
                ErrorKind::Body,
                DECODING,
                ExtraBytes,
            ))),
            Some(Ok(_)) | None => None,
            Some(Err(error)) => Some(Err(error)),
        })
    }
}

/// reqwest's text for every error reading a response body (`error::decode`, as its `chunk` maps
/// the body's errors).
const DECODING: &str = "error decoding response body";

/// tower-http's error for data after the end of a decompressed body.
#[derive(Debug)]
struct ExtraBytes;

impl fmt::Display for ExtraBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("there are extra bytes after body has been decompressed")
    }
}

impl Error for ExtraBytes {}

/// hyper's body of an h2 stream, which tower-http's decompression also reads as a stream of
/// data, keeping the stream's error aside while the decoder sees an I/O error.
struct Frames {
    stream: RecvStream,
    /// The stream's error, which the decoder read as an I/O error.
    error: Option<RequestError>,
    /// The stream gave data (tower-http's `read_some_data`).
    read_some: bool,
    /// The data has ended: the trailers come next.
    data_done: bool,
    /// The body has ended.
    done: bool,
}

impl Frames {
    /// hyper's `Incoming::poll_frame` for an h2 stream, its trailers left out as reqwest's
    /// `chunk` leaves them.
    fn poll_frame(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Bytes, RequestError>>> {
        if self.done {
            return Poll::Ready(None);
        }
        if !self.data_done {
            match ready!(self.stream.poll_data(cx)) {
                Some(Ok(data)) => {
                    drop(self.stream.flow_control().release_capacity(data.len()));
                    return Poll::Ready(Some(Ok(data)));
                }
                Some(Err(error)) => {
                    self.done = true;
                    return Poll::Ready(early_end(error, "error reading a body from connection"));
                }
                None => self.data_done = true,
            }
        }
        let trailers = ready!(self.stream.poll_trailers(cx));
        self.done = true;
        Poll::Ready(
            trailers
                .err()
                .and_then(|error| early_end(error, "http2 error")),
        )
    }
}

/// hyper's reading of a stream's `error`: a reset with `NO_ERROR` ends the body, anything else
/// fails it with hyper's `kind` text, inside reqwest's.
fn early_end(error: h2::Error, kind: &str) -> Option<Result<Bytes, RequestError>> {
    (error.reason() != Some(Reason::NO_ERROR)).then(|| {
        Err(RequestError::chained(
            ErrorKind::Body,
            &format!("{DECODING}: {kind}"),
            error,
        ))
    })
}

impl Stream for Frames {
    type Item = io::Result<Bytes>;

    /// tower-http's `StreamErrorIntoIoError` over `BodyIntoStream`.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        Poll::Ready(match ready!(this.poll_frame(cx)) {
            Some(Ok(data)) => {
                this.read_some = true;
                Some(Ok(data))
            }
            Some(Err(error)) => {
                this.error = Some(error);
                Some(Err(io::Error::other("the response stream failed")))
            }
            None => None,
        })
    }
}
