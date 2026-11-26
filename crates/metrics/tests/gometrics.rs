//! The go-metrics engine against `testdata/golden/gometrics`, which `testdata/go/gometrics.go`
//! writes by replaying the scenario scripts through Go's engine (`metrics/go_metrics.go` over
//! rcrowley/go-metrics cf1acfcdf475): the registry after every scenario and the lines the engine
//! logs.

pub mod support;

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fmt::Write as _};

    use pbs_logger::Capture;
    use pbs_metrics::{
        go_metrics::new_metrics,
        gometrics::{Metric, Registry, Sample},
    };

    use super::support::{
        LabelUnicode, Scenario, TestResult, check, check_eq, core_bidder_names, fnv1a64, golden,
        read_scenarios, replay,
    };

    /// Go: `reporterPercentiles`, the percentiles the `InfluxDB` reporter sends.
    const REPORTER_PERCENTILES: [f64; 6] = [0.5, 0.75, 0.95, 0.99, 0.999, 0.9999];

    /// Go: `strconv.QuoteToASCII`: printable ASCII as it is but `"` and `\`, Go's escapes for the
    /// control characters, `\u` or `\U` for every other character.
    fn quote(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\u{7}' => out.push_str("\\a"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{b}' => out.push_str("\\v"),
                ' '..='~' => out.push(c),
                _ if c < ' ' || c == '\u{7f}' => {
                    write!(out, "\\x{:02x}", u32::from(c)).unwrap_or_default();
                }
                _ if u32::from(c) < 0x1_0000 => {
                    write!(out, "\\u{:04x}", u32::from(c)).unwrap_or_default();
                }
                _ => {
                    write!(out, "\\U{:08x}", u32::from(c)).unwrap_or_default();
                }
            }
        }
        out.push('"');
        out
    }

    /// Go: `strconv.FormatFloat(f, 'g', -1, 64)`: the shortest digits that read back as `f`, in
    /// `%e` form when the decimal exponent is below -4 or at least 6 (Go's precision for that
    /// choice in the shortest form), with at least two exponent digits.
    fn format_g(f: f64) -> String {
        if f.is_nan() {
            return "NaN".to_owned();
        }
        if f.is_infinite() {
            return if f > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
        }
        if f == 0.0 {
            return if f.is_sign_negative() { "-0" } else { "0" }.to_owned();
        }
        // Rust's `{:e}` writes the shortest round-trip digits as `d.ddde<exp>`.
        let exp_form = format!("{:e}", f.abs());
        let (mantissa, exp) = exp_form.split_once('e').unwrap_or((exp_form.as_str(), "0"));
        let exp: i32 = exp.parse().unwrap_or_default();
        let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
        let sign = if f.is_sign_negative() { "-" } else { "" };
        if !(-4..6).contains(&exp) {
            let (first, rest) = digits.split_at(1);
            let fraction = if rest.is_empty() {
                String::new()
            } else {
                format!(".{rest}")
            };
            let exp_sign = if exp < 0 { '-' } else { '+' };
            return format!(
                "{sign}{first}{fraction}e{exp_sign}{:02}",
                exp.unsigned_abs()
            );
        }
        let nd = i32::try_from(digits.len()).unwrap_or(i32::MAX);
        let dp = exp + 1;
        let plain = if dp <= 0 {
            let zeros = "0".repeat(usize::try_from(-dp).unwrap_or_default());
            format!("0.{zeros}{digits}")
        } else if dp >= nd {
            let zeros = "0".repeat(usize::try_from(dp - nd).unwrap_or_default());
            format!("{digits}{zeros}")
        } else {
            let (int, frac) = digits.split_at(usize::try_from(dp).unwrap_or_default());
            format!("{int}.{frac}")
        };
        format!("{sign}{plain}")
    }

    /// Go: `statistics`: the percentiles first (they sort the snapshot), then the rest.
    macro_rules! statistics {
        ($snapshot:expr) => {{
            let mut snapshot = $snapshot;
            let ps = snapshot.percentiles(&REPORTER_PERCENTILES);
            let mut out = format!(
                "count={} max={} mean={} min={} stddev={} variance={} sum={}",
                snapshot.count(),
                snapshot.max(),
                format_g(snapshot.mean()),
                snapshot.min(),
                format_g(snapshot.std_dev()),
                format_g(snapshot.variance()),
                snapshot.sum(),
            );
            for (name, p) in ["p50", "p75", "p95", "p99", "p999", "p9999"]
                .iter()
                .zip(&ps)
            {
                write!(out, " {name}={}", format_g(*p)).unwrap_or_default();
            }
            out
        }};
    }

    /// Go: `dumpRegistry`: one row per registered metric, by name. A name the engine lowered with a
    /// letter std's newer Unicode tables lower and Go's leave is written with Go's letter
    /// (`metrics.label-unicode`; no scenario input holds such a lower case).
    fn dump_registry(registry: &Registry, unicode: &LabelUnicode) -> String {
        let mut entries: Vec<(String, Metric)> = Vec::new();
        registry.each(|name, metric| entries.push((name.to_owned(), metric.clone())));
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out = String::new();
        for (name, metric) in entries {
            let (kind, values) = match metric {
                Metric::Counter(c) => ("counter", format!("count={}", c.snapshot().count())),
                Metric::Meter(m) => ("meter", format!("count={}", m.snapshot().count())),
                Metric::Timer(t) => ("timer", statistics!(t.snapshot())),
                Metric::Histogram(h) => {
                    let kind = match h.sample() {
                        Sample::ExpDecay(_) => "histogram/expdecay",
                        Sample::Uniform(_) => "histogram/uniform",
                    };
                    (kind, statistics!(h.snapshot()))
                }
            };
            let name = unicode.undo(&name);
            writeln!(out, "{}\t{}\t{}", quote(&name), quote(kind), quote(&values))
                .unwrap_or_default();
        }
        out
    }

    /// Go: `goMetricsScenario`: the log rows of the replayed calls (each line the severity and the
    /// message, as `normalizeGlog` leaves glog's), then the registry.
    fn dump_scenario(
        s: &Scenario,
        exchanges: &[String],
        unicode: &LabelUnicode,
    ) -> TestResult<String> {
        let registry = Registry::new_prefixed("prebidserver.");
        let m = new_metrics(
            registry.clone(),
            exchanges,
            s.disabled,
            &s.syncers,
            &s.modules,
        );
        let capture = Capture::new();
        let mut seen = 0;
        let mut out = String::new();
        for (line, tokens) in &s.calls {
            capture
                .run(|| replay(&m, tokens))
                .map_err(|e| format!("{}: line {line}: {e}", s.name))?;
            let lines = capture.lines();
            let mut logged = String::new();
            for l in lines.get(seen..).unwrap_or_default() {
                let severity = l.severity.name().chars().next().unwrap_or('?');
                write!(logged, "{severity} {}", l.message).unwrap_or_default();
                if !l.message.ends_with('\n') {
                    logged.push('\n');
                }
            }
            seen = lines.len();
            if !logged.is_empty() {
                writeln!(
                    out,
                    "{}\t{}\t{}",
                    quote("log"),
                    quote(&line.to_string()),
                    quote(&logged)
                )
                .unwrap_or_default();
            }
        }
        out.push_str(&dump_registry(&registry, unicode));
        Ok(out)
    }

    /// Go: `recordedRows`: the log rows and the rows whose count is not 0.
    fn recorded_rows(dump: &str) -> String {
        dump.split_inclusive('\n')
            .filter(|row| {
                let values = row
                    .trim_end_matches('\n')
                    .rsplit('\t')
                    .next()
                    .unwrap_or_default();
                row.starts_with("\"log\"\t")
                    || !(values == "\"count=0\"" || values.starts_with("\"count=0 "))
            })
            .collect()
    }

    /// The first line that differs, for a readable failure.
    fn first_difference(expected: &str, actual: &str) -> String {
        let mut expected_lines = expected.lines();
        let mut actual_lines = actual.lines();
        for line_no in 1.. {
            match (expected_lines.next(), actual_lines.next()) {
                (None, None) => break,
                (e, a) if e == a => {}
                (e, a) => return format!("line {line_no}: Go {e:?}, Rust {a:?}"),
            }
        }
        "no line differs".to_owned()
    }

    /// Every scenario's registry and log lines are Go's: the recorded rows byte for byte
    /// (`dumps.txt`), the complete dumps by length and FNV-1a (`digests.txt`) and the complete
    /// start-up registry (`registry.txt`), also over all core bidders. The adapter names in the
    /// metric names are lowered as Go's `strings.ToLower` lowers them (`metrics.label-case`).
    #[test]
    fn dumps_match_go() -> TestResult {
        // Go: testdata/go/gometrics.go:81 (runGoMetrics), over metrics/go_metrics.go:294
        // (NewMetrics) and the Record methods
        let scenarios = read_scenarios(&[
            "scenarios.txt",
            "scenarios_generated.txt",
            "gometrics_scenarios.txt",
        ])?;
        let exchanges: Vec<String> = golden("gometrics/exchanges.txt")?
            .lines()
            .map(str::to_owned)
            .collect();
        let mut digests = BTreeMap::new();
        for row in golden("gometrics/digests.txt")?.lines() {
            let mut fields = row.split(' ');
            let (Some(name), Some(len), Some(hash), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(format!("digests.txt: malformed row {row:?}").into());
            };
            drop(digests.insert(name.to_owned(), (len.parse::<usize>()?, hash.to_owned())));
        }
        check_eq(
            &digests.len(),
            &(scenarios.len() + 1),
            "one digest per scenario, and startup_viper_defaults@core",
        )?;

        let unicode = LabelUnicode::new()?;
        for input in exchanges.iter().chain(&core_bidder_names()?) {
            check(!unicode.touches(input), &format!("no adapter is {input:?}"))?;
        }
        for s in &scenarios {
            for input in s.inputs()? {
                check(
                    !unicode.touches(&input),
                    &format!("{}: no input is {input:?}", s.name),
                )?;
            }
        }

        let mut dumps = String::new();
        let mut actual = Vec::new();
        for s in &scenarios {
            let dump = dump_scenario(s, &exchanges, &unicode)?;
            if s.name == "startup_viper_defaults" {
                let expected = golden("gometrics/registry.txt")?;
                check(
                    dump == expected,
                    &format!("registry.txt: {}", first_difference(&expected, &dump)),
                )?;
                let core = dump_scenario(s, &core_bidder_names()?, &unicode)?;
                actual.push((format!("{}@core", s.name), core));
            }
            writeln!(dumps, "{}\t{}", quote("scenario"), quote(&s.name)).unwrap_or_default();
            dumps.push_str(&recorded_rows(&dump));
            actual.push((s.name.clone(), dump));
        }
        // The recorded rows first: a difference there names the row.
        let expected = golden("gometrics/dumps.txt")?;
        check(
            dumps == expected,
            &format!("dumps.txt: {}", first_difference(&expected, &dumps)),
        )?;
        for (name, dump) in actual {
            let go = digests
                .get(&name)
                .ok_or_else(|| format!("no digest for {name}"))?;
            let rust = (dump.len(), format!("{:016x}", fnv1a64(dump.as_bytes())));
            check_eq(&rust, go, &format!("digest of the complete dump of {name}"))?;
        }
        Ok(())
    }
}
