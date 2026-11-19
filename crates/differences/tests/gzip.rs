//! Pins of `flate2` against Go's `compress/gzip` as the exchange compresses a bid request for a
//! bidder with `endpointCompression: gzip` (`testdata/golden/gzip.json`).

#[cfg(test)]
mod tests {
    use std::io::{self, Write as _};

    use flate2::{Compression, read::GzDecoder, write::GzEncoder};
    use pbs_differences::{TestResult, check_table, golden, hex, text, unhex};

    /// The text `bytes` inflate to.
    fn inflate(bytes: &[u8]) -> Result<String, io::Error> {
        io::read_to_string(GzDecoder::new(bytes))
    }

    /// Both write the same 10-byte gzip header (no name, no modification time, OS unknown) and
    /// both streams inflate to the same request, but the compressed bytes differ: `flate2`'s
    /// deflate (`miniz_oxide`) chooses other blocks than Go's `compress/flate`, whose `Close`
    /// also writes an empty stored block. A bidder sees other bytes and another
    /// `Content-Length`.
    // Go: exchange/bidder.go:848-874 (getRequestBody: w.Reset, Write, Close) and :876-880
    // (gzipWriterPool of gzip.NewWriter at the default level)
    #[test]
    fn go_difference_header_same_payload_differs() -> TestResult {
        let doc = golden("gzip.json")?;
        let payload = text(&doc, "/payload")?;
        let go = unhex(text(&doc, "/compressed_hex")?)?;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload.as_bytes())?;
        let rust = encoder.finish()?;
        let header = |bytes: &[u8]| hex(bytes.get(..10).unwrap_or_default());
        check_table(
            &[
                vec!["header".to_owned(), header(&go), header(&rust)],
                vec![
                    "length".to_owned(),
                    go.len().to_string(),
                    rust.len().to_string(),
                ],
                vec![
                    "inflates to the payload".to_owned(),
                    (inflate(&go)? == payload).to_string(),
                    (inflate(&rust)? == payload).to_string(),
                ],
                vec!["bytes".to_owned(), hex(&go), hex(&rust)],
            ],
            &[
                &["header", "1f8b08000000000000ff", "1f8b08000000000000ff"],
                &["length", "129", "128"],
                &["inflates to the payload", "true", "true"],
                &[
                    "bytes",
                    "1f8b08000000000000ffeccd310a02311085e1bbbc3a9aa8d8cc55c422ab0f33905dc624a2b0e4\
                     eeb2207889b4ff5f7c2bf40e41e173778083ce06b9fce216a6b82c2c90156fc829048704399e43\
                     ef5787aa8ddbb2f82004a935abe2bdbda6ac35b1ecf989b365fa589ade32d1fbf08637bcbff70d\
                     0000ffff452e35b780030000",
                    "1f8b08000000000000ffedcd410ac2301085e1bbbc7534517133571117a90e66202d6312b11072\
                     f7b6147a8aac1efc6ff155c81b84c4dfd30506322ae851f7b885c14f132750c51f7473ce2080ae\
                     77d7dad3204be1ed52ff5917a114cd64adfe862839703af3ec478d6c7d2af28a8cd66af7bad7bd\
                     c35b00452e35b780030000",
                ],
            ],
            "property, compress/gzip, flate2",
        )
    }
}
