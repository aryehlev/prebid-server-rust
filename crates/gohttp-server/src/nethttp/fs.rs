//! Go: `net/http/fs.go`: `http.Dir`, `http.FileServer`, `http.ServeFile` and the content
//! serving they share (`Last-Modified`, conditional requests, byte ranges, directory listings,
//! `index.html`, redirects to canonical paths).

use core::{
    fmt::Write as _,
    hash::{BuildHasher as _, Hasher as _},
};
use std::{
    collections::hash_map::RandomState,
    io,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use pbs_gocompat::{
    http::trim_string,
    strconv::parse_int,
    time::{ANSIC, RFC850, Time, parse},
    url::Url,
};
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt as _, AsyncSeekExt as _},
};

use super::{
    BoxFuture,
    helpers::{base_path, clean_path, error, html_escape},
    mime::type_by_extension,
    request::{Request, first_raw},
    response::{Handler, ResponseWriter, TIME_FORMAT},
    sniff::detect_content_type,
    status::{
        STATUS_INTERNAL_SERVER_ERROR, STATUS_MOVED_PERMANENTLY, STATUS_NOT_MODIFIED, STATUS_OK,
        STATUS_PARTIAL_CONTENT, STATUS_PRECONDITION_FAILED, STATUS_REQUESTED_RANGE_NOT_SATISFIABLE,
    },
};

/// Go: `sniffLen`.
const SNIFF_LEN: usize = 512;

/// Go: `http.Dir`: serves the files under a directory of the local file system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dir(pub String);

/// Why opening or reading a file failed (Go's `toHTTPError` classes).
#[derive(Debug)]
enum FsError {
    NotExist,
    Permission,
    InvalidPath,
    Other,
}

impl FsError {
    fn from_io(e: &io::Error) -> Self {
        let kind = e.kind();
        if kind == io::ErrorKind::NotFound {
            Self::NotExist
        } else if kind == io::ErrorKind::PermissionDenied {
            Self::Permission
        } else {
            Self::Other
        }
    }

    /// Go: `toHTTPError`.
    const fn http_error(&self) -> (&'static str, u16) {
        match self {
            Self::NotExist | Self::InvalidPath => ("404 page not found", 404),
            Self::Permission => ("403 Forbidden", 403),
            Self::Other => ("500 Internal Server Error", 500),
        }
    }
}

/// An open file or directory with what `Stat` reported.
struct Opened {
    file: File,
    full: String,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
}

impl Opened {
    /// Go: `fs.FileInfo.Name()`.
    fn name(&self) -> &str {
        base_path(&self.full)
    }
}

/// Go: `mapOpenError`.
async fn map_open_error(err: &io::Error, full: &str) -> FsError {
    let original = FsError::from_io(err);
    if matches!(original, FsError::NotExist | FsError::Permission) {
        return original;
    }
    let parts: Vec<&str> = full.split('/').collect();
    for i in 0..parts.len() {
        if parts.get(i).is_some_and(|p| p.is_empty()) {
            continue;
        }
        let prefix = parts.get(..=i).unwrap_or_default().join("/");
        match fs::metadata(&prefix).await {
            Err(_) => return original,
            Ok(m) if !m.is_dir() => return FsError::NotExist,
            Ok(_) => {}
        }
    }
    original
}

