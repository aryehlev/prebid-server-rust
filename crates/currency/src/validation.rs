//! Port of Go `currency/validation.go`.

use std::{error::Error, fmt};

use crate::{iso4217::parse_iso, rates::ConversionRates};

/// A custom rate names a code that is not a recognized ISO 4217 currency.
///
/// Go returns it as `*errortypes.BadInput` (error code 2) with this message.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InvalidCurrencyCodeError {
    /// The rejected code, as written in the request.
    pub code: String,
}

impl fmt::Display for InvalidCurrencyCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "currency code {} is not recognized or malformed",
            self.code
        )
    }
}

impl Error for InvalidCurrencyCodeError {}

/// Checks that every code in the request's custom rates is a valid ISO 4217 code
/// (Go: `ValidateCustomRates`); `None` stands for both a nil `ExtRequestCurrency` and a nil
/// rates map.
///
/// Go ranges over the maps in random order and reports the first invalid code it meets, a
/// from-currency before the currencies it maps to. This port goes in key order, so it reports
/// the invalid code that sorts first among those Go may report.
///
/// # Errors
///
/// [`InvalidCurrencyCodeError`] for the first invalid code.
pub fn validate_custom_rates(
    conversion_rates: Option<&ConversionRates>,
) -> Result<(), InvalidCurrencyCodeError> {
    let check = |code: &String| match parse_iso(code) {
        Ok(_unit) => Ok(()),
        Err(_err) => Err(InvalidCurrencyCodeError { code: code.clone() }),
    };
    for (from_currency, rates) in conversion_rates.into_iter().flatten() {
        // Check if fromCurrency is a valid 3-letter currency code.
        check(from_currency)?;
        // Check if currencies mapped to fromCurrency are valid 3-letter currency codes.
        for to_currency in rates.iter().flat_map(|rates| rates.keys()) {
            check(to_currency)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{InvalidCurrencyCodeError, validate_custom_rates};
    use crate::{
        golden::{
            TestResult, check, check_outcome, conversion_rates, hex_decode, rates_from_json,
            read_golden,
        },
        rates::ConversionRates,
    };

    #[test]
    fn validate_custom_rates_cases() -> TestResult {
        // Go: TestValidateCustomRates (UsePBSRates does not affect the validation).
        let invalid = |code: &str| {
            Err(InvalidCurrencyCodeError {
                code: code.to_owned(),
            })
        };
        let empty = ConversionRates::new();
        let invalid_from =
            conversion_rates(&[("FOO", &[("GBP", 1.2), ("MXN", 0.05), ("JPY", 0.95)])]);
        let invalid_to = conversion_rates(&[("USD", &[("FOO", 10.0), ("MXN", 0.05)])]);
        let both = conversion_rates(&[("FOO", &[("MXN", 0.05), ("CAD", 0.95)])]);
        let valid = conversion_rates(&[
            ("USD", &[("MXN", 0.05)]),
            ("MXN", &[("JPY", 10.0), ("EUR", 10.95)]),
        ]);
        let cases = [
            ("nil input, no errors expected", None, Ok(())),
            (
                "empty custom currency rates but UsePBSRates is set to false",
                Some(&empty),
                Ok(()),
            ),
            (
                "UsePBSRates is nil and defaults to true, bidExt fromCurrency is invalid",
                Some(&invalid_from),
                invalid("FOO"),
            ),
            (
                "UsePBSRates set to false, some of the bidExt 'to' Currencies are invalid",
                Some(&invalid_to),
                invalid("FOO"),
            ),
            (
                "some of the bidExt 'from' and 'to' currencies are invalid",
                Some(&both),
                invalid("FOO"),
            ),
            (
                "All 3-digit currency codes exist, expect no error",
                Some(&valid),
                Ok(()),
            ),
        ];
        for (description, rates, expected) in cases {
            check(validate_custom_rates(rates) == expected, description)?;
        }
        check(
            invalid("FOO").err().map(|err| err.to_string())
                == Some("currency code FOO is not recognized or malformed".to_owned()),
            "message",
        )
    }

    #[test]
    fn go_golden_validation() -> TestResult {
        let mut checked = 0_usize;
        for fields in read_golden("validation.golden")? {
            if let [kind, json, expected] = fields.as_slice()
                && kind == "validate"
            {
                let json = String::from_utf8(hex_decode(json)?)?;
                let rates = rates_from_json(&json)?;
                let actual = match validate_custom_rates(rates.conversions.as_ref()) {
                    Ok(()) => "ok".to_owned(),
                    Err(err) => format!("err:{err}"),
                };
                check_outcome(&actual, expected, &json)?;
                checked += 1;
            }
        }
        check(checked == 10, "the golden must hold the validation cases")
    }

    #[test]
    fn first_invalid_code_in_key_order() -> TestResult {
        let rates = conversion_rates(&[
            ("USD", &[("BAR", 1.0), ("FOO", 2.0)]),
            ("EUR", &[("US", 1.0)]),
        ]);
        check(
            validate_custom_rates(Some(&rates))
                == Err(InvalidCurrencyCodeError {
                    code: "US".to_owned(),
                }),
            "EUR sorts before USD, so its invalid US is reported",
        )
    }
}
