//! Port of Go `currency/constant_rates.go`.

use crate::{
    errors::{ConversionNotFoundError, RateError},
    iso4217::parse_iso,
    rate_converter::Conversions,
    rates::Rates,
};

/// Converts nothing: only a currency to itself has a rate (Go: `currency.ConstantRates`).
///
/// [`RateConverter::rates`](crate::RateConverter::rates) returns it before the first successful
/// fetch and after the fetched rates go stale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConstantRates;

impl ConstantRates {
    /// Go: `NewConstantRates`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Conversions for ConstantRates {
    /// Go: `ConstantRates.GetRate`: 1 for the same currency, otherwise
    /// [`ConversionNotFoundError`].
    fn get_rate(&self, from: &str, to: &str) -> Result<f64, RateError> {
        let from_unit = parse_iso(from)?;
        let to_unit = parse_iso(to)?;
        if from_unit != to_unit {
            return Err(ConversionNotFoundError {
                from_cur: from_unit,
                to_cur: to_unit,
            }
            .into());
        }
        Ok(1.0)
    }

    /// Go: `ConstantRates.GetRates` returns nil.
    fn get_rates(&self) -> Option<&Rates> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::ConstantRates;
    use crate::{
        golden::{TestResult, check, check_eq, read_golden, render_rate},
        rate_converter::Conversions as _,
    };

    #[test]
    fn get_rate() -> TestResult {
        // Go: TestGetRate_ConstantRates
        let rates = ConstantRates::new();
        let cases: [(&str, &str, Option<f64>); 9] = [
            ("USD", "GBP", None),
            ("GBP", "USD", None),
            ("GBP", "EUR", None),
            ("CNY", "EUR", None),
            ("", "EUR", None),
            ("CNY", "", None),
            ("", "", None),
            ("USD", "USD", Some(1.0)),
            ("EUR", "EUR", Some(1.0)),
        ];
        for (from, to, expected) in cases {
            check_eq(
                &rates.get_rate(from, to).ok(),
                &expected,
                &format!("{from}->{to}"),
            )?;
        }
        Ok(())
    }

    #[test]
    fn get_rate_not_valid_iso_currency() -> TestResult {
        // Go: TestGetRate_ConstantRates_NotValidISOCurrency
        let rates = ConstantRates::new();
        check(rates.get_rate("foo", "foo").is_err(), "foo->foo")?;
        check(rates.get_rate("bar", "foo").is_err(), "bar->foo")
    }

    #[test]
    fn get_rates_is_nil() -> TestResult {
        // Go: currency/constant_rates.go:37-39 (GetRates is nil)
        check(
            ConstantRates::new().get_rates().is_none(),
            "ConstantRates has no rates",
        )
    }

    #[test]
    fn go_golden_constant_rates() -> TestResult {
        // Go: currency/constant_rates.go:19-34 (GetRate)
        let mut checked = 0_usize;
        for fields in read_golden("get_rate.golden")? {
            if let [kind, from, to, expected] = fields.as_slice()
                && kind == "const"
            {
                let actual = render_rate(ConstantRates::new().get_rate(from, to));
                check_eq(&actual, expected, &format!("{from:?}->{to:?}"))?;
                checked += 1;
            }
        }
        check(
            checked > 300,
            "the golden must hold the ConstantRates cases",
        )
    }
}
