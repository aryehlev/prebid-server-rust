//! Go: `timer.go`.

use std::sync::{Arc, Mutex, PoisonError};

use super::{
    histogram::{HistogramSnapshot, StandardHistogram},
    meter::{MeterSnapshot, StandardMeter, new_standard_meter},
    sample::new_exp_decay_sample,
};
use crate::metrics::GoDuration;

/// Go: `metrics.StandardTimer`: a histogram of durations (in nanoseconds) over an
/// exponentially-decaying sample, and a meter of the rate at which they are recorded.
#[derive(Debug)]
pub struct StandardTimer {
    histogram: StandardHistogram,
    meter: Arc<StandardMeter>,
    mutex: Mutex<()>,
}

impl StandardTimer {
    /// Go: `Count`, the number of durations recorded.
    #[must_use]
    pub fn count(&self) -> i64 {
        self.histogram.count()
    }

    /// Go: `Max`.
    #[must_use]
    pub fn max(&self) -> i64 {
        self.histogram.max()
    }

    /// Go: `Mean`.
    #[must_use]
    pub fn mean(&self) -> f64 {
        self.histogram.mean()
    }

    /// Go: `Min`.
    #[must_use]
    pub fn min(&self) -> i64 {
        self.histogram.min()
    }

    /// Go: `Percentile`.
    #[must_use]
    pub fn percentile(&self, p: f64) -> f64 {
        self.histogram.percentile(p)
    }

    /// Go: `Percentiles`.
    #[must_use]
    pub fn percentiles(&self, ps: &[f64]) -> Vec<f64> {
        self.histogram.percentiles(ps)
    }

    /// Go: `Rate1`.
    #[must_use]
    pub fn rate1(&self) -> f64 {
        self.meter.rate1()
    }

    /// Go: `Rate5`.
    #[must_use]
    pub fn rate5(&self) -> f64 {
        self.meter.rate5()
    }

    /// Go: `Rate15`.
    #[must_use]
    pub fn rate15(&self) -> f64 {
        self.meter.rate15()
    }

    /// Go: `RateMean`.
    #[must_use]
    pub fn rate_mean(&self) -> f64 {
        self.meter.rate_mean()
    }

    /// Go: `Snapshot`, taken under the timer's mutex.
    #[must_use]
    pub fn snapshot(&self) -> TimerSnapshot {
        let _guard = self.mutex.lock().unwrap_or_else(PoisonError::into_inner);
        TimerSnapshot {
            histogram: self.histogram.snapshot(),
            meter: self.meter.snapshot(),
        }
    }

    /// Go: `StdDev`.
    #[must_use]
    pub fn std_dev(&self) -> f64 {
        self.histogram.std_dev()
    }

    /// Go: `Sum`.
    #[must_use]
    pub fn sum(&self) -> i64 {
        self.histogram.sum()
    }

    /// Go: `Update`: records `int64(d)` and marks the meter, under the timer's mutex.
    pub fn update(&self, d: GoDuration) {
        let _guard = self.mutex.lock().unwrap_or_else(PoisonError::into_inner);
        self.histogram.update(d.as_nanos());
        self.meter.mark(1);
    }

    /// Go: `Variance`.
    #[must_use]
    pub fn variance(&self) -> f64 {
        self.histogram.variance()
    }
}

/// Go: `metrics.TimerSnapshot`, a read-only copy of a timer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimerSnapshot {
    histogram: HistogramSnapshot,
    meter: MeterSnapshot,
}

impl TimerSnapshot {
    /// Go: `Count`.
    #[must_use]
    pub const fn count(&self) -> i64 {
        self.histogram.count()
    }

    /// Go: `Max`.
    #[must_use]
    pub fn max(&self) -> i64 {
        self.histogram.max()
    }

    /// Go: `Mean`.
    #[must_use]
    pub fn mean(&self) -> f64 {
        self.histogram.mean()
    }

    /// Go: `Min`.
    #[must_use]
    pub fn min(&self) -> i64 {
        self.histogram.min()
    }

    /// Go: `Percentile`, which sorts the snapshot's values.
    pub fn percentile(&mut self, p: f64) -> f64 {
        self.histogram.percentile(p)
    }

    /// Go: `Percentiles`, which sorts the snapshot's values.
    pub fn percentiles(&mut self, ps: &[f64]) -> Vec<f64> {
        self.histogram.percentiles(ps)
    }

    /// Go: `Rate1`.
    #[must_use]
    pub const fn rate1(&self) -> f64 {
        self.meter.rate1()
    }

    /// Go: `Rate5`.
    #[must_use]
    pub const fn rate5(&self) -> f64 {
        self.meter.rate5()
    }

    /// Go: `Rate15`.
    #[must_use]
    pub const fn rate15(&self) -> f64 {
        self.meter.rate15()
    }

    /// Go: `RateMean`.
    #[must_use]
    pub const fn rate_mean(&self) -> f64 {
        self.meter.rate_mean()
    }

    /// Go: `StdDev`.
    #[must_use]
    pub fn std_dev(&self) -> f64 {
        self.histogram.std_dev()
    }

    /// Go: `Sum`.
    #[must_use]
    pub fn sum(&self) -> i64 {
        self.histogram.sum()
    }

    /// Go: `Variance`.
    #[must_use]
    pub fn variance(&self) -> f64 {
        self.histogram.variance()
    }
}

/// Go: the `metrics.Timer` interface: a [`StandardTimer`], or the no-op `NilTimer` when `None`
/// (the [`Default`]).
#[derive(Clone, Debug, Default)]
pub struct Timer(pub Option<Arc<StandardTimer>>);

impl Timer {
    /// Go: `&metrics.NilTimer{}`.
    pub const NIL: Self = Self(None);

    /// Go: `Count`.
    #[must_use]
    pub fn count(&self) -> i64 {
        self.0.as_ref().map_or(0, |t| t.count())
    }

    /// Go: `Snapshot` (`NilTimer`'s is empty).
    #[must_use]
    pub fn snapshot(&self) -> TimerSnapshot {
        self.0
            .as_ref()
            .map_or_else(TimerSnapshot::default, |t| t.snapshot())
    }

    /// Go: `Update`.
    pub fn update(&self, d: GoDuration) {
        if let Some(t) = &self.0 {
            t.update(d);
        }
    }
}

/// Go: `NewTimer`'s body: an exponentially-decaying sample of 1028 values with alpha 0.015
/// (Go: `NewExpDecaySample(1028, 0.015)`) and a new meter.
pub(super) fn new_standard_timer() -> StandardTimer {
    let histogram = StandardHistogram::new(new_exp_decay_sample(1028, 0.015));
    StandardTimer {
        histogram,
        meter: new_standard_meter(),
        mutex: Mutex::new(()),
    }
}

/// Go: `metrics.NewTimer()`.
#[must_use]
pub fn new_timer() -> Timer {
    Timer(Some(Arc::new(new_standard_timer())))
}
