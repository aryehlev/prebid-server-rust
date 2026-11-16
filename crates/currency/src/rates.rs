//! Port of Go `currency/rates.go`.

use std::collections::BTreeMap;

use crate::{
    errors::{ConversionNotFoundError, RateError},
    iso4217::{Unit, parse_iso},
    rate_converter::Conversions,
};

/// Go: `map[string]map[string]float64`, from-currency to to-currency to rate.
///
/// `None` inner values are Go nil inner maps (JSON `null`), which behave like empty maps for
/// lookups but marshal as `null`. The map is ordered by key bytes, which is the order Go's
/// JSON encoder writes and the order this port uses wherever Go iterates the map randomly.
pub type ConversionRates = BTreeMap<String, Option<BTreeMap<String, f64>>>;

/// Currency rates, as in the currency file `{"conversions": {...}}` (Go: `currency.Rates`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rates {
    /// Go: `Conversions`; `None` is Go's nil map, which [`Conversions::get_rate`] reports as
    /// `rates are nil`, unlike an empty map.
    pub conversions: Option<ConversionRates>,
}

impl Rates {
    /// Go: `NewRates`.
    #[must_use]
    pub const fn new(conversions: Option<ConversionRates>) -> Self {
        Self { conversions }
    }

    /// Looks up `conversions[from][to]` like a Go map index (nil maps are empty).
    fn lookup(&self, from: Unit, to: Unit) -> Option<f64> {
        self.conversions
            .as_ref()?
            .get(from.as_str())?
            .as_ref()?
            .get(to.as_str())
            .copied()
    }
}

/// Finds `from` -> `to` through any currency whose rate map contains both
/// (Go: `FindIntermediateConversionRate`), returning `rates[to] / rates[from]`.
///
/// Go walks `r.Conversions` in Go's randomized map order and returns the first pivot that has
/// both currencies. This port walks the pivots in key order, so it returns the pivot with the
/// smallest code. That is always one of the results Go can return, and the results only differ
/// when two or more pivots contain both currencies with inconsistent cross rates.
///
/// # Errors
///
/// [`ConversionNotFoundError`] when no pivot has both currencies.
pub fn find_intermediate_conversion_rate(
    r: &Rates,
    from: Unit,
    to: Unit,
) -> Result<f64, ConversionNotFoundError> {
    let not_found = ConversionNotFoundError {
        from_cur: from,
        to_cur: to,
    };
    let Some(conversions) = &r.conversions else {
        return Err(not_found);
    };
    conversions
        .values()
        .flatten()
        .find_map(|rates| Some(rates.get(to.as_str())? / rates.get(from.as_str())?))
        .ok_or(not_found)
}

impl Conversions for Rates {
    /// Go: `Rates.GetRate`: 1 for the same currency, then the direct rate, then the reciprocal
    /// of the reverse rate, then [`find_intermediate_conversion_rate`].
    fn get_rate(&self, from: &str, to: &str) -> Result<f64, RateError> {
        let from_unit = parse_iso(from)?;
        let to_unit = parse_iso(to)?;
        if from_unit == to_unit {
            return Ok(1.0);
        }
        if self.conversions.is_none() {
            return Err(RateError::RatesAreNil);
        }
        if let Some(conversion) = self.lookup(from_unit, to_unit) {
            return Ok(conversion);
        }
        if let Some(conversion) = self.lookup(to_unit, from_unit) {
            return Ok(1.0 / conversion);
        }
        Ok(find_intermediate_conversion_rate(self, from_unit, to_unit)?)
    }

    /// Go: `Rates.GetRates` returns `&r.Conversions`, never a nil pointer.
    fn get_rates(&self) -> Option<&Rates> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{Rates, find_intermediate_conversion_rate};
    use crate::{
        errors::RateError,
        golden::{
            TestResult, check, check_eq, check_outcome, conversion_rates, hex_decode,
            rates_from_json, read_golden, render_rate,
        },
        iso4217::parse_iso,
        rate_converter::Conversions as _,
    };

