//! Helpers for the unit tests.

use std::{
    fmt::Debug,
    task::{Context as TaskContext, Poll, Waker},
};

use crate::{bufio::GoRead, error::HttpError};

/// `Ok` when `got == want`, otherwise a message naming `what`.
///
/// # Errors
///
/// The mismatch, with both values.
pub fn ensure_eq<T: PartialEq + Debug + ?Sized>(
    got: &T,
    want: &T,
    what: &str,
) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}"))
    }
}

/// A Go reader over a byte slice that returns at most `chunk` bytes per read.
#[derive(Debug)]
pub struct SliceSource<'a> {
    data: &'a [u8],
    chunk: usize,
}

impl<'a> SliceSource<'a> {
    /// A source returning `data` in pieces of at most `chunk` bytes.
    #[must_use]
    pub const fn new(data: &'a [u8], chunk: usize) -> Self {
        Self { data, chunk }
    }
}

impl GoRead for SliceSource<'_> {
    fn poll_go_read(
        &mut self,
        _cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, HttpError>> {
        if self.data.is_empty() {
            return Poll::Ready(Err(HttpError::Eof));
        }
        let n = buf.len().min(self.chunk.max(1)).min(self.data.len());
        let (head, tail) = self.data.split_at(n);
        if let Some(dst) = buf.get_mut(..n) {
            dst.copy_from_slice(head);
        }
        self.data = tail;
        Poll::Ready(Ok(n))
    }
}

/// Polls `f` once with a no-op waker; `Pending` is an error.
///
/// # Errors
///
/// `unexpected Pending` when `f` is not ready.
pub fn poll_ready<T, F: FnMut(&mut TaskContext<'_>) -> Poll<T>>(mut f: F) -> Result<T, String> {
    let mut cx = TaskContext::from_waker(Waker::noop());
    match f(&mut cx) {
        Poll::Ready(v) => Ok(v),
        Poll::Pending => Err("unexpected Pending".to_owned()),
    }
}
