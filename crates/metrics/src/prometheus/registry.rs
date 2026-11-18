//! The subset of the Go Prometheus client the engine uses: descriptors, counter and histogram
//! vectors, the registry, and the gathered data model.
//!
//! Go: `github.com/prometheus/client_golang/prometheus` v1.12.1 (`desc.go`, `counter.go`,
//! `histogram.go`, `vec.go`, `registry.go`, `internal/metric.go`) and the data model of
//! `github.com/prometheus/client_model` v0.2.0.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::goconv;

/// Go: `prometheus.Labels`, a label name to label value map.
pub(crate) type PromLabels = BTreeMap<String, String>;

/// A registration failure. Go's `MustRegister` panics with this error text at startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationError {
    message: String,
}

impl RegistrationError {
    const fn new(message: String) -> Self {
        Self { message }
    }
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for RegistrationError {}

/// Go: `prometheus.BuildFQName`.
#[must_use]
pub(crate) fn build_fq_name(namespace: &str, subsystem: &str, name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    match (namespace.is_empty(), subsystem.is_empty()) {
        (false, false) => format!("{namespace}_{subsystem}_{name}"),
        (false, true) => format!("{namespace}_{name}"),
        (true, false) => format!("{subsystem}_{name}"),
        (true, true) => name.to_owned(),
    }
}

/// Go: `model.IsValidMetricName` (`github.com/prometheus/common/model`).
fn is_valid_metric_name(name: &str) -> bool {
    !name.is_empty()
        && name.char_indices().all(|(i, c)| {
            c.is_ascii_alphabetic() || c == '_' || c == ':' || (c.is_ascii_digit() && i > 0)
        })
}

/// Go: `checkLabelName` — `model.LabelName.IsValid` and no reserved `__` prefix.
fn is_valid_label_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .char_indices()
            .all(|(i, c)| c.is_ascii_alphabetic() || c == '_' || (c.is_ascii_digit() && i > 0))
        && !name.starts_with("__")
}

/// A metric descriptor.
///
/// Go: `prometheus.Desc`. Constant labels are never used by the engine and are not modelled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Desc {
    fq_name: String,
    help: String,
    /// The variable label names in declaration order.
    variable_labels: Vec<String>,
    /// `sorted[k] = variable_labels[order[k]]`: label pairs are reported sorted by name.
    order: Vec<usize>,
}

impl Desc {
    /// Go: `prometheus.NewDesc`, with the validation `Registry.Register` reports.
    pub(crate) fn new(
        fq_name: String,
        help: &str,
        variable_labels: &[&str],
    ) -> Result<Self, RegistrationError> {
        let mut desc = Self {
            fq_name,
            help: help.to_owned(),
            variable_labels: variable_labels.iter().map(|l| (*l).to_owned()).collect(),
            order: Vec::new(),
        };
        let invalid = |d: &Self, reason: String| {
            RegistrationError::new(format!("descriptor {d} is invalid: {reason}"))
        };
        if !is_valid_metric_name(&desc.fq_name) {
            let reason = format!(
                "{} is not a valid metric name",
                goconv::quote(&desc.fq_name)
            );
            return Err(invalid(&desc, reason));
        }
        for label in variable_labels {
            if !is_valid_label_name(label) {
                let reason = format!(
                    "{} is not a valid label name for metric {}",
                    goconv::quote(label),
                    goconv::quote(&desc.fq_name)
                );
                return Err(invalid(&desc, reason));
            }
        }
        let distinct: BTreeSet<&str> = variable_labels.iter().copied().collect();
        if distinct.len() != variable_labels.len() {
            return Err(invalid(&desc, "duplicate label names".to_owned()));
        }
        let mut order: Vec<usize> = (0..desc.variable_labels.len()).collect();
        order.sort_by(|a, b| {
            desc.variable_labels
                .get(*a)
                .cmp(&desc.variable_labels.get(*b))
        });
        desc.order = order;
        Ok(desc)
    }

