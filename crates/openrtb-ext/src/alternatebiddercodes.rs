//! Go: `openrtb_ext/alternatebiddercodes.go`: checking a bid's alternate bidder code (the
//! types are generated).

use pbs_errortypes::GoError;
use pbs_gocompat::{strconv::quote, strings};

use crate::generated::{ExtAdapterAlternateBidderCodes, ExtAlternateBidderCodes};

/// Go: `alternateBidderDisabledError`.
fn alternate_bidder_disabled_error(bidder: &str, alternate_bidder: &str) -> GoError {
    GoError::generic(format!(
        "alternateBidderCodes disabled for {}, rejecting bids for {}",
        quote(bidder),
        quote(alternate_bidder)
    ))
}

/// Go: `alternateBidderNotDefinedError`.
fn alternate_bidder_not_defined_error(bidder: &str, alternate_bidder: &str) -> GoError {
    GoError::generic(format!(
        "alternateBidderCodes not defined for adapter {}, rejecting bids for {}",
        quote(bidder),
        quote(alternate_bidder)
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
        if alternate_bidder.is_empty() || strings::equal_fold(bidder, alternate_bidder) {
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
            "invalid biddercode {} sent by adapter {}",
            quote(alternate_bidder),
            quote(bidder)
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
            .find(|(bidder_name, _)| strings::equal_fold(bidder, bidder_name))
            .map(|(_, adapter_cfg)| adapter_cfg)
    }
}
