compile_error!(
    "patches/webpki-root-certs stands in for webpki-root-certs, which only \
     rustls-platform-verifier's wasm32 branch uses: Prebid Server does not target wasm32."
);
