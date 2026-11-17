//! Go: `httprouter/tree.go`, the radix tree of one method's routes. Paths are handled as bytes,
//! as Go handles strings, so nodes may split inside a multi-byte character exactly as Go's do.

use core::{mem, str};
use std::sync::Arc;

use pbs_gocompat::unicode::{simple_fold, to_lower, to_upper};

use super::{Handle, Param, Params, RouteError};

/// Go: `maxParamCount`.
const MAX_PARAM_COUNT: u8 = u8::MAX;

/// Go: `countParams`.
fn count_params(path: &[u8]) -> u8 {
    let n = path.iter().filter(|&&c| c == b':' || c == b'*').count();
    u8::try_from(n).unwrap_or(MAX_PARAM_COUNT)
}

/// Go: `nodeType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum NodeType {
    #[default]
    Static,
    Root,
    Param,
    CatchAll,
}

/// Go: `node`.
#[derive(Default)]
pub(super) struct Node {
    pub(super) path: Vec<u8>,
    pub(super) wild_child: bool,
    pub(super) n_type: NodeType,
    pub(super) max_params: u8,
    pub(super) priority: u32,
    pub(super) indices: Vec<u8>,
    pub(super) children: Vec<Self>,
    pub(super) handle: Option<Arc<dyn Handle>>,
}

/// Go's text for a runtime index panic.
fn index_error(i: usize, len: usize) -> RouteError {
    RouteError(format!(
        "runtime error: index out of range [{i}] with length {len}"
    ))
}

/// Go's `string(b)` for messages built from path bytes.
fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Go: `strings.Index` over bytes.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The child at `i`, or Go's index panic.
fn child_mut(n: &mut Node, i: usize) -> Result<&mut Node, RouteError> {
    let len = n.children.len();
    n.children.get_mut(i).ok_or_else(|| index_error(i, len))
}

/// Go: `utf8.RuneStart`.
const fn rune_start(b: u8) -> bool {
    b & 0xC0 != 0x80
}

/// Go: `utf8.DecodeRune`: the first rune of `b` and its width, `(U+FFFD, 1)` for an invalid
/// encoding.
fn decode_rune(b: &[u8]) -> (char, usize) {
    let Some(&first) = b.first() else {
        return (char::REPLACEMENT_CHARACTER, 0);
    };
    let width = match first {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        0x80..=0xC1 | 0xF5..=0xFF => return (char::REPLACEMENT_CHARACTER, 1),
    };
    b.get(..width)
        .and_then(|s| str::from_utf8(s).ok())
        .and_then(|s| s.chars().next())
        .map_or((char::REPLACEMENT_CHARACTER, 1), |c| (c, width))
}

/// Go: `strings.EqualFold` over bytes (invalid bytes decode as U+FFFD, one byte each).
fn equal_fold(s: &[u8], t: &[u8]) -> bool {
    let (mut s, mut t) = (s, t);
    loop {
        match (s.is_empty(), t.is_empty()) {
            (true, true) => return true,
            (true, false) | (false, true) => return false,
            (false, false) => {}
        }
        let (sr, sw) = decode_rune(s);
        let (tr, tw) = decode_rune(t);
        s = s.get(sw..).unwrap_or_default();
        t = t.get(tw..).unwrap_or_default();
        if sr == tr {
            continue;
        }
        let (sr, tr) = if tr < sr { (tr, sr) } else { (sr, tr) };
        if tr.is_ascii() {
            if sr.is_ascii_uppercase() && sr.to_ascii_lowercase() == tr {
                continue;
            }
            return false;
        }
        let mut r = simple_fold(sr);
        while r != sr && r < tr {
            r = simple_fold(r);
        }
        if r != tr {
            return false;
        }
    }
}

/// Go: `shiftNRuneBytes`.
const fn shift_n_rune_bytes(rb: [u8; 4], n: usize) -> [u8; 4] {
    match n {
        0 => rb,
        1 => [rb[1], rb[2], rb[3], 0],
        2 => [rb[2], rb[3], 0, 0],
        3 => [rb[3], 0, 0, 0],
        _ => [0; 4],
    }
}

