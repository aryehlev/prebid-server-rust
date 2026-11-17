//! Go `net/http/transfer.go`: the framing rules for outgoing requests (`transferWriter`) and
//! incoming responses (`readTransfer`).

use pbs_gocompat::{
    http::{Header, canonical_mime_header_key, trim_string},
    strconv::{parse_uint, quote},
};

use crate::{
    error::HttpError,
    httpguts::{has_token, header_values_contains_token},
    request::{Body, Request},
};

/// Go: `chunked(te)`.
#[must_use]
pub fn is_chunked(te: &[String]) -> bool {
    te.first().is_some_and(|t| t == "chunked")
}

/// Go: `isIdentity(te)`.
fn is_identity(te: &[String]) -> bool {
    te.len() == 1 && te.first().is_some_and(|t| t == "identity")
}

/// Go: `requestMethodUsuallyLacksBody`.
#[must_use]
pub fn request_method_usually_lacks_body(method: &str) -> bool {
    matches!(
        method,
        "GET" | "HEAD" | "DELETE" | "OPTIONS" | "PROPFIND" | "SEARCH"
    )
}

/// What the request body probe found (Go: `transferWriter.probeRequestBody`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    /// The body was empty: send no body.
    Empty,
    /// The body has at least one byte: send it chunked.
    HasData,
    /// The body did not answer within 200ms: send it chunked and flush the headers first.
    Slow,
}

/// Go: `transferWriter` for a `*Request`.
#[derive(Debug, Clone)]
pub struct TransferWriter {
    /// Go: `Method` (defaulted to `GET`).
    pub method: String,
    /// Go: `Close`.
    pub close: bool,
    /// Go: `TransferEncoding` after sanitizing.
    pub transfer_encoding: Vec<String>,
    /// Go: `ContentLength` after sanitizing (`-1` when chunked or unknown).
    pub content_length: i64,
    /// Go: `Trailer` (only kept for chunked bodies).
    pub trailer: Option<Header>,
    /// Go: `Body != nil`.
    pub has_body: bool,
    /// Go: `FlushHeaders`.
    pub flush_headers: bool,
    /// Go: `Header.get("Connection")` (exact key).
    pub connection: String,
}

impl TransferWriter {
    /// Go: `newTransferWriter(r *Request)` up to the body probe; call
    /// [`TransferWriter::after_probe`] when [`TransferWriter::needs_probe`] holds, then
    /// [`TransferWriter::sanitize`].
    ///
    /// # Errors
    ///
    /// Go's `http: Request.ContentLength=%d with nil Body`.
    pub fn for_request(req: &Request) -> Result<Self, HttpError> {
        let has_body = !matches!(req.body, Body::Nil);
        if req.content_length != 0 && !has_body {
            return Err(HttpError::Message(format!(
                "http: Request.ContentLength={} with nil Body",
                req.content_length
            )));
        }
        let method = if req.method.is_empty() {
            "GET".to_owned()
        } else {
            req.method.clone()
        };
        let content_length = req.outgoing_length();
        let mut tw = Self {
            method,
            close: req.close,
            transfer_encoding: req.transfer_encoding.clone(),
            content_length,
            trailer: req.trailer.clone(),
            has_body,
            flush_headers: false,
            connection: req
                .header
                .get_raw("Connection")
                .and_then(|v| v.first())
                .cloned()
                .unwrap_or_default(),
        };
        // Go: shouldSendChunkedRequestBody; GET-like methods wait for the probe.
        if tw.content_length < 0
            && tw.transfer_encoding.is_empty()
            && tw.method != "CONNECT"
            && !request_method_usually_lacks_body(&tw.method)
        {
            tw.transfer_encoding = vec!["chunked".to_owned()];
        }
        // Go: isKnownInMemoryReader skips the early flush for in-memory bodies.
        if tw.content_length != 0 && matches!(req.body, Body::Reader(_)) {
            tw.flush_headers = true;
        }
        Ok(tw)
    }

    /// Whether Go's `probeRequestBody` decides the framing: a GET-like request whose body
    /// has an unknown length.
    #[must_use]
    pub fn needs_probe(&self) -> bool {
        self.content_length < 0
            && self.transfer_encoding.is_empty()
            && self.method != "CONNECT"
            && request_method_usually_lacks_body(&self.method)
    }

    /// Applies the body probe's result (Go: `probeRequestBody`, after which `FlushHeaders`
    /// holds for the wrapped, no longer in-memory body).
    pub fn after_probe(&mut self, result: &ProbeResult) {
        match result {
            ProbeResult::Empty => {
                self.has_body = false;
                self.content_length = 0;
                self.flush_headers = false;
            }
            ProbeResult::HasData | ProbeResult::Slow => {
                self.transfer_encoding = vec!["chunked".to_owned()];
                self.flush_headers = true;
            }
        }
    }

