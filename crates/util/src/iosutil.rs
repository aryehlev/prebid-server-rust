//! Go `util/iosutil`: iOS version parsing and the version classes Prebid treats differently
//! (the App Tracking Transparency changes of iOS 14.0, 14.1 and 14.2).

use std::{error, fmt};

use pbs_gocompat::strconv;

/// Go: `iosutil.Version`, the version of an iOS device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Version {
    /// Go: `Major`.
    pub major: i64,
    /// Go: `Minor`.
    pub minor: i64,
}

/// Go: the errors of `iosutil.ParseVersion`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParseVersionError {
    /// Not two or three dot-separated parts.
    Format,
    /// The first part is not an integer.
    Major,
    /// The second part is not an integer.
    Minor,
}

impl fmt::Display for ParseVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Format => "expected either major.minor or major.minor.patch format",
            Self::Major => "major version is not an integer",
            Self::Minor => "minor version is not an integer",
        })
    }
}

impl error::Error for ParseVersionError {}

/// Go: `iosutil.ParseVersion`: the major and minor numbers of `major.minor` or
/// `major.minor.patch`; the patch is not checked. The numbers are parsed with
/// `strconv.Atoi`, so a sign is allowed (`+14.2`, `14.-1`).
///
/// # Errors
///
/// [`ParseVersionError`] when the text has another shape or a number is not an integer.
pub fn parse_version(v: &str) -> Result<Version, ParseVersionError> {
    let parts: Vec<&str> = v.split('.').collect();
    let [major, minor, ..] = parts.as_slice() else {
        return Err(ParseVersionError::Format);
    };
    if parts.len() > 3 {
        return Err(ParseVersionError::Format);
    }
    // Go replaces the strconv error with its own.
    let major = strconv::atoi(major).map_err(|_atoi_error| ParseVersionError::Major)?;
    let minor = strconv::atoi(minor).map_err(|_atoi_error| ParseVersionError::Minor)?;
    Ok(Version { major, minor })
}

impl Version {
    /// Go: `Version.Equal`.
    #[must_use]
    pub const fn equal(&self, major: i64, minor: i64) -> bool {
        self.major == major && self.minor == minor
    }

    /// Go: `Version.EqualOrGreater`: semantic version comparison of major and minor.
    #[must_use]
    pub const fn equal_or_greater(&self, major: i64, minor: i64) -> bool {
        if self.major == major {
            self.minor >= minor
        } else {
            self.major > major
        }
    }
}

/// Go: `iosutil.VersionClassification`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VersionClassification {
    /// Go: `VersionUnknown` (0).
    #[default]
    Unknown,
    /// Go: `Version140` (1).
    Version140,
    /// Go: `Version141` (2).
    Version141,
    /// Go: `Version142OrGreater` (3).
    Version142OrGreater,
}

impl VersionClassification {
    /// The Go integer value.
    #[must_use]
    pub const fn as_int(self) -> i64 {
        match self {
            Self::Unknown => 0,
            Self::Version140 => 1,
            Self::Version141 => 2,
            Self::Version142OrGreater => 3,
        }
    }
}

/// Go: `iosutil.DetectVersionClassification`.
#[must_use]
pub fn detect_version_classification(v: &str) -> VersionClassification {
    let Ok(version) = parse_version(v) else {
        return VersionClassification::Unknown;
    };
    if version.equal(14, 0) {
        VersionClassification::Version140
    } else if version.equal(14, 1) {
        VersionClassification::Version141
    } else if version.equal_or_greater(14, 2) {
        VersionClassification::Version142OrGreater
    } else {
        VersionClassification::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ParseVersionError, Version, VersionClassification, detect_version_classification,
        parse_version,
    };
    use crate::golden::{TestResult, check, check_eq, err_text, fields, read_golden, text};

    #[test]
    fn parse_version_go_cases() -> TestResult {
        // Go: TestParseVersion.
        let cases = [
            (
                "14.2",
                Ok(Version {
                    major: 14,
                    minor: 2,
                }),
            ),
            (
                "14.2.1",
                Ok(Version {
                    major: 14,
                    minor: 2,
                }),
            ),
            ("", Err(ParseVersionError::Format)),
            ("14", Err(ParseVersionError::Format)),
            ("14.2.1.3", Err(ParseVersionError::Format)),
            ("xxx.2", Err(ParseVersionError::Major)),
            ("14.xxx", Err(ParseVersionError::Minor)),
        ];
        for (given, want) in cases {
            check_eq(&parse_version(given), &want, given)?;
        }
        Ok(())
    }

    #[test]
    fn equal_and_equal_or_greater_go_cases() -> TestResult {
        // Go: TestEqualOrGreater and TestEqual against 14.2.
        let cases = [
            ((13, 1), false, false),
            ((13, 2), false, false),
            ((14, 1), false, false),
            ((14, 2), true, true),
            ((15, 3), true, false),
            ((15, 2), true, false),
            ((14, 3), true, false),
        ];
        for ((major, minor), greater, equal) in cases {
            let v = Version { major, minor };
            check_eq(
                &v.equal_or_greater(14, 2),
                &greater,
                &format!("{v:?} >= 14.2"),
            )?;
            check_eq(&v.equal(14, 2), &equal, &format!("{v:?} == 14.2"))?;
        }
        Ok(())
    }

    #[test]
    fn detect_version_classification_go_cases() -> TestResult {
        // Go: TestDetectVersionClassification.
        let cases = [
            ("13.0", VersionClassification::Unknown),
            ("14.0", VersionClassification::Version140),
            ("14.0.1", VersionClassification::Version140),
            ("14.1", VersionClassification::Version141),
            ("14.1.2", VersionClassification::Version141),
            ("14.2", VersionClassification::Version142OrGreater),
            ("14.2.3", VersionClassification::Version142OrGreater),
            ("14.3", VersionClassification::Version142OrGreater),
            ("14.3.2", VersionClassification::Version142OrGreater),
            ("15.0", VersionClassification::Version142OrGreater),
            ("15.0.1", VersionClassification::Version142OrGreater),
        ];
        for (given, want) in cases {
            check_eq(&detect_version_classification(given), &want, given)?;
        }
        Ok(())
    }

    #[test]
    fn iosutil_golden() -> TestResult {
        // Signs, leading zeros, overflow, white space and non-ASCII digits.
        let rows = read_golden("iosutil.txt")?;
        check(rows.len() > 30, "iosutil.txt is complete")?;
        for row in &rows {
            let [s, major, minor, err, class] = fields::<5>(row)?;
            let s = text(s)?;
            let got = parse_version(s);
            let v = got.unwrap_or_default();
            check_eq(v.major.to_string().as_bytes(), major, s)?;
            check_eq(v.minor.to_string().as_bytes(), minor, s)?;
            check_eq(err_text(&got).as_bytes(), err, s)?;
            check_eq(
                detect_version_classification(s)
                    .as_int()
                    .to_string()
                    .as_bytes(),
                class,
                s,
            )?;
        }
        Ok(())
    }
}
