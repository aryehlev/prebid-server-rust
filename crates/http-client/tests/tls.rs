//! Pins of the clients' TLS against Go's crypto/tls and crypto/x509: the verdict on each
//! adversarial chain of `testdata/golden/x509_adversarial.json` (`testdata/go/x509adv`) next to
//! rustls' webpki verifier, the one rustls-platform-verifier runs on Linux (tls.acceptance,
//! ledgered), and the certificates a `certificates_file` adds, as `CertPool.AppendCertsFromPEM`
//! takes them (`testdata/golden/certificates_file.json`, `testdata/go/certfile.go`; restored),
//! and the `ClientHello` the clients send next to the one router.go's Transports send
//! (`testdata/golden/client_hello.json`, `testdata/go/clienthello.go`; tls.client-hello,
//! ledgered).

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc, time::Duration};

    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use pbs_http_client::{Builder, HttpClientSettings, ProxyEnv, Roots};
    use rustls::{
        RootCertStore,
        client::{WebPkiServerVerifier, danger::ServerCertVerifier as _},
        crypto::aws_lc_rs::default_provider,
        pki_types::{CertificateDer, ServerName, UnixTime},
    };
    use serde_json::Value;
    use tokio::{io::AsyncReadExt as _, net::TcpListener};

    use crate::support::{
        BoxError, RawServer, TestPki, TestResult, at, check, check_eq, golden, int, list,
        loopback_client, request, run, strings, text, unhex,
    };

    /// The fields of a `ClientHello` the pin compares, as Go's `ClientHelloInfo` reads them: the
    /// ALPN protocols, the supported versions, the cipher suites, the supported groups and the
    /// signature schemes.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Hello {
        alpn: Vec<String>,
        versions: Vec<u16>,
        suites: Vec<u16>,
        groups: Vec<u16>,
        schemes: Vec<u16>,
    }

    /// How one client's `ClientHello` differs from Go's: both ALPN lists and both version lists,
    /// and of the suites, groups and signature schemes, those Go offers alone and those rustls
    /// offers alone.
    #[derive(Debug, PartialEq, Eq)]
    struct HelloDiff {
        client: String,
        alpn: (Vec<String>, Vec<String>),
        versions: (Vec<u16>, Vec<u16>),
        suites: (Vec<u16>, Vec<u16>),
        groups: (Vec<u16>, Vec<u16>),
        schemes: (Vec<u16>, Vec<u16>),
    }

    /// A reader over the bytes of a TLS message.
    struct Cursor<'b>(&'b [u8]);

    impl<'b> Cursor<'b> {
        /// The next `n` bytes.
        fn take(&mut self, n: usize) -> Result<&'b [u8], BoxError> {
            let head = self.0.get(..n).ok_or("a short ClientHello")?;
            self.0 = self.0.get(n..).unwrap_or_default();
            Ok(head)
        }

        /// The next byte.
        fn byte(&mut self) -> Result<u8, BoxError> {
            Ok(self.take(1)?.first().copied().unwrap_or_default())
        }

        /// The next two bytes, big-endian.
        fn u16(&mut self) -> Result<u16, BoxError> {
            Ok(u16::from_be_bytes(self.take(2)?.try_into()?))
        }

        /// The next vector, whose length takes `len` bytes.
        fn vec(&mut self, len: usize) -> Result<Self, BoxError> {
            let n = self
                .take(len)?
                .iter()
                .fold(0, |n, byte| n << 8 | usize::from(*byte));
            Ok(Cursor(self.take(n)?))
        }

        /// The rest as two-byte values.
        fn u16s(mut self) -> Result<Vec<u16>, BoxError> {
            let mut values = Vec::new();
            while !self.0.is_empty() {
                values.push(self.u16()?);
            }
            Ok(values)
        }
    }

    /// The fields of the `ClientHello` in `record`, a TLS handshake record.
    fn parse_hello(record: &[u8]) -> Result<Hello, BoxError> {
        let mut record = Cursor(record);
        check(record.byte()? == 22, "a handshake record")?;
        let _record_version = record.u16()?;
        let mut handshake = record.vec(2)?;
        check(handshake.byte()? == 1, "a ClientHello")?;
        let mut hello = handshake.vec(3)?;
        let _legacy_version = hello.u16()?;
        let _random = hello.take(32)?;
        let _session_id = hello.vec(1)?;
        let mut fields = Hello {
            suites: hello.vec(2)?.u16s()?,
            ..Hello::default()
        };
        let _compression = hello.vec(1)?;
        let mut extensions = hello.vec(2)?;
        while !extensions.0.is_empty() {
            let kind = extensions.u16()?;
            let mut data = extensions.vec(2)?;
            match kind {
                10 => fields.groups = data.vec(2)?.u16s()?,
                13 => fields.schemes = data.vec(2)?.u16s()?,
                16 => {
                    let mut names = data.vec(2)?;
                    while !names.0.is_empty() {
                        fields
                            .alpn
                            .push(String::from_utf8(names.vec(1)?.0.to_vec())?);
                    }
                }
                43 => fields.versions = data.vec(1)?.u16s()?,
                _ => {}
            }
        }
        Ok(fields)
    }

    /// The `ClientHello` the client built from `settings` sends to an https server, read off the
    /// wire by a server that then hangs up.
    async fn rust_hello(settings: HttpClientSettings) -> Result<Hello, BoxError> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let read = tokio::spawn(async move {
            let (mut conn, _peer) = listener.accept().await?;
            let mut header = [0; 5];
            let _header_len = conn.read_exact(&mut header).await?;
            let [_, _, _, high, low] = header;
            let mut body = vec![0; usize::from(u16::from_be_bytes([high, low]))];
            let _body_len = conn.read_exact(&mut body).await?;
            Ok::<_, BoxError>([header.as_slice(), &body].concat())
        });
        let client = loopback_client(settings, port)?;
        let url = format!("https://bidder.test:{port}/h");
        let failed = client
            .send(request("GET", &url)?, None, None)
            .await
            .is_err();
        check(failed, "the handshake failed")?;
        parse_hello(&read.await??)
    }

    /// The code points of the list at `pointer`.
    fn code_points(case: &Value, pointer: &str) -> Result<Vec<u16>, BoxError> {
        list(case, pointer)?
            .iter()
            .map(|value| Ok(u16::try_from(value.as_u64().ok_or("not a number")?)?))
            .collect()
    }

    /// The values of `of` that `besides` lacks, in order.
    fn alone(of: &[u16], besides: &[u16]) -> Vec<u16> {
        of.iter()
            .copied()
            .filter(|value| !besides.contains(value))
            .collect()
    }

    /// The DER bytes of a one-certificate PEM text.
    fn der(pem: &str) -> Result<CertificateDer<'static>, BoxError> {
        let base64: String = pem
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        Ok(CertificateDer::from(STANDARD.decode(base64)?))
    }

    /// The certificate named `name`.
    fn cert(doc: &Value, name: &str) -> Result<CertificateDer<'static>, BoxError> {
        der(text(doc, &format!("/certs/{name}"))?)
    }

    /// webpki's verdict on `case`: the chain's first certificate as the server's, the rest as
    /// the intermediates it sends, the case's roots as the only anchors, at the golden's time
    /// plus the case's nanos.
    fn webpki_verdict(doc: &Value, case: &Value) -> Result<Result<(), String>, BoxError> {
        let mut roots = RootCertStore::empty();
        for name in strings(case, "/roots")? {
            // A root rustls cannot read as a trust anchor anchors nothing.
            let _added = roots.add(cert(doc, &name)?).is_ok();
        }
        let chain: Vec<CertificateDer<'static>> = strings(case, "/chain")?
            .iter()
            .map(|name| cert(doc, name))
            .collect::<Result<_, _>>()?;
        let Some((leaf, intermediates)) = chain.split_first() else {
            return Err("an empty chain".into());
        };
        let verifier = match WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(default_provider()),
        )
        .build()
        {
            Ok(verifier) => verifier,
            Err(e) => return Ok(Err(e.to_string())),
        };
        let host = text(case, "/host")?;
        let Ok(name) = ServerName::try_from(host.to_owned()) else {
            return Ok(Err(format!("{host:?} is not a server name")));
        };
        let second = Duration::from_secs(u64::try_from(int(doc, "/now")?)?);
        let nanos = case.get("nanos").and_then(Value::as_i64).unwrap_or(0);
        let offset = Duration::from_nanos(nanos.unsigned_abs());
        let now = if nanos < 0 {
            second.checked_sub(offset).ok_or("a time before 1970")?
        } else {
            second + offset
        };
        Ok(verifier
            .verify_server_cert(
                leaf,
                intermediates,
                &name,
                &[],
                UnixTime::since_unix_epoch(now),
            )
            .map(|_verified| ())
            .map_err(|e| e.to_string()))
    }

    /// The kind of rustls' error `text`: its variant, or the webpki error it wraps.
    fn short(text: &str) -> String {
        let text = text
            .strip_prefix("invalid peer certificate: ")
            .unwrap_or(text);
        if let Some(inner) = text.strip_prefix("Other(OtherError(") {
            return inner.trim_end_matches(')').to_owned();
        }
        if text.starts_with("certificate not valid for name") {
            return "NotValidForName".to_owned();
        }
        if text.ends_with("is not a server name") {
            return "ServerName".to_owned();
        }
        text.chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect()
    }

    /// The chains Go verifies and webpki refuses, with webpki's error kind.
    fn expected_go_only() -> Vec<(String, String)> {
        // Go verifies these and webpki refuses them: URI, email and directory-name
        // constraints, critical policy extensions, an empty EKU, validity times in encodings
        // webpki does not read, 1024-bit RSA, a CA certificate as the leaf, host forms, paths
        // deeper than webpki's limit and RSA-PSS parameters in other encodings.
        [
            ("nc_uri_perm", "NameConstraintViolation"),
            ("nc_email_perm", "NameConstraintViolation"),
            ("nc_dirname", "NameConstraintViolation"),
            ("pol_crit", "UnsupportedCriticalExtension"),
            ("eku_empty", "EmptyEkuExtension"),
            ("val_offset", "BadEncoding"),
            ("val_inter_offset", "BadEncoding"),
            ("val_minutes", "BadEncoding"),
            ("val_pre1970", "BadEncoding"),
            ("val_inter_pre1970", "BadEncoding"),
            ("sig_rsa1024", "BadSignature"),
            ("bc_pl300", "BadEncoding"),
            ("leaf_ca", "CaUsedAsEndEntity"),
            ("crit_inhibit", "UnsupportedCriticalExtension"),
            ("host_trailing", "NotValidForName"),
            ("host_star", "ServerName"),
            ("host_v6_brackets", "ServerName"),
            ("host_mapped", "NotValidForName"),
            ("host_mapped_host", "NotValidForName"),
            ("deep7", "MaximumPathDepthExceeded"),
            ("pinned_with_chain", "UnknownIssuer"),
            ("hs_policies", "UnsupportedCriticalExtension"),
            ("pss_wrapper_long", "UnsupportedSignatureAlgorithmContext"),
            ("pss_wrapper_short", "UnsupportedSignatureAlgorithmContext"),
            ("pss_wrapper_spill", "UnsupportedSignatureAlgorithmContext"),
            ("pss_trailer_one", "UnsupportedSignatureAlgorithmContext"),
            ("pss_trailer_nulls", "UnsupportedSignatureAlgorithmContext"),
            ("pss_trailer_octets", "UnsupportedSignatureAlgorithmContext"),
            (
                "pss_trailer_primitive",
                "UnsupportedSignatureAlgorithmContext",
            ),
            (
                "pss_trailer_high_tag",
                "UnsupportedSignatureAlgorithmContext",
            ),
            ("pss_trailer_extra", "UnsupportedSignatureAlgorithmContext"),
            ("pss_hash_extra", "UnsupportedSignatureAlgorithmContext"),
        ]
        .into_iter()
        .map(|(name, kind)| (name.to_owned(), kind.to_owned()))
        .collect()
    }

    /// The chains webpki verifies and Go refuses.
    fn expected_webpki_only() -> Vec<String> {
        // webpki verifies these and Go refuses them: policy constraints and mappings Go
        // enforces, constraint and key encodings Go rejects even in certificates it does not
        // use, leaf keys Go cannot read, validity bounds within the second webpki's time
        // truncates, and the paths Go builds through other cross-signed certificates.
        [
            "nc_space_constraint",
            "nc_ip_badmask",
            "nc_ip_badlen",
            "nc_email_bad",
            "nc_uri_ip",
            "pol_req",
            "pol_req_pol",
            "pol_req_any_none",
            "pol_map_any",
            "pol_leaf_req",
            "pol_inhibit",
            "pol_map_bad",
            "eku_root_code",
            "val_boundary_half",
            "val_inter_boundary_half",
            "val_root_boundary_half",
            "root_ku_nosign",
            "crit_root",
            "unused_rsa_zero",
            "unused_rsa_noparams",
            "unused_rsa_big",
            "unused_ec_offcurve",
            "unused_ec_curve",
            "unused_ec_compressed",
            "unused_ed_params",
            "unused_ed_size",
            "unused_dsa_bad",
            "unused_uri_bad",
            "unused_uri_baddomain",
            "unused_nc_badmask",
            "leaf_x25519",
            "leaf_unknown_key",
            "leaf_rsa_big",
            "pool_baduri",
            "cross_old_only",
            "twin_expired_first",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    // Go: crypto/tls/handshake_client.go verifyServerCertificate and crypto/x509/verify.go
    // Certificate.Verify.
    // Go: crypto/tls handshake_client.go Config.makeClientHello with the defaults of defaults.go
    // (defaultCipherSuites, defaultCurvePreferences, supportedSignatureAlgorithms) under the go
    // 1.25 module's GODEBUG, and net/http/transport.go Transport.protocols (no h2, so no ALPN).
    #[test]
    fn client_hello_differs() -> TestResult {
        run(async {
            let doc = golden("client_hello.json")?;
            let mut seen = Vec::new();
            for case in list(&doc, "/cases")? {
                let client = text(case, "/client")?;
                let settings = match client {
                    "general" => HttpClientSettings::GENERAL,
                    "cache" => HttpClientSettings::CACHE,
                    other => return Err(format!("unknown client {other}").into()),
                };
                let go = Hello {
                    alpn: strings(case, "/alpn")?,
                    versions: code_points(case, "/versions")?,
                    suites: code_points(case, "/cipher_suites")?,
                    groups: code_points(case, "/groups")?,
                    schemes: code_points(case, "/signature_schemes")?,
                };
                let rust = rust_hello(settings).await?;
                seen.push(HelloDiff {
                    client: client.to_owned(),
                    suites: (
                        alone(&go.suites, &rust.suites),
                        alone(&rust.suites, &go.suites),
                    ),
                    groups: (
                        alone(&go.groups, &rust.groups),
                        alone(&rust.groups, &go.groups),
                    ),
                    schemes: (
                        alone(&go.schemes, &rust.schemes),
                        alone(&rust.schemes, &go.schemes),
                    ),
                    alpn: (go.alpn, rust.alpn),
                    versions: (go.versions, rust.versions),
                });
            }
            let expected = |client: &str| HelloDiff {
                client: client.to_owned(),
                alpn: (Vec::new(), vec!["http/1.1".to_owned()]),
                versions: (vec![0x0304, 0x0303], vec![0x0304, 0x0303]),
                // ECDHE with AES-CBC-SHA on TLS 1.2; the renegotiation SCSV.
                suites: (vec![0xC009, 0xC013, 0xC00A, 0xC014], vec![0x00FF]),
                // P-521.
                groups: (vec![0x0019], Vec::new()),
                // ML-DSA-44, ML-DSA-65 and ML-DSA-87.
                schemes: (Vec::new(), vec![0x0904, 0x0905, 0x0906]),
            };
            check_eq(
                &seen,
                &vec![expected("general"), expected("cache")],
                "how the ClientHellos differ (Go's, the clients')",
            )
        })
    }

    #[test]
    fn x509_goldens() -> TestResult {
        let doc = golden("x509_adversarial.json")?;
        let mut go_only = Vec::new();
        let mut webpki_only = Vec::new();
        let mut cases = 0;
        for case in list(&doc, "/cases")? {
            cases += 1;
            let name = text(case, "/name")?.to_owned();
            let go = text(case, "/error")?.is_empty();
            match (go, webpki_verdict(&doc, case)?) {
                (true, Err(e)) => go_only.push((name, short(&e))),
                (false, Ok(())) => webpki_only.push(name),
                (true, Ok(())) | (false, Err(_)) => {}
            }
        }
        check_eq(&cases, &192, "the cases")?;
        check_eq(
            &(go_only, webpki_only),
            &(expected_go_only(), expected_webpki_only()),
            "the chains only Go verifies, and those only webpki verifies",
        )
    }

    // Go: crypto/x509/cert_pool.go CertPool.AppendCertsFromPEM and encoding/pem/pem.go Decode.
    #[test]
    fn certificates_file_like_go() -> TestResult {
        let doc = golden("certificates_file.json")?;
        let mut names: BTreeMap<Vec<u8>, String> = BTreeMap::new();
        for name in ["cert-a", "cert-b"] {
            let der = unhex(text(&doc, &format!("/certs/{name}"))?)?;
            let _earlier = names.insert(der, name.to_owned());
        }
        for case in list(&doc, "/cases")? {
            let name = text(case, "/name")?;
            let roots = Roots::from_pem(text(case, "/pem")?.as_bytes());
            let accepted: Vec<String> = roots
                .certificates()
                .iter()
                .map(|der| {
                    names
                        .get(der.as_ref())
                        .cloned()
                        .unwrap_or_else(|| "?".to_owned())
                })
                .collect();
            check_eq(
                &(accepted, !roots.is_empty()),
                &(
                    strings(case, "/accepted")?,
                    at(case, "/ok")?.as_bool().ok_or("ok is not a boolean")?,
                ),
                &format!("{name}: the certificates taken, and ok"),
            )?;
        }

        // A client trusts a server whose chain ends at a certificates_file root, and only then.
        run(async {
            let pki = TestPki::new()?;
            let server =
                RawServer::start_tls(pki.acceptor(&[b"http/1.1"])?, |mut conn| async move {
                    if conn.read_request().await.is_some() {
                        conn.write(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                            .await;
                    }
                })
                .await?;
            let url = format!("https://localhost:{}/t", server.port());
            let mut outcomes = Vec::new();
            for roots in [pki.roots(), Roots::none()] {
                let client = Builder::new(HttpClientSettings::GENERAL, ProxyEnv::default())
                    .roots(roots)
                    .loopback_only(0)
                    .build()?;
                outcomes.push(match client.send(request("GET", &url)?, None, None).await {
                    Ok(response) => response.status().as_u16().to_string(),
                    Err(e) => e.to_string().contains("certificate").to_string(),
                });
            }
            let _counts = server.close().await;
            check_eq(
                &outcomes,
                &vec!["204".to_owned(), "true".to_owned()],
                "with the file's root, and without it",
            )
        })
    }
}
