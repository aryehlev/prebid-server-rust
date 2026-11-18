//! Go: `github.com/rs/cors` v1.11.0: a handler adding the CORS response headers of the W3C
//! specification, answering preflight requests itself.

mod sortedset;
#[cfg(test)]
mod tests;

use core::fmt::{self, Write as _};
use std::{
    io::{self, Write as _},
    sync::Arc,
};

use pbs_gocompat::{http::canonical_header_key, strings::to_lower, time::Time};
use sortedset::SortedSet;

use crate::{BoxFuture, Handler, Header, Request, ResponseWriter, STATUS_NO_CONTENT};

/// Go: `Options.AllowOriginFunc`.
pub type OriginFunc = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Go: `Options.AllowOriginRequestFunc`.
pub type OriginRequestFunc = Arc<dyn Fn(&Request, &str) -> bool + Send + Sync>;

/// Go: `Options.AllowOriginVaryRequestFunc`: whether the origin is allowed, and the request
/// headers the decision depended on (added to `Vary`).
pub type OriginVaryRequestFunc = Arc<dyn Fn(&Request, &str) -> (bool, Vec<String>) + Send + Sync>;

/// Go: `cors.Logger` (`Printf`), receiving each formatted debug line.
pub type Logger = Arc<dyn Fn(&str) + Send + Sync>;

/// Go: `cors.Options`.
#[derive(Clone, Default)]
pub struct Options {
    /// Go: `AllowedOrigins` (`*` allows all; one `*` wildcard per origin).
    pub allowed_origins: Vec<String>,
    /// Go: `AllowOriginFunc`.
    pub allow_origin_func: Option<OriginFunc>,
    /// Go: `AllowOriginRequestFunc`.
    pub allow_origin_request_func: Option<OriginRequestFunc>,
    /// Go: `AllowOriginVaryRequestFunc`.
    pub allow_origin_vary_request_func: Option<OriginVaryRequestFunc>,
    /// Go: `AllowedMethods` (default `GET`, `POST`, `HEAD`).
    pub allowed_methods: Vec<String>,
    /// Go: `AllowedHeaders` (default `Accept`, `Content-Type`, `X-Requested-With`).
    pub allowed_headers: Vec<String>,
    /// Go: `ExposedHeaders`.
    pub exposed_headers: Vec<String>,
    /// Go: `MaxAge` (seconds; negative sends `0`).
    pub max_age: i64,
    /// Go: `AllowCredentials`.
    pub allow_credentials: bool,
    /// Go: `AllowPrivateNetwork`.
    pub allow_private_network: bool,
    /// Go: `OptionsPassthrough`.
    pub options_passthrough: bool,
    /// Go: `OptionsSuccessStatus` (zero: 204).
    pub options_success_status: u16,
    /// Go: `Debug` and `Logger`.
    pub log: CorsLog,
}

/// Go: `Options.Debug` and `Options.Logger`: where the handler's debug lines go.
#[derive(Clone, Default)]
pub enum CorsLog {
    /// Go: `Debug` false and no `Logger`.
    #[default]
    Off,
    /// Go: `Debug` true without a `Logger`: standard output, prefixed `[cors] ` and the time.
    Stdout,
    /// Go: `Logger` (used whether or not `Debug` is set).
    Logger(Logger),
}

impl fmt::Debug for CorsLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "Off",
            Self::Stdout => "Stdout",
            Self::Logger(_) => "Logger",
        })
    }
}

impl fmt::Debug for Options {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("allowed_origins", &self.allowed_origins)
            .field("allow_origin_func", &self.allow_origin_func.is_some())
            .field(
                "allow_origin_request_func",
                &self.allow_origin_request_func.is_some(),
            )
            .field(
                "allow_origin_vary_request_func",
                &self.allow_origin_vary_request_func.is_some(),
            )
            .field("allowed_methods", &self.allowed_methods)
            .field("allowed_headers", &self.allowed_headers)
            .field("exposed_headers", &self.exposed_headers)
            .field("max_age", &self.max_age)
            .field("allow_credentials", &self.allow_credentials)
            .field("allow_private_network", &self.allow_private_network)
            .field("options_passthrough", &self.options_passthrough)
            .field("options_success_status", &self.options_success_status)
            .field("log", &self.log)
            .finish()
    }
}