/// Go: `utf8.EncodeRune(rb[:], r)`: overwrites the first bytes of `rb`.
fn encode_rune(rb: &mut [u8; 4], r: char) {
    let mut tmp = [0_u8; 4];
    let enc = r.encode_utf8(&mut tmp).as_bytes();
    for (dst, src) in rb.iter_mut().zip(enc) {
        *dst = *src;
    }
}

/// What [`Node::get_value`] found.
pub(super) struct Lookup<'a> {
    pub(super) handle: Option<&'a Arc<dyn Handle>>,
    pub(super) params: Params,
    pub(super) tsr: bool,
}

impl Node {
    /// Go: `incrementChildPrio`.
    fn increment_child_prio(&mut self, pos: usize) -> usize {
        let Some(child) = self.children.get_mut(pos) else {
            return pos;
        };
        child.priority = child.priority.wrapping_add(1);
        let prio = child.priority;
        let mut new_pos = pos;
        while new_pos > 0
            && self
                .children
                .get(new_pos - 1)
                .is_some_and(|c| c.priority < prio)
        {
            self.children.swap(new_pos - 1, new_pos);
            new_pos -= 1;
        }
        if new_pos != pos && pos < self.indices.len() {
            let c = self.indices.remove(pos);
            self.indices.insert(new_pos, c);
        }
        new_pos
    }

    /// Go: the edge split of `addRoute`: this node keeps `path[..i]` and gets one child with
    /// the rest of its path, children and handle.
    fn split_edge(&mut self, i: usize) {
        let mut child = Self {
            path: self.path.split_off(i),
            wild_child: self.wild_child,
            n_type: NodeType::Static,
            max_params: 0,
            priority: self.priority.wrapping_sub(1),
            indices: mem::take(&mut self.indices),
            children: mem::take(&mut self.children),
            handle: self.handle.take(),
        };
        for c in &child.children {
            if c.max_params > child.max_params {
                child.max_params = c.max_params;
            }
        }
        self.indices = child.path.first().map_or_else(Vec::new, |&c| vec![c]);
        self.children = vec![child];
        self.wild_child = false;
    }

    /// Go: `addRoute`. On an error the tree is left as Go leaves it when the corresponding
    /// panic unwinds.
    pub(super) fn add_route(
        &mut self,
        full_path: &[u8],
        handle: Arc<dyn Handle>,
    ) -> Result<(), RouteError> {
        let mut path = full_path;
        self.priority = self.priority.wrapping_add(1);
        let mut num_params = count_params(path);
        if self.path.is_empty() && self.children.is_empty() {
            self.insert_child(num_params, path, full_path, handle)?;
            self.n_type = NodeType::Root;
            return Ok(());
        }
        let mut n = self;
        loop {
            if num_params > n.max_params {
                n.max_params = num_params;
            }
            let max = path.len().min(n.path.len());
            let mut i = 0;
            while i < max && path.get(i) == n.path.get(i) {
                i += 1;
            }
            if i < n.path.len() {
                n.split_edge(i);
            }
            if i >= path.len() {
                if n.handle.is_some() {
                    return Err(RouteError(format!(
                        "a handle is already registered for path '{}'",
                        text(full_path)
                    )));
                }
                n.handle = Some(handle);
                return Ok(());
            }
            path = path.get(i..).unwrap_or_default();
            if n.wild_child {
                n = child_mut(n, 0)?;
                n.priority = n.priority.wrapping_add(1);
                if num_params > n.max_params {
                    n.max_params = num_params;
                }
                num_params = num_params.wrapping_sub(1);
                let np = n.path.len();
                if path.len() >= np
                    && path.get(..np) == Some(n.path.as_slice())
                    && n.n_type != NodeType::CatchAll
                    && (np >= path.len() || path.get(np) == Some(&b'/'))
                {
                    continue;
                }
                let path_seg = if n.n_type == NodeType::CatchAll {
                    path
                } else {
                    path.split(|&c| c == b'/').next().unwrap_or_default()
                };
                let at = find(full_path, path_seg).unwrap_or(0);
                let mut prefix = full_path.get(..at).unwrap_or_default().to_vec();
                prefix.extend_from_slice(&n.path);
                return Err(RouteError(format!(
                    "'{}' in new path '{}' conflicts with existing wildcard '{}' in existing \
                     prefix '{}'",
                    text(path_seg),
                    text(full_path),
                    text(&n.path),
                    text(&prefix),
                )));
            }
            let Some(&c) = path.first() else {
                return Err(index_error(0, 0));
            };
            if n.n_type == NodeType::Param && c == b'/' && n.children.len() == 1 {
                n = child_mut(n, 0)?;
                n.priority = n.priority.wrapping_add(1);
                continue;
            }
            if let Some(pos) = n.indices.iter().position(|&x| x == c) {
                let pos = n.increment_child_prio(pos);
                n = child_mut(n, pos)?;
                continue;
            }
            if c != b':' && c != b'*' {
                n.indices.push(c);
                n.children.push(Self {
                    max_params: num_params,
                    ..Self::default()
                });
                let last = n.indices.len() - 1;
                let pos = n.increment_child_prio(last);
                n = child_mut(n, pos)?;
            }
            return n.insert_child(num_params, path, full_path, handle);
        }
    }

