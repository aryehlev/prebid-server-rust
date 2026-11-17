//! Go `crypto/tls` client handshakes and `crypto/x509` root pools, as the transport uses them.
//!
//! The configuration matches Go's client defaults for Prebid's transports: TLS 1.2 and 1.3, no
//! session resumption (Go's `ClientSessionCache` is nil), no ALPN (HTTP/2 is off), SNI for host
//! names but not for IP addresses, and the hybrid post-quantum key exchange preferred. Handshake
//! failures carry Go's texts for the common cases.
//!
//! # TLS stack
//!
//! rustls 0.23 with the aws-lc-rs provider performs the handshake. The server's certificate is
//! checked by [`crate::x509`]'s verifier: Go's `Certificate.Verify` order and error texts over
//! rustls-webpki's path building, against a [`CertPool`] loaded the way Go loads one. The
//! alternatives, weighed against Go's behaviour, the licence allow list and the workspace's one
//! version per crate:
//!
//! - ring has no post-quantum key exchange (Go 1.26, like aws-lc-rs, prefers `X25519MLKEM768`), and
//!   its newest release, 0.17.14, depends on windows-sys 0.52 where tokio, mio and socket2 use
//!   0.61. Only tokio 1.39 to 1.46 use 0.52 (see below), and the workspace needs tokio 1.54.
//! - [`CertPool::system`] reads `SSL_CERT_FILE`, `SSL_CERT_DIR` and Go's Linux file and directory
//!   lists as `x509.SystemCertPool` does. `rustls-native-certs` searches openssl-probe's lists,
//!   whose entries and order differ from Go's; `webpki-roots` is a bundled Mozilla list under
//!   CDLA-Permissive-2.0; `rustls-platform-verifier` brings jni-macros, which needs a second `syn`.
//! - native-tls and openssl build against OpenSSL and need `openssl-macros`, which also needs a
//!   second `syn`; native-tls on macOS stops at TLS 1.2.
//! - s2n-tls is a C library whose errors lack the certificate names and validity times that Go's
//!   texts quote.
//!
//! aws-lc-sys builds with the `cc` crate, so on Linux and macOS it needs a C compiler but not
//! `cmake`.
//!
//! Nothing builds ring, but `Cargo.lock` would still list it: rustls-webpki's `alloc` feature
//! names `ring?/alloc`, and Cargo's lock resolution keeps an optional dependency that a weak
//! feature names. `cargo tree` and `cargo deny` leave ring out, but `cargo metadata`, which
//! clippy's `multiple_crate_versions` reads, includes it, so that lint would count ring's
//! windows-sys 0.52 beside tokio's 0.61. `rust/Cargo.toml` therefore patches ring with
//! `rust/patches/ring`: the same version, with ring's features, no dependencies, and a `lib.rs`
//! that is only a `compile_error!`. The lock then holds one windows-sys, and a crate that enables
//! ring stops the build with an error that says what to do. The other way out, tokio 1.46, mio
//! 1.0.3 and socket2 0.5.10 (the last releases on windows-sys 0.52), is worse: tokio 1.46 is not
//! a long-term-support line and gets no fixes, its tokio-macros needs `syn` 2 beside the
//! workspace's `syn` 3 (the same lint again), and pbs-gohttp-server uses
//! `SocketAddr::as_abstract_name`, which tokio 1.48 added.
//!
//! The patch goes once rustls 0.24 and rustls-webpki 0.104 are released and adopted, with a
//! tokio-rustls built on them (as of 0.24.0-dev.3 and 0.104.0-alpha.8 both are pre-releases):
//! rustls 0.24 moves the crypto providers into crates of their own, and rustls-webpki 0.104 has
//! no ring feature, so nothing names ring any more.

use std::{
    env,
    ffi::OsStr,
    fmt, fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, OnceLock},
    task::{Context as TaskContext, Poll},
};

use rustls::{
    CertificateError, ClientConfig, Error as TlsError,
    client::Resumption,
    crypto::aws_lc_rs,
    pki_types::{CertificateDer, ServerName, pem::PemObject as _},
    version,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::TlsConnector;

use crate::{
    error::HttpError,
    httptrace::ConnectionState,
    net::{Conn, conn_io_error, errno_text},
    x509::{self, Rejection, Roots, ServerVerifier},
};

/// Go: `*x509.CertPool`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CertPool {
    certs: Vec<CertificateDer<'static>>,
}