/// Go: `wildcard`.
#[derive(Debug, Clone)]
struct Wildcard {
    prefix: String,
    suffix: String,
}

impl Wildcard {
    /// Go: `wildcard.match`.
    fn matches(&self, s: &str) -> bool {
        s.len() >= self.prefix.len() + self.suffix.len()
            && s.starts_with(self.prefix.as_str())
            && s.ends_with(self.suffix.as_str())
    }
}

/// Go: the origin check `Cors.allowOriginFunc` wraps.
#[derive(Clone)]
enum OriginCheck {
    Plain(OriginFunc),
    Request(OriginRequestFunc),
    Vary(OriginVaryRequestFunc),
}

/// Go: `allowedOriginsAll`, `allowedOrigins`, `allowedWOrigins` and `allowOriginFunc`.
#[derive(Clone)]
enum Origins {
    /// Go: `allowedOriginsAll` (replies `*`).
    All,
    /// Go: `allowOriginFunc`.
    Func(OriginCheck),
    /// Go: `allowedOrigins` and `allowedWOrigins`.
    List {
        exact: Vec<String>,
        wildcards: Vec<Wildcard>,
    },
}

/// Go: `allowedHeadersAll` and `allowedHeaders`.
#[derive(Debug, Clone)]
enum AllowedHeaders {
    All,
    Set(SortedSet),
}

/// Go: `*cors.Cors`.
pub struct Cors {
    log: Option<Logger>,
    origins: Origins,
    allowed_headers: AllowedHeaders,
    allowed_methods: Vec<String>,
    exposed_headers: Option<String>,
    max_age: Option<String>,
    options_success_status: u16,
    allow_credentials: bool,
    allow_private_network: bool,
    option_passthrough: bool,
    preflight_vary: &'static str,
}

impl fmt::Debug for Cors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cors")
            .field("allowed_headers", &self.allowed_headers)
            .field("allowed_methods", &self.allowed_methods)
            .field("allow_credentials", &self.allow_credentials)
            .finish_non_exhaustive()
    }
}

impl Default for Cors {
    /// Go: `cors.Default()`.
    fn default() -> Self {
        Self::new(Options::default())
    }
}

/// Go: the `%v` of an `http.Header` (`map[K:[v1 v2] ...]`).
fn format_header_map(h: &Header) -> String {
    let mut out = String::from("map[");
    for (i, (k, vv)) in h.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let _ok = write!(out, "{k}:[{}]", vv.join(" "));
    }
    out.push(']');
    out
}

/// Go: `log.New(os.Stdout, "[cors] ", log.LstdFlags)`.
fn stdout_logger(msg: &str) {
    let mut line = String::from("[cors] ");
    line.push_str(&Time::now().format("2006/01/02 15:04:05"));
    line.push(' ');
    line.push_str(msg);
    if !msg.ends_with('\n') {
        line.push('\n');
    }
    let _ignored = io::stdout().write_all(line.as_bytes());
}