    /// The label names in the order the label pairs are reported (sorted by name).
    fn sorted_label_names(&self) -> impl Iterator<Item = &String> {
        self.order
            .iter()
            .filter_map(|i| self.variable_labels.get(*i))
    }

    /// Reorders label values given in declaration order into reporting order.
    fn sorted_key(&self, values: &[&str]) -> Option<Vec<String>> {
        if values.len() != self.variable_labels.len() {
            return None;
        }
        self.order
            .iter()
            .map(|i| values.get(*i).map(|v| (*v).to_owned()))
            .collect()
    }

    /// Go: `MetricVec.With(prometheus.Labels)` resolution of a label map into values.
    fn key_from_labels(&self, labels: &PromLabels) -> Option<Vec<String>> {
        if labels.len() != self.variable_labels.len() {
            return None;
        }
        self.sorted_label_names()
            .map(|name| labels.get(name).cloned())
            .collect()
    }
}

impl fmt::Display for Desc {
    /// Go: `(*Desc).String`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Desc{{fqName: {}, help: {}, constLabels: {{}}, variableLabels: [{}]}}",
            goconv::quote(&self.fq_name),
            goconv::quote(&self.help),
            self.variable_labels.join(" ")
        )
    }
}

fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One counter series.
///
/// Go: `prometheus.counter` — an integer part for whole increments and a float part for the
/// rest, reported as their sum.
#[derive(Debug, Default)]
pub(crate) struct Counter {
    val_int: AtomicU64,
    val_bits: AtomicU64,
}

impl Counter {
    /// Go: `(*counter).Inc`.
    pub(crate) fn inc(&self) {
        let _previous = self.val_int.fetch_add(1, Ordering::Relaxed);
    }

    /// Go: `(*counter).Add`. Go panics with "counter cannot decrease in value" for a negative
    /// value; no Go caller passes one, and the Rust port ignores it instead of panicking.
    pub(crate) fn add(&self, v: f64) {
        if v < 0.0 {
            return;
        }
        if let Some(ival) = goconv::f64_to_u64_exact(v) {
            let _previous = self.val_int.fetch_add(ival, Ordering::Relaxed);
            return;
        }
        let mut current = self.val_bits.load(Ordering::Relaxed);
        loop {
            let next = (f64::from_bits(current) + v).to_bits();
            match self.val_bits.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    }

    /// Go: `(*counter).get` — `float64(valInt) + float64frombits(valBits)`.
    pub(crate) fn value(&self) -> f64 {
        let int = goconv::u64_to_f64(self.val_int.load(Ordering::Relaxed));
        int + f64::from_bits(self.val_bits.load(Ordering::Relaxed))
    }
}

/// The mutable state of one histogram series.
#[derive(Debug)]
struct HistogramState {
    /// Non-cumulative counts per upper bound.
    buckets: Vec<u64>,
    count: u64,
    sum: f64,
}

/// One histogram series.
///
/// Go: `prometheus.histogram`.
#[derive(Debug)]
pub(crate) struct HistogramChild {
    upper_bounds: Arc<[f64]>,
    state: Mutex<HistogramState>,
}

impl HistogramChild {
    fn new(upper_bounds: Arc<[f64]>) -> Self {
        let buckets = vec![0; upper_bounds.len()];
        Self {
            upper_bounds,
            state: Mutex::new(HistogramState {
                buckets,
                count: 0,
                sum: 0.0,
            }),
        }
    }

    /// Go: `(*histogram).Observe`. The bucket is `sort.SearchFloat64s(upperBounds, v)`, the
    /// first bound `>= v`; a value above every bound (or NaN) only counts in `+Inf`.
    pub(crate) fn observe(&self, v: f64) {
        let index = self
            .upper_bounds
            .partition_point(|bound| v.is_nan() || *bound < v);
        let mut state = lock(&self.state);
        if let Some(bucket) = state.buckets.get_mut(index) {
            *bucket = bucket.wrapping_add(1);
        }
        state.count = state.count.wrapping_add(1);
        state.sum += v;
    }

