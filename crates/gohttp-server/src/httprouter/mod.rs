//! Go: `github.com/julienschmidt/httprouter` v1.3.0.
//!
//! A radix-tree request router with named (`:name`) and catch-all (`*name`) parameters,
//! trailing-slash and case-fixing redirects, automatic `OPTIONS` replies and
//! `405 Method Not Allowed` handling.
//!
//! Registration errors that Go reports by panicking are returned as [`RouteError`] with Go's
//! panic text; Prebid treats them as fatal startup errors, as a panic during `router.New` is in
//! Go. After an error the router holds whatever Go's would hold after recovering that panic.

mod path;
#[cfg(test)]
mod tests;
mod tree;

use core::{error::Error, fmt, mem};
use std::sync::Arc;

pub use path::clean_path;
use tree::Node;

use crate::{
    BoxFuture, CatchUnwind, Dir, FileServer, Handler, PanicPayload, Request, RequestContext,
    ResponseWriter, STATUS_METHOD_NOT_ALLOWED, error, not_found, redirect, status_text,
};

/// Go: `httprouter.Param`: a URL parameter, a key and a value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Param {
    /// Go: `Key`.
    pub key: String,
    /// Go: `Value`.
    pub value: String,
}

/// Go: `httprouter.Params`: the URL parameters of a route, in path order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Params(pub Vec<Param>);

impl Params {
    /// Go: `ps.ByName(name)`: the value of the first parameter named `name`, or `""`.
    #[must_use]
    pub fn by_name(&self, name: &str) -> &str {
        self.0
            .iter()
            .find(|p| p.key == name)
            .map_or("", |p| p.value.as_str())
    }

    /// The number of parameters (Go: `len(ps)`).
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no parameters.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Go: `httprouter.ParamsFromContext(ctx)`: the parameters [`Router::handler`] stored in the
/// request context, or none.
#[must_use]
pub fn params_from_context(ctx: &RequestContext) -> Params {
    ctx.value::<Params>().cloned().unwrap_or_default()
}

/// Go: `httprouter.Handle`: a request handler that also receives the route's parameters.
pub trait Handle: Send + Sync {
    /// Serves `r`, writing the response to `w`.
    fn handle<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
        ps: Params,
    ) -> BoxFuture<'a, ()>;
}

impl<H: Handle + ?Sized> Handle for Arc<H> {
    fn handle<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
        ps: Params,
    ) -> BoxFuture<'a, ()> {
        (**self).handle(w, r, ps)
    }
}

impl<H: Handle + ?Sized> Handle for Box<H> {
    fn handle<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
        ps: Params,
    ) -> BoxFuture<'a, ()> {
        (**self).handle(w, r, ps)
    }
}

/// A function as a [`Handle`] (Go: a `func(w, r, ps)` literal).
#[derive(Clone, Copy)]
pub struct HandleFunc<F>(pub F);

impl<F> fmt::Debug for HandleFunc<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HandleFunc")
    }
}

impl<F> Handle for HandleFunc<F>
where
    F: for<'a> Fn(&'a mut dyn ResponseWriter, &'a mut Request, Params) -> BoxFuture<'a, ()>
        + Send
        + Sync,
{
    fn handle<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
        ps: Params,
    ) -> BoxFuture<'a, ()> {
        (self.0)(w, r, ps)
    }
}

/// Go: `router.PanicHandler`: called with the value a handler panicked with.
pub type PanicHandler = Arc<
    dyn for<'a> Fn(&'a mut dyn ResponseWriter, &'a mut Request, PanicPayload) -> BoxFuture<'a, ()>
        + Send
        + Sync,
>;

/// A route registration Go's `httprouter` rejects by panicking, with the panic text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteError(pub String);

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for RouteError {}

/// Go: `Router.Handler`'s adapter of an `http.Handler`.
struct HandlerAdapter(Arc<dyn Handler>);

