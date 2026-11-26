//! Errors returned by [`Conversions::get_rate`](crate::Conversions::get_rate).

use std::{error::Error, fmt};

use crate::iso4217::{ParseIsoError, Unit};

/// No rate, reverse rate or intermediate rate links the two currencies
/// (Go: `currency.ConversionNotFoundError`).
///
/// Go fills `FromCur`/`ToCur` with `Unit.String()`, so they are always canonical codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConversionNotFoundError {
    /// Go: `FromCur`.
    pub from_cur: Unit,
    /// Go: `ToCur`.
    pub to_cur: Unit,
}

impl fmt::Display for ConversionNotFoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Go: currency/errors.go:12
        write!(
            f,
            "Currency conversion rate not found: '{}' => '{}'",
            self.from_cur, self.to_cur
        )
    }
}

impl Error for ConversionNotFoundError {}

/// Every error a Go `Conversions.GetRate` implementation in the currency package returns.
///
/// Callers that classify errors the way Go does: `AggregateConversions` and adapters such as
/// aduptech only special-case [`RateError::ConversionNotFound`]; the exchange reports all of
/// them with Go's unknown error code 999.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateError {
    /// Go: x/text `errSyntax`, `currency: tag is not well-formed`.
    NotWellFormed,
    /// Go: x/text `errValue`, `currency: tag is not a recognized currency`.
    NotRecognized,
    /// Go: `errors.New("rates are nil")` from `Rates.GetRate` when the conversions map is nil.
    RatesAreNil,
    /// Go: [`ConversionNotFoundError`].
    ConversionNotFound(ConversionNotFoundError),
}

impl RateError {
    /// The missing-rate error, if this is one (Go: `err.(ConversionNotFoundError)`).
    #[must_use]
    pub const fn as_conversion_not_found(&self) -> Option<&ConversionNotFoundError> {
        match self {
            Self::ConversionNotFound(err) => Some(err),
            Self::NotWellFormed | Self::NotRecognized | Self::RatesAreNil => None,
        }
    }
}

impl fmt::Display for RateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotWellFormed => ParseIsoError::NotWellFormed.fmt(f),
            Self::NotRecognized => ParseIsoError::NotRecognized.fmt(f),
            Self::RatesAreNil => f.write_str("rates are nil"),
            Self::ConversionNotFound(err) => err.fmt(f),
        }
    }
}

impl Error for RateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ConversionNotFound(err) => Some(err),
            Self::NotWellFormed | Self::NotRecognized | Self::RatesAreNil => None,
        }
    }
}

impl From<ParseIsoError> for RateError {
    fn from(err: ParseIsoError) -> Self {
        match err {
            ParseIsoError::NotWellFormed => Self::NotWellFormed,
            ParseIsoError::NotRecognized => Self::NotRecognized,
        }
    }
}

impl From<ConversionNotFoundError> for RateError {
    fn from(err: ConversionNotFoundError) -> Self {
        Self::ConversionNotFound(err)
    }
}

#[cfg(test)]
mod tests {
    use super::{ConversionNotFoundError, RateError};
    use crate::{
        golden::{TestResult, check_eq},
        iso4217::parse_iso,
    };

    #[test]
    fn messages_match_go() -> TestResult {
        // Go: currency/errors.go:11-13 (ConversionNotFoundError.Error) and
        // golang.org/x/text@v0.37.0/currency/currency.go:87-88 (errSyntax, errValue)
        let not_found = ConversionNotFoundError {
            from_cur: parse_iso("ron")?,
            to_cur: parse_iso("EUR")?,
        };
        check_eq(
            &not_found.to_string(),
            &"Currency conversion rate not found: 'RON' => 'EUR'".to_owned(),
            "ConversionNotFoundError",
        )?;
        check_eq(
            &RateError::from(not_found).to_string(),
            &not_found.to_string(),
            "RateError::ConversionNotFound",
        )?;
        check_eq(
            &RateError::NotWellFormed.to_string(),
            &"currency: tag is not well-formed".to_owned(),
            "NotWellFormed",
        )?;
        check_eq(
            &RateError::NotRecognized.to_string(),
            &"currency: tag is not a recognized currency".to_owned(),
            "NotRecognized",
        )?;
        check_eq(
            &RateError::RatesAreNil.to_string(),
            &"rates are nil".to_owned(),
            "RatesAreNil",
        )
    }
}
