//! Go: `registry.go` and the `GetOrRegister*` functions of `counter.go`, `meter.go`,
//! `timer.go` and `histogram.go`.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    sync::{Arc, PoisonError, RwLock, Weak},
};

use super::{
    counter::{Counter, StandardCounter},
    histogram::{Histogram, StandardHistogram},
    meter::{Meter, StandardMeter, new_standard_meter},
    sample::Sample,
    timer::{StandardTimer, Timer, new_standard_timer},
};

/// A metric held by a [`Registry`] (Go: the `interface{}` values of `StandardRegistry.metrics`,
/// which `register` limits to the metric types).
///
/// Two values are equal when they are the same metric, as Go's `==` on the interface values
/// compares the pointers.
#[derive(Clone, Debug)]
pub enum Metric {
    /// Go: `*metrics.StandardCounter`.
    Counter(Arc<StandardCounter>),
    /// Go: `*metrics.StandardMeter`.
    Meter(Arc<StandardMeter>),
    /// Go: `*metrics.StandardTimer`.
    Timer(Arc<StandardTimer>),
    /// Go: `*metrics.StandardHistogram`.
    Histogram(Arc<StandardHistogram>),
}

impl PartialEq for Metric {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Counter(a), Self::Counter(b)) => Arc::ptr_eq(a, b),
            (Self::Meter(a), Self::Meter(b)) => Arc::ptr_eq(a, b),
            (Self::Timer(a), Self::Timer(b)) => Arc::ptr_eq(a, b),
            (Self::Histogram(a), Self::Histogram(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl Eq for Metric {}

impl From<&Counter> for Option<Metric> {
    /// The registry value of a counter handle (`None` for `NilCounter`).
    fn from(c: &Counter) -> Self {
        c.0.clone().map(Metric::Counter)
    }
}

impl From<&Meter> for Option<Metric> {
    /// The registry value of a meter handle (`None` for `NilMeter`).
    fn from(m: &Meter) -> Self {
        m.0.clone().map(Metric::Meter)
    }
}

impl From<&Timer> for Option<Metric> {
    /// The registry value of a timer handle (`None` for `NilTimer`).
    fn from(t: &Timer) -> Self {
        t.0.clone().map(Metric::Timer)
    }
}

impl From<&Histogram> for Option<Metric> {
    /// The registry value of a histogram handle (`None` for `NilHistogram`).
    fn from(h: &Histogram) -> Self {
        h.0.clone().map(Metric::Histogram)
    }
}

/// Go: `metrics.DuplicateMetric`, the error `Register` returns for a name already registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateMetric(pub String);

impl fmt::Display for DuplicateMetric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Go: registry.go:15
        write!(f, "duplicate metric: {}", self.0)
    }
}

impl Error for DuplicateMetric {}

type Metrics = RwLock<BTreeMap<String, Metric>>;

/// Go: `metrics.StandardRegistry` (from `NewRegistry`), or a `metrics.PrefixedRegistry` over a
/// new one (from `NewPrefixedRegistry`): the metrics by name, behind a read-write lock.
///
/// Clones share the metrics. A prefixed registry prepends its prefix to the names it is given;
/// [`Registry::each`] passes the full names, of the metrics that have the prefix.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    prefix: String,
    metrics: Arc<Metrics>,
}