impl Handle for HandlerAdapter {
    fn handle<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
        ps: Params,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if ps.is_empty() {
                return self.0.serve_http(w, r).await;
            }
            let derived = r.ctx.with_value(ps);
            let saved = mem::replace(&mut r.ctx, derived);
            self.0.serve_http(w, r).await;
            r.ctx = saved;
        })
    }
}

/// Go: `ServeFiles`' handle.
struct FilesHandle(FileServer);

impl Handle for FilesHandle {
    fn handle<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
        ps: Params,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            ps.by_name("filepath").clone_into(&mut r.url.path);
            self.0.serve_http(w, r).await;
        })
    }
}

/// Go: `RedirectTrailingSlash` and `RedirectFixedPath`.
#[derive(Debug, Clone, Copy)]
struct Redirects {
    trailing_slash: bool,
    fixed_path: bool,
}

/// Go: `*httprouter.Router` (`httprouter.New()` defaults: every redirect and automatic reply
/// enabled, no custom handlers).
pub struct Router {
    trees: Vec<(String, Node)>,
    redirect: Redirects,
    handle_method_not_allowed: bool,
    handle_options: bool,
    global_options: Option<Arc<dyn Handler>>,
    global_allowed: String,
    not_found: Option<Arc<dyn Handler>>,
    method_not_allowed: Option<Arc<dyn Handler>>,
    panic_handler: Option<PanicHandler>,
}

