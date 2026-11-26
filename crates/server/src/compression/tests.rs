//! Unit pins of the gzip layer against gziphandler v1.1.1, without a server: the q values it
//! reads and the decisions its writer makes, replayed from `testdata/golden/gziphandler.golden`,
//! and what the layer does with bodies Go's writer never sees whole.

use std::{
    error::Error,
    fmt::Debug,
    fs,
    io::{self, Read as _},
};

use axum::{
    body::{self, Body, Bytes},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::Response,
};
use flate2::{Crc, read::GzDecoder};
use futures_util::{StreamExt as _, stream};
use tokio::runtime;

use super::{
    Kind, SHARED_LIMIT, STREAM_LIMIT, answer, atoi, gzip_payload,
    negotiation::{accepts_gzip, gzip_q, parse_float},
    read, vary_first,
};

/// A failed pin.
type BoxError = Box<dyn Error + Send + Sync>;

/// What the pins return.
type TestResult<T = ()> = Result<T, BoxError>;

/// Fails, showing both values, unless `got` equals `want`.
fn ensure_eq<A: PartialEq<B> + Debug + ?Sized, B: Debug + ?Sized>(
    got: &A,
    want: &B,
    what: &str,
) -> TestResult {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}:\n got {got:#?}\nwant {want:#?}").into())
    }
}

/// Reads one Go string literal as `strconv.Quote` writes it from the start of `text`, and
/// returns its bytes and the text after it.
fn unquote(text: &str) -> TestResult<(Vec<u8>, &str)> {
    let rest = text
        .strip_prefix('"')
        .ok_or("a value does not start with a quote")?;
    let mut bytes = Vec::new();
    let mut chars = rest.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Ok((bytes, rest.get(at + 1..).unwrap_or_default())),
            '\\' => {
                let (_, escape) = chars.next().ok_or("an escape at the end of a value")?;
                let simple = match escape {
                    'a' => Some(0x07),
                    'b' => Some(0x08),
                    'f' => Some(0x0C),
                    'n' => Some(b'\n'),
                    'r' => Some(b'\r'),
                    't' => Some(b'\t'),
                    'v' => Some(0x0B),
                    '\\' => Some(b'\\'),
                    '"' => Some(b'"'),
                    _ => None,
                };
                if let Some(byte) = simple {
                    bytes.push(byte);
                    continue;
                }
                let digits = match escape {
                    'x' => 2,
                    'u' => 4,
                    'U' => 8,
                    other => return Err(format!("the escape \\{other}").into()),
                };
                let hex: String = chars
                    .by_ref()
                    .take(digits)
                    .map(|(_, digit)| digit)
                    .collect();
                let value = u32::from_str_radix(&hex, 16)?;
                if escape == 'x' {
                    bytes.push(u8::try_from(value)?);
                } else {
                    let decoded = char::from_u32(value).ok_or("an escape that is no character")?;
                    bytes.extend_from_slice(decoded.encode_utf8(&mut [0; 4]).as_bytes());
                }
            }
            c => bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
        }
    }
    Err("a value without its closing quote".into())
}

/// The golden's rows of `kind`: each input, unquoted, with Go's output.
fn rows(kind: &str) -> TestResult<Vec<(Vec<u8>, String)>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/golden/gziphandler.golden"
    );
    let text = fs::read_to_string(path)?;
    let mut rows = Vec::new();
    for line in text.lines().skip(1) {
        let (row_kind, rest) = line.split_once(' ').ok_or("a row without a kind")?;
        if row_kind != kind {
            continue;
        }
        let (input, rest) = unquote(rest)?;
        let (output, rest) = unquote(rest.strip_prefix(' ').ok_or("a row with one value")?)?;
        ensure_eq(rest, "", "the end of a row")?;
        rows.push((input, String::from_utf8(output)?));
    }
    Ok(rows)
}

/// The q `parseEncodings` gave gzip in an `encodings` output, as Go formats it, or `None`.
fn go_gzip_q(output: &str) -> TestResult<Option<String>> {
    let mut rest = output;
    while rest.starts_with('"') {
        let (coding, after) = unquote(rest)?;
        let (q, after) = after
            .strip_prefix('=')
            .and_then(|after| after.split_once(' '))
            .ok_or("a coding without its q")?;
        if coding == b"gzip" {
            return Ok(Some(q.to_owned()));
        }
        rest = after;
    }
    Ok(None)
}

