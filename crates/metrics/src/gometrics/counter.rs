//! Go: `counter.go`.

use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

/// Go: `metrics.StandardCounter`, an `int64` updated atomically.
#[derive(Debug, Default)]
pub struct StandardCounter {
    count: AtomicI64,
}

impl StandardCounter {
    /// Go: `Count`.
    #[must_use]
    pub fn count(&self) -> i64 {
        self.count.load(Ordering::SeqCst)
    }

    /// Go: `Dec`, which adds `-i` (wrapping like Go's `atomic.AddInt64`).
    pub fn dec(&self, i: i64) {
        let _previous = self.count.fetch_add(i.wrapping_neg(), Ordering::SeqCst);
    }

    /// Go: `Inc` (wrapping like Go's `atomic.AddInt64`).
    pub fn inc(&self, i: i64) {
        let _previous = self.count.fetch_add(i, Ordering::SeqCst);
    }

    /// Go: `Snapshot`.
    #[must_use]
    pub fn snapshot(&self) -> CounterSnapshot {
        CounterSnapshot(self.count())
    }
}

/// Go: `metrics.CounterSnapshot`, a read-only copy of a counter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CounterSnapshot(i64);

impl CounterSnapshot {
    /// Go: `Count`, the count when the snapshot was taken.
    #[must_use]
    pub const fn count(self) -> i64 {
        self.0
    }
}

/// Go: the `metrics.Counter` interface: a [`StandardCounter`], or the no-op `NilCounter` when
/// `None` (the [`Default`]).
#[derive(Clone, Debug, Default)]
pub struct Counter(pub Option<Arc<StandardCounter>>);

impl Counter {
    /// Go: `metrics.NilCounter{}`.
    pub const NIL: Self = Self(None);

    /// Go: `Count` (0 for `NilCounter`).
    #[must_use]
    pub fn count(&self) -> i64 {
        self.0.as_ref().map_or(0, |c| c.count())
    }

    /// Go: `Dec`.
    pub fn dec(&self, i: i64) {
        if let Some(c) = &self.0 {
            c.dec(i);
        }
    }

    /// Go: `Inc`.
    pub fn inc(&self, i: i64) {
        if let Some(c) = &self.0 {
            c.inc(i);
        }
    }

    /// Go: `Snapshot` (`NilCounter`'s counts 0).
    #[must_use]
    pub fn snapshot(&self) -> CounterSnapshot {
        self.0
            .as_ref()
            .map_or_else(CounterSnapshot::default, |c| c.snapshot())
    }
}

/// Go: `metrics.NewCounter()`.
#[must_use]
pub fn new_counter() -> Counter {
    Counter(Some(Arc::new(StandardCounter::default())))
}
