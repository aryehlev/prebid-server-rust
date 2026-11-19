//! Pins of pbs-server's gzip compression against gziphandler v1.1.1 as server/server.go uses it.
//!
//! `testdata/golden/gziphandler.golden` is what `testdata/go/gziphandler` recorded of
//! gziphandler: its `encodings` rows say, for 1,556 Accept-Encoding values, whether
//! `GzipHandler` compresses for a request that carries the value, and its `prebid` rows what
//! `getCompressionEnabledHandler` (server/server.go:120-125) does to six handler plans, with
//! `compression.response.enable_gzip` on and off and with and without `Accept-Encoding: gzip`.
//! The same requests go here, on loopback, to the same handlers behind
//! [`pbs_server::compression::compression_enabled`] on [`pbs_server::serve`].

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fmt::Debug,
        fs,
        io::{ErrorKind, Read as _},
        net::SocketAddr,
        time::Duration,
    };

    use axum::{
        Router,
        body::Body,
        http::{StatusCode, header},
        response::{IntoResponse as _, Response},
        routing::get,
    };
    use flate2::{Crc, read::GzDecoder};
    use pbs_server::{Budget, Stopped, compression, serve};
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
        ([(header::CONTENT_TYPE, content_type)], Body::from(bytes)).into_response()
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

    /// The data of a chunked body, or `None` when its framing is broken.
    fn unchunk(mut body: &[u8]) -> Option<Vec<u8>> {
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

    /// The length and CRC-32 (IEEE) of `bytes`, as the generator prints them.
    fn sum(bytes: &[u8]) -> String {
        let mut crc = Crc::new();
        crc.update(bytes);
        format!("{}/{:08x}", bytes.len(), crc.sum())
    }

    /// Whether `param` is one q-value as RFC 9110 writes it, which tower-http and gziphandler
    /// read alike: `q=`, then 0 or 1 with at most three decimals, at most 1.
    fn strict_q(param: &str) -> bool {
        let Some(number) = param.strip_prefix("q=") else {
            return false;
        };
        let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
        fraction.len() <= 3
            && fraction.bytes().all(|b| b.is_ascii_digit())
            && (whole == "0" || (whole == "1" && fraction.bytes().all(|b| b == b'0')))
    }

    /// The class of an Accept-Encoding value: the first of the features that can set
    /// gziphandler's answer apart from tower-http's, or `plain` when it has none.
    fn class(value: &[u8]) -> &'static str {
        let Ok(text) = str::from_utf8(value) else {
            return "non-ascii";
        };
        if value
            .iter()
            .any(|&b| b != b'\t' && !(0x20..0x7F).contains(&b))
        {
            return "non-ascii";
        }
        let items: Vec<(String, Option<&str>)> = text
            .split(',')
            .map(|item| {
                let (coding, params) = item
                    .split_once(';')
                    .map_or((item, None), |(coding, params)| (coding, Some(params)));
                (coding.trim().to_ascii_lowercase(), params)
            })
            .collect();
        let named = |name: &'static str| items.iter().filter(move |(coding, _)| coding == name);
        if named("x-gzip").next().is_some() {
            "x-gzip"
        } else if named("gzip").count() > 1 {
            "repeated"
        } else if named("gzip")
            .chain(named("identity"))
            .any(|(_, params)| params.is_some_and(|params| !strict_q(params.trim())))
        {
            "loose parameter"
        } else if named("identity").next().is_some() {
            "identity"
        } else {
            "plain"
        }
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

    /// Accept-Encoding values the negotiation pin shows with both answers: plain ones, and some
    /// of each class.
    const EXAMPLES: [&str; 21] = [
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
    ];

    /// Which Accept-Encoding values get gzip. gziphandler reads the header with its own parser
    /// and tower-http with RFC 9110's grammar, and of the golden's 1,556 values (the package's
    /// test values, hand-picked ones and 1,500 seeded combinations) 542 get different answers:
    /// 456 that gziphandler compresses and tower-http does not, 86 the other way. Each has at
    /// least one of five features, and the 345 values with none, `plain`, never differ:
    /// `non-ascii`, a byte outside visible ASCII and tab, so tower-http reads no Accept-Encoding
    /// at all, where gziphandler trims Unicode spaces and lower-cases Unicode (`gzİp` is gzip);
    /// `x-gzip`, which tower-http takes for gzip and gziphandler ignores; `repeated`, gzip
    /// twice, where gziphandler keeps the last q and tower-http the best; `loose parameter`, a
    /// parameter on gzip or identity that is not one strict q-value, which makes tower-http drop
    /// the coding, where gziphandler reads `ParseFloat`'s syntax clamped to 0 to 1, ignores `Q=`
    /// and other parameters and keeps the last parameter's q; and `identity`, which tower-http
    /// prefers to a gzip of lower q and gziphandler ignores.
    // Go: acceptsGzip (gziphandler v1.1.1 gzip.go:450-453) over parseEncodings and parseCoding
    // (gzip.go:483-532)
    #[test]
    fn gzip_negotiation() -> TestResult {
        let verdicts = verdicts()?;
        let classes = [
            "non-ascii",
            "x-gzip",
            "repeated",
            "loose parameter",
            "identity",
            "plain",
        ];
        let counts: Vec<String> = classes
            .iter()
            .map(|&name| {
                let of_class = verdicts.iter().filter(|(value, ..)| class(value) == name);
                let values = of_class.clone().count();
                let differ = of_class.filter(|(_, go, rust)| go != rust).count();
                format!("{name}: {values} values, {differ} differ")
            })
            .collect();
        ensure_eq(
            &counts,
            &[
                "non-ascii: 288 values, 201 differ",
                "x-gzip: 267 values, 100 differ",
                "repeated: 229 values, 81 differ",
                "loose parameter: 394 values, 158 differ",
                "identity: 33 values, 2 differ",
                "plain: 345 values, 0 differ",
            ],
            "the classes",
        )?;
        let only_go = verdicts
            .iter()
            .filter(|(_, go, rust)| *go && !*rust)
            .count();
        let only_rust = verdicts
            .iter()
            .filter(|(_, go, rust)| !*go && *rust)
            .count();
        ensure_eq(
            &(verdicts.len(), only_go, only_rust),
            &(1556, 456, 86),
            "values, gzip from gziphandler only, gzip from tower-http only",
        )?;
        let shown: Vec<String> = EXAMPLES
            .iter()
            .map(|example| {
                let (value, go, rust) = verdicts
                    .iter()
                    .find(|(value, ..)| value.as_slice() == example.as_bytes())
                    .ok_or_else(|| format!("the golden has no value {example:?}"))?;
                Ok(format!(
                    "{example:?} ({}): gziphandler {go}, tower-http {rust}",
                    class(value)
                ))
            })
            .collect::<TestResult<_>>()?;
        ensure_eq(
            &shown,
            &[
                "\"gzip\" (plain): gziphandler true, tower-http true",
                "\"compress, gzip\" (plain): gziphandler true, tower-http true",
                "\"GZIP\" (plain): gziphandler true, tower-http true",
                "\"gzip;q=0\" (plain): gziphandler false, tower-http false",
                "\"gzip;q=0.001\" (plain): gziphandler true, tower-http true",
                "\"br;q=1.0, gzip;q=0.8, *;q=0.1\" (plain): gziphandler true, tower-http true",
                "\"*\" (plain): gziphandler false, tower-http false",
                "\"\\u{a0}gzip\" (non-ascii): gziphandler true, tower-http false",
                "\"gzİp\" (non-ascii): gziphandler true, tower-http false",
                "\"x-gzip\" (x-gzip): gziphandler false, tower-http true",
                "\"gzip;q=0, gzip\" (repeated): gziphandler true, tower-http true",
                "\"gzip, gzip;q=0\" (repeated): gziphandler false, tower-http true",
                "\"gzip;q=2\" (loose parameter): gziphandler true, tower-http false",
                "\"gzip;Q=0\" (loose parameter): gziphandler true, tower-http false",
                "\"gzip;q = 0\" (loose parameter): gziphandler true, tower-http false",
                "\"gzip;q=inf\" (loose parameter): gziphandler true, tower-http false",
                "\"gzip;q=.5\" (loose parameter): gziphandler true, tower-http false",
                "\"gzip;q=0;foo\" (loose parameter): gziphandler true, tower-http false",
                "\"gzip;level=9\" (loose parameter): gziphandler true, tower-http false",
                "\"gzip; q=0.0000001\" (loose parameter): gziphandler true, tower-http false",
                "\";q=0;x , ;q=1.5, \\tgzip;q=0.5 ,identity\" (identity): gziphandler true, \
                 tower-http false",
            ],
            "the examples",
        )
    }

    /// One `prebid` row's answer: status, Content-Encoding, Content-Type and the body's length
    /// and CRC-32, with what it gunzips to when it is gzip.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Answer {
        /// The status code.
        code: String,
        /// Content-Encoding, or empty.
        encoding: String,
        /// Content-Type, or empty.
        content_type: String,
        /// The body as sent, without chunked framing: `<length>/<crc32>`.
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

    /// The gzip Prebid's servers send. getCompressionEnabledHandler gzips the same answers as
    /// tower-http's layer with gziphandler's 1,400-byte minimum: with gzip on, an answer of
    /// 1,400 bytes or more to a request that accepts gzip, whatever its type, and never a 204 or
    /// an empty answer; the types and every other body are the same. The gzip bytes are not:
    /// compress/gzip's deflate stream and flate2's differ in length and content (and tower-http
    /// chunks them, `server.framing`), though both gunzip to the handler's bytes.
    // Go: GzipHandler compresses at gzip.DefaultCompression (gziphandler v1.1.1 gzip.go:443-446)
    // from startGzip (gzip.go:150) on, through compress/gzip
    #[test]
    fn gzip_bytes_differ() -> TestResult {
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
                body: go_body.split('/').take(2).collect::<Vec<_>>().join("/"),
            };
            let name = format!("{enable}/{accept}/{plan}");
            let router = compression::compression_enabled(plans(), *enable == "true");
            let accept = (!accept.is_empty()).then_some(accept.as_bytes());
            let answers = exchange_all(router, vec![request(&format!("/{plan}"), accept)])?;
            let (head, body) = split(answers.first().map_or(&[][..], Vec::as_slice));
            let sent = if field(&head, "transfer-encoding") == "chunked" {
                unchunk(body).ok_or("a broken chunked body")?
            } else {
                body.to_vec()
            };
            let mut gunzipped = Vec::new();
            if field(&head, "content-encoding") == "gzip" {
                let _read = GzDecoder::new(sent.as_slice()).read_to_end(&mut gunzipped)?;
            }
            let rust = Answer {
                code: head.split(' ').nth(1).unwrap_or_default().to_owned(),
                encoding: field(&head, "content-encoding").to_owned(),
                content_type: field(&head, "content-type").to_owned(),
                body: sum(&sent),
            };
            cases.push((name, go, rust, gunzipped));
        }
        let mut gzipped = Vec::new();
        for (name, go, rust, gunzipped) in &cases {
            let same = |answer: &Answer| {
                (
                    answer.code.clone(),
                    answer.encoding.clone(),
                    answer.content_type.clone(),
                )
            };
            ensure_eq(&same(rust), &same(go), name)?;
            if go.encoding == "gzip" {
                gzipped.push(name.clone());
                if rust.body == go.body {
                    return Err(format!("{name}: flate2 wrote compress/gzip's bytes").into());
                }
                let plain = cases
                    .iter()
                    .find(|(other, ..)| *other == name.replacen("true", "false", 1))
                    .map(|(_, go_plain, ..)| go_plain.body.clone())
                    .ok_or("no row with gzip off")?;
                ensure_eq(&sum(gunzipped), &plain, name)?;
            } else {
                ensure_eq(&rust.body, &go.body, name)?;
            }
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
}