impl Dir {
    /// Go: `Dir.Open(name)`.
    async fn open(&self, name: &str) -> Result<Opened, FsError> {
        let cleaned = clean_path(&format!("/{name}"));
        let mut path = cleaned.get(1..).unwrap_or_default().to_owned();
        if path.is_empty() {
            path.push('.');
        }
        if path.contains('\0') {
            return Err(FsError::InvalidPath);
        }
        let dir = if self.0.is_empty() {
            "."
        } else {
            self.0.as_str()
        };
        let full = clean_path(&format!("{dir}/{path}"));
        let file = match File::open(Path::new(&full)).await {
            Ok(f) => f,
            Err(e) => return Err(map_open_error(&e, &full).await),
        };
        let meta = file.metadata().await.map_err(|e| FsError::from_io(&e))?;
        Ok(Opened {
            file,
            full,
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// Go: a file's `ModTime()` as a `time.Time` (`None` for the zero time).
fn go_time(t: Option<SystemTime>) -> Option<Time> {
    t.map(Time::from_system_time)
}

/// Go: `isZeroTime`.
fn is_zero_time(t: Option<&Time>) -> bool {
    t.is_none_or(|t| t.unix() == 0 && t.nanosecond() == 0)
}

/// Go: `http.ParseTime`.
fn parse_http_time(text: &str) -> Option<Time> {
    [TIME_FORMAT, RFC850, ANSIC]
        .iter()
        .find_map(|layout| parse(layout, text).ok())
}

/// Go: `condResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cond {
    None,
    True,
    False,
}

/// Go: `scanETag`.
fn scan_etag(s: &str) -> Option<(&str, &str)> {
    let s = trim_string(s);
    let start = if s.starts_with("W/") { 2 } else { 0 };
    let tail = s.get(start..).unwrap_or_default();
    if tail.len() < 2 || !tail.starts_with('"') {
        return None;
    }
    for (i, c) in s.bytes().enumerate().skip(start + 1) {
        match c {
            0x21 | 0x23..=0x7E | 0x80..=0xFF => {}
            b'"' => return Some((s.get(..=i)?, s.get(i + 1..)?)),
            _ => return None,
        }
    }
    None
}

/// Go: `etagStrongMatch`.
fn etag_strong_match(a: &str, b: &str) -> bool {
    a == b && a.starts_with('"')
}

/// Go: `etagWeakMatch`.
fn etag_weak_match(a: &str, b: &str) -> bool {
    a.strip_prefix("W/").unwrap_or(a) == b.strip_prefix("W/").unwrap_or(b)
}

/// Go: `checkIfMatch`.
fn check_if_match(w: &mut dyn ResponseWriter, r: &Request) -> Cond {
    let mut im = r.header.get("If-Match");
    if im.is_empty() {
        return Cond::None;
    }
    loop {
        im = trim_string(im);
        if im.is_empty() {
            break;
        }
        if let Some(rest) = im.strip_prefix(',') {
            im = rest;
            continue;
        }
        if im.starts_with('*') {
            return Cond::True;
        }
        let Some((etag, remain)) = scan_etag(im) else {
            break;
        };
        if etag_strong_match(etag, first_raw(w.header(), "Etag")) {
            return Cond::True;
        }
        im = remain;
    }
    Cond::False
}

/// Go: `checkIfUnmodifiedSince`.
fn check_if_unmodified_since(r: &Request, modtime: Option<&Time>) -> Cond {
    let ius = r.header.get("If-Unmodified-Since");
    if ius.is_empty() || is_zero_time(modtime) {
        return Cond::None;
    }
    let (Some(t), Some(m)) = (parse_http_time(ius), modtime) else {
        return Cond::None;
    };
    if m.unix() <= t.unix() {
        Cond::True
    } else {
        Cond::False
    }
}

/// Go: `checkIfNoneMatch`.
fn check_if_none_match(w: &mut dyn ResponseWriter, r: &Request) -> Cond {
    let inm = first_raw(&r.header, "If-None-Match");
    if inm.is_empty() {
        return Cond::None;
    }
    let mut buf = inm;
    loop {
        buf = trim_string(buf);
        if buf.is_empty() {
            break;
        }
        if let Some(rest) = buf.strip_prefix(',') {
            buf = rest;
            continue;
        }
        if buf.starts_with('*') {
            return Cond::False;
        }
        let Some((etag, remain)) = scan_etag(buf) else {
            break;
        };
        if etag_weak_match(etag, first_raw(w.header(), "Etag")) {
            return Cond::False;
        }
        buf = remain;
    }
    Cond::True
}

/// Go: `checkIfModifiedSince`.
fn check_if_modified_since(r: &Request, modtime: Option<&Time>) -> Cond {
    if r.method != "GET" && r.method != "HEAD" {
        return Cond::None;
    }
    let ims = r.header.get("If-Modified-Since");
    if ims.is_empty() || is_zero_time(modtime) {
        return Cond::None;
    }
    let (Some(t), Some(m)) = (parse_http_time(ims), modtime) else {
        return Cond::None;
    };
    if m.unix() <= t.unix() {
        Cond::False
    } else {
        Cond::True
    }
}

/// Go: `checkIfRange`.
fn check_if_range(w: &mut dyn ResponseWriter, r: &Request, modtime: Option<&Time>) -> Cond {
    if r.method != "GET" && r.method != "HEAD" {
        return Cond::None;
    }
    let ir = first_raw(&r.header, "If-Range");
    if ir.is_empty() {
        return Cond::None;
    }
    if let Some((etag, _)) = scan_etag(ir) {
        return if etag_strong_match(etag, w.header().get("Etag")) {
            Cond::True
        } else {
            Cond::False
        };
    }
    let Some(m) = modtime else {
        return Cond::False;
    };
    match parse_http_time(ir) {
        Some(t) if t.unix() == m.unix() => Cond::True,
        Some(_) | None => Cond::False,
    }
}

/// Go: `setLastModified`.
fn set_last_modified(w: &mut dyn ResponseWriter, modtime: Option<&Time>) {
    if let Some(m) = modtime
        && !is_zero_time(Some(m))
    {
        w.header().set("Last-Modified", m.utc().format(TIME_FORMAT));
    }
}

/// Go: `writeNotModified`.
fn write_not_modified(w: &mut dyn ResponseWriter) {
    let h = w.header();
    let _ct = h.remove_raw("Content-Type");
    let _cl = h.remove_raw("Content-Length");
    let _ce = h.remove_raw("Content-Encoding");
    if !h.get("Etag").is_empty() {
        let _lm = h.remove_raw("Last-Modified");
    }
    w.write_header(STATUS_NOT_MODIFIED);
}

/// Go: `checkPreconditions`: `None` when the reply is complete, else the `Range` header to
/// honour.
fn check_preconditions(
    w: &mut dyn ResponseWriter,
    r: &Request,
    modtime: Option<&Time>,
) -> Option<String> {
    let mut ch = check_if_match(w, r);
    if ch == Cond::None {
        ch = check_if_unmodified_since(r, modtime);
    }
    if ch == Cond::False {
        w.write_header(STATUS_PRECONDITION_FAILED);
        return None;
    }
    match check_if_none_match(w, r) {
        Cond::False => {
            if r.method == "GET" || r.method == "HEAD" {
                write_not_modified(w);
            } else {
                w.write_header(STATUS_PRECONDITION_FAILED);
            }
            return None;
        }
        Cond::None => {
            if check_if_modified_since(r, modtime) == Cond::False {
                write_not_modified(w);
                return None;
            }
        }
        Cond::True => {}
    }
    let mut range = first_raw(&r.header, "Range").to_owned();
    if !range.is_empty() && check_if_range(w, r, modtime) == Cond::False {
        range.clear();
    }
    Some(range)
}

/// Go: `serveError`.
async fn serve_error(w: &mut dyn ResponseWriter, text: &str, code: u16) {
    let h = w.header();
    for k in ["Cache-Control", "Content-Encoding", "Etag", "Last-Modified"] {
        if h.get_raw(k).is_some() {
            h.del(k);
        }
    }
    error(w, text, code).await;
}

/// Go: `httpRange`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HttpRange {
    start: i64,
    length: i64,
}

impl HttpRange {
    /// Go: `contentRange`.
    fn content_range(self, size: i64) -> String {
        format!(
            "bytes {}-{}/{size}",
            self.start,
            self.start + self.length - 1
        )
    }
}

/// Go: `parseRange`'s errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeError {
    Invalid,
    NoOverlap,
}

/// Go: `parseRange`.
fn parse_range(s: &str, size: i64) -> Result<Vec<HttpRange>, RangeError> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let Some(spec) = s.strip_prefix("bytes=") else {
        return Err(RangeError::Invalid);
    };
    let mut ranges = Vec::new();
    let mut no_overlap = false;
    for ra in spec.split(',') {
        let ra = trim_string(ra);
        if ra.is_empty() {
            continue;
        }
        let Some((start, end)) = ra.split_once('-') else {
            return Err(RangeError::Invalid);
        };
        let (start, end) = (trim_string(start), trim_string(end));
        let r = if start.is_empty() {
            if end.is_empty() || end.starts_with('-') {
                return Err(RangeError::Invalid);
            }
            let mut i = parse_int(end, 10, 64).map_err(|_parse| RangeError::Invalid)?;
            if i < 0 {
                return Err(RangeError::Invalid);
            }
            i = i.min(size);
            HttpRange {
                start: size - i,
                length: i,
            }
        } else {
            let i = parse_int(start, 10, 64).map_err(|_parse| RangeError::Invalid)?;
            if i < 0 {
                return Err(RangeError::Invalid);
            }
            if i >= size {
                no_overlap = true;
                continue;
            }
            if end.is_empty() {
                HttpRange {
                    start: i,
                    length: size - i,
                }
            } else {
                let mut e = parse_int(end, 10, 64).map_err(|_parse| RangeError::Invalid)?;
                if i > e {
                    return Err(RangeError::Invalid);
                }
                e = e.min(size - 1);
                HttpRange {
                    start: i,
                    length: e - i + 1,
                }
            }
        };
        ranges.push(r);
    }
    if no_overlap && ranges.is_empty() {
        return Err(RangeError::NoOverlap);
    }
    Ok(ranges)
}

