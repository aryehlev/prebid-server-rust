//! Go: the tests of `github.com/rcrowley/go-metrics` cf1acfcdf475 (`counter_test.go`,
//! `ewma_test.go`, `histogram_test.go`, `meter_test.go`, `registry_test.go`, `sample_test.go`,
//! `timer_test.go`) whose results depend neither on the clock nor on `math/rand`.
//!
//! Not transliterated:
//!
//! - the tests of what the port leaves out (`Clear`, `Stop`, `Time`, `Unregister`, gauges, the
//!   default and child registries, `WalkRegistries`), and the `-race` concurrency tests;
//! - `TestMeterDecay`, which runs a private arbiter on a one-millisecond ticker;
//! - the statistics of full reservoirs (`TestExpDecaySampleSnapshot`,
//!   `TestExpDecaySampleStatistics`, `TestUniformSampleSnapshot`, `TestUniformSampleStatistics`):
//!   Go seeds `math/rand` with 1 and pins which values the reservoir keeps. The statistics
//!   functions themselves are pinned by `testdata/golden/gometrics/samples.txt`.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use super::{
    Metric, Registry, Sample, StandardEwma, new_counter, new_ewma1, new_ewma5, new_ewma15,
    new_exp_decay_sample, new_histogram, new_meter, new_timer, new_uniform_sample,
};
use crate::{float_from_i64, float_from_usize, metrics::GoDuration, testutil::TestResult};

/// Go: `elapseMinute`: twelve five-second ticks.
fn elapse_minute(a: &StandardEwma) {
    for _ in 0..12 {
        a.tick();
    }
}

/// Go: `TestEWMA1`, `TestEWMA5` and `TestEWMA15`: three events, one tick, then the rate after
/// each of fifteen minutes.
fn check_ewma(name: &str, a: &StandardEwma, expected: &[f64; 16]) -> TestResult {
    a.update(3);
    a.tick();
    for (minute, &rate) in expected.iter().enumerate() {
        if minute > 0 {
            elapse_minute(a);
        }
        ensure_eq!(a.rate(), rate, "{name}: {minute} minute a.Rate()");
    }
    Ok(())
}

/// Go: `TestEWMA1`.
#[test]
fn test_ewma1() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 ewma_test.go:47 (TestEWMA1)
    let expected = [
        0.6,
        0.220_727_664_702_865_53,
        0.081_201_169_941_967_72,
        0.029_872_241_020_718_428,
        0.010_989_383_333_240_54,
        0.004_042_768_199_451_294,
        0.001_487_251_305_999_821_2,
        0.000_547_129_179_332_712_2,
        0.000_201_277_576_741_508_15,
        7.404_588_245_200_814e-5,
        2.723_995_785_749_108_3e-5,
        1.002_102_047_414_746_2e-5,
        3.686_527_411_996_952_5e-6,
        1.356_197_644_188_643_3e-6,
        4.989_172_314_621_449e-7,
        1.835_413_923_010_972_2e-7,
    ];
    check_ewma("EWMA1", &new_ewma1(), &expected)
}

/// Go: `TestEWMA5`.
#[test]
fn test_ewma5() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 ewma_test.go:116 (TestEWMA5)
    let expected = [
        0.6,
        0.491_238_451_846_789_05,
        0.402_192_027_621_383_7,
        0.329_286_981_656_415_96,
        0.269_597_378_470_333,
        0.220_727_664_702_865_4,
        0.180_716_527_147_321_28,
        0.147_958_178_364_963_92,
        0.121_137_910_796_793_26,
        0.099_179_332_932_951_93,
        0.081_201_169_941_967_63,
        0.066_481_895_017_400_36,
        0.054_430_771_973_647_52,
        0.044_564_146_928_600_35,
        0.036_486_037_575_130_79,
        0.029_872_241_020_718_383,
    ];
    check_ewma("EWMA5", &new_ewma5(), &expected)
}

