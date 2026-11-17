//! Go `context`: cancellation, deadlines, causes and values as `net/http` observes them.
//!
//! A [`Context`] is cheap to clone and shared between tasks. Cancellation propagates eagerly from
//! a parent to its children, as Go's `propagateCancel` does; deadlines are checked when observed
//! ([`Context::err`], [`Context::done`]), which gives the same results as Go's timers: a context
//! whose deadline has passed reports `context deadline exceeded`, and a cancel function called
//! after the deadline leaves that error in place.

use std::{
    any::{Any, TypeId},
    fmt,
    future::pending,
    mem, ptr,
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
    time::{Duration, Instant},
};

use tokio::{sync::Notify, time::sleep_until};

use crate::error::{ContextError, HttpError};

/// Go: `context.Context`.
#[derive(Clone, Default)]
pub struct Context {
    /// The nearest node that can be canceled (none for `Background` and `WithoutCancel`).
    cancel: Option<Arc<CancelNode>>,
    /// The innermost value, chained to the parent's values.
    values: Option<Arc<ValueNode>>,
}

struct ValueNode {
    key: TypeId,
    value: Arc<dyn Any + Send + Sync>,
    parent: Option<Arc<Self>>,
}

struct CancelNode {
    /// The effective deadline: the earliest of this node's and its ancestors' deadlines.
    deadline: Option<Instant>,
    parent: Option<Arc<Self>>,
    state: Mutex<CancelState>,
    notify: Notify,
}

#[derive(Default)]
struct CancelState {
    /// Go: `cancelCtx.err` and `cancelCtx.cause`.
    done: Option<(ContextError, HttpError)>,
    children: Vec<Weak<CancelNode>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl CancelNode {
    fn new(parent: Option<&Arc<Self>>, deadline: Option<Instant>) -> Arc<Self> {
        let inherited = parent.and_then(|p| p.deadline);
        let deadline = match (inherited, deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let node = Arc::new(Self {
            deadline,
            parent: parent.map(Arc::clone),
            state: Mutex::new(CancelState::default()),
            notify: Notify::new(),
        });
        if let Some(p) = parent {
            // Go: propagateCancel. A parent that is already done cancels the child at once.
            let inherited_done = {
                let mut st = lock(&p.state);
                if st.done.is_none() {
                    st.children.retain(|w| w.strong_count() > 0);
                    st.children.push(Arc::downgrade(&node));
                }
                st.done.clone()
            };
            if let Some((err, cause)) = inherited_done.or_else(|| p.expired()) {
                node.cancel(false, err, &cause);
            }
        }
        node
    }

    /// The deadline error when the deadline has passed (and the node is not yet done).
    fn expired(&self) -> Option<(ContextError, HttpError)> {
        self.deadline.filter(|d| Instant::now() >= *d).map(|_| {
            (
                ContextError::DeadlineExceeded,
                HttpError::Context(ContextError::DeadlineExceeded),
            )
        })
    }

    fn done(&self) -> Option<(ContextError, HttpError)> {
        let current = lock(&self.state).done.clone();
        if current.is_some() {
            return current;
        }
        let (err, cause) = self.expired()?;
        self.cancel(true, err, &cause);
        lock(&self.state).done.clone()
    }

    /// Go: `cancelCtx.cancel`. The first cancellation wins; children inherit it.
    fn cancel(&self, remove_from_parent: bool, err: ContextError, cause: &HttpError) {
        let children = {
            let mut st = lock(&self.state);
            if st.done.is_some() {
                return;
            }
            st.done = Some((err, cause.clone()));
            mem::take(&mut st.children)
        };
        self.notify.notify_waiters();
        for child in children.iter().filter_map(Weak::upgrade) {
            child.cancel(false, err, cause);
        }
        if remove_from_parent && let Some(p) = &self.parent {
            let me: *const Self = self;
            lock(&p.state)
                .children
                .retain(|w| w.strong_count() > 0 && !ptr::eq(w.as_ptr(), me));
        }
    }
}

/// Go: the `CancelFunc` returned by `WithCancel`, `WithDeadline` and `WithTimeout`.
#[derive(Clone)]
pub struct CancelFunc(Option<Arc<CancelNode>>);

/// Go: the `CancelCauseFunc` returned by `WithCancelCause`.
#[derive(Clone)]
pub struct CancelCauseFunc(Arc<CancelNode>);

impl CancelFunc {
    /// Cancels the context with `context canceled`, unless it is already done (an expired
    /// deadline counts as done).
    pub fn cancel(&self) {
        if let Some(node) = &self.0 {
            if let Some((err, cause)) = node.expired() {
                node.cancel(true, err, &cause);
            } else {
                node.cancel(
                    true,
                    ContextError::Canceled,
                    &HttpError::Context(ContextError::Canceled),
                );
            }
        }
    }
}

impl CancelCauseFunc {
    /// Cancels the context with `context canceled` and the given cause (`None` means the cause
    /// is `context.Canceled`, as Go's `cancel(nil)`).
    pub fn cancel(&self, cause: Option<HttpError>) {
        if let Some((err, deadline_cause)) = self.0.expired() {
            self.0.cancel(true, err, &deadline_cause);
            return;
        }
        let cause = cause.unwrap_or(HttpError::Context(ContextError::Canceled));
        self.0.cancel(true, ContextError::Canceled, &cause);
    }
}

impl fmt::Debug for CancelFunc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CancelFunc")
    }
}

