//! Go: the `context.Context` of a server request (`Request.Context()`), reduced to what the
//! server and the router set on it: cancellation when the client goes away, a write fails or
//! the handler returns, and values (`context.WithValue`).

use core::{
    any::Any,
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};
use std::{error::Error as StdError, sync::Arc};

use tokio::sync::Notify;

/// Go: `context.Canceled` (`context canceled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Canceled;

impl fmt::Display for Canceled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("context canceled")
    }
}

impl StdError for Canceled {}

#[derive(Debug, Default)]
struct Inner {
    cancelled: AtomicBool,
    notify: Notify,
}

/// Go: the request's `context.Context` (`r.Context()`).
///
/// The server cancels it when the client's connection closes (seen by the background read once
/// the body is consumed), when a write to the connection fails, or when the handler returns;
/// [`RequestContext::cancel`] cancels it too. Clones and contexts derived with
/// [`RequestContext::with_value`] share the cancellation state.
#[derive(Clone, Default)]
pub struct RequestContext {
    inner: Arc<Inner>,
    values: Vec<Arc<dyn Any + Send + Sync>>,
}

impl fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestContext")
            .field("cancelled", &self.is_done())
            .field("values", &self.values.len())
            .finish_non_exhaustive()
    }
}

impl RequestContext {
    /// A context that is not cancelled (Go: `context.Background()` wrapped by `WithCancel`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Go: `cancel()`. Idempotent.
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
        }
    }

    /// Whether the context is cancelled (Go: `ctx.Err() != nil`).
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Go: `ctx.Err()`: `Some(Canceled)` once cancelled.
    #[must_use]
    pub fn err(&self) -> Option<Canceled> {
        self.is_done().then_some(Canceled)
    }

    /// Go: `<-ctx.Done()`: completes once the context is cancelled.
    pub async fn done(&self) {
        loop {
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            let _registered = notified.as_mut().enable();
            if self.is_done() {
                return;
            }
            notified.await;
        }
    }

    /// Go: `context.WithValue(ctx, key, value)` with the value's type as the key: a derived
    /// context that shares this one's cancellation.
    #[must_use]
    pub fn with_value<T: Any + Send + Sync>(&self, value: T) -> Self {
        let mut values = self.values.clone();
        values.push(Arc::new(value));
        Self {
            inner: Arc::clone(&self.inner),
            values,
        }
    }

    /// Go: `ctx.Value(key)`: the innermost value of type `T`.
    #[must_use]
    pub fn value<T: Any>(&self) -> Option<&T> {
        self.values
            .iter()
            .rev()
            .find_map(|v| v.as_ref().downcast_ref::<T>())
    }
}
