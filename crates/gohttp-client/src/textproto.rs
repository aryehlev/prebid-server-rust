//! Go `net/textproto.Reader`: `ReadLine` and `ReadMIMEHeader` as `http.ReadResponse` uses them.
//!
//! [`MimeHeaderReader`] is a restartable state machine over [`BufReader`]; it follows Go's
//! `readMIMEHeader` step for step (the leading-space check, `readContinuedLineSlice` with
//! `skipSpace`, key canonicalization and value validation), so the same bytes produce the same
//! header map and the same `malformed MIME header` errors.

use std::{
    collections::BTreeMap,
    mem,
    task::{Context as TaskContext, Poll, ready},
};

use pbs_gocompat::strconv::quote;

use crate::{
    bufio::{BufReader, GoRead},
    error::HttpError,
};

/// Go: `textproto.ProtocolError`.
fn protocol_error(msg: &str, line: &[u8]) -> HttpError {
    HttpError::Message(format!("{msg}{}", String::from_utf8_lossy(line)))
}

/// Go: `textproto.errMessageTooLarge`.
const ERR_MESSAGE_TOO_LARGE: HttpError = HttpError::Static("message too large");

/// Go: `textproto.trim`: drops leading and trailing spaces and tabs.
fn trim(s: &[u8]) -> &[u8] {
    let start = s
        .iter()
        .position(|&b| b != b' ' && b != b'\t')
        .unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&b| b != b' ' && b != b'\t')
        .map_or(start, |i| i + 1);
    s.get(start..end).unwrap_or_default()
}

