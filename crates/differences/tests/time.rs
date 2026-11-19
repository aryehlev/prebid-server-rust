//! Pins of the `time` crate against Go's `time` layouts: RFC 3339 with and without a fraction
//! (`testdata/golden/time.json`).

#[cfg(test)]
mod tests {
    use pbs_differences::{TestResult, check_table, golden, list, number, text};
    use time::{
        OffsetDateTime, format_description::well_known::Rfc3339, macros::format_description,
    };

    /// Go's `time.RFC3339` layout writes whole seconds (`2026-11-18T10:20:30Z`) and
    /// `time.RFC3339Nano` the fraction without trailing zeros; the `time` crate's `Rfc3339`
    /// writes the fraction whenever there is one, as `RFC3339Nano` does, so whole seconds need a
    /// format description of their own.
    // Go: stored_requests/events/http/http.go:127 (the last-modified query parameter,
    // lastUpdate.Format(time.RFC3339))
    #[test]
    fn go_difference_rfc3339_fraction() -> TestResult {
        let doc = golden("time.json")?;
        let seconds = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
        let mut rows = Vec::new();
        for case in list(&doc, "/times")? {
            let nanos = number(case, "/nanos")?;
            let at = OffsetDateTime::from_unix_timestamp_nanos(
                i128::from(number(case, "/unix")?) * 1_000_000_000 + i128::from(nanos),
            )?;
            rows.push(vec![
                nanos.to_string(),
                text(case, "/rfc3339")?.to_owned(),
                text(case, "/rfc3339_nano")?.to_owned(),
                at.format(&Rfc3339)?,
                at.format(seconds)?,
            ]);
        }
        check_table(
            &rows,
            &[
                &[
                    "250000000",
                    "2026-11-18T10:20:30Z",
                    "2026-11-18T10:20:30.25Z",
                    "2026-11-18T10:20:30.25Z",
                    "2026-11-18T10:20:30Z",
                ],
                &[
                    "0",
                    "2026-11-18T10:20:30Z",
                    "2026-11-18T10:20:30Z",
                    "2026-11-18T10:20:30Z",
                    "2026-11-18T10:20:30Z",
                ],
                &[
                    "123456789",
                    "2026-11-18T10:20:30Z",
                    "2026-11-18T10:20:30.123456789Z",
                    "2026-11-18T10:20:30.123456789Z",
                    "2026-11-18T10:20:30Z",
                ],
            ],
            "nanoseconds, Go RFC3339, Go RFC3339Nano, time Rfc3339, time whole seconds",
        )
    }
}
