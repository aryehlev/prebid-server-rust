//! Port of Go `currency/converter_info.go`.

use std::{sync::Arc, time::SystemTime};

use crate::rates::Rates;

/// What a [`RateConverter`](crate::RateConverter) reports about itself for `/currency/rates`
/// (Go: the `currency.ConverterInfo` interface and its `converterInfo` implementation).
///
/// Go's `AdditionalInfo()` is always nil for the rate converter, so it has no counterpart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConverterInfo {
    source: String,
    last_updated: Option<SystemTime>,
    rates: Option<Arc<Rates>>,
}

impl ConverterInfo {
    /// Info with the given source URL, last update time and rates table.
    #[must_use]
    pub const fn new(
        source: String,
        last_updated: Option<SystemTime>,
        rates: Option<Arc<Rates>>,
    ) -> Self {
        Self {
            source,
            last_updated,
            rates,
        }
    }

    /// The URL the rates are fetched from (Go: `Source`).
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// When the rates were last fetched (Go: `LastUpdated`; `None` is Go's zero `time.Time`,
    /// which the endpoint still renders as `"0001-01-01T00:00:00Z"`).
    #[must_use]
    pub const fn last_updated(&self) -> Option<SystemTime> {
        self.last_updated
    }

    /// The fetched rates (Go: `Rates`; `None` is a nil pointer, which the endpoint omits, while
    /// `Some` with `conversions: None` is a pointer to a nil map, which it renders as `null`).
    #[must_use]
    pub fn rates(&self) -> Option<&Rates> {
        self.rates.as_deref()
    }
}