    /// Go: the "Sanitize `Body`, `ContentLength`, `TransferEncoding`" and trailer steps.
    pub fn sanitize(&mut self) {
        if !self.has_body {
            self.transfer_encoding.clear();
        }
        if is_chunked(&self.transfer_encoding) {
            self.content_length = -1;
        } else if !self.has_body {
            self.content_length = 0;
        }
        if !is_chunked(&self.transfer_encoding) {
            self.trailer = None;
        }
    }

    /// Whether the body is sent with the chunked coding.
    #[must_use]
    pub fn chunked(&self) -> bool {
        is_chunked(&self.transfer_encoding)
    }

    /// Go: `shouldSendContentLength`.
    #[must_use]
    pub fn should_send_content_length(&self) -> bool {
        if self.chunked() {
            return false;
        }
        if self.content_length > 0 {
            return true;
        }
        if self.content_length < 0 {
            return false;
        }
        // Many servers expect a Content-Length for these methods.
        if matches!(self.method.as_str(), "POST" | "PUT" | "PATCH") {
            return true;
        }
        if self.content_length == 0 && is_identity(&self.transfer_encoding) {
            return !matches!(self.method.as_str(), "GET" | "HEAD");
        }
        false
    }

    /// Go: `transferWriter.writeHeader`.
    ///
    /// # Errors
    ///
    /// Go's `invalid Trailer key "Content-Length"`.
    pub fn write_header(&self, out: &mut Vec<u8>) -> Result<(), HttpError> {
        if self.close && !has_token(&self.connection, "close") {
            out.extend_from_slice(b"Connection: close\r\n");
        }
        if self.should_send_content_length() {
            out.extend_from_slice(b"Content-Length: ");
            out.extend_from_slice(self.content_length.to_string().as_bytes());
            out.extend_from_slice(b"\r\n");
        } else if self.chunked() {
            out.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
        }
        if let Some(trailer) = &self.trailer {
            let mut keys = Vec::with_capacity(trailer.len());
            for (k, _) in trailer.iter() {
                let k = canonical_mime_header_key(k);
                if matches!(
                    k.as_str(),
                    "Transfer-Encoding" | "Trailer" | "Content-Length"
                ) {
                    return Err(HttpError::Message(format!(
                        "invalid Trailer key {}",
                        quote(&k)
                    )));
                }
                keys.push(k);
            }
            if !keys.is_empty() {
                keys.sort();
                out.extend_from_slice(b"Trailer: ");
                out.extend_from_slice(keys.join(",").as_bytes());
                out.extend_from_slice(b"\r\n");
            }
        }
        Ok(())
    }
}

/// How a response body is delimited (Go: the `t.Body` choice in `readTransfer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// Go: `NoBody`.
    NoBody,
    /// Go: `io.LimitReader(r, n)`.
    Length(u64),
    /// Go: `internal.NewChunkedReader(r)` with trailer reading.
    Chunked,
    /// Go: the connection's remaining bytes (`closing` set).
    Close,
}

/// The result of Go's `readTransfer` for a response.
#[derive(Debug, Clone)]
pub struct ResponseTransfer {
    /// Go: `ContentLength`.
    pub content_length: i64,
    /// Go: `TransferEncoding`.
    pub transfer_encoding: Vec<String>,
    /// Go: `Close`.
    pub close: bool,
    /// Go: `Trailer` (announced keys with no values).
    pub trailer: Option<Header>,
    /// The body framing.
    pub framing: Framing,
}

/// Go: `bodyAllowedForStatus`.
#[must_use]
pub const fn body_allowed_for_status(status: u16) -> bool {
    !matches!(status, 100..=199 | 204 | 304)
}

/// Go: `parseContentLength`.
fn parse_content_length(cl_headers: &[String]) -> Result<i64, HttpError> {
    let Some(first) = cl_headers.first() else {
        return Ok(-1);
    };
    let cl = trim_string(first);
    if cl.is_empty() {
        return Err(HttpError::Message(format!(
            "invalid empty Content-Length {}",
            quote(cl)
        )));
    }
    parse_uint(cl, 10, 63)
        .ok()
        .and_then(|n| i64::try_from(n).ok())
        .ok_or_else(|| HttpError::Message(format!("bad Content-Length {}", quote(cl))))
}

/// Go: `shouldClose`.
pub fn should_close(major: u8, minor: u8, header: &mut Header, remove_close_header: bool) -> bool {
    if major < 1 {
        return true;
    }
    let conv = header.get_raw("Connection").unwrap_or_default();
    let has_close = header_values_contains_token(conv, "close");
    if major == 1 && minor == 0 {
        return has_close || !header_values_contains_token(conv, "keep-alive");
    }
    if has_close && remove_close_header {
        header.del("Connection");
    }
    has_close
}

/// Go: `foreachHeaderElement`.
fn for_each_header_element(v: &str, mut f: impl FnMut(&str)) {
    let v = trim_string(v);
    if v.is_empty() {
        return;
    }
    if !v.contains(',') {
        f(v);
        return;
    }
    for part in v.split(',') {
        let part = trim_string(part);
        if !part.is_empty() {
            f(part);
        }
    }
}

