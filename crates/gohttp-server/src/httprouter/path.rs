//! Go: `httprouter/path.go`.

/// Go: `bufApp`: writes `c` at `w`, copying `s[..w]` into a new buffer the first time the
/// output differs from `s`.
fn buf_app(buf: &mut Option<Vec<u8>>, s: &[u8], w: usize, c: u8) {
    if buf.is_none() {
        if s.get(w) == Some(&c) {
            return;
        }
        let mut fresh = vec![0; s.len()];
        if let (Some(dst), Some(src)) = (fresh.get_mut(..w), s.get(..w)) {
            dst.copy_from_slice(src);
        }
        *buf = Some(fresh);
    }
    if let Some(slot) = buf.as_mut().and_then(|b| b.get_mut(w)) {
        *slot = c;
    }
}

/// Go: `httprouter.CleanPath`: the canonical form of a URL path (one leading `/`, no `.` or
/// `..` elements, no repeated slashes, a trailing slash kept).
#[must_use]
pub fn clean_path(p: &str) -> String {
    if p.is_empty() {
        return "/".to_owned();
    }
    let p = p.as_bytes();
    let len = p.len();
    let at = |i: usize| p.get(i).copied();
    let mut buf: Option<Vec<u8>> = None;
    let mut r = 1;
    let mut w = 1;
    if at(0) != Some(b'/') {
        r = 0;
        let mut fresh = vec![0; len + 1];
        if let Some(first) = fresh.first_mut() {
            *first = b'/';
        }
        buf = Some(fresh);
    }
    let mut trailing = len > 1 && at(len - 1) == Some(b'/');
    while r < len {
        let cur = at(r);
        if cur == Some(b'/') {
            r += 1;
        } else if cur == Some(b'.') && r + 1 == len {
            trailing = true;
            r += 1;
        } else if cur == Some(b'.') && at(r + 1) == Some(b'/') {
            r += 2;
        } else if cur == Some(b'.')
            && at(r + 1) == Some(b'.')
            && (r + 2 == len || at(r + 2) == Some(b'/'))
        {
            r += 3;
            if w > 1 {
                w -= 1;
                let out = buf.as_deref().unwrap_or(p);
                while w > 1 && out.get(w) != Some(&b'/') {
                    w -= 1;
                }
            }
        } else {
            if w > 1 {
                buf_app(&mut buf, p, w, b'/');
                w += 1;
            }
            while r < len && at(r) != Some(b'/') {
                if let Some(ch) = at(r) {
                    buf_app(&mut buf, p, w, ch);
                }
                w += 1;
                r += 1;
            }
        }
    }
    if trailing && w > 1 {
        buf_app(&mut buf, p, w, b'/');
        w += 1;
    }
    let out = buf.as_deref().unwrap_or(p);
    String::from_utf8_lossy(out.get(..w).unwrap_or(out)).into_owned()
}

#[cfg(test)]
mod tests {
    use super::clean_path;

    #[test]
    fn path_clean_like_go() -> Result<(), String> {
        // Go: httprouter path_test.go cleanTests.
        let tests = [
            // Already clean
            ("/", "/"),
            ("/abc", "/abc"),
            ("/a/b/c", "/a/b/c"),
            ("/abc/", "/abc/"),
            ("/a/b/c/", "/a/b/c/"),
            // missing root
            ("", "/"),
            ("a/", "/a/"),
            ("abc", "/abc"),
            ("abc/def", "/abc/def"),
            ("a/b/c", "/a/b/c"),
            // Remove doubled slash
            ("//", "/"),
            ("/abc//", "/abc/"),
            ("/abc/def//", "/abc/def/"),
            ("/a/b/c//", "/a/b/c/"),
            ("/abc//def//ghi", "/abc/def/ghi"),
            ("//abc", "/abc"),
            ("///abc", "/abc"),
            ("//abc//", "/abc/"),
            // Remove . elements
            (".", "/"),
            ("./", "/"),
            ("/abc/./def", "/abc/def"),
            ("/./abc/def", "/abc/def"),
            ("/abc/.", "/abc/"),
            // Remove .. elements
            ("..", "/"),
            ("../", "/"),
            ("../../", "/"),
            ("../..", "/"),
            ("../../abc", "/abc"),
            ("/abc/def/ghi/../jkl", "/abc/def/jkl"),
            ("/abc/def/../ghi/../jkl", "/abc/jkl"),
            ("/abc/def/..", "/abc"),
            ("/abc/def/../..", "/"),
            ("/abc/def/../../..", "/"),
            ("/abc/def/../../..", "/"),
            ("/abc/def/../../../ghi/jkl/../../../mno", "/mno"),
            // Combinations
            ("abc/./../def", "/def"),
            ("abc//./../def", "/def"),
            ("abc/../../././../def", "/def"),
        ];
        for (path, want) in tests {
            let got = clean_path(path);
            if got != want {
                return Err(format!("CleanPath({path:?}) = {got:?}, want {want:?}"));
            }
            let again = clean_path(want);
            if again != want {
                return Err(format!("CleanPath({want:?}) = {again:?}, want {want:?}"));
            }
        }
        Ok(())
    }
}