    /// Go: `insertChild`.
    fn insert_child(
        &mut self,
        mut num_params: u8,
        path: &[u8],
        full_path: &[u8],
        handle: Arc<dyn Handle>,
    ) -> Result<(), RouteError> {
        let full = || text(full_path);
        let mut n = self;
        let mut offset = 0;
        let max = path.len();
        let mut i = 0;
        while num_params > 0 {
            let c = *path.get(i).ok_or_else(|| index_error(i, max))?;
            if c != b':' && c != b'*' {
                i += 1;
                continue;
            }
            let mut end = i + 1;
            while end < max && path.get(end) != Some(&b'/') {
                if matches!(path.get(end), Some(b':' | b'*')) {
                    return Err(RouteError(format!(
                        "only one wildcard per path segment is allowed, has: '{}' in path '{}'",
                        text(path.get(i..).unwrap_or_default()),
                        full()
                    )));
                }
                end += 1;
            }
            if !n.children.is_empty() {
                return Err(RouteError(format!(
                    "wildcard route '{}' conflicts with existing children in path '{}'",
                    text(path.get(i..end).unwrap_or_default()),
                    full()
                )));
            }
            if end - i < 2 {
                return Err(RouteError(format!(
                    "wildcards must be named with a non-empty name in path '{}'",
                    full()
                )));
            }
            if c == b':' {
                if i > 0 {
                    n.path = path.get(offset..i).unwrap_or_default().to_vec();
                    offset = i;
                }
                n.children = vec![Self {
                    n_type: NodeType::Param,
                    max_params: num_params,
                    ..Self::default()
                }];
                n.wild_child = true;
                n = child_mut(n, 0)?;
                n.priority = n.priority.wrapping_add(1);
                num_params -= 1;
                if end < max {
                    n.path = path.get(offset..end).unwrap_or_default().to_vec();
                    offset = end;
                    n.children = vec![Self {
                        max_params: num_params,
                        priority: 1,
                        ..Self::default()
                    }];
                    n = child_mut(n, 0)?;
                }
            } else {
                if end != max || num_params > 1 {
                    return Err(RouteError(format!(
                        "catch-all routes are only allowed at the end of the path in path '{}'",
                        full()
                    )));
                }
                return n.insert_catch_all(path, offset, i, full_path, handle);
            }
            i += 1;
        }
        n.path = path.get(offset..).unwrap_or_default().to_vec();
        n.handle = Some(handle);
        Ok(())
    }