impl fmt::Debug for CertPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CertPool")
            .field("len", &self.certs.len())
            .finish()
    }
}

/// Go: `x509.certFiles` (Linux): the first readable file is loaded.
const CERT_FILES: [&str; 6] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
    "/etc/pki/tls/cacert.pem",
    "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
    "/etc/ssl/cert.pem",
];

/// Go: `x509.certDirectories` (Linux): every file in each is loaded.
const CERT_DIRECTORIES: [&str; 2] = ["/etc/ssl/certs", "/etc/pki/tls/certs"];

/// Go's `*fs.PathError` text for a failed `op` (`open`, `read`) on `path`.
fn path_error(op: &str, path: &OsStr, e: &io::Error) -> String {
    format!("{op} {}: {}", Path::new(path).display(), errno_text(e))
}

/// Go: `os.ReadFile`: `None` when the error is "not found" (Go: `os.IsNotExist`), else Go's
/// `*fs.PathError` text, which names the step that failed: `open`, or `read` (a directory opens
/// but does not read).
fn read_file(path: &OsStr) -> Result<Vec<u8>, Option<String>> {
    let failed = |op: &str, e: &io::Error| {
        (e.kind() != ErrorKind::NotFound).then(|| path_error(op, path, e))
    };
    let mut file = fs::File::open(path).map_err(|e| failed("open", &e))?;
    let mut data = Vec::new();
    let _len = io::copy(&mut file, &mut data).map_err(|e| failed("read", &e))?;
    Ok(data)
}

impl CertPool {
    /// Go: `x509.NewCertPool()`.
    #[must_use]
    pub const fn new() -> Self {
        Self { certs: Vec::new() }
    }

    /// Go: `CertPool.AppendCertsFromPEM`: adds every parsable `CERTIFICATE` block (once) and
    /// reports whether any parsed.
    pub fn append_certs_from_pem(&mut self, pem: &[u8]) -> bool {
        let mut ok = false;
        for cert in CertificateDer::pem_slice_iter(pem).filter_map(Result::ok) {
            if !x509::parses(&cert) {
                continue;
            }
            ok = true;
            if !self.certs.contains(&cert) {
                self.certs.push(cert);
            }
        }
        ok
    }

    /// The number of certificates.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.certs.len()
    }

    /// Whether the pool is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.certs.is_empty()
    }

    /// Go: `x509.SystemCertPool()` with Linux's `loadSystemRoots`: `SSL_CERT_FILE` or the first
    /// readable file of Go's list, then every file of `SSL_CERT_DIR` (colon separated) or Go's
    /// directories, skipping links within a directory.
    ///
    /// # Errors
    ///
    /// The first open error other than "not found" (Go's `*fs.PathError` text) when no
    /// certificate was loaded.
    pub fn system() -> Result<Self, HttpError> {
        let mut roots = Self::new();
        let mut first_err: Option<String> = None;
        let env_file = env::var_os("SSL_CERT_FILE").filter(|f| !f.is_empty());
        let files: Vec<&OsStr> = env_file
            .as_deref()
            .map_or_else(|| CERT_FILES.iter().map(OsStr::new).collect(), |f| vec![f]);
        for file in files {
            match read_file(file) {
                Ok(data) => {
                    let _added = roots.append_certs_from_pem(&data);
                    break;
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = e;
                    }
                }
            }
        }
        let env_dir = env::var_os("SSL_CERT_DIR").filter(|d| !d.is_empty());
        // Go: strings.Split(d, ":"), which split_paths does on the variable's bytes on Unix.
        let dirs: Vec<PathBuf> = env_dir.map_or_else(
            || CERT_DIRECTORIES.iter().map(PathBuf::from).collect(),
            |d| env::split_paths(&d).collect(),
        );
        for dir in &dirs {
            let entries = match fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) => {
                    // Go opens directories with O_DIRECTORY, so a file fails to open too.
                    if first_err.is_none() && e.kind() != ErrorKind::NotFound {
                        first_err = Some(path_error("open", dir.as_os_str(), &e));
                    }
                    continue;
                }
            };
            let mut names: Vec<_> = entries.filter_map(Result::ok).collect();
            names.sort_by_key(fs::DirEntry::file_name);
            for entry in names {
                // Go: readUniqueDirectoryEntries skips links into the same directory.
                if let Ok(target) = fs::read_link(entry.path())
                    && !target.as_os_str().as_encoded_bytes().contains(&b'/')
                {
                    continue;
                }
                if let Ok(data) = read_file(entry.path().as_os_str()) {
                    let _added = roots.append_certs_from_pem(&data);
                }
            }
        }
        match first_err {
            Some(e) if roots.is_empty() => Err(HttpError::Message(e)),
            Some(_) | None => Ok(roots),
        }
    }

    /// The process's system pool, loaded once (Go: `systemRoots`).
    #[must_use]
    pub fn system_shared() -> &'static Self {
        static SYSTEM: OnceLock<CertPool> = OnceLock::new();
        SYSTEM.get_or_init(|| Self::system().unwrap_or_default())
    }
}