    /// Go: `(*histogram).Write`.
    pub(crate) fn snapshot(&self) -> Histogram {
        let state = lock(&self.state);
        let mut cumulative = 0_u64;
        let buckets = self
            .upper_bounds
            .iter()
            .zip(&state.buckets)
            .map(|(upper_bound, count)| {
                cumulative = cumulative.wrapping_add(*count);
                Bucket {
                    cumulative_count: cumulative,
                    upper_bound: *upper_bound,
                }
            })
            .collect();
        Histogram {
            sample_count: state.count,
            sample_sum: state.sum,
            buckets,
        }
    }
}

/// A family of counter series with the same name and label names.
///
/// Go: `prometheus.CounterVec` (and `prometheus.Counter`, a vector without labels whose only
/// series exists from the start).
#[derive(Debug)]
pub(crate) struct CounterVec {
    desc: Desc,
    children: RwLock<BTreeMap<Vec<String>, Arc<Counter>>>,
}

impl CounterVec {
    fn new(desc: Desc) -> Self {
        let children = if desc.variable_labels.is_empty() {
            BTreeMap::from([(Vec::new(), Arc::new(Counter::default()))])
        } else {
            BTreeMap::new()
        };
        Self {
            desc,
            children: RwLock::new(children),
        }
    }

    fn child(&self, key: Vec<String>) -> Arc<Counter> {
        if let Some(child) = read_lock(&self.children).get(&key) {
            return Arc::clone(child);
        }
        Arc::clone(write_lock(&self.children).entry(key).or_default())
    }

    /// Go: `WithLabelValues(values...)` — the series for label values in declaration order,
    /// created when missing. `None` when the number of values is wrong (Go panics).
    pub(crate) fn with_label_values(&self, values: &[&str]) -> Option<Arc<Counter>> {
        self.desc.sorted_key(values).map(|key| self.child(key))
    }

    /// Go: `With(prometheus.Labels)`. `None` when the label names do not match (Go panics).
    pub(crate) fn with(&self, labels: &PromLabels) -> Option<Arc<Counter>> {
        self.desc.key_from_labels(labels).map(|key| self.child(key))
    }

    /// Increments the series for `values` (declaration order).
    pub(crate) fn inc(&self, values: &[&str]) {
        if let Some(child) = self.with_label_values(values) {
            child.inc();
        }
    }

    /// Adds `v` to the series for `values` (declaration order).
    pub(crate) fn add(&self, values: &[&str], v: f64) {
        if let Some(child) = self.with_label_values(values) {
            child.add(v);
        }
    }

    /// Creates the series for `labels` with a zero value (Go: `counter.With(labels)` whose
    /// result is discarded, as `preload.go` does).
    pub(crate) fn touch(&self, labels: &PromLabels) {
        drop(self.with(labels));
    }

    /// The series currently present as gathered data, in reporting order.
    pub(crate) fn metrics(&self) -> Vec<Metric> {
        read_lock(&self.children)
            .iter()
            .map(|(key, counter)| Metric {
                labels: label_pairs(&self.desc, key),
                value: MetricValue::Counter(counter.value()),
            })
            .collect()
    }
}

/// A family of histogram series with the same name, label names and buckets.
///
/// Go: `prometheus.HistogramVec` (and `prometheus.Histogram`, a vector without labels whose
/// only series exists from the start).
#[derive(Debug)]
pub(crate) struct HistogramVec {
    desc: Desc,
    upper_bounds: Arc<[f64]>,
    children: RwLock<BTreeMap<Vec<String>, Arc<HistogramChild>>>,
}

impl HistogramVec {
    /// Go: `newHistogram` bucket handling: the bounds must increase strictly, and a trailing
    /// `+Inf` bound is implicit and removed.
    fn new(desc: Desc, buckets: &[f64]) -> Result<Self, RegistrationError> {
        if desc.variable_labels.iter().any(|l| l == "le") {
            return Err(RegistrationError::new(
                "\"le\" is not allowed as label name in histograms".to_owned(),
            ));
        }
        let mut bounds = buckets.to_vec();
        for pair in buckets.windows(2) {
            if let [lower, upper] = pair
                && lower >= upper
            {
                return Err(RegistrationError::new(format!(
                    "histogram buckets must be in increasing order: {lower:.6} >= {upper:.6}"
                )));
            }
        }
        if bounds.last().is_some_and(|b| b.is_infinite() && *b > 0.0) {
            let _implicit_inf = bounds.pop();
        }
        let upper_bounds: Arc<[f64]> = bounds.into();
        let children = if desc.variable_labels.is_empty() {
            BTreeMap::from([(
                Vec::new(),
                Arc::new(HistogramChild::new(Arc::clone(&upper_bounds))),
            )])
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            desc,
            upper_bounds,
            children: RwLock::new(children),
        })
    }

