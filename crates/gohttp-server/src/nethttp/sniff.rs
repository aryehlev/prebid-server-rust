//! Go: `net/http/sniff.go`, the WHATWG MIME sniffing algorithm behind `DetectContentType`.

/// Go: `sniffLen`: at most this many bytes are considered.
const SNIFF_LEN: usize = 512;

const HTML: &str = "text/html; charset=utf-8";

enum Sig {
    Html(&'static [u8]),
    Masked {
        mask: &'static [u8],
        pat: &'static [u8],
        skip_ws: bool,
        ct: &'static str,
    },
    Exact(&'static [u8], &'static str),
    Mp4,
    Text,
}

const fn masked(mask: &'static [u8], pat: &'static [u8], ct: &'static str) -> Sig {
    Sig::Masked {
        mask,
        pat,
        skip_ws: false,
        ct,
    }
}

/// Go: `sniffSignatures`, in the same order.
const SIGNATURES: &[Sig] = &[
    Sig::Html(b"<!DOCTYPE HTML"),
    Sig::Html(b"<HTML"),
    Sig::Html(b"<HEAD"),
    Sig::Html(b"<SCRIPT"),
    Sig::Html(b"<IFRAME"),
    Sig::Html(b"<H1"),
    Sig::Html(b"<DIV"),
    Sig::Html(b"<FONT"),
    Sig::Html(b"<TABLE"),
    Sig::Html(b"<A"),
    Sig::Html(b"<STYLE"),
    Sig::Html(b"<TITLE"),
    Sig::Html(b"<B"),
    Sig::Html(b"<BODY"),
    Sig::Html(b"<BR"),
    Sig::Html(b"<P"),
    Sig::Html(b"<!--"),
    Sig::Masked {
        mask: b"\xFF\xFF\xFF\xFF\xFF",
        pat: b"<?xml",
        skip_ws: true,
        ct: "text/xml; charset=utf-8",
    },
    Sig::Exact(b"%PDF-", "application/pdf"),
    Sig::Exact(b"%!PS-Adobe-", "application/postscript"),
    masked(
        b"\xFF\xFF\x00\x00",
        b"\xFE\xFF\x00\x00",
        "text/plain; charset=utf-16be",
    ),
    masked(
        b"\xFF\xFF\x00\x00",
        b"\xFF\xFE\x00\x00",
        "text/plain; charset=utf-16le",
    ),
    masked(
        b"\xFF\xFF\xFF\x00",
        b"\xEF\xBB\xBF\x00",
        "text/plain; charset=utf-8",
    ),
    Sig::Exact(b"\x00\x00\x01\x00", "image/x-icon"),
    Sig::Exact(b"\x00\x00\x02\x00", "image/x-icon"),
    Sig::Exact(b"BM", "image/bmp"),
    Sig::Exact(b"GIF87a", "image/gif"),
    Sig::Exact(b"GIF89a", "image/gif"),
    masked(
        b"\xFF\xFF\xFF\xFF\x00\x00\x00\x00\xFF\xFF\xFF\xFF\xFF\xFF",
        b"RIFF\x00\x00\x00\x00WEBPVP",
        "image/webp",
    ),
    Sig::Exact(b"\x89PNG\x0D\x0A\x1A\x0A", "image/png"),
    Sig::Exact(b"\xFF\xD8\xFF", "image/jpeg"),
    masked(
        b"\xFF\xFF\xFF\xFF\x00\x00\x00\x00\xFF\xFF\xFF\xFF",
        b"FORM\x00\x00\x00\x00AIFF",
        "audio/aiff",
    ),
    masked(b"\xFF\xFF\xFF", b"ID3", "audio/mpeg"),
    masked(b"\xFF\xFF\xFF\xFF\xFF", b"OggS\x00", "application/ogg"),
    masked(
        b"\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF",
        b"MThd\x00\x00\x00\x06",
        "audio/midi",
    ),
    masked(
        b"\xFF\xFF\xFF\xFF\x00\x00\x00\x00\xFF\xFF\xFF\xFF",
        b"RIFF\x00\x00\x00\x00AVI ",
        "video/avi",
    ),
    masked(
        b"\xFF\xFF\xFF\xFF\x00\x00\x00\x00\xFF\xFF\xFF\xFF",
        b"RIFF\x00\x00\x00\x00WAVE",
        "audio/wave",
    ),
    Sig::Mp4,
    Sig::Exact(b"\x1A\x45\xDF\xA3", "video/webm"),
    masked(
        b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\
          \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xFF\xFF",
        b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\
          \x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00LP",
        "application/vnd.ms-fontobject",
    ),
    Sig::Exact(b"\x00\x01\x00\x00", "font/ttf"),
    Sig::Exact(b"OTTO", "font/otf"),
    Sig::Exact(b"ttcf", "font/collection"),
    Sig::Exact(b"wOFF", "font/woff"),
    Sig::Exact(b"wOF2", "font/woff2"),
    Sig::Exact(b"\x1F\x8B\x08", "application/x-gzip"),
    Sig::Exact(b"PK\x03\x04", "application/zip"),
    Sig::Exact(b"Rar!\x1A\x07\x00", "application/x-rar-compressed"),
    Sig::Exact(b"Rar!\x1A\x07\x01\x00", "application/x-rar-compressed"),
    Sig::Exact(b"\x00\x61\x73\x6D", "application/wasm"),
    Sig::Text,
];

/// Go: `isWS`.
const fn is_ws(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | 0x0C | b'\r' | b' ')
}

/// Go: `isTT`.
const fn is_tt(b: u8) -> bool {
    matches!(b, b' ' | b'>')
}