impl fmt::Debug for Router {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Router")
            .field(
                "methods",
                &self
                    .trees
                    .iter()
                    .map(|(m, _)| m.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("redirect", &self.redirect)
            .field("handle_method_not_allowed", &self.handle_method_not_allowed)
            .field("handle_options", &self.handle_options)
            .finish_non_exhaustive()
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

/// The result of [`Router::lookup`].
pub struct Lookup {
    /// The handle registered for the path, if any.
    pub handle: Option<Arc<dyn Handle>>,
    /// The path's parameters.
    pub params: Params,
    /// Whether a handle exists for the path with its trailing slash added or removed.
    pub tsr: bool,
}

impl fmt::Debug for Lookup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lookup")
            .field("handle", &self.handle.is_some())
            .field("params", &self.params)
            .field("tsr", &self.tsr)
            .finish()
    }
}

impl Router {
    /// Go: `httprouter.New()`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            trees: Vec::new(),
            redirect: Redirects {
                trailing_slash: true,
                fixed_path: true,
            },
            handle_method_not_allowed: true,
            handle_options: true,
            global_options: None,
            global_allowed: String::new(),
            not_found: None,
            method_not_allowed: None,
            panic_handler: None,
        }
    }

    /// Go: `RedirectTrailingSlash`.
    pub const fn set_redirect_trailing_slash(&mut self, on: bool) {
        self.redirect.trailing_slash = on;
    }

    /// Go: `RedirectFixedPath`.
    pub const fn set_redirect_fixed_path(&mut self, on: bool) {
        self.redirect.fixed_path = on;
    }

    /// Go: `HandleMethodNotAllowed`.
    pub const fn set_handle_method_not_allowed(&mut self, on: bool) {
        self.handle_method_not_allowed = on;
    }

    /// Go: `HandleOPTIONS`.
    pub const fn set_handle_options(&mut self, on: bool) {
        self.handle_options = on;
    }

    /// Go: `GlobalOPTIONS`.
    pub fn set_global_options(&mut self, handler: Option<Arc<dyn Handler>>) {
        self.global_options = handler;
    }

    /// Go: `NotFound`.
    pub fn set_not_found(&mut self, handler: Option<Arc<dyn Handler>>) {
        self.not_found = handler;
    }

    /// Go: `MethodNotAllowed`.
    pub fn set_method_not_allowed(&mut self, handler: Option<Arc<dyn Handler>>) {
        self.method_not_allowed = handler;
    }

    /// Go: `PanicHandler`.
    pub fn set_panic_handler(&mut self, handler: Option<PanicHandler>) {
        self.panic_handler = handler;
    }

    /// Go: `router.GET(path, handle)`.
    ///
    /// # Errors
    ///
    /// Go's panic text when the path is invalid or conflicts with a registered one.
    pub fn get<H: Handle + 'static>(&mut self, path: &str, handle: H) -> Result<(), RouteError> {
        self.handle("GET", path, handle)
    }

    /// Go: `router.HEAD(path, handle)`.
    ///
    /// # Errors
    ///
    /// As [`Router::get`].
    pub fn head<H: Handle + 'static>(&mut self, path: &str, handle: H) -> Result<(), RouteError> {
        self.handle("HEAD", path, handle)
    }

    /// Go: `router.OPTIONS(path, handle)`.
    ///
    /// # Errors
    ///
    /// As [`Router::get`].
    pub fn options<H: Handle + 'static>(
        &mut self,
        path: &str,
        handle: H,
    ) -> Result<(), RouteError> {
        self.handle("OPTIONS", path, handle)
    }

    /// Go: `router.POST(path, handle)`.
    ///
    /// # Errors
    ///
    /// As [`Router::get`].
    pub fn post<H: Handle + 'static>(&mut self, path: &str, handle: H) -> Result<(), RouteError> {
        self.handle("POST", path, handle)
    }

    /// Go: `router.PUT(path, handle)`.
    ///
    /// # Errors
    ///
    /// As [`Router::get`].
    pub fn put<H: Handle + 'static>(&mut self, path: &str, handle: H) -> Result<(), RouteError> {
        self.handle("PUT", path, handle)
    }

    /// Go: `router.PATCH(path, handle)`.
    ///
    /// # Errors
    ///
    /// As [`Router::get`].
    pub fn patch<H: Handle + 'static>(&mut self, path: &str, handle: H) -> Result<(), RouteError> {
        self.handle("PATCH", path, handle)
    }

    /// Go: `router.DELETE(path, handle)`.
    ///
    /// # Errors
    ///
    /// As [`Router::get`].
    pub fn delete<H: Handle + 'static>(&mut self, path: &str, handle: H) -> Result<(), RouteError> {
        self.handle("DELETE", path, handle)
    }

    /// Go: `router.Handle(method, path, handle)`.
    ///
    /// # Errors
    ///
    /// As [`Router::get`].
    pub fn handle<H: Handle + 'static>(
        &mut self,
        method: &str,
        path: &str,
        handle: H,
    ) -> Result<(), RouteError> {
        self.handle_arc(method, path, Arc::new(handle))
    }

    fn handle_arc(
        &mut self,
        method: &str,
        path: &str,
        handle: Arc<dyn Handle>,
    ) -> Result<(), RouteError> {
        if !path.starts_with('/') {
            return Err(RouteError(format!(
                "path must begin with '/' in path '{path}'"
            )));
        }
        let pos = if let Some(pos) = self.trees.iter().position(|(m, _)| m == method) {
            pos
        } else {
            self.trees.push((method.to_owned(), Node::default()));
            self.global_allowed = self.allowed("*", "");
            self.trees.len() - 1
        };
        match self.trees.get_mut(pos) {
            Some((_, root)) => root.add_route(path.as_bytes(), handle),
            None => Ok(()),
        }
    }

    /// Go: `router.Handler(method, path, handler)`: serves `handler`, with the route's
    /// parameters in the request context ([`params_from_context`]) when there are any.
    ///
    /// # Errors
    ///
    /// As [`Router::get`].
    pub fn handler(
        &mut self,
        method: &str,
        path: &str,
        handler: Arc<dyn Handler>,
    ) -> Result<(), RouteError> {
        self.handle(method, path, HandlerAdapter(handler))
    }

    /// Go: `router.ServeFiles(path, root)`: serves the files under `root` at `path`, which must
    /// end in `/*filepath`.
    ///
    /// # Errors
    ///
    /// Go's panic text when `path` does not end in `/*filepath`, or as [`Router::get`].
    pub fn serve_files(&mut self, path: &str, root: Dir) -> Result<(), RouteError> {
        if path.len() < 10 || !path.ends_with("/*filepath") {
            return Err(RouteError(format!(
                "path must end with /*filepath in path '{path}'"
            )));
        }
        self.get(path, FilesHandle(FileServer::new(root)))
    }

    /// Go: `router.Lookup(method, path)`.
    #[must_use]
    pub fn lookup(&self, method: &str, path: &str) -> Lookup {
        self.tree(method).map_or_else(
            || Lookup {
                handle: None,
                params: Params::default(),
                tsr: false,
            },
            |root| {
                let found = root.get_value(path);
                Lookup {
                    handle: found.handle.map(Arc::clone),
                    params: found.params,
                    tsr: found.tsr,
                }
            },
        )
    }

    fn tree(&self, method: &str) -> Option<&Node> {
        self.trees
            .iter()
            .find(|(m, _)| m == method)
            .map(|(_, root)| root)
    }

    /// Go: `router.allowed(path, reqMethod)`.
    fn allowed(&self, path: &str, req_method: &str) -> String {
        let mut allowed: Vec<&str> = Vec::with_capacity(9);
        if path == "*" {
            if !req_method.is_empty() {
                return self.global_allowed.clone();
            }
            allowed.extend(
                self.trees
                    .iter()
                    .map(|(m, _)| m.as_str())
                    .filter(|&m| m != "OPTIONS"),
            );
        } else {
            for (method, root) in &self.trees {
                if method == req_method || method == "OPTIONS" {
                    continue;
                }
                if root.get_value(path).handle.is_some() {
                    allowed.push(method);
                }
            }
        }
        if allowed.is_empty() {
            return String::new();
        }
        allowed.push("OPTIONS");
        allowed.sort_unstable();
        allowed.join(", ")
    }

    /// Go: `router.ServeHTTP` without the panic recovery.
    async fn route(&self, w: &mut dyn ResponseWriter, req: &mut Request) {
        let root = self.tree(&req.method);
        let tsr = if let Some(root) = root {
            let found = root.get_value(&req.url.path);
            if let Some(handle) = found.handle {
                return handle.handle(w, req, found.params).await;
            }
            found.tsr
        } else {
            false
        };
        let path = req.url.path.clone();
        if let Some(root) = root
            && req.method != "CONNECT"
            && path != "/"
        {
            let code = if req.method == "GET" { 301 } else { 307 };
            if tsr && self.redirect.trailing_slash {
                req.url.path = if path.len() > 1 && path.ends_with('/') {
                    path.get(..path.len() - 1).unwrap_or_default().to_owned()
                } else {
                    format!("{path}/")
                };
                let target = req.url.to_string();
                return redirect(w, req, &target, code).await;
            }
            if self.redirect.fixed_path
                && let Some(fixed) = root
                    .find_case_insensitive_path(&clean_path(&path), self.redirect.trailing_slash)
            {
                req.url.path = String::from_utf8_lossy(&fixed).into_owned();
                let target = req.url.to_string();
                return redirect(w, req, &target, code).await;
            }
        }
        if req.method == "OPTIONS" && self.handle_options {
            let allow = self.allowed(&path, "OPTIONS");
            if !allow.is_empty() {
                w.header().set("Allow", allow);
                if let Some(h) = &self.global_options {
                    h.serve_http(w, req).await;
                }
                return;
            }
        } else if self.handle_method_not_allowed {
            let allow = self.allowed(&path, &req.method);
            if !allow.is_empty() {
                w.header().set("Allow", allow);
                if let Some(h) = &self.method_not_allowed {
                    h.serve_http(w, req).await;
                } else {
                    error(
                        w,
                        status_text(STATUS_METHOD_NOT_ALLOWED),
                        STATUS_METHOD_NOT_ALLOWED,
                    )
                    .await;
                }
                return;
            }
        }
        if let Some(h) = &self.not_found {
            h.serve_http(w, req).await;
        } else {
            not_found(w).await;
        }
    }
}

impl Handler for Router {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let Some(ph) = &self.panic_handler else {
                return self.route(w, r).await;
            };
            let outcome = CatchUnwind::new(Box::pin(self.route(w, r))).await;
            if let Err(payload) = outcome {
                ph(w, r, payload).await;
            }
        })
    }
}
