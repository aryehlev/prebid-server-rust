//! Go: `openrtb_ext/alternatebiddercodes.go`: checking a bid's alternate bidder code (the
//! types are generated).

use pbs_errortypes::GoError;

use crate::{
    equal_fold,
    generated::{ExtAdapterAlternateBidderCodes, ExtAlternateBidderCodes},
};

/// Go: `alternateBidderDisabledError`. Go quotes the names with `%q` (`strconv.Quote`), the
/// port as Rust quotes strings (`config.alternate-code-quoting`).
fn alternate_bidder_disabled_error(bidder: &str, alternate_bidder: &str) -> GoError {
    GoError::generic(format!(
        "alternateBidderCodes disabled for {bidder:?}, rejecting bids for {alternate_bidder:?}"
    ))
}

/// Go: `alternateBidderNotDefinedError`, the names quoted as in
/// [`alternate_bidder_disabled_error`].
fn alternate_bidder_not_defined_error(bidder: &str, alternate_bidder: &str) -> GoError {
    GoError::generic(format!(
        "alternateBidderCodes not defined for adapter {bidder:?}, rejecting bids for \
         {alternate_bidder:?}"
    ))
}

impl ExtAlternateBidderCodes {
    /// Go: `(*ExtAlternateBidderCodes).IsValidBidderCode(bidder, alternateBidder)`: whether
    /// `bidder` may bid as `alternateBidder`. An empty alternate code or one equal to the
    /// bidder's name under `strings.EqualFold` is always valid; otherwise the codes and the
    /// bidder's entry must be enabled, and its allowed codes must be absent, exactly `["*"]`, or
    /// contain the code.
    ///
    /// # Errors
    ///
    /// Go's reason the code is not valid (Go's `false` result).
    pub fn is_valid_bidder_code(
        &self,
        bidder: &str,
        alternate_bidder: &str,
    ) -> Result<(), GoError> {
        // Go: openrtb_ext/alternatebiddercodes.go:19
        if alternate_bidder.is_empty() || equal_fold(bidder, alternate_bidder) {
            return Ok(());
        }
        if !self.enabled {
            return Err(alternate_bidder_disabled_error(bidder, alternate_bidder));
        }
        if self.bidders.is_none() {
            return Err(alternate_bidder_not_defined_error(bidder, alternate_bidder));
        }
        let Some(adapter_cfg) = self.is_bidder_in_alternate_bidder_codes(bidder) else {
            return Err(alternate_bidder_not_defined_error(bidder, alternate_bidder));
        };
        if !adapter_cfg.enabled {
            // config has bidder entry but is not enabled, report it
            return Err(alternate_bidder_disabled_error(bidder, alternate_bidder));
        }
        let Some(codes) = adapter_cfg.allowed_bidder_codes.as_deref() else {
            return Ok(());
        };
        if matches!(codes, [only] if only == "*")
            || codes.iter().any(|code| code == alternate_bidder)
        {
            return Ok(());
        }
        Err(GoError::generic(format!(
            "invalid biddercode {alternate_bidder:?} sent by adapter {bidder:?}"
        )))
    }

    /// Go: `(*ExtAlternateBidderCodes).IsBidderInAlternateBidderCodes(bidder)` tries to find
    /// bidder in the `bidders` map in a case sensitive manner first. If no match is found it'll
    /// try it in a case insensitive way (`strings.EqualFold`) in linear time. `None` is Go's
    /// `(ExtAdapterAlternateBidderCodes{}, false)`; Go's nil receiver is the caller's `None`.
    ///
    /// When several entries match case-insensitively, Go returns whichever its random map order
    /// visits first; here the first in byte order.
    #[must_use]
    pub fn is_bidder_in_alternate_bidder_codes(
        &self,
        bidder: &str,
    ) -> Option<&ExtAdapterAlternateBidderCodes> {
        // Go: openrtb_ext/alternatebiddercodes.go:65
        if bidder.is_empty() {
            return None;
        }
        let bidders = self.bidders.as_ref()?;
        if let Some(adapter_cfg) = bidders.get(bidder) {
            return Some(adapter_cfg);
        }
        bidders
            .iter()
            .find(|(bidder_name, _)| equal_fold(bidder, bidder_name))
            .map(|(_, adapter_cfg)| adapter_cfg)
    }
}

#[cfg(test)]
mod tests {
    use crate::generated::ExtAlternateBidderCodes;

    /// A test result whose error is the failure message.
    type TestResult = Result<(), String>;

    /// The names in the errors are quoted as Rust quotes strings, where Go's `%q` quotes them
    /// with `strconv.Quote` (`config.alternate-code-quoting`): go1.26.2 writes `"pub\amatic"`,
    /// `"a\x00b"`, `"x\x7f"`, `"\u00ad"`, `"a\u00a0b"` and `"\ua7cb"` (a letter Unicode assigned
    /// after its 15.0.0), and U+0301 after an `A`, U+0488 and U+3164 as they are, the port
    /// `"pub\u{7}matic"`, `"a\0b"`, `"x\u{7f}"`, `"\u{ad}"`, `"a\u{a0}b"`, U+A7CB as it is,
    /// `"A\u{301}"`, `"\u{488}"` and `"\u{3164}"`; printable characters, `"` and `\` are quoted
    /// alike.
    // Go: openrtb_ext/alternatebiddercodes.go:52, 56 and 60 (fmt.Errorf with %q)
    #[test]
    fn names_quoted_as_rust_quotes_them() -> TestResult {
        let codes = ExtAlternateBidderCodes::default();
        for (bidder, quoted) in [
            ("pub\u{7}matic", r#""pub\u{7}matic""#),
            ("a\0b", r#""a\0b""#),
            ("x\u{7f}", r#""x\u{7f}""#),
            ("\u{ad}", r#""\u{ad}""#),
            ("a\u{a0}b", r#""a\u{a0}b""#),
            ("\u{a7cb}", "\"\u{a7cb}\""),
            ("A\u{301}", r#""A\u{301}""#),
            ("\u{488}", r#""\u{488}""#),
            ("\u{3164}", r#""\u{3164}""#),
            ("pub\"matic", r#""pub\"matic""#),
            ("group\u{e9}", "\"group\u{e9}\""),
        ] {
            let want = format!(
                "alternateBidderCodes disabled for {quoted}, rejecting bids for \"groupm\""
            );
            match codes.is_valid_bidder_code(bidder, "groupm") {
                Err(e) if e.message() == want => {}
                got => return Err(format!("{bidder:?}: got {got:?}, want {want:?}")),
            }
        }
        Ok(())
    }
}
