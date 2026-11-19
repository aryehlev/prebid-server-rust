//! Glue (Rule B (c)): `certificates_file`, the operator's PEM file of extra roots, read as Go
//! reads it.
//!
//! router.go gives its general client the system roots plus the certificates of
//! `certificates_file` (router/router.go:131-141, server/ssl/ssl.go): `os.ReadFile`, then
//! `CertPool.AppendCertsFromPEM`, which takes every `CERTIFICATE` block without headers that
//! `pem.Decode` finds and `x509.ParseCertificate` reads, and skips the rest. The file's format is
//! an operator interface, so [`Roots::from_pem`] ports Go 1.26's `pem.Decode` block by block, and
//! a block counts as a certificate when rustls takes it as a trust anchor, the verifier's own
//! reading standing for Go's parser (`tls.certificates-file`, pin
//! `crates/http-client/tests/tls.rs::certificates_file_like_go` against
//! `testdata/golden/certificates_file.json`). The cache and floors clients take the system roots
//! only, as router.go builds them.

use std::{error::Error, fmt, fs, io};

use base64::{
    Engine as _,
    alphabet::STANDARD,
    engine::{GeneralPurpose, GeneralPurposeConfig},
};
use rustls::{RootCertStore, pki_types::CertificateDer};

/// Extra trust anchors for a client.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roots {
    certificates: Vec<CertificateDer<'static>>,
}

/// `certificates_file` could not be read: Go's `Failed to read file <path>: <err>`, which router.go
/// logs as `Could not read certificates file: ...` before it goes on with the system roots.
#[derive(Debug)]
pub struct ReadError {
    path: String,
    op: &'static str,
    source: io::Error,
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Failed to read file {path}: {op} {path}: {err}",
            path = self.path,
            op = self.op,
            err = go_errno_text(&self.source)
        )
    }
}

impl Error for ReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

/// Go's `syscall.Errno` text for `err`: the C library's `strerror` text, which Go's tables give
/// with a lower-case first letter, without Rust's ` (os error N)`.
fn go_errno_text(err: &io::Error) -> String {
    let text = err.to_string();
    let text = err
        .raw_os_error()
        .and_then(|code| text.strip_suffix(&format!(" (os error {code})")))
        .unwrap_or(&text);
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

impl Roots {
    /// No extra roots: the cache, floors and plain clients.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            certificates: Vec::new(),
        }
    }

    /// Go's `ssl.AppendPEMFileToCertPool` for `certificates_file`: no roots when `path` is empty,
    /// else the certificates of the file.
    ///
    /// # Errors
    ///
    /// The file could not be opened or read; router.go logs the error and goes on without the
    /// file's roots.
    pub fn from_certificates_file(path: &str) -> Result<Self, ReadError> {
        if path.is_empty() {
            return Ok(Self::none());
        }
        // Opening a directory succeeds and reading it fails, in Go's os.ReadFile as here; every
        // other failure comes from the open.
        let pem = fs::read(path).map_err(|source| ReadError {
            path: path.to_owned(),
            op: if source.kind() == io::ErrorKind::IsADirectory {
                "read"
            } else {
                "open"
            },
            source,
        })?;
        Ok(Self::from_pem(&pem))
    }

    /// Go's `CertPool.AppendCertsFromPEM` into an empty pool: the certificate blocks of `pem`,
    /// each once. Go's `ok` is whether the result is not empty.
    #[must_use]
    pub fn from_pem(pem: &[u8]) -> Self {
        let mut certificates: Vec<CertificateDer<'static>> = Vec::new();
        let mut rest = pem;
        while !rest.is_empty() {
            let Some((block, next)) = pem_decode(rest) else {
                break;
            };
            rest = next;
            if block.kind != b"CERTIFICATE" || block.has_headers {
                continue;
            }
            let der = CertificateDer::from(block.bytes);
            if !parses(&der) || certificates.contains(&der) {
                continue;
            }
            certificates.push(der);
        }
        Self { certificates }
    }

    /// The certificates, in file order.
    #[must_use]
    pub fn certificates(&self) -> &[CertificateDer<'static>] {
        &self.certificates
    }

    /// Whether there are none.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.certificates.is_empty()
    }
}

/// Whether rustls takes `der` as a trust anchor: the stand-in for Go's `x509.ParseCertificate`.
fn parses(der: &CertificateDer<'_>) -> bool {
    RootCertStore::empty().add(der.clone().into_owned()).is_ok()
}

/// A PEM block: its type, whether it had headers, and its decoded bytes.
#[derive(Debug)]
struct Block<'a> {
    kind: &'a [u8],
    has_headers: bool,
    bytes: Vec<u8>,
}

const PEM_START: &[u8] = b"\n-----BEGIN ";
const PEM_END: &[u8] = b"\n-----END ";
const PEM_END_OF_LINE: &[u8] = b"-----";

/// Go's `base64.StdEncoding` (padding required, trailing bits allowed).
const STD: GeneralPurpose = GeneralPurpose::new(
    &STANDARD,
    GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
);