/// Go: `TestEWMA15`.
#[test]
fn test_ewma15() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 ewma_test.go:185 (TestEWMA15)
    let expected = [
        0.6,
        0.561_304_191_018_970_6,
        0.525_103_991_425_768_4,
        0.491_238_451_846_788_8,
        0.459_557_003_018_789,
        0.429_918_786_344_273_2,
        0.402_192_027_621_383_1,
        0.376_253_451_163_833_13,
        0.351_987_731_706_018_5,
        0.329_286_981_656_415_3,
        0.308_050_271_419_554_6,
        0.288_183_180_653_878_9,
        0.269_597_378_470_332_16,
        0.252_210_230_705_208_3,
        0.235_944_432_521_158_15,
        0.220_727_664_702_864_62,
    ];
    check_ewma("EWMA15", &new_ewma15(), &expected)
}

/// Go: `TestCounterDec1`, `TestCounterDec2`, `TestCounterInc1`, `TestCounterInc2` and
/// `TestCounterZero`.
#[test]
fn test_counter_inc_dec_zero() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 counter_test.go:22-69 (TestCounterDec1 to
    // TestCounterZero)
    ensure_eq!(new_counter().count(), 0, "c.Count()");
    for (dec, expected) in [(1, -1), (2, -2)] {
        let decremented = new_counter();
        decremented.dec(dec);
        ensure_eq!(decremented.count(), expected, "c.Count() after Dec({dec})");
    }
    for (inc, expected) in [(1, 1), (2, 2)] {
        let incremented = new_counter();
        incremented.inc(inc);
        ensure_eq!(incremented.count(), expected, "c.Count() after Inc({inc})");
    }
    Ok(())
}

/// Go: `TestCounterSnapshot`.
#[test]
fn test_counter_snapshot() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 counter_test.go:54 (TestCounterSnapshot)
    let c = new_counter();
    c.inc(1);
    let snapshot = c.snapshot();
    c.inc(1);
    ensure_eq!(snapshot.count(), 1, "c.Count()");
    Ok(())
}

/// Go: `TestGetOrRegisterCounter` (`NewRegisteredCounter` is `Register` of a new counter).
#[test]
fn test_get_or_register_counter() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 counter_test.go:71 (TestGetOrRegisterCounter)
    let r = Registry::new();
    let c = new_counter();
    r.register("foo", Metric::Counter(c.0.clone().ok_or("nil counter")?))?;
    c.inc(47);
    ensure_eq!(r.get_or_register_counter("foo").count(), 47, "count");
    Ok(())
}

/// Go: `TestGetOrRegisterHistogram`.
#[test]
fn test_get_or_register_histogram() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 histogram_test.go:13 (TestGetOrRegisterHistogram)
    let r = Registry::new();
    let h = new_histogram(new_uniform_sample(100));
    r.register(
        "foo",
        Metric::Histogram(h.0.clone().ok_or("nil histogram")?),
    )?;
    h.update(47);
    let registered = r.get_or_register_histogram("foo", new_uniform_sample(100));
    ensure_eq!(registered.count(), 1, "count");
    Ok(())
}

/// Go: `testHistogram10000`, over the live histogram or its snapshot.
macro_rules! check_histogram_10000 {
    ($h:expr) => {{
        let h = $h;
        ensure_eq!(h.count(), 10000, "h.Count()");
        ensure_eq!(h.min(), 1, "h.Min()");
        ensure_eq!(h.max(), 10000, "h.Max()");
        ensure_eq!(h.mean(), 5000.5, "h.Mean()");
        ensure_eq!(h.std_dev(), 2_886.751_331_514_372, "h.StdDev()");
        let ps = h.percentiles(&[0.5, 0.75, 0.99]);
        ensure_eq!(ps, vec![5000.5, 7500.75, 9900.99], "percentiles");
    }};
}

/// Go: `TestHistogram10000`.
#[test]
fn test_histogram_10000() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 histogram_test.go:22 (TestHistogram10000)
    let h = new_histogram(new_uniform_sample(100_000));
    for i in 1..=10000 {
        h.update(i);
    }
    let standard = h.0.as_ref().ok_or("nil histogram")?;
    check_histogram_10000!(Arc::clone(standard));
    Ok(())
}

/// Go: `TestHistogramEmpty`.
#[test]
fn test_histogram_empty() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 histogram_test.go:30 (TestHistogramEmpty)
    let h = new_histogram(new_uniform_sample(100));
    let h = h.0.as_ref().ok_or("nil histogram")?;
    ensure_eq!(h.count(), 0, "h.Count()");
    ensure_eq!(h.min(), 0, "h.Min()");
    ensure_eq!(h.max(), 0, "h.Max()");
    ensure_eq!(h.mean(), 0.0, "h.Mean()");
    ensure_eq!(h.std_dev(), 0.0, "h.StdDev()");
    let ps = h.percentiles(&[0.5, 0.75, 0.99]);
    ensure_eq!(ps, vec![0.0, 0.0, 0.0], "percentiles");
    Ok(())
}

