use std::io::Write as _;

use base64::{DecodeError, Engine as _, engine::general_purpose::STANDARD};
use flate2::{Compression, write::GzEncoder};

use super::{Body, BodyError};
use crate::golden::{Json, TestResult, check, check_eq, decode_b64, golden_json};

/// `body` as `serde_json` writes it.
fn encoded(body: Option<&Body>) -> Result<String, String> {
    serde_json::to_string(&body).map_err(|e| e.to_string())
}

/// `content` gzip-compressed by flate2, as a server would send it.
fn gzipped(content: &[u8]) -> Result<Vec<u8>, String> {
    let mut zw = GzEncoder::new(Vec::new(), Compression::default());
    zw.write_all(content).map_err(|e| e.to_string())?;
    zw.finish().map_err(|e| e.to_string())
}

/// The `json` member of `body`, as text.
fn json_of(body: Option<&Body>) -> Option<String> {
    body.and_then(|b| b.json.as_ref())
        .map(|j| j.get().to_owned())
}

/// An input body holding `b64`.
fn b64_body(b64: &str) -> Body {
    Body {
        b64: Some(b64.to_owned()),
        ..Body::default()
    }
}

// Go: parity/body_test.go TestNewBody.
#[test]
fn records_wire_bytes() -> TestResult {
    check(Body::new(b"", "").is_none(), "empty")?;
    check_eq(
        &encoded(Body::new(br#"{"a":1}"#, "").as_ref())?,
        &r#"{"text":"{\"a\":1}","json":{"a":1}}"#.to_owned(),
        "json",
    )?;
    check_eq(
        &encoded(Body::new(b"\xff\xfe", "").as_ref())?,
        &r#"{"b64":"//4="}"#.to_owned(),
        "invalid UTF-8",
    )?;
    check_eq(
        &encoded(Body::new(br#"{"a":1}"#, " IDENTITY ").as_ref())?,
        &r#"{"text":"{\"a\":1}","json":{"a":1}}"#.to_owned(),
        "identity",
    )?;
    check_eq(
        &encoded(Body::new(br#"{"a":1}"#, "BR").as_ref())?,
        &r#"{"b64":"eyJhIjoxfQ==","content_encoding":"br"}"#.to_owned(),
        "content-encoded bodies are base64 without json",
    )?;
    let wire = gzipped(br#"{"a":1}"#)?;
    let gz = Body::new(&wire, "x-gzip");
    check_eq(
        &json_of(gz.as_ref()),
        &Some(r#"{"a":1}"#.to_owned()),
        "gzip content is inflated for json",
    )?;
    check_eq(
        &gz.as_ref().and_then(|b| b.b64.as_deref()).map(decode_b64),
        &Some(Ok(wire)),
        "gzip wire bytes are kept as base64",
    )?;
    check_eq(
        &encoded(Body::new(b"\x1f\x8b", "gzip").as_ref())?,
        &r#"{"b64":"H4s=","content_encoding":"gzip"}"#.to_owned(),
        "gzip that does not inflate has no json",
    )
}

// Go: parity/body_test.go TestWireBytes and TestDecodedBytes.
#[test]
fn produces_wire_and_decoded_bytes() -> TestResult {
    let text = Body::text_body("hi");
    check_eq(&text.wire_bytes(), &Ok(b"hi".to_vec()), "text wire")?;
    check_eq(&text.decoded_bytes(), &Ok(b"hi".to_vec()), "text decoded")?;
    let b64 = Body {
        text: Some("ignored".to_owned()),
        ..b64_body("aGk=")
    };
    check_eq(&b64.wire_bytes(), &Ok(b"hi".to_vec()), "b64 wins")?;
    check_eq(&b64.decoded_bytes(), &Ok(b"hi".to_vec()), "b64 decoded")?;
    let json: Body = serde_json::from_str(r#"{"json":{"a" : 1}}"#).map_err(|e| e.to_string())?;
    check_eq(
        &json.wire_bytes(),
        &Ok(br#"{"a" : 1}"#.to_vec()),
        "json wire keeps the raw value",
    )?;
    check_eq(&Body::default().wire_bytes(), &Ok(Vec::new()), "none")?;
    check_eq(
        &Body::default().decoded_bytes(),
        &Ok(Vec::new()),
        "none decoded",
    )?;
    let br = Body {
        text: Some("hi".to_owned()),
        content_encoding: "br".to_owned(),
        ..Body::default()
    };
    check_eq(
        &br.wire_bytes().map_err(|e| e.to_string()),
        &Err(r#"unsupported content encoding "br""#.to_owned()),
        "unsupported encoding",
    )?;
    check_eq(
        &br.decoded_bytes(),
        &Ok(b"hi".to_vec()),
        "text is the content",
    )?;

    let gzip: Body = serde_json::from_str(r#"{"json":{},"content_encoding":"GZIP"}"#)
        .map_err(|e| e.to_string())?;
    let wire = gzip.wire_bytes().map_err(|e| e.to_string())?;
    check_eq(
        &wire.get(..4),
        &Some([0x1F, 0x8B, 8, 0].as_slice()),
        "gzip header",
    )?;
    let recorded = Body::new(&wire, "gzip").ok_or("a gzip body")?;
    check_eq(
        &recorded.decoded_bytes(),
        &Ok(b"{}".to_vec()),
        "gzip decoded",
    )?;
    let truncated = Body {
        content_encoding: "x-gzip".to_owned(),
        ..b64_body("H4s=")
    };
    check(
        matches!(
            truncated.decoded_bytes(),
            Err(BodyError::Gzip { encode: false, .. })
        ),
        "gzip that does not inflate",
    )
}

/// The input of the bodies golden named `name`.
fn golden_input(name: &str) -> Result<Json, String> {
    golden_json("bodies.json")?
        .get("golden")?
        .array("inputs")?
        .into_iter()
        .find(|c| c.string("name").is_ok_and(|n| n == name))
        .ok_or_else(|| format!("no input {name} in bodies.json"))
}

// The base64 crate's standard engine reads `b64` (base64.parity-std-encoding): it refuses the
// line breaks and the set unused bits Go's `base64.StdEncoding` reads, and its messages are its
// own (`body.b64: Invalid symbol 33, offset 0.` where Go says `body.b64: illegal base64 data at
// input byte 0`). Go's results come from the bodies golden.
#[test]
fn b64_is_read_by_the_standard_engine() -> TestResult {
    let wire = |b64: &str| b64_body(b64).wire_bytes();
    check_eq(
        &wire("!!"),
        &Err(BodyError::Base64(DecodeError::InvalidByte(0, b'!'))),
        "invalid symbol",
    )?;
    check_eq(
        &wire("!!").map_err(|e| e.to_string()),
        &Err("body.b64: Invalid symbol 33, offset 0.".to_owned()),
        "the crate's message",
    )?;
    check(wire("aGl=").is_err(), "set unused bits are refused")?;
    check_eq(&wire("aGk="), &Ok(b"hi".to_vec()), "canonical")?;

    let newlines = golden_input("b64_newlines")?;
    check_eq(
        &newlines.string("wire_b64")?,
        &"aGk=".to_owned(),
        "Go skips the line break",
    )?;
    let body: Body = serde_json::from_str(&newlines.string("body")?).map_err(|e| e.to_string())?;
    check_eq(
        &body.wire_bytes(),
        &Err(BodyError::Base64(DecodeError::InvalidByte(2, b'\n'))),
        "the line break is refused here",
    )?;
    check_eq(
        &golden_input("b64_invalid")?.string("wire_error")?,
        &"body.b64: illegal base64 data at input byte 0".to_owned(),
        "Go's message",
    )
}

// The member names match exactly (json.parity-documents): Go fills `text`, `b64` and `json` from
// `TEXT`, `B64` and `Json`, serde leaves them unset.
#[test]
fn member_names_match_exactly() -> TestResult {
    let body: Body = serde_json::from_str(
        r#"{"TEXT":"a","B64":"YQ==","Json":{"x":1},"content_encoding":"gzip"}"#,
    )
    .map_err(|e| e.to_string())?;
    check_eq(&body.text, &None, "text")?;
    check_eq(&body.b64, &None, "b64")?;
    check(body.json.is_none(), "json")?;
    check_eq(&body.content_encoding.as_str(), &"gzip", "encoding")
}

/// `wire` read as a gzip input body, inflated.
fn inflated(wire: &[u8]) -> Result<Vec<u8>, BodyError> {
    Body {
        content_encoding: "gzip".to_owned(),
        ..b64_body(&STANDARD.encode(wire))
    }
    .decoded_bytes()
}

// The bodies' gzip is flate2's, and their Content-Encoding is lower-cased by `str::to_lowercase`
// (gzip.parity-bodies): flate2 compresses the same content to other bytes than compress/gzip (the
// bodies golden holds Go's) and words its errors its own way, and U+0130 lower-cases to `i` and
// U+0307, where Go's `strings.ToLower` gives `i`, so `GZ\u{130}P` is no gzip here.
#[test]
fn gzip_is_flate2s_and_case_mapping_rusts() -> TestResult {
    let go = golden_input("json_gzip")?;
    let go_wire = decode_b64(&go.string("wire_b64")?)?;
    let body: Body = serde_json::from_str(&go.string("body")?).map_err(|e| e.to_string())?;
    let wire = body.wire_bytes().map_err(|e| e.to_string())?;
    check(wire != go_wire, "other gzip bytes")?;
    check_eq(&inflated(&wire), &inflated(&go_wire), "the same content")?;
    check_eq(
        &inflated(b"").map_err(|e| e.to_string()),
        &Err("gzip decode: unexpected end of file".to_owned()),
        "flate2's message, where Go says gzip decode: EOF",
    )?;

    let recorded = Body::new(&wire, "GZ\u{130}P").ok_or("a recorded body")?;
    check_eq(
        &recorded.content_encoding.as_str(),
        &"gzi\u{307}p",
        "lower-cased",
    )?;
    check(recorded.json.is_none(), "not inflated")?;
    let input = Body {
        text: Some("{}".to_owned()),
        content_encoding: "GZ\u{130}P".to_owned(),
        ..Body::default()
    };
    check_eq(
        &input.wire_bytes().map_err(|e| e.to_string()),
        &Err(r#"unsupported content encoding "gzi\u{307}p""#.to_owned()),
        "not compressed",
    )
}
