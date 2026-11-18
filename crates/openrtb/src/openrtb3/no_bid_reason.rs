//! The methods of Go `openrtb3.NoBidReason` (`openrtb3/no_bid_reason.go`).

use super::{NO_BID_UNKNOWN_ERROR, NoBidReason};

impl NoBidReason {
    /// `Ptr` returns pointer to own value.
    #[must_use]
    pub const fn ptr(self) -> Option<Self> {
        Some(self)
    }

    /// `Val` safely dereferences pointer, returning default value (`NoBidUnknownError`) for nil.
    #[must_use]
    pub const fn val(p: Option<Self>) -> Self {
        match p {
            Some(v) => v,
            None => NO_BID_UNKNOWN_ERROR,
        }
    }
}