/// Go: `textproto.validHeaderFieldByte` (RFC 7230 `tchar`).
const fn valid_header_field_byte(c: u8) -> bool {
    matches!(c,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`'
        | b'|' | b'~' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z')
}

/// Go: `textproto.validHeaderValueByte`: tab, space, VCHAR and obs-text.
const fn valid_header_value_byte(c: u8) -> bool {
    c == b'\t' || (c >= 0x20 && c != 0x7F)
}

/// Go: `textproto.canonicalMIMEHeaderKey` over bytes: `None` when the key has a byte that is
/// neither a token byte nor a space; keys with a space are returned unchanged.
#[must_use]
pub fn canonical_mime_header_key_bytes(a: &[u8]) -> Option<String> {
    if a.is_empty() {
        return None;
    }
    let mut no_canon = false;
    for &c in a {
        if valid_header_field_byte(c) {
            continue;
        }
        if c == b' ' {
            no_canon = true;
            continue;
        }
        return None;
    }
    if no_canon {
        return Some(String::from_utf8_lossy(a).into_owned());
    }
    let mut upper = true;
    let canon: String = a
        .iter()
        .map(|&c| {
            let out = if upper {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            };
            upper = out == b'-';
            char::from(out)
        })
        .collect();
    Some(canon)
}

/// Go: `textproto.Reader.ReadLine` (`readLineSlice(-1)`): one logical line with long lines
/// joined, restartable across `Pending`.
#[derive(Debug, Default)]
pub struct LineReader {
    line: Vec<u8>,
}

impl LineReader {
    /// Reads the next line; `limit` is Go's `lim` (`None` for no limit).
    pub fn poll_read<R: GoRead + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        br: &mut BufReader,
        src: &mut R,
        limit: Option<usize>,
    ) -> Poll<Result<Vec<u8>, HttpError>> {
        loop {
            let before = self.line.len();
            let more = match ready!(br.poll_read_line(cx, src, &mut self.line)) {
                Ok(more) => more,
                Err(e) => {
                    self.line.clear();
                    return Poll::Ready(Err(e));
                }
            };
            let _ = before;
            if limit.is_some_and(|lim| self.line.len() > lim) {
                self.line.clear();
                return Poll::Ready(Err(ERR_MESSAGE_TOO_LARGE));
            }
            if !more {
                return Poll::Ready(Ok(mem::take(&mut self.line)));
            }
        }
    }
}

/// Go: `textproto.MIMEHeader`, in key order.
pub type MimeHeader = BTreeMap<String, Vec<String>>;

#[derive(Debug, Default)]
enum MimeState {
    /// Go: the `Peek(1)` leading-space check.
    #[default]
    Start,
    /// Go: reading the malformed initial line (limit 80) for the error text.
    InitialLine,
    /// Go: `readContinuedLineSlice`, the first line.
    Line,
    /// Go: `skipSpace` after a line; `n` spaces skipped so far.
    Skip { n: usize },
    /// Go: reading a continuation line.
    Continuation,
}

/// Go: `textproto.Reader.ReadMIMEHeader`, restartable across `Pending`.
#[derive(Debug, Default)]
pub struct MimeHeaderReader {
    state: MimeState,
    lines: LineReader,
    /// Go: `r.buf`, the logical line being assembled.
    cur: Vec<u8>,
    header: MimeHeader,
}

impl MimeHeaderReader {
    /// Reads header lines up to and including the blank line.
    pub fn poll_read<R: GoRead + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        br: &mut BufReader,
        src: &mut R,
    ) -> Poll<Result<MimeHeader, HttpError>> {
        loop {
            match self.state {
                MimeState::Start => {
                    // The first line cannot start with a leading space.
                    let peeked = ready!(br.poll_peek(cx, src, 1));
                    let leading_space =
                        peeked.is_ok() && matches!(br.buffer().first(), Some(b' ' | b'\t'));
                    self.state = if leading_space {
                        MimeState::InitialLine
                    } else {
                        MimeState::Line
                    };
                }
                MimeState::InitialLine => {
                    let line = ready!(self.lines.poll_read(cx, br, src, Some(80)))?;
                    return Poll::Ready(Err(protocol_error(
                        "malformed MIME header initial line: ",
                        &line,
                    )));
                }
                MimeState::Line => {
                    let line = ready!(self.lines.poll_read(cx, br, src, None))?;
                    if line.is_empty() {
                        return Poll::Ready(Ok(mem::take(&mut self.header)));
                    }
                    if !line.contains(&b':') {
                        return Poll::Ready(Err(HttpError::Message(format!(
                            "malformed MIME header: missing colon: {}",
                            quote(&String::from_utf8_lossy(&line))
                        ))));
                    }
                    self.cur.clear();
                    self.cur.extend_from_slice(trim(&line));
                    self.state = MimeState::Skip { n: 0 };
                }
                MimeState::Skip { n } => {
                    // Go: skipSpace (ReadByte until a byte that is not space or tab).
                    let mut skipped = n;
                    loop {
                        if br.buffered() == 0 {
                            if br.take_err().is_some() {
                                break;
                            }
                            self.state = MimeState::Skip { n: skipped };
                            ready!(br.poll_fill(cx, src));
                            continue;
                        }
                        if matches!(br.buffer().first(), Some(b' ' | b'\t')) {
                            br.consume(1);
                            skipped += 1;
                        } else {
                            break;
                        }
                    }
                    if skipped > 0 {
                        self.cur.push(b' ');
                        self.state = MimeState::Continuation;
                    } else {
                        if let Err(e) = self.add_line() {
                            return Poll::Ready(Err(e));
                        }
                        self.state = MimeState::Line;
                    }
                }
                MimeState::Continuation => {
                    match ready!(self.lines.poll_read(cx, br, src, None)) {
                        Ok(line) => {
                            self.cur.extend_from_slice(trim(&line));
                            self.state = MimeState::Skip { n: 0 };
                        }
                        Err(_continuation_error) => {
                            // Go breaks out of the continuation loop and keeps the line.
                            if let Err(e) = self.add_line() {
                                return Poll::Ready(Err(e));
                            }
                            self.state = MimeState::Line;
                        }
                    }
                }
            }
        }
    }

    /// Go: the body of `readMIMEHeader`'s loop for one logical line `kv`.
    fn add_line(&mut self) -> Result<(), HttpError> {
        let kv = &self.cur;
        let colon = kv.iter().position(|&b| b == b':').unwrap_or(kv.len());
        let (k, rest) = kv.split_at(colon);
        let v = rest.get(1..).unwrap_or_default();
        let Some(key) = canonical_mime_header_key_bytes(k) else {
            return Err(protocol_error("malformed MIME header line: ", kv));
        };
        if !v.iter().all(|&c| valid_header_value_byte(c)) {
            return Err(protocol_error("malformed MIME header line: ", kv));
        }
        let start = v
            .iter()
            .position(|&b| b != b' ' && b != b'\t')
            .unwrap_or(v.len());
        let value = String::from_utf8_lossy(v.get(start..).unwrap_or_default()).into_owned();
        self.header.entry(key).or_default().push(value);
        Ok(())
    }
}