impl fmt::Debug for CancelCauseFunc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CancelCauseFunc")
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context")
            .field("deadline", &self.deadline())
            .field("err", &self.err())
            .finish_non_exhaustive()
    }
}

impl Context {
    /// Go: `context.Background()`: never canceled, no deadline, no values.
    #[must_use]
    pub const fn background() -> Self {
        Self {
            cancel: None,
            values: None,
        }
    }

    /// Go: `context.WithCancel(parent)`.
    #[must_use]
    pub fn with_cancel(&self) -> (Self, CancelFunc) {
        let node = CancelNode::new(self.cancel.as_ref(), None);
        let ctx = Self {
            cancel: Some(Arc::clone(&node)),
            values: self.values.clone(),
        };
        (ctx, CancelFunc(Some(node)))
    }

    /// Go: `context.WithCancelCause(parent)`.
    #[must_use]
    pub fn with_cancel_cause(&self) -> (Self, CancelCauseFunc) {
        let node = CancelNode::new(self.cancel.as_ref(), None);
        let ctx = Self {
            cancel: Some(Arc::clone(&node)),
            values: self.values.clone(),
        };
        (ctx, CancelCauseFunc(node))
    }

    /// Go: `context.WithDeadline(parent, d)`. A parent deadline that is earlier wins.
    #[must_use]
    pub fn with_deadline(&self, deadline: Instant) -> (Self, CancelFunc) {
        let node = CancelNode::new(self.cancel.as_ref(), Some(deadline));
        let ctx = Self {
            cancel: Some(Arc::clone(&node)),
            values: self.values.clone(),
        };
        if let Some((err, cause)) = node.expired() {
            node.cancel(true, err, &cause);
        }
        (ctx, CancelFunc(Some(node)))
    }

    /// Go: `context.WithTimeout(parent, d)`.
    #[must_use]
    pub fn with_timeout(&self, timeout: Duration) -> (Self, CancelFunc) {
        self.with_deadline(Instant::now() + timeout)
    }

    /// Go: `context.WithoutCancel(parent)`: keeps the values, drops cancellation and deadline.
    #[must_use]
    pub fn without_cancel(&self) -> Self {
        Self {
            cancel: None,
            values: self.values.clone(),
        }
    }

    /// Go: `context.WithValue(parent, key, value)` with the value's type as the key.
    #[must_use]
    pub fn with_value<T: Any + Send + Sync>(&self, value: T) -> Self {
        Self {
            cancel: self.cancel.clone(),
            values: Some(Arc::new(ValueNode {
                key: TypeId::of::<T>(),
                value: Arc::new(value),
                parent: self.values.clone(),
            })),
        }
    }

