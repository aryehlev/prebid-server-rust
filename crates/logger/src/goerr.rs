//! Go's text for operating-system errors: `*os.PathError` over a `syscall.Errno`.

use std::{
    io::{self, ErrorKind},
    path::Path,
};

/// Go's `syscall.Errno` texts for the error kinds file logging meets, used for an
/// [`io::Error`] that carries no OS error number.
const ERRNO_TEXT: [(ErrorKind, &str); 7] = [
    (ErrorKind::NotFound, "no such file or directory"),
    (ErrorKind::PermissionDenied, "permission denied"),
    (ErrorKind::AlreadyExists, "file exists"),
    (ErrorKind::IsADirectory, "is a directory"),
    (ErrorKind::NotADirectory, "not a directory"),
    (ErrorKind::ReadOnlyFilesystem, "read-only file system"),
    (ErrorKind::StorageFull, "no space left on device"),
];

/// Go: `syscall.Errno.Error()`.
///
/// Go's errno tables were generated from the C library's `strerror` (Go:
/// `syscall/mkerrors.sh`), with the first letter lowercased when the second is lowercase
/// (`Bad` becomes `bad`, `RPC` and `STREAM` stay), and an errno without an entry prints as
/// `errno N`. An error with an OS error number is rendered that way from the same `strerror`
/// text, so errors that share an [`ErrorKind`] stay apart (`EPERM` is "operation not
/// permitted", `EACCES` "permission denied"). An error without a number uses the table of
/// its kind, else its own text.
#[must_use]
pub fn errno_text(err: &io::Error) -> String {
    let Some(code) = err.raw_os_error() else {
        return ERRNO_TEXT
            .iter()
            .find(|(kind, _)| *kind == err.kind())
            .map_or_else(|| err.to_string(), |&(_, text)| text.to_owned());
    };
    let full = err.to_string();
    let message = full
        .rfind(" (os error ")
        .and_then(|i| full.get(..i))
        .unwrap_or(&full);
    // glibc: "Unknown error N"; macOS: "Unknown error: N".
    if message.is_empty() || message.starts_with("Unknown error") {
        return format!("errno {code}");
    }
    match message.as_bytes() {
        [first @ b'A'..=b'Z', b'a'..=b'z', ..] => {
            let mut text = String::with_capacity(message.len());
            text.push(char::from(first.to_ascii_lowercase()));
            text.push_str(message.get(1..).unwrap_or_default());
            text
        }
        _ => message.to_owned(),
    }
}

/// Go: `(&os.PathError{Op: op, Path: path, Err: err}).Error()`.
#[must_use]
pub fn path_error(op: &str, path: &Path, err: &io::Error) -> String {
    format!("{op} {}: {}", path.display(), errno_text(err))
}

#[cfg(test)]
mod tests {
    use std::io::{self, ErrorKind};

    use super::errno_text;

    #[test]
    fn errno_text_is_go_syscall_errno_error() -> Result<(), String> {
        // Go: syscall.Errno(n).Error(), from the errors tables of zerrors_linux_amd64.go and
        // zerrors_darwin_arm64.go, which agree on these numbers. EPERM and EACCES share
        // Rust's PermissionDenied kind but not Go's text.
        let cases = [
            (1, "operation not permitted"),
            (2, "no such file or directory"),
            (5, "input/output error"),
            (9, "bad file descriptor"),
            (12, "cannot allocate memory"),
            (13, "permission denied"),
            (17, "file exists"),
            (20, "not a directory"),
            (21, "is a directory"),
            (24, "too many open files"),
            (28, "no space left on device"),
            (30, "read-only file system"),
            (100_000, "errno 100000"),
        ];
        for (code, want) in cases {
            let got = errno_text(&io::Error::from_raw_os_error(code));
            if got != want {
                return Err(format!("errno {code}: got {got:?}, want {want:?}"));
            }
        }
        // Errors without a number fall back to the table of their kind.
        let synthetic = errno_text(&io::Error::from(ErrorKind::NotFound));
        if synthetic != "no such file or directory" {
            return Err(format!("synthetic NotFound: {synthetic:?}"));
        }
        Ok(())
    }
}