/// Go: `multipart.randomBoundary` (60 hexadecimal digits).
fn random_boundary() -> String {
    let state = RandomState::new();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut out = String::with_capacity(64);
    let mut i: u64 = 0;
    while out.len() < 60 {
        let mut h = state.build_hasher();
        h.write_u128(nanos);
        h.write_u64(i);
        let _ok = write!(out, "{:016x}", h.finish());
        i += 1;
    }
    out.truncate(60);
    out
}

/// Go: the part headers `multipart.Writer.CreatePart` writes for a range.
fn part_header(first: bool, boundary: &str, ra: HttpRange, ctype: &str, size: i64) -> String {
    format!(
        "{}--{boundary}\r\nContent-Range: {}\r\nContent-Type: {ctype}\r\n\r\n",
        if first { "" } else { "\r\n" },
        ra.content_range(size)
    )
}

/// Go: the closing delimiter `multipart.Writer.Close` writes.
fn closing_delimiter(boundary: &str) -> String {
    format!("\r\n--{boundary}--\r\n")
}

/// Go: `io.CopyN(w, f, n)` (through `writerOnly{w}` and a 32 KB buffer).
async fn copy_n(w: &mut dyn ResponseWriter, f: &mut File, n: i64) {
    let mut left = u64::try_from(n).unwrap_or(0);
    let mut buf = vec![0_u8; 32 << 10];
    while left > 0 {
        let want = usize::try_from(left).map_or(buf.len(), |l| l.min(buf.len()));
        let Some(dst) = buf.get_mut(..want) else {
            return;
        };
        let got = match f.read(dst).await {
            Ok(0) | Err(_) => return,
            Ok(got) => got,
        };
        if w.write(buf.get(..got).unwrap_or_default()).await.is_err() {
            return;
        }
        left -= u64::try_from(got).unwrap_or(left);
    }
}

