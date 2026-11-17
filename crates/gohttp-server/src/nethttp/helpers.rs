//! Go: the helpers of `server.go` handlers use (`Error`, `NotFound`, `Redirect`,
//! `StripPrefix`, `RedirectHandler`, `NotFoundHandler`) and the `path` functions they rely on.

use core::mem;

use pbs_gocompat::url::Url;

use super::{
    BoxFuture,
    lex::hex_escape_non_ascii,
    request::Request,
    response::{Handler, ResponseWriter},
    status::{STATUS_NOT_FOUND, status_text},
};

/// Go: `path.Clean`: the shortest lexically equivalent path (`""` becomes `"."`).
#[must_use]
pub fn clean_path(path: &str) -> String {
    let p = path.as_bytes();
    if p.is_empty() {
        return ".".to_owned();
    }
    let rooted = p.first() == Some(&b'/');
    let n = p.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let (mut r, mut dotdot) = (0, 0);
    if rooted {
        out.push(b'/');
        r = 1;
        dotdot = 1;
    }
    let at = |i: usize| p.get(i).copied();
    while r < n {
        let c = at(r);
        if c == Some(b'/') || (c == Some(b'.') && (r + 1 == n || at(r + 1) == Some(b'/'))) {
            // Empty or `.` element.
            r += 1;
        } else if c == Some(b'.')
            && at(r + 1) == Some(b'.')
            && (r + 2 == n || at(r + 2) == Some(b'/'))
        {
            r += 2;
            if out.len() > dotdot {
                // Go: back up to (and over) the previous `/`.
                let mut removed = out.pop();
                while out.len() > dotdot && removed != Some(b'/') {
                    removed = out.pop();
                }
            } else if !rooted {
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(b'/');
            }
            while r < n && at(r) != Some(b'/') {
                if let Some(ch) = at(r) {
                    out.push(ch);
                }
                r += 1;
            }
        }
    }
    if out.is_empty() {
        return ".".to_owned();
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Go: `path.Split`: everything up to and including the last `/`, and the rest.
pub(super) fn split_path(path: &str) -> (&str, &str) {
    path.rfind('/').map_or(("", path), |i| path.split_at(i + 1))
}

/// Go: `path.Base`.
pub(super) fn base_path(path: &str) -> &str {
    if path.is_empty() {
        return ".";
    }
    let trimmed = path.trim_end_matches('/');
    let base = trimmed
        .rfind('/')
        .map_or(trimmed, |i| trimmed.get(i + 1..).unwrap_or_default());
    if base.is_empty() { "/" } else { base }
}

/// Go: `htmlEscape` (`htmlReplacer`).
#[must_use]
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Go: `http.Error(w, error, code)`: a `text/plain` reply with `error` and a newline.
pub async fn error(w: &mut dyn ResponseWriter, error: &str, code: u16) {
    let h = w.header();
    h.del("Content-Length");
    h.set("Content-Type", "text/plain; charset=utf-8");
    h.set("X-Content-Type-Options", "nosniff");
    w.write_header(code);
    let mut body = String::with_capacity(error.len() + 1);
    body.push_str(error);
    body.push('\n');
    let _written = w.write(body.as_bytes()).await;
}

/// Go: `http.NotFound`.
pub async fn not_found(w: &mut dyn ResponseWriter) {
    error(w, "404 page not found", STATUS_NOT_FOUND).await;
}

/// Go: `http.Redirect(w, r, url, code)`.
pub async fn redirect(w: &mut dyn ResponseWriter, r: &Request, url: &str, code: u16) {
    let mut url = url.to_owned();
    if let Ok(u) = Url::parse(&url)
        && u.scheme.is_empty()
        && u.host.is_empty()
    {
        let mut oldpath = r.url.escaped_path();
        if oldpath.is_empty() {
            oldpath.push('/');
        }
        if !url.starts_with('/') {
            let (olddir, _) = split_path(&oldpath);
            url.insert_str(0, olddir);
        }
        let query = url.find('?').map_or_else(String::new, |i| url.split_off(i));
        let trailing = url.ends_with('/');
        url = clean_path(&url);
        if trailing && !url.ends_with('/') {
            url.push('/');
        }
        url.push_str(&query);
    }
    let h = w.header();
    let had_ct = h.get_raw("Content-Type").is_some();
    h.set("Location", hex_escape_non_ascii(&url));
    if !had_ct && (r.method == "GET" || r.method == "HEAD") {
        h.set("Content-Type", "text/html; charset=utf-8");
    }
    w.write_header(code);
    if !had_ct && r.method == "GET" {
        let body = format!(
            "<a href=\"{}\">{}</a>.\n\n",
            html_escape(&url),
            status_text(code)
        );
        let _written = w.write(body.as_bytes()).await;
    }
}

/// Go: `http.NotFoundHandler()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NotFoundHandler;

impl Handler for NotFoundHandler {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        _r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        Box::pin(not_found(w))
    }
}

/// Go: `http.RedirectHandler(url, code)`.
#[derive(Debug, Clone)]
pub struct RedirectHandler {
    url: String,
    code: u16,
}

impl RedirectHandler {
    /// Go: `http.RedirectHandler(url, code)`.
    #[must_use]
    pub fn new<U: Into<String>>(url: U, code: u16) -> Self {
        Self {
            url: url.into(),
            code,
        }
    }
}

impl Handler for RedirectHandler {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        Box::pin(redirect(w, r, &self.url, self.code))
    }
}

