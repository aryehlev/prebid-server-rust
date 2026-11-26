//! Port of Go `currency/aggregate_conversions.go`.

use std::sync::Arc;

use crate::{errors::RateError, rate_converter::Conversions, rates::Rates};

/// The request's custom rates (`ext.prebid.currency.rates`) in front of the server's fetched
/// rates (Go: `currency.AggregateConversions`).
#[derive(Debug, Clone)]
pub struct AggregateConversions {
    custom_rates: Arc<dyn Conversions>,
    server_rates: Arc<dyn Conversions>,
}

impl AggregateConversions {
    /// Go: `NewAggregateConversions(customRates, pbsRates)`.
    #[must_use]
    pub const fn new(custom_rates: Arc<dyn Conversions>, pbs_rates: Arc<dyn Conversions>) -> Self {
        Self {
            custom_rates,
            server_rates: pbs_rates,
        }
    }
}

impl Conversions for AggregateConversions {
    /// Go: `AggregateConversions.GetRate`: the custom rate when there is one; only a
    /// [`RateError::ConversionNotFound`] from the custom rates falls back to the server rates,
    /// every other error (malformed codes, `rates are nil`) is returned as is.
    fn get_rate(&self, from: &str, to: &str) -> Result<f64, RateError> {
        match self.custom_rates.get_rate(from, to) {
            Ok(rate) => Ok(rate),
            Err(RateError::ConversionNotFound(_)) => self.server_rates.get_rate(from, to),
            Err(
                err
                @ (RateError::NotWellFormed | RateError::NotRecognized | RateError::RatesAreNil),
            ) => Err(err),
        }
    }

    /// Go: `AggregateConversions.GetRates` returns nil.
    fn get_rates(&self) -> Option<&Rates> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use super::AggregateConversions;
    use crate::{
        errors::{ConversionNotFoundError, RateError},
        golden::{
            RATE_CORPORA, TestResult, check, check_eq, check_outcome, check_random_corpus,
            conversion_rates, fetch_rates, random_rate_cases, rates_from_json, read_golden,
            render_rate,
        },
        iso4217::parse_iso,
        rate_converter::Conversions as _,
        rates::Rates,
    };

    #[test]
    fn grouped_get_rate() -> TestResult {
        // Go: TestGroupedGetRate
        let custom = Rates::new(Some(conversion_rates(&[(
            "USD",
            &[("GBP", 3.0), ("EUR", 2.0)],
        )])));
        let pbs = Rates::new(Some(conversion_rates(&[(
            "USD",
            &[("GBP", 4.0), ("MXN", 10.0)],
        )])));
        let aggregate = AggregateConversions::new(Arc::new(custom), Arc::new(pbs));
        let not_found = RateError::ConversionNotFound(ConversionNotFoundError {
            from_cur: parse_iso("RON")?,
            to_cur: parse_iso("EUR")?,
        });
        let not_found_reverse = RateError::ConversionNotFound(ConversionNotFoundError {
            from_cur: parse_iso("EUR")?,
            to_cur: parse_iso("RON")?,
        });
        let cases = [
            ("Found in both, return custom rate", "USD", "GBP", Ok(3.0)),
            (
                "Found in both, return inverse custom rate",
                "GBP",
                "USD",
                Ok(1.0 / 3.0),
            ),
            ("Found in custom rates only", "USD", "EUR", Ok(2.0)),
            ("Found in PBS rates only", "USD", "MXN", Ok(10.0)),
            (
                "Found in PBS rates only, return inverse",
                "MXN",
                "USD",
                Ok(1.0 / 10.0),
            ),
            ("Same currency, return unitary rate", "USD", "USD", Ok(1.0)),
            (
                "From-currency three-digit code malformed",
                "XX",
                "EUR",
                Err(RateError::NotWellFormed),
            ),
            (
                "To-currency three-digit code malformed",
                "GBP",
                "",
                Err(RateError::NotWellFormed),
            ),
            (
                "Both currencies malformed",
                "",
                "",
                Err(RateError::NotWellFormed),
            ),
            (
                "From-currency three-digit code not found",
                "FOO",
                "EUR",
                Err(RateError::NotRecognized),
            ),
            (
                "To-currency three-digit code not found",
                "GBP",
                "BAR",
                Err(RateError::NotRecognized),
            ),
            (
                "Valid codes, but conversion from-currency rate not found",
                "RON",
                "EUR",
                Err(not_found),
            ),
            (
                "Valid codes, but conversion to-currency rate not found",
                "EUR",
                "RON",
                Err(not_found_reverse),
            ),
            (
                "Valid codes, intermediate conversion rate has been used",
                "GBP",
                "EUR",
                Ok(2.0 / 3.0),
            ),
        ];
        for (description, from, to, expected) in cases {
            check_eq(&aggregate.get_rate(from, to), &expected, description)?;
        }
        Ok(())
    }