    fn child(&self, key: Vec<String>) -> Arc<HistogramChild> {
        if let Some(child) = read_lock(&self.children).get(&key) {
            return Arc::clone(child);
        }
        let bounds = Arc::clone(&self.upper_bounds);
        Arc::clone(
            write_lock(&self.children)
                .entry(key)
                .or_insert_with(|| Arc::new(HistogramChild::new(bounds))),
        )
    }

    /// Go: `WithLabelValues(values...)`.
    pub(crate) fn with_label_values(&self, values: &[&str]) -> Option<Arc<HistogramChild>> {
        self.desc.sorted_key(values).map(|key| self.child(key))
    }

    /// Go: `With(prometheus.Labels)`.
    pub(crate) fn with(&self, labels: &PromLabels) -> Option<Arc<HistogramChild>> {
        self.desc.key_from_labels(labels).map(|key| self.child(key))
    }

    /// Observes `v` in the series for `values` (declaration order).
    pub(crate) fn observe(&self, values: &[&str], v: f64) {
        if let Some(child) = self.with_label_values(values) {
            child.observe(v);
        }
    }

    /// Creates the series for `labels` with no observations.
    pub(crate) fn touch(&self, labels: &PromLabels) {
        drop(self.with(labels));
    }

    /// The series currently present as gathered data, in reporting order.
    pub(crate) fn metrics(&self) -> Vec<Metric> {
        read_lock(&self.children)
            .iter()
            .map(|(key, child)| Metric {
                labels: label_pairs(&self.desc, key),
                value: MetricValue::Histogram(Box::new(child.snapshot())),
            })
            .collect()
    }
}

fn label_pairs(desc: &Desc, key: &[String]) -> Vec<LabelPair> {
    desc.sorted_label_names()
        .zip(key)
        .map(|(name, value)| LabelPair {
            name: name.clone(),
            value: value.clone(),
        })
        .collect()
}

#[derive(Clone, Debug)]
enum Collector {
    Counter(Arc<CounterVec>),
    Histogram(Arc<HistogramVec>),
}

impl Collector {
    fn desc(&self) -> &Desc {
        match self {
            Self::Counter(c) => &c.desc,
            Self::Histogram(h) => &h.desc,
        }
    }
}

/// A metrics registry that gathers its families the way Go's `prometheus.Registry` does.
///
/// Go: `prometheus.Registry` (the engine's `Gatherer`).
#[derive(Debug, Default)]
pub struct Registry {
    collectors: Vec<Collector>,
}

impl Registry {
    /// An empty registry.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Go: `Registry.Register` descriptor consistency checks for one new descriptor.
    fn check(&self, desc: &Desc) -> Result<(), RegistrationError> {
        for existing in &self.collectors {
            let other = existing.desc();
            if other.fq_name != desc.fq_name {
                continue;
            }
            let mut a: Vec<&String> = other.variable_labels.iter().collect();
            let mut b: Vec<&String> = desc.variable_labels.iter().collect();
            a.sort();
            b.sort();
            if other.help != desc.help || a != b {
                return Err(RegistrationError::new(format!(
                    "a previously registered descriptor with the same fully-qualified name as \
                     {desc} has different label names or a different help string"
                )));
            }
            return Err(RegistrationError::new(
                "duplicate metrics collector registration attempted".to_owned(),
            ));
        }
        Ok(())
    }

