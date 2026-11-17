use pbs_gojson::{GoEncode, RawJson, stdjson};

use super::{Body, BodyError};
use crate::golden::{TestResult, check, check_eq, u_escape};

fn encoded<T: GoEncode>(value: &T) -> Result<String, String> {
    stdjson::marshal(value)
        .map(|raw| String::from_utf8_lossy(&raw).into_owned())
        .map_err(|e| e.to_string())
}

// Go: parity/body_test.go TestNewBody.
#[test]
fn records_wire_bytes() -> TestResult {
    check_eq(&Body::new(b"", ""), &None, "empty")?;
    check_eq(
        &encoded(&Body::new(br#"{"a":1}"#, ""))?,
        &r#"{"text":"{\"a\":1}","json":{"a":1}}"#.to_owned(),
        "json",
    )?;
    check_eq(
        &encoded(&Body::new(b"hello <b>", ""))?,
        &format!(
            r#"{{"text":"hello {}b{}"}}"#,
            u_escape("003c"),
            u_escape("003e")
        ),
        "text (json.Marshal escapes HTML)",
    )?;
    check_eq(
        &encoded(&Body::new(b"\xff\xfe", ""))?,
        &r#"{"b64":"//4="}"#.to_owned(),
        "invalid UTF-8",
    )?;
    check_eq(
        &encoded(&Body::new(br#"{"a":1}"#, " IDENTITY "))?,
        &r#"{"text":"{\"a\":1}","json":{"a":1}}"#.to_owned(),
        "identity",
    )?;
    check_eq(
        &encoded(&Body::new(br#"{"a":1}"#, "BR"))?,
        &r#"{"b64":"eyJhIjoxfQ==","content_encoding":"br"}"#.to_owned(),
        "content-encoded bodies are base64 without json",
    )?;
    let gz = Body::new(b"\x1f\x8b", "gzip");
    check_eq(
        &gz.as_ref().map(|b| (b.b64.clone(), b.json.is_nil())),
        &Some((Some("H4s=".to_owned()), true)),
        "gzip (decoding not wired in yet)",
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
        b64: Some("aGk=".to_owned()),
        ..Body::default()
    };
    check_eq(&b64.wire_bytes(), &Ok(b"hi".to_vec()), "b64 wins")?;
    check_eq(&b64.decoded_bytes(), &Ok(b"hi".to_vec()), "b64 decoded")?;
    let json = Body {
        json: RawJson::from_slice(br#"{"a" : 1}"#),
        ..Body::default()
    };
    check_eq(
        &json.wire_bytes(),
        &Ok(br#"{"a" : 1}"#.to_vec()),
        "json wire",
    )?;
    check_eq(&Body::default().wire_bytes(), &Ok(Vec::new()), "none")?;
    check_eq(
        &Body::default().decoded_bytes(),
        &Ok(Vec::new()),
        "none decoded",
    )?;

    let bad = Body {
        b64: Some("!!".to_owned()),
        ..Body::default()
    };
    check_eq(
        &bad.wire_bytes().map_err(|e| e.to_string()),
        &Err("body.b64: illegal base64 data at input byte 0".to_owned()),
        "invalid base64",
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
    let gzip = Body {
        json: RawJson::from_slice(b"{}"),
        content_encoding: "gzip".to_owned(),
        ..Body::default()
    };
    check_eq(
        &gzip.wire_bytes(),
        &Err(BodyError::GzipNotPorted { encode: true }),
        "gzip encode",
    )?;
    let x_gzip = Body {
        b64: Some("H4s=".to_owned()),
        content_encoding: "x-gzip".to_owned(),
        ..Body::default()
    };
    check_eq(
        &x_gzip.decoded_bytes(),
        &Err(BodyError::GzipNotPorted { encode: false }),
        "gzip decode",
    )?;
    check(
        BodyError::GzipNotPorted { encode: false }
            .to_string()
            .starts_with("gzip decode: "),
        "gzip error text",
    )
}

#[test]
fn decodes_with_go_names() -> TestResult {
    let mut body = Body::default();
    stdjson::unmarshal(
        br#"{"TEXT":"a","B64":"YQ==","Json":{"x":1},"content_encoding":"gzip"}"#,
        &mut body,
    )
    .map_err(|e| e.to_string())?;
    check_eq(&body.text.as_deref(), &Some("a"), "text")?;
    check_eq(&body.b64.as_deref(), &Some("YQ=="), "b64")?;
    check_eq(&body.json.as_bytes(), &br#"{"x":1}"#.as_slice(), "json")?;
    check_eq(&body.content_encoding.as_str(), &"gzip", "encoding")
}
