//! Go: `net/http/httptest`: [`ResponseRecorder`] and [`new_request`] for handler tests.

use std::io;

use pbs_gocompat::{
    http::{canonical_header_key, trim_string},
    strconv::{parse_uint, quote},
    url::Url,
};

use crate::{
    Body, BoxFuture, ERR_BODY_NOT_ALLOWED, Header, Request, ResponseWriter, TRAILER_PREFIX,
    detect_content_type, status_text,
};

/// Go: `httptest.DefaultRemoteAddr`.
pub const DEFAULT_REMOTE_ADDR: &str = "1.2.3.4";

/// Go: `httptest.NewRequest(method, target, body)`.
///
/// A server request as a handler would receive it (`RemoteAddr` `192.0.2.1:1234`, `Host`
/// `example.com` unless the target names one, HTTP/1.1). A body sets `ContentLength` to its
/// length, as Go does for a `*bytes.Reader` or `*strings.Reader`.
///
/// # Errors
///
/// Go's panic text (`invalid NewRequest arguments; ...`) for an invalid method or target.
pub fn new_request(method: &str, target: &str, body: Option<&[u8]>) -> Result<Request, String> {
    let method = if method.is_empty() { "GET" } else { method };
    let invalid = |e: &str| format!("invalid NewRequest arguments; {e}");
    let token = |b: u8| b.is_ascii_graphic() && !b"()<>@,;:\\\"/[]?={}".contains(&b);
    if !method.bytes().all(token) {
        return Err(invalid(&format!("invalid method {}", quote(method))));
    }
    let just_authority = method == "CONNECT" && !target.starts_with('/');
    let raw = if just_authority {
        format!("http://{target}")
    } else {
        target.to_owned()
    };
    let mut url = Url::parse_request_uri(&raw).map_err(|e| invalid(&e.to_string()))?;
    if just_authority {
        url.scheme.clear();
    }
    let host = if url.host.is_empty() {
        String::from("example.com")
    } else {
        url.host.clone()
    };
    let (body, content_length) = body.map_or_else(
        || (Body::empty(), 0),
        |b| {
            (
                Body::from_bytes(b.to_vec()),
                i64::try_from(b.len()).unwrap_or(i64::MAX),
            )
        },
    );
    Ok(Request {
        method: method.to_owned(),
        url,
        proto: "HTTP/1.1".to_owned(),
        proto_major: 1,
        proto_minor: 1,
        body,
        content_length,
        host,
        remote_addr: "192.0.2.1:1234".to_owned(),
        request_uri: target.to_owned(),
        ..Request::default()
    })
}

/// Go: `*httptest.ResponseRecorder`.
#[derive(Debug, Clone)]
pub struct ResponseRecorder {
    code: u16,
    header_map: Header,
    body: Vec<u8>,
    flushed: bool,
    snap_header: Option<Header>,
    wrote_header: bool,
    panic: Option<String>,
    /// Go: `result`, what the first [`ResponseRecorder::result`] returned.
    result: Option<RecordedResponse>,
}

impl Default for ResponseRecorder {
    fn default() -> Self {
        Self::new()
    }
}

/// Go: the `*http.Response` of [`ResponseRecorder::result`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedResponse {
    /// Go: `StatusCode`.
    pub status_code: u16,
    /// Go: `Status` (`200 OK`).
    pub status: String,
    /// Go: `Header` (the snapshot taken at `WriteHeader`).
    pub header: Header,
    /// Go: `Body`.
    pub body: Vec<u8>,
    /// Go: `ContentLength` (-1 when not declared).
    pub content_length: i64,
    /// Go: `Trailer`.
    pub trailer: Option<Header>,
}

