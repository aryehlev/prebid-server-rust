//! Go: `http.ServeMux` (Go 1.22+ routing, `httpmuxgo121=0`) for the patterns Prebid's admin
//! server registers: literal paths, with a trailing slash matching the whole subtree. Patterns
//! with a method, a host or wildcards are rejected at registration.

use core::{error::Error, fmt};
use std::{collections::BTreeMap, sync::Arc};

use pbs_gocompat::{
    strconv::quote,
    url::{Url, path_unescape},
};

use super::{
    BoxFuture,
    helpers::{RedirectHandler, clean_path as path_clean, not_found},
    request::Request,
    response::{Handler, ResponseWriter},
    status::{STATUS_BAD_REQUEST, STATUS_TEMPORARY_REDIRECT},
};

/// A pattern [`ServeMux::handle`] rejects, with Go's panic text where Go has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError(pub String);

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for PatternError {}

/// Go: `routingNode` (literal and multi children only).
#[derive(Default)]
struct Node {
    /// The pattern string and handler of a leaf.
    leaf: Option<(String, Arc<dyn Handler>, usize, bool)>,
    children: BTreeMap<String, Self>,
    multi: Option<Box<Self>>,
}

impl Node {
    /// Go: `routingNode.matchPath` for literal and trailing-slash patterns.
    fn match_path(&self, path: &str) -> Option<&Self> {
        if path.is_empty() {
            return self.leaf.as_ref().map(|_| self);
        }
        let (seg, rest) = first_segment(path);
        if let Some(n) = self.children.get(&seg).and_then(|c| c.match_path(rest)) {
            return Some(n);
        }
        self.multi.as_deref()
    }
}

/// Go: `firstSegment`.
fn first_segment(path: &str) -> (String, &str) {
    if path == "/" {
        return ("/".to_owned(), "");
    }
    let path = path.get(1..).unwrap_or_default();
    let i = path.find('/').unwrap_or(path.len());
    let (seg, rest) = path.split_at(i);
    (path_unescape(seg).unwrap_or_else(|_| seg.to_owned()), rest)
}

/// Go: `exactMatch`.
fn exact_match(n: Option<&Node>, path: &str) -> bool {
    let Some((_, _, segments, multi)) = n.and_then(|n| n.leaf.as_ref()) else {
        return false;
    };
    if !multi {
        return true;
    }
    if !path.is_empty() && !path.ends_with('/') {
        return false;
    }
    *segments == path.matches('/').count()
}

/// Go: `cleanPath` (the `ServeMux` one, which keeps a trailing slash).
fn clean_mux_path(p: &str) -> String {
    if p.is_empty() {
        return "/".to_owned();
    }
    let p = if p.starts_with('/') {
        p.to_owned()
    } else {
        format!("/{p}")
    };
    let mut np = path_clean(&p);
    if p.ends_with('/') && np != "/" {
        if p.len() == np.len() + 1 && p.starts_with(&np) {
            np = p;
        } else {
            np.push('/');
        }
    }
    np
}

/// Go: `*http.ServeMux` with literal patterns.
#[derive(Default)]
pub struct ServeMux {
    root: Node,
}

impl fmt::Debug for ServeMux {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ServeMux")
    }
}

impl ServeMux {
    /// Go: `http.NewServeMux()`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Go: `mux.Handle(pattern, handler)` for a literal pattern such as `/version` or
    /// `/debug/pprof/`.
    ///
    /// # Errors
    ///
    /// Go's panic text for an invalid or duplicate pattern; patterns with a method, host or
    /// wildcard are not supported.
    pub fn handle(&mut self, pattern: &str, handler: Arc<dyn Handler>) -> Result<(), PatternError> {
        if pattern.is_empty() {
            return Err(PatternError("http: invalid pattern".to_owned()));
        }
        if !pattern.starts_with('/') || pattern.contains(['{', '}', ' ', '\t']) {
            return Err(PatternError(format!(
                "parsing {}: only literal paths are supported",
                quote(pattern)
            )));
        }
        let mut node = &mut self.root;
        let mut rest = pattern;
        let mut segments = 0;
        let mut multi = false;
        while let Some(after) = rest.strip_prefix('/') {
            if after.is_empty() {
                multi = true;
                segments += 1;
                node = node.multi.get_or_insert_with(Box::default);
                break;
            }
            let i = after.find('/').unwrap_or(after.len());
            let (seg, tail) = after.split_at(i);
            let seg = path_unescape(seg).unwrap_or_else(|_| seg.to_owned());
            segments += 1;
            node = node.children.entry(seg).or_default();
            rest = tail;
        }
        if let Some((existing, ..)) = &node.leaf {
            return Err(PatternError(format!(
                "pattern {} conflicts with pattern {}: {pattern} matches the same \
                 requests as {existing}",
                quote(pattern),
                quote(existing),
            )));
        }
        node.leaf = Some((pattern.to_owned(), handler, segments, multi));
        Ok(())
    }

    /// Go: `mux.matchOrRedirect`.
    fn match_or_redirect(&self, path: &str, u: Option<&Url>) -> (Option<&Node>, Option<Url>) {
        let n = self.root.match_path(path);
        if !exact_match(n, path)
            && let Some(u) = u
            && !path.ends_with('/')
            && !path.is_empty()
        {
            let with_slash = format!("{path}/");
            let n2 = self.root.match_path(&with_slash);
            if exact_match(n2, &with_slash) {
                let mut target = clean_mux_path(&u.path);
                target.push('/');
                return (
                    n2,
                    Some(Url {
                        path: target,
                        raw_query: u.raw_query.clone(),
                        ..Url::default()
                    }),
                );
            }
        }
        (n, None)
    }

    /// Go: `mux.findHandler`: the handler for `r`, or a redirect or not-found handler.
    fn find_handler(&self, r: &Request) -> Match<'_> {
        let escaped = r.url.escaped_path();
        if r.method == "CONNECT" {
            let (_, u) = self.match_or_redirect(&escaped, Some(&r.url));
            if let Some(u) = u {
                return Match::Redirect(u.to_string());
            }
            let (n, _) = self.match_or_redirect(&escaped, None);
            return n
                .and_then(|n| n.leaf.as_ref())
                .map_or(Match::NotFound, |(_, h, ..)| Match::Handler(h.as_ref()));
        }
        let path = clean_mux_path(&escaped);
        let (n, u) = self.match_or_redirect(&path, Some(&r.url));
        if let Some(u) = u {
            return Match::Redirect(u.to_string());
        }
        if path != escaped {
            let target = Url {
                path,
                raw_query: r.url.raw_query.clone(),
                ..Url::default()
            };
            return Match::Redirect(target.to_string());
        }
        n.and_then(|n| n.leaf.as_ref())
            .map_or(Match::NotFound, |(_, h, ..)| Match::Handler(h.as_ref()))
    }
}

enum Match<'a> {
    Handler(&'a dyn Handler),
    Redirect(String),
    NotFound,
}

impl Handler for ServeMux {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if r.request_uri == "*" {
                if r.proto_at_least(1, 1) {
                    w.header().set("Connection", "close");
                }
                w.write_header(STATUS_BAD_REQUEST);
                return;
            }
            match self.find_handler(r) {
                Match::Handler(h) => h.serve_http(w, r).await,
                Match::Redirect(url) => {
                    RedirectHandler::new(url, STATUS_TEMPORARY_REDIRECT)
                        .serve_http(w, r)
                        .await;
                }
                Match::NotFound => not_found(w).await,
            }
        })
    }
}
