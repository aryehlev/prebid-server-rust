//! HTTP bodies in parity documents (Go: `parity/body.go`).
//!
//! Rules shared by every implementation:
//!
//! - `b64`, when present, is always the exact wire bytes;
//! - `text`, when present and `content_encoding` is empty, is the exact wire bytes;
//! - `json` is the semantic JSON of the decoded content when it parses as JSON;
//! - a recorded body (traces, `http_out`) has `text` for valid UTF-8 identity bodies and `b64`
//!   otherwise, plus `json` when the decoded content is JSON; a content-encoded body is always
//!   recorded as `b64`;
//! - on input documents only, `text` or `json` may come with a `content_encoding`, and the receiver
//!   encodes the content itself.
//!
//! The base64 is the base64 crate's standard engine, which refuses the line breaks and the set
//! unused bits Go's `base64.StdEncoding` lets through (`base64.parity-std-encoding`); gzip content
//! goes through flate2, whose deflate stream and messages differ from `compress/gzip`'s, and the
//! `Content-Encoding` is lower-cased by `str::to_lowercase`, which maps U+0130 otherwise than Go's
//! `strings.ToLower` (`gzip.parity-bodies`).

use core::{error::Error, fmt};
use std::{
    borrow::Cow,
    io::{self, Read as _, Write as _},
    str,
};

use base64::{DecodeError, Engine as _, engine::general_purpose::STANDARD};
use flate2::{Compression, read::MultiGzDecoder, write::GzEncoder};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// An HTTP body (Go: `parity.Body`). Go's nil `*Body` ("no body") is `Option::None`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Body {
    /// The exact bytes of a valid UTF-8 identity body (or input content to encode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The exact wire bytes, standard base64 with padding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub b64: Option<String>,
    /// The decoded content, when it is JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json: Option<Box<RawValue>>,
    /// The normalized `Content-Encoding` (empty for identity).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content_encoding: String,
}

/// Why a body's bytes could not be produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyError {
    /// `b64` is not standard base64 (`body.b64: <the base64 crate's message>`).
    Base64(DecodeError),
    /// A content encoding other than identity and gzip (`unsupported content encoding "br"`).
    UnsupportedEncoding(String),
    /// Gzip content that does not inflate, or could not be deflated (`gzip decode: <flate2's
    /// message>`).
    Gzip {
        /// Whether encoding (true) or decoding (false) failed.
        encode: bool,
        /// flate2's message.
        message: String,
    },
}

impl fmt::Display for BodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Base64(err) => write!(f, "body.b64: {err}"),
            Self::UnsupportedEncoding(enc) => write!(f, "unsupported content encoding {enc:?}"),
            Self::Gzip { encode, message } => write!(
                f,
                "gzip {}: {message}",
                if *encode { "encode" } else { "decode" }
            ),
        }
    }
}

impl Error for BodyError {}

impl Body {
    /// A body holding `s` as identity text (Go: `parity.TextBody`).
    #[must_use]
    pub fn text_body<S: Into<String>>(s: S) -> Self {
        Self {
            text: Some(s.into()),
            ..Self::default()
        }
    }

    /// Records raw wire bytes (Go: `parity.NewBody`). `content_encoding` is the
    /// `Content-Encoding` header value; gzip content is inflated to fill `json`. Returns `None`
    /// for an empty body.
    #[must_use]
    pub fn new(raw: &[u8], content_encoding: &str) -> Option<Self> {
        if raw.is_empty() {
            return None;
        }
        let content_encoding = normalize_encoding(content_encoding);
        let (text, b64) = match str::from_utf8(raw) {
            Ok(text) if content_encoding.is_empty() => (Some(text.to_owned()), None),
            Ok(_) | Err(_) => (None, Some(STANDARD.encode(raw))),
        };
        let json = decode_content(raw, &content_encoding)
            .ok()
            .and_then(|decoded| json_value(&decoded));
        Some(Self {
            text,
            b64,
            json,
            content_encoding,
        })
    }

    /// The bytes to put on the wire for an input body (Go: `Body.WireBytes`): `b64` decoded,
    /// otherwise `text` or `json` encoded with `content_encoding`. A body with neither is empty.
    ///
    /// # Errors
    ///
    /// Invalid base64, or a content encoding that cannot be applied.
    pub fn wire_bytes(&self) -> Result<Vec<u8>, BodyError> {
        if let Some(b64) = &self.b64 {
            return STANDARD.decode(b64).map_err(BodyError::Base64);
        }
        let content: &[u8] = match (&self.text, &self.json) {
            (Some(text), _) => text.as_bytes(),
            (None, Some(json)) => json.get().as_bytes(),
            (None, None) => return Ok(Vec::new()),
        };
        encode_content(content, &normalize_encoding(&self.content_encoding))
    }

    /// The decoded content of a recorded or input body (Go: `Body.DecodedBytes`).
    ///
    /// # Errors
    ///
    /// Invalid base64, or a content encoding that cannot be decoded.
    pub fn decoded_bytes(&self) -> Result<Vec<u8>, BodyError> {
        if self.b64.is_none() {
            return Ok(match (&self.text, &self.json) {
                (Some(text), _) => text.as_bytes().to_vec(),
                (None, Some(json)) => json.get().as_bytes().to_vec(),
                (None, None) => Vec::new(),
            });
        }
        let raw = self.wire_bytes()?;
        decode_content(&raw, &normalize_encoding(&self.content_encoding)).map(Cow::into_owned)
    }
}

/// `bytes` as a raw JSON value, when they are UTF-8 JSON (Go: `json.Valid`, which also passes
/// strings holding bytes that are not UTF-8).
fn json_value(bytes: &[u8]) -> Option<Box<RawValue>> {
    let text = str::from_utf8(bytes).ok()?;
    RawValue::from_string(text.to_owned()).ok()
}

/// Go: `normalizeEncoding`: lower-cased and trimmed, with identity as empty.
fn normalize_encoding(enc: &str) -> String {
    let enc = enc.trim().to_lowercase();
    if enc == "identity" {
        String::new()
    } else {
        enc
    }
}

/// Go: `decodeContent`.
fn decode_content<'a>(raw: &'a [u8], enc: &str) -> Result<Cow<'a, [u8]>, BodyError> {
    match enc {
        "" => Ok(Cow::Borrowed(raw)),
        "gzip" | "x-gzip" => {
            let mut out = Vec::new();
            let _read = MultiGzDecoder::new(raw)
                .read_to_end(&mut out)
                .map_err(|err| BodyError::Gzip {
                    encode: false,
                    message: err.to_string(),
                })?;
            Ok(Cow::Owned(out))
        }
        _ => Err(BodyError::UnsupportedEncoding(enc.to_owned())),
    }
}

/// Go: `encodeContent`.
fn encode_content(content: &[u8], enc: &str) -> Result<Vec<u8>, BodyError> {
    match enc {
        "" => Ok(content.to_vec()),
        "gzip" | "x-gzip" => {
            let failed = |err: io::Error| BodyError::Gzip {
                encode: true,
                message: err.to_string(),
            };
            let mut zw = GzEncoder::new(Vec::new(), Compression::default());
            zw.write_all(content).map_err(failed)?;
            zw.finish().map_err(failed)
        }
        _ => Err(BodyError::UnsupportedEncoding(enc.to_owned())),
    }
}

#[cfg(test)]
#[path = "body_test.rs"]
mod tests;