/// gzip's q for each of the golden's 1,679 Accept-Encoding values is the one `parseEncodings`
/// gives it, to the bit, NaN included, and absent where Go's is: the q values at the edges of
/// `ParseFloat`'s syntax, Unicode spaces and letters, bytes that are not UTF-8, repeated
/// codings and parameters.
// Go: parseEncodings and parseCoding (gziphandler v1.1.1 gzip.go:483-532) over
// strconv.ParseFloat (internal/strconv/atof.go)
#[test]
fn q_values_like_go() -> TestResult {
    let encodings = rows("encodings")?;
    let mut differ = Vec::new();
    let mut with_gzip = 0_usize;
    for (value, output) in &encodings {
        let go = go_gzip_q(output)?.map(|q| q.parse::<f64>()).transpose()?;
        let rust = gzip_q(Some(value));
        let same = match (go, rust) {
            (Some(go), Some(rust)) => {
                go.to_bits() == rust.to_bits() || (go.is_nan() && rust.is_nan())
            }
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        };
        if !same {
            differ.push(format!(
                "{:?}: Go {go:?}, pbs-server {rust:?}",
                String::from_utf8_lossy(value)
            ));
        }
        with_gzip += usize::from(go.is_some());
    }
    ensure_eq(&differ, &Vec::<String>::new(), "q values")?;
    ensure_eq(
        &(encodings.len(), with_gzip),
        &(1679, 1062),
        "values, and values that give gzip a q",
    )
}

/// `strconv.ParseFloat(s, 64)` before gziphandler clamps it: Go's syntax, its range errors and
/// its rounding.
// Go: ParseFloat (internal/strconv/atof.go); the values as go1.26.2 prints them
#[test]
fn parse_float_like_go() -> TestResult {
    let cases: [(&str, Option<f64>); 14] = [
        ("0.5", Some(0.5)),
        ("1_0", Some(10.0)),
        ("0x1p-2", Some(0.25)),
        ("0x1.8p1", Some(3.0)),
        ("-0x1p-2", Some(-0.25)),
        ("2", Some(2.0)),
        ("1e400", None),
        ("0x1p1024", None),
        ("1e-400", Some(0.0)),
        ("+inf", Some(f64::INFINITY)),
        ("-Infinity", Some(f64::NEG_INFINITY)),
        ("+nan", None),
        ("infin", None),
        ("", None),
    ];
    for (text, want) in cases {
        ensure_eq(
            &parse_float(text).map(f64::to_bits),
            &want.map(f64::to_bits),
            text,
        )?;
    }
    ensure_eq(&parse_float("NaN").map(f64::is_nan), &Some(true), "NaN")
}

/// `n` bytes cycling through the alphabet, as the generator's `pattern`.
fn pattern(n: usize) -> Vec<u8> {
    (b'a'..=b'z').cycle().take(n).collect()
}

/// `n` bytes of the generator's xorshift64 sequence, as its `noise`.
fn noise(n: usize) -> Vec<u8> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x.to_be_bytes().first().copied().unwrap_or_default()
        })
        .collect()
}

/// The length and CRC-32 (IEEE) of `bytes`, as the generator prints them.
fn sum(bytes: &[u8]) -> String {
    let mut crc = Crc::new();
    crc.update(bytes);
    format!("{}/{:08x}", bytes.len(), crc.sum())
}

/// What `bytes` gunzip to.
fn gunzip(bytes: &[u8]) -> TestResult<Vec<u8>> {
    let mut plain = Vec::new();
    let _read = GzDecoder::new(bytes).read_to_end(&mut plain)?;
    Ok(plain)
}

/// Sets the header `name` to `value`.
fn set(headers: &mut HeaderMap, name: HeaderName, value: &str) -> TestResult {
    drop(headers.insert(name, HeaderValue::from_str(value)?));
    Ok(())
}

