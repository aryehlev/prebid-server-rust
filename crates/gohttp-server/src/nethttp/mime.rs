//! Go: the parts of the `mime` package `net/http` uses: `TypeByExtension` (the built-in table
//! plus the system tables Go reads on Unix: the shared-mime-info `globs2` database, or else the
//! `mime.types` files), `ParseMediaType` and `FormatMediaType`.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    sync::LazyLock,
};

use pbs_gocompat::strings::{fields, to_lower, trim_space};

/// Go: `builtinTypesLower`.
const BUILTIN_TYPES: &[(&str, &str)] = &[
    (".ai", "application/postscript"),
    (".apk", "application/vnd.android.package-archive"),
    (".apng", "image/apng"),
    (".avif", "image/avif"),
    (".bin", "application/octet-stream"),
    (".bmp", "image/bmp"),
    (".com", "application/octet-stream"),
    (".css", "text/css; charset=utf-8"),
    (".csv", "text/csv; charset=utf-8"),
    (".doc", "application/msword"),
    (
        ".docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    (".ehtml", "text/html; charset=utf-8"),
    (".eml", "message/rfc822"),
    (".eps", "application/postscript"),
    (".exe", "application/octet-stream"),
    (".flac", "audio/flac"),
    (".gif", "image/gif"),
    (".gz", "application/gzip"),
    (".htm", "text/html; charset=utf-8"),
    (".html", "text/html; charset=utf-8"),
    (".ico", "image/vnd.microsoft.icon"),
    (".ics", "text/calendar; charset=utf-8"),
    (".jfif", "image/jpeg"),
    (".jpeg", "image/jpeg"),
    (".jpg", "image/jpeg"),
    (".js", "text/javascript; charset=utf-8"),
    (".json", "application/json"),
    (".m4a", "audio/mp4"),
    (".mjs", "text/javascript; charset=utf-8"),
    (".mp3", "audio/mpeg"),
    (".mp4", "video/mp4"),
    (".oga", "audio/ogg"),
    (".ogg", "audio/ogg"),
    (".ogv", "video/ogg"),
    (".opus", "audio/ogg"),
    (".pdf", "application/pdf"),
    (".pjp", "image/jpeg"),
    (".pjpeg", "image/jpeg"),
    (".png", "image/png"),
    (".ppt", "application/vnd.ms-powerpoint"),
    (
        ".pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    (".ps", "application/postscript"),
    (".rdf", "application/rdf+xml"),
    (".rtf", "application/rtf"),
    (".shtml", "text/html; charset=utf-8"),
    (".svg", "image/svg+xml"),
    (".text", "text/plain; charset=utf-8"),
    (".tif", "image/tiff"),
    (".tiff", "image/tiff"),
    (".txt", "text/plain; charset=utf-8"),
    (".vtt", "text/vtt; charset=utf-8"),
    (".wasm", "application/wasm"),
    (".wav", "audio/wav"),
    (".webm", "audio/webm"),
    (".webp", "image/webp"),
    (".xbl", "text/xml; charset=utf-8"),
    (".xbm", "image/x-xbitmap"),
    (".xht", "application/xhtml+xml"),
    (".xhtml", "application/xhtml+xml"),
    (".xls", "application/vnd.ms-excel"),
    (
        ".xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    (".xml", "text/xml; charset=utf-8"),
    (".xsl", "text/xml; charset=utf-8"),
    (".zip", "application/zip"),
];

/// Go: `mimeGlobs`.
const MIME_GLOBS: &[&str] = &["/usr/local/share/mime/globs2", "/usr/share/mime/globs2"];

/// Go: `typeFiles`.
const TYPE_FILES: &[&str] = &[
    "/etc/mime.types",
    "/etc/apache2/mime.types",
    "/etc/apache/mime.types",
    "/etc/httpd/conf/mime.types",
];

/// Go: `mimeTypes` and `mimeTypesLower`.
#[derive(Debug, Default)]
struct Tables {
    exact: HashMap<String, String>,
    lower: HashMap<String, String>,
}

impl Tables {
    /// Go: `setExtensionType`.
    fn set_extension_type(&mut self, extension: &str, mime_type: &str) {
        let Ok((_, params)) = parse_media_type(mime_type) else {
            return;
        };
        let mut mime_type = mime_type.to_owned();
        if mime_type.starts_with("text/") && params.get("charset").is_none_or(String::is_empty) {
            let mut params = params;
            let _previous = params.insert("charset".to_owned(), "utf-8".to_owned());
            mime_type = format_media_type(&mime_type, &params);
        }
        let _exact = self.exact.insert(extension.to_owned(), mime_type.clone());
        let _lower = self.lower.insert(to_lower(extension), mime_type);
    }

    /// Go: `loadMimeGlobsFile`; `false` when the file cannot be read.
    fn load_globs(&mut self, filename: &str) -> bool {
        let Ok(text) = fs::read(filename) else {
            return false;
        };
        for line in text.split(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(line.strip_suffix(b"\r").unwrap_or(line));
            let fields: Vec<&str> = line.split(':').collect();
            let (Some(weight), Some(typ), Some(glob)) =
                (fields.first(), fields.get(1), fields.get(2))
            else {
                continue;
            };
            if weight.is_empty() || glob.len() < 3 {
                continue;
            }
            if weight.starts_with('#') || !glob.starts_with("*.") {
                continue;
            }
            let extension = glob.get(1..).unwrap_or_default();
            if extension.contains(['?', '*', '[']) {
                continue;
            }
            if self.exact.contains_key(extension) {
                continue;
            }
            self.set_extension_type(extension, typ);
        }
        true
    }

    /// Go: `loadMimeFile`.
    fn load_mime_file(&mut self, filename: &str) {
        let Ok(text) = fs::read(filename) else {
            return;
        };
        for line in text.split(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(line.strip_suffix(b"\r").unwrap_or(line));
            let fields = fields(&line);
            let Some((mime_type, exts)) = fields.split_first() else {
                continue;
            };
            if exts.is_empty() || mime_type.starts_with('#') {
                continue;
            }
            for ext in exts {
                if ext.starts_with('#') {
                    break;
                }
                self.set_extension_type(&format!(".{ext}"), mime_type);
            }
        }
    }
}

/// Go: `initMime` (built-in table, then `initMimeUnix`).
static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    let mut t = Tables::default();
    for (ext, typ) in BUILTIN_TYPES {
        let _exact = t.exact.insert((*ext).to_owned(), (*typ).to_owned());
        let _lower = t.lower.insert((*ext).to_owned(), (*typ).to_owned());
    }
    if !MIME_GLOBS.iter().any(|f| t.load_globs(f)) {
        for f in TYPE_FILES {
            t.load_mime_file(f);
        }
    }
    t
});

/// Go: `mime.TypeByExtension(ext)` (`ext` starts with a dot): the type for the extension,
/// exact first, then ignoring case; `""` when unknown.
#[must_use]
pub fn type_by_extension(ext: &str) -> String {
    let tables = &*TABLES;
    if let Some(t) = tables.exact.get(ext) {
        return t.clone();
    }
    tables
        .lower
        .get(&to_lower(ext))
        .cloned()
        .unwrap_or_default()
}

/// Go: `isTSpecial`.
fn is_tspecial(c: char) -> bool {
    "()<>@,;:\\\"/[]?=".contains(c)
}

/// Go: `isTokenChar`.
fn is_token_char(c: char) -> bool {
    c > ' ' && c < '\u{7f}' && !is_tspecial(c)
}

/// Go: `mime.isToken`.
fn is_token(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_token_char)
}