/// Go: `tls.Config`, the fields the transport uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsConfig {
    /// Go: `RootCAs` (`None`: the system pool).
    pub root_cas: Option<CertPool>,
    /// Go: `ServerName` (empty: the dialed host).
    pub server_name: String,
}

impl TlsConfig {
    /// Builds the rustls configuration and the verifier's root pool (once per transport).
    pub(crate) fn client(&self) -> Result<TlsClient, HttpError> {
        let pool = self
            .root_cas
            .as_ref()
            .unwrap_or_else(|| CertPool::system_shared());
        let provider = aws_lc_rs::default_provider();
        let roots = Arc::new(Roots::new(
            &pool.certs,
            provider.signature_verification_algorithms,
        ));
        let mut config = ClientConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&[&version::TLS13, &version::TLS12])
            .map_err(|e| HttpError::Message(format!("tls: {e}")))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(ServerVerifier::new(
                Arc::clone(&roots),
                None,
            )))
            .with_no_client_auth();
        config.resumption = Resumption::disabled();
        config.alpn_protocols = Vec::new();
        config.enable_sni = true;
        Ok(TlsClient {
            config: Arc::new(config),
            roots,
        })
    }
}

/// A transport's TLS client: the rustls configuration and the root pool its verifier checks
/// against.
#[derive(Debug, Clone)]
pub(crate) struct TlsClient {
    config: Arc<ClientConfig>,
    roots: Arc<Roots>,
}

/// A connection that watches the TLS records it reads: Go reports a first record that is not a
/// TLS handshake (`RecordHeaderError`) and tells `EOF` at a record boundary from
/// `unexpected EOF`.
#[derive(Debug)]
struct RecordingConn {
    inner: Box<dyn Conn>,
    /// The first five bytes read.
    first: Vec<u8>,
    /// The header bytes read of the current record.
    header: Vec<u8>,
    /// The body bytes of the current record not read yet.
    body_left: usize,
    /// The operation of the last I/O error (`read` or `write`).
    failed_op: Option<&'static str>,
}

impl RecordingConn {
    fn new(inner: Box<dyn Conn>) -> Self {
        Self {
            inner,
            first: Vec::with_capacity(5),
            header: Vec::with_capacity(5),
            body_left: 0,
            failed_op: None,
        }
    }

    /// Follows the record framing of bytes just read.
    fn track(&mut self, mut data: &[u8]) {
        if self.first.len() < 5 {
            let take = (5 - self.first.len()).min(data.len());
            self.first
                .extend_from_slice(data.get(..take).unwrap_or_default());
        }
        while !data.is_empty() {
            if self.body_left > 0 {
                let n = self.body_left.min(data.len());
                self.body_left -= n;
                data = data.get(n..).unwrap_or_default();
                continue;
            }
            let n = (5 - self.header.len()).min(data.len());
            let Some((head, rest)) = data.split_at_checked(n) else {
                break;
            };
            self.header.extend_from_slice(head);
            data = rest;
            if let [_, _, _, hi, lo] = *self.header.as_slice() {
                self.body_left = usize::from(u16::from_be_bytes([hi, lo]));
                self.header.clear();
            }
        }
    }

