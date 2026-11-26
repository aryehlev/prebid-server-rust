//! Port of Go `currency/currency.go`.

use std::sync::Arc;

use crate::{
    aggregate_conversions::AggregateConversions,
    rate_converter::{Conversions, RateConverter},
    rates::{ConversionRates, Rates},
};

/// The request's `ext.prebid.currency` (Go: the fields of `openrtb_ext.ExtRequestCurrency`
/// that the currency package reads).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RequestCurrency<'a> {
    /// Go: `ConversionRates` (`rates`); `None` is a nil map.
    pub conversion_rates: Option<&'a ConversionRates>,
    /// Go: `UsePBSRates` (`usepbsrates`); `None` counts as `true`.
    pub use_pbs_rates: Option<bool>,
}

/// The conversions an auction uses (Go: `GetAuctionCurrencyRates`).
///
/// `None` (Go's nil) only when there is neither a converter nor a request currency object.
/// Without request rates the converter's rates are used; with `usepbsrates: false`, or without
/// a converter, only the request rates are; otherwise the request rates take precedence over the
/// converter's, unless the request rates are empty.
#[must_use]
pub fn get_auction_currency_rates(
    currency_converter: Option<&RateConverter>,
    request_rates: Option<RequestCurrency<'_>>,
) -> Option<Arc<dyn Conversions>> {
    let Some(request_rates) = request_rates else {
        // No bidRequest.ext.currency field was found, use PBS rates as usual.
        return currency_converter.map(RateConverter::rates);
    };
    let custom_rates = || Arc::new(Rates::new(request_rates.conversion_rates.cloned()));
    let Some(currency_converter) = currency_converter else {
        return Some(custom_rates());
    };
    if !request_rates.use_pbs_rates.unwrap_or(true) {
        return Some(custom_rates());
    }
    if request_rates
        .conversion_rates
        .is_none_or(ConversionRates::is_empty)
    {
        return Some(currency_converter.rates());
    }
    Some(Arc::new(AggregateConversions::new(
        custom_rates(),
        currency_converter.rates(),
    )))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{RequestCurrency, get_auction_currency_rates};
    use crate::{
        currency_mock::MockCurrencyRatesHttpClient,
        golden::{FAKE_URL, TestResult, check, check_eq, conversion_rates},
        rate_converter::RateConverter,
        rates::ConversionRates,
    };

    /// A test name, the converter, the request currency and the rates the result must have.
    type Case<'a> = (
        &'a str,
        Option<&'a RateConverter>,
        Option<RequestCurrency<'a>>,
        Option<&'a ConversionRates>,
    );

    fn pbs_rates() -> ConversionRates {
        conversion_rates(&[("MXN", &[("USD", 20.13), ("EUR", 27.82), ("JPY", 5.09)])])
    }

    fn custom_rates() -> ConversionRates {
        conversion_rates(&[("MXN", &[("USD", 25.00), ("EUR", 27.82), ("GBP", 31.12)])])
    }

    fn expected_rate_engine_rates() -> ConversionRates {
        conversion_rates(&[(
            "MXN",
            &[
                ("USD", 25.00),
                ("EUR", 27.82),
                ("JPY", 5.09),
                ("GBP", 31.12),
            ],
        )])
    }

    /// Go: `setupMockRateConverter`, which marshals the rates with `jsonutil.Marshal`.
    async fn mock_rate_converter() -> RateConverter {
        let body = concat!(
            r#"{"dataAsOf":"2018-09-12","conversions":"#,
            r#"{"MXN":{"EUR":27.82,"JPY":5.09,"USD":20.13}}}"#,
        );
        let client = MockCurrencyRatesHttpClient {
            response_body: body.to_owned(),
        }
        .into_client();
        let converter = RateConverter::new(
            client,
            Duration::from_secs(60),
            FAKE_URL.to_owned(),
            Duration::from_hours(24),
        );
        let _result = converter.run().await;
        converter
    }

    #[tokio::test]
    async fn auction_currency_rates() -> TestResult {
        // Go: TestGetAuctionCurrencyRates
        let custom = custom_rates();
        let pbs = pbs_rates();
        let combined = expected_rate_engine_rates();
        let converter = mock_rate_converter().await;
        let request = |rates, use_pbs_rates| RequestCurrency {
            conversion_rates: rates,
            use_pbs_rates,
        };
        let cases: [Case<'_>; 10] = [
            (
                "valid ConversionRates, valid pbsRates, false UsePBSRates. \
                 Resulting rates identical to customRates",
                Some(&converter),
                Some(request(Some(&custom), Some(false))),
                Some(&custom),
            ),
            (
                "valid ConversionRates, valid pbsRates, true UsePBSRates. \
                 Resulting rates are a mix but customRates gets priority",
                Some(&converter),
                Some(request(Some(&custom), Some(true))),
                Some(&combined),
            ),
            (
                "valid ConversionRates, nil pbsRates, false UsePBSRates. \
                 Resulting rates identical to customRates",
                None,
                Some(request(Some(&custom), Some(false))),
                Some(&custom),
            ),
            (
                "valid ConversionRates, nil pbsRates, true UsePBSRates. \
                 Resulting rates identical to customRates",
                None,
                Some(request(Some(&custom), Some(true))),
                Some(&custom),
            ),
            (
                "empty ConversionRates, valid pbsRates, false UsePBSRates. \
                 Because pbsRates cannot be used, disable currency conversion",
                Some(&converter),
                Some(request(None, Some(false))),
                None,
            ),
            (
                "nil ConversionRates, valid pbsRates, true UsePBSRates. \
                 Resulting rates will be identical to pbsRates",
                Some(&converter),
                None,
                Some(&pbs),
            ),
            (
                "empty ConversionRates, nil pbsRates, false UsePBSRates. \
                 No conversion rates available, disable currency conversion",
                Some(&converter),
                Some(request(None, Some(false))),
                None,
            ),
            (
                "empty ConversionRates, nil pbsRates, true UsePBSRates. \
                 No conversion rates available, disable currency conversion",
                None,
                Some(request(None, Some(true))),
                None,
            ),
            (
                "nil customRates, nil pbsRates. \
                 No conversion rates available, disable currency conversion",
                None,
                None,
                None,
            ),
            (
                "empty ConversionRates, valid pbsRates, true UsePBSRates. \
                 Resulting rates will be identical to pbsRates",
                Some(&converter),
                Some(request(None, Some(true))),
                Some(&pbs),
            ),
        ];
        for (name, currency_converter, request_rates, assert_rates) in cases {
            let auction_rates = get_auction_currency_rates(currency_converter, request_rates);
            if currency_converter.is_none() && request_rates.is_none() && assert_rates.is_none() {
                check(auction_rates.is_none(), name)?;
                continue;
            }
            let auction_rates = auction_rates.ok_or(name)?;
            match assert_rates {
                None => check(auction_rates.get_rate("USD", "MXN").is_err(), name)?,
                Some(rates) => {
                    for (from, inner) in rates {
                        for (to, expected) in inner.iter().flatten() {
                            check_eq(&auction_rates.get_rate(from, to), &Ok(*expected), name)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn empty_custom_rates_use_the_server_rates() -> TestResult {
        let converter = mock_rate_converter().await;
        let empty = ConversionRates::new();
        let request = RequestCurrency {
            conversion_rates: Some(&empty),
            use_pbs_rates: None,
        };
        let rates = get_auction_currency_rates(Some(&converter), Some(request)).ok_or("rates")?;
        check_eq(&rates.get_rate("MXN", "JPY"), &Ok(5.09), "server rate")?;
        check(rates.get_rates().is_some(), "the server rates themselves")
    }
}