/// Go: `consumeToken`.
fn consume_token(v: &str) -> (&str, &str) {
    v.find(|c: char| !is_token_char(c))
        .map_or((v, ""), |i| v.split_at(i))
}

/// Go: `consumeValue`.
fn consume_value(v: &str) -> Option<(String, &str)> {
    if v.is_empty() {
        return None;
    }
    let Some(quoted) = v.strip_prefix('"') else {
        let (tok, rest) = consume_token(v);
        return (!tok.is_empty()).then(|| (tok.to_owned(), rest));
    };
    let mut out = String::new();
    let mut chars = quoted.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '"' {
            return Some((out, quoted.get(i + 1..).unwrap_or_default()));
        }
        if c == '\\'
            && let Some(next) = quoted.get(i + 1..).and_then(|s| s.chars().next())
            && is_tspecial(next)
        {
            out.push(next);
            let _skipped = chars.next();
            continue;
        }
        if c == '\r' || c == '\n' {
            return None;
        }
        out.push(c);
    }
    None
}

/// Go: `consumeMediaParam`: `None` when `v` does not start with a parameter.
fn consume_media_param(v: &str) -> Option<(String, String, &str)> {
    let rest = v.trim_start().strip_prefix(';')?;
    let (param, rest) = consume_token(rest.trim_start());
    if param.is_empty() {
        return None;
    }
    let rest = rest.trim_start().strip_prefix('=')?;
    let (value, rest) = consume_value(rest.trim_start())?;
    Some((to_lower(param), value, rest))
}

