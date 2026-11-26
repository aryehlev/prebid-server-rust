//! Differential tests of the go-metrics library against `testdata/golden/gometrics`, which
//! `testdata/go/gometrics.go` writes with Go's library (`writeEWMA`, `writeSamples`) and
//! `testdata/go/arbiter` with Go's runtime (`ticks.txt`).

use std::{
    error::Error,
    fmt::Write as _,
    time::{Duration, Instant},
};

use super::{
    StandardEwma,
    ewma::{M1_ALPHA, M5_ALPHA, M15_ALPHA},
    meter::{TICK, next_tick},
    new_ewma1, new_ewma5, new_ewma15, sample_max, sample_mean, sample_min, sample_percentiles,
    sample_std_dev, sample_sum, sample_variance,
};
use crate::testutil::{TestResult, first_difference, fnv1a64, gometrics_golden as golden};

/// Go: the `ps` of `writeSamples`.
const PERCENTILES: [f64; 10] = [0.5, 0.75, 0.95, 0.99, 0.999, 0.9999, 0.0, 0.1, 0.25, 1.0];

/// The IEEE-754 bits of `f` as 16 hex digits (Go: `%016x` of `math.Float64bits`).
fn hex(f: f64) -> String {
    format!("{:016x}", f.to_bits())
}

/// The EWMA of `minutes` (Go: `ctors[n]`).
fn new_ewma(minutes: &str) -> Result<StandardEwma, Box<dyn Error>> {
    match minutes {
        "1" => Ok(new_ewma1()),
        "5" => Ok(new_ewma5()),
        "15" => Ok(new_ewma15()),
        _ => Err(format!("no {minutes}-minute EWMA").into()),
    }
}

/// `ewma.txt`: the alphas, and the rate after every tick of the scripted `Update`/`Tick`
/// sequences.
#[test]
fn ewma_matches_go() -> TestResult {
    // Go: testdata/go/gometrics.go:332 (writeEWMA, over rcrowley/go-metrics cf1acfcdf475 ewma.go)
    let mut sequences = 0;
    for line in golden("ewma.txt")?.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["alpha", minutes, bits] => {
                let alpha = match *minutes {
                    "1" => M1_ALPHA,
                    "5" => M5_ALPHA,
                    "15" => M15_ALPHA,
                    _ => return Err(format!("no {minutes}-minute alpha").into()),
                };
                ensure_eq!(hex(alpha), *bits, "alpha {minutes}");
            }
            ["ewma", minutes, ops, rates] => {
                let e = new_ewma(minutes)?;
                let mut actual = Vec::new();
                for op in ops.split(',') {
                    if op == "t" {
                        e.tick();
                        actual.push(hex(e.rate()));
                        ensure!(
                            e.snapshot().rate().to_bits() == e.rate().to_bits(),
                            "snapshot rate differs"
                        );
                    } else {
                        let n = op.strip_prefix('u').ok_or("bad op")?.parse::<i64>()?;
                        e.update(n);
                    }
                }
                ensure_eq!(
                    actual.join(","),
                    *rates,
                    "{minutes}-minute EWMA after {ops}"
                );
                sequences += 1;
            }
            _ => return Err(format!("malformed row {line:?}").into()),
        }
    }
    ensure_eq!(sequences, 24 * 3, "sequences");
    Ok(())
}

/// Go: `splitmix64`, the generator of the sample inputs.
struct SplitMix64(u64);