/// Go 1.26's `pem.Decode` (encoding/pem/pem.go:89-198): the next block and the rest, or `None`
/// when no block is left. Offsets are signed, as Go's are, so the checks that turn them negative
/// end the search the same way.
fn pem_decode(data: &[u8]) -> Option<(Block<'_>, &[u8])> {
    let mut rest = data;
    let mut end_trailer_index: isize = 0;
    loop {
        let skip = usize::try_from(end_trailer_index).ok()?;
        rest = rest.get(skip..)?;
        let mut end_index = signed(find(rest, PEM_END)?);
        end_trailer_index = end_index + signed(PEM_END.len());
        let before_end = rest.get(..usize::try_from(end_index).ok()?)?;
        let Some(begin_index) = rfind(before_end, PEM_START.get(1..)?) else {
            continue;
        };
        if begin_index > 0 && before_end.get(begin_index - 1) != Some(&b'\n') {
            continue;
        }
        let start = begin_index + PEM_START.len() - 1;
        rest = rest.get(start..)?;
        end_index -= signed(start);
        end_trailer_index -= signed(start);

        let (type_line, after_type, consumed) = get_line(rest);
        rest = after_type;
        end_index -= signed(consumed);
        end_trailer_index -= signed(consumed);
        let Some(kind) = type_line.strip_suffix(PEM_END_OF_LINE) else {
            continue;
        };

        let mut has_headers = false;
        loop {
            if rest.is_empty() {
                return None;
            }
            let (line, next, line_len) = get_line(rest);
            if !line.contains(&b':') {
                break;
            }
            has_headers = true;
            rest = next;
            end_index -= signed(line_len);
            end_trailer_index -= signed(line_len);
        }
        if has_headers && end_index < 0 {
            continue;
        }

        let Some(end_trailer) = usize::try_from(end_trailer_index)
            .ok()
            .and_then(|i| rest.get(i..))
        else {
            // Go slices out of range here and panics; a negative index can only come with headers,
            // which the check above has already sent on.
            continue;
        };
        let trailer_len = kind.len() + PEM_END_OF_LINE.len();
        let Some((trailer, rest_of_end_line)) = end_trailer.split_at_checked(trailer_len) else {
            continue;
        };
        if !trailer.starts_with(kind) || !trailer.ends_with(PEM_END_OF_LINE) {
            continue;
        }
        if !get_line(rest_of_end_line).0.is_empty() {
            continue;
        }

        let mut bytes = Vec::new();
        if end_index > 0 {
            let body = rest.get(..usize::try_from(end_index).ok()?)?;
            let base64: Vec<u8> = body
                .iter()
                .copied()
                .filter(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
                .collect();
            match STD.decode(&base64) {
                Ok(decoded) => bytes = decoded,
                Err(_) => continue,
            }
        }
        // The -1: an empty block's END line matched without its leading newline, which the type
        // line took (Go's comment).
        let after = usize::try_from(end_index + signed(PEM_END.len()) - 1).ok()?;
        let (_, next, _) = get_line(rest.get(after..)?);
        return Some((
            Block {
                kind,
                has_headers,
                bytes,
            },
            next,
        ));
    }
}

/// Go's `getLine`: the line without its `\n` (and a `\r` before it) and its trailing spaces and
/// tabs, the rest, and how many bytes the line took.
fn get_line(data: &[u8]) -> (&[u8], &[u8], usize) {
    let (line, consumed) = data
        .iter()
        .position(|&b| b == b'\n')
        .map_or((data, data.len()), |i| {
            let line = data.get(..i).unwrap_or_default();
            (line.strip_suffix(b"\r").unwrap_or(line), i + 1)
        });
    let trimmed_len = line
        .iter()
        .rposition(|b| !matches!(b, b' ' | b'\t'))
        .map_or(0, |last| last + 1);
    (
        line.get(..trimmed_len).unwrap_or_default(),
        data.get(consumed..).unwrap_or_default(),
        consumed,
    )
}

/// The first index of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The last index of `needle` in `haystack`.
fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// A slice length as a signed offset (slices never exceed `isize::MAX` bytes).
fn signed(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

#[cfg(test)]
mod tests {
    use super::{get_line, pem_decode};

    // Go: encoding/pem/pem.go Decode and getLine.
    #[test]
    fn pem_blocks_like_go() {
        let decode = |pem: &[u8]| {
            pem_decode(pem).map(|(block, rest)| {
                (
                    block.kind.to_vec(),
                    block.has_headers,
                    block.bytes,
                    rest.to_vec(),
                )
            })
        };
        assert_eq!(
            get_line(b"ab \t\r\ncd"),
            (&b"ab"[..], &b"cd"[..], 6),
            "getLine"
        );
        assert_eq!(
            decode(b"junk\n-----BEGIN A-----\nAQID\n-----END A-----\ntail"),
            Some((b"A".to_vec(), false, vec![1, 2, 3], b"tail".to_vec())),
            "a block between junk and a tail"
        );
        assert_eq!(
            decode(b"-----BEGIN A-----\nAQID\n-----END B-----\n"),
            None,
            "mismatched END"
        );
        assert_eq!(
            decode(b"-----BEGIN A-----\n-----END A-----\nx"),
            Some((b"A".to_vec(), false, Vec::new(), b"x".to_vec())),
            "an empty block"
        );
        assert_eq!(
            decode(b"-----BEGIN A-----\nK: v\n\nAQID\n-----END A-----\n"),
            Some((b"A".to_vec(), true, vec![1, 2, 3], Vec::new())),
            "headers"
        );
    }
}