impl ResponseRecorder {
    /// Go: `httptest.NewRecorder()`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            code: 200,
            header_map: Header::new(),
            body: Vec::new(),
            flushed: false,
            snap_header: None,
            wrote_header: false,
            panic: None,
            result: None,
        }
    }

    /// Go: `Code`.
    #[must_use]
    pub const fn code(&self) -> u16 {
        self.code
    }

    /// Go: `HeaderMap` (the handler's live header map).
    #[must_use]
    pub const fn header_map(&self) -> &Header {
        &self.header_map
    }

    /// Go: `Body.Bytes()`.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Go: `Body.String()`.
    #[must_use]
    pub fn body_string(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Go: `Flushed`.
    #[must_use]
    pub const fn flushed(&self) -> bool {
        self.flushed
    }

    /// Go's panic text when the handler called `WriteHeader` with a code outside 100–999
    /// (`invalid WriteHeader code N`); the recorder then ignored the call.
    #[must_use]
    pub fn panic(&self) -> Option<&str> {
        self.panic.as_deref()
    }

    /// Go: `writeHeader(b, str)`.
    fn write_header_for(&mut self, b: &[u8]) {
        if self.wrote_header {
            return;
        }
        let has_type = self.header_map.get_raw("Content-Type").is_some();
        let has_te = !self.header_map.get("Transfer-Encoding").is_empty();
        if !has_type && !has_te {
            self.header_map.set("Content-Type", detect_content_type(b));
        }
        self.write_header(200);
    }

    /// Go: `rw.Result()`: the response as a client would see it. As in Go, the first call fixes
    /// it (taking the header snapshot if the handler has not written yet): later calls return
    /// the same response, whatever the handler did in between.
    pub fn result(&mut self) -> RecordedResponse {
        if let Some(res) = &self.result {
            return res.clone();
        }
        let header = self
            .snap_header
            .get_or_insert_with(|| self.header_map.clone())
            .clone();
        let status_code = if self.code == 0 { 200 } else { self.code };
        let status = format!("{status_code:03} {}", status_text(status_code));
        let content_length = parse_content_length(header.get("Content-Length"));
        let mut trailer = header.get_raw("Trailer").map(|declared| {
            let mut t = Header::new();
            for line in declared {
                for part in line.split(',') {
                    let k = canonical_header_key(trim_string(part));
                    if !valid_trailer_header(&k) {
                        continue;
                    }
                    if let Some(vv) = self.header_map.get_raw(&k) {
                        t.insert_raw(k.clone(), vv.to_vec());
                    }
                }
            }
            t
        });
        for (k, vv) in self.header_map.iter() {
            let Some(name) = k.strip_prefix(TRAILER_PREFIX) else {
                continue;
            };
            let t = trailer.get_or_insert_with(Header::new);
            for v in vv {
                t.add(name, v.clone());
            }
        }
        let res = RecordedResponse {
            status_code,
            status,
            header,
            body: self.body.clone(),
            content_length,
            trailer,
        };
        self.result = Some(res.clone());
        res
    }
}

/// Go: `httpguts.ValidTrailerHeader`.
fn valid_trailer_header(name: &str) -> bool {
    let name = canonical_header_key(name);
    !(name.starts_with("If-")
        || matches!(
            name.as_str(),
            "Authorization"
                | "Cache-Control"
                | "Connection"
                | "Content-Encoding"
                | "Content-Length"
                | "Content-Range"
                | "Content-Type"
                | "Expect"
                | "Host"
                | "Keep-Alive"
                | "Max-Forwards"
                | "Pragma"
                | "Proxy-Authenticate"
                | "Proxy-Authorization"
                | "Proxy-Connection"
                | "Range"
                | "Realm"
                | "Te"
                | "Trailer"
                | "Transfer-Encoding"
                | "Www-Authenticate"
        ))
}

/// Go: httptest's `parseContentLength`.
fn parse_content_length(cl: &str) -> i64 {
    let cl = trim_string(cl);
    if cl.is_empty() {
        return -1;
    }
    parse_uint(cl, 10, 63)
        .ok()
        .and_then(|n| i64::try_from(n).ok())
        .unwrap_or(-1)
}

impl ResponseWriter for ResponseRecorder {
    fn header(&mut self) -> &mut Header {
        &mut self.header_map
    }

    fn write_header(&mut self, code: u16) {
        if self.wrote_header {
            return;
        }
        if !(100..=999).contains(&code) {
            self.panic = Some(format!("invalid WriteHeader code {code}"));
            return;
        }
        self.code = code;
        self.wrote_header = true;
        self.snap_header = Some(self.header_map.clone());
    }

    fn write<'a>(&'a mut self, data: &'a [u8]) -> BoxFuture<'a, io::Result<usize>> {
        self.write_header_for(data);
        self.body.extend_from_slice(data);
        let allowed = !matches!(self.code, 100..=199 | 204 | 304);
        Box::pin(async move {
            if allowed {
                Ok(data.len())
            } else {
                Err(io::Error::other(ERR_BODY_NOT_ALLOWED))
            }
        })
    }

    fn flush(&mut self) -> BoxFuture<'_, io::Result<()>> {
        if !self.wrote_header {
            self.write_header(200);
        }
        self.flushed = true;
        Box::pin(async { Ok(()) })
    }
}
