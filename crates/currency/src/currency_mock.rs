//! Port of Go `currency/currency_mock.go`.

use crate::rate_converter::{HttpClient, HttpResponse, ResponseFuture};

/// An [`HttpClient`] that answers every request with `200 OK` and a fixed body (Go:
/// `currency.MockCurrencyRatesHttpClient`, which other packages' tests use too).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MockCurrencyRatesHttpClient {
    /// Go: `ResponseBody`.
    pub response_body: String,
}

impl HttpClient for MockCurrencyRatesHttpClient {
    fn get<'a>(&'a self, _url: &'a str) -> ResponseFuture<'a> {
        let body = self.response_body.clone().into_bytes();
        Box::pin(async move { Ok(HttpResponse::from_bytes(200, body)) })
    }
}
