//! Glue (Rule B (d)): Prebid-authored error texts. The `UnmarshalJSON` methods of the ext types
//! fail with Prebid's own messages (`request.device.ext.prebid.interstitial.minwidthperc must be
//! a number between 0 and 100`), which `jsonutil.Unmarshal` hands its caller bare wherever the
//! value sits in the document (`tryExtractErrorMessage` leaves jsoniter's context off a message
//! that starts with `request.`, util/jsonutil/jsonutil.go:209-220). `serde_json` appends the
//! position (` at line 1 column 55`) when the value sits inside the document, and
//! `pbs_jsonutil::unmarshal` passes that on; [`bare`] takes it off again, so that the decodes of
//! the request wrapper give Prebid's text as Go gives it (`json.prebid-message-context`, restored
//! here; pinned by `tests/parity/request_wrapper/mod.rs::prebid_messages_are_bare` against the
//! goldens of `testdata/go/request_wrapper`).

use pbs_errortypes::{ErrorKind, GoError};

/// The start of every message Prebid's `UnmarshalJSON` methods write.
const PREBID_PREFIX: &str = "request.";

/// `serde_json`'s position suffix: ` at line L column C`.
const POSITION: &str = " at line ";

/// A `FailedToUnmarshal` whose message is Prebid's own, without the position `serde_json` puts
/// after it; any other error as it is.
pub(super) fn bare(err: GoError) -> GoError {
    if !matches!(err.kind(), ErrorKind::FailedToUnmarshal) {
        return err;
    }
    let Some(text) = without_position(err.message()).map(str::to_owned) else {
        return err;
    };
    GoError::failed_to_unmarshal(text)
}

/// `message` without its trailing ` at line L column C`, when it is a Prebid message that has
/// one.
fn without_position(message: &str) -> Option<&str> {
    if !message.starts_with(PREBID_PREFIX) {
        return None;
    }
    let (text, position) = message.rsplit_once(POSITION)?;
    let (line, column) = position.split_once(" column ")?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(line) && digits(column)).then_some(text)
}

#[cfg(test)]
mod tests {
    use pbs_errortypes::GoError;

    use super::bare;

    /// A test result whose error is the failure message.
    type TestResult = Result<(), String>;

    /// Fails, showing both texts, unless they are equal.
    fn ensure_eq(got: &str, want: &str) -> TestResult {
        if got == want {
            Ok(())
        } else {
            Err(format!("got {got:?}, want {want:?}"))
        }
    }

    /// The position is taken off Prebid's messages only, and only when it is `serde_json`'s.
    #[test]
    fn positions_come_off_prebid_messages() -> TestResult {
        const WIDTH: &str = concat!(
            "request.device.ext.prebid.interstitial.minwidthperc ",
            "must be a number between 0 and 100"
        );
        let cases = [
            (format!("{WIDTH} at line 1 column 55"), WIDTH.to_owned()),
            (WIDTH.to_owned(), WIDTH.to_owned()),
            (
                "invalid type: integer `1`, expected a string at line 1 column 1".to_owned(),
                "invalid type: integer `1`, expected a string at line 1 column 1".to_owned(),
            ),
            (
                "request.x at line one column 2".to_owned(),
                "request.x at line one column 2".to_owned(),
            ),
            (
                "request.x at line 1 column ".to_owned(),
                "request.x at line 1 column ".to_owned(),
            ),
        ];
        for (given, want) in cases {
            ensure_eq(bare(GoError::failed_to_unmarshal(given)).message(), &want)?;
        }
        let generic = format!("{WIDTH} at line 1 column 55");
        ensure_eq(bare(GoError::generic(generic.clone())).message(), &generic)
    }
}
