//! Go: `sample.go`.

use std::{
    cmp::Ordering,
    mem,
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use crate::{float_from_i64, float_from_usize, i64_from_float, metrics::GoDuration};

/// Go: `rescaleThreshold`: the decaying sample rescales its priorities once an hour.
const RESCALE_THRESHOLD: Duration = Duration::from_secs(3600);

/// Go: the `metrics.Sample` implementations histograms are built over.
#[derive(Debug)]
pub enum Sample {
    /// Go: `*metrics.ExpDecaySample`.
    ExpDecay(ExpDecaySample),
    /// Go: `*metrics.UniformSample`.
    Uniform(UniformSample),
}

impl Sample {
    /// Go: `Count`, the number of values recorded (which may exceed the reservoir size).
    #[must_use]
    pub fn count(&self) -> i64 {
        match self {
            Self::ExpDecay(s) => s.count(),
            Self::Uniform(s) => s.count(),
        }
    }

    /// Go: `Max`.
    #[must_use]
    pub fn max(&self) -> i64 {
        match self {
            Self::ExpDecay(s) => sample_max(&s.values()),
            Self::Uniform(s) => s.with_values(|v| sample_max(v)),
        }
    }

    /// Go: `Mean`.
    #[must_use]
    pub fn mean(&self) -> f64 {
        match self {
            Self::ExpDecay(s) => sample_mean(&s.values()),
            Self::Uniform(s) => s.with_values(|v| sample_mean(v)),
        }
    }

    /// Go: `Min`.
    #[must_use]
    pub fn min(&self) -> i64 {
        match self {
            Self::ExpDecay(s) => sample_min(&s.values()),
            Self::Uniform(s) => s.with_values(|v| sample_min(v)),
        }
    }

    /// Go: `Percentile`.
    #[must_use]
    pub fn percentile(&self, p: f64) -> f64 {
        self.percentiles(&[p]).first().copied().unwrap_or_default()
    }

    /// Go: `Percentiles`. The decaying sample sorts a copy of its values; the uniform sample
    /// sorts its reservoir in place (see the module docs).
    #[must_use]
    pub fn percentiles(&self, ps: &[f64]) -> Vec<f64> {
        match self {
            Self::ExpDecay(s) => sample_percentiles(&mut s.values(), ps),
            Self::Uniform(s) => s.with_values(|v| sample_percentiles(v, ps)),
        }
    }

    /// Go: `Snapshot`.
    #[must_use]
    pub fn snapshot(&self) -> SampleSnapshot {
        match self {
            Self::ExpDecay(s) => s.snapshot(),
            Self::Uniform(s) => s.snapshot(),
        }
    }

    /// Go: `StdDev`.
    #[must_use]
    pub fn std_dev(&self) -> f64 {
        match self {
            Self::ExpDecay(s) => sample_std_dev(&s.values()),
            Self::Uniform(s) => s.with_values(|v| sample_std_dev(v)),
        }
    }

    /// Go: `Sum`.
    #[must_use]
    pub fn sum(&self) -> i64 {
        match self {
            Self::ExpDecay(s) => sample_sum(&s.values()),
            Self::Uniform(s) => s.with_values(|v| sample_sum(v)),
        }
    }

    /// Go: `Update`.
    pub fn update(&self, v: i64) {
        match self {
            Self::ExpDecay(s) => s.update(Instant::now(), v),
            Self::Uniform(s) => s.update(v),
        }
    }

    /// Go: `Values`, a copy of the values in the sample (the decaying sample's in heap order).
    #[must_use]
    pub fn values(&self) -> Vec<i64> {
        match self {
            Self::ExpDecay(s) => s.values(),
            Self::Uniform(s) => s.with_values(|v| v.to_vec()),
        }
    }

    /// Go: `Variance`.
    #[must_use]
    pub fn variance(&self) -> f64 {
        match self {
            Self::ExpDecay(s) => sample_variance(&s.values()),
            Self::Uniform(s) => s.with_values(|v| sample_variance(v)),
        }
    }
}

/// Go: `metrics.ExpDecaySample`: an exponentially-decaying sample using a forward-decaying
/// priority reservoir (Cormode et al., "Forward Decay: A Practical Time Decay Model for
/// Streaming Systems").
#[derive(Debug)]
pub struct ExpDecaySample {
    alpha: f64,
    reservoir_size: usize,
    state: Mutex<ExpDecayState>,
}

#[derive(Debug)]
struct ExpDecayState {
    count: i64,
    t0: Instant,
    t1: Instant,
    values: ExpDecaySampleHeap,
}

impl ExpDecaySample {
    fn lock(&self) -> MutexGuard<'_, ExpDecayState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Go: `Count`.
    #[must_use]
    pub fn count(&self) -> i64 {
        self.lock().count
    }

    /// Go: `Snapshot`: the count and a copy of the values in heap order.
    #[must_use]
    pub fn snapshot(&self) -> SampleSnapshot {
        let state = self.lock();
        let values = state.values.s.iter().map(|item| item.v).collect();
        let count = state.count;
        drop(state);
        SampleSnapshot { count, values }
    }

    /// Go: `Values`.
    #[must_use]
    pub fn values(&self) -> Vec<i64> {
        self.lock().values.s.iter().map(|item| item.v).collect()
    }

    /// The priorities in heap order (Go: the `k` of `s.values.Values()`).
    #[cfg(test)]
    pub(super) fn priorities(&self) -> Vec<f64> {
        self.lock().values.s.iter().map(|item| item.k).collect()
    }

    /// Go: `update(t, v)`: the value enters with priority `exp(age * alpha) / rand.Float64()`,
    /// evicting the lowest priority when the reservoir is full, and the priorities are rescaled
    /// once `t` passes the rescale time.
    pub(super) fn update(&self, t: Instant, v: i64) {
        let mut state = self.lock();
        state.count = state.count.wrapping_add(1);
        if state.values.size() == self.reservoir_size {
            state.values.pop();
        }
        // Go: t.Sub(s.t0).Seconds(), negative when another update rescaled between this one's
        // time.Now() and its lock.
        let age = t.checked_duration_since(state.t0).map_or_else(
            || -go_seconds(state.t0.saturating_duration_since(t)),
            go_seconds,
        );
        state.values.push(ExpDecayItem {
            k: (age * self.alpha).exp() / rand::random::<f64>(),
            v,
        });
        if t > state.t1 {
            // Go: values := s.values.Values() aliases the heap that Clear and Push then write,
            // but each element is read before its slot is written, so this is a rebuild.
            let values = mem::take(&mut state.values.s);
            let t0 = state.t0;
            state.t0 = t;
            state.t1 = t.checked_add(RESCALE_THRESHOLD).unwrap_or(t);
            let elapsed = go_seconds(state.t0.saturating_duration_since(t0));
            for mut item in values {
                item.k *= (-self.alpha * elapsed).exp();
                state.values.push(item);
            }
        }
    }
}

/// Go: `Duration.Seconds()` of a monotonic interval.
fn go_seconds(d: Duration) -> f64 {
    GoDuration::from(d).seconds()
}

/// Go: `metrics.NewExpDecaySample(reservoirSize, alpha)`.
#[must_use]
pub fn new_exp_decay_sample(reservoir_size: usize, alpha: f64) -> Sample {
    let t0 = Instant::now();
    Sample::ExpDecay(ExpDecaySample {
        alpha,
        reservoir_size,
        state: Mutex::new(ExpDecayState {
            count: 0,
            t0,
            t1: t0.checked_add(RESCALE_THRESHOLD).unwrap_or(t0),
            values: ExpDecaySampleHeap {
                s: Vec::with_capacity(reservoir_size),
            },
        }),
    })
}

/// Go: `metrics.UniformSample`: a uniform sample using Vitter's Algorithm R.
#[derive(Debug)]
pub struct UniformSample {
    reservoir_size: usize,
    state: Mutex<UniformState>,
}

#[derive(Debug)]
struct UniformState {
    count: i64,
    values: Vec<i64>,
}

impl UniformSample {
    fn lock(&self) -> MutexGuard<'_, UniformState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Go: `Count`.
    #[must_use]
    pub fn count(&self) -> i64 {
        self.lock().count
    }

    /// Runs `f` over the reservoir under the sample's mutex, as Go's statistics methods do.
    fn with_values<T, F: FnOnce(&mut [i64]) -> T>(&self, f: F) -> T {
        let mut state = self.lock();
        let result = f(&mut state.values);
        drop(state);
        result
    }

    /// Go: `Snapshot`: the count and a copy of the reservoir.
    #[must_use]
    pub fn snapshot(&self) -> SampleSnapshot {
        let state = self.lock();
        let snapshot = SampleSnapshot {
            count: state.count,
            values: state.values.clone(),
        };
        drop(state);
        snapshot
    }

    /// Go: `Update`: the value fills the reservoir, then replaces a random slot with
    /// decreasing probability.
    fn update(&self, v: i64) {
        let mut state = self.lock();
        state.count = state.count.wrapping_add(1);
        if state.values.len() < self.reservoir_size {
            state.values.push(v);
        } else if state.count > 0 {
            // Go: rand.Int63n(s.count), whose count is at least 1 here.
            let r = rand::random_range(0..state.count);
            if let Ok(index) = usize::try_from(r)
                && let Some(slot) = state.values.get_mut(index)
            {
                *slot = v;
            }
        }
    }
}

/// Go: `metrics.NewUniformSample(reservoirSize)`.
#[must_use]
pub fn new_uniform_sample(reservoir_size: usize) -> Sample {
    Sample::Uniform(UniformSample {
        reservoir_size,
        state: Mutex::new(UniformState {
            count: 0,
            values: Vec::with_capacity(reservoir_size),
        }),
    })
}

/// Go: `metrics.SampleSnapshot`, a read-only copy of a sample.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SampleSnapshot {
    count: i64,
    values: Vec<i64>,
}