/// Go: `TestHistogramSnapshot`.
#[test]
fn test_histogram_snapshot() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 histogram_test.go:59 (TestHistogramSnapshot)
    let h = new_histogram(new_uniform_sample(100_000));
    for i in 1..=10000 {
        h.update(i);
    }
    let mut snapshot = h.snapshot();
    h.update(0);
    check_histogram_10000!(&mut snapshot);
    Ok(())
}

/// Go: `TestGetOrRegisterMeter`.
#[test]
fn test_get_or_register_meter() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 meter_test.go:55 (TestGetOrRegisterMeter)
    let r = Registry::new();
    let m = new_meter();
    r.register("foo", Metric::Meter(m.0.clone().ok_or("nil meter")?))?;
    m.mark(47);
    ensure_eq!(r.get_or_register_meter("foo").count(), 47, "count");
    Ok(())
}

/// Go: `TestMeterNonzero` and `TestMeterZero`.
#[test]
fn test_meter_zero_nonzero() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 meter_test.go:79 and :107 (TestMeterNonzero,
    // TestMeterZero)
    ensure_eq!(new_meter().count(), 0, "m.Count()");
    let m = new_meter();
    m.mark(3);
    ensure_eq!(m.count(), 3, "m.Count()");
    Ok(())
}

/// Go: `TestMeterSnapshot`: the mean rate is the one of the last update, in both.
#[test]
fn test_meter_snapshot() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 meter_test.go:99 (TestMeterSnapshot)
    let m = new_meter();
    m.mark(1);
    let snapshot = m.snapshot();
    ensure_eq!(m.rate_mean(), snapshot.rate_mean(), "snapshot");
    Ok(())
}

/// Go: `TestGetOrRegisterTimer`.
#[test]
fn test_get_or_register_timer() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 timer_test.go:18 (TestGetOrRegisterTimer)
    let r = Registry::new();
    let t = new_timer();
    r.register("foo", Metric::Timer(t.0.clone().ok_or("nil timer")?))?;
    t.update(GoDuration::from_nanos(47));
    ensure_eq!(r.get_or_register_timer("foo").count(), 1, "count");
    Ok(())
}

/// Go: `TestTimerExtremes`.
#[test]
fn test_timer_extremes() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 timer_test.go:26 (TestTimerExtremes)
    let tm = new_timer();
    tm.update(GoDuration::from_nanos(i64::MAX));
    tm.update(GoDuration::from_nanos(0));
    let tm = tm.0.as_ref().ok_or("nil timer")?;
    ensure_eq!(tm.std_dev(), 4.611_686_018_427_388e18, "tm.StdDev()");
    Ok(())
}

/// Go: `TestTimerZero`.
#[test]
fn test_timer_zero() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 timer_test.go:55 (TestTimerZero)
    let tm = new_timer();
    let tm = tm.0.as_ref().ok_or("nil timer")?;
    ensure_eq!(tm.count(), 0, "tm.Count()");
    ensure_eq!(tm.min(), 0, "tm.Min()");
    ensure_eq!(tm.max(), 0, "tm.Max()");
    ensure_eq!(tm.mean(), 0.0, "tm.Mean()");
    ensure_eq!(tm.std_dev(), 0.0, "tm.StdDev()");
    let ps = tm.percentiles(&[0.5, 0.75, 0.99]);
    ensure_eq!(ps, vec![0.0, 0.0, 0.0], "percentiles");
    ensure_eq!(tm.rate1(), 0.0, "tm.Rate1()");
    ensure_eq!(tm.rate5(), 0.0, "tm.Rate5()");
    ensure_eq!(tm.rate15(), 0.0, "tm.Rate15()");
    ensure_eq!(tm.rate_mean(), 0.0, "tm.RateMean()");
    Ok(())
}