impl Cors {
    /// Go: `cors.New(options)`.
    #[must_use]
    pub fn new(options: Options) -> Self {
        let log: Option<Logger> = match options.log {
            CorsLog::Off => None,
            CorsLog::Stdout => Some(Arc::new(stdout_logger)),
            CorsLog::Logger(l) => Some(l),
        };
        let origins = if let Some(f) = options.allow_origin_vary_request_func {
            Origins::Func(OriginCheck::Vary(f))
        } else if let Some(f) = options.allow_origin_request_func {
            Origins::Func(OriginCheck::Request(f))
        } else if let Some(f) = options.allow_origin_func {
            Origins::Func(OriginCheck::Plain(f))
        } else {
            origin_list(&options.allowed_origins)
        };
        let allowed_headers = if options.allowed_headers.is_empty() {
            AllowedHeaders::Set(SortedSet::new(vec![
                "accept".to_owned(),
                "content-type".to_owned(),
                "x-requested-with".to_owned(),
            ]))
        } else if options.allowed_headers.iter().any(|h| h == "*") {
            AllowedHeaders::All
        } else {
            AllowedHeaders::Set(SortedSet::new(
                options
                    .allowed_headers
                    .iter()
                    .map(|h| to_lower(h))
                    .collect(),
            ))
        };
        let allowed_methods = if options.allowed_methods.is_empty() {
            vec!["GET".to_owned(), "POST".to_owned(), "HEAD".to_owned()]
        } else {
            options.allowed_methods
        };
        let options_success_status = if options.options_success_status == 0 {
            STATUS_NO_CONTENT
        } else {
            options.options_success_status
        };
        let exposed_headers =
            (!options.exposed_headers.is_empty()).then(|| join_canonical(&options.exposed_headers));
        let preflight_vary = if options.allow_private_network {
            "Origin, Access-Control-Request-Method, Access-Control-Request-Headers, \
             Access-Control-Request-Private-Network"
        } else {
            "Origin, Access-Control-Request-Method, Access-Control-Request-Headers"
        };
        let max_age = match options.max_age {
            0 => None,
            n if n > 0 => Some(n.to_string()),
            _ => Some("0".to_owned()),
        };
        Self {
            log,
            origins,
            allowed_headers,
            allowed_methods,
            exposed_headers,
            max_age,
            options_success_status,
            allow_credentials: options.allow_credentials,
            allow_private_network: options.allow_private_network,
            option_passthrough: options.options_passthrough,
            preflight_vary,
        }
    }

