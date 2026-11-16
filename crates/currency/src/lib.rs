//! Currency rates, conversion and rate fetching matching Prebid Server Go.
//!
//! A port of the Go `currency` package together with the pieces of its dependencies whose
//! behaviour shows through it:
//!
//! - [`parse_iso`] validates currency codes exactly like `golang.org/x/text/currency.ParseISO`
//!   (CLDR 32 table, case-insensitive, Go's error texts).
//! - [`Conversions`] is Go's interface for conversion rates, implemented by [`Rates`] (direct,
//!   reverse and intermediate rates), [`ConstantRates`] (only a currency to itself) and
//!   [`AggregateConversions`] (request rates in front of the server's).
//! - [`RateConverter`] fetches the currency file through an injected [`HttpClient`], decodes it
//!   exactly like Go's `jsonutil.UnmarshalValid` (json-iterator), logs failures with Go's texts
//!   through the `log` facade, and falls back to [`ConstantRates`] before the first fetch and once
//!   the fetched rates are stale. [`TickerTask`] runs it periodically on tokio.
//! - [`get_auction_currency_rates`] and [`validate_custom_rates`] handle the request's
//!   `ext.prebid.currency`.
//!
//! Go ranges over its rate maps in random order in two places: the intermediate-currency
//! search and the custom rate validation. This port uses key order ([`ConversionRates`] is a
//! `BTreeMap`), which always yields one of the results Go can produce; see
//! [`find_intermediate_conversion_rate`] and [`validate_custom_rates`].
//!
//! ```
//! use pbs_currency::{Conversions as _, RateError, Rates};
//!
//! let rates = Rates::new(Some(
//!     [(
//!         "USD".to_owned(),
//!         Some([("GBP".to_owned(), 0.8)].into_iter().collect()),
//!     )]
//!     .into_iter()
//!     .collect(),
//! ));
//! // The reverse of USD -> GBP, with case-insensitive codes: 1.25.
//! let rate = rates.get_rate("gbp", "USD")?;
//! # let _ = rate;
//! # Ok::<(), RateError>(())
//! ```

mod aggregate_conversions;
mod constant_rates;
mod converter_info;
mod currency;
mod currency_mock;
mod errors;
#[cfg(test)]
pub mod golden;
mod iso4217;
mod rate_converter;
mod rates;
mod ticker_task;
mod validation;

pub use aggregate_conversions::AggregateConversions;
pub use constant_rates::ConstantRates;
pub use converter_info::ConverterInfo;
pub use currency::{RequestCurrency, get_auction_currency_rates};
pub use currency_mock::MockCurrencyRatesHttpClient;
pub use errors::{ConversionNotFoundError, RateError};
pub use iso4217::{ParseIsoError, Unit, parse_iso};
pub use rate_converter::{
    BodyFuture, BoxError, Conversions, FetchError, HttpClient, HttpResponse, RateConverter,
    ResponseFuture,
};
pub use rates::{ConversionRates, Rates, find_intermediate_conversion_rate};
pub use ticker_task::TickerTask;
pub use validation::{InvalidCurrencyCodeError, validate_custom_rates};