    #[test]
    fn nil_custom_rates_do_not_fall_back() -> TestResult {
        // Go: currency/aggregate_conversions.go:22-41 (GetRate falls back only on a
        // ConversionNotFoundError; GetRates is nil)
        let pbs = Rates::new(Some(conversion_rates(&[("USD", &[("GBP", 4.0)])])));
        let aggregate = AggregateConversions::new(Arc::new(Rates::new(None)), Arc::new(pbs));
        check_eq(
            &aggregate.get_rate("USD", "GBP"),
            &Err(RateError::RatesAreNil),
            "rates are nil",
        )?;
        check(
            aggregate.get_rates().is_none(),
            "AggregateConversions has no rates",
        )
    }

    #[test]
    fn go_golden_aggregate_conversions() -> TestResult {
        // Go: currency/aggregate_conversions.go:22-35 (GetRate over the get_rate.golden tables)
        let mut tables = BTreeMap::new();
        let mut checked = 0_usize;
        for fields in read_golden("get_rate.golden")? {
            match fields.as_slice() {
                [kind, name, json] if kind == "table" => {
                    drop(tables.insert(name.clone(), Arc::new(rates_from_json(json)?)));
                }
                [kind, custom, server, from, to, expected] if kind == "agg" => {
                    let custom_rates = tables.get(custom).ok_or("unknown custom table")?;
                    let server_rates = tables.get(server).ok_or("unknown server table")?;
                    let aggregate = AggregateConversions::new(
                        Arc::<Rates>::clone(custom_rates),
                        Arc::<Rates>::clone(server_rates),
                    );
                    let actual = render_rate(aggregate.get_rate(from, to));
                    check_outcome(
                        &actual,
                        expected,
                        &format!("{custom}+{server}: {from:?}->{to:?}"),
                    )?;
                    checked += 1;
                }
                _ => {}
            }
        }
        check(
            checked > 900,
            "the golden must hold the AggregateConversions cases",
        )
    }

    #[tokio::test]
    async fn go_random_aggregate_conversions() -> TestResult {
        // Go: currency/aggregate_conversions.go:22-35 (GetRate)
        // The random tables of rates::tests::go_random_get_rate, the first as the request
        // rates and the second as the server's: precedence when both have a rate, fallback
        // only on a missing rate (not on "rates are nil" or a bad code).
        for corpus in RATE_CORPORA {
            let mut outcomes = Vec::new();
            for case in random_rate_cases(corpus) {
                let aggregate = AggregateConversions::new(
                    Arc::new(fetch_rates(&case.custom).await?),
                    Arc::new(fetch_rates(&case.server).await?),
                );
                outcomes.push((
                    case.input,
                    render_rate(aggregate.get_rate(case.from, case.to)),
                ));
            }
            check_random_corpus(&format!("{}-agg", corpus.name), &outcomes)?;
        }
        Ok(())
    }
}
