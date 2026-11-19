//! What the pins share: the Go goldens of `testdata/golden` (written by `testdata/go`), loopback
//! servers that see the bytes a client writes, a test PKI, and checks that return errors.

use std::{
    error::Error,
    fmt::{self, Debug},
    fs,
    future::Future,
    path::Path,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use aws_lc_rs::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use pbs_http_client::{
    Builder, Client, HttpClientSettings, ProxyEnv, RequestError, Response, Roots,
    reqwest::{Method, Request, StatusCode, Url, Version, header::HeaderMap},
};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer,
    KeyIdMethod, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PublicKeyData, SerialNumber,
    SignatureAlgorithm, SigningKey, date_time_ymd,
};
use rustls::{
    ServerConfig,
    crypto::aws_lc_rs::default_provider,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use serde_json::Value;
use tokio::{
    io::{
        AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _,
        BufReader, copy, sink,
    },
    net::TcpListener,
    runtime,
    task::JoinHandle,
    time::timeout,
};
use tokio_rustls::TlsAcceptor;

/// A failure.
pub type BoxError = Box<dyn Error + Send + Sync>;

/// What every pin returns.
pub type TestResult = Result<(), BoxError>;

/// The directory the Go program writes the goldens to.
const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden");

/// The generator every golden's header names.
const GENERATOR: &str = "crates/http-client/testdata/go";

/// Reads `testdata/golden/<name>` and, when it has a header, checks it: written by
/// [`GENERATOR`] on amd64 (D-ARCH).
///
/// # Errors
///
/// The file cannot be read or parsed, or its header names another generator or architecture.
pub fn golden(name: &str) -> Result<Value, BoxError> {
    let path = Path::new(GOLDEN_DIR).join(name);
    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: Value = serde_json::from_slice(&bytes)?;
    if doc.get("header").is_some() {
        check_eq(text(&doc, "/header/generator")?, GENERATOR, "the generator")?;
        check_eq(text(&doc, "/header/goarch")?, "amd64", "the architecture")?;
    }
    Ok(doc)
}

/// The value at `pointer`.
///
/// # Errors
///
/// There is none.
pub fn at<'v>(doc: &'v Value, pointer: &str) -> Result<&'v Value, BoxError> {
    doc.pointer(pointer)
        .ok_or_else(|| format!("the golden has no {pointer}").into())
}

/// The string at `pointer`.
///
/// # Errors
///
/// There is none, or it is not a string.
pub fn text<'v>(doc: &'v Value, pointer: &str) -> Result<&'v str, BoxError> {
    at(doc, pointer)?
        .as_str()
        .ok_or_else(|| format!("{pointer} is not a string").into())
}

/// The array at `pointer`; `null` reads as an empty one, as Go writes a nil slice.
///
/// # Errors
///
/// There is none, or it is not an array.
pub fn list<'v>(doc: &'v Value, pointer: &str) -> Result<&'v [Value], BoxError> {
    let value = at(doc, pointer)?;
    if value.is_null() {
        return Ok(&[]);
    }
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{pointer} is not an array").into())
}

/// The strings of the array at `pointer`.
///
/// # Errors
///
/// There is none, or it holds something else.
pub fn strings(doc: &Value, pointer: &str) -> Result<Vec<String>, BoxError> {
    list(doc, pointer)?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{pointer} holds a non-string").into())
        })
        .collect()
}

/// The integer at `pointer`.
///
/// # Errors
///
/// There is none, or it is not an integer.
pub fn int(doc: &Value, pointer: &str) -> Result<i64, BoxError> {
    at(doc, pointer)?
        .as_i64()
        .ok_or_else(|| format!("{pointer} is not an integer").into())
}

/// The boolean at `pointer`, false when the key is absent (Go's `omitempty`).
///
/// # Errors
///
/// It is not a boolean.
pub fn flag(doc: &Value, pointer: &str) -> Result<bool, BoxError> {
    doc.pointer(pointer).map_or(Ok(false), |value| {
        value
            .as_bool()
            .ok_or_else(|| format!("{pointer} is not a boolean").into())
    })
}

/// The bytes of a hex string.
///
/// # Errors
///
/// It is not hex.
pub fn unhex(hex: &str) -> Result<Vec<u8>, BoxError> {
    let digits: Vec<u8> = hex
        .chars()
        .map(|c| {
            c.to_digit(16)
                .and_then(|d| u8::try_from(d).ok())
                .ok_or_else(|| format!("{c:?} is not a hex digit"))
        })
        .collect::<Result<_, _>>()?;
    Ok(digits
        .chunks(2)
        .map(|pair| pair.iter().fold(0, |byte, digit| (byte << 4) | digit))
        .collect())
}