/// Go: the `Content-Type` part of `serveContent`: the declared type, else the type of the
/// name's extension, else the sniffed type (set on the header). `None` when the file could not
/// be rewound after sniffing (the error reply is written).
async fn content_type(w: &mut dyn ResponseWriter, name: &str, f: &mut File) -> Option<String> {
    if let Some(v) = w.header().get_raw("Content-Type") {
        return Some(v.first().cloned().unwrap_or_default());
    }
    let ext = name
        .rfind(['.', '/'])
        .filter(|&i| name.as_bytes().get(i) == Some(&b'.'))
        .map_or("", |i| name.get(i..).unwrap_or_default());
    let mut ctype = type_by_extension(ext);
    if ctype.is_empty() {
        let mut buf = [0_u8; SNIFF_LEN];
        let mut n = 0;
        while n < SNIFF_LEN {
            match f.read(buf.get_mut(n..).unwrap_or_default()).await {
                Ok(0) | Err(_) => break,
                Ok(got) => n += got,
            }
        }
        detect_content_type(buf.get(..n).unwrap_or_default()).clone_into(&mut ctype);
        if f.seek(io::SeekFrom::Start(0)).await.is_err() {
            serve_error(w, "seeker can't seek", STATUS_INTERNAL_SERVER_ERROR).await;
            return None;
        }
    }
    w.header().set("Content-Type", ctype.clone());
    Some(ctype)
}

