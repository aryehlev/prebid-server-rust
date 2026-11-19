//! Pins of the `prometheus` crate against `client_golang` as `metrics/prometheus` registers and
//! `server/prometheus.go` serves it: the text exposition of a counter
//! (`testdata/golden/metrics.json`).

#[cfg(test)]
mod tests {
    use pbs_differences::{TestResult, check_eq, check_table, golden, list, number, text};
    use prometheus::{CounterVec, Encoder as _, Opts, Registry, TextEncoder};

    /// `client_golang` writes a sample value with `strconv.FormatFloat(v, 'g', -1, 64)`, so a
    /// counter at one million is `1e+06` and one at 123456789 is `1.23456789e+08`; the
    /// `prometheus` crate writes `1000000` and `123456789`. The HELP and TYPE lines and the label
    /// order are the same.
    // Go: metrics/prometheus/prometheus.go:229-232 (the requests counter) and :648-658
    // (newCounter), served by server/prometheus.go:20-27 (promhttp.HandlerFor)
    #[test]
    fn go_difference_sample_float_format() -> TestResult {
        let doc = golden("metrics.json")?;
        let registry = Registry::new();
        let requests = CounterVec::new(
            Opts::new(
                "requests",
                "Count of total requests to Prebid Server labeled by type and status.",
            )
            .namespace("pbs"),
            &["request_type", "request_status"],
        )?;
        registry.register(Box::new(requests.clone()))?;
        for sample in list(&doc, "/samples")? {
            let count = u32::try_from(number(sample, "/value")?)?;
            requests
                .with_label_values(&[text(sample, "/request_type")?, "ok"])
                .inc_by(f64::from(count));
        }
        let mut exposition = Vec::new();
        TextEncoder::new().encode(&registry.gather(), &mut exposition)?;
        let rust = String::from_utf8(exposition)?;
        let go = text(&doc, "/exposition")?;
        check_eq(
            &go.lines().count(),
            &rust.lines().count(),
            "the number of lines",
        )?;
        let rows: Vec<Vec<String>> = go
            .lines()
            .zip(rust.lines())
            .map(|(go_line, rust_line)| vec![go_line.to_owned(), rust_line.to_owned()])
            .collect();
        let help = "# HELP pbs_requests Count of total requests to Prebid Server labeled by type \
                    and status.";
        check_table(
            &rows,
            &[
                &[help, help],
                &["# TYPE pbs_requests counter", "# TYPE pbs_requests counter"],
                &[
                    "pbs_requests{request_status=\"ok\",request_type=\"amp\"} 1e+06",
                    "pbs_requests{request_status=\"ok\",request_type=\"amp\"} 1000000",
                ],
                &[
                    "pbs_requests{request_status=\"ok\",request_type=\"cookie_sync\"} 100000",
                    "pbs_requests{request_status=\"ok\",request_type=\"cookie_sync\"} 100000",
                ],
                &[
                    "pbs_requests{request_status=\"ok\",request_type=\"openrtb2-web\"} 1",
                    "pbs_requests{request_status=\"ok\",request_type=\"openrtb2-web\"} 1",
                ],
                &[
                    "pbs_requests{request_status=\"ok\",request_type=\"video\"} 1.23456789e+08",
                    "pbs_requests{request_status=\"ok\",request_type=\"video\"} 123456789",
                ],
            ],
            "client_golang, prometheus",
        )
    }
}
