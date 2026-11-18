//! Go's `util/jsonutil`: the JSON entry points Prebid Server code calls, and its helper types.
//!
//! `unmarshal`/`unmarshal_valid` decode with jsoniter and clean the error up with
//! `tryExtractErrorMessage`; `marshal` encodes with jsoniter and Prebid's
//! `RawMessageExtension`.

use core::{error::Error, fmt};

mod element;
#[doc(hidden)]
pub mod gjson;
mod merge;
mod types;

pub use element::{ElementError, FoundElement, drop_element, find_element};
pub use merge::{ext_merge_decode, merge_clone, merge_clone_raw};
pub use types::{IntString, StringInt, parse_into_string};

use crate::{
    encode::{Flavor, GoEncode, marshal_with},
    gostring::GoString,
    jsoniter::{self, JsoniterDecode},
};

/// `errortypes.FailedToUnmarshal`: the error `jsonutil.Unmarshal` returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedToUnmarshal {
    /// The cleaned-up message (`tryExtractErrorMessage`).
    pub message: GoString,
}

impl fmt::Display for FailedToUnmarshal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message.to_string_lossy())
    }
}

impl Error for FailedToUnmarshal {}

/// `errortypes.FailedToMarshal`: the error `jsonutil.Marshal` returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedToMarshal {
    /// jsoniter's error text.
    pub message: GoString,
}

impl fmt::Display for FailedToMarshal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message.to_string_lossy())
    }
}

impl Error for FailedToMarshal {}

/// `jsonutil.Unmarshal(data, v)` (jsoniter without raw-message validation; decoding is the
/// same as [`unmarshal_valid`]).
///
/// # Errors
///
/// [`FailedToUnmarshal`] with the cleaned-up jsoniter message, e.g.
/// `cannot unmarshal openrtb2.Bid.W: unexpected character: \xff`.
pub fn unmarshal<T: JsoniterDecode + ?Sized>(
    data: &[u8],
    v: &mut T,
) -> Result<(), FailedToUnmarshal> {
    // Go: util/jsonutil/jsonutil.go Unmarshal
    jsoniter::unmarshal(data, v).map_err(|err| FailedToUnmarshal {
        message: GoString::from_bytes(try_extract_error_message(&err.go_text())),
    })
}

/// `jsonutil.UnmarshalValid(data, v)`.
///
/// # Errors
///
/// [`FailedToUnmarshal`], as [`unmarshal`].
pub fn unmarshal_valid<T: JsoniterDecode + ?Sized>(
    data: &[u8],
    v: &mut T,
) -> Result<(), FailedToUnmarshal> {
    unmarshal(data, v)
}