    /// Go: the catch-all branch of `insertChild`, for the `*` at `i`.
    fn insert_catch_all(
        &mut self,
        path: &[u8],
        offset: usize,
        i: usize,
        full_path: &[u8],
        handle: Arc<dyn Handle>,
    ) -> Result<(), RouteError> {
        if self.path.last() == Some(&b'/') {
            return Err(RouteError(format!(
                "catch-all conflicts with existing handle for the path segment root in path '{}'",
                text(full_path)
            )));
        }
        let Some(slash) = i.checked_sub(1) else {
            return Err(RouteError(
                "runtime error: index out of range [-1]".to_owned(),
            ));
        };
        if path.get(slash) != Some(&b'/') {
            return Err(RouteError(format!(
                "no / before catch-all in path '{}'",
                text(full_path)
            )));
        }
        self.path = path.get(offset..slash).unwrap_or_default().to_vec();
        if self.max_params < 1 {
            self.max_params = 1;
        }
        self.indices = vec![b'/'];
        self.children = vec![Self {
            wild_child: true,
            n_type: NodeType::CatchAll,
            max_params: 1,
            priority: 1,
            children: vec![Self {
                path: path.get(slash..).unwrap_or_default().to_vec(),
                n_type: NodeType::CatchAll,
                max_params: 1,
                handle: Some(handle),
                priority: 1,
                ..Self::default()
            }],
            ..Self::default()
        }];
        Ok(())
    }