/// The names and kinds `each` passes.
fn each_names(r: &Registry) -> Vec<(String, &'static str)> {
    let mut names = Vec::new();
    r.each(|name, metric| {
        let kind = match metric {
            Metric::Counter(_) => "counter",
            Metric::Meter(_) => "meter",
            Metric::Timer(_) => "timer",
            Metric::Histogram(_) => "histogram",
        };
        names.push((name.to_owned(), kind));
    });
    names
}

/// Go: `TestRegistry`, up to `Unregister`.
#[test]
fn test_registry() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 registry_test.go:43 (TestRegistry)
    let r = Registry::new();
    r.register(
        "foo",
        Metric::Counter(new_counter().0.ok_or("nil counter")?),
    )?;
    ensure_eq!(each_names(&r), vec![("foo".to_owned(), "counter")], "Each");
    Ok(())
}

/// Go: `TestRegistryDuplicate`, with a meter where Go registers a gauge.
#[test]
fn test_registry_duplicate() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 registry_test.go:67 (TestRegistryDuplicate)
    let r = Registry::new();
    r.register(
        "foo",
        Metric::Counter(new_counter().0.ok_or("nil counter")?),
    )?;
    let Err(duplicate) = r.register("foo", Metric::Meter(new_meter().0.ok_or("nil meter")?)) else {
        return Err("the second Register succeeded".into());
    };
    ensure_eq!(
        duplicate.to_string(),
        "duplicate metric: foo",
        "DuplicateMetric"
    );
    ensure_eq!(each_names(&r), vec![("foo".to_owned(), "counter")], "Each");
    Ok(())
}

/// Go: `TestRegistryGet`.
#[test]
fn test_registry_get() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 registry_test.go:87 (TestRegistryGet)
    let r = Registry::new();
    r.register(
        "foo",
        Metric::Counter(new_counter().0.ok_or("nil counter")?),
    )?;
    let Some(Metric::Counter(c)) = r.get("foo") else {
        return Err("foo is not a counter".into());
    };
    ensure_eq!(c.count(), 0, "count");
    c.inc(1);
    let Some(Metric::Counter(again)) = r.get("foo") else {
        return Err("foo is not a counter".into());
    };
    ensure_eq!(again.count(), 1, "count");
    Ok(())
}

/// Go: `TestRegistryGetOrRegister` and `TestRegistryGetOrRegisterWithLazyInstantiation`: the
/// first metric wins, and the constructor of the second is not called.
#[test]
fn test_registry_get_or_register() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 registry_test.go:99 and :124
    // (TestRegistryGetOrRegister*)
    let r = Registry::new();
    let counter = new_counter().0.ok_or("nil counter")?;
    let first = r.get_or_register("foo", || Metric::Counter(counter));
    ensure!(matches!(first, Metric::Counter(_)), "foo is a counter");
    let meter = new_meter().0.ok_or("nil meter")?;
    let mut called = false;
    let m = r.get_or_register("foo", || {
        called = true;
        Metric::Meter(meter)
    });
    ensure!(matches!(m, Metric::Counter(_)), "the first metric wins");
    ensure!(!called, "the second constructor is not called");
    ensure_eq!(each_names(&r), vec![("foo".to_owned(), "counter")], "Each");
    Ok(())
}

/// Go: `TestPrefixedRegistryGetOrRegister` and `TestPrefixedRegistryRegister` (without the
/// `DefaultRegistry` registration).
#[test]
fn test_prefixed_registry_get_or_register_and_register() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 registry_test.go:184 and :201
    let r = Registry::new_prefixed("prefix.");
    let _counter = r.get_or_register_counter("foo");
    ensure_eq!(
        each_names(&r),
        vec![("prefix.foo".to_owned(), "counter")],
        "Each"
    );

    let registered = Registry::new_prefixed("prefix.");
    registered.register(
        "foo",
        Metric::Counter(new_counter().0.ok_or("nil counter")?),
    )?;
    ensure_eq!(
        each_names(&registered),
        vec![("prefix.foo".to_owned(), "counter")],
        "Each"
    );
    Ok(())
}

/// Go: `TestPrefixedRegistryGet`.
#[test]
fn test_prefixed_registry_get() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 registry_test.go:250 (TestPrefixedRegistryGet)
    let pr = Registry::new_prefixed("prefix.");
    let name = "foo";
    pr.register(name, Metric::Counter(new_counter().0.ok_or("nil counter")?))?;
    ensure!(pr.get(name).is_some(), "{name}");
    Ok(())
}

