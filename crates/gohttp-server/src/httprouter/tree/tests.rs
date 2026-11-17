//! Go: `httprouter/tree_test.go`, case for case.
//!
//! Go tells handles apart by calling them (`fakeHandler` stores its route in a global); here each
//! route keeps the `Arc` it was registered with and lookups compare against it. Go's
//! `TestTreeInvalidNodeType` has no counterpart: [`NodeType`] cannot hold an invalid value.

use std::sync::Arc;

use super::{Node, NodeType, count_params, decode_rune, equal_fold};
use crate::{
    BoxFuture, Request, ResponseWriter,
    httprouter::{Handle, Param, Params, RouteError},
};

/// Go: `fakeHandler(val)`.
#[derive(Debug)]
struct Fake;

impl Handle for Fake {
    fn handle<'a>(
        &'a self,
        _w: &'a mut dyn ResponseWriter,
        _r: &'a mut Request,
        _ps: Params,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

/// A tree and the handle each route was registered with.
#[derive(Default)]
struct Tree {
    root: Node,
    handles: Vec<(&'static str, Arc<dyn Handle>)>,
}

impl Tree {
    /// A tree holding `routes`, failing as Go's `t.Fatalf` on a panic.
    fn with(routes: &[&'static str]) -> Result<Self, String> {
        let mut t = Self::default();
        for &route in routes {
            t.add(route)
                .map_err(|e| format!("panic inserting route '{route}': {e}"))?;
        }
        Ok(t)
    }

    /// Go: `tree.addRoute(route, fakeHandler(route))`.
    fn add(&mut self, route: &'static str) -> Result<(), RouteError> {
        let handle: Arc<dyn Handle> = Arc::new(Fake);
        self.handles.push((route, Arc::clone(&handle)));
        self.root.add_route(route.as_bytes(), handle)
    }
}

/// Go: one of `testRequests`: the path, whether no handle is expected, the route whose handle
/// is expected, and the expected parameters.
type TestRequest<'a> = (&'a str, bool, &'a str, &'a [(&'a str, &'a str)]);

fn params(ps: &[(&str, &str)]) -> Params {
    Params(
        ps.iter()
            .map(|&(key, value)| Param {
                key: key.to_owned(),
                value: value.to_owned(),
            })
            .collect(),
    )
}

/// Go: `checkRequests`.
fn check_requests(t: &Tree, requests: &[TestRequest<'_>]) -> Result<(), String> {
    for &(path, nil_handler, route, ps) in requests {
        let found = t.root.get_value(path);
        match (found.handle, nil_handler) {
            (None, false) => {
                return Err(format!(
                    "handle mismatch for route '{path}': Expected non-nil handle"
                ));
            }
            (Some(_), true) => {
                return Err(format!(
                    "handle mismatch for route '{path}': Expected nil handle"
                ));
            }
            (Some(h), false) => {
                let registered = t.handles.iter().find(|(r, _)| *r == route);
                if !registered.is_some_and(|(_, want)| Arc::ptr_eq(want, h)) {
                    return Err(format!(
                        "handle mismatch for route '{path}': Wrong handle (want {route})"
                    ));
                }
            }
            (None, true) => {}
        }
        if found.params != params(ps) {
            return Err(format!(
                "Params mismatch for route '{path}': {:?}",
                found.params
            ));
        }
    }
    Ok(())
}

/// Go: `checkPriorities`.
fn check_priorities(n: &Node) -> Result<u32, String> {
    let mut prio = 0;
    for c in &n.children {
        prio += check_priorities(c)?;
    }
    if n.handle.is_some() {
        prio += 1;
    }
    if n.priority != prio {
        return Err(format!(
            "priority mismatch for node '{}': is {}, should be {prio}",
            String::from_utf8_lossy(&n.path),
            n.priority
        ));
    }
    Ok(prio)
}

/// Go: `checkMaxParams`.
fn check_max_params(n: &Node) -> Result<u8, String> {
    let mut max_params = 0;
    for c in &n.children {
        max_params = max_params.max(check_max_params(c)?);
    }
    if matches!(n.n_type, NodeType::Param | NodeType::CatchAll) && !n.wild_child {
        max_params += 1;
    }
    if n.max_params != max_params {
        return Err(format!(
            "maxParams mismatch for node '{}': is {}, should be {max_params}",
            String::from_utf8_lossy(&n.path),
            n.max_params
        ));
    }
    Ok(max_params)
}

/// Go: `testRoutes`: registers the routes in order on one tree; exactly the conflicting ones
/// must fail.
fn test_routes(routes: &[(&'static str, bool)]) -> Result<(), String> {
    let mut t = Tree::default();
    for &(path, conflict) in routes {
        match (t.add(path), conflict) {
            (Ok(()), true) => return Err(format!("no panic for conflicting route '{path}'")),
            (Err(e), false) => return Err(format!("unexpected panic for route '{path}': {e}")),
            (Ok(()), false) | (Err(_), true) => {}
        }
    }
    Ok(())
}

#[test]
fn equal_fold_like_go() -> Result<(), String> {
    for (s, t, want) in [
        (&b"abc"[..], &b"ABC"[..], true),
        (b"\xc3\xa9", b"\xc3\x89", true),
        (b"\xa9", b"\xa8", true),
        (b"a", b"ab", false),
        (b"k", "\u{212a}".as_bytes(), true),
    ] {
        if equal_fold(s, t) != want {
            return Err(format!("EqualFold({s:?}, {t:?}) != {want}"));
        }
    }
    if decode_rune(b"\xe2\x82") != (char::REPLACEMENT_CHARACTER, 1) {
        return Err("truncated sequence".to_owned());
    }
    Ok(())
}

#[test]
fn count_params_like_go() -> Result<(), String> {
    // Go: TestCountParams.
    if count_params(b"/path/:param1/static/*catch-all") != 2 {
        return Err("countParams(/path/:param1/static/*catch-all) != 2".to_owned());
    }
    if count_params("/:param".repeat(256).as_bytes()) != 255 {
        return Err("countParams of 256 parameters != 255".to_owned());
    }
    Ok(())
}

#[test]
fn tree_add_and_get() -> Result<(), String> {
    // Go: TestTreeAddAndGet.
    let t = Tree::with(&[
        "/hi",
        "/contact",
        "/co",
        "/c",
        "/a",
        "/ab",
        "/doc/",
        "/doc/go_faq.html",
        "/doc/go1.html",
        "/α",
        "/β",
    ])?;
    check_requests(
        &t,
        &[
            ("/a", false, "/a", &[]),
            ("/", true, "", &[]),
            ("/hi", false, "/hi", &[]),
            ("/contact", false, "/contact", &[]),
            ("/co", false, "/co", &[]),
            ("/con", true, "", &[]),  // key mismatch
            ("/cona", true, "", &[]), // key mismatch
            ("/no", true, "", &[]),   // no matching child
            ("/ab", false, "/ab", &[]),
            ("/α", false, "/α", &[]),
            ("/β", false, "/β", &[]),
        ],
    )?;
    let _prio = check_priorities(&t.root)?;
    let _max = check_max_params(&t.root)?;
    Ok(())
}

#[test]
fn tree_wildcard() -> Result<(), String> {
    // Go: TestTreeWildcard.
    let t = Tree::with(&[
        "/",
        "/cmd/:tool/:sub",
        "/cmd/:tool/",
        "/src/*filepath",
        "/search/",
        "/search/:query",
        "/user_:name",
        "/user_:name/about",
        "/files/:dir/*filepath",
        "/doc/",
        "/doc/go_faq.html",
        "/doc/go1.html",
        "/info/:user/public",
        "/info/:user/project/:project",
    ])?;
    check_requests(
        &t,
        &[
            ("/", false, "/", &[]),
            ("/cmd/test/", false, "/cmd/:tool/", &[("tool", "test")]),
            ("/cmd/test", true, "", &[("tool", "test")]),
            (
                "/cmd/test/3",
                false,
                "/cmd/:tool/:sub",
                &[("tool", "test"), ("sub", "3")],
            ),
            ("/src/", false, "/src/*filepath", &[("filepath", "/")]),
            (
                "/src/some/file.png",
                false,
                "/src/*filepath",
                &[("filepath", "/some/file.png")],
            ),
            ("/search/", false, "/search/", &[]),
            (
                "/search/someth!ng+in+ünìcodé",
                false,
                "/search/:query",
                &[("query", "someth!ng+in+ünìcodé")],
            ),
            (
                "/search/someth!ng+in+ünìcodé/",
                true,
                "",
                &[("query", "someth!ng+in+ünìcodé")],
            ),
            ("/user_gopher", false, "/user_:name", &[("name", "gopher")]),
            (
                "/user_gopher/about",
                false,
                "/user_:name/about",
                &[("name", "gopher")],
            ),
            (
                "/files/js/inc/framework.js",
                false,
                "/files/:dir/*filepath",
                &[("dir", "js"), ("filepath", "/inc/framework.js")],
            ),
            (
                "/info/gordon/public",
                false,
                "/info/:user/public",
                &[("user", "gordon")],
            ),
            (
                "/info/gordon/project/go",
                false,
                "/info/:user/project/:project",
                &[("user", "gordon"), ("project", "go")],
            ),
        ],
    )?;
    let _prio = check_priorities(&t.root)?;
    let _max = check_max_params(&t.root)?;
    Ok(())
}

#[test]
fn tree_wildcard_conflict() -> Result<(), String> {
    // Go: TestTreeWildcardConflict. Go registers these routes with nil handles, so its second
    // "/user_:name" only overwrites a nil handle; handles here are never nil, so that
    // registration is the duplicate it is.
    test_routes(&[
        ("/cmd/:tool/:sub", false),
        ("/cmd/vet", true),
        ("/src/*filepath", false),
        ("/src/*filepathx", true),
        ("/src/", true),
        ("/src1/", false),
        ("/src1/*filepath", true),
        ("/src2*filepath", true),
        ("/search/:query", false),
        ("/search/invalid", true),
        ("/user_:name", false),
        ("/user_x", true),
        ("/user_:name", true),
        ("/id:id", false),
        ("/id/:id", true),
    ])
}

#[test]
fn tree_child_conflict() -> Result<(), String> {
    // Go: TestTreeChildConflict.
    test_routes(&[
        ("/cmd/vet", false),
        ("/cmd/:tool/:sub", true),
        ("/src/AUTHORS", false),
        ("/src/*filepath", true),
        ("/user_x", false),
        ("/user_:name", true),
        ("/id/:id", false),
        ("/id:id", true),
        ("/:id", true),
        ("/*filepath", true),
    ])
}

#[test]
fn tree_duplicate_path() -> Result<(), String> {
    // Go: TestTreeDupliatePath.
    let mut t = Tree::default();
    for route in [
        "/",
        "/doc/",
        "/src/*filepath",
        "/search/:query",
        "/user_:name",
    ] {
        t.add(route)
            .map_err(|e| format!("panic inserting route '{route}': {e}"))?;
        // Add again
        if t.add(route).is_ok() {
            return Err(format!("no panic while inserting duplicate route '{route}"));
        }
    }
    check_requests(
        &t,
        &[
            ("/", false, "/", &[]),
            ("/doc/", false, "/doc/", &[]),
            (
                "/src/some/file.png",
                false,
                "/src/*filepath",
                &[("filepath", "/some/file.png")],
            ),
            (
                "/search/someth!ng+in+ünìcodé",
                false,
                "/search/:query",
                &[("query", "someth!ng+in+ünìcodé")],
            ),
            ("/user_gopher", false, "/user_:name", &[("name", "gopher")]),
        ],
    )
}

#[test]
fn empty_wildcard_name() -> Result<(), String> {
    // Go: TestEmptyWildcardName.
    let mut t = Tree::default();
    for route in ["/user:", "/user:/", "/cmd/:/", "/src/*"] {
        match t.add(route) {
            Ok(()) => {
                return Err(format!(
                    "no panic while inserting route with empty wildcard name '{route}"
                ));
            }
            Err(RouteError(msg)) => {
                let want =
                    format!("wildcards must be named with a non-empty name in path '{route}'");
                if msg != want {
                    return Err(msg);
                }
            }
        }
    }
    Ok(())
}

#[test]
fn tree_catch_all_conflict() -> Result<(), String> {
    // Go: TestTreeCatchAllConflict.
    test_routes(&[
        ("/src/*filepath/x", true),
        ("/src2/", false),
        ("/src2/*filepath/x", true),
        ("/src3/*filepath", false),
        ("/src3/*filepath/x", true),
    ])
}

#[test]
fn tree_catch_all_conflict_root() -> Result<(), String> {
    // Go: TestTreeCatchAllConflictRoot.
    test_routes(&[("/", false), ("/*filepath", true)])
}

#[test]
fn tree_catch_max_params() -> Result<(), String> {
    // Go: TestTreeCatchMaxParams.
    let t = Tree::with(&["/cmd/*filepath"])?;
    let _max = check_max_params(&t.root)?;
    Ok(())
}

#[test]
fn tree_double_wildcard() -> Result<(), String> {
    // Go: TestTreeDoubleWildcard.
    const PANIC_MSG: &str = "only one wildcard per path segment is allowed";
    for route in ["/:foo:bar", "/:foo:bar/", "/:foo*bar"] {
        let mut t = Tree::default();
        match t.add(route) {
            Err(RouteError(msg)) if msg.starts_with(PANIC_MSG) => {}
            other => {
                return Err(format!(
                    "Expected panic \"{PANIC_MSG}\" for route '{route}', got \"{other:?}\""
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn tree_trailing_slash_redirect() -> Result<(), String> {
    // Go: TestTreeTrailingSlashRedirect.
    let t = Tree::with(&[
        "/hi",
        "/b/",
        "/search/:query",
        "/cmd/:tool/",
        "/src/*filepath",
        "/x",
        "/x/y",
        "/y/",
        "/y/z",
        "/0/:id",
        "/0/:id/1",
        "/1/:id/",
        "/1/:id/2",
        "/aa",
        "/a/",
        "/admin",
        "/admin/:category",
        "/admin/:category/:page",
        "/doc",
        "/doc/go_faq.html",
        "/doc/go1.html",
        "/no/a",
        "/no/b",
        "/api/hello/:name",
    ])?;
    for route in [
        "/hi/",
        "/b",
        "/search/gopher/",
        "/cmd/vet",
        "/src",
        "/x/",
        "/y",
        "/0/go/",
        "/1/go",
        "/a",
        "/admin/",
        "/admin/config/",
        "/admin/config/permissions/",
        "/doc/",
    ] {
        let found = t.root.get_value(route);
        if found.handle.is_some() {
            return Err(format!("non-nil handler for TSR route '{route}"));
        }
        if !found.tsr {
            return Err(format!("expected TSR recommendation for route '{route}'"));
        }
    }
    for route in ["/", "/no", "/no/", "/_", "/_/", "/api/world/abc"] {
        let found = t.root.get_value(route);
        if found.handle.is_some() {
            return Err(format!("non-nil handler for No-TSR route '{route}"));
        }
        if found.tsr {
            return Err(format!(
                "expected no TSR recommendation for route '{route}'"
            ));
        }
    }
    Ok(())
}

#[test]
fn tree_root_trailing_slash_redirect() -> Result<(), String> {
    // Go: TestTreeRootTrailingSlashRedirect.
    let t = Tree::with(&["/:test"])?;
    let found = t.root.get_value("/");
    if found.handle.is_some() {
        return Err("non-nil handler".to_owned());
    }
    if found.tsr {
        return Err("expected no TSR recommendation".to_owned());
    }
    Ok(())
}

#[test]
fn tree_find_case_insensitive_path() -> Result<(), String> {
    // Go: TestTreeFindCaseInsensitivePath.
    let routes = [
        "/hi",
        "/b/",
        "/ABC/",
        "/search/:query",
        "/cmd/:tool/",
        "/src/*filepath",
        "/x",
        "/x/y",
        "/y/",
        "/y/z",
        "/0/:id",
        "/0/:id/1",
        "/1/:id/",
        "/1/:id/2",
        "/aa",
        "/a/",
        "/doc",
        "/doc/go_faq.html",
        "/doc/go1.html",
        "/doc/go/away",
        "/no/a",
        "/no/b",
        "/Π",
        "/u/apfêl/",
        "/u/äpfêl/",
        "/u/öpfêl",
        "/v/Äpfêl/",
        "/v/Öpfêl",
        "/w/♬",   // 3 byte
        "/w/♭/",  // 3 byte, last byte differs
        "/w/𠜎",  // 4 byte
        "/w/𠜏/", // 4 byte
    ];
    let t = Tree::with(&routes)?;

    // Check out == in for all registered routes, with and without fixTrailingSlash.
    for fix_trailing_slash in [true, false] {
        for route in routes {
            match t.root.find_case_insensitive_path(route, fix_trailing_slash) {
                None => return Err(format!("Route '{route}' not found!")),
                Some(out) if out != route.as_bytes() => {
                    return Err(format!(
                        "Wrong result for route '{route}': {}",
                        String::from_utf8_lossy(&out)
                    ));
                }
                Some(_) => {}
            }
        }
    }

    // in, out, found, slash
    let tests = [
        ("/HI", "/hi", true, false),
        ("/HI/", "/hi", true, true),
        ("/B", "/b/", true, true),
        ("/B/", "/b/", true, false),
        ("/abc", "/ABC/", true, true),
        ("/abc/", "/ABC/", true, false),
        ("/aBc", "/ABC/", true, true),
        ("/aBc/", "/ABC/", true, false),
        ("/abC", "/ABC/", true, true),
        ("/abC/", "/ABC/", true, false),
        ("/SEARCH/QUERY", "/search/QUERY", true, false),
        ("/SEARCH/QUERY/", "/search/QUERY", true, true),
        ("/CMD/TOOL/", "/cmd/TOOL/", true, false),
        ("/CMD/TOOL", "/cmd/TOOL/", true, true),
        ("/SRC/FILE/PATH", "/src/FILE/PATH", true, false),
        ("/x/Y", "/x/y", true, false),
        ("/x/Y/", "/x/y", true, true),
        ("/X/y", "/x/y", true, false),
        ("/X/y/", "/x/y", true, true),
        ("/X/Y", "/x/y", true, false),
        ("/X/Y/", "/x/y", true, true),
        ("/Y/", "/y/", true, false),
        ("/Y", "/y/", true, true),
        ("/Y/z", "/y/z", true, false),
        ("/Y/z/", "/y/z", true, true),
        ("/Y/Z", "/y/z", true, false),
        ("/Y/Z/", "/y/z", true, true),
        ("/y/Z", "/y/z", true, false),
        ("/y/Z/", "/y/z", true, true),
        ("/Aa", "/aa", true, false),
        ("/Aa/", "/aa", true, true),
        ("/AA", "/aa", true, false),
        ("/AA/", "/aa", true, true),
        ("/aA", "/aa", true, false),
        ("/aA/", "/aa", true, true),
        ("/A/", "/a/", true, false),
        ("/A", "/a/", true, true),
        ("/DOC", "/doc", true, false),
        ("/DOC/", "/doc", true, true),
        ("/NO", "", false, true),
        ("/DOC/GO", "", false, true),
        ("/π", "/Π", true, false),
        ("/π/", "/Π", true, true),
        ("/u/ÄPFÊL/", "/u/äpfêl/", true, false),
        ("/u/ÄPFÊL", "/u/äpfêl/", true, true),
        ("/u/ÖPFÊL/", "/u/öpfêl", true, true),
        ("/u/ÖPFÊL", "/u/öpfêl", true, false),
        ("/v/äpfêL/", "/v/Äpfêl/", true, false),
        ("/v/äpfêL", "/v/Äpfêl/", true, true),
        ("/v/öpfêL/", "/v/Öpfêl", true, true),
        ("/v/öpfêL", "/v/Öpfêl", true, false),
        ("/w/♬/", "/w/♬", true, true),
        ("/w/♭", "/w/♭/", true, true),
        ("/w/𠜎/", "/w/𠜎", true, true),
        ("/w/𠜏", "/w/𠜏/", true, true),
    ];
    // With fixTrailingSlash = true
    for (input, want, want_found, _) in tests {
        let out = t.root.find_case_insensitive_path(input, true);
        if out.is_some() != want_found || out.as_ref().is_some_and(|o| o != want.as_bytes()) {
            return Err(format!(
                "Wrong result for '{input}': got {out:?}; want {want}, {want_found}"
            ));
        }
    }
    // With fixTrailingSlash = false
    for (input, want, want_found, slash) in tests {
        let out = t.root.find_case_insensitive_path(input, false);
        if slash {
            // test needs a trailingSlash fix. It must not be found!
            if let Some(o) = out {
                return Err(format!(
                    "Found without fixTrailingSlash: {input}; got {}",
                    String::from_utf8_lossy(&o)
                ));
            }
        } else if out.is_some() != want_found || out.as_ref().is_some_and(|o| o != want.as_bytes())
        {
            return Err(format!(
                "Wrong result for '{input}': got {out:?}; want {want}, {want_found}"
            ));
        }
    }
    Ok(())
}

#[test]
fn tree_wildcard_conflict_ex() -> Result<(), String> {
    // Go: TestTreeWildcardConflictEx. Go matches the panic against a pattern with `.*` for the
    // new path; the text is compared whole here.
    let conflicts = [
        ("/who/are/foo", "/foo", "/who/are/*you", "/*you"),
        ("/who/are/foo/", "/foo/", "/who/are/*you", "/*you"),
        ("/who/are/foo/bar", "/foo/bar", "/who/are/*you", "/*you"),
        ("/conxxx", "xxx", "/con:tact", ":tact"),
        ("/conooo/xxx", "ooo", "/con:tact", ":tact"),
    ];
    for (route, seg_path, exist_path, exist_seg_path) in conflicts {
        // A new tree each time: a failed registration leaves the tree as Go's recovered panic
        // does.
        let mut t = Tree::with(&["/con:tact", "/who/are/*you", "/who/foo/hello"])?;
        let want = format!(
            "'{seg_path}' in new path '{route}' conflicts with existing wildcard \
             '{exist_seg_path}' in existing prefix '{exist_path}'"
        );
        match t.add(route) {
            Err(RouteError(msg)) if msg == want => {}
            other => return Err(format!("invalid wildcard conflict error ({other:?})")),
        }
    }
    Ok(())
}