impl Registry {
    /// Go: `metrics.NewRegistry()`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Go: `metrics.NewPrefixedRegistry(prefix)`.
    #[must_use]
    pub fn new_prefixed(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_owned(),
            metrics: Arc::default(),
        }
    }

    /// The prefix prepended to every name (empty for `NewRegistry`).
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    fn real_name(&self, name: &str) -> String {
        // Go: registry.go:294 (realName := r.prefix + name)
        let mut real_name = String::with_capacity(self.prefix.len() + name.len());
        real_name.push_str(&self.prefix);
        real_name.push_str(name);
        real_name
    }

    /// Go: `Each`: calls `f` with every registered metric whose name has the prefix, by full
    /// name. The metrics are copied under the read lock first, as in Go, and passed in ascending
    /// name order (Go's map order is random).
    pub fn each<F: FnMut(&str, &Metric)>(&self, mut f: F) {
        // Go: registry.go:221 (registered)
        let metrics: Vec<(String, Metric)> = self
            .metrics
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(name, metric)| (name.clone(), metric.clone()))
            .collect();
        // Go: registry.go:267 (the PrefixedRegistry filter)
        for (name, metric) in &metrics {
            if name.starts_with(&self.prefix) {
                f(name, metric);
            }
        }
    }

    /// Go: `Get`: the metric registered under the (prefixed) name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Metric> {
        // Go: registry.go:75
        self.metrics
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&self.real_name(name))
            .cloned()
    }

    /// Go: `GetOrRegister`: the metric registered under the (prefixed) name, or the one `ctor`
    /// makes, registered under it. `ctor` runs under the write lock, as Go calls a constructor
    /// function.
    pub fn get_or_register<F: FnOnce() -> Metric>(&self, name: &str, ctor: F) -> Metric {
        // Go: registry.go:85
        let real_name = self.real_name(name);
        if let Some(metric) = self
            .metrics
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&real_name)
        {
            return metric.clone();
        }
        let mut metrics = self.metrics.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(metric) = metrics.get(&real_name) {
            return metric.clone();
        }
        let metric = ctor();
        drop(metrics.insert(real_name, metric.clone()));
        drop(metrics);
        metric
    }

    /// Go: `Register`: registers `metric` under the (prefixed) name, or returns
    /// [`DuplicateMetric`] (with the full name) when the name is taken.
    ///
    /// # Errors
    ///
    /// [`DuplicateMetric`] when a metric is already registered under the name.
    pub fn register(&self, name: &str, metric: Metric) -> Result<(), DuplicateMetric> {
        // Go: registry.go:109 and registry.go:205
        let real_name = self.real_name(name);
        let mut metrics = self.metrics.write().unwrap_or_else(PoisonError::into_inner);
        if metrics.contains_key(&real_name) {
            return Err(DuplicateMetric(real_name));
        }
        drop(metrics.insert(real_name, metric));
        drop(metrics);
        Ok(())
    }

    /// Go: `metrics.GetOrRegisterCounter(name, r)`. Go panics when another type of metric holds
    /// the name; the port returns `NilCounter`.
    #[must_use]
    pub fn get_or_register_counter(&self, name: &str) -> Counter {
        // Go: counter.go:16
        match self.get_or_register(name, || {
            Metric::Counter(Arc::new(StandardCounter::default()))
        }) {
            Metric::Counter(c) => Counter(Some(c)),
            Metric::Meter(_) | Metric::Timer(_) | Metric::Histogram(_) => Counter::NIL,
        }
    }

    /// Go: `metrics.GetOrRegisterMeter(name, r)`. Go panics when another type of metric holds
    /// the name; the port returns `NilMeter`.
    #[must_use]
    pub fn get_or_register_meter(&self, name: &str) -> Meter {
        // Go: meter.go:27
        match self.get_or_register(name, || Metric::Meter(new_standard_meter())) {
            Metric::Meter(m) => Meter(Some(m)),
            Metric::Counter(_) | Metric::Timer(_) | Metric::Histogram(_) => Meter::NIL,
        }
    }

    /// Go: `metrics.GetOrRegisterTimer(name, r)`. Go panics when another type of metric holds
    /// the name; the port returns `NilTimer`.
    #[must_use]
    pub fn get_or_register_timer(&self, name: &str) -> Timer {
        // Go: timer.go:34
        match self.get_or_register(name, || Metric::Timer(Arc::new(new_standard_timer()))) {
            Metric::Timer(t) => Timer(Some(t)),
            Metric::Counter(_) | Metric::Meter(_) | Metric::Histogram(_) => Timer::NIL,
        }
    }

    /// Go: `metrics.GetOrRegisterHistogram(name, r, s)`: the histogram registered under the
    /// name, or a new one over `sample` (which is dropped when the name is taken, as Go drops
    /// it). Go panics when another type of metric holds the name; the port returns
    /// `NilHistogram`.
    #[must_use]
    pub fn get_or_register_histogram(&self, name: &str, sample: Sample) -> Histogram {
        // Go: histogram.go:22
        match self.get_or_register(name, || {
            Metric::Histogram(Arc::new(StandardHistogram::new(sample)))
        }) {
            Metric::Histogram(h) => Histogram(Some(h)),
            Metric::Counter(_) | Metric::Meter(_) | Metric::Timer(_) => Histogram::NIL,
        }
    }

    /// A reference that does not keep the metrics alive (see [`WeakRegistry`]).
    #[must_use]
    pub fn downgrade(&self) -> WeakRegistry {
        WeakRegistry {
            prefix: self.prefix.clone(),
            metrics: Arc::downgrade(&self.metrics),
        }
    }
}

/// A [`Registry`] reference that does not keep its metrics alive, for background tasks that
/// should end with the registry's last owner (Go's reporter goroutines run forever).
#[derive(Clone, Debug)]
pub struct WeakRegistry {
    prefix: String,
    metrics: Weak<Metrics>,
}

impl WeakRegistry {
    /// The registry, unless every [`Registry`] clone has been dropped.
    #[must_use]
    pub fn upgrade(&self) -> Option<Registry> {
        self.metrics.upgrade().map(|metrics| Registry {
            prefix: self.prefix.clone(),
            metrics,
        })
    }
}
