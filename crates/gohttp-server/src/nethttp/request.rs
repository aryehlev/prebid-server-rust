//! Go: `net/http/request.go` (the server side): the [`Request`] type, `readRequest`, the
//! request half of `readTransfer`, form parsing and request cookies.

use core::{error::Error, fmt, str};
use std::io;

use pbs_gocompat::{
    http::{Header, trim_string},
    strconv::quote,
    url::{QueryError, Url, Values, parse_query_pair},
};

use super::{
    body::{Body, Framing, declared_trailer},
    bufio::{BufReader, ReadErr},
    context::RequestContext,
    lex::{ascii_equal_fold, has_token, header_values_contains_token, is_token, lossy},
    mime::parse_media_type,
    netconn::NetError,
};

/// Go: `http.Request` as a server handler receives it.
#[derive(Debug, Default)]
pub struct Request {
    /// Go: `Method`.
    pub method: String,
    /// Go: `URL` (from `url.ParseRequestURI` of the request target).
    pub url: Url,
    /// Go: `Proto` (`HTTP/1.1`).
    pub proto: String,
    /// Go: `ProtoMajor`.
    pub proto_major: i32,
    /// Go: `ProtoMinor`.
    pub proto_minor: i32,
    /// Go: `Header` (the server removes `Host`).
    pub header: Header,
    /// Go: `Body`.
    pub body: Body,
    /// Go: `ContentLength` (-1 for chunked bodies).
    pub content_length: i64,
    /// Go: `TransferEncoding`.
    pub transfer_encoding: Vec<String>,
    /// Go: `Close`.
    pub close: bool,
    /// Go: `Host`.
    pub host: String,
    /// Go: `Form` (`None` until [`Request::parse_form`]).
    pub form: Option<Values>,
    /// Go: `PostForm`.
    pub post_form: Option<Values>,
    /// Go: `Trailer` as declared by the request's `Trailer` header (the values arrive through
    /// [`Body::trailer`]).
    pub trailer: Option<Header>,
    /// Go: `RemoteAddr`.
    pub remote_addr: String,
    /// Go: `RequestURI`.
    pub request_uri: String,
    /// Go: `ctx` (`Request.Context()`).
    pub ctx: RequestContext,
}

/// Go: `http.ErrNoCookie`.
pub const ERR_NO_COOKIE: &str = "http: named cookie not present";

/// Go: `http.Cookie` as `Request.Cookies` produces it (name, value and whether the value was
/// quoted; request cookies carry no attributes).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cookie {
    /// Go: `Name`.
    pub name: String,
    /// Go: `Value`.
    pub value: String,
    /// Go: `Quoted`.
    pub quoted: bool,
}

/// Go: `defaultCookieMaxNum`.
const DEFAULT_COOKIE_MAX_NUM: usize = 3000;

/// Go: `validCookieValueByte`.
const fn valid_cookie_value_byte(b: u8) -> bool {
    0x20 <= b && b < 0x7F && b != b'"' && b != b';' && b != b'\\'
}

/// Go: `parseCookieValue(raw, true)`.
fn parse_cookie_value(raw: &str) -> Option<(&str, bool)> {
    let (raw, quoted) = if raw.len() > 1 && raw.starts_with('"') && raw.ends_with('"') {
        (raw.get(1..raw.len() - 1).unwrap_or_default(), true)
    } else {
        (raw, false)
    };
    raw.bytes()
        .all(valid_cookie_value_byte)
        .then_some((raw, quoted))
}

/// Go: `readCookies(h, filter)`.
fn read_cookies(h: &Header, filter: &str) -> Vec<Cookie> {
    let Some(lines) = h.get_raw("Cookie") else {
        return Vec::new();
    };
    let count: usize = lines.iter().map(|l| l.matches(';').count() + 1).sum();
    if count > DEFAULT_COOKIE_MAX_NUM {
        return Vec::new();
    }
    let mut cookies = Vec::new();
    for line in lines {
        let mut line = trim_string(line);
        while !line.is_empty() {
            let (part, rest) = line.split_once(';').unwrap_or((line, ""));
            line = rest;
            let part = trim_string(part);
            if part.is_empty() {
                continue;
            }
            let (name, val) = part.split_once('=').unwrap_or((part, ""));
            let name = trim_string(name);
            if !is_token(name.as_bytes()) {
                continue;
            }
            if !filter.is_empty() && filter != name {
                continue;
            }
            let Some((value, quoted)) = parse_cookie_value(val) else {
                continue;
            };
            cookies.push(Cookie {
                name: name.to_owned(),
                value: value.to_owned(),
                quoted,
            });
        }
    }
    cookies
}