/// `jsonutil.Marshal(v)`.
///
/// # Errors
///
/// [`FailedToMarshal`] with jsoniter's error text, e.g.
/// `openrtb2.Bid.Price: unsupported value: NaN`.
pub fn marshal<T: GoEncode + ?Sized>(v: &T) -> Result<Vec<u8>, FailedToMarshal> {
    // Go: util/jsonutil/jsonutil.go Marshal
    marshal_with(v, Flavor::Jsoniter).map_err(|err| FailedToMarshal {
        message: GoString::from_bytes(err.go_text()),
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// Go's `tryExtractErrorMessage`, byte for byte: strips jsoniter's position context and
/// operation prefixes, naming the innermost `pkg.Type.Field` when the message needs it.
///
/// (Where Go would panic on a malformed message, the message is returned unchanged.)
#[must_use]
pub fn try_extract_error_message(msg: &[u8]) -> Vec<u8> {
    // Go: util/jsonutil/jsonutil.go tryExtractErrorMessage
    let Some(msg_end) = rfind(msg, b", error found in #") else {
        return msg.to_vec();
    };
    let Some(mut msg_start) = find(msg, b": ") else {
        return msg.to_vec();
    };
    let mut operation_stack: Vec<&[u8]> = vec![msg.get(..msg_start).unwrap_or_default()];
    loop {
        let rest = msg.get(msg_start + 2..).unwrap_or_default();
        let Some(next) = find(rest, b": ") else {
            break;
        };
        if msg_start + next >= msg_end {
            break;
        }
        let candidate = rest.get(..next).unwrap_or_default();
        if candidate.contains(&b' ') {
            break;
        }
        operation_stack.push(candidate);
        msg_start += next + 2;
    }
    let Some(detail) = msg.get(msg_start + 2..msg_end) else {
        return msg.to_vec();
    };
    let tail = msg.get(msg_start + 2..).unwrap_or_default();
    if operation_stack.len() > 1 && !tail.starts_with(b"request.") {
        let mut out = b"cannot unmarshal ".to_vec();
        out.extend_from_slice(
            operation_stack
                .get(operation_stack.len() - 2)
                .copied()
                .unwrap_or_default(),
        );
        out.extend_from_slice(b": ");
        out.extend_from_slice(detail);
        return out;
    }
    detail.to_vec()
}

#[cfg(test)]
mod tests {
    use super::try_extract_error_message;

    #[test]
    fn try_extract_error_message_cases() -> Result<(), String> {
        // Go: util/jsonutil/jsonutil_test.go TestTryExtractErrorMessage
        let cases: [(&str, &str, &str); 8] = [
            (
                "level-1",
                concat!(
                    "readObjectStart: expect { or n, but found m, error found in #1 byte of ",
                    "...|malformed|..., bigger context ...|malformed|..",
                ),
                "expect { or n, but found m",
            ),
            (
                "level-2",
                concat!(
                    "openrtb_ext.ExtRequestPrebidCache.Bids: readObjectStart: expect { or n, ",
                    "but found t, error found in #10 byte of ...|:{\"bids\":true}}|..., ",
                    "bigger context ...|{\"cache\":{\"bids\":true}}|...",
                ),
                concat!(
                    "cannot unmarshal openrtb_ext.ExtRequestPrebidCache.Bids: ",
                    "expect { or n, but found t",
                ),
            ),
            (
                "level-3+",
                concat!(
                    "openrtb_ext.ExtRequestPrebid.Cache: openrtb_ext.ExtRequestPrebidCache.Bids: ",
                    "readObjectStart: expect { or n, but found t, error found in #10 byte of ",
                    "...|:{\"bids\":true}}|..., bigger context ",
                    "...|{\"cache\":{\"bids\":true}}|...",
                ),
                concat!(
                    "cannot unmarshal openrtb_ext.ExtRequestPrebidCache.Bids: ",
                    "expect { or n, but found t",
                ),
            ),
            (
                "error-msg",
                concat!(
                    "Skip: do not know how to skip: 109, error found in #10 byte of ",
                    "...|prebid\": malformed}|..., bigger context ...|{\"prebid\": malformed}|...",
                ),
                "do not know how to skip: 109",
            ),
            (
                "specific",
                concat!(
                    "openrtb_ext.ExtDevicePrebid.Interstitial: unmarshalerDecoder: ",
                    "request.device.ext.prebid.interstitial.minwidthperc must be a number ",
                    "between 0 and 100, error found in #10 byte of ...|         }\n        }|..., ",
                    "bigger context ...|: 120,\n            \"minheightperc\": 60\n          ",
                    "}\n        }|...",
                ),
                concat!(
                    "request.device.ext.prebid.interstitial.minwidthperc must be a number ",
                    "between 0 and 100",
                ),
            ),
            ("normal", "normal error message", "normal error message"),
            (
                "norma-false-start",
                "false: normal error message",
                "false: normal error message",
            ),
            (
                "norma-false-end",
                "normal error message, error found in #10 but doesn't follow format",
                "normal error message, error found in #10 but doesn't follow format",
            ),
        ];
        for (name, given, expected) in cases {
            let got = try_extract_error_message(given.as_bytes());
            if got != expected.as_bytes() {
                return Err(format!(
                    "{name}: got {:?}, want {expected:?}",
                    String::from_utf8_lossy(&got)
                ));
            }
        }
        Ok(())
    }
}