/// Go: `checkMediaTypeDisposition`.
fn check_media_type_disposition(s: &str) -> Result<(), &'static str> {
    let (typ, rest) = consume_token(s);
    if typ.is_empty() {
        return Err("mime: no media type");
    }
    if rest.is_empty() {
        return Ok(());
    }
    let Some(rest) = rest.strip_prefix('/') else {
        return Err("mime: expected slash after first token");
    };
    let (subtype, rest) = consume_token(rest);
    if subtype.is_empty() {
        return Err("mime: expected token after slash");
    }
    if !rest.is_empty() {
        return Err("mime: unexpected content after media subtype");
    }
    Ok(())
}

/// An error of [`parse_media_type`]: Go's text, and the media type Go returns with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MediaTypeError {
    pub(super) media_type: String,
    pub(super) text: &'static str,
}

/// Go: `mime.ParseMediaType(v)`. RFC 2231 extended and continued parameters are kept under
/// their raw names rather than decoded.
pub(super) fn parse_media_type(
    v: &str,
) -> Result<(String, BTreeMap<String, String>), MediaTypeError> {
    let base = v.split_once(';').map_or(v, |(b, _)| b);
    let lowered = to_lower(base);
    let media_type = trim_space(&lowered).to_owned();
    check_media_type_disposition(&media_type).map_err(|text| MediaTypeError {
        media_type: String::new(),
        text,
    })?;
    let mut params = BTreeMap::new();
    let mut rest = v.get(base.len()..).unwrap_or_default();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let Some((key, value, after)) = consume_media_param(rest) else {
            if trim_space(rest) == ";" {
                break;
            }
            return Err(MediaTypeError {
                media_type,
                text: "mime: invalid media parameter",
            });
        };
        if let Some(existing) = params.get(&key)
            && *existing != value
        {
            return Err(MediaTypeError {
                media_type: String::new(),
                text: "mime: duplicate parameter name",
            });
        }
        let _previous = params.insert(key, value);
        rest = after;
    }
    Ok((media_type, params))
}

/// Go: `mime.FormatMediaType(t, param)` for values that need no RFC 2231 encoding.
pub(super) fn format_media_type(t: &str, params: &BTreeMap<String, String>) -> String {
    let mut b = String::new();
    match t.split_once('/') {
        None => {
            if !is_token(t) {
                return String::new();
            }
            b.push_str(&to_lower(t));
        }
        Some((major, sub)) => {
            if !is_token(major) || !is_token(sub) {
                return String::new();
            }
            b.push_str(&to_lower(major));
            b.push('/');
            b.push_str(&to_lower(sub));
        }
    }
    for (attribute, value) in params {
        b.push_str("; ");
        if !is_token(attribute) {
            return String::new();
        }
        b.push_str(&to_lower(attribute));
        b.push('=');
        if is_token(value) {
            b.push_str(value);
            continue;
        }
        b.push('"');
        for c in value.chars() {
            if c == '"' || c == '\\' {
                b.push('\\');
            }
            b.push(c);
        }
        b.push('"');
    }
    b
}

#[cfg(test)]
mod tests {
    use core::fmt::Debug;
    use std::collections::BTreeMap;

    use super::{format_media_type, parse_media_type, type_by_extension};

    fn check<T: PartialEq + Debug>(got: &T, want: &T, name: &str) -> Result<(), String> {
        if got == want {
            Ok(())
        } else {
            Err(format!("{name}: got {got:?}, want {want:?}"))
        }
    }

    #[test]
    fn builtin_types() -> Result<(), String> {
        check(&type_by_extension(".png"), &"image/png".to_owned(), ".png")?;
        check(&type_by_extension(".PNG"), &"image/png".to_owned(), ".PNG")?;
        check(
            &type_by_extension(".no-such-ext"),
            &String::new(),
            "unknown",
        )
    }

    #[test]
    fn media_types() -> Result<(), String> {
        let (t, p) =
            parse_media_type("Text/HTML; Charset=\"utf-8\"").map_err(|e| e.text.to_owned())?;
        check(&t, &"text/html".to_owned(), "type")?;
        check(
            &p.get("charset").cloned(),
            &Some("utf-8".to_owned()),
            "charset",
        )?;
        check(
            &parse_media_type("text/plain; charset").map_err(|e| e.text),
            &Err("mime: invalid media parameter"),
            "bad param",
        )?;
        check(
            &parse_media_type("").map_err(|e| e.text),
            &Err("mime: no media type"),
            "empty",
        )?;
        let mut params = BTreeMap::new();
        let _previous = params.insert("charset".to_owned(), "utf-8".to_owned());
        check(
            &format_media_type("text/html", &params),
            &"text/html; charset=utf-8".to_owned(),
            "format",
        )
    }
}