/// Go: `serveContent`'s range parsing: the ranges to send, or `None` when the error reply is
/// written.
async fn content_ranges(
    w: &mut dyn ResponseWriter,
    range_req: &str,
    size: i64,
) -> Option<Vec<HttpRange>> {
    let ranges = match parse_range(range_req, size) {
        Ok(parsed) => parsed,
        Err(RangeError::NoOverlap) if size == 0 => Vec::new(),
        Err(RangeError::NoOverlap) => {
            w.header().set("Content-Range", format!("bytes */{size}"));
            serve_error(
                w,
                "invalid range: failed to overlap",
                STATUS_REQUESTED_RANGE_NOT_SATISFIABLE,
            )
            .await;
            return None;
        }
        Err(RangeError::Invalid) => {
            serve_error(w, "invalid range", STATUS_REQUESTED_RANGE_NOT_SATISFIABLE).await;
            return None;
        }
    };
    if ranges.iter().map(|ra| ra.length).sum::<i64>() > size {
        return Some(Vec::new());
    }
    Some(ranges)
}

/// Go: the goroutine of `serveContent` writing a `multipart/byteranges` body.
async fn write_multipart(
    w: &mut dyn ResponseWriter,
    f: &mut File,
    ranges: &[HttpRange],
    boundary: &str,
    ctype: &str,
    size: i64,
) {
    for (i, ra) in ranges.iter().enumerate() {
        let header = part_header(i == 0, boundary, *ra, ctype, size);
        if w.write(header.as_bytes()).await.is_err() {
            return;
        }
        if f.seek(io::SeekFrom::Start(u64::try_from(ra.start).unwrap_or(0)))
            .await
            .is_err()
        {
            return;
        }
        copy_n(w, f, ra.length).await;
    }
    let _written = w.write(closing_delimiter(boundary).as_bytes()).await;
}

/// Go: `serveContent` for a file of `size` bytes.
async fn serve_content(
    w: &mut dyn ResponseWriter,
    r: &Request,
    name: &str,
    modtime: Option<&Time>,
    size: i64,
    f: &mut File,
) {
    set_last_modified(w, modtime);
    let Some(range_req) = check_preconditions(w, r, modtime) else {
        return;
    };
    let Some(ctype) = content_type(w, name, f).await else {
        return;
    };
    let Some(ranges) = content_ranges(w, &range_req, size).await else {
        return;
    };
    let mut code = STATUS_OK;
    let mut send_size = size;
    let mut boundary = None;
    match ranges.as_slice() {
        [ra] => {
            if f.seek(io::SeekFrom::Start(u64::try_from(ra.start).unwrap_or(0)))
                .await
                .is_err()
            {
                serve_error(w, "seek error", STATUS_REQUESTED_RANGE_NOT_SATISFIABLE).await;
                return;
            }
            send_size = ra.length;
            code = STATUS_PARTIAL_CONTENT;
            w.header().set("Content-Range", ra.content_range(size));
        }
        [] => {}
        many => {
            let b = random_boundary();
            let mut total: i64 = 0;
            for (i, ra) in many.iter().enumerate() {
                total += i64::try_from(part_header(i == 0, &b, *ra, &ctype, size).len())
                    .unwrap_or(0)
                    + ra.length;
            }
            total += i64::try_from(closing_delimiter(&b).len()).unwrap_or(0);
            send_size = total;
            code = STATUS_PARTIAL_CONTENT;
            w.header().set(
                "Content-Type",
                format!("multipart/byteranges; boundary={b}"),
            );
            boundary = Some(b);
        }
    }
    w.header().set("Accept-Ranges", "bytes");
    if !ranges.is_empty() || w.header().get("Content-Encoding").is_empty() {
        w.header().set("Content-Length", send_size.to_string());
    }
    w.write_header(code);
    if r.method == "HEAD" {
        return;
    }
    match boundary {
        None => copy_n(w, f, send_size).await,
        Some(b) => write_multipart(w, f, &ranges, &b, &ctype, size).await,
    }
}