impl SampleSnapshot {
    /// Go: `metrics.NewSampleSnapshot(count, values)`.
    #[must_use]
    pub const fn new(count: i64, values: Vec<i64>) -> Self {
        Self { count, values }
    }

    /// Go: `Count`, the number of values recorded when the snapshot was taken.
    #[must_use]
    pub const fn count(&self) -> i64 {
        self.count
    }

    /// Go: `Max`.
    #[must_use]
    pub fn max(&self) -> i64 {
        sample_max(&self.values)
    }

    /// Go: `Mean`.
    #[must_use]
    pub fn mean(&self) -> f64 {
        sample_mean(&self.values)
    }

    /// Go: `Min`.
    #[must_use]
    pub fn min(&self) -> i64 {
        sample_min(&self.values)
    }

    /// Go: `Percentile`, which sorts the snapshot's values.
    pub fn percentile(&mut self, p: f64) -> f64 {
        sample_percentile(&mut self.values, p)
    }

    /// Go: `Percentiles`, which sorts the snapshot's values (see the module docs).
    pub fn percentiles(&mut self, ps: &[f64]) -> Vec<f64> {
        sample_percentiles(&mut self.values, ps)
    }

    /// Go: `Size`.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.values.len()
    }

    /// Go: `StdDev`.
    #[must_use]
    pub fn std_dev(&self) -> f64 {
        sample_std_dev(&self.values)
    }

    /// Go: `Sum`.
    #[must_use]
    pub fn sum(&self) -> i64 {
        sample_sum(&self.values)
    }

    /// Go: `Values`, a copy of the values.
    #[must_use]
    pub fn values(&self) -> Vec<i64> {
        self.values.clone()
    }

    /// Go: `Variance`.
    #[must_use]
    pub fn variance(&self) -> f64 {
        sample_variance(&self.values)
    }
}