/// Go: `http.StripPrefix(prefix, h)`.
///
/// Serves `h` with `prefix` removed from the URL's `Path` and `RawPath`, or a 404 when the path
/// does not start with it. (Go hands `h` a shallow copy of the request; here the request's URL
/// is changed for the call and restored after it.)
#[derive(Debug, Clone)]
pub struct StripPrefix<H> {
    prefix: String,
    handler: H,
}

impl<H: Handler> StripPrefix<H> {
    /// Go: `http.StripPrefix(prefix, h)` (an empty prefix serves `h` unchanged).
    pub fn new<P: Into<String>>(prefix: P, handler: H) -> Self {
        Self {
            prefix: prefix.into(),
            handler,
        }
    }
}

impl<H: Handler> Handler for StripPrefix<H> {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if self.prefix.is_empty() {
                return self.handler.serve_http(w, r).await;
            }
            let p = r.url.path.strip_prefix(self.prefix.as_str());
            let rp = r.url.raw_path.strip_prefix(self.prefix.as_str());
            let path_ok = p.is_some_and(|p| p.len() < r.url.path.len());
            let raw_ok =
                r.url.raw_path.is_empty() || rp.is_some_and(|rp| rp.len() < r.url.raw_path.len());
            if !(path_ok && raw_ok) {
                return not_found(w).await;
            }
            let new_path = p.unwrap_or_default().to_owned();
            let new_raw = rp.map_or_else(|| r.url.raw_path.clone(), str::to_owned);
            let saved_path = mem::replace(&mut r.url.path, new_path);
            let saved_raw = mem::replace(&mut r.url.raw_path, new_raw);
            self.handler.serve_http(w, r).await;
            r.url.path = saved_path;
            r.url.raw_path = saved_raw;
        })
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Debug;

    use super::{base_path, clean_path, split_path};

    fn check<T: PartialEq + Debug>(got: &T, want: &T, name: &str) -> Result<(), String> {
        if got == want {
            Ok(())
        } else {
            Err(format!("{name}: got {got:?}, want {want:?}"))
        }
    }

    #[test]
    fn clean_like_go() -> Result<(), String> {
        // Go: path/path_test.go cleantests.
        for (input, want) in [
            ("", "."),
            ("abc", "abc"),
            ("abc/def", "abc/def"),
            ("a/b/c", "a/b/c"),
            (".", "."),
            ("..", ".."),
            ("../..", "../.."),
            ("../../abc", "../../abc"),
            ("/abc", "/abc"),
            ("/", "/"),
            ("abc/", "abc"),
            ("abc/def/", "abc/def"),
            ("/abc/", "/abc"),
            ("abc//def//ghi", "abc/def/ghi"),
            ("//abc", "/abc"),
            ("///abc", "/abc"),
            ("//abc//", "/abc"),
            ("abc//", "abc"),
            ("abc/./def", "abc/def"),
            ("/./abc/def", "/abc/def"),
            ("abc/.", "abc"),
            ("abc/def/ghi/../jkl", "abc/def/jkl"),
            ("abc/def/../ghi/../jkl", "abc/jkl"),
            ("abc/def/..", "abc"),
            ("abc/def/../..", "."),
            ("/abc/def/../..", "/"),
            ("abc/def/../../..", ".."),
            ("/abc/def/../../..", "/"),
            ("abc/def/../../../ghi/jkl/../../../mno", "../../mno"),
            ("abc/./../def", "def"),
            ("abc//./../def", "def"),
            ("abc/../../././../def", "../../def"),
        ] {
            check(&clean_path(input), &want.to_owned(), input)?;
        }
        Ok(())
    }

    #[test]
    fn split_and_base() -> Result<(), String> {
        check(
            &split_path("static/myfile.css"),
            &("static/", "myfile.css"),
            "split",
        )?;
        check(
            &split_path("myfile.css"),
            &("", "myfile.css"),
            "split no dir",
        )?;
        check(&base_path("/a/b/"), &"b", "base")?;
        check(&base_path("///"), &"/", "base slashes")?;
        check(&base_path(""), &".", "base empty")
    }
}
