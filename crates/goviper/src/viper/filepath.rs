//! Go `path/filepath` on Unix: the lexical path functions viper uses to search for and name
//! configuration files.

/// Go: `filepath.Clean`: the shortest equivalent path, by lexical processing only.
pub(super) fn clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let p = path.as_bytes();
    let n = p.len();
    let rooted = p.first() == Some(&b'/');
    // Invariants: reading from p, r is the index of the next byte to process; writing to out;
    // dotdot is the length of out that `..` must not remove, either because it is the leading
    // slash or a leading `../../..` prefix.
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let (mut r, mut dotdot) = if rooted {
        out.push(b'/');
        (1, 1)
    } else {
        (0, 0)
    };
    let is_sep = |i: usize| p.get(i) == Some(&b'/');
    while r < n {
        let dot = p.get(r) == Some(&b'.');
        if is_sep(r) {
            // empty path element
            r += 1;
        } else if dot && (r + 1 == n || is_sep(r + 1)) {
            // . element
            r += 1;
        } else if dot && p.get(r + 1) == Some(&b'.') && (r + 2 == n || is_sep(r + 2)) {
            // .. element: remove to last separator
            r += 2;
            if out.len() > dotdot {
                // can backtrack
                let mut removed = out.pop();
                while out.len() > dotdot && removed != Some(b'/') {
                    removed = out.pop();
                }
            } else if !rooted {
                // cannot backtrack, but not rooted, so append .. element.
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            // real path element; add slash if needed
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(b'/');
            }
            // copy element
            while let Some(&c) = p.get(r) {
                if c == b'/' {
                    break;
                }
                out.push(c);
                r += 1;
            }
        }
    }
    // Turn empty string into "."
    if out.is_empty() {
        out.push(b'.');
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Go: `filepath.Join`: the non-empty elements joined with `/`, cleaned; `""` if all are
/// empty.
pub(super) fn join(elems: &[&str]) -> String {
    let Some(first) = elems.iter().position(|e| !e.is_empty()) else {
        return String::new();
    };
    clean(&elems.get(first..).unwrap_or_default().join("/"))
}

/// Go: `filepath.Ext`: the suffix from the final `.` in the final element, or `""`.
pub(super) fn ext(path: &str) -> &str {
    for (i, b) in path.bytes().enumerate().rev() {
        match b {
            b'/' => break,
            b'.' => return path.get(i..).unwrap_or_default(),
            _ => {}
        }
    }
    ""
}

/// Go: `filepath.IsAbs` on Unix.
pub(super) fn is_abs(path: &str) -> bool {
    path.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::{clean, ext, join};

    /// Go: `path/filepath`'s `cleantests` (the Unix ones).
    #[test]
    fn clean_like_go() -> Result<(), String> {
        for (path, want) in [
            // Already clean
            ("abc", "abc"),
            ("abc/def", "abc/def"),
            ("a/b/c", "a/b/c"),
            (".", "."),
            ("..", ".."),
            ("../..", "../.."),
            ("../../abc", "../../abc"),
            ("/abc", "/abc"),
            ("/", "/"),
            // Empty is current dir
            ("", "."),
            // Remove trailing slash
            ("abc/", "abc"),
            ("abc/def/", "abc/def"),
            ("a/b/c/", "a/b/c"),
            ("./", "."),
            ("../", ".."),
            ("../../", "../.."),
            ("/abc/", "/abc"),
            // Remove doubled slash
            ("abc//def//ghi", "abc/def/ghi"),
            ("abc//", "abc"),
            // Remove . elements
            ("abc/./def", "abc/def"),
            ("/./abc/def", "/abc/def"),
            ("abc/.", "abc"),
            // Remove .. elements
            ("abc/def/ghi/../jkl", "abc/def/jkl"),
            ("abc/def/../ghi/../jkl", "abc/jkl"),
            ("abc/def/..", "abc"),
            ("abc/def/../..", "."),
            ("/abc/def/../..", "/"),
            ("abc/def/../../..", ".."),
            ("/abc/def/../../..", "/"),
            ("abc/def/../../../ghi/jkl/../../../mno", "../../mno"),
            ("/../abc", "/abc"),
            ("a/../b:/../../c", "../c"),
            // Combinations
            ("abc/./../def", "def"),
            ("abc//./../def", "def"),
            ("abc/../../././../def", "../../def"),
        ] {
            let got = clean(path);
            if got != want {
                return Err(format!("Clean({path:?}) = {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }

    #[test]
    fn join_and_ext_like_go() -> Result<(), String> {
        let joins: [(&[&str], &str); 8] = [
            (&[], ""),
            (&[""], ""),
            (&["a", "b"], "a/b"),
            (&["a", ""], "a"),
            (&["", "b"], "b"),
            (&["/", "a"], "/a"),
            (&["a/", "b"], "a/b"),
            (&["", ""], ""),
        ];
        for (elems, want) in joins {
            let got = join(elems);
            if got != want {
                return Err(format!("Join({elems:?}) = {got:?}, want {want:?}"));
            }
        }
        for (path, want) in [
            ("path.go", ".go"),
            ("path.pb.go", ".go"),
            ("a.dir/b", ""),
            ("a.dir/b.go", ".go"),
            ("a.dir/", ""),
        ] {
            let got = ext(path);
            if got != want {
                return Err(format!("Ext({path:?}) = {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }
}