    /// Go: `ctx.Value(key)` for a value stored with [`Context::with_value`].
    #[must_use]
    pub fn value<T: Any + Send + Sync>(&self) -> Option<&T> {
        let mut node = self.values.as_deref();
        while let Some(n) = node {
            if n.key == TypeId::of::<T>() {
                return n.value.downcast_ref::<T>();
            }
            node = n.parent.as_deref();
        }
        None
    }

    /// Go: `ctx.Deadline()`.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        self.cancel.as_ref().and_then(|c| c.deadline)
    }

    /// Go: `ctx.Err()`: `None` while the context is live.
    #[must_use]
    pub fn err(&self) -> Option<ContextError> {
        self.cancel.as_ref().and_then(|c| c.done()).map(|d| d.0)
    }

    /// Go: `context.Cause(ctx)`: the cancel cause, `None` while the context is live.
    #[must_use]
    pub fn cause(&self) -> Option<HttpError> {
        self.cancel.as_ref().and_then(|c| c.done()).map(|d| d.1)
    }

    /// Whether the context can ever be done (Go: `ctx.Done() != nil`).
    #[must_use]
    pub const fn can_be_done(&self) -> bool {
        self.cancel.is_some()
    }

    /// Go: `<-ctx.Done()`: completes when the context is canceled or its deadline passes, and
    /// never for a context that cannot be canceled.
    pub async fn done(&self) {
        let Some(node) = &self.cancel else {
            return pending().await;
        };
        loop {
            let notified = node.notify.notified();
            tokio::pin!(notified);
            let _registered = notified.as_mut().enable();
            if node.done().is_some() {
                return;
            }
            match node.deadline {
                Some(d) => {
                    tokio::select! {
                        () = &mut notified => {}
                        () = sleep_until(d.into()) => {}
                    }
                }
                None => notified.await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::ensure_eq;

    #[tokio::test]
    async fn cancel_propagates_and_keeps_first_error() -> Result<(), String> {
        let (parent, cancel) = Context::background().with_cancel();
        let (child, child_cancel) = parent.with_cancel_cause();
        ensure_eq(&child.err(), &None, "live child")?;
        cancel.cancel();
        child.done().await;
        ensure_eq(&child.err(), &Some(ContextError::Canceled), "child err")?;
        child_cancel.cancel(Some(HttpError::Static("late")));
        ensure_eq(
            &child.cause(),
            &Some(HttpError::Context(ContextError::Canceled)),
            "first cause wins",
        )
    }

    #[tokio::test]
    async fn deadline_wins_over_late_cancel() -> Result<(), String> {
        let (ctx, cancel) = Context::background().with_timeout(Duration::from_millis(5));
        ctx.done().await;
        cancel.cancel();
        ensure_eq(
            &ctx.err(),
            &Some(ContextError::DeadlineExceeded),
            "deadline",
        )?;
        let (child, _cancel) = ctx.with_cancel();
        ensure_eq(
            &child.err(),
            &Some(ContextError::DeadlineExceeded),
            "child of expired",
        )?;
        let detached = ctx.without_cancel();
        ensure_eq(&detached.err(), &None, "without cancel")
    }

    #[tokio::test]
    async fn cause_and_values() -> Result<(), String> {
        let (ctx, cancel) = Context::background().with_value(7_u32).with_cancel_cause();
        cancel.cancel(Some(HttpError::Static("net/http: request completed")));
        ensure_eq(&ctx.err(), &Some(ContextError::Canceled), "err")?;
        ensure_eq(
            &ctx.cause().map(|e| e.to_string()),
            &Some("net/http: request completed".to_owned()),
            "cause",
        )?;
        ensure_eq(&ctx.value::<u32>(), &Some(&7), "value")?;
        ensure_eq(
            &ctx.without_cancel().value::<u32>(),
            &Some(&7),
            "detached value",
        )
    }
}