/// A generator plan as the whole answer an axum handler gives: its status and headers, and its
/// writes as one body. `None` for a plan no such answer stands for: one that writes nothing
/// (`n`), flushes or panics, sets a header or the status after it wrote, or writes a body
/// beside a status that allows none.
fn plan(spec: &str) -> TestResult<Option<(Response, Vec<u8>)>> {
    let mut status = None;
    let mut headers = HeaderMap::new();
    let mut bytes = Vec::new();
    for op in spec.split('.').filter(|op| !op.is_empty()) {
        let (kind, arg) = op.split_at_checked(1).ok_or("an empty step")?;
        let n = || arg.parse::<usize>().unwrap_or(0);
        match kind {
            "w" | "x" => bytes.extend(pattern(n())),
            "z" => bytes.extend(noise(n())),
            "b" => bytes.extend_from_slice(match arg {
                "html" => b"<html>".as_slice(),
                "notfound" => b"404 page not found\n".as_slice(),
                other => return Err(format!("no body {other}").into()),
            }),
            _ if !bytes.is_empty() => return Ok(None),
            "s" => status = status.or(Some(u16::try_from(n())?)),
            "t" => match arg {
                "nil" => drop(headers.remove(header::CONTENT_TYPE)),
                "empty" => set(&mut headers, header::CONTENT_TYPE, "")?,
                "json" => set(&mut headers, header::CONTENT_TYPE, "application/json")?,
                "html" => set(
                    &mut headers,
                    header::CONTENT_TYPE,
                    "text/html; charset=utf-8",
                )?,
                other => return Err(format!("no type {other}").into()),
            },
            "l" => set(&mut headers, header::CONTENT_LENGTH, arg)?,
            "e" => set(&mut headers, header::CONTENT_ENCODING, arg)?,
            _ => return Ok(None),
        }
    }
    let status = StatusCode::from_u16(status.unwrap_or(200))?;
    let bodiless = status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED;
    if bodiless && !bytes.is_empty() {
        return Ok(None);
    }
    let mut response = Response::new(Body::from(bytes.clone()));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(Some((response, bytes)))
}

/// The values of `name` in the header section of a Go `gzip` output, joined by `, `.
fn go_header(output: &str, name: &str) -> TestResult<String> {
    let mut rest = output
        .split_once(" header:")
        .and_then(|(_, rest)| rest.rsplit_once(" body="))
        .map_or("", |(header, _)| header);
    let mut values = Vec::new();
    while let Some(entry) = rest.strip_prefix(' ') {
        let (field, value) = entry.split_once('=').ok_or("a header without =")?;
        let (value, after) = match value.strip_prefix("<none>") {
            Some(after) => (Vec::new(), after),
            None => unquote(value)?,
        };
        if field == name {
            values.push(String::from_utf8(value)?);
        }
        rest = after;
    }
    Ok(values.join(", "))
}

