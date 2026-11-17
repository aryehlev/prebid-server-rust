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
//! Gzip content (`gzip`, `x-gzip`) needs Go's `compress/gzip`, which `pbs-gogzip` ports and this
//! crate does not use yet: until it does, gzip decoding and encoding fail with
//! [`BodyError::GzipNotPorted`], so [`Body::new`] records gzip bodies without their `json`.

use core::{error::Error, fmt};
use std::{borrow::Cow, str};

use pbs_gocompat::{
    base64::{CorruptInputError, STD_ENCODING},
    strconv::quote,
    strings::{to_lower, trim_space},
};
use pbs_gojson::{GoJson, RawJson, stdjson};

/// An HTTP body (Go: `parity.Body`). Go's nil `*Body` ("no body") is `Option::None`.
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "parity.Body")]
pub struct Body {
    /// The exact bytes of a valid UTF-8 identity body (or input content to encode).
    #[gojson(json = "text,omitempty")]
    pub text: Option<String>,
    /// The exact wire bytes, standard base64 with padding.
    #[gojson(go = "B64", json = "b64,omitempty")]
    pub b64: Option<String>,
    /// The decoded content, when it is JSON.
    #[gojson(go = "JSON", json = "json,omitempty")]
    pub json: RawJson,
    /// The normalized `Content-Encoding` (empty for identity).
    #[gojson(json = "content_encoding,omitempty")]
    pub content_encoding: String,
}

/// Why a body's bytes could not be produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyError {
    /// `b64` is not standard base64 (Go: `body.b64: illegal base64 data at input byte N`).
    Base64(CorruptInputError),
    /// A content encoding other than identity and gzip (Go: `unsupported content encoding
    /// "br"`).
    UnsupportedEncoding(String),
    /// Gzip content, which needs `pbs-gogzip` (see the module docs).
    GzipNotPorted {
        /// Whether encoding (true) or decoding (false) was asked for.
        encode: bool,
    },
}

impl fmt::Display for BodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Base64(err) => write!(f, "body.b64: {err}"),
            Self::UnsupportedEncoding(enc) => {
                write!(f, "unsupported content encoding {}", quote(enc))
            }
            Self::GzipNotPorted { encode } => write!(
                f,
                "gzip {}: compress/gzip is not wired in yet (pbs-gogzip)",
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
    /// `Content-Encoding` header value. Returns `None` for an empty body.
    #[must_use]
    pub fn new(raw: &[u8], content_encoding: &str) -> Option<Self> {
        if raw.is_empty() {
            return None;
        }
        let enc = normalize_encoding(content_encoding);
        let mut b = Self::default();
        match str::from_utf8(raw) {
            Ok(text) if enc.is_empty() => b.text = Some(text.to_owned()),
            Ok(_) | Err(_) => b.b64 = Some(STD_ENCODING.encode_to_string(raw)),
        }
        if let Ok(decoded) = decode_content(raw, &enc)
            && stdjson::valid(&decoded)
        {
            b.json = RawJson::from_slice(&decoded);
        }
        b.content_encoding = enc;
        Some(b)
    }

    /// The bytes to put on the wire for an input body (Go: `Body.WireBytes`): `b64` decoded,
    /// otherwise `text` or `json` encoded with `content_encoding`. A body with neither is empty.
    ///
    /// # Errors
    ///
    /// Invalid base64, or a content encoding that cannot be applied.
    pub fn wire_bytes(&self) -> Result<Vec<u8>, BodyError> {
        if let Some(b64) = &self.b64 {
            return STD_ENCODING.decode_string(b64).map_err(BodyError::Base64);
        }
        let content: &[u8] = match &self.text {
            Some(text) => text.as_bytes(),
            None if !self.json.is_empty() => self.json.as_bytes(),
            None => return Ok(Vec::new()),
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
            return Ok(self.text.as_ref().map_or_else(
                || self.json.as_bytes().to_vec(),
                |text| text.as_bytes().to_vec(),
            ));
        }
        let raw = self.wire_bytes()?;
        decode_content(&raw, &normalize_encoding(&self.content_encoding)).map(Cow::into_owned)
    }
}

/// Go: `normalizeEncoding`: lower-cased and trimmed, with identity as empty.
fn normalize_encoding(enc: &str) -> String {
    let enc = to_lower(trim_space(enc));
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
        "gzip" | "x-gzip" => Err(BodyError::GzipNotPorted { encode: false }),
        _ => Err(BodyError::UnsupportedEncoding(enc.to_owned())),
    }
}

/// Go: `encodeContent`.
fn encode_content(content: &[u8], enc: &str) -> Result<Vec<u8>, BodyError> {
    match enc {
        "" => Ok(content.to_vec()),
        "gzip" | "x-gzip" => Err(BodyError::GzipNotPorted { encode: true }),
        _ => Err(BodyError::UnsupportedEncoding(enc.to_owned())),
    }
}

#[cfg(test)]
#[path = "body_test.rs"]
mod tests;