/// Fails with `message` unless `condition` holds.
///
/// # Errors
///
/// `condition` is false.
pub fn check(condition: bool, message: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

/// Fails, showing both values, unless they are equal.
///
/// # Errors
///
/// They differ.
pub fn check_eq<T: PartialEq + Debug + ?Sized>(actual: &T, expected: &T, what: &str) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{what}:\n  got  {actual:?}\n  want {expected:?}").into())
    }
}

/// Runs a pin on a runtime of its own.
///
/// # Errors
///
/// The runtime cannot start, or the pin fails.
pub fn run<T, F: Future<Output = Result<T, BoxError>>>(pin: F) -> Result<T, BoxError> {
    runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(pin)
}

/// `text` with each port of `ports` written as its placeholder, as the goldens write them.
#[must_use]
pub fn with_ports(text: &str, ports: &[(u16, &str)]) -> String {
    ports.iter().fold(text.to_owned(), |text, (port, name)| {
        text.replace(&format!(":{port}"), &format!(":{name}"))
    })
}

/// `text` with each placeholder of `ports` written as its port.
#[must_use]
pub fn ports_in(text: &str, ports: &[(u16, &str)]) -> String {
    ports.iter().fold(text.to_owned(), |text, (port, name)| {
        text.replace(&format!(":{name}"), &format!(":{port}"))
    })
}

/// Go's `textproto.CanonicalMIMEHeaderKey`: the first letter and every letter after a hyphen
/// upper case, the others lower case; a name holding a byte that is not a token byte stays as
/// it is.
#[must_use]
pub fn canonical_key(name: &str) -> String {
    let token = |b: u8| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b);
    if !name.bytes().all(token) {
        return name.to_owned();
    }
    let mut upper = true;
    name.chars()
        .map(|c| {
            let out = if upper {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            };
            upper = c == '-';
            out
        })
        .collect()
}

/// A client with `settings`, no proxies and the loopback guard, names resolving to 127.0.0.1 at
/// `default_port` when a URL names none.
///
/// # Errors
///
/// It cannot be built.
pub fn loopback_client(
    settings: HttpClientSettings,
    default_port: u16,
) -> Result<Client, BoxError> {
    Ok(Builder::new(settings, ProxyEnv::default())
        .loopback_only(default_port)
        .build()?)
}

/// A request without headers or body.
///
/// # Errors
///
/// `method` or `url` does not parse.
pub fn request(method: &str, url: &str) -> Result<Request, BoxError> {
    Ok(Request::new(
        Method::from_bytes(method.as_bytes())?,
        Url::parse(url)?,
    ))
}

/// A response read to its end: its head, and its body or the error reading it.
#[derive(Debug)]
pub struct Fetched {
    /// The status.
    pub status: StatusCode,
    /// The protocol.
    pub version: Version,
    /// The headers.
    pub headers: HeaderMap,
    /// The body, or why it could not be read.
    pub body: Result<Vec<u8>, RequestError>,
}

/// Reads `response` to its end.
pub async fn fetch(response: Response) -> Fetched {
    let status = response.status();
    let version = response.version();
    let headers = response.headers().clone();
    Fetched {
        status,
        version,
        headers,
        body: response.bytes().await.map(|body| body.to_vec()),
    }
}

/// Locks `mutex`, through a poisoning.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A byte stream a server reads requests from: TCP, or TLS over TCP.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// The connections a server accepted, how many are open, and the most that were open at once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// Accepted so far.
    pub accepted: usize,
    /// Open now.
    pub open: usize,
    /// The most open at once.
    pub max_open: usize,
}

/// A loopback TCP listener that serves every connection it accepts with a handler, each in a
/// task of its own (`testdata/go/wire.go`'s `rawServer`).
#[derive(Debug)]
pub struct RawServer {
    port: u16,
    counts: Arc<Mutex<Counts>>,
    accept: JoinHandle<()>,
    handlers: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl RawServer {
    /// Listens on 127.0.0.1 at a free port and serves plain TCP.
    ///
    /// # Errors
    ///
    /// It cannot listen.
    pub async fn start<F, Fut>(handle: F) -> Result<Self, BoxError>
    where
        F: Fn(RawConn) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self::serve(None, handle).await
    }

