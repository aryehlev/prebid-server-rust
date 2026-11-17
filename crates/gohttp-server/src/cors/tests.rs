//! Go: `rs/cors` `cors_test.go` and `utils_test.go`, case for case. Go's `TestConvert` has no
//! counterpart: the port has no `convert` helper.

use std::sync::{Arc, Mutex};

use pbs_gocompat::url::Url;

use super::{AllowedHeaders, Cors, CorsLog, Options, Origins, Wildcard};
use crate::{BoxFuture, Handler, Header, Request, ResponseWriter, httptest::ResponseRecorder};

/// Go: `testHandler`.
#[derive(Debug, Clone, Copy)]
struct TestHandler;

impl Handler for TestHandler {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        _r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let _written = w.write(b"bar").await;
        })
    }
}

/// Go: `allHeaders`.
const ALL_HEADERS: [&str; 8] = [
    "Vary",
    "Access-Control-Allow-Origin",
    "Access-Control-Allow-Methods",
    "Access-Control-Allow-Headers",
    "Access-Control-Allow-Credentials",
    "Access-Control-Allow-Private-Network",
    "Access-Control-Max-Age",
    "Access-Control-Expose-Headers",
];

/// Go: `assertHeaders`.
fn assert_headers(res_headers: &Header, exp_headers: &[(&str, &str)]) -> Result<(), String> {
    for name in ALL_HEADERS {
        let got = res_headers
            .get_raw(name)
            .map_or_else(String::new, |v| v.join(", "));
        let want = exp_headers
            .iter()
            .find(|&&(k, _)| k == name)
            .map_or("", |&(_, v)| v);
        if got != want {
            return Err(format!("Response header {name:?} = {got:?}, want {want:?}"));
        }
    }
    Ok(())
}

/// Go: `http.NewRequest(method, "http://example.com/foo", nil)` with `headers` added.
fn request(method: &str, headers: &[(&str, &str)]) -> Result<Request, String> {
    let mut req = Request {
        method: method.to_owned(),
        url: Url::parse("http://example.com/foo").map_err(|e| e.to_string())?,
        ..Request::default()
    };
    for &(name, value) in headers {
        req.header.add(name, value);
    }
    Ok(req)
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|&v| v.to_owned()).collect()
}

/// One case of Go's `TestSpec`.
struct Spec {
    name: &'static str,
    options: Options,
    method: &'static str,
    req_headers: &'static [(&'static str, &'static str)],
    res_headers: &'static [(&'static str, &'static str)],
    origin_allowed: bool,
}

const PREFLIGHT_VARY: &str =
    "Origin, Access-Control-Request-Method, Access-Control-Request-Headers";

fn origins(allowed: &[&str]) -> Options {
    Options {
        allowed_origins: strings(allowed),
        ..Options::default()
    }
}

fn foo_origin(o: &str) -> bool {
    o.starts_with("http://foo")
}