/// Go: `localRedirect`.
fn local_redirect(w: &mut dyn ResponseWriter, r: &Request, new_path: &str) {
    let mut target = new_path.to_owned();
    if !r.url.raw_query.is_empty() {
        target.push('?');
        target.push_str(&r.url.raw_query);
    }
    w.header().set("Location", target);
    w.write_header(STATUS_MOVED_PERMANENTLY);
}

/// Go: `dirList`.
async fn dir_list(w: &mut dyn ResponseWriter, full: &str) {
    let mut entries: Vec<(Vec<u8>, bool)> = Vec::new();
    let read = async {
        let mut rd = fs::read_dir(full).await?;
        while let Some(entry) = rd.next_entry().await? {
            let is_dir = entry.file_type().await.is_ok_and(|t| t.is_dir());
            entries.push((entry.file_name().into_encoded_bytes(), is_dir));
        }
        Ok::<(), io::Error>(())
    };
    if read.await.is_err() {
        error(w, "Error reading directory", STATUS_INTERNAL_SERVER_ERROR).await;
        return;
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    w.header().set("Content-Type", "text/html; charset=utf-8");
    let _head = w.write(b"<!doctype html>\n").await;
    let _meta = w
        .write(b"<meta name=\"viewport\" content=\"width=device-width\">\n")
        .await;
    let _pre = w.write(b"<pre>\n").await;
    for (name, is_dir) in entries {
        let mut name = String::from_utf8_lossy(&name).into_owned();
        if is_dir {
            name.push('/');
        }
        let url = Url {
            path: name.clone(),
            ..Url::default()
        };
        let line = format!("<a href=\"{url}\">{}</a>\n", html_escape(&name));
        let _line = w.write(line.as_bytes()).await;
    }
    let _end = w.write(b"</pre>\n").await;
}

/// Go: `serveFile(w, r, fs, name, redirect)`.
async fn serve_file_inner(
    w: &mut dyn ResponseWriter,
    r: &Request,
    fs: &Dir,
    name: &str,
    redirect: bool,
) {
    const INDEX_PAGE: &str = "/index.html";
    if r.url.path.ends_with(INDEX_PAGE) {
        local_redirect(w, r, "./");
        return;
    }
    let mut opened = match fs.open(name).await {
        Ok(o) => o,
        Err(e) => {
            let (msg, code) = e.http_error();
            serve_error(w, msg, code).await;
            return;
        }
    };
    if redirect {
        let url = &r.url.path;
        if opened.is_dir {
            if !url.ends_with('/') {
                let target = format!("{}/", base_path(url));
                local_redirect(w, r, &target);
                return;
            }
        } else if url.ends_with('/') {
            let base = base_path(url);
            if base == "/" || base == "." {
                serve_error(
                    w,
                    "http: attempting to traverse a non-directory",
                    STATUS_INTERNAL_SERVER_ERROR,
                )
                .await;
                return;
            }
            let target = format!("../{base}");
            local_redirect(w, r, &target);
            return;
        }
    }
    if opened.is_dir {
        let url = &r.url.path;
        if !url.ends_with('/') {
            let target = format!("{}/", base_path(url));
            local_redirect(w, r, &target);
            return;
        }
        let index = format!("{}{INDEX_PAGE}", name.strip_suffix('/').unwrap_or(name));
        if let Ok(ff) = fs.open(&index).await {
            opened = ff;
        }
    }
    let modtime = go_time(opened.modified);
    if opened.is_dir {
        if check_if_modified_since(r, modtime.as_ref()) == Cond::False {
            write_not_modified(w);
            return;
        }
        set_last_modified(w, modtime.as_ref());
        dir_list(w, &opened.full).await;
        return;
    }
    let size = i64::try_from(opened.size).unwrap_or(i64::MAX);
    let base = opened.name().to_owned();
    serve_content(w, r, &base, modtime.as_ref(), size, &mut opened.file).await;
}

/// Go: `containsDotDot`.
fn contains_dot_dot(v: &str) -> bool {
    v.contains("..") && v.split(['/', '\\']).any(|ent| ent == "..")
}

/// Go: `http.ServeFile(w, r, name)`.
pub async fn serve_file(w: &mut dyn ResponseWriter, r: &Request, name: &str) {
    if contains_dot_dot(&r.url.path) {
        serve_error(w, "invalid URL path", 400).await;
        return;
    }
    let (dir, file) = name.rfind('/').map_or(("", name), |i| name.split_at(i + 1));
    serve_file_inner(w, r, &Dir(dir.to_owned()), file, false).await;
}

/// Go: `http.FileServer(root)`.
#[derive(Debug, Clone)]
pub struct FileServer {
    root: Dir,
}

impl FileServer {
    /// Go: `http.FileServer(root)`.
    #[must_use]
    pub const fn new(root: Dir) -> Self {
        Self { root }
    }
}

impl Handler for FileServer {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if !r.url.path.starts_with('/') {
                r.url.path.insert(0, '/');
            }
            let name = clean_path(&r.url.path);
            serve_file_inner(w, r, &self.root, &name, true).await;
        })
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Debug;

    use super::{HttpRange, RangeError, contains_dot_dot, parse_range, scan_etag};

    fn check<T: PartialEq + Debug>(got: &T, want: &T, name: &str) -> Result<(), String> {
        if got == want {
            Ok(())
        } else {
            Err(format!("{name}: got {got:?}, want {want:?}"))
        }
    }

    const fn r(start: i64, length: i64) -> HttpRange {
        HttpRange { start, length }
    }

    #[test]
    fn parse_range_like_go() -> Result<(), String> {
        // Go: net/http/range_test.go ParseRangeTests (a selection).
        check(&parse_range("", 0), &Ok(vec![]), "empty")?;
        check(&parse_range("bytes=", 0), &Ok(vec![]), "bytes=")?;
        check(&parse_range("foo", 0), &Err(RangeError::Invalid), "foo")?;
        check(
            &parse_range("bytes=x-1", 10),
            &Err(RangeError::Invalid),
            "x-1",
        )?;
        check(&parse_range("bytes=0-", 10), &Ok(vec![r(0, 10)]), "0-")?;
        check(&parse_range("bytes=2-", 10), &Ok(vec![r(2, 8)]), "2-")?;
        check(&parse_range("bytes=-5", 10), &Ok(vec![r(5, 5)]), "-5")?;
        check(
            &parse_range("bytes=0-1,5-8", 10),
            &Ok(vec![r(0, 2), r(5, 4)]),
            "multi",
        )?;
        check(
            &parse_range("bytes=0-0,-1", 10),
            &Ok(vec![r(0, 1), r(9, 1)]),
            "suffix",
        )?;
        check(
            &parse_range("bytes=10-20", 10),
            &Err(RangeError::NoOverlap),
            "no overlap",
        )?;
        check(
            &parse_range("bytes=5-4", 10),
            &Err(RangeError::Invalid),
            "reversed",
        )?;
        check(
            &parse_range("bytes=--6", 10),
            &Err(RangeError::Invalid),
            "--6",
        )?;
        check(
            &parse_range("bytes=0-2,1-", 10),
            &Ok(vec![r(0, 3), r(1, 9)]),
            "overlap",
        )
    }

    #[test]
    fn etags_and_dot_dot() -> Result<(), String> {
        check(
            &scan_etag("\"abc\", W/\"x\""),
            &Some(("\"abc\"", ", W/\"x\"")),
            "strong",
        )?;
        check(&scan_etag("W/\"x\""), &Some(("W/\"x\"", "")), "weak")?;
        check(&scan_etag("abc"), &None, "bare")?;
        check(&contains_dot_dot("/a/../b"), &true, "dotdot")?;
        check(&contains_dot_dot("/a..b/"), &false, "inner dots")
    }
}
