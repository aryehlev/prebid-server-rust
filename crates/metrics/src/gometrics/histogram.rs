//! Go: `histogram.go`.

use std::sync::Arc;

use super::sample::{Sample, SampleSnapshot};

/// Go: `metrics.StandardHistogram`, the statistics of the values recorded in a [`Sample`].
#[derive(Debug)]
pub struct StandardHistogram {
    sample: Sample,
}

impl StandardHistogram {
    /// Go: `&StandardHistogram{sample: s}`.
    pub(super) const fn new(sample: Sample) -> Self {
        Self { sample }
    }

    /// Go: `Count`.
    #[must_use]
    pub fn count(&self) -> i64 {
        self.sample.count()
    }

    /// Go: `Max`.
    #[must_use]
    pub fn max(&self) -> i64 {
        self.sample.max()
    }

    /// Go: `Mean`.
    #[must_use]
    pub fn mean(&self) -> f64 {
        self.sample.mean()
    }

    /// Go: `Min`.
    #[must_use]
    pub fn min(&self) -> i64 {
        self.sample.min()
    }

    /// Go: `Percentile`.
    #[must_use]
    pub fn percentile(&self, p: f64) -> f64 {
        self.sample.percentile(p)
    }

    /// Go: `Percentiles`.
    #[must_use]
    pub fn percentiles(&self, ps: &[f64]) -> Vec<f64> {
        self.sample.percentiles(ps)
    }

    /// Go: `Sample`, the sample the histogram reads.
    #[must_use]
    pub const fn sample(&self) -> &Sample {
        &self.sample
    }

    /// Go: `Snapshot`.
    #[must_use]
    pub fn snapshot(&self) -> HistogramSnapshot {
        HistogramSnapshot {
            sample: self.sample.snapshot(),
        }
    }

    /// Go: `StdDev`.
    #[must_use]
    pub fn std_dev(&self) -> f64 {
        self.sample.std_dev()
    }

    /// Go: `Sum`.
    #[must_use]
    pub fn sum(&self) -> i64 {
        self.sample.sum()
    }

    /// Go: `Update`.
    pub fn update(&self, v: i64) {
        self.sample.update(v);
    }

    /// Go: `Variance`.
    #[must_use]
    pub fn variance(&self) -> f64 {
        self.sample.variance()
    }
}

/// Go: `metrics.HistogramSnapshot`, a read-only copy of a histogram.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistogramSnapshot {
    sample: SampleSnapshot,
}

impl HistogramSnapshot {
    /// Go: `Count`.
    #[must_use]
    pub const fn count(&self) -> i64 {
        self.sample.count()
    }

    /// Go: `Max`.
    #[must_use]
    pub fn max(&self) -> i64 {
        self.sample.max()
    }

    /// Go: `Mean`.
    #[must_use]
    pub fn mean(&self) -> f64 {
        self.sample.mean()
    }

    /// Go: `Min`.
    #[must_use]
    pub fn min(&self) -> i64 {
        self.sample.min()
    }

    /// Go: `Percentile`, which sorts the snapshot's values.
    pub fn percentile(&mut self, p: f64) -> f64 {
        self.sample.percentile(p)
    }

    /// Go: `Percentiles`, which sorts the snapshot's values.
    pub fn percentiles(&mut self, ps: &[f64]) -> Vec<f64> {
        self.sample.percentiles(ps)
    }

    /// Go: `Sample`.
    #[must_use]
    pub const fn sample(&self) -> &SampleSnapshot {
        &self.sample
    }

    /// Go: `StdDev`.
    #[must_use]
    pub fn std_dev(&self) -> f64 {
        self.sample.std_dev()
    }

    /// Go: `Sum`.
    #[must_use]
    pub fn sum(&self) -> i64 {
        self.sample.sum()
    }

    /// Go: `Variance`.
    #[must_use]
    pub fn variance(&self) -> f64 {
        self.sample.variance()
    }
}

/// Go: the `metrics.Histogram` interface: a [`StandardHistogram`], or the no-op
/// `NilHistogram` when `None` (the [`Default`]).
#[derive(Clone, Debug, Default)]
pub struct Histogram(pub Option<Arc<StandardHistogram>>);

impl Histogram {
    /// Go: `metrics.NilHistogram{}`.
    pub const NIL: Self = Self(None);

    /// Go: `Count`.
    #[must_use]
    pub fn count(&self) -> i64 {
        self.0.as_ref().map_or(0, |h| h.count())
    }

    /// Go: `Snapshot` (`NilHistogram`'s is empty).
    #[must_use]
    pub fn snapshot(&self) -> HistogramSnapshot {
        self.0
            .as_ref()
            .map_or_else(HistogramSnapshot::default, |h| h.snapshot())
    }

    /// Go: `Update`.
    pub fn update(&self, v: i64) {
        if let Some(h) = &self.0 {
            h.update(v);
        }
    }
}

/// Go: `metrics.NewHistogram(s)`.
#[must_use]
pub fn new_histogram(sample: Sample) -> Histogram {
    Histogram(Some(Arc::new(StandardHistogram::new(sample))))
}
