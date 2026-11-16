//! GPP section identifiers. Go: package `github.com/prebid/go-gpp/constants`.

use std::fmt;

/// A GPP section identifier. Go: `constants.SectionID` (a named `int`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SectionId(pub i64);

impl fmt::Display for SectionId {
    /// Formats like Go's `%d`/`%v` of the underlying `int`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// EU TCF v2. Go: `SectionTCFEU2`.
pub const SECTION_TCFEU2: SectionId = SectionId(2);
/// The GPP header. Go: `SectionGPP`.
pub const SECTION_GPP: SectionId = SectionId(3);
/// US Privacy (CCPA) v1. Go: `SectionUSPV1`.
pub const SECTION_USPV1: SectionId = SectionId(6);
/// US national. Go: `SectionUSPNAT`.
pub const SECTION_USPNAT: SectionId = SectionId(7);
/// California. Go: `SectionUSPCA`.
pub const SECTION_USPCA: SectionId = SectionId(8);
/// Virginia. Go: `SectionUSPVA`.
pub const SECTION_USPVA: SectionId = SectionId(9);
/// Colorado. Go: `SectionUSPCO`.
pub const SECTION_USPCO: SectionId = SectionId(10);
/// Utah. Go: `SectionUSPUT`.
pub const SECTION_USPUT: SectionId = SectionId(11);
/// Connecticut. Go: `SectionUSPCT`.
pub const SECTION_USPCT: SectionId = SectionId(12);

/// Section names keyed by ID. Go: `SectionNamesByID`.
pub const SECTION_NAMES_BY_ID: [(i64, &str); 9] = [
    (2, "tcfeu2"),
    (3, "gpp header"),
    (6, "uspv1"),
    (7, "uspnat"),
    (8, "uspca"),
    (9, "uspva"),
    (10, "uspco"),
    (11, "usput"),
    (12, "uspct"),
];

/// Looks up a section name like Go's `SectionNamesByID[id]`: an unknown ID yields `""`.
#[must_use]
pub fn section_name_by_id(id: i64) -> &'static str {
    SECTION_NAMES_BY_ID
        .iter()
        .find(|&&(known, _)| known == id)
        .map_or("", |&(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::{SECTION_USPCT, SectionId, section_name_by_id};

    #[test]
    fn names() {
        assert_eq!(section_name_by_id(7), "uspnat", "known");
        assert_eq!(section_name_by_id(3), "gpp header", "header");
        assert_eq!(section_name_by_id(5), "", "unknown");
    }

    #[test]
    fn display_is_the_integer() {
        assert_eq!(SECTION_USPCT.to_string(), "12", "constant");
        assert_eq!(SectionId(-4).to_string(), "-4", "negative");
    }
}