    /// Listens on 127.0.0.1 at a free port and serves TLS with `acceptor`; a connection whose
    /// handshake fails is closed without the handler.
    ///
    /// # Errors
    ///
    /// It cannot listen.
    pub async fn start_tls<F, Fut>(acceptor: TlsAcceptor, handle: F) -> Result<Self, BoxError>
    where
        F: Fn(RawConn) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self::serve(Some(acceptor), handle).await
    }

    async fn serve<F, Fut>(tls: Option<TlsAcceptor>, handle: F) -> Result<Self, BoxError>
    where
        F: Fn(RawConn) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let counts = Arc::new(Mutex::new(Counts::default()));
        let handlers = Arc::new(Mutex::new(Vec::new()));
        let accept = tokio::spawn({
            let counts = Arc::clone(&counts);
            let handlers = Arc::clone(&handlers);
            let handle = Arc::new(handle);
            async move {
                while let Ok((tcp, _)) = listener.accept().await {
                    {
                        let mut now = lock(&counts);
                        now.accepted += 1;
                        now.open += 1;
                        now.max_open = now.max_open.max(now.open);
                    }
                    let handle = Arc::clone(&handle);
                    let counts = Arc::clone(&counts);
                    let tls = tls.clone();
                    let task = tokio::spawn(async move {
                        let conn = match tls {
                            None => Some(RawConn::new(Box::new(tcp), None)),
                            Some(acceptor) => acceptor.accept(tcp).await.ok().map(|stream| {
                                let alpn = stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                                RawConn::new(Box::new(stream), alpn)
                            }),
                        };
                        if let Some(conn) = conn {
                            handle(conn).await;
                        }
                        lock(&counts).open -= 1;
                    });
                    lock(&handlers).push(task);
                }
            }
        });
        Ok(Self {
            port,
            counts,
            accept,
            handlers,
        })
    }

    /// The port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// The counts so far.
    #[must_use]
    pub fn counts(&self) -> Counts {
        *lock(&self.counts)
    }

    /// Stops listening, waits up to five seconds for each handler to return (aborting the ones
    /// that do not), and gives the final counts.
    pub async fn close(self) -> Counts {
        let Self {
            counts,
            accept,
            handlers,
            ..
        } = self;
        accept.abort();
        let _aborted = accept.await;
        let tasks: Vec<JoinHandle<()>> = lock(&handlers).drain(..).collect();
        for mut task in tasks {
            if timeout(Duration::from_secs(5), &mut task).await.is_err() {
                task.abort();
            }
        }
        *lock(&counts)
    }
}

/// One accepted connection.
pub struct RawConn {
    reader: BufReader<Box<dyn Stream>>,
    alpn: Option<Vec<u8>>,
}

impl Debug for RawConn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawConn")
            .field("alpn", &self.alpn)
            .finish_non_exhaustive()
    }
}

/// One request a server read: the request line and header lines as they came over the wire,
/// and the body its `Content-Length` gave.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawRequest {
    /// The request line and the header lines.
    pub head: Vec<String>,
    /// The body.
    pub body: Vec<u8>,
}

impl RawConn {
    fn new(stream: Box<dyn Stream>, alpn: Option<Vec<u8>>) -> Self {
        Self {
            reader: BufReader::new(stream),
            alpn,
        }
    }

    /// The protocol TLS negotiated (ALPN), if any.
    #[must_use]
    pub fn alpn(&self) -> Option<&[u8]> {
        self.alpn.as_deref()
    }

    /// Reads the next request; `None` once the client has closed the connection or sent
    /// something that is not a request head.
    pub async fn read_request(&mut self) -> Option<RawRequest> {
        let mut head = Vec::new();
        let mut length = 0_usize;
        loop {
            let mut raw = String::new();
            match self.reader.read_line(&mut raw).await {
                Ok(0) | Err(_) => return None,
                Ok(_) => {}
            }
            let line = raw.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().ok()?;
            }
            head.push(line.to_owned());
        }
        let mut body = vec![0; length];
        let _read = self.reader.read_exact(&mut body).await.ok()?;
        Some(RawRequest { head, body })
    }

    /// Writes `bytes`, ignoring a client that has gone away.
    pub async fn write(&mut self, bytes: &[u8]) {
        let stream = self.reader.get_mut();
        if stream.write_all(bytes).await.is_ok() {
            let _flushed = stream.flush().await;
        }
    }

    /// Reads until the client closes the connection.
    pub async fn drain(&mut self) {
        let _drained = copy(&mut self.reader, &mut sink()).await;
    }
}

