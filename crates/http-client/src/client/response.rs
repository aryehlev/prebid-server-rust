//! A response whose head has arrived, and its body, read under the request's deadline as Go's
//! readers read it.

use bytes::{Bytes, BytesMut};
use reqwest::{StatusCode, Url, Version, header::HeaderMap};
use tokio::{sync::OwnedSemaphorePermit, time::Instant};

use super::{pool::Body, until};
use crate::error::{READING_BODY, RequestError};

/// A response whose head has arrived: one reqwest received, or one a
/// [`Transport`](crate::Transport) made ([`Response::new`]). It holds its host's
/// `max_connections_per_host` slot until its body is read or it is dropped.
#[derive(Debug)]
pub struct Response {
    source: Source,
    /// The error the body ends with in place of its end.
    end_error: Option<RequestError>,
    /// When the body must be read by: the request's deadline, or its client's timeout when that
    /// is earlier.
    deadline: Option<Instant>,
    /// The client's timeout, when it is earlier than the request's deadline: once it has
    /// passed, a body error names it (Go's `cancelTimerBody`).
    client_deadline: Option<Instant>,
    permit: Option<OwnedSemaphorePermit>,
}

/// Where a response came from.
#[derive(Debug)]
enum Source {
    /// reqwest received it.
    Received(Box<reqwest::Response>),
    /// A transport made it.
    Made(Box<Made>),
    /// It came over the bidder client's pool of HTTP/2 connections.
    Pooled(Box<Pooled>),
}

/// A response over the bidder client's pool.
#[derive(Debug)]
struct Pooled {
    url: Url,
    status: StatusCode,
    headers: HeaderMap,
    body: Body,
}

/// A response a transport made.
#[derive(Debug)]
struct Made {
    url: Url,
    status: StatusCode,
    headers: HeaderMap,
    /// The part of the body not read yet.
    body: Option<Bytes>,
}

impl Response {
    /// A response from `url` that did not come over the network, with `body` as its whole body:
    /// what a transport answers in place of a server (a test's `RoundTripper` returning
    /// `&http.Response{StatusCode: 200, Body: ...}`, the parity transport's inline answer). Its
    /// version is HTTP/1.1, and its body reads at once, whatever the deadline, as Go's
    /// in-memory readers do.
    #[must_use]
    pub fn new(url: Url, status: StatusCode, headers: HeaderMap, body: Bytes) -> Self {
        Self {
            source: Source::Made(Box::new(Made {
                url,
                status,
                headers,
                body: Some(body),
            })),
            end_error: None,
            deadline: None,
            client_deadline: None,
            permit: None,
        }
    }

    /// This response with a body that fails with `error` where it would end: Go's body reader
    /// that returns an error after what it holds (a test's `errorReader`, a connection that
    /// breaks before the body's end).
    #[must_use]
    pub fn with_body_error(self, error: RequestError) -> Self {
        Self {
            end_error: Some(error),
            ..self
        }
    }

    /// A response reqwest received, holding `permit` until its body is read.
    pub(crate) fn received(
        response: reqwest::Response,
        permit: Option<OwnedSemaphorePermit>,
    ) -> Self {
        Self {
            source: Source::Received(Box::new(response)),
            end_error: None,
            deadline: None,
            client_deadline: None,
            permit,
        }
    }

    /// A response from `url` over the bidder client's pool, with `status`, `headers` and `body`,
    /// which holds the response's stream until it is read.
    pub(super) fn pooled(url: Url, status: StatusCode, headers: HeaderMap, body: Body) -> Self {
        Self {
            source: Source::Pooled(Box::new(Pooled {
                url,
                status,
                headers,
                body,
            })),
            end_error: None,
            deadline: None,
            client_deadline: None,
            permit: None,
        }
    }

    /// This response with its body read by `deadline`, and `client_deadline`, the client's
    /// timeout when it is earlier than the request's deadline, named by a body error after it.
    pub(crate) fn bounded(
        self,
        deadline: Option<Instant>,
        client_deadline: Option<Instant>,
    ) -> Self {
        Self {
            deadline,
            client_deadline,
            ..self
        }
    }

