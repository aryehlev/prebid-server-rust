//! Prebid's error wrappers against Go (`testdata/golden/decode.json`, `jsonutil_test.go`):
//! `FailedToUnmarshal` and `FailedToMarshal` of Go's types around the library's words, and
//! `tryExtractErrorMessage`.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pbs_jsonutil::{marshal, try_extract_error_message, unmarshal, unmarshal_valid};
    use serde::Deserialize;

    use super::support::{TestResult, ensure_eq, golden, list, text};

    /// Go's `idTarget`.
    #[derive(Debug, Deserialize)]
    struct IdTarget {
        #[serde(default, rename = "id")]
        _id: String,
        #[serde(default, rename = "n")]
        _n: i64,
    }

    /// `unmarshal` and `unmarshal_valid` fail with a `FailedToUnmarshal`, and `marshal` with a
    /// `FailedToMarshal`, as Go's wrappers do; the message inside is `serde_json`'s where Go's is
    /// jsoniter's, trimmed by `tryExtractErrorMessage` (`json.error-texts`). A NaN is written as
    /// `null` (`json.non-finite`), and `[]` reads as a struct (`json.struct-from-array`).
    // Go: util/jsonutil/jsonutil.go:136-170 (Unmarshal, UnmarshalValid, Marshal); gen/decode.go
    // errorTexts
    #[test]
    fn wrappers_exact_inner_text_differs() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = list(&doc, "/error_texts")?;
        let mut decodes = 0;
        for row in rows {
            let (name, input) = (text(row, "/name")?, text(row, "/input")?);
            let (go, go_type) = (text(row, "/err")?, text(row, "/err_type")?);
            if input == "[]" {
                continue;
            }
            let rust = match name {
                "jsonutil.Unmarshal" => unmarshal::<IdTarget>(input.as_bytes()).err(),
                "jsonutil.UnmarshalValid" => unmarshal_valid::<IdTarget>(input.as_bytes()).err(),
                "jsonutil.Marshal chan int" => marshal(&BTreeMap::from([(vec![1_u8], 1)])).err(),
                _ => {
                    ensure_eq(&marshal(&f64::NAN)?.as_slice(), &b"null".as_slice(), name)?;
                    continue;
                }
            };
            let rust = rust.ok_or_else(|| format!("{name} {input}: no error where Go failed"))?;
            ensure_eq(&rust.kind().go_type(), &go_type, name)?;
            ensure_eq(&(rust.message() == go), &false, &format!("{name} {input}"))?;
            decodes += usize::from(name != "jsonutil.Marshal chan int");
        }
        ensure_eq(&decodes, &12, "decodes that fail")
    }

    /// `TestTryExtractErrorMessage`: jsoniter's message cut at its context and prefixed with the
    /// innermost field, and any other message kept as it is.
    // Go: util/jsonutil/jsonutil.go:172-221 (tryExtractErrorMessage), jsonutil_test.go:193-247
    #[test]
    fn try_extract_error_message_cases() -> TestResult {
        let cases = [
            (
                "readObjectStart: expect { or n, but found m, error found in #1 byte of \
                ...|malformed|..., bigger context ...|malformed|..",
                "expect { or n, but found m",
            ),
            (
                "openrtb_ext.ExtRequestPrebidCache.Bids: readObjectStart: \
                expect { or n, but found t, error found in #10 byte of \
                ...|:{\"bids\":true}}|..., bigger context ...|{\"cache\":{\"bids\":true}}|...",
                "cannot unmarshal openrtb_ext.ExtRequestPrebidCache.Bids: \
                expect { or n, but found t",
            ),
            (
                "openrtb_ext.ExtRequestPrebid.Cache: \
                openrtb_ext.ExtRequestPrebidCache.Bids: readObjectStart: \
                expect { or n, but found t, error found in #10 byte of \
                ...|:{\"bids\":true}}|..., bigger context ...|{\"cache\":{\"bids\":true}}|...",
                "cannot unmarshal openrtb_ext.ExtRequestPrebidCache.Bids: \
                expect { or n, but found t",
            ),
            (
                "Skip: do not know how to skip: 109, error found in #10 byte of \
                ...|prebid\": malformed}|..., bigger context ...|{\"prebid\": malformed}|...",
                "do not know how to skip: 109",
            ),
            (
                "openrtb_ext.ExtDevicePrebid.Interstitial: unmarshalerDecoder: \
                request.device.ext.prebid.interstitial.minwidthperc must be a \
                number between 0 and 100, error found in #10 byte of \
                ...|         }\n        }|..., bigger context \
                ...|: 120,\n            \"minheightperc\": 60\n          }\n        }|...",
                "request.device.ext.prebid.interstitial.minwidthperc must be a \
                number between 0 and 100",
            ),
            ("normal error message", "normal error message"),
            ("false: normal error message", "false: normal error message"),
            (
                "normal error message, error found in #10 but doesn't follow format",
                "normal error message, error found in #10 but doesn't follow format",
            ),
        ];
        for (given, want) in cases {
            ensure_eq(&try_extract_error_message(given).as_str(), &want, given)?;
        }
        let doc = golden("decode.json")?;
        ensure_eq(
            &list(&doc, "/error_texts")?.is_empty(),
            &false,
            "the golden",
        )
    }
}