    #[test]
    fn get_rate() -> TestResult {
        // Go: TestGetRate
        let rates = Rates::new(Some(conversion_rates(&[
            ("USD", &[("GBP", 0.77208)]),
            ("GBP", &[("USD", 1.2952)]),
        ])));
        let cases: [(&str, &str, Option<f64>); 8] = [
            ("USD", "GBP", Some(0.77208)),
            ("GBP", "USD", Some(1.2952)),
            ("GBP", "EUR", None),
            ("CNY", "EUR", None),
            ("", "EUR", None),
            ("CNY", "", None),
            ("", "", None),
            ("USD", "USD", Some(1.0)),
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
    fn get_rate_reverse_conversion() -> TestResult {
        // Go: TestGetRate_ReverseConversion
        let rates = Rates::new(Some(conversion_rates(&[
            ("USD", &[("GBP", 0.77208)]),
            ("EUR", &[("USD", 0.88723)]),
        ])));
        let cases = [
            (
                "USD",
                "GBP",
                0.77208,
                "case 1 - Rate is present directly and will be returned as is",
            ),
            (
                "EUR",
                "USD",
                0.88723,
                "case 2 - Rate is present directly and will be returned as is (2)",
            ),
            (
                "GBP",
                "USD",
                1.0 / 0.77208,
                "case 3 - Rate is not present but the reverse one is",
            ),
            (
                "USD",
                "EUR",
                1.0 / 0.88723,
                "case 4 - Rate is not present but the reverse one is (2)",
            ),
        ];
        for (from, to, expected, description) in cases {
            check_eq(&rates.get_rate(from, to), &Ok(expected), description)?;
        }
        Ok(())
    }

    #[test]
    fn get_rate_find_intermediate_conversion_rate() -> TestResult {
        // Go: TestGetRate_FindIntermediateConversionRate
        let rates = Rates::new(Some(conversion_rates(&[
            ("USD", &[("SEK", 10.23842), ("NOK", 10.47089)]),
            ("EUR", &[("THB", 35.23842), ("ZAR", 18.47089)]),
        ])));
        let cases = [
            (
                "in_same_intermediate_USD_currency",
                "NOK",
                "SEK",
                Some(0.977_798_448_842_457_4),
            ),
            (
                "in_same_intermediate_USD_currency_inverse",
                "SEK",
                "NOK",
                Some(1.0 / 0.977_798_448_842_457_4),
            ),
            (
                "in_same_intermediate_EUR_currency",
                "THB",
                "ZAR",
                Some(0.524_169_074_549_880_6),
            ),
            (
                "in_same_intermediate_EUR_currency_inverse",
                "ZAR",
                "THB",
                Some(1.0 / 0.524_169_074_549_880_6),
            ),
            ("in_different_intermediate_currencies", "NOK", "ZAR", None),
        ];
        for (description, from, to, expected) in cases {
            let rate = find_intermediate_conversion_rate(&rates, parse_iso(from)?, parse_iso(to)?);
            check_eq(&rate.ok(), &expected, description)?;
        }
        Ok(())
    }

    #[test]
    fn get_rate_empty_rates() -> TestResult {
        // Go: TestGetRate_EmptyRates
        check_eq(
            &Rates::new(None).get_rate("USD", "EUR"),
            &Err(RateError::RatesAreNil),
            "nil rates",
        )
    }

    #[test]
    fn get_rate_not_valid_iso_currency() -> TestResult {
        // Go: TestGetRate_NotValidISOCurrency
        let rates = Rates::new(None);
        check(rates.get_rate("foo", "foo").is_err(), "foo->foo")?;
        check(rates.get_rate("bar", "foo").is_err(), "bar->foo")
    }

    #[test]
    fn intermediate_pivot_is_the_smallest_code() -> TestResult {
        let rates = Rates::new(Some(conversion_rates(&[
            ("USD", &[("GBP", 0.8), ("SEK", 10.0)]),
            ("EUR", &[("GBP", 0.88), ("SEK", 11.5)]),
        ])));
        check_eq(
            &rates.get_rate("GBP", "SEK"),
            &Ok(11.5 / 0.88),
            "EUR is the first pivot",
        )
    }

    #[test]
    fn get_rates_points_at_the_conversions() -> TestResult {
        let rates = Rates::new(None);
        check_eq(
            &rates.get_rates().map(|r| r.conversions.is_none()),
            &Some(true),
            "nil map",
        )
    }

    #[test]
    fn go_golden_get_rate() -> TestResult {
        let mut tables = BTreeMap::new();
        let mut checked = 0_usize;
        for fields in read_golden("get_rate.golden")? {
            match fields.as_slice() {
                [kind, name, json] if kind == "table" => {
                    drop(tables.insert(name.clone(), rates_from_json(json)?));
                }
                [kind, table, from, to, expected] if kind == "rate" => {
                    let rates = tables.get(table).ok_or("unknown table")?;
                    let actual = render_rate(rates.get_rate(from, to));
                    check_outcome(&actual, expected, &format!("{table}: {from:?}->{to:?}"))?;
                    checked += 1;
                }
                _ => {}
            }
        }
        check(checked > 1000, "the golden must hold the GetRate cases")
    }

    #[test]
    fn hex_helper_round_trips() -> TestResult {
        check_eq(&hex_decode("55534400")?, &b"USD\0".to_vec(), "hex")
    }
}
