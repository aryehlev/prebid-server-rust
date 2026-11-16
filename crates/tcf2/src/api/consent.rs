//! Go: `api/consent.go`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::consentconstants::Purpose;

/// The `time.Time` that `Created()` and `LastUpdated()` return.
///
/// Consent strings store these as 36-bit counts of deciseconds since the Unix epoch, and go-gdpr
/// converts them with `time.Unix(deciseconds/10, (deciseconds%10)*1e8)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ConsentTime {
    deciseconds: u64,
}

impl ConsentTime {
    pub(crate) const fn from_deciseconds(deciseconds: u64) -> Self {
        Self { deciseconds }
    }

    /// The raw value stored in the consent string.
    #[must_use]
    pub const fn deciseconds(self) -> u64 {
        self.deciseconds
    }

    /// Go `Time.Unix()`: whole seconds since the Unix epoch.
    #[must_use]
    pub const fn unix(self) -> u64 {
        self.deciseconds / 10
    }

    /// Go `Time.Nanosecond()`: the nanosecond offset within the second.
    #[must_use]
    pub const fn nanosecond(self) -> u32 {
        // At most 9 * 100_000_000, which fits in a u32.
        let [_, _, _, _, a, b, c, d] = ((self.deciseconds % 10) * 100_000_000).to_be_bytes();
        u32::from_be_bytes([a, b, c, d])
    }

    /// The same instant as a [`SystemTime`], or `None` if the platform cannot represent it.
    #[must_use]
    pub fn to_system_time(self) -> Option<SystemTime> {
        UNIX_EPOCH.checked_add(Duration::new(self.unix(), self.nanosecond()))
    }
}

/// Go: `api.VendorConsents`, a parsed GDPR vendor consent string.
pub trait VendorConsents {
    /// The version of the consent string.
    fn version(&self) -> u8;

    /// The time that the consent string was first created.
    fn created(&self) -> ConsentTime;

    /// The time that the consent string was last updated.
    fn last_updated(&self) -> ConsentTime;

    /// The ID of the CMP used to update the consent string.
    fn cmp_id(&self) -> u16;

    /// The version of the CMP used to update the consent string.
    fn cmp_version(&self) -> u16;

    /// The number of the CMP screen where consent was given.
    fn consent_screen(&self) -> u8;

    /// The bytes of the Go string `ConsentLanguage()` returns: two letters encoded as 6-bit values
    /// plus 65. A value of 63 yields the byte `0x80`, so the Go string is not always valid UTF-8.
    fn consent_language(&self) -> [u8; 2];

    /// The version of the global vendor list needed to interpret this consent string.
    fn vendor_list_version(&self) -> u16;

    /// The TCF policy version needed to interpret this consent string (always 0 for TCF 1).
    fn tcf_policy_version(&self) -> u8;

    /// The highest vendor ID encoded in the consent section.
    fn max_vendor_id(&self) -> u16;

    /// Whether the user consented to the given purpose.
    fn purpose_allowed(&self, id: Purpose) -> bool;

    /// Whether the given vendor has consent.
    fn vendor_consent(&self, id: u16) -> bool;
}