impl Sig {
    fn matches(&self, data: &[u8], first_non_ws: usize) -> Option<&'static str> {
        let rest = data.get(first_non_ws..).unwrap_or_default();
        match *self {
            Self::Html(sig) => {
                let term = rest.get(sig.len())?;
                let head = rest.get(..sig.len())?;
                let equal = sig.iter().zip(head).all(|(&b, &d)| {
                    let d = if b.is_ascii_uppercase() { d & 0xDF } else { d };
                    b == d
                });
                (equal && is_tt(*term)).then_some(HTML)
            }
            Self::Masked {
                mask,
                pat,
                skip_ws,
                ct,
            } => {
                let d = if skip_ws { rest } else { data };
                if pat.len() != mask.len() || d.len() < pat.len() {
                    return None;
                }
                pat.iter()
                    .zip(mask)
                    .zip(d)
                    .all(|((&p, &m), &b)| b & m == p)
                    .then_some(ct)
            }
            Self::Exact(sig, ct) => data.starts_with(sig).then_some(ct),
            Self::Mp4 => mp4(data),
            Self::Text => rest
                .iter()
                .all(|&b| !matches!(b, 0x00..=0x08 | 0x0B | 0x0E..=0x1A | 0x1C..=0x1F))
                .then_some("text/plain; charset=utf-8"),
        }
    }
}

/// Go: `mp4Sig.match`.
fn mp4(data: &[u8]) -> Option<&'static str> {
    let size: [u8; 4] = data.get(..4)?.try_into().ok()?;
    if data.len() < 12 {
        return None;
    }
    let box_size = usize::try_from(u32::from_be_bytes(size)).ok()?;
    if data.len() < box_size || box_size % 4 != 0 {
        return None;
    }
    if data.get(4..8)? != b"ftyp" {
        return None;
    }
    let mut st = 8;
    while st < box_size {
        if st != 12 && data.get(st..st + 3)? == b"mp4" {
            return Some("video/mp4");
        }
        st += 4;
    }
    None
}

/// Go: `http.DetectContentType`: the MIME type of `data` by the WHATWG sniffing algorithm,
/// considering at most the first 512 bytes; `application/octet-stream` when nothing matches.
#[must_use]
pub fn detect_content_type(data: &[u8]) -> &'static str {
    let data = data.get(..SNIFF_LEN).unwrap_or(data);
    let first_non_ws = data.iter().position(|&b| !is_ws(b)).unwrap_or(data.len());
    SIGNATURES
        .iter()
        .find_map(|sig| sig.matches(data, first_non_ws))
        .unwrap_or("application/octet-stream")
}

#[cfg(test)]
mod tests {
    use super::detect_content_type;

    #[test]
    fn sniff_table() -> Result<(), String> {
        // Go: net/http/sniff_test.go sniffTests.
        let cases: &[(&str, &[u8], &str)] = &[
            ("Empty", b"", "text/plain; charset=utf-8"),
            ("Binary", &[1, 2, 3], "application/octet-stream"),
            (
                "HTML document #1",
                b"<HtMl><bOdY>blah blah blah</body></html>",
                "text/html; charset=utf-8",
            ),
            (
                "HTML document #2",
                b"<HTML></HTML>",
                "text/html; charset=utf-8",
            ),
            (
                "HTML document #3 (leading whitespace)",
                b"   <!DOCTYPE HTML>...",
                "text/html; charset=utf-8",
            ),
            (
                "HTML document #4 (leading CRLF)",
                b"\r\n<html>...",
                "text/html; charset=utf-8",
            ),
            (
                "Plain text",
                b"This is not HTML. It has \xe2\x98\x83 though.",
                "text/plain; charset=utf-8",
            ),
            ("XML", b"\n<?xml!", "text/xml; charset=utf-8"),
            ("Windows icon", b"\x00\x00\x01\x00", "image/x-icon"),
            ("Windows cursor", b"\x00\x00\x02\x00", "image/x-icon"),
            ("BMP image", b"BM...", "image/bmp"),
            ("GIF 87a", b"GIF87a", "image/gif"),
            ("GIF 89a", b"GIF89a...", "image/gif"),
            ("WEBP image", b"RIFF\x00\x00\x00\x00WEBPVP", "image/webp"),
            ("PNG image", b"\x89PNG\x0D\x0A\x1A\x0A", "image/png"),
            ("JPEG image", b"\xFF\xD8\xFF", "image/jpeg"),
            ("PDF", b"%PDF-1.4", "application/pdf"),
            (
                "MP4 video",
                b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom<\x06t\xbfmdat",
                "video/mp4",
            ),
            ("AVI video #1", b"RIFF,O\n\x00AVI LIST\xc3\x80", "video/avi"),
            (
                "woff",
                b"wOFF\x00\x01\x00\x00\x00\x00\x30\x54\x00\x0d\x00\x00",
                "font/woff",
            ),
            ("gzip", b"\x1f\x8b\x08\x08\xa1", "application/x-gzip"),
            ("wasm", b"\x00asm\x01\x00", "application/wasm"),
            ("JSON", br#"{"a":1}"#, "text/plain; charset=utf-8"),
            (
                "HTML tag no terminator",
                b"<HTML",
                "text/plain; charset=utf-8",
            ),
            ("comment", b"<!-- x -->", "text/html; charset=utf-8"),
        ];
        for &(name, data, want) in cases {
            let got = detect_content_type(data);
            if got != want {
                return Err(format!("{name}: got {got}, want {want}"));
            }
        }
        Ok(())
    }
}