/// Go: `fixTrailer`.
fn fix_trailer(header: &mut Header, chunked: bool) -> Result<Option<Header>, HttpError> {
    let Some(vv) = header.get_raw("Trailer").map(<[String]>::to_vec) else {
        return Ok(None);
    };
    if !chunked {
        return Ok(None);
    }
    header.del("Trailer");
    let mut trailer = Header::new();
    let mut err = None;
    for v in &vv {
        for_each_header_element(v, |key| {
            let key = canonical_mime_header_key(key);
            if matches!(
                key.as_str(),
                "Transfer-Encoding" | "Trailer" | "Content-Length"
            ) {
                if err.is_none() {
                    err = Some(HttpError::Message(format!(
                        "bad trailer key {}",
                        quote(&key)
                    )));
                }
                return;
            }
            trailer.insert_raw(key, Vec::new());
        });
    }
    if let Some(e) = err {
        return Err(e);
    }
    Ok((!trailer.is_empty()).then_some(trailer))
}

/// Go: `fixLength` for a response.
fn fix_length(
    status: u16,
    request_method: &str,
    header: &mut Header,
    chunked: bool,
) -> Result<i64, HttpError> {
    let mut content_lens = header
        .get_raw("Content-Length")
        .map_or_else(Vec::new, <[String]>::to_vec);
    if content_lens.len() > 1 {
        let first = trim_string(content_lens.first().map_or("", String::as_str)).to_owned();
        if content_lens
            .iter()
            .skip(1)
            .any(|ct| trim_string(ct) != first)
        {
            let list: Vec<String> = content_lens.iter().map(|s| quote(s)).collect();
            return Err(HttpError::Message(format!(
                "http: message cannot contain multiple Content-Length headers; got [{}]",
                list.join(" ")
            )));
        }
        header.del("Content-Length");
        header.add("Content-Length", first.clone());
        content_lens = vec![first];
    }
    // Go: reject an invalid Content-Length before the status rules (`-1` when absent).
    let n = parse_content_length(&content_lens)?;
    if request_method == "HEAD" || status / 100 == 1 || matches!(status, 204 | 304) {
        return Ok(0);
    }
    if chunked {
        header.del("Content-Length");
        return Ok(-1);
    }
    if !content_lens.is_empty() {
        return Ok(n);
    }
    header.del("Content-Length");
    Ok(-1)
}

/// Go: `transferReader.parseTransferEncoding`: whether the body is chunked.
fn parse_transfer_encoding(header: &mut Header, major: u8, minor: u8) -> Result<bool, HttpError> {
    let Some(raw) = header.remove_raw("Transfer-Encoding") else {
        return Ok(false);
    };
    // Go: Transfer-Encoding is ignored before HTTP/1.1 (issue 12785).
    if major < 1 || (major == 1 && minor < 1) {
        return Ok(false);
    }
    if raw.len() != 1 {
        let list: Vec<String> = raw.iter().map(|s| quote(s)).collect();
        return Err(HttpError::Message(format!(
            "too many transfer encodings: [{}]",
            list.join(" ")
        )));
    }
    let te = raw.first().map_or("", String::as_str);
    if !te.eq_ignore_ascii_case("chunked") {
        return Err(HttpError::Message(format!(
            "unsupported transfer encoding: {}",
            quote(te)
        )));
    }
    Ok(true)
}

/// Go: `readTransfer` for a response to a request with `request_method`.
///
/// # Errors
///
/// Go's framing errors (`too many transfer encodings: ["a" "b"]`, `unsupported transfer
/// encoding: "gzip"`, `bad Content-Length "x"`, ...).
pub fn read_transfer(
    status: u16,
    proto: (u8, u8),
    request_method: &str,
    header: &mut Header,
) -> Result<ResponseTransfer, HttpError> {
    let (major, minor) = if proto == (0, 0) { (1, 1) } else { proto };
    let mut close = should_close(major, minor, header, true);
    let chunked = parse_transfer_encoding(header, major, minor)?;
    let real_length = fix_length(status, request_method, header, chunked)?;
    let content_length = if request_method == "HEAD" {
        parse_content_length(header.get_raw("Content-Length").unwrap_or_default())?
    } else {
        real_length
    };
    let trailer = fix_trailer(header, chunked)?;
    if real_length == -1 && !chunked && body_allowed_for_status(status) {
        close = true;
    }
    let framing = if chunked {
        if request_method == "HEAD" || !body_allowed_for_status(status) {
            Framing::NoBody
        } else {
            Framing::Chunked
        }
    } else if real_length == 0 {
        Framing::NoBody
    } else if real_length > 0 {
        Framing::Length(real_length.unsigned_abs())
    } else if close {
        Framing::Close
    } else {
        Framing::NoBody
    };
    Ok(ResponseTransfer {
        content_length,
        transfer_encoding: if chunked {
            vec!["chunked".to_owned()]
        } else {
            Vec::new()
        },
        close,
        trailer,
        framing,
    })
}
