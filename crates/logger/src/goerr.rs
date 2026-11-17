//! Go's text for operating-system errors: `*os.PathError` over a `syscall.Errno`.

use std::{
    io::{self, ErrorKind},
    path::Path,
};

/// Go's `syscall.Errno` texts for the errors file logging meets.
const ERRNO_TEXT: [(ErrorKind, &str); 7] = [
    (ErrorKind::NotFound, "no such file or directory"),
    (ErrorKind::PermissionDenied, "permission denied"),
    (ErrorKind::AlreadyExists, "file exists"),
    (ErrorKind::IsADirectory, "is a directory"),
    (ErrorKind::NotADirectory, "not a directory"),
    (ErrorKind::ReadOnlyFilesystem, "read-only file system"),
    (ErrorKind::StorageFull, "no space left on device"),
];

/// Go: `syscall.Errno.Error()`: a table entry, or else the C library's message without Rust's
/// ` (os error N)` suffix and with a lowercase first letter, which is how Go's table reads.
#[must_use]
pub fn errno_text(err: &io::Error) -> String {
    if let Some(&(_, text)) = ERRNO_TEXT.iter().find(|(kind, _)| *kind == err.kind()) {
        return text.to_owned();
    }
    let full = err.to_string();
    let message = full
        .rfind(" (os error ")
        .and_then(|i| full.get(..i))
        .unwrap_or(&full);
    let mut chars = message.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

/// Go: `(&os.PathError{Op: op, Path: path, Err: err}).Error()`.
#[must_use]
pub fn path_error(op: &str, path: &Path, err: &io::Error) -> String {
    format!("{op} {}: {}", path.display(), errno_text(err))
}