    /// Whether every byte read belongs to a complete record (Go: `rawInput.Len() == 0`).
    const fn at_record_boundary(&self) -> bool {
        self.header.is_empty() && self.body_left == 0
    }

    /// Go's first-record check: a record that is neither an alert nor a handshake, or a version
    /// of 16.0 or more, is not TLS (Go: `RecordHeaderError`).
    fn bad_first_record(&self) -> Option<[u8; 5]> {
        let header = <[u8; 5]>::try_from(self.first.as_slice()).ok()?;
        let [typ, major, minor, _, _] = header;
        let version = u16::from_be_bytes([major, minor]);
        (!matches!(typ, 21 | 22) || version >= 0x1000).then_some(header)
    }
}

impl AsyncRead for RecordingConn {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        match &res {
            Poll::Ready(Ok(())) => self.track(buf.filled().get(before..).unwrap_or_default()),
            Poll::Ready(Err(_)) => self.failed_op = Some("read"),
            Poll::Pending => {}
        }
        res
    }
}

impl AsyncWrite for RecordingConn {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Err(_)) = &res {
            self.failed_op = Some("write");
        }
        res
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Go: `alert.String()`.
fn alert_text(code: u8) -> String {
    let text = match code {
        0 => "close notify",
        10 => "unexpected message",
        20 => "bad record MAC",
        21 => "decryption failed",
        22 => "record overflow",
        30 => "decompression failure",
        40 => "handshake failure",
        42 => "bad certificate",
        43 => "unsupported certificate",
        44 => "revoked certificate",
        45 => "expired certificate",
        46 => "unknown certificate",
        47 => "illegal parameter",
        48 => "unknown certificate authority",
        49 => "access denied",
        50 => "error decoding message",
        51 => "error decrypting message",
        60 => "export restriction",
        70 => "protocol version not supported",
        71 => "insufficient security level",
        80 => "internal error",
        86 => "inappropriate fallback",
        90 => "user canceled",
        100 => "no renegotiation",
        109 => "missing extension",
        110 => "unsupported extension",
        111 => "certificate unobtainable",
        112 => "unrecognized name",
        113 => "bad certificate status response",
        114 => "bad certificate hash value",
        115 => "unknown PSK identity",
        116 => "certificate required",
        120 => "no application protocol",
        121 => "encrypted client hello required",
        _ => return format!("tls: alert({code})"),
    };
    format!("tls: {text}")
}

/// Go's error for a TLS protocol failure.
fn tls_error(e: &TlsError) -> HttpError {
    if let TlsError::InvalidCertificate(CertificateError::Other(other)) = e
        && let Some(rejection) = other.0.downcast_ref::<Rejection>()
    {
        return HttpError::Message(rejection.to_string());
    }
    if matches!(
        e,
        TlsError::InvalidCertificate(CertificateError::BadEncoding)
    ) {
        return HttpError::Static(
            "tls: failed to parse certificate from server: x509: malformed certificate",
        );
    }
    if let TlsError::InvalidCertificate(c) = e {
        return HttpError::Message(format!("tls: failed to verify certificate: x509: {c}"));
    }
    if let TlsError::AlertReceived(a) = e {
        let code = u8::from(*a);
        // Go: a close_notify is the end of the stream.
        if code == 0 {
            return HttpError::Eof;
        }
        return HttpError::Message(format!("remote error: {}", alert_text(code)));
    }
    if let TlsError::InappropriateMessage { .. } | TlsError::InappropriateHandshakeMessage { .. } =
        e
    {
        return HttpError::Message(format!("local error: {}", alert_text(10)));
    }
    if let TlsError::InvalidMessage(_) = e {
        return HttpError::Message(format!("local error: {}", alert_text(50)));
    }
    HttpError::Message(format!("tls: {e}"))
}

/// Maps a handshake failure to Go's error.
fn handshake_error(
    e: &io::Error,
    rec: &RecordingConn,
    local: Option<&String>,
    remote: Option<&String>,
) -> HttpError {
    if let Some(header) = rec.bad_first_record() {
        return HttpError::RecordHeader(header);
    }
    if let Some(tls) = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<TlsError>())
    {
        return tls_error(tls);
    }
    if e.kind() == ErrorKind::UnexpectedEof {
        // Go accepts a close at a record boundary as EOF.
        return if rec.at_record_boundary() {
            HttpError::Eof
        } else {
            HttpError::UnexpectedEof
        };
    }
    conn_io_error(rec.failed_op.unwrap_or("read"), local, remote, e)
}