    /// Registers a counter family (Go: `NewCounterVec`/`NewCounter` + `MustRegister`).
    pub(crate) fn register_counter(
        &mut self,
        desc: Desc,
    ) -> Result<Arc<CounterVec>, RegistrationError> {
        self.check(&desc)?;
        let vec = Arc::new(CounterVec::new(desc));
        self.collectors.push(Collector::Counter(Arc::clone(&vec)));
        Ok(vec)
    }

    /// Registers a histogram family (Go: `NewHistogramVec`/`NewHistogram` + `MustRegister`).
    pub(crate) fn register_histogram(
        &mut self,
        desc: Desc,
        buckets: &[f64],
    ) -> Result<Arc<HistogramVec>, RegistrationError> {
        self.check(&desc)?;
        let vec = Arc::new(HistogramVec::new(desc, buckets)?);
        self.collectors.push(Collector::Histogram(Arc::clone(&vec)));
        Ok(vec)
    }

    /// Go: `Registry.Gather` — the families with at least one series, sorted by name, each
    /// with its series sorted by label values (`internal.NormalizeMetricFamilies`).
    #[must_use]
    pub fn gather(&self) -> Vec<MetricFamily> {
        let mut families: Vec<MetricFamily> = self
            .collectors
            .iter()
            .filter_map(|collector| {
                let (desc, metric_type, metrics) = match collector {
                    Collector::Counter(c) => (&c.desc, MetricType::Counter, c.metrics()),
                    Collector::Histogram(h) => (&h.desc, MetricType::Histogram, h.metrics()),
                };
                (!metrics.is_empty()).then(|| MetricFamily {
                    name: desc.fq_name.clone(),
                    help: desc.help.clone(),
                    metric_type,
                    metrics,
                })
            })
            .collect();
        families.sort_by(|a, b| a.name.cmp(&b.name));
        families
    }
}

/// The type of a metric family.
///
/// Go: `dto.MetricType` (only the types the engine produces).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MetricType {
    /// Go: `dto.MetricType_COUNTER`.
    Counter,
    /// Go: `dto.MetricType_HISTOGRAM`.
    Histogram,
}

/// A gathered metric family.
///
/// Go: `dto.MetricFamily`.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricFamily {
    /// The fully-qualified name.
    pub name: String,
    /// The help string.
    pub help: String,
    /// The metric type.
    pub metric_type: MetricType,
    /// The series, sorted by label values.
    pub metrics: Vec<Metric>,
}

/// One gathered series.
///
/// Go: `dto.Metric`.
#[derive(Clone, Debug, PartialEq)]
pub struct Metric {
    /// The label pairs, sorted by name.
    pub labels: Vec<LabelPair>,
    /// The value.
    pub value: MetricValue,
}

impl Metric {
    /// The value of the label called `name`.
    #[must_use]
    pub fn label(&self, name: &str) -> Option<&str> {
        self.labels
            .iter()
            .find(|l| l.name == name)
            .map(|l| l.value.as_str())
    }
}

/// A label name and value.
///
/// Go: `dto.LabelPair`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LabelPair {
    /// The label name.
    pub name: String,
    /// The label value.
    pub value: String,
}

/// The value of a gathered series.
#[derive(Clone, Debug, PartialEq)]
pub enum MetricValue {
    /// Go: `dto.Counter.Value`.
    Counter(f64),
    /// Go: `dto.Histogram`.
    Histogram(Box<Histogram>),
}

/// A gathered histogram. The default is Go's zero `dto.Histogram`.
///
/// Go: `dto.Histogram`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Histogram {
    /// Go: `SampleCount`.
    pub sample_count: u64,
    /// Go: `SampleSum`.
    pub sample_sum: f64,
    /// Go: `Bucket`, without the implicit `+Inf` bucket.
    pub buckets: Vec<Bucket>,
}

/// A histogram bucket.
///
/// Go: `dto.Bucket`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bucket {
    /// Go: `CumulativeCount`.
    pub cumulative_count: u64,
    /// Go: `UpperBound`.
    pub upper_bound: f64,
}
