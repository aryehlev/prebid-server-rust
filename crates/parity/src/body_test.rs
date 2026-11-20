use std::io::Write as _;

use base64::{DecodeError, Engine as _, engine::general_purpose::STANDARD};
use flate2::{Compression, write::GzEncoder};
use serde_json::value::RawValue;

use super::{Body, BodyError};
use crate::{
    envelope::{HeaderList, HeaderPair},
    golden::{Json, TestResult, check, check_eq, decode_b64, golden_json},
};

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

/// What Go's `TestNewBody` checks of a recorded body: the text, the base64, the `json` member's
/// text and the content encoding.
type Recorded<'a> = (Option<&'a str>, Option<&'a str>, Option<&'a str>, &'a str);

/// What [`Body::new`] recorded in `body`.
fn recorded(body: &Body) -> Recorded<'_> {
    (
        body.text.as_deref(),
        body.b64.as_deref(),
        body.json.as_deref().map(RawValue::get),
        body.content_encoding.as_str(),
    )
}

// Go: parity/body_test.go TestNewBody, case by case. Go's `json` member holds `{}\n` as recorded
// until `encoding/json` compacts it when the trace is written; a `RawValue` holds the value
// without the whitespace around it, so both write `{}`.
#[test]
fn new_body_cases() -> TestResult {
    let gz = gzipped(br#"{"a":1}"#)?;
    let gz_b64 = STANDARD.encode(&gz);
    let xx_b64 = STANDARD.encode(b"xx");
    let invalid_b64 = STANDARD.encode([0xFF, 0x00]);
    let cases: [(&str, &[u8], &str, Recorded<'_>); 7] = [
        (
            "json text",
            br#"{"a":1}"#,
            "",
            (Some(r#"{"a":1}"#), None, Some(r#"{"a":1}"#), ""),
        ),
        (
            "plain text keeps bytes",
            b"hello",
            "",
            (Some("hello"), None, None, ""),
        ),
        (
            "trailing newline kept in text",
            b"{}\n",
            "",
            (Some("{}\n"), None, Some("{}"), ""),
        ),
        (
            "invalid utf8 becomes b64",
            &[0xFF, 0x00],
            "",
            (None, Some(invalid_b64.as_str()), None, ""),
        ),
        (
            "gzip is b64 plus decoded json",
            &gz,
            "GZIP",
            (None, Some(gz_b64.as_str()), Some(r#"{"a":1}"#), "gzip"),
        ),
        (
            "identity encoding is dropped",
            b"[]",
            "identity",
            (Some("[]"), None, Some("[]"), ""),
        ),
        (
            "unsupported encoding has no json",
            b"xx",
            "br",
            (None, Some(xx_b64.as_str()), None, "br"),
        ),
    ];
    check(Body::new(&[], "").is_none(), "empty body is nil")?;
    for (name, raw, encoding, want) in cases {
        let body = Body::new(raw, encoding).ok_or_else(|| format!("{name}: no body"))?;
        check_eq(&recorded(&body), &want, name)?;
        check_eq(
            &body.wire_bytes(),
            &Ok(raw.to_vec()),
            &format!("{name}: recorded bodies must reproduce the exact wire bytes"),
        )?;
    }
    Ok(())
}

// Go: parity/body_test.go TestBodyWireAndDecodedBytes, case by case. Go's nil body is the empty
// `Body` here: a Rust body is not a pointer.
#[test]
fn wire_and_decoded_bytes_cases() -> TestResult {
    let text = |s: &str, encoding: &str| Body {
        text: Some(s.to_owned()),
        content_encoding: encoding.to_owned(),
        ..Body::default()
    };
    let json: Body = serde_json::from_str(r#"{"json":{"x":true}}"#).map_err(|e| e.to_string())?;
    let decoded: [(&str, Body, &[u8]); 3] = [
        ("nil body", Body::default(), b""),
        ("text", Body::text_body("abc"), b"abc"),
        ("json only", json, br#"{"x":true}"#),
    ];
    for (name, body, want) in decoded {
        check(body.wire_bytes().is_ok(), name)?;
        check_eq(&body.decoded_bytes(), &Ok(want.to_vec()), name)?;
    }
    let what = "text with gzip input is encoded by receiver";
    let wire = text("zip me", "gzip")
        .wire_bytes()
        .map_err(|e| format!("{what}: {e}"))?;
    let round_trip = Body::new(&wire, "gzip").ok_or(what)?;
    check_eq(&round_trip.decoded_bytes(), &Ok(b"zip me".to_vec()), what)?;
    check(b64_body("!!").wire_bytes().is_err(), "bad b64")?;
    check(
        text("x", "br").wire_bytes().is_err(),
        "unsupported input encoding",
    )
}

// Go: parity/body_test.go TestNewBody, beyond its table: the encoded documents, the encoding
// trimmed and spelled in any case, an x-gzip body and gzip that does not inflate.
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

// Go: parity/body_test.go TestBodyWireAndDecodedBytes, beyond its table: `b64` winning over
// `text`, a raw `json` kept as written, and a gzip input body's wire and decoded bytes.
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

// The bodies' gzip is flate2's, and their Content-Encoding is found and lower-cased by Rust's case
// rules (gzip.parity-bodies): flate2 compresses the same content to other bytes than compress/gzip
// (the bodies golden holds Go's) and words its errors its own way, and `str::to_lowercase` maps
// U+0130 to `i` and U+0307 and a final U+03A3 to U+03C2, where Go's `strings.ToLower` maps each
// rune on its own (parity/body.go:103-109), so `GZ\u{130}P` is no gzip here. The header names
// compare lower-cased, where Go's `HeaderList.Values` uses `strings.EqualFold`
// (parity/envelope.go:42-50), which the exec handler finds the Content-Encoding with
// (parity/server/exec.go:206-221). Go 1.26.2's `NewBody` and `Values` give the values named below.
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
    )?;
    check_eq(
        &Body::new(b"xx", "A\u{3a3}").map(|b| b.content_encoding),
        &Some("a\u{3c2}".to_owned()),
        "a final sigma, which Go lower-cases to a\u{3c3}",
    )?;
    check_eq(
        &Body::new(b"xx", "\u{3a3}").map(|b| b.content_encoding),
        &Some("\u{3c3}".to_owned()),
        "a sigma on its own, as Go",
    )?;

    let headers = HeaderList::from_pairs(vec![
        HeaderPair::new("S", "1"),
        HeaderPair::new("\u{17f}", "2"),
        HeaderPair::new("\u{391}\u{3a3}", "3"),
        HeaderPair::new("\u{130}", "4"),
    ]);
    check_eq(
        &headers.values("s"),
        &vec!["1"],
        "U+017F does not match s, where Go's [1 2] has it",
    )?;
    check_eq(
        &headers.values("\u{3b1}\u{3c3}"),
        &Vec::<&str>::new(),
        "a final capital sigma does not match U+03C3, where Go's [3] has it",
    )?;
    check_eq(
        &headers.values("\u{3b1}\u{3c2}"),
        &vec!["3"],
        "it matches U+03C2, as Go",
    )?;
    check_eq(
        &headers.values("i\u{307}"),
        &vec!["4"],
        "U+0130 matches i and U+0307, where Go's [] has nothing",
    )
}
