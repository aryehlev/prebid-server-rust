//! The Prometheus engine against Go's: every scenario of `testdata/scenarios.txt` and the
//! generated `testdata/scenarios_generated.txt`, replayed through the Rust engine and compared
//! with what Go's engine served through `promhttp` (`testdata/go`: go1.26.2, `client_golang`
//! v1.12.1) after both scrapes are read back into families and samples; and the pins of the
//! ledger's `metrics` entries.

pub mod support;

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use pbs_metrics::{
        MetricsEngine as _,
        metrics::{AdapterError, AdapterLabels},
        options::{DisabledMetrics, PrometheusMetrics},
        prometheus::{Metrics, TEXT_FORMAT},
    };

    use super::support::{
        Exposition, LabelUnicode, Scenario, Scrape, TestResult, adapter_label, all_scenarios,
        canonical_value, check, check_eq, core_adapter_labels, core_bidder_names, fnv1a64,
        full_digests, go_to_lower, go_unquote, golden, keep_sample_adapters, path_unescape, scrape,
        semantics, value_spellings,
    };

    /// The scenario named `name`.
    fn scenario(name: &str) -> TestResult<Scenario> {
        all_scenarios()?
            .into_iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("no scenario {name}").into())
    }

    /// The Rust scrape of the scenario named `name`, reduced to the sample adapters as the
    /// committed `.prom` scrapes are, before `metrics.label-unicode` is undone.
    fn sample_scrape(name: &str) -> TestResult<String> {
        let Scrape::Text(body) = scrape(&scenario(name)?, &core_bidder_names()?)? else {
            return Err(format!("{name}: the engine refuses the configuration").into());
        };
        Ok(keep_sample_adapters(&body, &core_adapter_labels()?))
    }

    /// The lines of `text` whose adapter label is `adapter`.
    fn adapter_lines<'a>(text: &'a str, adapter: &str) -> Vec<&'a str> {
        text.lines()
            .filter(|line| adapter_label(line) == Some(adapter))
            .collect()
    }

    /// Every scenario against Go. Where Go's `NewMetrics` panics the Rust engine refuses the
    /// configuration (`registration_errors` compares the texts). Otherwise both scrapes, read
    /// back into families and samples, have the same names, HELP, TYPE, label sets and values,
    /// and the same lines in the same order but for the spelling of the values
    /// (`semantics.txt`); the committed `.prom` scrapes differ from Go's in value spellings only
    /// (`metrics.sample-float-format`). The Rust scrape is read with `metrics.label-unicode`
    /// undone, which only an adapter label the engine lowers can show: no scenario passes a
    /// character of it.
    // Go: metrics/prometheus/prometheus.go (NewMetrics, the Record methods) and preload.go,
    // served by server/prometheus.go:20-27
    #[test]
    fn scrapes_match_go_after_parsing() -> TestResult {
        let scenarios = all_scenarios()?;
        let semantics = semantics()?;
        let full = full_digests()?;
        let core_bidder_names = core_bidder_names()?;
        let core = core_adapter_labels()?;
        let unicode = LabelUnicode::new()?;
        check(
            scenarios.len() >= 60,
            &format!("the scripts have {} scenarios", scenarios.len()),
        )?;
        check_eq(
            &semantics.len(),
            &scenarios.len(),
            "one semantics line per scenario",
        )?;
        check_eq(&full.len(), &scenarios.len(), "one digest per scenario")?;

        let mut failures = Vec::new();
        let mut refused = 0_usize;
        let mut respelled = BTreeSet::new();
        for s in &scenarios {
            for input in s.inputs()? {
                check(
                    !unicode.touches(&input),
                    &format!(
                        "{}: {input:?} holds a lower case of metrics.label-unicode",
                        s.name
                    ),
                )?;
            }
            let go = semantics
                .get(&s.name)
                .ok_or_else(|| format!("{}: no semantics line", s.name))?;
            let (body, go) = match (scrape(s, &core_bidder_names)?, go) {
                (Scrape::Text(body), Some(go)) => (unicode.undo(&body), go),
                (Scrape::Refused(_), None) => {
                    refused += 1;
                    continue;
                }
                (Scrape::Refused(err), Some(_)) => {
                    failures.push(format!("{}: Go registers, Rust refuses: {err}", s.name));
                    continue;
                }
                (Scrape::Text(_), None) => {
                    failures.push(format!("{}: Go panics, Rust registers", s.name));
                    continue;
                }
            };
            let rust = Exposition::read(&body)?.digests();
            if rust != *go {
                failures.push(format!("{}: Rust {rust:?}, Go {go:?}", s.name));
                continue;
            }
            if !s.digest_only {
                let go_text = golden(&format!("{}.prom", s.name))?;
                if let Err(e) = value_spellings(&go_text, &keep_sample_adapters(&body, &core)) {
                    failures.push(format!("{}: {e}", s.name));
                }
            }
            let digest = full
                .get(&s.name)
                .copied()
                .ok_or_else(|| format!("{}: no digest", s.name))?;
            if (body.len(), fnv1a64(body.as_bytes())) != digest {
                let _new = respelled.insert(s.name.as_str());
            }
        }
        check(
            failures.is_empty(),
            &format!(
                "{} of {} scenarios differ from Go:\n{}",
                failures.len(),
                scenarios.len(),
                failures.join("\n")
            ),
        )?;
        check_eq(
            &refused,
            &21,
            "the configurations Go's NewMetrics panics on",
        )?;
        // The complete scrapes that are not Go's bytes: each holds a value at a million or
        // more, or below 0.0001, in a sample the semantic digests show equal.
        check_eq(
            &respelled.into_iter().collect::<Vec<_>>(),
            &RESPELLED.to_vec(),
            "the scrapes whose values Go spells otherwise",
        )
    }

    /// The scenarios whose complete scrape is not byte for byte Go's.
    const RESPELLED: &[&str] = &[
        "all_methods",
        "disabled_combo_00",
        "disabled_combo_02",
        "disabled_combo_04",
        "disabled_combo_06",
        "disabled_combo_08",
        "disabled_combo_10",
        "disabled_combo_12",
        "disabled_combo_14",
        "disabled_combo_16",
        "disabled_combo_18",
        "disabled_combo_20",
        "disabled_combo_22",
        "disabled_combo_24",
        "disabled_combo_26",
        "disabled_combo_28",
        "disabled_combo_30",
        "large_values",
        "namespace_only_edge_preload",
        "prefix_04",
        "random_calls",
    ];

    /// A name as Go's `%q` quotes it, with each escape `strconv.Quote` writes, in pieces so that
    /// no escape is in the source as it is.
    const GO_QUOTED: [&str; 3] = ["\"a\\\"b\\\\c\\td\\x7fe", "\\", "u00adf\\U000e0001g\" rest"];

    #[test]
    fn script_helpers_match_go() -> TestResult {
        check_eq(
            path_unescape("a%20b%22%5C%0A%C3%A9")?.as_str(),
            "a b\"\\\né",
            "PathUnescape",
        )?;
        check(path_unescape("bad%2").is_err(), "a truncated escape")?;
        check_eq(
            &fnv1a64(b""),
            &0xCBF2_9CE4_8422_2325,
            "the FNV offset basis",
        )?;
        check_eq(&fnv1a64(b"a"), &0xAF63_DC4C_8601_EC8C, "FNV-1a of a")?;
        check_eq(
            &adapter_label("x{adapter=\"appnexus\",y=\"1\"} 0"),
            &Some("appnexus"),
            "the first label",
        )?;
        check_eq(
            &adapter_label("x{a=\"1\",adapter=\"rubicon\"} 0"),
            &Some("rubicon"),
            "a later label",
        )?;
        check_eq(
            &adapter_label("# HELP x{adapter=\"appnexus\"}"),
            &None,
            "a comment",
        )?;
        check_eq(
            canonical_value("1e+06")?.as_str(),
            "412e848000000000",
            "1e+06",
        )?;
        check_eq(
            &canonical_value("1000000")?,
            &canonical_value("1e+06")?,
            "one value, two spellings",
        )?;
        check_eq(canonical_value("-0")?.as_str(), "0", "negative zero")?;
        check_eq(canonical_value("NaN")?.as_str(), "NaN", "NaN")?;
        check_eq(
            &go_unquote(&GO_QUOTED.concat())?,
            &("a\"b\\c\td\u{7f}e\u{ad}f\u{e0001}g".to_owned(), " rest"),
            "strconv.Unquote",
        )?;
        let e = Exposition::read(
            "# HELP a_b x\\\\y\n# TYPE a_b counter\na_b{l=\"q\\\"\",k=\"v\"} 1e+06\n",
        )?
        .digests();
        let f = Exposition::read(
            "# HELP a_b x\\\\y\n# TYPE a_b counter\na_b{k=\"v\",l=\"q\\\"\"} 1000000\n",
        )?
        .digests();
        check_eq(&(e.families, e.samples), &(1, 1), "the counts")?;
        check_eq(
            &e.semantic,
            &f.semantic,
            "label order and spelling are not semantics",
        )?;
        check(e.values != f.values, "the label order is in the lines")
    }

    /// Go's `writeFloat` (prometheus/common v0.32.1 `expfmt/text_create.go`):
    /// `strconv.FormatFloat` with `'g'` and the shortest precision, but `0` for either zero.
    fn write_float(f: f64, formatted_g: &str) -> String {
        if f == 0.0 {
            "0".to_owned()
        } else {
            formatted_g.to_owned()
        }
    }

    /// How Go's spelling of a sample value and the crate's compare: the same, or one of the
    /// three classes of `metrics.sample-float-format`.
    fn spelling_class(go: &str, rust: &str) -> TestResult<&'static str> {
        let both = (go, rust);
        Ok(if go == rust {
            "same"
        } else if both == ("+Inf", "inf") || both == ("-Inf", "-inf") {
            "infinity"
        } else if both == ("0", "-0") {
            "negative zero"
        } else {
            check(
                go.contains('e') && !rust.contains('e'),
                &format!("Go {go:?}, Rust {rust:?}"),
            )?;
            check_eq(
                &canonical_value(rust)?,
                &canonical_value(go)?,
                &format!("Go {go:?}, Rust {rust:?}: one value"),
            )?;
            "exponent"
        })
    }

    /// `client_golang` spells a sample value as `strconv.FormatFloat(v, 'g', -1, 64)`, which
    /// takes an exponent at a million and above and below 0.0001 (`1e+06`, `1.234567e+06`,
    /// `1e-05`); the prometheus crate writes `f64`'s `Display`, the same digits without an
    /// exponent (`1000000`, `1234567`, `0.00001`), and `inf`, `-inf` and `-0` where Go writes
    /// `+Inf`, `-Inf` and `0`. The values are the same numbers. The large values scenario shows
    /// ten such samples; `float_g.txt` is Go's spelling of 3,540 values (ledger:
    /// `metrics.sample-float-format`, and `metrics.exposition-lexical` for the infinities).
    // Go: prometheus/common v0.32.1 expfmt/text_create.go:433-454 (writeFloat), with which
    // client_golang v1.12.1 writes the text format
    #[test]
    fn float_spelling() -> TestResult {
        let spellings = value_spellings(
            &golden("large_values.prom")?,
            &sample_scrape("large_values")?,
        )?;
        let expected: Vec<(String, String)> = LARGE_VALUES
            .iter()
            .map(|&(go, rust)| (go.to_owned(), rust.to_owned()))
            .collect();
        check_eq(&spellings, &expected, "large_values: (Go, Rust) spellings")?;

        let mut counts = BTreeMap::new();
        for line in golden("float_g.txt")?.lines() {
            let (bits, formatted_g) = line.split_once(' ').ok_or("malformed line")?;
            let f = f64::from_bits(u64::from_str_radix(bits, 16)?);
            let class = spelling_class(&write_float(f, formatted_g), &f.to_string())
                .map_err(|e| format!("{line}: {e}"))?;
            *counts.entry(class).or_insert(0_usize) += 1;
        }
        check_eq(
            &counts.into_iter().collect::<Vec<_>>(),
            &FLOAT_TABLE.to_vec(),
            "float_g.txt by class",
        )
    }

    /// The samples of the large values scenario whose value Go spells otherwise, as (Go, Rust).
    const LARGE_VALUES: &[(&str, &str)] = &[
        ("1.2345678901234568e+17", "123456789012345680"),
        ("1e+17", "100000000000000000"),
        ("1.0100000000000002e-05", "0.000010100000000000002"),
        ("1.23456789e+08", "123456789"),
        ("-9.223372036854776e+09", "-9223372036.854776"),
        ("9.223372036854776e+09", "9223372036.854776"),
        ("1.234567e+06", "1234567"),
        ("9.007199254740992e+15", "9007199254740992"),
        ("1e+06", "1000000"),
        ("1e-09", "0.000000001"),
    ];

    /// The values of `float_g.txt` by how the spellings compare: 3,540 values, the spelling of
    /// 741 the same.
    const FLOAT_TABLE: &[(&str, usize)] = &[
        ("exponent", 2796),
        ("infinity", 2),
        ("negative zero", 1),
        ("same", 741),
    ];

    /// Go's handler (`promhttp.HandlerFor` with Prebid's options) negotiates by Accept: the
    /// delimited, text and compact-text protobuf encodings, and the text format with
    /// `; charset=utf-8` for any other Accept, `OpenMetrics` too (Prebid leaves it off); it
    /// gzips whenever Accept-Encoding names gzip, `gzip;q=0` too. The engine serves the
    /// crate's text format 0.0.4 only, as `text/plain; version=0.0.4`; the server compresses
    /// it with tower-http, which honours q-values.
    // Go: server/prometheus.go:20-27; client_golang v1.12.1 prometheus/promhttp/http.go:86
    // (HandlerFor) and :360 (gzipAccepted); prometheus/common v0.32.1 expfmt/encode.go:62
    // (Negotiate)
    #[test]
    fn text_only() -> TestResult {
        let negotiation = golden("negotiation.txt")?;
        let rows: Vec<Vec<&str>> = negotiation
            .lines()
            .map(|line| line.split('\t').collect())
            .collect();
        check_eq(&rows.len(), &33, "11 Accept headers by 3 Accept-Encodings")?;
        let go_text = golden("content_type.txt")?;
        let go_text = go_text.trim_end();
        let protobuf = "application/vnd.google.protobuf; \
                        proto=io.prometheus.client.MetricFamily; encoding=";
        let mut content_types = BTreeSet::new();
        for row in &rows {
            let [accept, encoding, status, content_type, content_encoding] = row.as_slice() else {
                return Err(format!("malformed row {row:?}").into());
            };
            check_eq(*status, "200", "Go answers every Accept")?;
            check_eq(
                *content_encoding,
                if *encoding == "-" { "-" } else { "gzip" },
                &format!("{accept} with {encoding}"),
            )?;
            let _new = content_types.insert(*content_type);
            if accept.starts_with("application/vnd.google.protobuf") {
                check(
                    content_type.starts_with(protobuf),
                    &format!("{accept}: {content_type}"),
                )?;
            } else {
                check_eq(*content_type, go_text, accept)?;
            }
        }
        check_eq(
            &content_types.len(),
            &4,
            "Go's four answers: three protobuf encodings and the text format",
        )?;
        check_eq(
            go_text,
            "text/plain; version=0.0.4; charset=utf-8",
            "Go's text format",
        )?;
        check_eq(
            TEXT_FORMAT,
            "text/plain; version=0.0.4",
            "the crate's text format",
        )?;

        let cfg = PrometheusMetrics {
            port: 8080,
            ..PrometheusMetrics::default()
        };
        let m = Metrics::new(
            &cfg,
            DisabledMetrics::NONE,
            &[],
            &BTreeMap::new(),
            &core_bidder_names()?,
        )?;
        let body = m.text_exposition()?;
        check(body.starts_with("# HELP "), "the text format")?;
        check(
            Exposition::read(&body)?.digests().families > 50,
            "the text format reads back",
        )
    }

    /// Go registers `collectors.NewGoCollector()` under Prebid's prefix, 140 families with the
    /// namespace `pbs` (76 counters, 55 gauges, 8 histograms, a summary); the Rust engine
    /// registers none of them (D-GOCOLL's default).
    // Go: metrics/prometheus/prometheus.go:576-585
    #[test]
    fn no_go_families() -> TestResult {
        let go_families = golden("go_families.txt")?;
        let mut go = BTreeMap::new();
        for line in go_families.lines() {
            let (name, typ) = line.split_once(' ').ok_or("malformed line")?;
            check(name.starts_with("pbs_go_"), name)?;
            check(go.insert(name, typ).is_none(), name)?;
        }
        let mut types = BTreeMap::new();
        for typ in go.values() {
            *types.entry(*typ).or_insert(0_usize) += 1;
        }
        check_eq(
            &types.into_iter().collect::<Vec<_>>(),
            &vec![
                ("counter", 76),
                ("gauge", 55),
                ("histogram", 8),
                ("summary", 1),
            ],
            "the Go collector's families",
        )?;
        for name in ["pbs_go_goroutines", "pbs_go_threads", "pbs_go_info"] {
            check(go.contains_key(name), name)?;
        }

        let cfg = PrometheusMetrics {
            namespace: "pbs".to_owned(),
            ..PrometheusMetrics::default()
        };
        let m = Metrics::new(
            &cfg,
            DisabledMetrics::NONE,
            &[],
            &BTreeMap::new(),
            &core_bidder_names()?,
        )?;
        let rust: Vec<String> = m
            .gatherer()
            .gather()
            .iter()
            .map(|f| f.name().to_owned())
            .collect();
        check(rust.len() > 50, "the engine's families")?;
        check(
            rust.iter().all(|name| !name.starts_with("pbs_go_")),
            "no Go collector family",
        )
    }

    /// The lexical classes of the committed scrapes: the escaping of label values and HELP, and
    /// the order of families, series and labels, are Go's (the `values` digests of every
    /// scenario agree as well); what differs is the spelling of values, in exponent form
    /// (`metrics.sample-float-format`) or infinite (`metrics.exposition-lexical`).
    // Go: prometheus/common v0.32.1 expfmt/text_create.go (MetricFamilyToText,
    // writeEscapedString); client_golang v1.12.1 prometheus/registry.go (Gather's sorting)
    #[test]
    fn lexical_classes() -> TestResult {
        let core = core_adapter_labels()?;
        let core_bidder_names = core_bidder_names()?;
        let unicode = LabelUnicode::new()?;
        let mut escapes = BTreeSet::new();
        let mut classes = BTreeMap::new();
        for s in all_scenarios()? {
            if s.digest_only {
                continue;
            }
            let Scrape::Text(body) = scrape(&s, &core_bidder_names)? else {
                continue;
            };
            let go = golden(&format!("{}.prom", s.name))?;
            for (go_value, rust_value) in
                value_spellings(&go, &keep_sample_adapters(&unicode.undo(&body), &core))?
            {
                let class = spelling_class(&go_value, &rust_value)
                    .map_err(|e| format!("{}: {e}", s.name))?;
                *classes.entry(class).or_insert(0_usize) += 1;
            }
            for escape in ["\\\"", "\\\\", "\\n"] {
                if go
                    .lines()
                    .any(|line| !line.starts_with('#') && line.contains(escape))
                {
                    let _new = escapes.insert(escape);
                }
            }
        }
        check_eq(
            &escapes.into_iter().collect::<Vec<_>>(),
            &vec!["\\\"", "\\\\", "\\n"],
            "the label escapes the goldens hold",
        )?;
        check_eq(
            &classes.into_iter().collect::<Vec<_>>(),
            &LEXICAL.to_vec(),
            "the values spelled otherwise, by class",
        )
    }

    /// The samples of the committed scrapes whose value Go spells otherwise, by class.
    const LEXICAL: &[(&str, usize)] = &[("exponent", 21), ("infinity", 2)];

    /// The crate keys a labelled series by the FNV-1a hash of its label values concatenated
    /// without a separator (`MetricVecCore::hash_label_values`) and never compares the values,
    /// so adapter `x` with error `tmaxtimeout` and adapter `xtmax` with error `timeout` are one
    /// series: Go scrapes two series of 1, the crate one series of 2 under the labels recorded
    /// first.
    // Go: metrics/prometheus/prometheus.go:859-873 (RecordAdapterRequest's adapter errors);
    // client_golang v1.12.1 prometheus/vec.go:225-247 (a separator after each value) and :468
    // (the values compared)
    #[test]
    fn label_hash() -> TestResult {
        let cfg = PrometheusMetrics {
            namespace: "pbs".to_owned(),
            ..PrometheusMetrics::default()
        };
        let m = Metrics::new(
            &cfg,
            DisabledMetrics::NONE,
            &[],
            &BTreeMap::new(),
            &core_bidder_names()?,
        )?;
        for (adapter, error) in [
            ("x", AdapterError::TMAX_TIMEOUT),
            ("xtmax", AdapterError::TIMEOUT),
        ] {
            let mut labels = AdapterLabels {
                adapter: adapter.to_owned(),
                ..AdapterLabels::default()
            };
            let _new = labels.adapter_errors.insert(error);
            m.record_adapter_request(&labels);
        }
        let body = m.text_exposition()?;
        let rust: Vec<&str> = body
            .lines()
            .filter(|line| {
                line.starts_with("pbs_adapter_errors{")
                    && matches!(adapter_label(line), Some("x" | "xtmax"))
            })
            .collect();
        let go = golden("label_collision.prom")?;
        check_eq(
            &go.lines().collect::<Vec<_>>(),
            &vec![
                "pbs_adapter_errors{adapter=\"x\",adapter_error=\"tmaxtimeout\"} 1",
                "pbs_adapter_errors{adapter=\"xtmax\",adapter_error=\"timeout\"} 1",
            ],
            "Go's series",
        )?;
        check_eq(
            &rust,
            &vec!["pbs_adapter_errors{adapter=\"x\",adapter_error=\"tmaxtimeout\"} 2"],
            "the crate's series",
        )
    }

    /// Where Go's `MustRegister` panics on a family name, the crate refuses the same family:
    /// Go's text is `descriptor Desc{fqName: "<name>", ...} is invalid: "<name>" is not a valid
    /// metric name` with `%q` quoting, the crate's `Error: '<name>' is not a valid metric name`
    /// with the name as it is.
    // Go: client_golang v1.12.1 prometheus/desc.go:86 (NewDesc), registry.go:292 (Register)
    // and :400-406 (MustRegister)
    #[test]
    fn registration_errors() -> TestResult {
        let core_bidder_names = core_bidder_names()?;
        let semantics = semantics()?;
        let mut names = Vec::new();
        for s in all_scenarios()? {
            if semantics.get(&s.name).is_none_or(Option::is_some) {
                continue;
            }
            let Scrape::Refused(err) = scrape(&s, &core_bidder_names)? else {
                return Err(format!("{}: Go panics, Rust registers", s.name).into());
            };
            let go = golden(&format!("{}.prom", s.name))?;
            let quoted = go
                .strip_prefix("panic: descriptor Desc{fqName: ")
                .ok_or_else(|| format!("{}: {go:?}", s.name))?;
            let (name, rest) = go_unquote(quoted)?;
            check(
                rest.ends_with(" is not a valid metric name\n"),
                &format!("{}: {go:?}", s.name),
            )?;
            check_eq(
                err.as_str(),
                format!("Error: '{name}' is not a valid metric name").as_str(),
                &s.name,
            )?;
            names.push(name);
        }
        check_eq(&names.len(), &21, "the refused configurations")?;
        check(
            names.iter().any(|name| name == "tab\t_connections_closed"),
            "a name Go quotes with an escape",
        )
    }

    /// std's lower case tables are a later Unicode version than go1.26.2's 15.0.0: a letter
    /// cased since then is lowered by the engine and kept by Go, so the adapter `Ᲊnew` (U+1C89)
    /// is `ᲊnew` (U+1C8A) where Go's scrape has `Ᲊnew`, and `𐵐garay` (U+10D50) is `𐵰garay`.
    // Go: metrics/prometheus/prometheus.go:860 (strings.ToLower(labels.Adapter)) and the other
    // adapter labels
    #[test]
    fn label_unicode() -> TestResult {
        let unicode = LabelUnicode::new()?;
        check(
            char::UNICODE_VERSION.0 >= 16,
            &format!("std's Unicode {:?}", char::UNICODE_VERSION),
        )?;
        for (lower, letter) in [('\u{1c8a}', '\u{1c89}'), ('\u{10d70}', '\u{10d50}')] {
            check_eq(
                &unicode.pairs().get(&lower),
                &Some(&letter),
                "a letter std lowers and Go keeps",
            )?;
        }
        let go = golden("random_calls.prom")?;
        let rust = sample_scrape("random_calls")?;
        for (go_adapter, rust_adapter) in [
            ("\u{1c89}new", "\u{1c8a}new"),
            ("\u{10d50}garay", "\u{10d70}garay"),
        ] {
            let go_lines = adapter_lines(&go, go_adapter);
            let rust_lines = adapter_lines(&rust, rust_adapter);
            check(!go_lines.is_empty(), go_adapter)?;
            check_eq(
                &adapter_lines(&rust, go_adapter).len(),
                &0,
                "Rust lowers the letter",
            )?;
            let undone: Vec<String> = rust_lines.iter().map(|line| unicode.undo(line)).collect();
            let _spellings = value_spellings(&go_lines.join("\n"), &undone.join("\n"))
                .map_err(|e| format!("{go_adapter}: {e}"))?;
        }
        let mut touched = Vec::new();
        let core_bidder_names = core_bidder_names()?;
        for s in all_scenarios()? {
            if let Scrape::Text(body) = scrape(&s, &core_bidder_names)?
                && unicode.touches(&body)
            {
                touched.push(s.name);
            }
        }
        check_eq(
            &touched,
            &vec!["random_calls".to_owned()],
            "the scrapes that show it",
        )
    }

    /// The restoration of `metrics.label-case`: the adapters of the random calls whose lower case
    /// std's `str::to_lowercase` gives otherwise than Go (`İstanbul`, where std writes
    /// `i̇stanbul`) or the same (`ΣIGMA`, `Straße`, `Àccent`, `Ⱟglagolitic`) have Go's series in
    /// the Rust scrape.
    // Go: metrics/prometheus/prometheus.go:860 (strings.ToLower(labels.Adapter))
    #[test]
    fn label_case_matches_go() -> TestResult {
        let go = golden("random_calls.prom")?;
        let rust = sample_scrape("random_calls")?;
        let table = go_to_lower()?;
        check_eq(&table.get(&'\u{130}'), &Some(&'i'), "Go lowers U+0130 to i")?;
        check_eq(
            "\u{130}stanbul".to_lowercase().as_str(),
            "i\u{307}stanbul",
            "std's full lower case",
        )?;
        for (bidder, adapter) in [
            ("\u{130}stanbul", "istanbul"),
            ("\u{3a3}IGMA", "\u{3c3}igma"),
            ("Stra\u{df}e", "stra\u{df}e"),
            ("\u{c0}ccent", "\u{e0}ccent"),
            ("\u{2c2f}glagolitic", "\u{2c5f}glagolitic"),
        ] {
            let go_lines = adapter_lines(&go, adapter);
            check(go_lines.len() > 2, &format!("{bidder}: Go's series"))?;
            let _spellings = value_spellings(
                &go_lines.join("\n"),
                &adapter_lines(&rust, adapter).join("\n"),
            )
            .map_err(|e| format!("{bidder}: {e}"))?;
        }
        Ok(())
    }
}