/// An ECDSA P-256 key of aws-lc-rs that rcgen signs certificates with.
struct EcKey(EcdsaKeyPair);

impl PublicKeyData for EcKey {
    fn der_bytes(&self) -> &[u8] {
        self.0.public_key().as_ref()
    }

    fn algorithm(&self) -> &'static SignatureAlgorithm {
        &PKCS_ECDSA_P256_SHA256
    }
}

impl SigningKey for EcKey {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        match self.0.sign(&SystemRandom::new(), msg) {
            Ok(signature) => Ok(signature.as_ref().to_vec()),
            Err(_unspecified) => Err(rcgen::Error::RemoteKeyError),
        }
    }
}

/// A test CA and a leaf it issued for `localhost`, `bidder.test` and 127.0.0.1, valid from an
/// hour ago for a day (`testdata/go/pki.go`'s `newTestPKI`).
pub struct TestPki {
    ca: CertificateDer<'static>,
    leaf: CertificateDer<'static>,
    leaf_key: Vec<u8>,
}

impl Debug for TestPki {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestPki").finish_non_exhaustive()
    }
}

impl TestPki {
    /// Makes the CA and the leaf.
    ///
    /// # Errors
    ///
    /// A key cannot be made or a certificate signed.
    pub fn new() -> Result<Self, BoxError> {
        let now = date_time_ymd(1970, 1, 1) + SystemTime::now().duration_since(UNIX_EPOCH)?;
        let hour = Duration::from_secs(3600);
        let ca_key = EcKey(
            EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING)
                .map_err(|e| format!("CA key: {e}"))?,
        );
        let mut ca = CertificateParams::new(Vec::<String>::new())?;
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        ca.distinguished_name
            .push(DnType::CommonName, "pbs http-client test CA");
        ca.serial_number = Some(SerialNumber::from(1_u64));
        ca.key_identifier_method = KeyIdMethod::PreSpecified(vec![1; 20]);
        ca.not_before = now - hour;
        ca.not_after = now + 24 * hour;
        let ca_cert = ca.self_signed(&ca_key)?;

        let leaf_key = EcKey(
            EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING)
                .map_err(|e| format!("leaf key: {e}"))?,
        );
        let mut leaf = CertificateParams::new(vec![
            "localhost".to_owned(),
            "bidder.test".to_owned(),
            "127.0.0.1".to_owned(),
        ])?;
        leaf.distinguished_name
            .push(DnType::CommonName, "bidder.test");
        leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        leaf.serial_number = Some(SerialNumber::from(2_u64));
        leaf.key_identifier_method = KeyIdMethod::PreSpecified(vec![2; 20]);
        leaf.use_authority_key_identifier_extension = true;
        leaf.not_before = now - hour;
        leaf.not_after = now + 24 * hour;
        let leaf_cert = leaf.signed_by(&leaf_key, &Issuer::new(ca, &ca_key))?;
        let pkcs8 = leaf_key
            .0
            .to_pkcs8v1()
            .map_err(|e| format!("leaf key PKCS #8: {e}"))?;
        Ok(Self {
            ca: ca_cert.der().clone(),
            leaf: leaf_cert.der().clone(),
            leaf_key: pkcs8.as_ref().to_vec(),
        })
    }

    /// The CA certificate as a PEM file holds it.
    #[must_use]
    pub fn ca_pem(&self) -> String {
        pem("CERTIFICATE", &self.ca)
    }

    /// The CA as extra roots (`certificates_file`).
    #[must_use]
    pub fn roots(&self) -> Roots {
        Roots::from_pem(self.ca_pem().as_bytes())
    }

    /// A TLS acceptor serving the leaf and offering the ALPN protocols `alpn`.
    ///
    /// # Errors
    ///
    /// rustls refuses the certificate or the key.
    pub fn acceptor(&self, alpn: &[&[u8]]) -> Result<TlsAcceptor, BoxError> {
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.leaf_key.clone()));
        let mut config = ServerConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(vec![self.leaf.clone(), self.ca.clone()], key)?;
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

/// `der` as a PEM block of type `kind`, 64 characters a line, as Go's `pem.Encode` writes it.
#[must_use]
pub fn pem(kind: &str, der: &[u8]) -> String {
    let encoded = STANDARD.encode(der);
    let mut out = format!("-----BEGIN {kind}-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        out.push_str(&String::from_utf8_lossy(chunk));
        out.push('\n');
    }
    out.push_str("-----END ");
    out.push_str(kind);
    out.push_str("-----\n");
    out
}