/// The values of `name` in `headers`, joined by `, `.
fn rust_header(headers: &HeaderMap, name: &HeaderName) -> String {
    headers
        .get_all(name)
        .iter()
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

/// gziphandler's writer decides on what the handler set and wrote, and the layer decides alike
/// on the whole answer: for each of the 452 `gzip` rows of the golden with Prebid's options
/// (none) whose plan an axum handler's answer stands for, every Accept-Encoding of the rows
/// included, the
/// status, Content-Encoding, Content-Length and Vary are Go's, and the body is Go's or, where
/// both compress it, gunzips to the handler's bytes. The rows hold the handler's Content-Length
/// as `strconv.Atoi` reads it (`+1400`, ` 1400`, `abc`, past the range), a Content-Encoding set
/// or empty, statuses, types and bodies of several writes on either side of 1,400 bytes.
// Go: GzipResponseWriter.Write, startGzip and startPlain (gziphandler v1.1.1 gzip.go:100-206)
#[test]
fn writer_decisions_like_go() -> TestResult {
    let runtime = runtime::Builder::new_current_thread().build()?;
    let mut replayed = 0_usize;
    for (input, output) in rows("gzip")? {
        let input = String::from_utf8(input)?;
        let fields: Vec<&str> = input.split('\u{1f}').collect();
        let [opts, accept, spec] = fields.as_slice() else {
            return Err(format!("a gzip row of {} fields", fields.len()).into());
        };
        if !opts.is_empty() {
            continue;
        }
        let Some((response, bytes)) = plan(spec)? else {
            continue;
        };
        replayed += 1;
        let handler_gzip = rust_header(response.headers(), &header::CONTENT_ENCODING) == "gzip";
        let accepts = accepts_gzip((!accept.is_empty()).then_some(accept.as_bytes()));
        let (head, sent) = runtime.block_on(answer(accepts, response)).into_parts();
        let sent = runtime.block_on(body::to_bytes(sent, usize::MAX))?;
        let encoding = rust_header(&head.headers, &header::CONTENT_ENCODING);
        let compressed = encoding == "gzip" && !handler_gzip;
        let decided = |code: &str, ce: &str, length: String, vary: String, body: String| {
            format!("{code} | encoding {ce} | length {length} | vary {vary} | {body}")
        };
        let go_code = output
            .split_once(" code=")
            .and_then(|(_, rest)| rest.split_once(' '))
            .map_or("", |(code, _)| code);
        let go_body = output
            .rsplit_once(" body=")
            .map_or("", |(_, body)| body)
            .split('/')
            .take(2)
            .collect::<Vec<_>>()
            .join("/");
        let go = decided(
            go_code,
            &go_header(&output, "Content-Encoding")?,
            go_header(&output, "Content-Length")?,
            go_header(&output, "Vary")?,
            if compressed {
                format!("gzip of {}", sum(&bytes))
            } else {
                go_body
            },
        );
        let rust = decided(
            head.status.as_str(),
            &encoding,
            rust_header(&head.headers, &header::CONTENT_LENGTH),
            rust_header(&head.headers, &header::VARY),
            if compressed {
                format!("gzip of {}", sum(&gunzip(&sent)?))
            } else {
                sum(&sent)
            },
        );
        ensure_eq(&rust, &go, &format!("{accept:?} {spec}"))?;
    }
    ensure_eq(&replayed, &452, "the rows replayed")
}

/// gziphandler's `strconv.Atoi` of Content-Length with its error dropped: a sign and digits,
/// 0 for anything else, and the nearest `int64` past the range.
// Go: Atoi (internal/strconv/atoi.go), as GzipResponseWriter.Write calls it (gziphandler v1.1.1
// gzip.go:116)
#[test]
fn content_length_like_atoi() -> TestResult {
    let cases: [(&[u8], i64); 14] = [
        (b"", 0),
        (b"0", 0),
        (b"1400", 1400),
        (b"+1400", 1400),
        (b"-1", -1),
        (b" 1400", 0),
        (b"1400 ", 0),
        (b"abc", 0),
        (b"1_400", 0),
        (b"0x578", 0),
        (b"+", 0),
        (b"-", 0),
        (b"99999999999999999999", i64::MAX),
        (b"-99999999999999999999", i64::MIN),
    ];
    for (text, want) in cases {
        ensure_eq(&atoi(text), &want, &String::from_utf8_lossy(text))?;
    }
    Ok(())
}

/// `Vary: Accept-Encoding` goes ahead of the values the handler added, as gziphandler's `Add`
/// before the handler leaves it; one the handler added itself stays as well.
// Go: w.Header().Add(vary, acceptEncoding) (gziphandler v1.1.1 gzip.go:324)
#[test]
fn vary_ahead_of_the_handler() -> TestResult {
    let mut headers = HeaderMap::new();
    vary_first(&mut headers);
    ensure_eq(
        &rust_header(&headers, &header::VARY),
        "Accept-Encoding",
        "no Vary",
    )?;
    let mut added = HeaderMap::new();
    let _: bool = added.append(header::VARY, HeaderValue::from_static("Origin"));
    let _: bool = added.append(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    vary_first(&mut added);
    ensure_eq(
        &rust_header(&added, &header::VARY),
        "Accept-Encoding, Origin, Accept-Encoding",
        "the handler's Vary",
    )
}

/// The bytes of `body`'s chunks, and the error that ends it, if one does.
async fn drain(body: Body) -> (Vec<u8>, Option<String>) {
    let mut bytes = Vec::new();
    let mut chunks = body.into_data_stream();
    while let Some(chunk) = chunks.next().await {
        match chunk {
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(error) => return (bytes, Some(error.to_string())),
        }
    }
    (bytes, None)
}

/// `bytes` as a body streamed in chunks of 64 KiB, which ends with an error when `failing`.
fn streamed(bytes: &[u8], failing: bool) -> Body {
    let mut chunks: Vec<Result<Bytes, io::Error>> = bytes
        .chunks(64 << 10)
        .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
        .collect();
    if failing {
        chunks.push(Err(io::Error::other("the file went away")));
    }
    Body::from_stream(stream::iter(chunks))
}

/// A streamed body is read up to [`STREAM_LIMIT`] bytes and a body in memory whole. One that
/// goes past the limit, or fails, comes back as a body that sends every byte the stream had and
/// then its error, so the answer goes out uncompressed as the handler gave it.
#[test]
fn stream_reads() -> TestResult {
    let runtime = runtime::Builder::new_current_thread().build()?;
    let at_limit = pattern(STREAM_LIMIT);
    let whole = runtime.block_on(read(streamed(&at_limit, false), false));
    ensure_eq(
        &whole.map(|bytes| sum(&bytes)).ok(),
        &Some(sum(&at_limit)),
        "a stream at the limit",
    )?;
    let past = pattern(STREAM_LIMIT + 1);
    let Err(past_back) = runtime.block_on(read(streamed(&past, false), false)) else {
        return Err("a stream past the limit was read".into());
    };
    let (past_bytes, past_error) = runtime.block_on(drain(past_back));
    ensure_eq(
        &(sum(&past_bytes), past_error),
        &(sum(&past), None),
        "a stream past the limit",
    )?;
    let short = pattern(3000);
    let Err(failed_back) = runtime.block_on(read(streamed(&short, true), false)) else {
        return Err("a failing stream was read".into());
    };
    let (failed_bytes, failed_error) = runtime.block_on(drain(failed_back));
    ensure_eq(
        &(sum(&failed_bytes), failed_error),
        &(sum(&short), Some("the file went away".to_owned())),
        "a failing stream",
    )?;
    let in_memory = runtime.block_on(read(streamed(&past, false), true));
    ensure_eq(
        &in_memory.map(|bytes| sum(&bytes)).ok(),
        &Some(sum(&past)),
        "a body read whole",
    )
}

/// The kinds of payload a worker thread keeps a compressor for, by the answer's media type.
#[test]
fn payload_kinds() -> TestResult {
    let cases = [
        (Some("application/json"), Kind::Json),
        (Some("application/json; charset=utf-8"), Kind::Json),
        (Some("Application/JSON"), Kind::Json),
        (Some("application/vnd.api+json"), Kind::Json),
        (Some("text/html; charset=utf-8"), Kind::Text),
        (Some("text/plain"), Kind::Text),
        (Some("application/javascript"), Kind::Text),
        (Some("application/xml"), Kind::Text),
        (Some("image/svg+xml"), Kind::Text),
        (Some("application/x-yaml"), Kind::Text),
        (Some("application/octet-stream"), Kind::Other),
        (Some("image/png"), Kind::Other),
        (Some(""), Kind::Other),
        (None, Kind::Other),
    ];
    for (content_type, want) in cases {
        let mut headers = HeaderMap::new();
        if let Some(value) = content_type {
            drop(headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(value)));
        }
        ensure_eq(&Kind::of(&headers), &want, content_type.unwrap_or("none"))?;
    }
    Ok(())
}

/// Each kind's compressor, payloads of one kind after another's, and a payload past
/// [`SHARED_LIMIT`] bytes, which gets a compressor of its own, all write gzip that gunzips to
/// the payload.
#[test]
fn compressors_gunzip() -> TestResult {
    let json = br#"{"id":"1","seatbid":[{"bid":[{"id":"b","impid":"1","price":0.5}]}]}"#.repeat(40);
    let payloads = [
        (Kind::Json, json.clone()),
        (Kind::Text, pattern(5000)),
        (Kind::Json, json),
        (Kind::Other, noise(5000)),
        (Kind::Text, pattern(SHARED_LIMIT + 1)),
    ];
    for (kind, payload) in payloads {
        ensure_eq(
            &sum(&gunzip(&gzip_payload(kind, &payload))?),
            &sum(&payload),
            &format!("{kind:?} of {} bytes", payload.len()),
        )?;
    }
    Ok(())
}