    /// Go: `cors.AllowAll()`.
    #[must_use]
    pub fn allow_all() -> Self {
        Self::new(Options {
            allowed_origins: vec!["*".to_owned()],
            allowed_methods: ["HEAD", "GET", "POST", "PUT", "PATCH", "DELETE"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            allowed_headers: vec!["*".to_owned()],
            allow_credentials: false,
            ..Options::default()
        })
    }

    /// Go: `c.Handler(h)`: `h` behind this CORS handling.
    #[must_use]
    pub const fn handler<H: Handler>(self, h: H) -> CorsHandler<H> {
        CorsHandler {
            cors: self,
            next: h,
        }
    }

    /// Go: `c.HandlerFunc(w, r)`: the CORS handling alone, answering preflight requests.
    pub fn handler_func(&self, w: &mut dyn ResponseWriter, r: &Request) {
        if is_preflight(r) {
            self.logf(format_args!("HandlerFunc: Preflight request"));
            self.handle_preflight(w, r);
            w.write_header(self.options_success_status);
        } else {
            self.logf(format_args!("HandlerFunc: Actual request"));
            self.handle_actual_request(w, r);
        }
    }

    /// Go: `c.ServeHTTP(w, r, next)` (the negroni middleware form).
    pub async fn serve_http_next(
        &self,
        w: &mut dyn ResponseWriter,
        r: &mut Request,
        next: &dyn Handler,
    ) {
        if is_preflight(r) {
            self.logf(format_args!("ServeHTTP: Preflight request"));
            self.handle_preflight(w, r);
            if self.option_passthrough {
                next.serve_http(w, r).await;
            } else {
                w.write_header(self.options_success_status);
            }
        } else {
            self.logf(format_args!("ServeHTTP: Actual request"));
            self.handle_actual_request(w, r);
            next.serve_http(w, r).await;
        }
    }

    /// Go: `handlePreflight`.
    fn handle_preflight(&self, w: &mut dyn ResponseWriter, r: &Request) {
        let headers = w.header();
        let origin = r.header.get("Origin");
        if r.method != "OPTIONS" {
            self.logf(format_args!("  Preflight aborted: {}!=OPTIONS", r.method));
            return;
        }
        headers.add("Vary", self.preflight_vary);
        let (allowed, additional_vary) = self.is_origin_allowed(r, origin);
        if !additional_vary.is_empty() {
            headers.add("Vary", join_canonical(&additional_vary));
        }
        if origin.is_empty() {
            self.logf(format_args!("  Preflight aborted: empty origin"));
            return;
        }
        if !allowed {
            self.logf(format_args!(
                "  Preflight aborted: origin '{origin}' not allowed"
            ));
            return;
        }
        let req_method = r.header.get("Access-Control-Request-Method");
        if !self.is_method_allowed(req_method) {
            self.logf(format_args!(
                "  Preflight aborted: method '{req_method}' not allowed"
            ));
            return;
        }
        let req_headers = r
            .header
            .get_raw("Access-Control-Request-Headers")
            .and_then(<[String]>::first);
        if let Some(rh) = req_headers
            && let AllowedHeaders::Set(set) = &self.allowed_headers
            && !set.subsumes(rh)
        {
            self.logf(format_args!(
                "  Preflight aborted: headers '{rh}' not allowed"
            ));
            return;
        }
        if matches!(self.origins, Origins::All) {
            headers.insert_raw("Access-Control-Allow-Origin", vec!["*".to_owned()]);
        } else {
            copy_values(headers, "Access-Control-Allow-Origin", &r.header, "Origin");
        }
        copy_values(
            headers,
            "Access-Control-Allow-Methods",
            &r.header,
            "Access-Control-Request-Method",
        );
        if let Some(rh) = req_headers
            && !rh.is_empty()
        {
            headers.insert_raw("Access-Control-Allow-Headers", vec![rh.clone()]);
        }
        if self.allow_credentials {
            headers.insert_raw("Access-Control-Allow-Credentials", vec!["true".to_owned()]);
        }
        if self.allow_private_network
            && r.header.get("Access-Control-Request-Private-Network") == "true"
        {
            headers.insert_raw(
                "Access-Control-Allow-Private-Network",
                vec!["true".to_owned()],
            );
        }
        if let Some(max_age) = &self.max_age {
            headers.insert_raw("Access-Control-Max-Age", vec![max_age.clone()]);
        }
        if self.log.is_some() {
            let dump = format_header_map(headers);
            self.logf(format_args!("  Preflight response headers: {dump}"));
        }
    }

    /// Go: `handleActualRequest`.
    fn handle_actual_request(&self, w: &mut dyn ResponseWriter, r: &Request) {
        let headers = w.header();
        let origin = r.header.get("Origin");
        let (allowed, additional_vary) = self.is_origin_allowed(r, origin);
        headers.add("Vary", "Origin");
        if !additional_vary.is_empty() {
            headers.add("Vary", join_canonical(&additional_vary));
        }
        if origin.is_empty() {
            self.logf(format_args!(
                "  Actual request no headers added: missing origin"
            ));
            return;
        }
        if !allowed {
            self.logf(format_args!(
                "  Actual request no headers added: origin '{origin}' not allowed"
            ));
            return;
        }
        if !self.is_method_allowed(&r.method) {
            self.logf(format_args!(
                "  Actual request no headers added: method '{}' not allowed",
                r.method
            ));
            return;
        }
        if matches!(self.origins, Origins::All) {
            headers.insert_raw("Access-Control-Allow-Origin", vec!["*".to_owned()]);
        } else {
            copy_values(headers, "Access-Control-Allow-Origin", &r.header, "Origin");
        }
        if let Some(exposed) = &self.exposed_headers {
            headers.insert_raw("Access-Control-Expose-Headers", vec![exposed.clone()]);
        }
        if self.allow_credentials {
            headers.insert_raw("Access-Control-Allow-Credentials", vec!["true".to_owned()]);
        }
        if self.log.is_some() {
            let dump = format_header_map(headers);
            self.logf(format_args!("  Actual response added headers: {dump}"));
        }
    }

    /// Go: `c.logf`.
    fn logf(&self, args: fmt::Arguments<'_>) {
        if let Some(log) = &self.log {
            log(&args.to_string());
        }
    }

    /// Go: `c.OriginAllowed(r)`.
    #[must_use]
    pub fn origin_allowed(&self, r: &Request) -> bool {
        self.is_origin_allowed(r, r.header.get("Origin")).0
    }

    /// Go: `isOriginAllowed`.
    fn is_origin_allowed(&self, r: &Request, origin: &str) -> (bool, Vec<String>) {
        match &self.origins {
            Origins::Func(OriginCheck::Plain(f)) => (f(origin), Vec::new()),
            Origins::Func(OriginCheck::Request(f)) => (f(r, origin), Vec::new()),
            Origins::Func(OriginCheck::Vary(f)) => f(r, origin),
            Origins::All => (true, Vec::new()),
            Origins::List { exact, wildcards } => {
                let origin = to_lower(origin);
                let allowed =
                    exact.contains(&origin) || wildcards.iter().any(|w| w.matches(&origin));
                (allowed, Vec::new())
            }
        }
    }

    /// Go: `isMethodAllowed`.
    fn is_method_allowed(&self, method: &str) -> bool {
        if self.allowed_methods.is_empty() {
            return false;
        }
        method == "OPTIONS" || self.allowed_methods.iter().any(|m| m == method)
    }
}

/// Go: the `AllowedOrigins` loop of `cors.New` (an empty list allows all origins).
fn origin_list(allowed: &[String]) -> Origins {
    if allowed.is_empty() {
        return Origins::All;
    }
    let mut exact = Vec::new();
    let mut wildcards = Vec::new();
    for origin in allowed {
        let origin = to_lower(origin);
        if origin == "*" {
            return Origins::All;
        }
        if let Some((prefix, suffix)) = origin.split_once('*') {
            wildcards.push(Wildcard {
                prefix: prefix.to_owned(),
                suffix: suffix.to_owned(),
            });
        } else {
            exact.push(origin);
        }
    }
    Origins::List { exact, wildcards }
}

/// Go: `r.Method == http.MethodOptions && r.Header.Get("Access-Control-Request-Method") != ""`.
fn is_preflight(r: &Request) -> bool {
    r.method == "OPTIONS" && !r.header.get("Access-Control-Request-Method").is_empty()
}

/// Go: `strings.Join(convert(headers, http.CanonicalHeaderKey), ", ")`.
fn join_canonical(headers: &[String]) -> String {
    headers
        .iter()
        .map(|h| canonical_header_key(h))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Go: `headers[dst] = r.Header[src]` (all values; a missing key stores nil, which Go's
/// header writer skips).
fn copy_values(headers: &mut Header, dst: &str, src: &Header, key: &str) {
    match src.get_raw(key) {
        Some(v) => headers.insert_raw(dst, v.to_vec()),
        None => headers.insert_raw(dst, Vec::new()),
    }
}

/// Go: the handler `c.Handler(h)` returns.
pub struct CorsHandler<H> {
    cors: Cors,
    next: H,
}

impl<H> fmt::Debug for CorsHandler<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CorsHandler")
            .field("cors", &self.cors)
            .finish_non_exhaustive()
    }
}

impl<H: Handler> Handler for CorsHandler<H> {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let c = &self.cors;
            if is_preflight(r) {
                c.logf(format_args!("Handler: Preflight request"));
                c.handle_preflight(w, r);
                if c.option_passthrough {
                    self.next.serve_http(w, r).await;
                } else {
                    w.write_header(c.options_success_status);
                }
            } else {
                c.logf(format_args!("Handler: Actual request"));
                c.handle_actual_request(w, r);
                self.next.serve_http(w, r).await;
            }
        })
    }
}
