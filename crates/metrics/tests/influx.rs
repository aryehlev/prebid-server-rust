//! The `InfluxDB` reporter against `testdata/golden/gometrics`, which `testdata/go/influx.go` and
//! `influxedge.go` recorded from influxdata/influxdb1-client 8bf82d3c094d and
//! vrischmann/go-metrics-influxdb v0.1.1 talking to a raw HTTP endpoint that `support::endpoint`
//! reimplements: the requests of the client's writes and pings and what it made of the answers,
//! redirected to another port of the host or to the endpoint itself as well, the reporter's
//! batches and log lines, the client's spelling of points and keys, the host URLs and
//! `Time.Truncate`. The tests are also the pins of the ledger's `influx` entries.

pub mod support;

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        mem, thread,
        time::{Duration, Instant},
    };

    use influxdb::{Query as _, Type};
    use pbs_logger::{Capture, Severity};
    use pbs_metrics::{
        MetricsEngine as _,
        gometrics::{Registry, Sample, new_exp_decay_sample, new_uniform_sample},
        influx::{Client, Settings, USER_AGENT, point, run, truncate},
        metrics::GoDuration,
        new_metrics_engine,
        options::{InfluxMetrics, Metrics},
    };
    use reqwest::Url;
    use tokio::{
        runtime,
        time::{sleep, timeout},
    };

    use super::support::{
        TestResult, check, check_eq,
        endpoint::{Endpoint, refused_addr},
        fnv1a64, go_unquote_bytes, golden, path_unescape,
    };

    /// Go: `ok204` of `wireCases` and `writeReporter`, without the version header.
    const NO_CONTENT: &[u8] = b"HTTP/1.1 204 No Content\r\n\r\n";

    /// The fields of one row of quoted, tab-separated fields.
    fn quoted_fields(row: &str) -> TestResult<Vec<Vec<u8>>> {
        let mut fields = Vec::new();
        let mut rest = row;
        loop {
            let (field, after) = go_unquote_bytes(rest)?;
            fields.push(field);
            match after.strip_prefix('\t') {
                Some(next) => rest = next,
                None if after.is_empty() => return Ok(fields),
                None => return Err(format!("{row:?}: junk after a field").into()),
            }
        }
    }

    /// The rows of the golden `gometrics/<name>`, each a list of `N` quoted fields.
    fn quoted_rows<const N: usize>(name: &str) -> TestResult<Vec<[String; N]>> {
        golden(&format!("gometrics/{name}"))?
            .lines()
            .map(|row| {
                let fields = quoted_fields(row)?
                    .into_iter()
                    .map(String::from_utf8)
                    .collect::<Result<Vec<_>, _>>()?;
                fields.try_into().map_err(|fields: Vec<String>| {
                    format!("{name}: {} fields in {row:?}", fields.len()).into()
                })
            })
            .collect()
    }

    /// A single-threaded runtime with the clock and I/O.
    fn runtime() -> TestResult<runtime::Runtime> {
        Ok(runtime::Builder::new_current_thread()
            .enable_all()
            .build()?)
    }

    /// Go: `answer`: a response with the status line and a `Content-Length` body.
    fn answer(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    /// One point of a test, as Go's `client.Point` holds it: the tags in a map.
    struct TestPoint {
        measurement: String,
        tags: BTreeMap<String, String>,
        key: String,
        value: Type,
        unix_nanos: i128,
    }

    impl TestPoint {
        /// The point the reporter would build of it.
        fn build(&self) -> influxdb::WriteQuery {
            let tags: Vec<(&str, &str)> = self
                .tags
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            point(
                &self.measurement,
                &tags,
                &self.key,
                self.value.clone(),
                self.unix_nanos,
            )
        }

        /// The crate's line of the point.
        fn line(&self) -> TestResult<String> {
            Ok(self.build().build()?.get())
        }
    }

    /// The value of a point field, `f` (IEEE-754 hex) or `i`.
    fn field_value(kind: &str, value: &str) -> TestResult<Type> {
        match kind {
            "f" => Ok(Type::Float(f64::from_bits(u64::from_str_radix(value, 16)?))),
            "i" => Ok(Type::SignedInteger(value.parse()?)),
            _ => Err(format!("field kind {kind:?}").into()),
        }
    }

    /// Tags given as key, value, key, value..., into Go's map: the last value of a key wins.
    fn tag_map(parts: &[&str]) -> TestResult<BTreeMap<String, String>> {
        let mut tags = BTreeMap::new();
        for pair in parts.chunks(2) {
            let [k, v] = pair else {
                return Err(format!("odd tags {parts:?}").into());
            };
            drop(tags.insert((*k).to_owned(), (*v).to_owned()));
        }
        Ok(tags)
    }

    /// Go: `wirePoint.encode`, read back.
    fn wire_point(encoded: &str) -> TestResult<TestPoint> {
        let parts: Vec<&str> = encoded.split('\u{1f}').collect();
        let [measurement, tags, key, kind, value, unix_nanos] = parts.as_slice() else {
            return Err(format!("{encoded:?}: not a point").into());
        };
        let tag_parts: Vec<&str> = tags.split('\u{1e}').filter(|_| !tags.is_empty()).collect();
        Ok(TestPoint {
            measurement: (*measurement).to_owned(),
            tags: tag_map(&tag_parts)?,
            key: (*key).to_owned(),
            value: field_value(kind, value)?,
            unix_nanos: unix_nanos.parse()?,
        })
    }

    /// One row of `wire.txt`: Go's `wireCase` and what influxdb1-client did with it.
    struct WireCase {
        name: String,
        url: String,
        username: String,
        password: String,
        op: String,
        db: String,
        points: Vec<TestPoint>,
        responses: Vec<Vec<u8>>,
        has_endpoint: bool,
        requests: Vec<String>,
        error: Option<String>,
        version: String,
    }

    /// A field as text.
    fn text(field: &[u8]) -> TestResult<String> {
        Ok(String::from_utf8(field.to_vec())?)
    }

    /// The parts of a field joined by U+001D, none for an empty field.
    fn split(field: &str) -> Vec<String> {
        field
            .split('\u{1d}')
            .filter(|_| !field.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// The responses of a field, joined by U+001D.
    fn responses(field: &[u8]) -> Vec<Vec<u8>> {
        field.split(|&b| b == 0x1D).map(<[u8]>::to_vec).collect()
    }

    /// The points of a field, `wirePoint.encode`s joined by U+001D.
    fn points(field: &[u8]) -> TestResult<Vec<TestPoint>> {
        split(&text(field)?).iter().map(|p| wire_point(p)).collect()
    }

    /// Go: `wireCases`, as `writeWire` recorded them.
    fn wire_cases() -> TestResult<Vec<WireCase>> {
        let mut cases = Vec::new();
        for row in golden("gometrics/wire.txt")?.lines() {
            let fields = quoted_fields(row)?;
            let [
                name,
                url,
                username,
                password,
                op,
                db,
                point_field,
                answers,
                has,
                requests,
                error,
                version,
            ] = fields.as_slice()
            else {
                return Err(format!("{row:?}: not 12 fields").into());
            };
            cases.push(WireCase {
                name: text(name)?,
                url: text(url)?,
                username: text(username)?,
                password: text(password)?,
                op: text(op)?,
                db: text(db)?,
                points: points(point_field)?,
                responses: responses(answers),
                has_endpoint: text(has)? == "true",
                requests: split(&text(requests)?),
                error: text(error)?.strip_prefix("error: ").map(str::to_owned),
                version: text(version)?,
            });
        }
        Ok(cases)
    }

    /// One row of `redirects.txt`: a wire case whose endpoint redirects it to a second endpoint
    /// on another port of the host or to an absolute URL of its own, and what influxdb1-client
    /// sent to each.
    struct RedirectCase {
        /// The case, with the first endpoint's responses, which name it `{addr}` and the second
        /// endpoint `{addr2}`, and the requests it read.
        case: WireCase,
        /// The second endpoint's responses.
        next_responses: Vec<Vec<u8>>,
        /// The requests the second endpoint read, none when the first redirects to itself.
        next_requests: Vec<String>,
    }

    /// Go: `redirectCases`, as `writeRedirects` recorded them.
    fn redirect_cases() -> TestResult<Vec<RedirectCase>> {
        let mut cases = Vec::new();
        for row in golden("gometrics/redirects.txt")?.lines() {
            let fields = quoted_fields(row)?;
            let [
                name,
                url,
                username,
                password,
                op,
                db,
                point_field,
                answers,
                next_answers,
                requests,
                next_requests,
                error,
                version,
            ] = fields.as_slice()
            else {
                return Err(format!("{row:?}: not 13 fields").into());
            };
            cases.push(RedirectCase {
                case: WireCase {
                    name: text(name)?,
                    url: text(url)?,
                    username: text(username)?,
                    password: text(password)?,
                    op: text(op)?,
                    db: text(db)?,
                    points: points(point_field)?,
                    responses: responses(answers),
                    has_endpoint: true,
                    requests: split(&text(requests)?),
                    error: text(error)?.strip_prefix("error: ").map(str::to_owned),
                    version: text(version)?,
                },
                next_responses: responses(next_answers),
                next_requests: split(&text(next_requests)?),
            });
        }
        Ok(cases)
    }

    /// The wire case named `name`.
    fn wire_case<'a>(cases: &'a [WireCase], name: &str) -> TestResult<&'a WireCase> {
        cases
            .iter()
            .find(|case| case.name == name)
            .ok_or_else(|| format!("no wire case {name}").into())
    }

    /// What the reporter's client did with a wire case.
    struct Wire {
        requests: Vec<String>,
        error: Option<String>,
        version: String,
    }

    /// The case through [`Client`], against an endpoint with its responses when it has one. A
    /// host URL the url crate refuses fails before any request, with `parse: <error>`.
    fn run_wire(case: &WireCase) -> TestResult<Wire> {
        let endpoint = if case.has_endpoint {
            Some(Endpoint::new(case.responses.clone())?)
        } else {
            None
        };
        let refused_at = refused_addr()?;
        let endpoint_at = endpoint.as_ref().map_or_else(String::new, Endpoint::addr);
        let raw = case
            .url
            .replace("{refused}", &refused_at)
            .replace("{addr}", &endpoint_at);
        let normalize = |s: &str| {
            let s = s.replace(&refused_at, "{refused}");
            if endpoint_at.is_empty() {
                s
            } else {
                s.replace(&endpoint_at, "{addr}")
            }
        };
        let (error, version) = match Url::parse(&raw) {
            Err(err) => (Some(format!("parse: {err}")), String::new()),
            Ok(url) => {
                let client = Client::new(&url, &case.username, &case.password, &case.db)?;
                let rt = runtime()?;
                if case.op == "write" {
                    let points = case.points.iter().map(TestPoint::build).collect();
                    let result = rt.block_on(client.write(points));
                    (result.err().map(|e| e.to_string()), String::new())
                } else {
                    match rt.block_on(client.ping()) {
                        Ok(version) => (None, version),
                        Err(err) => (Some(err.to_string()), String::new()),
                    }
                }
            }
        };
        let requests = endpoint.as_ref().map_or_else(Vec::new, Endpoint::requests);
        Ok(Wire {
            requests: requests.iter().map(|r| normalize(r)).collect(),
            error: error.as_deref().map(normalize),
            version,
        })
    }

    /// Which client sent a request.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Side {
        Go,
        Rust,
    }

    /// A request as the endpoint read it.
    struct Request {
        method: String,
        path: String,
        query: Vec<(String, String)>,
        version: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl Request {
        /// The values of the header `name`, in any case.
        fn header(&self, name: &str) -> Vec<&str> {
            self.headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
                .collect()
        }

        /// The header names, in order.
        fn names(&self) -> Vec<&str> {
            self.headers.iter().map(|(k, _)| k.as_str()).collect()
        }
    }

    /// The pairs of a query as written, in order.
    fn query_pairs(query: &str) -> Vec<(String, String)> {
        query
            .split('&')
            .filter(|_| !query.is_empty())
            .map(|pair| {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                (k.to_owned(), v.to_owned())
            })
            .collect()
    }

    /// The pairs `(k, v)`, owned.
    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|&(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    }

    /// One request of the endpoint's.
    fn parse_request(raw: &str) -> TestResult<Request> {
        let (head, body) = raw.split_once("\r\n\r\n").ok_or("no blank line")?;
        let mut lines = head.split("\r\n");
        let line = lines.next().ok_or("no request line")?;
        let parts: Vec<&str> = line.split(' ').collect();
        let [method, target, version] = parts.as_slice() else {
            return Err(format!("request line {line:?}").into());
        };
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let headers = lines
            .map(|l| {
                l.split_once(':')
                    .map(|(k, v)| (k.to_owned(), v.trim().to_owned()))
                    .ok_or_else(|| format!("header line {l:?}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Request {
            method: (*method).to_owned(),
            path: path.to_owned(),
            query: query_pairs(query),
            version: (*version).to_owned(),
            headers,
            body: body.to_owned(),
        })
    }

    /// The only request of a list.
    fn only(requests: &[String]) -> TestResult<Request> {
        match requests {
            [request] => parse_request(request),
            _ => Err(format!("{} requests, not one", requests.len()).into()),
        }
    }

    /// Whether a line of a write's body is a point Go refuses: its `# ERROR:` comment line, or
    /// the crate's line with a value that is not finite (`influx.write-request`).
    fn refused_line(line: &str, side: Side) -> bool {
        match side {
            Side::Go => line.starts_with("# ERROR: "),
            Side::Rust => line.rsplit_once(' ').is_some_and(|(head, _)| {
                ["=NaN", "=inf", "=-inf"]
                    .iter()
                    .any(|value| head.ends_with(value))
            }),
        }
    }

    /// The query of a write down to what both clients send: `db`. Go adds `consistency`,
    /// `precision` and `rp`, empty, and keeps the host URL's pairs; the crate adds `precision`.
    fn write_query(query: &[(String, String)]) -> Vec<(String, String)> {
        query.iter().filter(|(k, _)| k == "db").cloned().collect()
    }

    /// A URL with a write's query taken down to `db` (the `Referer` of a redirect).
    fn canonical_url(url: &str) -> String {
        match url.split_once('?') {
            Some((path, query)) if path.ends_with("/write") => {
                format!("{path}?{}", join_query(&write_query(&query_pairs(query))))
            }
            _ => url.to_owned(),
        }
    }

    /// The pairs written back as a query.
    fn join_query(query: &[(String, String)]) -> String {
        query
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// The parts of a request both clients send alike, once the ledgered differences are taken
    /// out: the header names in lower case (D-HDRCASE) and sorted, without Go's empty
    /// `Content-Type`, reqwest's `Accept: */*` and the lengths, which the bodies decide; a
    /// write's query as `db` alone (`influx.write-request`, `influx.host-url`); an encoded slash
    /// of the host URL's path decoded, as Go's `path.Join` of its decoded path writes it
    /// (`influx.host-url`); and the body's lines without Go's final newline, a point Go refuses
    /// standing for the crate's line of it.
    fn canonical(request: &Request, side: Side) -> String {
        let path = match side {
            Side::Go => request.path.clone(),
            Side::Rust => request.path.replace("%2F", "/"),
        };
        let query = if request.path.ends_with("/write") {
            write_query(&request.query)
        } else {
            request.query.clone()
        };
        let mut headers: Vec<String> = request
            .headers
            .iter()
            .filter_map(|(name, value)| {
                let name = name.to_ascii_lowercase();
                let dropped = matches!(
                    (side, name.as_str(), value.as_str()),
                    (Side::Go, "content-type", "")
                        | (Side::Rust, "accept", "*/*")
                        | (_, "content-length", _)
                );
                let value = if name == "referer" {
                    canonical_url(value)
                } else {
                    value.clone()
                };
                (!dropped).then(|| format!("{name}: {value}"))
            })
            .collect();
        headers.sort();
        let body = match side {
            Side::Go => request.body.strip_suffix('\n').unwrap_or(&request.body),
            Side::Rust => request.body.as_str(),
        };
        let lines: Vec<&str> = body
            .split('\n')
            .map(|line| {
                if refused_line(line, side) {
                    "<refused>"
                } else {
                    line
                }
            })
            .collect();
        format!(
            "{} {path}?{} {}\n{}\n\n{}",
            request.method,
            join_query(&query),
            request.version,
            headers.join("\n"),
            lines.join("\n")
        )
    }

    /// The canonical forms of a list of requests.
    fn canonical_all(requests: &[String], side: Side) -> TestResult<Vec<String>> {
        requests
            .iter()
            .map(|r| Ok(canonical(&parse_request(r)?, side)))
            .collect()
    }

    /// The final status of a response, after any 1xx ones.
    fn final_status(response: &[u8]) -> Option<u16> {
        let text = String::from_utf8_lossy(response);
        let mut rest = text.strip_prefix('\u{4}').unwrap_or(&text);
        loop {
            let status: u16 = rest.strip_prefix("HTTP/1.")?.get(2..5)?.parse().ok()?;
            if !(100..200).contains(&status) {
                return Some(status);
            }
            rest = rest.split_once("\r\n\r\n")?.1;
        }
    }

    /// Whether Go fails the write and the crate does not: a 2xx answer other than 200 and 204
    /// (`influx.write-status`).
    fn written_by_crate_only(case: &WireCase) -> bool {
        case.op == "write"
            && case
                .responses
                .first()
                .and_then(|response| final_status(response))
                .is_some_and(|status| {
                    (200..300).contains(&status) && status != 200 && status != 204
                })
    }

    /// Every wire case sends Go's requests, the ledgered differences aside, fails when Go's
    /// does but on the statuses `influx.write-status` names, and reads Go's version. The
    /// `User-Agent` and `Authorization` lines are compared as written.
    #[test]
    fn wire_matches_go() -> TestResult {
        // Go: testdata/go/influx.go:293 (writeWire), over influxdb1-client influxdb.go:294
        // (Write) and :417 (Ping)
        let cases = wire_cases()?;
        check_eq(&cases.len(), &45, "the wire cases")?;
        for case in &cases {
            let rust = run_wire(case)?;
            check_eq(
                &canonical_all(&rust.requests, Side::Rust)?,
                &canonical_all(&case.requests, Side::Go)?,
                &format!("{}: the requests", case.name),
            )?;
            let go_failed = case.error.is_some();
            if written_by_crate_only(case) {
                check(go_failed && rust.error.is_none(), &case.name)?;
            } else {
                check_eq(
                    &rust.error.is_some(),
                    &go_failed,
                    &format!(
                        "{}: failed (Go: {:?}, Rust: {:?})",
                        case.name, case.error, rust.error
                    ),
                )?;
            }
            check_eq(
                &rust.version,
                &case.version,
                &format!("{}: version", case.name),
            )?;
        }
        Ok(())
    }

    /// `influx.user-agent`: reqwest sends no `User-Agent` of its own, influxdb1-client sends
    /// `InfluxDBClient`, and so does the reporter's client.
    #[test]
    fn user_agent() -> TestResult {
        // Go: influxdb1-client influxdb.go:187-189 (NewClient's default userAgent)
        let cases = wire_cases()?;
        let go = only(&wire_case(&cases, "ping")?.requests)?;
        check_eq(&go.header("User-Agent"), &vec!["InfluxDBClient"], "Go's")?;

        let endpoint = Endpoint::new(vec![NO_CONTENT.to_vec()])?;
        let url = Url::parse(&format!("http://{}", endpoint.addr()))?;
        let rt = runtime()?;
        let plain = reqwest::Client::builder().no_proxy().build()?;
        drop(rt.block_on(plain.get(url.clone()).send())?);
        let client = Client::new(&url, "", "", "")?;
        drop(rt.block_on(client.ping())?);
        let requests = endpoint.requests();
        let [plain_request, ours] = requests.as_slice() else {
            return Err(format!("{} requests", requests.len()).into());
        };
        check_eq(
            &parse_request(plain_request)?.header("User-Agent"),
            &Vec::<&str>::new(),
            "reqwest's",
        )?;
        check_eq(
            &parse_request(ours)?.header("User-Agent"),
            &vec![USER_AGENT],
            "the reporter's",
        )
    }

    /// The one request of the wire case `name`, Go's and the reporter client's.
    fn go_and_rust(cases: &[WireCase], name: &str) -> TestResult<(Request, Request)> {
        let case = wire_case(cases, name)?;
        Ok((only(&case.requests)?, only(&run_wire(case)?.requests)?))
    }

    /// `influx.write-request`: Go's write query, headers and lines, and the crate's.
    #[test]
    fn write_request() -> TestResult {
        // Go: influxdb1-client influxdb.go:294-354 (Write)
        let cases = wire_cases()?;
        let (go, rust) = go_and_rust(&cases, "write")?;
        check_eq(
            &go.query,
            &pairs(&[
                ("consistency", ""),
                ("db", "pbs"),
                ("precision", ""),
                ("rp", ""),
            ]),
            "Go's query",
        )?;
        let crate_query = [
            pairs(&[("db", "pbs"), ("precision", "ns")]),
            pairs(&[("precision", "ns"), ("db", "pbs")]),
        ];
        check(
            crate_query.contains(&rust.query),
            &format!("the crate's query, in its map's order: {:?}", rust.query),
        )?;
        check_eq(
            &go.names(),
            &vec![
                "Host",
                "User-Agent",
                "Content-Length",
                "Content-Type",
                "Accept-Encoding",
            ],
            "Go's headers",
        )?;
        check_eq(
            &rust.names(),
            &vec![
                "accept",
                "user-agent",
                "accept-encoding",
                "host",
                "content-length",
            ],
            "reqwest's headers",
        )?;
        check_eq(
            &go.header("Content-Type"),
            &vec![""],
            "Go's empty Content-Type",
        )?;
        check_eq(&rust.header("Accept"), &vec!["*/*"], "reqwest's Accept")?;
        check_eq(
            &go.body.strip_suffix('\n'),
            &Some(rust.body.as_str()),
            "the lines, Go's with a final newline",
        )?;

        // An empty batch: Go's length 0, the crate's millisecond precision and no length.
        let (go_empty, rust_empty) = go_and_rust(&cases, "write_empty_batch")?;
        check_eq(
            &go_empty.header("Content-Length"),
            &vec!["0"],
            "Go's length",
        )?;
        check_eq(
            &rust_empty.header("Content-Length"),
            &Vec::<&str>::new(),
            "hyper's length",
        )?;
        check(
            rust_empty
                .query
                .contains(&("precision".to_owned(), "ms".to_owned())),
            "the crate's precision of no points",
        )?;

        // A point with a NaN field: Go's comment line, the crate's line.
        let (go_nan, rust_nan) = go_and_rust(&cases, "write_point_error")?;
        check_eq(
            &go_nan.body.lines().next(),
            &Some("# ERROR: NaN is an unsupported value for field f,\"=  a b,c"),
            "Go's",
        )?;
        check_eq(
            &rust_nan.body.lines().next(),
            &Some("a\\ b\\,c,k\\==v\\ \\, f\\,\"\\=\\ =NaN 0"),
            "the crate's",
        )
    }

    /// `influx.write-status`: Go's write fails on a 2xx status other than 200 and 204 and
    /// succeeds on those whatever the body; the crate's succeeds on any 2xx unless the body
    /// names an error.
    #[test]
    fn write_status() -> TestResult {
        // Go: influxdb1-client influxdb.go:360-364
        let cases = wire_cases()?;
        let created = wire_case(&cases, "write_201_format")?;
        check(created.error.is_some(), "Go fails a 201")?;
        check(run_wire(created)?.error.is_none(), "the crate takes a 201")?;
        let ok = wire_case(&cases, "write_200")?;
        check(ok.error.is_none(), "Go takes a 200 with a body")?;
        check(run_wire(ok)?.error.is_none(), "the crate takes it")?;

        let endpoint = Endpoint::new(vec![answer("200 OK", "{\"error\":\"partial write\"}")])?;
        let url = Url::parse(&format!("http://{}", endpoint.addr()))?;
        let client = Client::new(&url, "", "", "pbs")?;
        let points = vec![point("pbs", &[], "f", Type::SignedInteger(1), 0)];
        let result = runtime()?.block_on(client.write(points));
        check_eq(
            &result.err().map(|e| e.to_string()),
            &Some(
                "InfluxDB encountered the following error: influxdb error: \
                 \"{\\\"error\\\":\\\"partial write\\\"}\""
                    .to_owned(),
            ),
            "the crate fails a 200 that names an error",
        )
    }

    /// Go: `replaceAddrs`: the endpoints' addresses written `{addr}` and `{addr2}`, the longer
    /// one first.
    fn replace_addrs(s: &str, first: &str, next: &str) -> String {
        let mut pairs = [(first, "{addr}"), (next, "{addr2}")];
        if next.len() > first.len() {
            pairs.swap(0, 1);
        }
        pairs
            .iter()
            .fold(s.to_owned(), |s, (addr, name)| s.replace(addr, name))
    }

    /// The redirect case through [`Client`], against two endpoints, the first answering with the
    /// case's redirect to the second or to itself: what the client did, with the first
    /// endpoint's requests, and the second endpoint's requests.
    fn run_redirect(redirect: &RedirectCase) -> TestResult<(Wire, Vec<String>)> {
        let case = &redirect.case;
        let next = Endpoint::new(redirect.next_responses.clone())?;
        let next_at = next.addr();
        let answers = case
            .responses
            .iter()
            .map(|r| text(r))
            .collect::<TestResult<Vec<_>>>()?;
        let first = Endpoint::for_addr(|first_at| {
            answers
                .iter()
                .map(|r| {
                    r.replace("{addr2}", &next_at)
                        .replace("{addr}", first_at)
                        .into_bytes()
                })
                .collect()
        })?;
        let first_at = first.addr();
        let url = Url::parse(&case.url.replace("{addr}", &first_at))?;
        let client = Client::new(&url, &case.username, &case.password, &case.db)?;
        let rt = runtime()?;
        let (error, version) = if case.op == "write" {
            let points = case.points.iter().map(TestPoint::build).collect();
            let result = rt.block_on(client.write(points));
            (result.err().map(|e| e.to_string()), String::new())
        } else {
            match rt.block_on(client.ping()) {
                Ok(version) => (None, version),
                Err(err) => (Some(err.to_string()), String::new()),
            }
        };
        let normalize = |s: &str| replace_addrs(s, &first_at, &next_at);
        let wire = Wire {
            requests: first.requests().iter().map(|r| normalize(r)).collect(),
            error: error.as_deref().map(normalize),
            version,
        };
        let next_requests = next.requests().iter().map(|r| normalize(r)).collect();
        Ok((wire, next_requests))
    }

    /// The request without its `Authorization` lines.
    fn without_authorization(mut request: Request) -> Request {
        request
            .headers
            .retain(|(name, _)| !name.eq_ignore_ascii_case("Authorization"));
        request
    }

    /// The `Authorization` values of a request.
    fn authorization(request: &Request) -> Vec<String> {
        request
            .header("Authorization")
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    /// `influx.redirect-auth`: which requests of a write or ping that the server redirects carry
    /// the credentials. In Go, the basic auth of `metrics.influxdb.username` follows a redirect
    /// to another port of the host, net/http copying the header to a redirect on the same host
    /// name, and the host URL's user info reaches no redirect to an absolute URL, not even one on
    /// its origin, net/http adding it to each request whose own URL has it. The reporter's client
    /// sends both as one header, which reqwest keeps while the redirects stay on the first
    /// request's origin and drops on any change of host, port or scheme. The requests are Go's
    /// otherwise, the ledgered differences aside.
    #[test]
    fn redirect_auth() -> TestResult {
        // Go: testdata/go/influx.go (writeRedirects), over net/http client.go
        // (shouldCopyHeaderOnRedirect, and send for the user info) and influxdb1-client
        // influxdb.go:332-334 and :428-430 (SetBasicAuth)
        let cases = redirect_cases()?;
        check_eq(&cases.len(), &4, "the redirect cases")?;
        for redirect in &cases {
            let case = &redirect.case;
            let name = &case.name;
            let (rust, rust_next) = run_redirect(redirect)?;
            check_eq(
                &rust.error.is_some(),
                &case.error.is_some(),
                &format!("{name}: an error on both sides or on neither"),
            )?;
            check_eq(&rust.version, &case.version, &format!("{name}: version"))?;
            check_eq(
                &rust.requests.len(),
                &case.requests.len(),
                &format!("{name}: the first endpoint's requests"),
            )?;
            // Every request, the first endpoint's and then the second's, as they were sent.
            let parse_all = |requests: &[String], more: &[String]| {
                requests
                    .iter()
                    .chain(more)
                    .map(|r| parse_request(r))
                    .collect::<TestResult<Vec<_>>>()
            };
            let go = parse_all(&case.requests, &redirect.next_requests)?;
            let reporter = parse_all(&rust.requests, &rust_next)?;
            check_eq(&reporter.len(), &go.len(), &format!("{name}: the requests"))?;
            check(go.len() > 1, &format!("{name}: the request is redirected"))?;
            let credentials = go.first().map_or_default(authorization);
            check(
                !credentials.is_empty(),
                &format!("{name}: the first request carries credentials"),
            )?;
            let on_origin = case.requests.len();
            for (i, (go_request, rust_request)) in go.into_iter().zip(reporter).enumerate() {
                // Go: SetBasicAuth's header on every request, the host name staying the first's;
                // the user info's on the first alone, every redirect going to an absolute URL.
                let go_carries = !case.username.is_empty() || i == 0;
                // reqwest: the header while the requests stay on the first request's origin.
                let rust_carries = i < on_origin;
                let carried = |carries: bool| {
                    if carries {
                        credentials.clone()
                    } else {
                        Vec::new()
                    }
                };
                check_eq(
                    &authorization(&go_request),
                    &carried(go_carries),
                    &format!("{name}: the credentials of Go's request {i}"),
                )?;
                check_eq(
                    &authorization(&rust_request),
                    &carried(rust_carries),
                    &format!("{name}: the credentials of the reporter's request {i}"),
                )?;
                check_eq(
                    &canonical(&without_authorization(rust_request), Side::Rust),
                    &canonical(&without_authorization(go_request), Side::Go),
                    &format!("{name}: request {i}, its credentials aside"),
                )?;
            }
        }
        Ok(())
    }

    /// The forms of the crate's and reqwest's error texts.
    fn crate_text(text: &str, op: &str) -> bool {
        let forms: &[&str] = if op == "write" {
            &[
                "API error with a status code: ",
                "connection error: ",
                "http protocol error: ",
            ]
        } else {
            &["error sending request for url (", "builder error for url ("]
        };
        text.starts_with("parse: ") || forms.iter().any(|form| text.starts_with(form))
    }

    /// `influx.error-texts`: every failure's text is the crate's, reqwest's or the url crate's,
    /// never Go's; and a 500 is the status whatever its body, which Go's text is.
    #[test]
    fn error_texts() -> TestResult {
        // Go: influxdb1-client influxdb.go:348-364, and net/http's *url.Error texts
        for case in &wire_cases()? {
            let (Some(go), Some(rust)) = (&case.error, run_wire(case)?.error) else {
                continue;
            };
            check(go != &rust, &format!("{}: {rust:?}", case.name))?;
            check(
                crate_text(&rust, &case.op),
                &format!("{}: {rust:?}", case.name),
            )?;
        }
        let rows = quoted_rows::<2>("errorf.txt")?;
        check_eq(&rows.len(), &1567, "the errorf bodies")?;
        let responses = rows
            .iter()
            .map(|[body, _]| answer("500 Internal Server Error", body));
        let endpoint = Endpoint::new(responses.collect())?;
        let url = Url::parse(&format!("http://{}", endpoint.addr()))?;
        let client = Client::new(&url, "", "", "pbs")?;
        let rt = runtime()?;
        for [body, go] in &rows {
            let points = vec![point("m", &[], "f", Type::SignedInteger(1), 0)];
            let rust = rt
                .block_on(client.write(points))
                .err()
                .map(|e| e.to_string());
            check_eq(
                &rust.as_deref(),
                &Some("API error with a status code: 500"),
                &format!("the body {body:?}, which Go's text {go:?} is made of"),
            )?;
        }
        Ok(())
    }

    /// `influx.host-url`: the hosts Go's `url.Parse` and the url crate read differently, the
    /// paths `path.Join` cleans differently, and the host URL's query and encoded slashes on
    /// the wire.
    #[test]
    fn host_url() -> TestResult {
        // Go: go-metrics-influxdb influxdb.go:35 (url.Parse), influxdb1-client influxdb.go:296
        // and :341-346 (path.Join and the host URL's query)
        let mut go_only = Vec::new();
        let mut rust_only = Vec::new();
        let rows = quoted_rows::<3>("urls.txt")?;
        check_eq(&rows.len(), &37, "the host URLs")?;
        for [raw, result, text] in &rows {
            match (result.as_str(), Url::parse(raw)) {
                ("ok", Err(_)) => go_only.push(raw.as_str()),
                ("error", Ok(_)) => rust_only.push(raw.as_str()),
                ("ok" | "error", _) => {}
                _ => return Err(format!("{raw:?}: {result:?} {text:?}").into()),
            }
        }
        check_eq(
            &go_only,
            &vec![
                "localhost",
                "//influx/p",
                "http://[fe80::1%25en0]:8086/",
                "http://h:99999",
                "http://h:8086:8086",
                "http://user@:80",
            ],
            "the hosts only Go reads",
        )?;
        check_eq(
            &rust_only,
            &vec![
                "http://ex%61mple.com/",
                "http://h#%zz",
                "http://h/%zz",
                "http://%41:8080/",
                "http://h\n",
                " http://h",
                "http://h/\0",
            ],
            "the hosts only the url crate reads",
        )?;
        check_eq(
            &Url::parse("http:///p")?.as_str(),
            &"http://p/",
            "an empty host",
        )?;
        check_eq(
            &Url::parse("HTTP://H/P")?.as_str(),
            &"http://h/P",
            "an upper-case host",
        )?;
        host_paths()?;
        host_queries()
    }

    /// Go's `path.Join(p, "write")` against the reporter's path of `p`: alike for every path of a
    /// URL with a host without repeated slashes, which Go collapses and the url crate keeps.
    fn host_paths() -> TestResult {
        let mut alike = 0;
        let mut differ = 0;
        for row in golden("gometrics/paths.txt")?.lines() {
            let parts: Vec<&str> = row.split('\t').collect();
            let [p, joined, _] = parts.as_slice() else {
                return Err(format!("paths.txt: {row:?}").into());
            };
            let p = if *p == "-" { "" } else { p };
            if !(p.is_empty() || p.starts_with('/')) {
                // Not the path of a URL with a host.
                continue;
            }
            // Go: URL.String writes a slash before a path without one.
            let go = format!("/{}", joined.trim_start_matches('/'));
            let mut url = Url::parse("http://h")?;
            url.set_path(p);
            let rust = format!("{}/write", url.path().trim_end_matches('/'));
            if p.contains("//") {
                differ += usize::from(rust != go);
            } else {
                check_eq(&rust, &go, &format!("the path of {p:?}"))?;
                alike += 1;
            }
        }
        check_eq(
            &(alike, differ),
            &(190, 109),
            "the paths alike, and those with // that differ",
        )?;
        let mut url = Url::parse("http://h")?;
        url.set_path("//a/../b");
        check_eq(&url.path(), &"//b", "a dot segment after repeated slashes")
    }

    /// The host URL's query and encoded slashes on the wire: Go keeps the pairs `url.ParseQuery`
    /// reads beside its own and decodes `%2F`; the crate drops the query and keeps `%2F`.
    fn host_queries() -> TestResult {
        let cases = wire_cases()?;
        for (name, go_query, rust_db) in [
            (
                "write_query",
                vec![
                    ("a", "1"),
                    ("consistency", ""),
                    ("db", "pbs+db%26x"),
                    ("precision", ""),
                    ("rp", ""),
                    ("z", "+"),
                ],
                "pbs+db%26x",
            ),
            (
                "write_query_odd",
                vec![
                    ("", "e"),
                    ("consistency", ""),
                    ("db", "pbs"),
                    ("f", ""),
                    ("g", "1"),
                    ("g", "2"),
                    ("precision", ""),
                    ("rp", ""),
                ],
                "pbs",
            ),
        ] {
            let case = wire_case(&cases, name)?;
            check_eq(&only(&case.requests)?.query, &pairs(&go_query), name)?;
            let mut rust = only(&run_wire(case)?.requests)?.query;
            rust.sort();
            check_eq(&rust, &pairs(&[("db", rust_db), ("precision", "ns")]), name)?;
        }
        let case = wire_case(&cases, "write_raw_path")?;
        check_eq(
            &only(&case.requests)?.path,
            &"/a/b%20c/write".to_owned(),
            "Go's path",
        )?;
        check_eq(
            &only(&run_wire(case)?.requests)?.path,
            &"/a%2Fb%20c/write".to_owned(),
            "the url crate's",
        )
    }

    /// A points.txt string: `rep:<n>:<unit>` is `<unit>` `n` times.
    fn decode_rep(s: &str) -> TestResult<String> {
        match s.strip_prefix("rep:") {
            Some(rest) => {
                let (n, unit) = rest.split_once(':').ok_or("rep: without a unit")?;
                Ok(unit.repeat(n.parse()?))
            }
            None => Ok(s.to_owned()),
        }
    }

    /// A line as points.txt records it: as it is up to 300 bytes, else its FNV-1a and length.
    fn recorded_line(line: &str) -> String {
        if line.len() > 300 {
            format!("fnv:{:016x}:{}", fnv1a64(line.as_bytes()), line.len())
        } else {
            line.to_owned()
        }
    }

    /// A points.txt time, `<unix seconds>.<nanoseconds>`, in Unix nanoseconds; `None` for Go's
    /// zero time.
    fn point_time(field: &str) -> TestResult<Option<i128>> {
        if field == "zero" {
            return Ok(None);
        }
        let (sec, nsec) = field.split_once('.').ok_or("a time without nanoseconds")?;
        Ok(Some(
            i128::from(sec.parse::<i64>()?) * NANOS + i128::from(nsec.parse::<i64>()?),
        ))
    }

    /// Nanoseconds per second.
    const NANOS: i128 = 1_000_000_000;

    /// The value and the timestamp of a line.
    fn value_and_time(line: &str) -> TestResult<(&str, &str)> {
        let (head, time) = line.rsplit_once(' ').ok_or("no timestamp")?;
        let (_, value) = head.rsplit_once('=').ok_or("no field")?;
        Ok((value, time))
    }

    /// Go's line for a point of a measurement without escapes or tags whose key is too long.
    fn key_length_refusal(p: &TestPoint) -> String {
        let size = p.measurement.len() + 4 + p.key.len();
        format!(
            "# ERROR: max key length exceeded: {size} > 65535 {}",
            p.measurement
        )
    }

    /// What the crate writes for a point Go refuses with a `# ERROR:` line, by Go's reason.
    fn refused_point(go: &str, rust: &str, p: &TestPoint, zero: bool) -> TestResult<&'static str> {
        let (value, time) = value_and_time(rust)?;
        let reason = go.strip_prefix("# ERROR: ").ok_or("not refused")?;
        let class = if reason.contains("is an unsupported value for field") {
            check(["NaN", "inf", "-inf"].contains(&value), rust)?;
            "non-finite"
        } else if reason.starts_with("time outside range") {
            check(!zero, "Go writes a zero time")?;
            check_eq(&time, &p.unix_nanos.max(0).to_string().as_str(), rust)?;
            "time"
        } else if reason.starts_with("max key length exceeded") {
            "key length"
        } else if reason.starts_with("all fields must have non-empty names") {
            check(p.key.is_empty() && rust.contains(" ="), rust)?;
            "field name"
        } else {
            return Err(format!("Go's reason {reason:?}").into());
        };
        Ok(class)
    }

    /// Every point of points.txt Go writes is the crate's line, but the strings with a backslash
    /// or a double quote (`influx.key-escapes`), a zero time, which Go leaves out, and a time
    /// before the epoch, which the reporter writes as 0; and the crate writes a line for every
    /// point Go refuses with a `# ERROR:` comment line (`influx.write-request`).
    #[test]
    fn points_match_go() -> TestResult {
        // Go: testdata/go/gometrics.go:471 (writePoints), over influxdb1-client
        // influxdb.go:642-646 (MarshalString)
        let rows = quoted_rows::<7>("points.txt")?;
        check_eq(&rows.len(), &243, "the points")?;
        let mut alike = 0;
        let mut with_escapes = 0;
        let mut refused: BTreeMap<&str, usize> = BTreeMap::new();
        for [measurement, tags, key, kind, value, time, go] in &rows {
            let parts: Vec<String> = tags
                .split('\u{1f}')
                .filter(|_| !tags.is_empty())
                .map(decode_rep)
                .collect::<TestResult<_>>()?;
            let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
            let unix_nanos = point_time(time)?;
            let p = TestPoint {
                measurement: decode_rep(measurement)?,
                tags: tag_map(&parts)?,
                key: decode_rep(key)?,
                value: field_value(kind, value)?,
                unix_nanos: unix_nanos.unwrap_or(0),
            };
            let rust = p.line()?;
            // A refusal of a long key is recorded by its digest.
            let long_key = key_length_refusal(&p);
            let refusal = if go.starts_with("# ERROR: ") {
                Some(go.as_str())
            } else {
                (recorded_line(&long_key) == *go).then_some(long_key.as_str())
            };
            if let Some(refusal) = refusal {
                let class = refused_point(refusal, &rust, &p, unix_nanos.is_none())?;
                *refused.entry(class).or_default() += 1;
                continue;
            }
            let has_escape = [&p.measurement, &p.key]
                .into_iter()
                .chain(p.tags.keys())
                .chain(p.tags.values())
                .any(|s| s.contains(['\\', '"']));
            if has_escape {
                with_escapes += 1;
                continue;
            }
            let (head, rust_time) = rust.rsplit_once(' ').ok_or("no timestamp")?;
            let rust = match unix_nanos {
                None => head.to_owned(),
                Some(t) if t < 0 => {
                    check_eq(&rust_time, &"0", "a time before the epoch")?;
                    format!("{head} {t}")
                }
                Some(_) => rust.clone(),
            };
            check_eq(
                &recorded_line(&rust),
                go,
                &format!("the point of {measurement:?} {key:?}"),
            )?;
            alike += 1;
        }
        check_eq(
            &(alike, with_escapes),
            &(84, 118),
            "the points alike, and those with escapes",
        )?;
        check_eq(
            &refused.into_iter().collect::<Vec<_>>(),
            &vec![
                ("field name", 31),
                ("key length", 2),
                ("non-finite", 3),
                ("time", 5),
            ],
            "the points Go refuses",
        )
    }

    /// `s` with a backslash before each of `chars`.
    fn escape(s: &str, chars: &[char]) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            if chars.contains(&c) {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }

    /// Go's line of keys.txt's point of `s` (influxdb1-client `models`): the measurement with
    /// `\,` and `\ ` unescaped (`unescapeMeasurement`), then `,` and ` ` escaped; the tag key and
    /// value with `,`, ` ` and `=` escaped (`escapeTag`); and the field key with `,`, `"`, ` `
    /// and `=` escaped (`escape.String`).
    fn go_spelling(s: &str) -> String {
        let measurement = s.replace("\\,", ",").replace("\\ ", " ");
        format!(
            "{},{}={} {}=1i",
            escape(&measurement, &[',', ' ']),
            escape(s, &[',', ' ', '=']),
            escape(&format!("{s}x"), &[',', ' ', '=']),
            escape(&format!("{s}f"), &[',', '"', ' ', '=']),
        )
    }

    /// The crate's line of it (`LineProtoTerm::escape`): the measurement with `,` and ` `
    /// escaped, the tag and field keys with `,`, ` ` and `=`, and the tag value with `\`, `,`,
    /// ` `, `=` and `"`.
    fn crate_spelling(s: &str) -> String {
        format!(
            "{},{}={} {}=1i",
            escape(s, &[',', ' ']),
            escape(s, &[',', ' ', '=']),
            escape(&format!("{s}x"), &['\\', ',', ' ', '=', '"']),
            escape(&format!("{s}f"), &[',', ' ', '=']),
        )
    }

    /// `influx.key-escapes`: for every string of up to four bytes over `a`, comma, space, equals,
    /// backslash and double quote, Go spells the point of keys.txt as [`go_spelling`] and the
    /// crate as [`crate_spelling`]; the two are alike exactly for the strings without a
    /// backslash or a double quote.
    #[test]
    fn key_escapes() -> TestResult {
        // Go: testdata/go/influxedge.go:52 (writeKeys), over influxdb1-client models/points.go
        // (EscapeMeasurement, unescapeMeasurement, escapeTag) and pkg/escape/strings.go
        let rows = quoted_rows::<2>("keys.txt")?;
        check_eq(&rows.len(), &1555, "the keys")?;
        let mut differ = 0;
        for [s, go] in &rows {
            let value = format!("{s}x");
            let tags = [(s.as_str(), value.as_str())];
            let line = point(s, &tags, &format!("{s}f"), Type::SignedInteger(1), 0)
                .build()?
                .get();
            let rust = line.strip_suffix(" 0").ok_or("no time")?;
            check_eq(go, &go_spelling(s), &format!("Go's spelling of {s:?}"))?;
            check_eq(
                &rust,
                &crate_spelling(s).as_str(),
                &format!("the crate's of {s:?}"),
            )?;
            let plain = !s.contains(['\\', '"']);
            check_eq(&(go == rust), &plain, &format!("{s:?} alike"))?;
            differ += usize::from(!plain);
        }
        check_eq(&differ, &1214, "the keys spelled otherwise")
    }

    /// The reporter's `Time.Truncate` of every time and interval of truncate.txt is Go's.
    #[test]
    fn truncate_matches_go() -> TestResult {
        // Go: testdata/go/gometrics.go:581 (writeTruncate), over time/time.go Truncate and div
        let mut rows = 0;
        for row in golden("gometrics/truncate.txt")?.lines() {
            let fields = row
                .split('\t')
                .map(str::parse)
                .collect::<Result<Vec<i64>, _>>()?;
            let [sec, nsec, d, rounded_sec, rounded_ns] = fields.as_slice() else {
                return Err(format!("truncate.txt: {row:?}").into());
            };
            let t = i128::from(*sec) * NANOS + i128::from(*nsec);
            let want = i128::from(*rounded_sec) * NANOS + i128::from(*rounded_ns);
            check_eq(&truncate(t, GoDuration::from_nanos(*d)), &want, row)?;
            rows += 1;
        }
        check_eq(&rows, &800, "the truncations")
    }

    /// Go: `buildReporterRegistry`: reporter.txt's `input` rows in a registry with the prefix
    /// `prebidserver.`.
    fn reporter_registry(inputs: &[String]) -> TestResult<Registry> {
        let registry = Registry::new_prefixed("prebidserver.");
        for input in inputs {
            let parts: Vec<&str> = input.split(' ').collect();
            let [kind, name, rest @ ..] = parts.as_slice() else {
                return Err(format!("input {input:?}").into());
            };
            let name = path_unescape(name)?;
            let values = match rest {
                [] => Vec::new(),
                [values] => values
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<Vec<i64>, _>>()?,
                _ => return Err(format!("input {input:?}").into()),
            };
            match *kind {
                "counter" => {
                    let c = registry.get_or_register_counter(&name);
                    for v in values {
                        c.inc(v);
                    }
                }
                "meter" => {
                    let m = registry.get_or_register_meter(&name);
                    for v in values {
                        m.mark(v);
                    }
                }
                "timer" => {
                    let t = registry.get_or_register_timer(&name);
                    for v in values {
                        t.update(GoDuration::from_nanos(v));
                    }
                }
                "expdecay" | "uniform" => {
                    let sample = if *kind == "uniform" {
                        new_uniform_sample(1024)
                    } else {
                        new_exp_decay_sample(1028, 0.015)
                    };
                    let h = registry.get_or_register_histogram(&name, sample);
                    for v in values {
                        h.update(v);
                    }
                }
                _ => return Err(format!("input {input:?}").into()),
            }
        }
        Ok(registry)
    }

    /// Go: `normalizeLine`: the timestamp written `<ts>`, and the value `<rate>` for a rate.
    fn normalize_line(line: &str) -> TestResult<String> {
        let (head, _) = line.rsplit_once(' ').ok_or("no timestamp")?;
        let eq = head.rfind('=').ok_or("no field")?;
        let key = head.find(' ').ok_or("no field")?;
        let series = head.get(..key).unwrap_or_default();
        let field = head.get(key + 1..eq).unwrap_or_default();
        let bucket = series
            .find(",bucket=")
            .and_then(|i| series.get(i + ",bucket=".len()..))
            .unwrap_or_default();
        let rate = ["m1", "m5", "m15", "meanrate"].contains(&bucket)
            || bucket == "mean"
                && field
                    .rsplit_once('.')
                    .is_some_and(|(_, kind)| kind == "meter");
        let head = match head.get(..=eq) {
            Some(series_and_key) if rate => format!("{series_and_key}<rate>"),
            _ => head.to_owned(),
        };
        Ok(format!("{head} <ts>"))
    }

    /// What reporter.txt recorded of Go's reporters.
    #[derive(Default)]
    struct ReporterGolden {
        inputs: Vec<String>,
        send: String,
        lines: Vec<String>,
        aligned: bool,
        logs: Vec<String>,
    }

    /// Go: the rows `writeReporter` writes.
    fn reporter_golden() -> TestResult<ReporterGolden> {
        let mut golden_rows = ReporterGolden::default();
        for row in golden("gometrics/reporter.txt")?.lines() {
            let fields = quoted_fields(row)?
                .into_iter()
                .map(String::from_utf8)
                .collect::<Result<Vec<_>, _>>()?;
            let [kind, value] = fields.as_slice() else {
                return Err(format!("reporter.txt: {row:?}").into());
            };
            match kind.as_str() {
                "input" => golden_rows.inputs.push(value.clone()),
                "send" => golden_rows.send.clone_from(value),
                "line" => golden_rows.lines.push(value.clone()),
                "aligned" => golden_rows.aligned = value == "true",
                "log" => golden_rows.logs.push(value.clone()),
                _ => return Err(format!("reporter.txt: {row:?}").into()),
            }
        }
        Ok(golden_rows)
    }

    /// The endpoints of the reporters that write.
    struct ReporterEndpoints {
        send: Endpoint,
        aligned: Endpoint,
        failing: Endpoint,
    }

    /// Waits until the endpoints read their first writes and the failing send and the refused
    /// ping are logged, as `writeReporter` does.
    async fn reporters_done(endpoints: &ReporterEndpoints, capture: &Capture) -> TestResult {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let lines = capture.lines();
            let logged = |text: &str| lines.iter().any(|l| l.message.contains(text));
            if !endpoints.send.requests().is_empty()
                && !endpoints.aligned.requests().is_empty()
                && logged("unable to send")
                && logged("got error while sending a ping")
            {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(format!("the reporters' logs are incomplete: {lines:?}").into());
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    /// The settings of a reporter of the database `pbsdb` and the measurement `pbs`.
    fn reporter_settings(
        interval: GoDuration,
        url: String,
        credentials: (&str, &str),
        align: bool,
    ) -> Settings {
        Settings {
            interval,
            url,
            database: "pbsdb".to_owned(),
            measurement: "pbs".to_owned(),
            username: credentials.0.to_owned(),
            password: credentials.1.to_owned(),
            align,
        }
    }

    /// The reporters of reporter.txt, run as `writeReporter` runs Go's: the first batch is
    /// Go's, the ledgered differences of the request aside and the double quote of a field key
    /// escaped as Go escapes it (`influx.key-escapes`); the aligned batch's times are one
    /// multiple of the interval; and the lines logged are Go's up to their `err=`
    /// (`influx.error-texts`), at ERROR through pbs-logger (`logging.std-log-lines`).
    #[test]
    fn reporter_matches_go() -> TestResult {
        // Go: testdata/go/influx.go:500 (writeReporter), over go-metrics-influxdb
        // influxdb.go:34-222
        let go = reporter_golden()?;
        let endpoints = ReporterEndpoints {
            send: Endpoint::new(vec![NO_CONTENT.to_vec()])?,
            aligned: Endpoint::new(vec![NO_CONTENT.to_vec()])?,
            failing: Endpoint::new(vec![
                answer("500 Internal Server Error", "%d oops %v."),
                NO_CONTENT.to_vec(),
            ])?,
        };
        let refused = refused_addr()?;
        let registries = [
            reporter_registry(&go.inputs)?,
            reporter_registry(&go.inputs)?,
            reporter_registry(&go.inputs)?,
        ];
        let [r0, r1, r2] = registries.each_ref().map(Registry::downgrade);
        let every = GoDuration::times(150, GoDuration::MILLISECOND);
        let url = |endpoint: &Endpoint| format!("http://{}", endpoint.addr());
        let none = ("", "");
        let reporters = [
            reporter_settings(every, url(&endpoints.send), ("admin", "pw"), false),
            reporter_settings(GoDuration::SECOND, url(&endpoints.aligned), none, true),
            reporter_settings(every, url(&endpoints.failing), none, false),
            reporter_settings(
                GoDuration::from_nanos(0),
                format!("http://{refused}"),
                none,
                false,
            ),
            reporter_settings(GoDuration::SECOND, "http://[::1".to_owned(), none, false),
            reporter_settings(GoDuration::SECOND, "%zz".to_owned(), none, false),
        ];
        let [s0, s1, s2, s3, s4, s5] = &reporters;
        let capture = Capture::new();
        let rt = runtime()?;
        capture.run(|| {
            rt.block_on(async {
                let all = async {
                    tokio::join!(
                        run(&r0, s0),
                        run(&r1, s1),
                        run(&r2, s2),
                        run(&r2, s3),
                        run(&r2, s4),
                        run(&r2, s5)
                    )
                };
                tokio::select! {
                    _ = all => Err("the reporters returned".into()),
                    done = reporters_done(&endpoints, &capture) => done,
                }
            })
        })?;
        check_first_batch(&go, &endpoints.send)?;
        check_aligned(&go, &endpoints.aligned)?;
        check_logs(&go, &capture, &refused, &endpoints.failing)
    }

    /// The first batch of the reporter with credentials against Go's.
    fn check_first_batch(go: &ReporterGolden, send: &Endpoint) -> TestResult {
        let requests = send.requests();
        let first = requests.first().ok_or("no batch")?;
        let mut rust = parse_request(&first.replace(&send.addr(), "{addr}"))?;
        let body = mem::take(&mut rust.body);
        check_eq(
            &canonical(&rust, Side::Rust),
            &canonical(&parse_request(&go.send)?, Side::Go),
            "the request of the first batch",
        )?;
        let mut lines = body
            .split('\n')
            .map(|line| normalize_line(&line.replace('"', "\\\"")))
            .collect::<TestResult<Vec<_>>>()?;
        lines.sort();
        check_eq(&lines, &go.lines, "the lines of the first batch")
    }

    /// The aligned reporter's first batch: one time, a multiple of its second.
    fn check_aligned(go: &ReporterGolden, aligned: &Endpoint) -> TestResult {
        let requests = aligned.requests();
        let request = parse_request(requests.first().ok_or("no aligned batch")?)?;
        let times = request
            .body
            .split('\n')
            .map(|line| -> TestResult<i64> { Ok(value_and_time(line)?.1.parse()?) })
            .collect::<TestResult<Vec<i64>>>()?;
        let one_time = times.windows(2).all(|w| w.first() == w.last());
        let on_the_second = times.iter().all(|t| t % 1_000_000_000 == 0);
        check_eq(&(one_time && on_the_second), &go.aligned, "aligned")
    }

    /// The lines logged against Go's, each up to its `err=`.
    fn check_logs(
        go: &ReporterGolden,
        capture: &Capture,
        refused_at: &str,
        failing: &Endpoint,
    ) -> TestResult {
        let lines = capture.lines();
        check(
            lines.iter().all(|l| matches!(l.severity, Severity::Error)),
            "every line is an ERROR",
        )?;
        let prefix = |line: &str| line.split_once("err=").map_or(line, |(p, _)| p).to_owned();
        let mut rust: Vec<String> = lines
            .iter()
            .map(|l| {
                let message = l.message.replace(refused_at, "{refused}");
                prefix(&message.replace(&failing.addr(), "{addr}"))
            })
            .collect();
        rust.sort();
        rust.dedup();
        let mut go_logs: Vec<String> = go
            .logs
            .iter()
            .map(|l| prefix(l.trim_start_matches("<date> ")))
            .collect();
        go_logs.sort();
        check_eq(&rust, &go_logs, "the lines logged, up to err=")
    }

    /// `influx.cadence`: the sends come one interval apart from one interval after the start, on
    /// the interval's grid, and the first ping five seconds after the start.
    #[test]
    fn cadence() -> TestResult {
        // Go: go-metrics-influxdb influxdb.go:70-72 (run: time.Tick(r.interval) and
        // time.Tick(time.Second * 5))
        let endpoint = Endpoint::new(vec![NO_CONTENT.to_vec()])?;
        let registry = Registry::new_prefixed("prebidserver.");
        registry
            .get_or_register_counter("active_connections")
            .inc(1);
        let interval = Duration::from_millis(250);
        let settings = reporter_settings(
            GoDuration::times(250, GoDuration::MILLISECOND),
            format!("http://{}", endpoint.addr()),
            ("", ""),
            false,
        );
        let weak = registry.downgrade();
        let rt = runtime()?;
        let start = Instant::now();
        let ran = rt
            .block_on(async { timeout(Duration::from_millis(5600), run(&weak, &settings)).await });
        check(ran.is_err(), "the reporter runs while its registry lives")?;
        let arrivals = endpoint.arrivals();
        let offsets = |line: &str| -> Vec<Duration> {
            arrivals
                .iter()
                .filter(|(_, request)| request.starts_with(line))
                .map(|(at, _)| at.duration_since(start))
                .collect()
        };
        let sends = offsets("POST /write");
        let pings = offsets("GET /ping");
        check(
            sends.len() >= 15,
            &format!("{} sends in 5.6 s", sends.len()),
        )?;
        for (n, offset) in sends.iter().enumerate() {
            let off_grid = Duration::from_nanos(
                u64::try_from(offset.as_nanos() % interval.as_nanos()).unwrap_or(u64::MAX),
            );
            check(
                *offset >= interval && off_grid < Duration::from_millis(100),
                &format!("send {n} at {offset:?}"),
            )?;
        }
        check_eq(&pings.len(), &1, "the pings")?;
        let ping = pings.first().copied().unwrap_or_default();
        check(
            ping >= Duration::from_secs(5) && ping < Duration::from_millis(5500),
            &format!("the ping at {ping:?}"),
        )
    }

    /// `influx.sampled-values`: a histogram's sample keeps as many of the values recorded as its
    /// reservoir holds, drawn at random: other ones from one sample to the next.
    #[test]
    fn reservoir_shape() -> TestResult {
        // Go: rcrowley/go-metrics sample.go:172 (ExpDecaySample.update, rand.Float64) and :514
        // (UniformSample.Update, rand.Int63n)
        let samples: [(usize, fn() -> Sample); 2] = [
            (1028, || new_exp_decay_sample(1028, 0.015)),
            (1024, || new_uniform_sample(1024)),
        ];
        for (size, make) in samples {
            let kept = || {
                let sample = make();
                (0..5000).for_each(|v| sample.update(v));
                let mut values = sample.values();
                values.sort_unstable();
                (sample.count(), values)
            };
            let (count, first) = kept();
            let (_, second) = kept();
            check_eq(&count, &5000, "the values recorded")?;
            check_eq(&first.len(), &size, "the values kept")?;
            check(
                first.windows(2).all(|w| w.first() < w.last())
                    && first.iter().all(|v| (0..5000).contains(v)),
                "values recorded, each once",
            )?;
            check(first != second, "other values from one sample to the next")?;
        }
        Ok(())
    }

    /// The engine selection starts the reporter of the go-metrics engine's registry when
    /// `metrics.influxdb.host` is set, with the configured database, measurement, credentials,
    /// interval and alignment.
    #[test]
    fn engine_reports_to_influxdb() -> TestResult {
        // Go: metrics/config/metrics.go:29-38
        let endpoint = Endpoint::new(vec![NO_CONTENT.to_vec()])?;
        let cfg = Metrics {
            influxdb: InfluxMetrics {
                host: format!("http://{}", endpoint.addr()),
                database: "pbsdb".to_owned(),
                measurement: "pbs".to_owned(),
                username: "admin".to_owned(),
                password: "pw".to_owned(),
                align_timestamps: true,
                metric_send_interval: 1,
            },
            ..Metrics::default()
        };
        let engine = new_metrics_engine(&cfg, &[], &[], &BTreeMap::new())?;
        engine.record_tmax_timeout();
        let deadline = Instant::now() + Duration::from_secs(5);
        let first = loop {
            if let Some(request) = endpoint.requests().into_iter().next() {
                break request;
            }
            if Instant::now() > deadline {
                return Err("no write in 5 s".into());
            }
            thread::sleep(Duration::from_millis(20));
        };
        let request = parse_request(&first)?;
        check_eq(&request.path.as_str(), &"/write", "the path")?;
        check_eq(
            &write_query(&request.query),
            &pairs(&[("db", "pbsdb")]),
            "the database",
        )?;
        check_eq(
            &request.header("Authorization"),
            &vec!["Basic YWRtaW46cHc="],
            "the credentials",
        )?;
        let line = request
            .body
            .lines()
            .find(|line| line.starts_with("pbs prebidserver.tmax_timeout.count="))
            .ok_or("no tmax_timeout line")?;
        let (value, time) = value_and_time(line)?;
        check_eq(&value, &"1i", "the counter")?;
        check(time.parse::<i64>()? % 1_000_000_000 == 0, "an aligned time")
    }
}