/// Go's `TestSpec` cases 1-5: origins that are allowed.
fn spec_allowed_origin_cases() -> Vec<Spec> {
    vec![
        Spec {
            name: "NoConfig",
            options: Options::default(),
            method: "GET",
            req_headers: &[],
            res_headers: &[("Vary", "Origin")],
            origin_allowed: true,
        },
        Spec {
            name: "MatchAllOrigin",
            options: origins(&["*"]),
            method: "GET",
            req_headers: &[("Origin", "http://foobar.com")],
            res_headers: &[("Vary", "Origin"), ("Access-Control-Allow-Origin", "*")],
            origin_allowed: true,
        },
        Spec {
            name: "MatchAllOriginWithCredentials",
            options: Options {
                allow_credentials: true,
                ..origins(&["*"])
            },
            method: "GET",
            req_headers: &[("Origin", "http://foobar.com")],
            res_headers: &[
                ("Vary", "Origin"),
                ("Access-Control-Allow-Origin", "*"),
                ("Access-Control-Allow-Credentials", "true"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "AllowedOrigin",
            options: origins(&["http://foobar.com"]),
            method: "GET",
            req_headers: &[("Origin", "http://foobar.com")],
            res_headers: &[
                ("Vary", "Origin"),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "WildcardOrigin",
            options: origins(&["http://*.bar.com"]),
            method: "GET",
            req_headers: &[("Origin", "http://foo.bar.com")],
            res_headers: &[
                ("Vary", "Origin"),
                ("Access-Control-Allow-Origin", "http://foo.bar.com"),
            ],
            origin_allowed: true,
        },
    ]
}

/// Go's `TestSpec` cases 6-11: disallowed origins and the origin functions.
fn spec_origin_check_cases() -> Vec<Spec> {
    vec![
        Spec {
            name: "DisallowedOrigin",
            options: origins(&["http://foobar.com"]),
            method: "GET",
            req_headers: &[("Origin", "http://barbaz.com")],
            res_headers: &[("Vary", "Origin")],
            origin_allowed: false,
        },
        Spec {
            name: "DisallowedWildcardOrigin",
            options: origins(&["http://*.bar.com"]),
            method: "GET",
            req_headers: &[("Origin", "http://foo.baz.com")],
            res_headers: &[("Vary", "Origin")],
            origin_allowed: false,
        },
        Spec {
            name: "AllowedOriginFuncMatch",
            options: Options {
                allow_origin_func: Some(Arc::new(foo_origin)),
                ..Options::default()
            },
            method: "GET",
            req_headers: &[("Origin", "http://foobar.com")],
            res_headers: &[
                ("Vary", "Origin"),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "AllowOriginRequestFuncMatch",
            options: Options {
                allow_origin_request_func: Some(Arc::new(|r: &Request, o: &str| {
                    foo_origin(o) && r.header.get("Authorization") == "secret"
                })),
                ..Options::default()
            },
            method: "GET",
            req_headers: &[("Origin", "http://foobar.com"), ("Authorization", "secret")],
            res_headers: &[
                ("Vary", "Origin"),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "AllowOriginVaryRequestFuncMatch",
            options: Options {
                allow_origin_vary_request_func: Some(Arc::new(|r: &Request, o: &str| {
                    (
                        foo_origin(o) && r.header.get("Authorization") == "secret",
                        strings(&["Authorization"]),
                    )
                })),
                ..Options::default()
            },
            method: "GET",
            req_headers: &[("Origin", "http://foobar.com"), ("Authorization", "secret")],
            res_headers: &[
                ("Vary", "Origin, Authorization"),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "AllowOriginRequestFuncNotMatch",
            options: Options {
                allow_origin_request_func: Some(Arc::new(|r: &Request, o: &str| {
                    foo_origin(o) && r.header.get("Authorization") == "secret"
                })),
                ..Options::default()
            },
            method: "GET",
            req_headers: &[
                ("Origin", "http://foobar.com"),
                ("Authorization", "not-secret"),
            ],
            res_headers: &[("Vary", "Origin")],
            origin_allowed: false,
        },
    ]
}

/// Go's `TestSpec` cases 12-15: preflight methods and `MaxAge`.
fn spec_method_cases() -> Vec<Spec> {
    vec![
        Spec {
            name: "MaxAge",
            options: Options {
                allowed_methods: strings(&["GET"]),
                max_age: 10,
                ..origins(&["http://example.com/"])
            },
            method: "OPTIONS",
            req_headers: &[
                ("Origin", "http://example.com/"),
                ("Access-Control-Request-Method", "GET"),
            ],
            res_headers: &[
                ("Vary", PREFLIGHT_VARY),
                ("Access-Control-Allow-Origin", "http://example.com/"),
                ("Access-Control-Allow-Methods", "GET"),
                ("Access-Control-Max-Age", "10"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "MaxAgeNegative",
            options: Options {
                allowed_methods: strings(&["GET"]),
                max_age: -1,
                ..origins(&["http://example.com/"])
            },
            method: "OPTIONS",
            req_headers: &[
                ("Origin", "http://example.com/"),
                ("Access-Control-Request-Method", "GET"),
            ],
            res_headers: &[
                ("Vary", PREFLIGHT_VARY),
                ("Access-Control-Allow-Origin", "http://example.com/"),
                ("Access-Control-Allow-Methods", "GET"),
                ("Access-Control-Max-Age", "0"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "AllowedMethod",
            options: Options {
                allowed_methods: strings(&["PUT", "DELETE"]),
                ..origins(&["http://foobar.com"])
            },
            method: "OPTIONS",
            req_headers: &[
                ("Origin", "http://foobar.com"),
                ("Access-Control-Request-Method", "PUT"),
            ],
            res_headers: &[
                ("Vary", PREFLIGHT_VARY),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
                ("Access-Control-Allow-Methods", "PUT"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "DisallowedMethod",
            options: Options {
                allowed_methods: strings(&["PUT", "DELETE"]),
                ..origins(&["http://foobar.com"])
            },
            method: "OPTIONS",
            req_headers: &[
                ("Origin", "http://foobar.com"),
                ("Access-Control-Request-Method", "PATCH"),
            ],
            res_headers: &[("Vary", PREFLIGHT_VARY)],
            origin_allowed: true,
        },
    ]
}

/// Go's `TestSpec` cases 16-20: allowed and exposed headers.
fn spec_header_cases() -> Vec<Spec> {
    vec![
        Spec {
            name: "AllowedHeaders",
            options: Options {
                allowed_headers: strings(&["X-Header-1", "x-header-2", "X-HEADER-3"]),
                ..origins(&["http://foobar.com"])
            },
            method: "OPTIONS",
            req_headers: &[
                ("Origin", "http://foobar.com"),
                ("Access-Control-Request-Method", "GET"),
                ("Access-Control-Request-Headers", "x-header-1,x-header-2"),
            ],
            res_headers: &[
                ("Vary", PREFLIGHT_VARY),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
                ("Access-Control-Allow-Methods", "GET"),
                ("Access-Control-Allow-Headers", "x-header-1,x-header-2"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "DefaultAllowedHeaders",
            options: Options {
                allowed_headers: Vec::new(),
                ..origins(&["http://foobar.com"])
            },
            method: "OPTIONS",
            req_headers: &[
                ("Origin", "http://foobar.com"),
                ("Access-Control-Request-Method", "GET"),
                ("Access-Control-Request-Headers", "x-requested-with"),
            ],
            res_headers: &[
                ("Vary", PREFLIGHT_VARY),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
                ("Access-Control-Allow-Methods", "GET"),
                ("Access-Control-Allow-Headers", "x-requested-with"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "AllowedWildcardHeader",
            options: Options {
                allowed_headers: strings(&["*"]),
                ..origins(&["http://foobar.com"])
            },
            method: "OPTIONS",
            req_headers: &[
                ("Origin", "http://foobar.com"),
                ("Access-Control-Request-Method", "GET"),
                ("Access-Control-Request-Headers", "x-header-1,x-header-2"),
            ],
            res_headers: &[
                ("Vary", PREFLIGHT_VARY),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
                ("Access-Control-Allow-Methods", "GET"),
                ("Access-Control-Allow-Headers", "x-header-1,x-header-2"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "DisallowedHeader",
            options: Options {
                allowed_headers: strings(&["X-Header-1", "x-header-2"]),
                ..origins(&["http://foobar.com"])
            },
            method: "OPTIONS",
            req_headers: &[
                ("Origin", "http://foobar.com"),
                ("Access-Control-Request-Method", "GET"),
                ("Access-Control-Request-Headers", "x-header-1,x-header-3"),
            ],
            res_headers: &[("Vary", PREFLIGHT_VARY)],
            origin_allowed: true,
        },
        Spec {
            name: "ExposedHeader",
            options: Options {
                exposed_headers: strings(&["X-Header-1", "x-header-2"]),
                ..origins(&["http://foobar.com"])
            },
            method: "GET",
            req_headers: &[("Origin", "http://foobar.com")],
            res_headers: &[
                ("Vary", "Origin"),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
                ("Access-Control-Expose-Headers", "X-Header-1, X-Header-2"),
            ],
            origin_allowed: true,
        },
    ]
}

/// Go's `TestSpec` cases 21-23: credentials and private network access.
fn spec_option_cases() -> Vec<Spec> {
    vec![
        Spec {
            name: "AllowedCredentials",
            options: Options {
                allow_credentials: true,
                ..origins(&["http://foobar.com"])
            },
            method: "OPTIONS",
            req_headers: &[
                ("Origin", "http://foobar.com"),
                ("Access-Control-Request-Method", "GET"),
            ],
            res_headers: &[
                ("Vary", PREFLIGHT_VARY),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
                ("Access-Control-Allow-Methods", "GET"),
                ("Access-Control-Allow-Credentials", "true"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "AllowedPrivateNetwork",
            options: Options {
                allow_private_network: true,
                ..origins(&["http://foobar.com"])
            },
            method: "OPTIONS",
            req_headers: &[
                ("Origin", "http://foobar.com"),
                ("Access-Control-Request-Method", "GET"),
                ("Access-Control-Request-Private-Network", "true"),
            ],
            res_headers: &[
                (
                    "Vary",
                    "Origin, Access-Control-Request-Method, Access-Control-Request-Headers, \
                     Access-Control-Request-Private-Network",
                ),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
                ("Access-Control-Allow-Methods", "GET"),
                ("Access-Control-Allow-Private-Network", "true"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "DisallowedPrivateNetwork",
            options: origins(&["http://foobar.com"]),
            method: "OPTIONS",
            req_headers: &[
                ("Origin", "http://foobar.com"),
                ("Access-Control-Request-Method", "GET"),
                ("Access-Control-Request-PrivateNetwork", "true"),
            ],
            res_headers: &[
                ("Vary", PREFLIGHT_VARY),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
                ("Access-Control-Allow-Methods", "GET"),
            ],
            origin_allowed: true,
        },
    ]
}

/// Go's `TestSpec` cases 24-26: passthrough, `OPTIONS` without a preflight, and an origin
/// function next to `AllowedOrigins`.
fn spec_passthrough_cases() -> Vec<Spec> {
    vec![
        Spec {
            name: "OptionPassthrough",
            options: Options {
                options_passthrough: true,
                ..Options::default()
            },
            method: "OPTIONS",
            req_headers: &[
                ("Origin", "http://foobar.com"),
                ("Access-Control-Request-Method", "GET"),
            ],
            res_headers: &[
                ("Vary", PREFLIGHT_VARY),
                ("Access-Control-Allow-Origin", "*"),
                ("Access-Control-Allow-Methods", "GET"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "NonPreflightOptions",
            options: origins(&["http://foobar.com"]),
            method: "OPTIONS",
            req_headers: &[("Origin", "http://foobar.com")],
            res_headers: &[
                ("Vary", "Origin"),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
            ],
            origin_allowed: true,
        },
        Spec {
            name: "AllowedOriginsPlusAllowOriginFunc",
            options: Options {
                allow_origin_func: Some(Arc::new(|_: &str| true)),
                ..origins(&["*"])
            },
            method: "GET",
            req_headers: &[("Origin", "http://foobar.com")],
            res_headers: &[
                ("Vary", "Origin"),
                ("Access-Control-Allow-Origin", "http://foobar.com"),
            ],
            origin_allowed: true,
        },
    ]
}

/// Go: the subtests of one `TestSpec` case.
async fn run_spec(tc: &Spec) -> Result<(), String> {
    let s = Cors::new(tc.options.clone());
    let mut req = request(tc.method, tc.req_headers)?;

    // OriginAllowed
    let have = s.origin_allowed(&req);
    if have != tc.origin_allowed {
        return Err(format!(
            "OriginAllowed have: {have} want: {}",
            tc.origin_allowed
        ));
    }

    // Handler
    let mut res = ResponseRecorder::new();
    Cors::new(tc.options.clone())
        .handler(TestHandler)
        .serve_http(&mut res, &mut req)
        .await;
    assert_headers(res.header_map(), tc.res_headers).map_err(|e| format!("Handler: {e}"))?;

    // HandlerFunc
    let mut res_func = ResponseRecorder::new();
    s.handler_func(&mut res_func, &req);
    assert_headers(res_func.header_map(), tc.res_headers)
        .map_err(|e| format!("HandlerFunc: {e}"))?;

    // Negroni
    let mut res_negroni = ResponseRecorder::new();
    s.serve_http_next(&mut res_negroni, &mut req, &TestHandler)
        .await;
    assert_headers(res_negroni.header_map(), tc.res_headers).map_err(|e| format!("Negroni: {e}"))
}

#[tokio::test]
async fn spec() -> Result<(), String> {
    // Go: TestSpec.
    let cases = [
        spec_allowed_origin_cases(),
        spec_origin_check_cases(),
        spec_method_cases(),
        spec_header_cases(),
        spec_option_cases(),
        spec_passthrough_cases(),
    ];
    for tc in cases.iter().flatten() {
        run_spec(tc)
            .await
            .map_err(|e| format!("{}: {e}", tc.name))?;
    }
    Ok(())
}

#[test]
fn debug() -> Result<(), String> {
    // Go: TestDebug.
    let s = Cors::new(Options {
        log: CorsLog::Stdout,
        ..Options::default()
    });
    if s.log.is_none() {
        return Err("Logger not created when debug=true".to_owned());
    }
    Ok(())
}

#[test]
fn logger() -> Result<(), String> {
    // Go: TestLogger.
    let buf = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&buf);
    let s = Cors::new(Options {
        log: CorsLog::Logger(Arc::new(move |msg: &str| {
            if let Ok(mut b) = sink.lock() {
                b.push_str(msg);
            }
        })),
        ..Options::default()
    });
    if s.log.is_none() {
        return Err("Logger not created when Logger is set".to_owned());
    }
    s.logf(format_args!("test"));
    let logged = buf.lock().map_err(|e| e.to_string())?.clone();
    if logged != "test" {
        return Err(format!("Logger not used: {logged:?}"));
    }
    Ok(())
}

#[test]
fn default() -> Result<(), String> {
    // Go: TestDefault.
    let s = Cors::default();
    if s.log.is_some() {
        return Err("c.log should be nil when Default".to_owned());
    }
    if !matches!(s.origins, Origins::All) {
        return Err("c.allowedOriginsAll should be true when Default".to_owned());
    }
    if let AllowedHeaders::Set(set) = &s.allowed_headers
        && set.size() == 0
    {
        return Err("c.allowedHeaders should be empty when Default".to_owned());
    }
    if s.allowed_methods.is_empty() {
        return Err("c.allowedMethods should be nil when Default".to_owned());
    }
    Ok(())
}

#[test]
fn handle_preflight_invalid_origin_abortion() -> Result<(), String> {
    // Go: TestHandlePreflightInvalidOriginAbortion.
    let s = Cors::new(origins(&["http://foo.com"]));
    let mut res = ResponseRecorder::new();
    let req = request("OPTIONS", &[("Origin", "http://example.com/")])?;
    s.handle_preflight(&mut res, &req);
    assert_headers(res.header_map(), &[("Vary", PREFLIGHT_VARY)])
}

#[test]
fn handle_preflight_no_options_abortion() -> Result<(), String> {
    // Go: TestHandlePreflightNoOptionsAbortion.
    let s = Cors::new(Options::default());
    let mut res = ResponseRecorder::new();
    let req = request("GET", &[])?;
    s.handle_preflight(&mut res, &req);
    assert_headers(res.header_map(), &[])
}

#[test]
fn handle_actual_request_invalid_origin_abortion() -> Result<(), String> {
    // Go: TestHandleActualRequestInvalidOriginAbortion.
    let s = Cors::new(origins(&["http://foo.com"]));
    let mut res = ResponseRecorder::new();
    let req = request("GET", &[("Origin", "http://example.com/")])?;
    s.handle_actual_request(&mut res, &req);
    assert_headers(res.header_map(), &[("Vary", "Origin")])
}

#[test]
fn handle_actual_request_invalid_method_abortion() -> Result<(), String> {
    // Go: TestHandleActualRequestInvalidMethodAbortion.
    let s = Cors::new(Options {
        allowed_methods: strings(&["POST"]),
        allow_credentials: true,
        ..Options::default()
    });
    let mut res = ResponseRecorder::new();
    let req = request("GET", &[("Origin", "http://example.com/")])?;
    s.handle_actual_request(&mut res, &req);
    assert_headers(res.header_map(), &[("Vary", "Origin")])
}

#[test]
fn is_method_allowed_returns_false_with_no_methods() -> Result<(), String> {
    // Go: TestIsMethodAllowedReturnsFalseWithNoMethods.
    let mut s = Cors::new(Options::default());
    s.allowed_methods = Vec::new();
    if s.is_method_allowed("") {
        return Err("IsMethodAllowed should return false when c.allowedMethods is nil.".to_owned());
    }
    Ok(())
}

#[test]
fn is_method_allowed_returns_true_with_options() -> Result<(), String> {
    // Go: TestIsMethodAllowedReturnsTrueWithOptions.
    let s = Cors::new(Options::default());
    if !s.is_method_allowed("OPTIONS") {
        return Err("IsMethodAllowed should return true when c.allowedMethods is nil.".to_owned());
    }
    Ok(())
}

/// Go: the three subtests of `TestOptionsSuccessStatusCode*`.
async fn check_options_status(options: &Options, code: u16) -> Result<(), String> {
    let mut req = request("OPTIONS", &[("Access-Control-Request-Method", "GET")])?;
    let s = Cors::new(options.clone());

    let mut res = ResponseRecorder::new();
    Cors::new(options.clone())
        .handler(TestHandler)
        .serve_http(&mut res, &mut req)
        .await;
    let mut res_func = ResponseRecorder::new();
    s.handler_func(&mut res_func, &req);
    let mut res_negroni = ResponseRecorder::new();
    s.serve_http_next(&mut res_negroni, &mut req, &TestHandler)
        .await;

    for (sub, got) in [
        ("Handler", res.code()),
        ("HandlerFunc", res_func.code()),
        ("Negroni", res_negroni.code()),
    ] {
        if got != code {
            return Err(format!(
                "{sub}: expected response code to be {code} but got {got}"
            ));
        }
    }
    Ok(())
}

#[tokio::test]
async fn options_success_status_code_default() -> Result<(), String> {
    // Go: TestOptionsSuccessStatusCodeDefault.
    check_options_status(&Options::default(), 204).await
}

#[tokio::test]
async fn options_success_status_code_override() -> Result<(), String> {
    // Go: TestOptionsSuccessStatusCodeOverride.
    let options = Options {
        options_success_status: 200,
        ..Options::default()
    };
    check_options_status(&options, 200).await
}

#[tokio::test]
async fn access_control_expose_headers_presence() -> Result<(), String> {
    // Go: TestAccessControlExposeHeadersPresence.
    let cases = [
        ("omit", Options::default(), false),
        (
            "include",
            Options {
                exposed_headers: strings(&["X-Something"]),
                ..Options::default()
            },
            true,
        ),
    ];
    for (name, options, want) in cases {
        let s = Cors::new(options.clone());
        let mut req = request("GET", &[("Origin", "http://foobar.com")])?;

        let mut res = ResponseRecorder::new();
        Cors::new(options)
            .handler(TestHandler)
            .serve_http(&mut res, &mut req)
            .await;
        let mut res_func = ResponseRecorder::new();
        s.handler_func(&mut res_func, &req);
        let mut res_negroni = ResponseRecorder::new();
        s.serve_http_next(&mut res_negroni, &mut req, &TestHandler)
            .await;

        for (sub, h) in [
            ("Handler", res.header_map()),
            ("HandlerFunc", res_func.header_map()),
            ("Negroni", res_negroni.header_map()),
        ] {
            let have = h.get_raw("Access-Control-Expose-Headers").is_some();
            if have != want {
                return Err(format!(
                    "{name}/{sub}: Access-Control-Expose-Headers have: {have} want: {want}"
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn wildcard() -> Result<(), String> {
    // Go: TestWildcard.
    let w = Wildcard {
        prefix: "foo".to_owned(),
        suffix: "bar".to_owned(),
    };
    if !w.matches("foobar") {
        return Err("foo*bar should match foobar".to_owned());
    }
    if !w.matches("foobazbar") {
        return Err("foo*bar should match foobazbar".to_owned());
    }
    if w.matches("foobaz") {
        return Err("foo*bar should not match foobaz".to_owned());
    }
    let overlapping = Wildcard {
        prefix: "foo".to_owned(),
        suffix: "oof".to_owned(),
    };
    if overlapping.matches("foof") {
        return Err("foo*oof should not match foof".to_owned());
    }
    Ok(())
}
