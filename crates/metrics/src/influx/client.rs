//! The reporter's client (Go: `github.com/influxdata/influxdb1-client` 8bf82d3c094d, `NewClient`,
//! `Write` and `Ping`): influxdb 0.8.0's client for the writes and a ping of its own, over one
//! reqwest client.

use std::io;

use base64::{Engine as _, prelude::BASE64_STANDARD};
use influxdb::WriteQuery;
use reqwest::{
    Url,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
    redirect::Policy,
};

/// Go: the `User-Agent` of influxdb1-client's requests (`influxdb.go:187-189` there): what the
/// reporter sends (D-UA, `influx.user-agent`).
pub const USER_AGENT: &str = "InfluxDBClient";

/// Go: net/http's `defaultCheckRedirect` sends at most ten requests, the first and nine
/// redirects; reqwest's limit counts the redirects alone.
const MAX_REDIRECTS: usize = 9;

/// Go: `client.Client` as `makeClient` configures it (the URL, the username and the password),
/// with the database of the reporter's writes.
#[derive(Debug, Clone)]
pub struct Client {
    /// The crate's client, which writes to `<root>/write`.
    influx: influxdb::Client,
    /// The HTTP client of both, for the pings.
    http: reqwest::Client,
    /// Go: `path.Join(u.Path, "ping")` with the URL's query.
    ping: Url,
}

impl Client {
    /// Go: `client.NewClient(client.Config{URL: url, Username: username, Password: password})`.
    ///
    /// The requests carry `User-Agent: InfluxDBClient` and, when `username` is not empty, its
    /// basic auth; otherwise the URL's user info, as Go's transport sends it when no
    /// `Authorization` header is set. The writes go to `<path>/write` without the URL's query
    /// and the pings to `<path>/ping` with it, the path without its trailing slashes
    /// (`influx.host-url`). No proxy, no timeout and HTTP/1.1, as influxdb1-client's transport,
    /// and Go's ten requests at most when the server redirects.
    ///
    /// # Errors
    ///
    /// When reqwest cannot build the client (Go's `NewClient` cannot fail).
    pub fn new(url: &Url, username: &str, password: &str, database: &str) -> io::Result<Self> {
        let mut base = url.clone();
        base.set_fragment(None);
        let mut headers = HeaderMap::new();
        if !username.is_empty() {
            // Go: influxdb.go:332-334 there (SetBasicAuth), which wins over the URL's user info:
            // reqwest would send that over a default header.
            base = without_user_info(&base);
            drop(headers.insert(AUTHORIZATION, basic_auth(username, password)?));
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .http1_only()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .redirect(Policy::limited(MAX_REDIRECTS))
            .build()
            .map_err(io::Error::other)?;
        let ping = endpoint(&base, "ping");
        base.set_query(None);
        let root = base.as_str().trim_end_matches('/');
        let influx = influxdb::Client::new(root, database).with_http_client(http.clone());
        Ok(Self { influx, http, ping })
    }

    /// Go: `Write(client.BatchPoints{Points: points, Database: database})`, through the crate:
    /// one POST of the points' lines with the `db` and `precision` query parameters
    /// (`influx.write-request`), a success on any 2xx status whose body names no error
    /// (`influx.write-status`).
    ///
    /// # Errors
    ///
    /// The crate's error, when the request fails or the server answers with another status or
    /// an error (`influx.error-texts`).
    pub async fn write(&self, points: Vec<WriteQuery>) -> Result<(), influxdb::Error> {
        // Go: influxdb.go:294 there
        self.influx.query(points).await.map(drop)
    }

    /// Go: `Ping`: `GET <path>/ping`, whatever the status; the `X-Influxdb-Version` header the
    /// server answers with, empty without one (the reporter drops it, as Go's does).
    ///
    /// # Errors
    ///
    /// When the request fails.
    pub async fn ping(&self) -> Result<String, reqwest::Error> {
        // Go: influxdb.go:417 there
        let response = self.http.get(self.ping.clone()).send().await?;
        let version = response
            .headers()
            .get("X-Influxdb-Version")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        Ok(version.to_owned())
    }
}

/// Go: `Request.SetBasicAuth(username, password)`.
fn basic_auth(username: &str, password: &str) -> io::Result<HeaderValue> {
    let credentials = BASE64_STANDARD.encode(format!("{username}:{password}"));
    let mut value =
        HeaderValue::try_from(format!("Basic {credentials}")).map_err(io::Error::other)?;
    value.set_sensitive(true);
    Ok(value)
}

/// `url` without its user info.
fn without_user_info(url: &Url) -> Url {
    let mut stripped = url.clone();
    match stripped
        .set_username("")
        .and_then(|()| stripped.set_password(None))
    {
        Ok(()) => stripped,
        // A URL that cannot have a host has no user info.
        Err(()) => url.clone(),
    }
}

/// Go: `u.Path = path.Join(u.Path, name)`: `name` under the URL's path, which the url crate
/// normalised when it read it, without the path's trailing slashes.
fn endpoint(base: &Url, name: &str) -> Url {
    let mut url = base.clone();
    let path = format!("{}/{name}", base.path().trim_end_matches('/'));
    url.set_path(&path);
    url
}
