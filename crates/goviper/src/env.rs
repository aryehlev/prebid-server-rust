//! The environment variables and working directory a [`Viper`](crate::Viper) reads (Go:
//! `os.LookupEnv`, `os.Getenv`, `os.ExpandEnv` and `os.Getwd`).
//!
//! [`Environment::process`] is the real process environment, as Go reads it. Tests and embedders
//! that must not touch the process (setting variables is `unsafe` in Rust 2024) use
//! [`Environment::from_vars`] and [`Environment::with_working_dir`]: the variables and the
//! directory relative paths resolve against are then those given, and nothing else is read.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

/// Where environment variables and the working directory come from.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    /// The variables, or `None` for the process environment.
    vars: Option<BTreeMap<String, String>>,
    /// The working directory, or `None` for the process's.
    working_dir: Option<PathBuf>,
}

impl Environment {
    /// The process environment and working directory (Go's `os` functions). Variables whose name
    /// or value is not UTF-8 are read with each invalid sequence replaced by U+FFFD.
    #[must_use]
    pub const fn process() -> Self {
        Self {
            vars: None,
            working_dir: None,
        }
    }

    /// An environment holding exactly `vars` (a later pair replaces an earlier one with the same
    /// name), with the process's working directory unless [`Environment::with_working_dir`] sets
    /// one.
    #[must_use]
    pub fn from_vars<I, K, V>(vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            vars: Some(
                vars.into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect(),
            ),
            working_dir: None,
        }
    }

    /// The same variables, with `dir` as the working directory: relative configuration paths
    /// resolve against it, and Go's `os.Getwd` returns it (with symbolic links resolved, as the
    /// kernel reports a directory changed into).
    #[must_use]
    pub fn with_working_dir<P: Into<PathBuf>>(self, dir: P) -> Self {
        Self {
            working_dir: Some(dir.into()),
            ..self
        }
    }

    /// Go: `os.LookupEnv`.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<String> {
        if let Some(vars) = &self.vars {
            return vars.get(name).cloned();
        }
        // Go: no environment entry has an empty name or one containing '=' (and the C
        // environment cannot hold a NUL).
        if name.is_empty() || name.contains(['=', '\0']) {
            return None;
        }
        env::var_os(name).map(|v| v.to_string_lossy().into_owned())
    }

    /// Go: `os.Getenv`: the value, or `""` for an unset variable.
    #[must_use]
    pub fn getenv(&self, name: &str) -> String {
        self.lookup(name).unwrap_or_default()
    }

    /// Go: `os.ExpandEnv`: replaces `$var` and `${var}` with the variables' values (`""` when
    /// unset), leaves a `$` not followed by a name alone, and drops the invalid `${}` and an
    /// unterminated `${`.
    #[must_use]
    pub fn expand(&self, s: &str) -> String {
        let bytes = s.as_bytes();
        let mut buf: Option<Vec<u8>> = None;
        // Go: os.Expand. `${}` is all ASCII, so bytes are fine for this operation.
        let mut i = 0;
        let mut j = 0;
        while j < bytes.len() {
            if bytes.get(j) == Some(&b'$') && j + 1 < bytes.len() {
                let out = buf.get_or_insert_with(|| Vec::with_capacity(2 * bytes.len()));
                out.extend_from_slice(bytes.get(i..j).unwrap_or_default());
                let (name, w) = shell_name(bytes.get(j + 1..).unwrap_or_default());
                if name.is_empty() && w > 0 {
                    // Encountered invalid syntax; eat the characters.
                } else if name.is_empty() {
                    // Valid syntax, but $ was not followed by a name. Leave the dollar character
                    // untouched.
                    out.push(b'$');
                } else {
                    let value = self.getenv(&String::from_utf8_lossy(name));
                    out.extend_from_slice(value.as_bytes());
                }
                j += w;
                i = j + 1;
            }
            j += 1;
        }
        let Some(mut out) = buf else {
            return s.to_owned();
        };
        out.extend_from_slice(bytes.get(i..).unwrap_or_default());
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Go: `os.Getwd` on Unix: `$PWD` if it names the working directory, else the working
    /// directory with symbolic links resolved; `None` on error.
    pub(crate) fn getwd(&self) -> Option<String> {
        let dot = self
            .working_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        // Go: "Clumsy but widespread kludge: if $PWD is set and matches ".", use it."
        if let Some(pwd) = self.lookup("PWD")
            && pwd.starts_with('/')
            && same_file(&dot, Path::new(&pwd))
        {
            return Some(pwd);
        }
        let dir = match &self.working_dir {
            Some(dir) => fs::canonicalize(dir).ok()?,
            None => env::current_dir().ok()?,
        };
        Some(dir.to_string_lossy().into_owned())
    }

    /// The path the file system is asked for: a relative `path` is relative to the working
    /// directory.
    pub(crate) fn resolve(&self, path: &str) -> PathBuf {
        match &self.working_dir {
            Some(dir) if !path.starts_with('/') => dir.join(path),
            Some(_) | None => PathBuf::from(path),
        }
    }
}