/// The values of a sample, checked as `TestExpDecaySample10` and the like check them: the count,
/// the size, and every value in `[0, max]`.
fn check_sample_values(s: &Sample, count: i64, size: usize, max: i64) -> TestResult {
    ensure_eq!(s.count(), count, "s.Count()");
    ensure_eq!(s.snapshot().size(), size, "s.Size()");
    let values = s.values();
    ensure_eq!(values.len(), size, "len(s.Values())");
    for v in values {
        ensure!((0..=max).contains(&v), "out of range [0, {max}): {v}");
    }
    Ok(())
}

/// Go: `TestExpDecaySample10`, `TestExpDecaySample100` and `TestExpDecaySample1000`.
#[test]
fn test_exp_decay_sample_sizes() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 sample_test.go:81, :103 and :125 (TestExpDecaySample10
    // to 1000)
    for (reservoir_size, alpha, updates, size) in [
        (100, 0.99, 10, 10),
        (1000, 0.01, 100, 100),
        (100, 0.99, 1000, 100),
    ] {
        let s = new_exp_decay_sample(reservoir_size, alpha);
        for i in 0..updates {
            s.update(i);
        }
        check_sample_values(&s, updates, size, updates)?;
    }
    Ok(())
}

/// Go: `TestExpDecaySampleNanosecondRegression`: priorities grow with the seconds since the start,
/// not the nanoseconds (which would make them `+Inf` and freeze the sample). Go seeds `math/rand`
/// so that its bound, a mean in `[14, 16]`, holds; with a random seed that bound fails about once
/// in 300 runs, so the port checks that the sample keeps both values: a mean strictly between 10
/// and 20 (the chance of a sample of one value is about 10^-58). The second half of the values is
/// recorded one millisecond later, explicitly, where Go sleeps.
#[test]
fn test_exp_decay_sample_nanosecond_regression() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 sample_test.go:151
    let s = new_exp_decay_sample(100, 0.99);
    let Sample::ExpDecay(sample) = &s else {
        return Err("not an exponentially-decaying sample".into());
    };
    let now = Instant::now();
    for _ in 0..100 {
        sample.update(now, 10);
    }
    let later = now
        .checked_add(Duration::from_millis(1))
        .ok_or("instant overflow")?;
    for _ in 0..100 {
        sample.update(later, 20);
    }
    let v = s.values();
    let avg = v.iter().map(|&x| float_from_i64(x)).sum::<f64>() / float_from_usize(v.len());
    ensure!(avg > 10.0 && avg < 20.0, "out of range (10, 20): {avg}");
    Ok(())
}

/// Go: `TestExpDecaySampleRescale`: a rescale an hour later keeps the priorities above zero.
#[test]
fn test_exp_decay_sample_rescale() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 sample_test.go:172 (TestExpDecaySampleRescale)
    let s = new_exp_decay_sample(2, 0.001);
    let Sample::ExpDecay(sample) = &s else {
        return Err("not an exponentially-decaying sample".into());
    };
    let now = Instant::now();
    sample.update(now, 1);
    let later = now
        .checked_add(Duration::from_secs(3600) + Duration::from_micros(1))
        .ok_or("instant overflow")?;
    sample.update(later, 1);
    for k in sample.priorities() {
        ensure!(k != 0.0, "v.k == 0.0");
    }
    Ok(())
}

/// Go: `TestUniformSample`.
#[test]
fn test_uniform_sample() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 sample_test.go:205 (TestUniformSample)
    let s = new_uniform_sample(100);
    for i in 0..1000 {
        s.update(i);
    }
    check_sample_values(&s, 1000, 100, 1000)
}

/// Go: `TestUniformSampleIncludesTail`: a reservoir that never fills keeps every value.
#[test]
fn test_uniform_sample_includes_tail() -> TestResult {
    // Go: rcrowley/go-metrics cf1acfcdf475 sample_test.go:227 (TestUniformSampleIncludesTail)
    let s = new_uniform_sample(100);
    let max = 100;
    for i in 0..max {
        s.update(i);
    }
    let sum: i64 = s.values().iter().sum();
    ensure_eq!(sum, (max - 1) * max / 2, "sum");
    Ok(())
}