/// Go: `ct, _, err = mime.ParseMediaType(ct)`: the media type and the error.
fn media_type(v: &str) -> (String, Option<&'static str>) {
    match parse_media_type(v) {
        Ok((t, _)) => (t, None),
        Err(e) => (e.media_type, Some(e.text)),
    }
}

/// Go: `copyValues(dst, src)`.
fn copy_values(dst: &mut Values, src: &Values) {
    for (k, vs) in src.iter() {
        let mut all = dst.get_all(k).map_or_else(Vec::new, <[String]>::to_vec);
        all.extend(vs.iter().cloned());
        dst.insert(k, all);
    }
}

/// An error of [`Request::parse_form`].
#[derive(Debug)]
pub enum FormError {
    /// Go: the body read error.
    Read(io::Error),
    /// Go: `http: POST too large`.
    TooLarge,
    /// Go: the `url.ParseQuery` error.
    Query(QueryError),
    /// Go: the `mime.ParseMediaType` error of the `Content-Type`.
    MediaType(&'static str),
}

impl fmt::Display for FormError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(e) => e.fmt(f),
            Self::TooLarge => f.write_str("http: POST too large"),
            Self::Query(e) => e.fmt(f),
            Self::MediaType(t) => f.write_str(t),
        }
    }
}

impl Error for FormError {}

impl Request {
    /// Go: `http.NewRequest(method, target, body)` for handler tests: the target is parsed
    /// with `url.Parse`, `Host` is the URL's host and the protocol is HTTP/1.1.
    ///
    /// # Errors
    ///
    /// Go's `net/http: invalid method` or the `url.Parse` error text.
    pub fn new(method: &str, target: &str, body: Body) -> Result<Self, String> {
        let method = if method.is_empty() { "GET" } else { method };
        if !is_token(method.as_bytes()) {
            return Err(format!("net/http: invalid method {}", quote(method)));
        }
        let url = Url::parse(target).map_err(|e| e.to_string())?;
        Ok(Self {
            method: method.to_owned(),
            host: url.host.clone(),
            url,
            proto: "HTTP/1.1".to_owned(),
            proto_major: 1,
            proto_minor: 1,
            body,
            ..Self::default()
        })
    }

    /// Go: `ProtoAtLeast`.
    #[must_use]
    pub const fn proto_at_least(&self, major: i32, minor: i32) -> bool {
        self.proto_major > major || (self.proto_major == major && self.proto_minor >= minor)
    }

    /// Go: `UserAgent`.
    #[must_use]
    pub fn user_agent(&self) -> &str {
        self.header.get("User-Agent")
    }

    /// Go: `Referer`.
    #[must_use]
    pub fn referer(&self) -> &str {
        self.header.get("Referer")
    }

    /// Go: `Context`.
    #[must_use]
    pub const fn context(&self) -> &RequestContext {
        &self.ctx
    }

    /// Go: `Cookies`.
    #[must_use]
    pub fn cookies(&self) -> Vec<Cookie> {
        read_cookies(&self.header, "")
    }