/// Go: `os.SameFile` of two directories' `Stat`s (here, of the paths with links resolved).
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        (Ok(_) | Err(_), Err(_)) | (Err(_), Ok(_)) => false,
    }
}

/// Go: `isShellSpecialVar`.
const fn is_shell_special_var(c: u8) -> bool {
    matches!(
        c,
        b'*' | b'#' | b'$' | b'@' | b'!' | b'?' | b'-' | b'0'..=b'9'
    )
}

/// Go: `isAlphaNum`.
const fn is_alpha_num(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}

/// Go: `getShellName`: the name after a `$` and the number of bytes it takes; an empty name
/// with a non-zero width is invalid syntax.
fn shell_name(s: &[u8]) -> (&[u8], usize) {
    match s.first() {
        Some(&b'{') => {
            if s.len() > 2
                && s.get(1).copied().is_some_and(is_shell_special_var)
                && s.get(2) == Some(&b'}')
            {
                return (s.get(1..2).unwrap_or_default(), 3);
            }
            // Scan to closing brace.
            for (i, &c) in s.iter().enumerate().skip(1) {
                if c == b'}' {
                    if i == 1 {
                        // Bad syntax; eat "${}".
                        return (&[], 2);
                    }
                    return (s.get(1..i).unwrap_or_default(), i + 1);
                }
            }
            // Bad syntax; eat "${".
            (&[], 1)
        }
        Some(&c) if is_shell_special_var(c) => (s.get(..1).unwrap_or_default(), 1),
        Some(_) | None => {
            // Scan alphanumerics.
            let n = s.iter().take_while(|&&c| is_alpha_num(c)).count();
            (s.get(..n).unwrap_or_default(), n)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Environment;

    /// Go: `os.TestExpand`'s table, with its `mapping` as the variables.
    #[test]
    fn expand_like_go() -> Result<(), String> {
        let env = Environment::from_vars([
            ("*", "all the args"),
            ("#", "NARGS"),
            ("$", "PID"),
            ("1", "ARGUMENT1"),
            ("HOME", "/usr/gopher"),
            ("H", "(Value of H)"),
            ("home_1", "/usr/foo"),
            ("_", "underscore"),
        ]);
        for (input, want) in [
            ("", ""),
            ("$*", "all the args"),
            ("$$", "PID"),
            ("${*}", "all the args"),
            ("$1", "ARGUMENT1"),
            ("${1}", "ARGUMENT1"),
            ("now is the time", "now is the time"),
            ("$HOME", "/usr/gopher"),
            ("$home_1", "/usr/foo"),
            ("${HOME}", "/usr/gopher"),
            ("${H}OME", "(Value of H)OME"),
            (
                "A$$$#$1$H$home_1*B",
                "APIDNARGSARGUMENT1(Value of H)/usr/foo*B",
            ),
            ("start$+middle$^end$", "start$+middle$^end$"),
            ("mixed$|bag$$$", "mixed$|bagPID$"),
            ("$", "$"),
            ("$}", "$}"),
            ("${", ""),
            ("${}", ""),
        ] {
            let got = env.expand(input);
            if got != want {
                return Err(format!("expand({input:?}) = {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }
}