/// Go: `tls.Client(conn, cfg).HandshakeContext(ctx)` for `server_name`.
///
/// # Errors
///
/// Go's handshake errors (`tls: failed to verify certificate: x509: certificate signed by
/// unknown authority`, `remote error: tls: handshake failure`, `EOF`, ...).
pub(crate) async fn handshake(
    client: &TlsClient,
    conn: Box<dyn Conn>,
    server_name: &str,
    local: Option<&String>,
    remote: Option<&String>,
) -> Result<(Box<dyn Conn>, ConnectionState), HttpError> {
    let host = server_name
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(server_name);
    let name = ServerName::try_from(host.to_owned())
        .map_err(|e| HttpError::Message(format!("tls: {e}")))?;
    // Go verifies against the name as given; rustls's ServerName drops a trailing dot.
    let mut config = ClientConfig::clone(&client.config);
    config
        .dangerous()
        .set_certificate_verifier(Arc::new(ServerVerifier::new(
            Arc::clone(&client.roots),
            Some(host.to_owned()),
        )));
    let connector = TlsConnector::from(Arc::new(config));
    match connector
        .connect(name, RecordingConn::new(conn))
        .into_fallible()
        .await
    {
        Ok(stream) => {
            let (_, session) = stream.get_ref();
            let state = ConnectionState {
                handshake_complete: true,
                server_name: host.to_owned(),
                version: session.protocol_version().map_or(0, u16::from),
                negotiated_protocol: String::new(),
            };
            Ok((Box::new(stream), state))
        }
        Err((e, rec)) => Err(handshake_error(&e, &rec, local, remote)),
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::empty;

    use super::*;
    use crate::testutil::ensure_eq;

    #[test]
    fn alert_texts() -> Result<(), String> {
        ensure_eq(
            alert_text(40).as_str(),
            "tls: handshake failure",
            "handshake failure",
        )?;
        ensure_eq(alert_text(41).as_str(), "tls: alert(41)", "unnamed alert")?;
        ensure_eq(
            alert_text(48).as_str(),
            "tls: unknown certificate authority",
            "unknown CA",
        )
    }

    #[test]
    fn record_tracking() -> Result<(), String> {
        let mut rec = RecordingConn::new(Box::new(empty()));
        rec.track(&[22, 3, 3, 0, 2, 1]);
        ensure_eq(&rec.at_record_boundary(), &false, "mid record")?;
        ensure_eq(&rec.bad_first_record(), &None, "handshake record")?;
        rec.track(&[2, 21, 3]);
        ensure_eq(&rec.at_record_boundary(), &false, "mid header")?;
        rec.track(&[3, 0, 0]);
        ensure_eq(&rec.at_record_boundary(), &true, "boundary")?;
        let mut http = RecordingConn::new(Box::new(empty()));
        http.track(b"HTTP/1.1 400 Bad Request\r\n");
        ensure_eq(
            &http.bad_first_record(),
            &Some(*b"HTTP/"),
            "plain HTTP reply",
        )
    }

    /// Go's `AppendCertsFromPEM` skips blocks `x509.ParseCertificate` rejects.
    #[test]
    fn pool_skips_unparsable() -> Result<(), String> {
        let mut pool = CertPool::new();
        let junk =
            b"-----BEGIN CERTIFICATE-----\nbm90IGEgY2VydGlmaWNhdGU=\n-----END CERTIFICATE-----\n";
        ensure_eq(&pool.append_certs_from_pem(junk), &false, "junk")?;
        ensure_eq(&pool.len(), &0, "pool size")
    }

    /// Go's `os.ReadFile` texts: a directory opens but does not read.
    #[test]
    fn read_file_texts() -> Result<(), String> {
        let dir = env!("CARGO_MANIFEST_DIR");
        ensure_eq(
            &read_file(OsStr::new(dir)),
            &Err(Some(format!("read {dir}: is a directory"))),
            "directory",
        )?;
        ensure_eq(
            &read_file(OsStr::new("/nonexistent/pbs-gohttp-client")),
            &Err(None),
            "missing file",
        )
    }
}