    /// Go: `Cookie(name)`.
    ///
    /// # Errors
    ///
    /// [`ERR_NO_COOKIE`] when there is no valid cookie of that name.
    pub fn cookie(&self, name: &str) -> Result<Cookie, &'static str> {
        if name.is_empty() {
            return Err(ERR_NO_COOKIE);
        }
        read_cookies(&self.header, name)
            .into_iter()
            .next()
            .ok_or(ERR_NO_COOKIE)
    }

    /// Go: `parsePostForm`.
    async fn parse_post_form(&mut self) -> (Option<Values>, Option<FormError>) {
        let ct = self.header.get("Content-Type");
        let ct = if ct.is_empty() {
            "application/octet-stream"
        } else {
            ct
        };
        let (ct, err) = media_type(ct);
        let mut err = err.map(FormError::MediaType);
        if ct != "application/x-www-form-urlencoded" {
            return (None, err);
        }
        // Go: no limit when the body is a `MaxBytesReader`, 10 MB otherwise.
        let max_form_size: usize = if self.body.is_max_bytes() {
            usize::MAX - 1
        } else {
            10 << 20
        };
        let mut b = Vec::new();
        let mut chunk = vec![0_u8; 4096];
        loop {
            let want = (max_form_size + 1 - b.len()).min(chunk.len());
            if want == 0 {
                break;
            }
            match self
                .body
                .read(chunk.get_mut(..want).unwrap_or_default())
                .await
            {
                Ok(0) => break,
                Ok(n) => b.extend_from_slice(chunk.get(..n).unwrap_or_default()),
                Err(e) => {
                    if err.is_none() {
                        err = Some(FormError::Read(e));
                    }
                    return (None, err);
                }
            }
        }
        if b.len() > max_form_size {
            return (None, Some(FormError::TooLarge));
        }
        let (vs, e) = parse_query_pair(&lossy(&b));
        if err.is_none() {
            err = e.map(FormError::Query);
        }
        (Some(vs), err)
    }

    /// Go: `ParseForm`: fills [`Request::form`] and [`Request::post_form`] from the query and,
    /// for POST, PUT and PATCH, an `application/x-www-form-urlencoded` body (at most 10 MB).
    ///
    /// # Errors
    ///
    /// The first body or query error; the forms hold what was parsed.
    pub async fn parse_form(&mut self) -> Result<(), FormError> {
        let mut err = None;
        if self.post_form.is_none() {
            if matches!(self.method.as_str(), "POST" | "PUT" | "PATCH") {
                let (pf, e) = self.parse_post_form().await;
                self.post_form = pf;
                err = e;
            }
            if self.post_form.is_none() {
                self.post_form = Some(Values::default());
            }
        }
        if self.form.is_none() {
            let form = self
                .post_form
                .as_ref()
                .filter(|pf| !pf.is_empty())
                .map(|pf| {
                    let mut f = Values::default();
                    copy_values(&mut f, pf);
                    f
                });
            let (new_values, e) = parse_query_pair(&self.url.raw_query);
            if err.is_none() {
                err = e.map(FormError::Query);
            }
            self.form = Some(match form {
                None => new_values,
                Some(mut f) => {
                    copy_values(&mut f, &new_values);
                    f
                }
            });
        }
        err.map_or(Ok(()), Err)
    }

    /// Go: `FormValue(key)`: the first value of `key` in the query or form body. Multipart
    /// bodies are not parsed.
    pub async fn form_value(&mut self, key: &str) -> String {
        if self.form.is_none() {
            let _ignored = self.parse_form().await;
        }
        self.form
            .as_ref()
            .and_then(|f| f.get_all(key))
            .and_then(<[String]>::first)
            .cloned()
            .unwrap_or_default()
    }

    /// Go: `PostFormValue(key)`.
    pub async fn post_form_value(&mut self, key: &str) -> String {
        if self.post_form.is_none() {
            let _ignored = self.parse_form().await;
        }
        self.post_form
            .as_ref()
            .and_then(|f| f.get_all(key))
            .and_then(<[String]>::first)
            .cloned()
            .unwrap_or_default()
    }

    /// Go: `expectsContinue`.
    pub(super) fn expects_continue(&self) -> bool {
        has_token(first_raw(&self.header, "Expect"), "100-continue")
    }

    /// Go: `wantsHttp10KeepAlive`.
    pub(super) fn wants_http10_keep_alive(&self) -> bool {
        self.proto_major == 1
            && self.proto_minor == 0
            && has_token(first_raw(&self.header, "Connection"), "keep-alive")
    }

    /// Go: `wantsClose`.
    pub(super) fn wants_close(&self) -> bool {
        self.close || has_token(first_raw(&self.header, "Connection"), "close")
    }

    /// Go: `isH2Upgrade`.
    pub(super) fn is_h2_upgrade(&self) -> bool {
        self.method == "PRI"
            && self.header.is_empty()
            && self.url.path == "*"
            && self.proto == "HTTP/2.0"
    }
}