    /// The status code.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match &self.source {
            Source::Received(response) => response.status(),
            Source::Made(made) => made.status,
            Source::Pooled(pooled) => pooled.status,
        }
    }

    /// The headers. reqwest removes `Content-Encoding` and `Content-Length` from a gzip response
    /// it decompresses, as Go's Transport does for the responses it decompresses.
    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        match &self.source {
            Source::Received(response) => response.headers(),
            Source::Made(made) => &made.headers,
            Source::Pooled(pooled) => &pooled.headers,
        }
    }

    /// The headers, to change or take (bidder.go hands `httpResp.Header` itself to the adapter,
    /// exchange/bidder.go:661-668).
    #[must_use]
    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        match &mut self.source {
            Source::Received(response) => response.headers_mut(),
            Source::Made(made) => &mut made.headers,
            Source::Pooled(pooled) => &mut pooled.headers,
        }
    }

    /// The HTTP version.
    #[must_use]
    pub fn version(&self) -> Version {
        match &self.source {
            Source::Received(response) => response.version(),
            Source::Made(_) => Version::HTTP_11,
            Source::Pooled(_) => Version::HTTP_2,
        }
    }

    /// The final URL, after redirects.
    #[must_use]
    pub fn url(&self) -> &Url {
        match &self.source {
            Source::Received(response) => response.url(),
            Source::Made(made) => &made.url,
            Source::Pooled(pooled) => &pooled.url,
        }
    }

    /// The next part of the body, `None` at its end, read before the deadline. The host's slot
    /// is given back once the body ends or fails.
    ///
    /// # Errors
    ///
    /// As [`bytes`](Self::bytes).
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, RequestError> {
        let next = match &mut self.source {
            Source::Made(made) => Ok(made.body.take().filter(|part| !part.is_empty())),
            Source::Received(response) => until(self.deadline, response.chunk()).await.map_or_else(
                || Err(RequestError::deadline()),
                |read| read.map_err(RequestError::body),
            ),
            Source::Pooled(pooled) => until(self.deadline, pooled.body.chunk())
                .await
                .unwrap_or_else(|| Err(RequestError::deadline())),
        };
        let next = match next {
            Ok(None) => self.end_error.take().map_or(Ok(None), Err),
            other => other,
        };
        match next {
            Ok(Some(part)) => Ok(Some(part)),
            Ok(None) => {
                self.permit = None;
                Ok(None)
            }
            Err(error) => {
                self.permit = None;
                Err(
                    if self
                        .client_deadline
                        .is_some_and(|end| Instant::now() >= end)
                    {
                        error.client_timeout(READING_BODY)
                    } else {
                        error
                    },
                )
            }
        }
    }

    /// The body as Go's `io.ReadAll` reads it: what was read, and the error that stopped the
    /// reading, if one did (the cache client reports the part it read,
    /// `prebid_cache_client/client.go:120-123`).
    pub async fn read_all(mut self) -> (Bytes, Option<RequestError>) {
        let mut parts = Vec::new();
        let error = loop {
            match self.chunk().await {
                Ok(Some(part)) => parts.push(part),
                Ok(None) => break None,
                Err(error) => break Some(error),
            }
        };
        let body = match parts.len() {
            0 | 1 => parts.pop().unwrap_or_default(),
            _ => {
                let mut whole = BytesMut::with_capacity(parts.iter().map(Bytes::len).sum());
                for part in &parts {
                    whole.extend_from_slice(part);
                }
                whole.freeze()
            }
        };
        (body, error)
    }

    /// The whole body, read before the request's deadline: bidder.go's `io.ReadAll`, whose error
    /// bidder.go returns unclassified, a deadline included (exchange/bidder.go:641-646).
    ///
    /// # Errors
    ///
    /// The deadline passed ([`ErrorKind::DeadlineExceeded`](crate::ErrorKind::DeadlineExceeded),
    /// or [`ErrorKind::ClientTimeout`](crate::ErrorKind::ClientTimeout) for the client's
    /// timeout), the body could not be read or decompressed
    /// ([`ErrorKind::Body`](crate::ErrorKind::Body)), or it ended with the error of
    /// [`with_body_error`](Self::with_body_error).
    pub async fn bytes(self) -> Result<Bytes, RequestError> {
        match self.read_all().await {
            (body, None) => Ok(body),
            (_, Some(error)) => Err(error),
        }
    }
}
