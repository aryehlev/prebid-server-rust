//! Pins of `serde-saphyr` against `gopkg.in/yaml.v3` as `config/bidderinfo.go` decodes the
//! bidder-info files: the boolean spellings a `bool` field takes, and underscores in integers
//! (`testdata/golden/yaml.json`).

#[cfg(test)]
mod tests {
    use pbs_differences::{BoxError, TestResult, check_table, golden, list, text, yaml_options};
    use serde::Deserialize;

    /// A `bool` field, as `config.BidderInfo`'s `disabled`.
    #[derive(Deserialize)]
    struct Flag {
        a: bool,
    }

    /// An integer field.
    #[derive(Deserialize)]
    struct Count {
        n: i64,
    }

    /// Per golden outcome: the input, then what yaml.v3 made of it (the value, or `error`).
    fn go_outcomes(doc: &serde_json::Value, pointer: &str) -> Result<Vec<Vec<String>>, BoxError> {
        list(doc, pointer)?
            .iter()
            .map(|outcome| {
                let err = text(outcome, "/err")?;
                Ok(vec![
                    text(outcome, "/input")?.to_owned(),
                    if err.is_empty() {
                        text(outcome, "/value")?.to_owned()
                    } else {
                        "error".to_owned()
                    },
                ])
            })
            .collect()
    }

    /// yaml.v3 takes the YAML 1.1 booleans into a `bool` only in three spellings each (`yes`,
    /// `Yes`, `YES`; `on`, `On`, `ON`; and so on) and rejects mixed case such as `yEs`, as well
    /// as `1`, `0`, `t` and `f`. `serde-saphyr` matches the words in any case, so a mixed-case
    /// value that fails Go's startup loads here.
    // Go: config/bidderinfo.go:323 (yaml.Unmarshal into config.BidderInfo, whose bool fields are at
    // lines 27, 28 and 35)
    #[test]
    fn go_difference_boolean_case() -> TestResult {
        let doc = golden("yaml.json")?;
        let mut rows = go_outcomes(&doc, "/bools")?;
        for row in &mut rows {
            let input = row.first().cloned().unwrap_or_default();
            let parsed = serde_saphyr::from_str_with_options::<Flag>(
                &format!("a: {input}\n"),
                yaml_options(),
            );
            row.push(parsed.map_or_else(|_| "error".to_owned(), |flag| flag.a.to_string()));
        }
        check_table(
            &rows,
            &[
                &["y", "true", "true"],
                &["Y", "true", "true"],
                &["yes", "true", "true"],
                &["Yes", "true", "true"],
                &["YES", "true", "true"],
                &["yEs", "error", "true"],
                &["on", "true", "true"],
                &["On", "true", "true"],
                &["ON", "true", "true"],
                &["oN", "error", "true"],
                &["n", "false", "false"],
                &["N", "false", "false"],
                &["no", "false", "false"],
                &["No", "false", "false"],
                &["NO", "false", "false"],
                &["nO", "error", "false"],
                &["off", "false", "false"],
                &["Off", "false", "false"],
                &["OFF", "false", "false"],
                &["oFf", "error", "false"],
                &["true", "true", "true"],
                &["True", "true", "true"],
                &["TRUE", "true", "true"],
                &["tRue", "error", "true"],
                &["false", "false", "false"],
                &["False", "false", "false"],
                &["FALSE", "false", "false"],
                &["fAlse", "error", "false"],
                &["1", "error", "error"],
                &["0", "error", "error"],
                &["t", "error", "error"],
                &["f", "error", "error"],
            ],
            "spelling, yaml.v3, serde-saphyr",
        )
    }

    /// yaml.v3 reads `1_000` as 1000 (it strips the underscores before `strconv.ParseInt`), and
    /// so does `serde-saphyr`.
    // Go: config/bidderinfo.go:323 (yaml.Unmarshal; yaml.v3's resolve strips '_' from integers)
    #[test]
    fn same_as_go_underscored_numbers() -> TestResult {
        let doc = golden("yaml.json")?;
        let mut rows = go_outcomes(&doc, "/underscored")?;
        for row in &mut rows {
            let input = row.first().cloned().unwrap_or_default();
            let parsed = serde_saphyr::from_str_with_options::<Count>(
                &format!("n: {input}\n"),
                yaml_options(),
            );
            row.push(parsed.map_or_else(|_| "error".to_owned(), |count| count.n.to_string()));
        }
        check_table(
            &rows,
            &[
                &["1_000", "1000", "1000"],
                &["1_000_000", "1000000", "1000000"],
                &["-1_0", "-10", "-10"],
            ],
            "input, yaml.v3, serde-saphyr",
        )
    }
}