/// Go: `Header.get` (no key canonicalization): the first value or `""`.
pub(super) fn first_raw<'a>(h: &'a Header, key: &str) -> &'a str {
    h.get_raw(key)
        .and_then(<[String]>::first)
        .map_or("", String::as_str)
}

/// Go: `ParseHTTPVersion`.
fn parse_http_version(vers: &[u8]) -> Option<(i32, i32)> {
    match vers {
        b"HTTP/1.1" => Some((1, 1)),
        b"HTTP/1.0" => Some((1, 0)),
        [b'H', b'T', b'T', b'P', b'/', maj, b'.', min]
            if maj.is_ascii_digit() && min.is_ascii_digit() =>
        {
            Some((i32::from(maj - b'0'), i32::from(min - b'0')))
        }
        _ => None,
    }
}

/// Why reading a request failed, as `conn.serve` sorts it.
#[derive(Debug)]
pub(super) enum RequestError {
    /// Go: `isUnsupportedTEError`: 501.
    UnsupportedTransferEncoding,
    /// Go: `isCommonNetReadError`: no reply.
    CommonNetRead,
    /// Go: `statusError`.
    Status(u16, &'static str),
    /// Any other error: `400 Bad Request`.
    BadRequest,
}

const fn classify_first_line(err: &ReadErr) -> RequestError {
    match err {
        ReadErr::Net(NetError::Eof | NetError::Timeout(_) | NetError::Op(_)) => {
            RequestError::CommonNetRead
        }
        ReadErr::BufferFull | ReadErr::Protocol(_) | ReadErr::MessageTooLarge => {
            RequestError::BadRequest
        }
    }
}

const fn classify_later(err: &ReadErr) -> RequestError {
    match err {
        // Go: `readRequest` turns io.EOF into io.ErrUnexpectedEOF after the first line.
        ReadErr::Net(NetError::Eof) => RequestError::BadRequest,
        other @ (ReadErr::Net(NetError::Timeout(_) | NetError::Op(_))
        | ReadErr::BufferFull
        | ReadErr::Protocol(_)
        | ReadErr::MessageTooLarge) => classify_first_line(other),
    }
}

/// The request as read, with how its body is framed.
#[derive(Debug)]
pub(super) struct ReadRequest {
    pub(super) req: Request,
    /// `None` for `http.NoBody`.
    pub(super) framing: Option<Framing>,
}

/// Percent-encodes the bytes >= 0x80 of a request target that is not UTF-8, so it can be held
/// in a Rust string (the decoded path then carries U+FFFD in their place).
fn target_string(raw: &[u8]) -> String {
    if let Ok(s) = str::from_utf8(raw) {
        return s.to_owned();
    }
    let mut out = String::with_capacity(raw.len() * 3);
    for &b in raw {
        if b >= 0x80 {
            out.push('%');
            for nibble in [b >> 4, b & 0xF] {
                out.push(
                    char::from_digit(u32::from(nibble), 16).map_or('0', |c| c.to_ascii_uppercase()),
                );
            }
        } else {
            out.push(char::from(b));
        }
    }
    out
}

/// Go: `readRequest(c.bufr)` followed by the checks of `conn.readRequest` (protocol version,
/// `Host`, header field names and values).
pub(super) async fn read_request(br: &mut BufReader) -> Result<ReadRequest, RequestError> {
    let line = br
        .read_line_slice(-1)
        .await
        .map_err(|e| classify_first_line(&e))?;
    let (method, rest) = split_once_byte(&line, b' ').ok_or(RequestError::BadRequest)?;
    let (request_uri, proto) = split_once_byte(rest, b' ').ok_or(RequestError::BadRequest)?;
    if !is_token(method) {
        return Err(RequestError::BadRequest);
    }
    let (proto_major, proto_minor) = parse_http_version(proto).ok_or(RequestError::BadRequest)?;
    let method = lossy(method);
    let request_uri_text = lossy(request_uri);
    let just_authority = method == "CONNECT" && !request_uri.starts_with(b"/");
    let mut raw_url = target_string(request_uri);
    if just_authority {
        raw_url.insert_str(0, "http://");
    }
    let mut url = Url::parse_request_uri(&raw_url).map_err(|_e| RequestError::BadRequest)?;
    if just_authority {
        url.scheme.clear();
    }

    let (fields, err) = br.read_mime_header().await;
    if let Some(e) = err {
        return Err(classify_later(&e));
    }
    let mut header = Header::new();
    for (k, v) in fields {
        let mut values = header.remove_raw(&k).unwrap_or_default();
        values.push(v);
        header.insert_raw(k, values);
    }
    if header.get_raw("Host").is_some_and(|h| h.len() > 1) {
        return Err(RequestError::BadRequest);
    }
    let mut req = Request {
        host: if url.host.is_empty() {
            first_raw(&header, "Host").to_owned()
        } else {
            url.host.clone()
        },
        method,
        url,
        proto: lossy(proto),
        proto_major,
        proto_minor,
        header,
        request_uri: request_uri_text,
        ..Request::default()
    };

    // Go: `fixPragmaCacheControl`.
    if req
        .header
        .get_raw("Pragma")
        .and_then(<[String]>::first)
        .map(String::as_str)
        == Some("no-cache")
        && req.header.get_raw("Cache-Control").is_none()
    {
        req.header
            .insert_raw("Cache-Control", vec!["no-cache".to_owned()]);
    }
    // Go: `shouldClose(major, minor, header, false)`.
    req.close = {
        let conv = req.header.get_raw("Connection").unwrap_or_default();
        let has_close = header_values_contains_token(conv, "close");
        if proto_major < 1 {
            true
        } else if proto_major == 1 && proto_minor == 0 {
            has_close || !header_values_contains_token(conv, "keep-alive")
        } else {
            has_close
        }
    };
    let framing = read_transfer(&mut req)?;
    if req.is_h2_upgrade() {
        req.content_length = -1;
        req.close = true;
    }
    Ok(ReadRequest { req, framing })
}

/// Go: the checks of `conn.readRequest` that follow `setInfiniteReadLimit`.
pub(super) fn check_request(req: &mut Request) -> Result<(), RequestError> {
    let hosts = req.header.get_raw("Host");
    let is_h2_upgrade = req.is_h2_upgrade();
    if req.proto_at_least(1, 1)
        && hosts.is_none_or(<[String]>::is_empty)
        && !is_h2_upgrade
        && req.method != "CONNECT"
    {
        return Err(RequestError::Status(400, "missing required Host header"));
    }
    if let Some([host]) = hosts
        && !super::lex::valid_host_header(host)
    {
        return Err(RequestError::Status(400, "malformed Host header"));
    }
    for (k, vv) in req.header.iter() {
        if !is_token(k.as_bytes()) {
            return Err(RequestError::Status(400, "invalid header name"));
        }
        if !vv.iter().all(|v| super::lex::valid_header_field_value(v)) {
            return Err(RequestError::Status(400, "invalid header value"));
        }
    }
    let _host = req.header.remove_raw("Host");
    Ok(())
}

fn split_once_byte(s: &[u8], b: u8) -> Option<(&[u8], &[u8])> {
    let i = s.iter().position(|&c| c == b)?;
    Some((s.get(..i)?, s.get(i + 1..)?))
}

/// Go: `parseContentLength`.
fn parse_content_length(cl: &str) -> Result<i64, RequestError> {
    let cl = trim_string(cl);
    if cl.is_empty() {
        return Err(RequestError::BadRequest);
    }
    if !cl.bytes().all(|b| b.is_ascii_digit()) {
        return Err(RequestError::BadRequest);
    }
    cl.parse::<u64>()
        .ok()
        .filter(|&n| n < (1 << 63))
        .and_then(|n| i64::try_from(n).ok())
        .ok_or(RequestError::BadRequest)
}

/// Go: `readTransfer` for a request: `Transfer-Encoding`, `Content-Length` and `Trailer`.
fn read_transfer(req: &mut Request) -> Result<Option<Framing>, RequestError> {
    let (major, minor) = if req.proto_major == 0 && req.proto_minor == 0 {
        (1, 1)
    } else {
        (req.proto_major, req.proto_minor)
    };
    // Go: `parseTransferEncoding`.
    let mut chunked = false;
    if let Some(raw) = req.header.remove_raw("Transfer-Encoding")
        && (major > 1 || (major == 1 && minor >= 1))
    {
        match raw.as_slice() {
            [one] if ascii_equal_fold(one, "chunked") => chunked = true,
            _ => return Err(RequestError::UnsupportedTransferEncoding),
        }
    }
    // Go: `fixLength`.
    let mut content_lens = req
        .header
        .get_raw("Content-Length")
        .map_or_else(Vec::new, <[String]>::to_vec);
    if content_lens.len() > 1 {
        let first = trim_string(content_lens.first().map_or("", String::as_str)).to_owned();
        if content_lens
            .iter()
            .skip(1)
            .any(|ct| trim_string(ct) != first)
        {
            return Err(RequestError::BadRequest);
        }
        req.header.del("Content-Length");
        req.header.add("Content-Length", first.clone());
        content_lens = vec![first];
    }
    let n = if let Some(cl) = content_lens.first() {
        parse_content_length(cl)?
    } else {
        0
    };
    let real_length = if chunked {
        req.header.del("Content-Length");
        -1
    } else if content_lens.is_empty() {
        req.header.del("Content-Length");
        0
    } else {
        n
    };
    req.content_length = real_length;
    // Go: `fixTrailer`.
    if chunked && let Some(vv) = req.header.get_raw("Trailer").map(<[String]>::to_vec) {
        req.header.del("Trailer");
        req.trailer = declared_trailer(&vv).map_err(|_e| RequestError::BadRequest)?;
    }
    if chunked {
        req.transfer_encoding = vec!["chunked".to_owned()];
        return Ok(Some(Framing::Chunked));
    }
    Ok((real_length > 0).then_some(Framing::Length(real_length)))
}

#[cfg(test)]
mod tests {
    use core::fmt::Debug;

