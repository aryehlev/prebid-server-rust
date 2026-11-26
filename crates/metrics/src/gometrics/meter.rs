//! Go: `meter.go`.

use std::{
    sync::{
        Arc, Mutex, PoisonError, Weak,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use super::ewma::{StandardEwma, new_ewma1, new_ewma5, new_ewma15};
use crate::{float_from_i64, metrics::GoDuration};

/// Go: `metrics.StandardMeter`: counts events and derives one-, five- and fifteen-minute
/// moving-average rates and the mean rate since the meter was created.
#[derive(Debug)]
pub struct StandardMeter {
    // Go: the fields of `snapshot *MeterSnapshot`, updated atomically.
    count: AtomicI64,
    rate1: AtomicU64,
    rate5: AtomicU64,
    rate15: AtomicU64,
    rate_mean: AtomicU64,
    a1: StandardEwma,
    a5: StandardEwma,
    a15: StandardEwma,
    start_time: Instant,
}

impl StandardMeter {
    /// Go: `newStandardMeter`.
    fn new() -> Self {
        Self {
            count: AtomicI64::new(0),
            rate1: AtomicU64::new(0),
            rate5: AtomicU64::new(0),
            rate15: AtomicU64::new(0),
            rate_mean: AtomicU64::new(0),
            a1: new_ewma1(),
            a5: new_ewma5(),
            a15: new_ewma15(),
            start_time: Instant::now(),
        }
    }

    /// Go: `Count`, the number of events recorded.
    #[must_use]
    pub fn count(&self) -> i64 {
        self.count.load(Ordering::SeqCst)
    }

    /// Go: `Mark`, which records `n` events (the count wraps like Go's `atomic.AddInt64`).
    pub fn mark(&self, n: i64) {
        let _previous = self.count.fetch_add(n, Ordering::SeqCst);
        self.a1.update(n);
        self.a5.update(n);
        self.a15.update(n);
        self.update_snapshot();
    }

    /// Go: `Rate1`.
    #[must_use]
    pub fn rate1(&self) -> f64 {
        f64::from_bits(self.rate1.load(Ordering::SeqCst))
    }

    /// Go: `Rate5`.
    #[must_use]
    pub fn rate5(&self) -> f64 {
        f64::from_bits(self.rate5.load(Ordering::SeqCst))
    }

    /// Go: `Rate15`.
    #[must_use]
    pub fn rate15(&self) -> f64 {
        f64::from_bits(self.rate15.load(Ordering::SeqCst))
    }

    /// Go: `RateMean`.
    #[must_use]
    pub fn rate_mean(&self) -> f64 {
        f64::from_bits(self.rate_mean.load(Ordering::SeqCst))
    }

    /// Go: `Snapshot`.
    #[must_use]
    pub fn snapshot(&self) -> MeterSnapshot {
        MeterSnapshot {
            count: self.count.load(Ordering::SeqCst),
            rate1: self.rate1.load(Ordering::SeqCst),
            rate5: self.rate5.load(Ordering::SeqCst),
            rate15: self.rate15.load(Ordering::SeqCst),
            rate_mean: self.rate_mean.load(Ordering::SeqCst),
        }
    }

    /// Go: `updateSnapshot`: the EWMA rates, and the mean rate
    /// `float64(m.Count()) / time.Since(m.startTime).Seconds()`.
    fn update_snapshot(&self) {
        let one_minute = self.a1.rate().to_bits();
        let five_minutes = self.a5.rate().to_bits();
        let fifteen_minutes = self.a15.rate().to_bits();
        let elapsed = GoDuration::from(self.start_time.elapsed()).seconds();
        let mean = (float_from_i64(self.count()) / elapsed).to_bits();
        self.rate1.store(one_minute, Ordering::SeqCst);
        self.rate5.store(five_minutes, Ordering::SeqCst);
        self.rate15.store(fifteen_minutes, Ordering::SeqCst);
        self.rate_mean.store(mean, Ordering::SeqCst);
    }

    /// Go: `tick`, the arbiter's five-second tick.
    pub(super) fn tick(&self) {
        self.a1.tick();
        self.a5.tick();
        self.a15.tick();
        self.update_snapshot();
    }
}

/// Go: `metrics.MeterSnapshot`, a read-only copy of a meter (the rates as IEEE-754 bits).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeterSnapshot {
    count: i64,
    rate1: u64,
    rate5: u64,
    rate15: u64,
    rate_mean: u64,
}

impl MeterSnapshot {
    /// Go: `Count`.
    #[must_use]
    pub const fn count(&self) -> i64 {
        self.count
    }

    /// Go: `Rate1`.
    #[must_use]
    pub const fn rate1(&self) -> f64 {
        f64::from_bits(self.rate1)
    }

    /// Go: `Rate5`.
    #[must_use]
    pub const fn rate5(&self) -> f64 {
        f64::from_bits(self.rate5)
    }

    /// Go: `Rate15`.
    #[must_use]
    pub const fn rate15(&self) -> f64 {
        f64::from_bits(self.rate15)
    }

    /// Go: `RateMean`.
    #[must_use]
    pub const fn rate_mean(&self) -> f64 {
        f64::from_bits(self.rate_mean)
    }
}

/// Go: the `metrics.Meter` interface: a [`StandardMeter`], or the no-op `NilMeter` when `None`
/// (the [`Default`]).
#[derive(Clone, Debug, Default)]
pub struct Meter(pub Option<Arc<StandardMeter>>);

impl Meter {
    /// Go: `&metrics.NilMeter{}`.
    pub const NIL: Self = Self(None);

    /// Go: `Count`.
    #[must_use]
    pub fn count(&self) -> i64 {
        self.0.as_ref().map_or(0, |m| m.count())
    }

    /// Go: `Mark`.
    pub fn mark(&self, n: i64) {
        if let Some(m) = &self.0 {
            m.mark(n);
        }
    }

    /// Go: `Rate1`.
    #[must_use]
    pub fn rate1(&self) -> f64 {
        self.0.as_ref().map_or(0.0, |m| m.rate1())
    }

    /// Go: `Rate5`.
    #[must_use]
    pub fn rate5(&self) -> f64 {
        self.0.as_ref().map_or(0.0, |m| m.rate5())
    }

    /// Go: `Rate15`.
    #[must_use]
    pub fn rate15(&self) -> f64 {
        self.0.as_ref().map_or(0.0, |m| m.rate15())
    }

    /// Go: `RateMean`.
    #[must_use]
    pub fn rate_mean(&self) -> f64 {
        self.0.as_ref().map_or(0.0, |m| m.rate_mean())
    }

    /// Go: `Snapshot` (`NilMeter`'s is all zeros).
    #[must_use]
    pub fn snapshot(&self) -> MeterSnapshot {
        self.0
            .as_ref()
            .map_or_else(MeterSnapshot::default, |m| m.snapshot())
    }
}

/// Go: the arbiter's ticker period, `time.NewTicker(5e9)`.
pub(super) const TICK: Duration = Duration::from_secs(5);

/// The arbiter's meter list is pruned of dropped meters when it reaches this length (and then
/// at twice the length that remains).
const PRUNE_AT: usize = 1024;

/// Go: `meterArbiter`: the meters to tick and whether the ticking thread runs.
#[derive(Debug)]
struct Arbiter {
    started: bool,
    meters: Vec<Weak<StandardMeter>>,
    prune_at: usize,
}

/// Go: `var arbiter = meterArbiter{...}`.
static ARBITER: Mutex<Arbiter> = Mutex::new(Arbiter {
    started: false,
    meters: Vec::new(),
    prune_at: PRUNE_AT,
});

/// Go: `NewMeter`'s body: a standard meter, added to the arbiter, whose thread starts with the
/// first meter.
pub(super) fn new_standard_meter() -> Arc<StandardMeter> {
    let meter = Arc::new(StandardMeter::new());
    let mut arbiter = ARBITER.lock().unwrap_or_else(PoisonError::into_inner);
    if arbiter.meters.len() >= arbiter.prune_at {
        arbiter.meters.retain(|m| m.strong_count() > 0);
        arbiter.prune_at = PRUNE_AT.max(arbiter.meters.len().saturating_mul(2));
    }
    arbiter.meters.push(Arc::downgrade(&meter));
    if !arbiter.started {
        // Go starts the goroutine once; a thread that cannot start is tried again with the next
        // meter.
        arbiter.started = thread::Builder::new()
            .name("go-metrics-arbiter".to_owned())
            .spawn(tick_meters_forever)
            .is_ok();
    }
    drop(arbiter);
    meter
}

/// Go: `meterArbiter.tick`: ticks every meter every five seconds, until no meter is left.
fn tick_meters_forever() {
    let mut next = Instant::now().checked_add(TICK);
    while let Some(deadline) = next {
        // A tick that fell due while the meters were ticked is taken at once: Go's ticker holds
        // one (and drops the ones after it).
        thread::sleep(deadline.saturating_duration_since(Instant::now()));
        next = next_tick(deadline, Instant::now());
        let Some(meters) = live_meters() else {
            return;
        };
        // Go: tickMeters.
        for meter in &meters {
            meter.tick();
        }
        drop(meters);
    }
    // Only an instant past what `Instant` can hold ends the schedule early.
    stop_arbiter();
}

/// The tick after the one due at `deadline` and taken at `now`: the first instant of the
/// five-second grid after `now` (Go: the runtime's `when + period*(1+(now-when)/period)` when it
/// runs a ticker, `runtime/time.go` in Go 1.26).
pub(super) fn next_tick(deadline: Instant, now: Instant) -> Option<Instant> {
    let period = TICK.as_nanos();
    let late = now.saturating_duration_since(deadline).as_nanos();
    let nanos = (late / period + 1).checked_mul(period)?;
    let secs = u64::try_from(nanos / 1_000_000_000).ok()?;
    let subsec = u32::try_from(nanos % 1_000_000_000).ok()?;
    deadline.checked_add(Duration::new(secs, subsec))
}

/// The meters still referenced, or `None` (and the arbiter marked stopped) when none is left.
fn live_meters() -> Option<Vec<Arc<StandardMeter>>> {
    let mut arbiter = ARBITER.lock().unwrap_or_else(PoisonError::into_inner);
    let meters: Vec<Arc<StandardMeter>> = arbiter.meters.iter().filter_map(Weak::upgrade).collect();
    if meters.is_empty() {
        arbiter.meters.clear();
        arbiter.started = false;
        return None;
    }
    arbiter.meters.retain(|m| m.strong_count() > 0);
    drop(arbiter);
    Some(meters)
}

/// Marks the arbiter stopped, so that the next meter starts a new thread.
fn stop_arbiter() {
    ARBITER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .started = false;
}

/// Go: `metrics.NewMeter()`.
#[must_use]
pub fn new_meter() -> Meter {
    Meter(Some(new_standard_meter()))
}
