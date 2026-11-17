//! Go: `gojsonschema/format_checkers.go` (`FormatCheckers`, the default checker chain).
//!
//! The checkers that are pure regular expressions are ported exactly. Checkers built on other
//! Go packages (`time.Parse`, `net/mail`, `net/url`, `net.ParseIP`) are not ported; a schema
//! that uses one of those formats is rejected when it is compiled, so it can never be validated
//! with different results than Go. No bidder schema uses them.

use std::sync::LazyLock;

use super::regexp::GoRegexp;

/// Go: `rxHostname`.
static RX_HOSTNAME: LazyLock<Option<GoRegexp>> = LazyLock::new(|| {
    GoRegexp::compile(concat!(
        r"^([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]{0,61}[a-zA-Z0-9])",
        r"(\.([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]{0,61}[a-zA-Z0-9]))*$",
    ))
    .ok()
});

/// Go: `rxUUID`.
static RX_UUID: LazyLock<Option<GoRegexp>> = LazyLock::new(|| {
    GoRegexp::compile("^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$").ok()
});

/// Go: `rxJSONPointer`.
static RX_JSON_POINTER: LazyLock<Option<GoRegexp>> =
    LazyLock::new(|| GoRegexp::compile("^(?:/(?:[^~/]|~0|~1)*)*$").ok());

/// Go: `rxRelJSONPointer`.
static RX_REL_JSON_POINTER: LazyLock<Option<GoRegexp>> =
    LazyLock::new(|| GoRegexp::compile("^(?:0|[1-9][0-9]*)(?:#|(?:/(?:[^~/]|~0|~1)*)*)$").ok());

/// Formats Go knows but whose checkers depend on Go packages that are not ported.
const UNPORTED_FORMATS: [&str; 12] = [
    "date",
    "time",
    "date-time",
    "email",
    "idn-email",
    "ipv4",
    "ipv6",
    "uri",
    "uri-reference",
    "iri",
    "iri-reference",
    "uri-template",
];

/// The ported checkers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Checker {
    Hostname,
    Uuid,
    Regex,
    JsonPointer,
    RelativeJsonPointer,
}

/// Go: `subSchema.format` together with the checker `FormatCheckers` selects for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) enum Format {
    /// No `format` keyword (Go: `""`, never checked).
    #[default]
    None,
    /// A format without a registered checker: Go's `IsFormat` returns true.
    Unchecked(String),
    /// A format with a ported checker.
    Checked(String, Checker),
}

impl Format {
    /// Selects the checker for `name`.
    ///
    /// # Errors
    ///
    /// Rejects the formats whose Go checkers are not ported.
    pub(super) fn parse(name: &str) -> Result<Self, String> {
        let checker = match name {
            "" => return Ok(Self::None),
            "hostname" => Checker::Hostname,
            "uuid" => Checker::Uuid,
            "regex" => Checker::Regex,
            "json-pointer" => Checker::JsonPointer,
            "relative-json-pointer" => Checker::RelativeJsonPointer,
            other if UNPORTED_FORMATS.contains(&other) => {
                return Err(format!(
                    "format {other:?} is not supported by the Rust port of gojsonschema"
                ));
            }
            other => return Ok(Self::Unchecked(other.to_owned())),
        };
        Ok(Self::Checked(name.to_owned(), checker))
    }

    /// Whether the keyword is present (Go: `format != ""`).
    pub(super) const fn is_set(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// The format name, for `Does not match format '{{.format}}'`.
    pub(super) fn name(&self) -> &str {
        match self {
            Self::None => "",
            Self::Unchecked(name) | Self::Checked(name, _) => name,
        }
    }

    /// Go: `FormatCheckers.IsFormat(format, stringValue)`.
    pub(super) fn is_format_string(&self, s: &str) -> bool {
        let rx = match self {
            Self::None | Self::Unchecked(_) => return true,
            Self::Checked(_, Checker::Hostname) => {
                return matches(&RX_HOSTNAME, s) && s.len() < 256;
            }
            Self::Checked(_, Checker::Regex) => {
                return s.is_empty() || GoRegexp::compile(s).is_ok();
            }
            Self::Checked(_, Checker::Uuid) => &RX_UUID,
            Self::Checked(_, Checker::JsonPointer) => &RX_JSON_POINTER,
            Self::Checked(_, Checker::RelativeJsonPointer) => &RX_REL_JSON_POINTER,
        };
        matches(rx, s)
    }

    /// Go: `FormatCheckers.IsFormat(format, *big.Rat)`: every registered checker type-asserts a
    /// `string` and fails, unregistered formats pass.
    pub(super) const fn is_format_number(&self) -> bool {
        matches!(self, Self::None | Self::Unchecked(_))
    }
}

fn matches(rx: &LazyLock<Option<GoRegexp>>, s: &str) -> bool {
    rx.as_ref().is_some_and(|r| r.match_string(s))
}

#[cfg(test)]
mod tests {
    use super::Format;

    #[test]
    fn checkers_match_go() -> Result<(), String> {
        let uuid = Format::parse("uuid")?;
        let host = Format::parse("hostname")?;
        let cases = [
            (&uuid, "123e4567-e89b-12d3-a456-426614174000", true),
            (&uuid, "123E4567-E89B-12D3-A456-426614174000", false),
            (&uuid, "123e4567e89b12d3a456426614174000", false),
            (&host, "example.com", true),
            (&host, "-example.com", false),
            (&host, "a..b", false),
            (&host, "localhost", true),
        ];
        for (format, input, want) in cases {
            if format.is_format_string(input) != want {
                return Err(format!("{} on {input:?}: want {want}", format.name()));
            }
        }
        if Format::parse("email").is_ok() {
            return Err("email must be rejected".to_owned());
        }
        if !Format::parse("custom")?.is_format_string("anything") {
            return Err("unknown formats always pass".to_owned());
        }
        Ok(())
    }
}