    use pbs_gocompat::http::Header;

    use super::{media_type, parse_http_version, read_cookies};

    fn check<T: PartialEq + Debug>(got: &T, want: &T, name: &str) -> Result<(), String> {
        if got == want {
            Ok(())
        } else {
            Err(format!("{name}: got {got:?}, want {want:?}"))
        }
    }

    #[test]
    fn http_versions() -> Result<(), String> {
        check(&parse_http_version(b"HTTP/1.1"), &Some((1, 1)), "1.1")?;
        check(&parse_http_version(b"HTTP/2.0"), &Some((2, 0)), "2.0")?;
        check(&parse_http_version(b"HTTP/1.10"), &None, "1.10")?;
        check(&parse_http_version(b"http/1.1"), &None, "lower")?;
        check(&parse_http_version(b"HTTP/a.1"), &None, "alpha")
    }

    #[test]
    fn media_types() -> Result<(), String> {
        check(
            &media_type("Application/X-WWW-Form-Urlencoded; charset=UTF-8"),
            &("application/x-www-form-urlencoded".to_owned(), None),
            "params",
        )?;
        check(
            &media_type("text"),
            &("text".to_owned(), None),
            "no subtype",
        )?;
        check(
            &media_type("text/"),
            &(String::new(), Some("mime: expected token after slash")),
            "empty subtype",
        )?;
        check(
            &media_type("text/plain; x"),
            &(
                "text/plain".to_owned(),
                Some("mime: invalid media parameter"),
            ),
            "bad parameter",
        )
    }

    #[test]
    fn cookies_like_go() -> Result<(), String> {
        // Go: net/http/cookie_test.go readCookiesTests (request side).
        let mut h = Header::new();
        h.insert_raw(
            "Cookie",
            vec![
                "Cookie-1=v$1; c2=v2".to_owned(),
                "c3=\"v3\"; bad\"=x; ; c4= ".to_owned(),
            ],
        );
        let got: Vec<(String, String, bool)> = read_cookies(&h, "")
            .into_iter()
            .map(|c| (c.name, c.value, c.quoted))
            .collect();
        check(
            &got,
            &vec![
                ("Cookie-1".to_owned(), "v$1".to_owned(), false),
                ("c2".to_owned(), "v2".to_owned(), false),
                ("c3".to_owned(), "v3".to_owned(), true),
                ("c4".to_owned(), String::new(), false),
            ],
            "all",
        )?;
        check(&read_cookies(&h, "c2").len(), &1, "filter")
    }
}