    /// Go: `getValue`: the handle registered for `path`, its parameters, and whether a handle
    /// exists for the path with the trailing slash added or removed (`tsr`).
    pub(super) fn get_value(&self, path: &str) -> Lookup<'_> {
        let mut path = path.as_bytes();
        let mut n = self;
        let mut params: Vec<Param> = Vec::new();
        let done = |handle, found: Vec<Param>, tsr| Lookup {
            handle,
            params: Params(found),
            tsr,
        };
        loop {
            let np = n.path.len();
            if path.len() > np {
                if path.get(..np) == Some(n.path.as_slice()) {
                    path = path.get(np..).unwrap_or_default();
                    if !n.wild_child {
                        let c = path.first().copied();
                        if let Some(child) = n
                            .indices
                            .iter()
                            .position(|&x| Some(x) == c)
                            .and_then(|i| n.children.get(i))
                        {
                            n = child;
                            continue;
                        }
                        let tsr = path == b"/" && n.handle.is_some();
                        return done(None, params, tsr);
                    }
                    let Some(wild) = n.children.first() else {
                        return done(None, params, false);
                    };
                    n = wild;
                    match n.n_type {
                        NodeType::Param => {
                            let end = path.iter().position(|&c| c == b'/').unwrap_or(path.len());
                            if params.capacity() == 0 {
                                params.reserve_exact(usize::from(n.max_params));
                            }
                            params.push(Param {
                                key: text(n.path.get(1..).unwrap_or_default()),
                                value: text(path.get(..end).unwrap_or_default()),
                            });
                            if end < path.len() {
                                if let Some(next) = n.children.first() {
                                    path = path.get(end..).unwrap_or_default();
                                    n = next;
                                    continue;
                                }
                                let tsr = path.len() == end + 1;
                                return done(None, params, tsr);
                            }
                            if let Some(h) = &n.handle {
                                return done(Some(h), params, false);
                            }
                            if let [only] = n.children.as_slice() {
                                let tsr = only.path == b"/" && only.handle.is_some();
                                return done(None, params, tsr);
                            }
                            return done(None, params, false);
                        }
                        NodeType::CatchAll => {
                            if params.capacity() == 0 {
                                params.reserve_exact(usize::from(n.max_params));
                            }
                            params.push(Param {
                                key: text(n.path.get(2..).unwrap_or_default()),
                                value: text(path),
                            });
                            return done(n.handle.as_ref(), params, false);
                        }
                        NodeType::Static | NodeType::Root => {
                            return done(None, params, false);
                        }
                    }
                }
            } else if path == n.path.as_slice() {
                if let Some(h) = &n.handle {
                    return done(Some(h), params, false);
                }
                if path == b"/" && n.wild_child && n.n_type != NodeType::Root {
                    return done(None, params, true);
                }
                if let Some(i) = n.indices.iter().position(|&x| x == b'/') {
                    let tsr = n.children.get(i).is_some_and(|c| {
                        (c.path.len() == 1 && c.handle.is_some())
                            || (c.n_type == NodeType::CatchAll
                                && c.children.first().is_some_and(|cc| cc.handle.is_some()))
                    });
                    return done(None, params, tsr);
                }
                return done(None, params, false);
            }
            let tsr = path == b"/"
                || (np == path.len() + 1
                    && n.path.get(path.len()) == Some(&b'/')
                    && n.path.get(..np - 1) == Some(path)
                    && n.handle.is_some());
            return done(None, params, tsr);
        }
    }

    /// Go: `findCaseInsensitivePath`: the registered path matching `path` case-insensitively
    /// (and with a trailing slash fixed when `fix_trailing_slash`).
    pub(super) fn find_case_insensitive_path(
        &self,
        path: &str,
        fix_trailing_slash: bool,
    ) -> Option<Vec<u8>> {
        let mut ci_path = Vec::with_capacity(path.len() + 1);
        self.find_case_insensitive_path_rec(
            path.as_bytes(),
            &mut ci_path,
            [0; 4],
            fix_trailing_slash,
        )
        .then_some(ci_path)
    }

    /// Go: `findCaseInsensitivePathRec`. `ci_path` holds the result when it returns `true`;
    /// otherwise its contents are unspecified (Go returns the partial path, which callers
    /// ignore).
    fn find_case_insensitive_path_rec(
        &self,
        path: &[u8],
        ci_path: &mut Vec<u8>,
        rb: [u8; 4],
        fix_trailing_slash: bool,
    ) -> bool {
        let mut n = self;
        let mut path = path;
        let mut rb = rb;
        let mut np_len = n.path.len();
        while path.len() >= np_len
            && (np_len == 0
                || equal_fold(
                    path.get(1..np_len).unwrap_or_default(),
                    n.path.get(1..).unwrap_or_default(),
                ))
        {
            let old_path = path;
            path = path.get(np_len..).unwrap_or_default();
            ci_path.extend_from_slice(&n.path);
            if path.is_empty() {
                return n.ci_path_end(ci_path, fix_trailing_slash);
            }
            if !n.wild_child {
                rb = shift_n_rune_bytes(rb, np_len);
                let next = if rb[0] == 0 {
                    match n.ci_next_rune(old_path, path, ci_path, &mut rb, fix_trailing_slash) {
                        RuneStep::Found => return true,
                        RuneStep::Walk(child) => Some(child),
                        RuneStep::Stuck => None,
                    }
                } else {
                    n.indices
                        .iter()
                        .position(|&x| x == rb[0])
                        .and_then(|i| n.children.get(i))
                };
                if let Some(child) = next {
                    n = child;
                    np_len = n.path.len();
                    continue;
                }
                return fix_trailing_slash && path == b"/" && n.handle.is_some();
            }
            let Some(wild) = n.children.first() else {
                return false;
            };
            n = wild;
            match n.n_type {
                NodeType::Param => {
                    let k = path.iter().position(|&c| c == b'/').unwrap_or(path.len());
                    ci_path.extend_from_slice(path.get(..k).unwrap_or_default());
                    if k < path.len() {
                        if let Some(next) = n.children.first() {
                            n = next;
                            np_len = n.path.len();
                            path = path.get(k..).unwrap_or_default();
                            continue;
                        }
                        return fix_trailing_slash && path.len() == k + 1;
                    }
                    return n.ci_param_end(ci_path, fix_trailing_slash);
                }
                NodeType::CatchAll => {
                    ci_path.extend_from_slice(path);
                    return true;
                }
                NodeType::Static | NodeType::Root => return false,
            }
        }
        if fix_trailing_slash {
            if path == b"/" {
                return true;
            }
            if path.len() + 1 == np_len
                && n.path.get(path.len()) == Some(&b'/')
                && equal_fold(
                    path.get(1..).unwrap_or_default(),
                    n.path.get(1..path.len()).unwrap_or_default(),
                )
                && n.handle.is_some()
            {
                ci_path.extend_from_slice(&n.path);
                return true;
            }
        }
        false
    }

    /// Go: `findCaseInsensitivePathRec` once the whole path is consumed at this node.
    fn ci_path_end(&self, ci_path: &mut Vec<u8>, fix_trailing_slash: bool) -> bool {
        if self.handle.is_some() {
            return true;
        }
        if !fix_trailing_slash {
            return false;
        }
        let Some(i) = self.indices.iter().position(|&x| x == b'/') else {
            return false;
        };
        let Some(c) = self.children.get(i) else {
            return false;
        };
        if (c.path.len() == 1 && c.handle.is_some())
            || (c.n_type == NodeType::CatchAll
                && c.children.first().is_some_and(|cc| cc.handle.is_some()))
        {
            ci_path.push(b'/');
            return true;
        }
        false
    }

    /// Go: `findCaseInsensitivePathRec` when a parameter consumed the rest of the path.
    fn ci_param_end(&self, ci_path: &mut Vec<u8>, fix_trailing_slash: bool) -> bool {
        if self.handle.is_some() {
            return true;
        }
        if fix_trailing_slash
            && let [only] = self.children.as_slice()
            && only.path == b"/"
            && only.handle.is_some()
        {
            ci_path.push(b'/');
            return true;
        }
        false
    }

    /// Go: the `rb[0] == 0` branch of `findCaseInsensitivePathRec`: decode the rune at the end
    /// of this node's path, try its lower-case form (recursively) and then its upper-case form.
    fn ci_next_rune<'n>(
        &'n self,
        old_path: &[u8],
        path: &[u8],
        ci_path: &mut Vec<u8>,
        rb: &mut [u8; 4],
        fix_trailing_slash: bool,
    ) -> RuneStep<'n> {
        let np_len = self.path.len();
        let mut rv = '\0';
        let mut off = 0;
        let max = np_len.min(3);
        while off < max {
            let i = np_len - off;
            if old_path.get(i).is_some_and(|&b| rune_start(b)) {
                rv = decode_rune(old_path.get(i..).unwrap_or_default()).0;
                break;
            }
            off += 1;
        }
        let lo = to_lower(rv);
        encode_rune(rb, lo);
        *rb = shift_n_rune_bytes(*rb, off);
        if let Some(i) = self.indices.iter().position(|&x| x == rb[0]) {
            let saved = ci_path.len();
            if self.children.get(i).is_some_and(|child| {
                child.find_case_insensitive_path_rec(path, ci_path, *rb, fix_trailing_slash)
            }) {
                return RuneStep::Found;
            }
            ci_path.truncate(saved);
        }
        let up = to_upper(rv);
        if up != lo {
            encode_rune(rb, up);
            *rb = shift_n_rune_bytes(*rb, off);
            if let Some(child) = self
                .indices
                .iter()
                .position(|&x| x == rb[0])
                .and_then(|i| self.children.get(i))
            {
                return RuneStep::Walk(child);
            }
        }
        RuneStep::Stuck
    }
}

/// The outcome of [`Node::ci_next_rune`].
enum RuneStep<'n> {
    /// The lower-case recursion found the path.
    Found,
    /// Continue the walk at this child (upper-case match).
    Walk(&'n Node),
    /// No child matches.
    Stuck,
}

#[cfg(test)]
mod tests;