/// Go: `metrics.SampleMax` (0 for no values).
#[must_use]
pub fn sample_max(values: &[i64]) -> i64 {
    if values.is_empty() {
        return 0;
    }
    values.iter().copied().fold(i64::MIN, i64::max)
}

/// Go: `metrics.SampleMean`: `float64(SampleSum(values)) / float64(len(values))` (0 for no
/// values).
#[must_use]
pub fn sample_mean(values: &[i64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    float_from_i64(sample_sum(values)) / float_from_usize(values.len())
}

/// Go: `metrics.SampleMin` (0 for no values).
#[must_use]
pub fn sample_min(values: &[i64]) -> i64 {
    if values.is_empty() {
        return 0;
    }
    values.iter().copied().fold(i64::MAX, i64::min)
}

/// Go: `metrics.SamplePercentile`, which sorts `values`.
pub fn sample_percentile(values: &mut [i64], p: f64) -> f64 {
    sample_percentiles(values, &[p])
        .first()
        .copied()
        .unwrap_or_default()
}

/// Go: `metrics.SamplePercentiles`: sorts `values`, then interpolates each percentile at
/// position `p * (len + 1)` (unfused, as Go's amd64 build computes it).
///
/// Go indexes out of range (and panics) for a NaN percentile, which no caller passes; the
/// port leaves that score 0.
pub fn sample_percentiles(values: &mut [i64], ps: &[f64]) -> Vec<f64> {
    let mut scores = vec![0.0; ps.len()];
    let size = values.len();
    if size > 0 {
        values.sort_unstable();
        let first = values.first().copied().unwrap_or_default();
        let last = values.last().copied().unwrap_or_default();
        for (score, &p) in scores.iter_mut().zip(ps) {
            let pos = p * float_from_usize(size + 1);
            *score = if pos < 1.0 {
                float_from_i64(first)
            } else if pos >= float_from_usize(size) {
                float_from_i64(last)
            } else {
                interpolate(values, pos).unwrap_or_default()
            };
        }
    }
    scores
}

/// Go: `lower + (pos-math.Floor(pos))*(upper-lower)` with `lower` and `upper` the values at
/// `int(pos)-1` and `int(pos)`.
fn interpolate(values: &[i64], pos: f64) -> Option<f64> {
    let index = usize::try_from(i64_from_float(pos)).ok()?;
    let lower = float_from_i64(*values.get(index.checked_sub(1)?)?);
    let upper = float_from_i64(*values.get(index)?);
    let step = (pos - pos.floor()) * (upper - lower);
    Some(lower + step)
}

/// Go: `metrics.SampleStdDev`.
#[must_use]
pub fn sample_std_dev(values: &[i64]) -> f64 {
    sample_variance(values).sqrt()
}

/// Go: `metrics.SampleSum` (wrapping like Go's `int64` addition).
#[must_use]
pub fn sample_sum(values: &[i64]) -> i64 {
    values.iter().fold(0_i64, |sum, &v| sum.wrapping_add(v))
}

/// Go: `metrics.SampleVariance`: the mean of the squared deviations from the mean, summed in the
/// order of `values` (unfused, as Go's amd64 build computes it; 0 for no values).
#[must_use]
pub fn sample_variance(values: &[i64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let m = sample_mean(values);
    let mut sum = 0.0;
    for &v in values {
        let d = float_from_i64(v) - m;
        let square = d * d;
        sum += square;
    }
    sum / float_from_usize(values.len())
}

/// Go: `expDecaySample`, one value in the heap.
#[derive(Clone, Copy, Debug)]
struct ExpDecayItem {
    k: f64,
    v: i64,
}

/// Go: `!(a < b)` on priorities, keeping Go's result for NaN.
fn not_less(a: f64, b: f64) -> bool {
    a.partial_cmp(&b) != Some(Ordering::Less)
}

/// Go: `expDecaySampleHeap`, a min-heap of priorities (Go's copy of `container/heap`).
#[derive(Debug)]
struct ExpDecaySampleHeap {
    s: Vec<ExpDecayItem>,
}

impl ExpDecaySampleHeap {
    const fn size(&self) -> usize {
        self.s.len()
    }

    fn priority(&self, i: usize) -> f64 {
        self.s.get(i).map_or(f64::NAN, |item| item.k)
    }

    fn push(&mut self, item: ExpDecayItem) {
        self.s.push(item);
        self.up(self.s.len() - 1);
    }

    /// Go: `Pop`, which removes the lowest priority (nothing when the heap is empty, where Go
    /// would index out of range).
    fn pop(&mut self) {
        let Some(n) = self.s.len().checked_sub(1) else {
            return;
        };
        self.s.swap(0, n);
        self.down(0, n);
        let _lowest = self.s.pop();
    }

    fn up(&mut self, mut j: usize) {
        while j > 0 {
            let i = (j - 1) / 2; // parent
            if not_less(self.priority(j), self.priority(i)) {
                break;
            }
            self.s.swap(i, j);
            j = i;
        }
    }

    fn down(&mut self, mut i: usize, n: usize) {
        loop {
            let j1 = 2 * i + 1;
            if j1 >= n {
                break;
            }
            let mut j = j1; // left child
            let j2 = j1 + 1;
            if j2 < n && not_less(self.priority(j1), self.priority(j2)) {
                j = j2; // right child
            }
            if not_less(self.priority(j), self.priority(i)) {
                break;
            }
            self.s.swap(i, j);
            i = j;
        }
    }
}
