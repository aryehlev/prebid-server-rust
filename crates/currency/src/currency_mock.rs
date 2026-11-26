//! Port of Go `currency/currency_mock.go`.

use std::{future, sync::Arc};

use bytes::Bytes;
use futures_util::future::BoxFuture;
use pbs_http_client::{
    Client, RequestError, Response, Trace, Transport,
    reqwest::{Request, StatusCode, header::HeaderMap},
};
use tokio::time::Instant;

/// A [`Transport`] that answers every request with `200 OK` and a fixed body (Go:
/// `currency.MockCurrencyRatesHttpClient`, which other packages' tests use too).
///
/// Go's mock is the whole client (`Do`), so Go's tests may hand the converter a URL without a
/// scheme (`currency.fake.com`); this one is the transport under a [`Client`], and the
/// converter parses the URL before sending, so the URL must be absolute.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MockCurrencyRatesHttpClient {
    /// Go: `ResponseBody`.
    pub response_body: String,
}

impl MockCurrencyRatesHttpClient {
    /// A client that sends through this mock, for
    /// [`RateConverter::new`](crate::RateConverter::new): Go's
    /// `&currency.MockCurrencyRatesHttpClient{ResponseBody: ...}`.
    #[must_use]
    pub fn into_client(self) -> Client {
        Client::from_transport(Arc::new(self))
    }
}

impl Transport for MockCurrencyRatesHttpClient {
    fn round_trip(
        &self,
        request: Request,
        _deadline: Option<Instant>,
        _trace: Option<Trace>,
    ) -> BoxFuture<'_, Result<Response, RequestError>> {
        let response = Response::new(
            request.url().clone(),
            StatusCode::OK,
            HeaderMap::new(),
            Bytes::from(self.response_body.clone()),
        );
        Box::pin(future::ready(Ok(response)))
    }
}
