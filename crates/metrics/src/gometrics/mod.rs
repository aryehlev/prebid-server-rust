//! The metrics library behind the go-metrics engine.
//!
//! Go: `github.com/rcrowley/go-metrics` at cf1acfcdf475 (the package Prebid imports as
//! `gometrics`), the parts Prebid Server uses: the counter, meter, timer and histogram metrics, the
//! exponentially-decaying and uniform samples, the EWMA rates, the registry and its prefixed form,
//! and the arbiter that ticks every meter every five seconds.
//!
//! The Go interfaces (`Counter`, `Meter`, `Timer`, `Histogram`) are handles over an
//! `Option<Arc<Standard*>>`: `Some` is the standard implementation, `None` the no-op `Nil*`
//! implementation. The registry stores the standard implementations as [`Metric`]s.
//!
//! Not ported, because Prebid does not use them: gauges, health checks, `Stop`, `Clear`, `Time`,
//! `UpdateSince`, the default registry, child registries and the reporters other than the
//! `InfluxDB` one ([`crate::influx`]).
//!
//! # Randomness and the clock
//!
//! Go seeds the global `math/rand` source randomly, and the samples read it: the exponentially-
//! decaying sample divides each priority by `rand.Float64()`, and the uniform sample replaces a
//! random slot with `rand.Int63n`. The port draws the same values in the same ranges from the rand
//! crate's thread generator (`rand::random::<f64>()` in [0, 1), `rand::random_range(0..n)`), so
//! which values a full reservoir keeps is random in both, from other numbers
//! (`influx.sampled-values` in the ledger). Meters, EWMA ticks and the decaying sample read the
//! monotonic clock like Go's.
//!
//! # Go aliasing
//!
//! - `SamplePercentiles` sorts the slice it is given. [`sample_percentiles`] sorts its argument,
//!   [`SampleSnapshot::percentiles`] sorts the snapshot (so the statistics read after it see sorted
//!   values) and [`UniformSample`]'s `percentiles` sorts the live reservoir, as Go's do.
//!
//! # Differences from Go
//!
//! - Go panics when `GetOrRegister` finds a metric of another type under the name; the port returns
//!   the no-op handle. No two metric names of the engine collide across types.
//! - The arbiter's first tick comes five seconds after the first meter is created (Go: five-second
//!   ticks counted from package initialisation), it ticks the meters that are still referenced, and
//!   its thread ends when none is left (a later meter starts it again).

mod counter;
mod ewma;
mod histogram;
mod meter;
mod registry;
mod sample;
mod timer;

pub use self::{
    counter::{Counter, CounterSnapshot, StandardCounter, new_counter},
    ewma::{EwmaSnapshot, StandardEwma, new_ewma, new_ewma1, new_ewma5, new_ewma15},
    histogram::{Histogram, HistogramSnapshot, StandardHistogram, new_histogram},
    meter::{Meter, MeterSnapshot, StandardMeter, new_meter},
    registry::{DuplicateMetric, Metric, Registry, WeakRegistry},
    sample::{
        ExpDecaySample, Sample, SampleSnapshot, UniformSample, new_exp_decay_sample,
        new_uniform_sample, sample_max, sample_mean, sample_min, sample_percentile,
        sample_percentiles, sample_std_dev, sample_sum, sample_variance,
    },
    timer::{StandardTimer, Timer, TimerSnapshot, new_timer},
};

#[cfg(test)]
#[path = "gometrics_test.rs"]
mod tests;

#[cfg(test)]
#[path = "golden_test.rs"]
mod golden_tests;
