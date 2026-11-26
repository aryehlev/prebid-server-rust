//! Pins of pbs-server's gzip compression against gziphandler v1.1.1 as server/server.go uses it.
//!
//! `testdata/golden/gziphandler.golden` is what `testdata/go/gziphandler` recorded of
//! gziphandler: its `encodings` rows say, for 1,679 Accept-Encoding values, whether
//! `GzipHandler` compresses for a request that carries the value, and its `prebid` rows what
//! `getCompressionEnabledHandler` (server/server.go:120-125) does to six handler plans, with
//! `compression.response.enable_gzip` on and off and with and without `Accept-Encoding: gzip`.
//! The same requests go here, on loopback, to the same handlers behind
//! [`pbs_server::compression::compression_enabled`] on [`pbs_server::serve`]. The gzip bytes
//! are fastgzip's, not compress/gzip's, so the pins compare what they gunzip to and the gzip
//! header's fixed fields, never the compressed bytes.

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        error::Error,
        fmt::Debug,
        fs,
        io::{self, ErrorKind, Read as _},
        net::SocketAddr,
        ops::Range,
        time::Duration,
    };

    use axum::{
        Router,
        body::{Body, Bytes},
        extract::Query,
        http::{StatusCode, header},
        response::{IntoResponse as _, Response},
        routing::get,
    };
    use flate2::{Crc, read::GzDecoder};
    use futures_util::stream;
    use pbs_server::{Budget, Stopped, compression, serve};
    use serde_json::Value;
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream},
        runtime::{self, Runtime},
        sync::oneshot,
        task::{self, JoinSet},
        time,
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
            .ok_or("a row value does not start with a quote")?;
        let mut bytes = Vec::new();
        let mut chars = rest.char_indices();
        while let Some((at, c)) = chars.next() {
            match c {
                '"' => return Ok((bytes, rest.get(at + 1..).unwrap_or_default())),
                '\\' => {
                    let (_, escape) = chars.next().ok_or("an escape at the end of a row")?;
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
                        let decoded =
                            char::from_u32(value).ok_or("an escape that is no character")?;
                        bytes.extend_from_slice(decoded.encode_utf8(&mut [0; 4]).as_bytes());
                    }
                }
                c => bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            }
        }
        Err("a row value without its closing quote".into())
    }

    /// One row of the golden: its input and Go's output.
    #[derive(Debug, Clone)]
    struct Row {
        /// The input, unquoted.
        input: Vec<u8>,
        /// Go's output, unquoted.
        output: String,
    }

    /// The golden's rows of `kind` (`encodings`, `contenttype`, `config`, `gzip` or `prebid`),
    /// its header checked.
    fn rows(kind: &str) -> TestResult<Vec<Row>> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/golden/gziphandler.golden"
        );
        let text = fs::read_to_string(path)?;
        let mut lines = text.lines();
        ensure_eq(
            lines.next().unwrap_or_default(),
            "# Generated by testdata/go/gziphandler (go1.26.2, github.com/NYTimes/gziphandler \
             v1.1.1). Do not edit.",
            "the golden's header",
        )?;
        let mut rows = Vec::new();
        for line in lines {
            let (row_kind, rest) = line.split_once(' ').ok_or("a row without a kind")?;
            if row_kind != kind {
                continue;
            }
            let (input, rest) = unquote(rest)?;
            let (output, rest) = unquote(rest.strip_prefix(' ').ok_or("a row with one value")?)?;
            ensure_eq(rest, "", "the end of a row")?;
            rows.push(Row {
                input,
                output: String::from_utf8(output)?,
            });
        }
        Ok(rows)
    }

    /// A runtime for the loopback server and clients.
    fn runtime() -> TestResult<Runtime> {
        Ok(runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?)
    }

    /// Sends `request` to `address` on a fresh connection and reads the answer until the server
    /// closes the connection.
    async fn exchange(address: SocketAddr, request: Vec<u8>) -> TestResult<Vec<u8>> {
        let mut stream = TcpStream::connect(address).await?;
        stream.write_all(&request).await?;
        let mut answer = Vec::new();
        match time::timeout(Duration::from_secs(20), stream.read_to_end(&mut answer)).await? {
            Ok(_) => Ok(answer),
            Err(e) if e.kind() == ErrorKind::ConnectionReset => Ok(answer),
            Err(e) => Err(e.into()),
        }
    }

    /// Serves `router` on a loopback port within the main server's budget, sends each of
    /// `requests` on its own connection, at most 32 at a time, and returns the answers in order.
    fn exchange_all(router: Router, requests: Vec<Vec<u8>>) -> TestResult<Vec<Vec<u8>>> {
        runtime()?.block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let (stop, stopped) = oneshot::channel::<()>();
            let server = task::spawn(serve(listener, router, Budget::MAIN, async move {
                stopped.await.unwrap_or_default();
            }));
            let mut answers = vec![Vec::new(); requests.len()];
            let mut pending = requests.into_iter().enumerate();
            let mut running = JoinSet::new();
            loop {
                while running.len() < 32 {
                    let Some((at, request)) = pending.next() else {
                        break;
                    };
                    let _handle =
                        running.spawn(async move { (at, exchange(address, request).await) });
                }
                let Some(done) = running.join_next().await else {
                    break;
                };
                let (at, answer) = done?;
                if let Some(slot) = answers.get_mut(at) {
                    *slot = answer?;
                }
            }
            stop.send(()).map_err(|()| "the server stopped early")?;
            ensure_eq(&server.await?, &Stopped::Drained, "the server's shutdown")?;
            Ok(answers)
        })
    }

    /// `n` bytes cycling through the alphabet, as the generator's `pattern`.
    fn pattern(n: usize) -> Vec<u8> {
        (b'a'..=b'z').cycle().take(n).collect()
    }

    /// `n` bytes of the generator's xorshift64 sequence, which deflate cannot shrink.
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

    /// An answer of `bytes` with the type net/http gives the generator's handler.
    fn typed(content_type: &'static str, bytes: Vec<u8>) -> Response {
        typed_body(content_type, Body::from(bytes))
    }

    /// An answer of `body` with `content_type`.
    fn typed_body(content_type: &'static str, body: Body) -> Response {
        ([(header::CONTENT_TYPE, content_type)], body).into_response()
    }

    /// The generator's plans as routes: `/` writes nothing, `/w10` and `/w2000` write pattern
    /// bytes (which net/http sniffs as text/plain; charset=utf-8), `/z5000` noise
    /// (application/octet-stream), `/s204` sets 204 and `/tjson.w1400` sets application/json
    /// and writes 1,400 pattern bytes. The handlers set the types net/http's sniffing gave Go's,
    /// so only the compression differs (sniffing is `server.content-sniffing`).
    fn plans() -> Router {
        Router::new()
            .route("/", get(|| async {}))
            .route(
                "/w10",
                get(|| async { typed("text/plain; charset=utf-8", pattern(10)) }),
            )
            .route(
                "/w2000",
                get(|| async { typed("text/plain; charset=utf-8", pattern(2000)) }),
            )
            .route(
                "/z5000",
                get(|| async { typed("application/octet-stream", noise(5000)) }),
            )
            .route("/s204", get(|| async { StatusCode::NO_CONTENT }))
            .route(
                "/tjson.w1400",
                get(|| async { typed("application/json", pattern(1400)) }),
            )
            .route("/stream", get(streamed))
    }

    /// `/stream?n=N&chunk=C&length=1`: N pattern bytes as text/plain, streamed in chunks of C
    /// bytes (64 KiB by default) as `ServeDir` streams a file, with a Content-Length when
    /// `length` is set, as `ServeDir` sets one.
    async fn streamed(Query(params): Query<BTreeMap<String, String>>) -> Response {
        let number = |name: &str, default: usize| {
            params
                .get(name)
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        };
        let n = number("n", 0);
        let bytes = Bytes::from(pattern(n));
        let chunk = number("chunk", 64 << 10).max(1);
        let chunks: Vec<Result<Bytes, io::Error>> = (0..n)
            .step_by(chunk)
            .map(|at| Ok(bytes.slice(at..n.min(at + chunk))))
            .collect();
        let mut response = typed_body(
            "text/plain; charset=utf-8",
            Body::from_stream(stream::iter(chunks)),
        );
        if params.contains_key("length") {
            drop(
                response
                    .headers_mut()
                    .insert(header::CONTENT_LENGTH, n.into()),
            );
        }
        response
    }

    /// A GET request for `path` with `accept` as its Accept-Encoding, when there is one.
    fn request(path: &str, accept: Option<&[u8]>) -> Vec<u8> {
        let mut request = format!("GET {path} HTTP/1.1\r\nHost: pbs.example\r\n").into_bytes();
        if let Some(accept) = accept {
            request.extend_from_slice(b"Accept-Encoding: ");
            request.extend_from_slice(accept);
            request.extend_from_slice(b"\r\n");
        }
        request.extend_from_slice(b"Connection: close\r\n\r\n");
        request
    }

    /// The head of an answer, as text, and its body.
    fn split(answer: &[u8]) -> (String, &[u8]) {
        let at = answer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap_or(answer.len());
        let (head, body) = answer.split_at(at);
        (
            String::from_utf8_lossy(head).into_owned(),
            body.get(4..).unwrap_or_default(),
        )
    }

    /// The value of the field `name` (case-insensitive) in `head`, or empty.
    fn field<'h>(head: &'h str, name: &str) -> &'h str {
        head.split("\r\n")
            .skip(1)
            .filter_map(|line| line.split_once(": "))
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map_or("", |(_, value)| value)
    }

    /// The length and CRC-32 (IEEE) of `bytes`, as the generator prints them.
    fn sum(bytes: &[u8]) -> String {
        let mut crc = Crc::new();
        crc.update(bytes);
        format!("{}/{:08x}", bytes.len(), crc.sum())
    }

    /// Each `encodings` value of the golden with whether gziphandler compresses for it and
    /// whether pbs-server's compression does.
    fn verdicts() -> TestResult<Vec<(Vec<u8>, bool, bool)>> {
        let encodings = rows("encodings")?;
        let requests = encodings
            .iter()
            .map(|row| request("/w2000", Some(&row.input)))
            .collect();
        let answers = exchange_all(compression::compression_enabled(plans(), true), requests)?;
        let mut verdicts = Vec::new();
        for (row, answer) in encodings.into_iter().zip(&answers) {
            let (head, _) = split(answer);
            ensure_eq(
                head.split("\r\n").next().unwrap_or_default(),
                "HTTP/1.1 200 OK",
                &format!("the answer to {:?}", String::from_utf8_lossy(&row.input)),
            )?;
            let go = row.output.ends_with(" accepts=true");
            let rust = field(&head, "content-encoding") == "gzip";
            verdicts.push((row.input, go, rust));
        }
        Ok(verdicts)
    }

    /// Whether `number`, the text after a `q=`, is a spelling `strconv.ParseFloat` and std's
    /// parser read otherwise (`server.gzip-qvalue-syntax`): a hexadecimal mantissa or an `_`,
    /// which only Go reads, a sign before `nan`, which only std reads, or a finite number past
    /// the largest float, which Go refuses and std reads as infinity.
    fn go_only_syntax(number: &str) -> bool {
        let unsigned = number
            .strip_prefix(['+', '-'])
            .unwrap_or(number)
            .to_ascii_lowercase();
        number.contains(['x', 'X', '_'])
            || (unsigned.len() < number.len() && unsigned == "nan")
            || number
                .parse::<f64>()
                .is_ok_and(|q| q.is_infinite() && !unsigned.starts_with("inf"))
    }

    /// Whether one of the `q=` numbers of the Accept-Encoding value `value` is such a spelling.
    fn has_go_only_q(value: &[u8]) -> bool {
        String::from_utf8_lossy(value)
            .split([',', ';'])
            .filter_map(|part| part.trim().strip_prefix("q="))
            .any(go_only_syntax)
    }

    /// Accept-Encoding values the negotiation pin shows with both answers: plain ones, ones
    /// tower-http's RFC 9110 reading answered otherwise (Unicode spaces and letters, `x-gzip`, a
    /// repeated gzip, parameters that are not one strict q and `identity`), and q values at the
    /// edges of `ParseFloat`'s syntax, which std's parser reads alike or otherwise
    /// (`server.gzip-qvalue-syntax`).
    const EXAMPLES: [&str; 29] = [
        "gzip",
        "compress, gzip",
        "GZIP",
        "gzip;q=0",
        "gzip;q=0.001",
        "br;q=1.0, gzip;q=0.8, *;q=0.1",
        "*",
        "\u{a0}gzip",
        "gzİp",
        "x-gzip",
        "gzip;q=0, gzip",
        "gzip, gzip;q=0",
        "gzip;q=2",
        "gzip;Q=0",
        "gzip;q = 0",
        "gzip;q=inf",
        "gzip;q=.5",
        "gzip;q=0;foo",
        "gzip;level=9",
        "gzip; q=0.0000001",
        ";q=0;x , ;q=1.5, \tgzip;q=0.5 ,identity",
        "gzip;q=1e-400",
        "gzip;q=1_000",
        "gzip;q=0x1p-1074",
        "gzip;q=0x1p-1075",
        "gzip;q=1e309",
        "gzip;q=+nan",
        "\u{200b}gzip",
        "gzip, gzip;q=abc;x",
    ];

    /// Which Accept-Encoding values get gzip: pbs-server reads Accept-Encoding with gziphandler's
    /// parser, so the golden's 1,679 values (the package's test values, hand-picked ones, 1,500
    /// seeded combinations and 123 q values and codings at the edges of `ParseFloat`'s syntax
    /// and of Unicode) get gziphandler's answer, the 542 values tower-http's RFC 9110 reading
    /// answered otherwise among them. The 102 values that get another answer each have a q in a
    /// spelling `strconv.ParseFloat` and std's float parser read otherwise
    /// (`server.gzip-qvalue-syntax`).
    // Go: acceptsGzip (gziphandler v1.1.1 gzip.go:450-453) over parseEncodings and parseCoding
    // (gzip.go:483-532)
    #[test]
    fn gzip_negotiation() -> TestResult {
        let verdicts = verdicts()?;
        let differ: Vec<&(Vec<u8>, bool, bool)> =
            verdicts.iter().filter(|(_, go, rust)| go != rust).collect();
        let unexpected: Vec<String> = differ
            .iter()
            .filter(|(value, ..)| !has_go_only_q(value))
            .map(|(value, go, _)| format!("{:?}: gziphandler {go}", String::from_utf8_lossy(value)))
            .collect();
        ensure_eq(
            &unexpected,
            &Vec::<String>::new(),
            "values pbs-server answers otherwise",
        )?;
        let gzipped = verdicts.iter().filter(|(_, go, _)| *go).count();
        ensure_eq(
            &(verdicts.len(), gzipped, differ.len()),
            &(1679, 794, 102),
            "values, values that get gzip, and values of `server.gzip-qvalue-syntax`",
        )?;
        let shown: Vec<String> = EXAMPLES
            .iter()
            .map(|example| {
                let (_, go, rust) = verdicts
                    .iter()
                    .find(|(value, ..)| value.as_slice() == example.as_bytes())
                    .ok_or_else(|| format!("the golden has no value {example:?}"))?;
                Ok(format!("{example:?}: gziphandler {go}, pbs-server {rust}"))
            })
            .collect::<TestResult<_>>()?;
        ensure_eq(
            &shown,
            &[
                "\"gzip\": gziphandler true, pbs-server true",
                "\"compress, gzip\": gziphandler true, pbs-server true",
                "\"GZIP\": gziphandler true, pbs-server true",
                "\"gzip;q=0\": gziphandler false, pbs-server false",
                "\"gzip;q=0.001\": gziphandler true, pbs-server true",
                "\"br;q=1.0, gzip;q=0.8, *;q=0.1\": gziphandler true, pbs-server true",
                "\"*\": gziphandler false, pbs-server false",
                "\"\\u{a0}gzip\": gziphandler true, pbs-server true",
                "\"gzİp\": gziphandler true, pbs-server true",
                "\"x-gzip\": gziphandler false, pbs-server false",
                "\"gzip;q=0, gzip\": gziphandler true, pbs-server true",
                "\"gzip, gzip;q=0\": gziphandler false, pbs-server false",
                "\"gzip;q=2\": gziphandler true, pbs-server true",
                "\"gzip;Q=0\": gziphandler true, pbs-server true",
                "\"gzip;q = 0\": gziphandler true, pbs-server true",
                "\"gzip;q=inf\": gziphandler true, pbs-server true",
                "\"gzip;q=.5\": gziphandler true, pbs-server true",
                "\"gzip;q=0;foo\": gziphandler true, pbs-server true",
                "\"gzip;level=9\": gziphandler true, pbs-server true",
                "\"gzip; q=0.0000001\": gziphandler true, pbs-server true",
                "\";q=0;x , ;q=1.5, \\tgzip;q=0.5 ,identity\": gziphandler true, pbs-server true",
                "\"gzip;q=1e-400\": gziphandler false, pbs-server false",
                "\"gzip;q=1_000\": gziphandler true, pbs-server false",
                "\"gzip;q=0x1p-1074\": gziphandler true, pbs-server false",
                "\"gzip;q=0x1p-1075\": gziphandler false, pbs-server false",
                "\"gzip;q=1e309\": gziphandler false, pbs-server true",
                "\"gzip;q=+nan\": gziphandler false, pbs-server false",
                "\"\\u{200b}gzip\": gziphandler false, pbs-server false",
                "\"gzip, gzip;q=abc;x\": gziphandler true, pbs-server true",
            ],
            "the examples",
        )
    }

    /// One `prebid` row's answer: status, Content-Encoding, Content-Type, Vary and the body's
    /// length and CRC-32, or for a gzip body those of what it gunzips to.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Answer {
        /// The status code.
        code: String,
        /// Content-Encoding, or empty.
        encoding: String,
        /// Content-Type, or empty.
        content_type: String,
        /// Vary, or empty.
        vary: String,
        /// The body as sent, `<length>/<crc32>`.
        body: String,
    }

    /// The value of `name` in the header section of a Go `prebid` output, or empty.
    fn go_field(output: &str, name: &str) -> TestResult<String> {
        let header = output
            .split_once(" header:")
            .and_then(|(_, rest)| rest.split_once(" body="))
            .map_or("", |(header, _)| header);
        let Some((_, value)) = header.split_once(&format!(" {name}=")) else {
            return Ok(String::new());
        };
        let (value, _) = unquote(value)?;
        Ok(String::from_utf8(value)?)
    }

    /// gzip's fixed header fields as RFC 1952 numbers them: ID1, ID2, CM and FLG, MTIME, XFL
    /// and OS, in hex.
    fn gzip_header(gzip: &[u8]) -> String {
        let hex = |range: Range<usize>| {
            gzip.get(range)
                .unwrap_or_default()
                .iter()
                .flat_map(|byte| [byte >> 4, byte & 0x0F])
                .map(|nibble| char::from_digit(u32::from(nibble), 16).unwrap_or('?'))
                .collect::<String>()
        };
        format!(
            "id, cm, flg {} | mtime {} | xfl {} | os {}",
            hex(0..4),
            hex(4..8),
            hex(8..9),
            hex(9..10)
        )
    }

    /// The trailer of a gzip stream as the length and CRC-32 of what it holds, as the generator
    /// prints a body: ISIZE and CRC32, little-endian.
    fn gzip_trailer(gzip: &[u8]) -> TestResult<String> {
        let at = gzip
            .len()
            .checked_sub(8)
            .ok_or("a gzip stream shorter than its trailer")?;
        let word = |from: usize| -> TestResult<u32> {
            let bytes: [u8; 4] = gzip
                .get(from..from + 4)
                .ok_or("a short trailer")?
                .try_into()?;
            Ok(u32::from_le_bytes(bytes))
        };
        Ok(format!("{}/{:08x}", word(at + 4)?, word(at)?))
    }

    /// The gzip Prebid's servers send: with gzip on, an answer of 1,400 bytes or more to a
    /// request that accepts gzip is compressed, whatever its type, and never a 204 or an empty
    /// answer; every answer, compressed or not, has `Vary: Accept-Encoding`, and the status and
    /// type are the handler's. On pbs-server as from gziphandler, the same answers are
    /// compressed, with the same fields, and gunzip to the handler's bytes, which the gzip
    /// trailer's length and CRC-32 name. The gzip bytes are fastgzip's: its header says XFL 4
    /// where compress/gzip's says 0 (`gzip.payload-bytes`), its deflate stream is another, of
    /// another length, and it is sent with a Content-Length.
    // Go: GzipHandler compresses at gzip.DefaultCompression (gziphandler v1.1.1 gzip.go:443-446)
    // from startGzip (gzip.go:150) on, through compress/gzip
    #[test]
    fn gzip_bytes() -> TestResult {
        let prebid = rows("prebid")?;
        let mut cases = Vec::new();
        for row in &prebid {
            let input = String::from_utf8(row.input.clone())?;
            let parts: Vec<&str> = input.split('\u{1f}').collect();
            let [enable, accept, plan] = parts.as_slice() else {
                return Err(format!("a prebid row of {} parts", parts.len()).into());
            };
            let code = row
                .output
                .split_once(" code=")
                .and_then(|(_, rest)| rest.split_once(' '))
                .map_or("", |(code, _)| code)
                .to_owned();
            let go_body = row.output.split_once(" body=").map_or("", |(_, body)| body);
            let go = Answer {
                code,
                encoding: go_field(&row.output, "Content-Encoding")?,
                content_type: go_field(&row.output, "Content-Type")?,
                vary: go_field(&row.output, "Vary")?,
                body: go_body.split('/').take(2).collect::<Vec<_>>().join("/"),
            };
            let name = format!("{enable}/{accept}/{plan}");
            let router = compression::compression_enabled(plans(), *enable == "true");
            let accept = (!accept.is_empty()).then_some(accept.as_bytes());
            let answers = exchange_all(router, vec![request(&format!("/{plan}"), accept)])?;
            let (head, sent) = split(answers.first().map_or(&[][..], Vec::as_slice));
            // A 204 has no length and no bytes.
            let length = field(&head, "content-length");
            ensure_eq(
                if length.is_empty() { "0" } else { length },
                sent.len().to_string().as_str(),
                &format!("{name}: the length"),
            )?;
            let encoding = field(&head, "content-encoding").to_owned();
            let mut fixed = String::new();
            let body = if encoding == "gzip" {
                let mut gunzipped = Vec::new();
                let _read = GzDecoder::new(sent).read_to_end(&mut gunzipped)?;
                ensure_eq(
                    &gzip_trailer(sent)?,
                    &sum(&gunzipped),
                    &format!("{name}: the trailer"),
                )?;
                fixed = gzip_header(sent);
                sum(&gunzipped)
            } else {
                sum(sent)
            };
            let rust = Answer {
                code: head.split(' ').nth(1).unwrap_or_default().to_owned(),
                encoding,
                content_type: field(&head, "content-type").to_owned(),
                vary: field(&head, "vary").to_owned(),
                body,
            };
            cases.push((name, go, rust, fixed));
        }
        let mut gzipped = Vec::new();
        for (name, go, rust, fixed) in &cases {
            let mut want = go.clone();
            if go.encoding == "gzip" {
                gzipped.push(name.clone());
                ensure_eq(
                    fixed.as_str(),
                    "id, cm, flg 1f8b0800 | mtime 00000000 | xfl 04 | os ff",
                    &format!("{name}: fastgzip's header"),
                )?;
                want.body = cases
                    .iter()
                    .find(|(other, ..)| *other == name.replacen("true", "false", 1))
                    .map(|(_, go_plain, ..)| go_plain.body.clone())
                    .ok_or("no row with gzip off")?;
            }
            ensure_eq(rust, &want, name)?;
        }
        ensure_eq(
            &gzipped,
            &[
                "true/gzip/w2000",
                "true/gzip/z5000",
                "true/gzip/tjson.w1400",
            ],
            "the gzipped answers",
        )
    }

    /// Bytes of a hexadecimal string.
    fn unhex(hex: &str) -> TestResult<Vec<u8>> {
        (0..hex.len())
            .step_by(2)
            .map(|at| {
                let pair = hex.get(at..at + 2).ok_or("an odd hex string")?;
                Ok(u8::from_str_radix(pair, 16)?)
            })
            .collect()
    }

    /// pbs-differences' gzip golden: the bid request the exchange compresses for a bidder with
    /// `endpointCompression: gzip`, and what compress/gzip writes of it, its header checked.
    fn bid_request_golden() -> TestResult<(String, Vec<u8>)> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../differences/testdata/golden/gzip.json"
        );
        let doc: Value = serde_json::from_slice(&fs::read(path)?)?;
        let text = |pointer: &str| -> TestResult<String> {
            doc.pointer(pointer)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("the golden has no string at {pointer}").into())
        };
        ensure_eq(
            text("/header/generator")?.as_str(),
            "crates/differences/testdata/go",
            "the gzip golden's generator",
        )?;
        ensure_eq(
            text("/header/goarch")?.as_str(),
            "amd64",
            "the gzip golden's goarch",
        )?;
        Ok((text("/payload")?, unhex(&text("/compressed_hex")?)?))
    }

    /// compress/gzip and fastgzip write the same gzip header but for XFL (compress/gzip's 0 at
    /// its default level, fastgzip's 4, "fastest", at level 0) and the same trailer, and both
    /// streams inflate to the bid request; the deflate streams between them differ, so a bidder
    /// or a publisher sees other compressed bytes and another Content-Length.
    // Go: exchange/bidder.go:848-874 (getRequestBody: w.Reset, Write, Close) and :876-880
    // (gzipWriterPool of gzip.NewWriter at the default level); compress/gzip's Writer.Write
    // writes XFL 0 below BestCompression
    #[test]
    fn go_difference_header_same_payload() -> TestResult {
        let (payload, go) = bid_request_golden()?;
        let mut compressor = fastgzip::Gzip::new();
        let rust = compressor.compress(payload.as_bytes()).to_vec();
        let inflate = |gzip: &[u8]| -> TestResult<bool> {
            let mut plain = Vec::new();
            let _read = GzDecoder::new(gzip).read_to_end(&mut plain)?;
            Ok(plain == payload.as_bytes())
        };
        let table = |gzip: &[u8]| -> TestResult<Vec<String>> {
            Ok(vec![
                gzip_header(gzip),
                format!("trailer {}", gzip_trailer(gzip)?),
                format!("inflates to the payload {}", inflate(gzip)?),
            ])
        };
        ensure_eq(
            &table(&go)?,
            &[
                "id, cm, flg 1f8b0800 | mtime 00000000 | xfl 00 | os ff",
                "trailer 896/b7352e45",
                "inflates to the payload true",
            ],
            "compress/gzip",
        )?;
        ensure_eq(
            &table(&rust)?,
            &[
                "id, cm, flg 1f8b0800 | mtime 00000000 | xfl 04 | os ff",
                "trailer 896/b7352e45",
                "inflates to the payload true",
            ],
            "fastgzip",
        )
    }

    /// Streamed bodies, which `ServeDir` gives its files, and long ones. A stream of up to
    /// [`compression::STREAM_LIMIT`] bytes is read whole and compressed, with a Content-Length
    /// of its gzip; a longer one is sent on as it is, chunked or with the handler's length, with
    /// `Vary` and without gzip, so no answer is held in memory past that size. gziphandler
    /// compresses those too, as the handler writes. A body in memory past
    /// [`compression::SHARED_LIMIT`] bytes is compressed by a compressor of its own.
    // Go: GzipResponseWriter.Write starts the gzip writer once 1,400 bytes are buffered and
    // writes the rest through it (gziphandler v1.1.1 gzip.go:100-147)
    #[test]
    fn long_streams_uncompressed() -> TestResult {
        let limit = compression::STREAM_LIMIT;
        let big = compression::SHARED_LIMIT * 2;
        let cases = [
            (
                "19,572 bytes, chunked",
                "/stream?n=19572&chunk=8192".to_owned(),
            ),
            (
                "19,572 bytes, with a length",
                "/stream?n=19572&chunk=8192&length=1".to_owned(),
            ),
            (
                "the limit, with a length",
                format!("/stream?n={limit}&length=1"),
            ),
            (
                "a byte past the limit, chunked",
                format!("/stream?n={}", limit + 1),
            ),
            (
                "a byte past the limit, with a length",
                format!("/stream?n={}&length=1", limit + 1),
            ),
            (
                "twice the limit, with a length",
                format!("/stream?n={big}&length=1"),
            ),
            ("twice the limit, in memory", format!("/w?n={big}")),
        ];
        let router = compression::compression_enabled(
            plans().route(
                "/w",
                get(
                    |Query(params): Query<BTreeMap<String, String>>| async move {
                        let n = params.get("n").and_then(|n| n.parse().ok()).unwrap_or(0);
                        typed("text/plain; charset=utf-8", pattern(n))
                    },
                ),
            ),
            true,
        );
        let requests = cases
            .iter()
            .map(|(_, path)| request(path, Some(b"gzip")))
            .collect();
        let answers = exchange_all(router, requests)?;
        let mut seen = Vec::new();
        for ((name, path), answer) in cases.iter().zip(&answers) {
            let (head, body) = split(answer);
            let n: usize = path
                .split_once("n=")
                .and_then(|(_, rest)| rest.split('&').next())
                .and_then(|n| n.parse().ok())
                .ok_or("a path without n")?;
            let encoding = field(&head, "content-encoding");
            let plain = if encoding == "gzip" {
                let mut gunzipped = Vec::new();
                let _read = GzDecoder::new(body).read_to_end(&mut gunzipped)?;
                gunzipped
            } else if field(&head, "transfer-encoding") == "chunked" {
                unchunked(body).ok_or("a broken chunked body")?
            } else {
                body.to_vec()
            };
            ensure_eq(&sum(&plain), &sum(&pattern(n)), path)?;
            let framing = if field(&head, "transfer-encoding") == "chunked" {
                "chunked".to_owned()
            } else if field(&head, "content-length") == body.len().to_string() {
                "length of what is sent".to_owned()
            } else {
                format!("length {}", field(&head, "content-length"))
            };
            seen.push(format!(
                "{name}: {} | vary {} | {framing}",
                if encoding.is_empty() {
                    "as it is"
                } else {
                    encoding
                },
                field(&head, "vary"),
            ));
        }
        ensure_eq(
            &seen,
            &[
                "19,572 bytes, chunked: gzip | vary Accept-Encoding | length of what is sent",
                "19,572 bytes, with a length: gzip | vary Accept-Encoding | length of what is \
                 sent",
                "the limit, with a length: gzip | vary Accept-Encoding | length of what is sent",
                "a byte past the limit, chunked: as it is | vary Accept-Encoding | chunked",
                "a byte past the limit, with a length: as it is | vary Accept-Encoding | length \
                 of what is sent",
                "twice the limit, with a length: as it is | vary Accept-Encoding | length of \
                 what is sent",
                "twice the limit, in memory: gzip | vary Accept-Encoding | length of what is \
                 sent",
            ],
            "streamed and long answers",
        )
    }

    /// The data of a chunked body, or `None` when its framing is broken.
    fn unchunked(mut body: &[u8]) -> Option<Vec<u8>> {
        let mut plain = Vec::new();
        loop {
            let end = body.windows(2).position(|window| window == b"\r\n")?;
            let (line, rest) = body.split_at_checked(end)?;
            let size = usize::from_str_radix(str::from_utf8(line).ok()?, 16).ok()?;
            if size == 0 {
                return Some(plain);
            }
            plain.extend_from_slice(rest.get(2..2 + size)?);
            body = rest.get(4 + size..)?;
        }
    }
}