impl SplitMix64 {
    const fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Go: `sampleValues`, the value slice of one `samples.txt` row.
fn sample_values(seed: u64) -> Result<Vec<i64>, Box<dyn Error>> {
    let mut r = SplitMix64(seed);
    let n = match seed % 10 {
        0 => 0,
        1 => 1,
        2 => 2,
        9 => r.next() % 3000,
        _ => r.next() % 60,
    };
    let mode = r.next() % 5;
    let mut values = Vec::with_capacity(usize::try_from(n)?);
    for _ in 0..n {
        let x = r.next();
        values.push(match mode {
            0 => i64::try_from(x % 201)? - 100,
            1 => x.cast_signed(),
            2 => i64::MAX - i64::try_from(x % 1000)?,
            3 => i64::try_from(x % 5_000_000_000)?,
            _ if x.is_multiple_of(2) => i64::MIN + i64::try_from(x % 1000)?,
            _ => i64::try_from(x % 7)?,
        });
    }
    Ok(values)
}

/// Go: one row of `writeSamples`: the statistics of the values, the percentiles (which sort
/// them), and the variance of the sorted values.
fn samples_row(seed: u64) -> Result<String, Box<dyn Error>> {
    let mut values = sample_values(seed)?;
    let mut printed = String::new();
    for v in &values {
        writeln!(printed, "{v}")?;
    }
    let (max, min, sum) = (
        sample_max(&values),
        sample_min(&values),
        sample_sum(&values),
    );
    let (mean, variance, std_dev) = (
        sample_mean(&values),
        sample_variance(&values),
        sample_std_dev(&values),
    );
    let percentiles: Vec<String> = sample_percentiles(&mut values, &PERCENTILES)
        .into_iter()
        .map(hex)
        .collect();
    let sorted_variance = sample_variance(&values);
    Ok(format!(
        "{seed}\t{}\t{:016x}\t{max}\t{min}\t{sum}\t{}\t{}\t{}\t{}\t{}\n",
        values.len(),
        fnv1a64(printed.as_bytes()),
        hex(mean),
        hex(variance),
        hex(std_dev),
        percentiles.join(","),
        hex(sorted_variance),
    ))
}

/// `samples.txt`: `SampleMax`, `SampleMin`, `SampleSum`, `SampleMean`, `SampleVariance`,
/// `SampleStdDev` and `SamplePercentiles` bit for bit (Go's amd64 build, which does not fuse
/// multiplications and additions).
#[test]
fn samples_match_go() -> TestResult {
    // Go: testdata/go/gometrics.go:439 (writeSamples, over rcrowley/go-metrics cf1acfcdf475
    // sample.go)
    let mut actual = String::new();
    for seed in 0..300 {
        actual.push_str(&samples_row(seed)?);
    }
    let expected = golden("samples.txt")?;
    ensure!(
        actual == expected,
        "samples.txt: {}",
        first_difference(&expected, &actual)
    );
    Ok(())
}

/// `ticks.txt` (`TestArbiterTicks` of `testdata/go/arbiter`): when the meter arbiter takes its
/// ticks while ticking the meters takes the scripted times, on a virtual clock, through the steps
/// of the arbiter's thread: sleep to the deadline (not at all when it is past), take the next
/// deadline, tick the meters. A tick that falls due while the meters are ticked is taken at once,
/// as Go's ticker holds one.
#[test]
fn arbiter_ticks_match_go() -> TestResult {
    // Go: testdata/go/arbiter/arbiter_test.go:29 (TestArbiterTicks, rcrowley/go-metrics
    // cf1acfcdf475 meter.go:236)
    let start = Instant::now();
    let mut deadline = start.checked_add(TICK).ok_or("instant overflow")?;
    let mut now = start;
    let mut rows = 0;
    for (n, line) in golden("ticks.txt")?.lines().enumerate() {
        let fields = line
            .split('\t')
            .map(str::parse::<u64>)
            .collect::<Result<Vec<u64>, _>>()?;
        let [busy, taken] = fields[..] else {
            return Err(format!("ticks.txt line {}: malformed row", n + 1).into());
        };
        now = now.max(deadline);
        ensure_eq!(
            now.saturating_duration_since(start),
            Duration::from_nanos(taken),
            "ticks.txt line {}: the tick taken",
            n + 1
        );
        deadline = next_tick(deadline, now).ok_or("no next tick")?;
        now = now
            .checked_add(Duration::from_nanos(busy))
            .ok_or("instant overflow")?;
        rows += 1;
    }
    ensure_eq!(rows, 11, "rows");
    Ok(())
}
