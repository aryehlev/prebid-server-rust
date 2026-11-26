//! Glue (Rule B (a)): gojsonschema's `uuid` and `hostname` format checkers, the formats the
//! bidder schemas name (jsonschema.formats).
//!
//! Go: `gojsonschema/format_checkers.go`. Both checkers are regular expressions, compiled here
//! by `pbs_util::goregex` so that they keep Go's meaning (`[a-f0-9]` and `$` as in RE2). A name
//! gojsonschema has no checker for checks nothing, as in Go; its other 15 checkers are not
//! ported, and a schema that names one fails to load (jsonschema.unsupported-keywords).

use std::sync::LazyLock;

use pbs_util::goregex;
use regex::Regex;

/// Go: `rxHostname`.
static RX_HOSTNAME: LazyLock<Option<Regex>> = LazyLock::new(|| {
    goregex::compile(concat!(
        r"^([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]{0,61}[a-zA-Z0-9])",
        r"(\.([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]{0,61}[a-zA-Z0-9]))*$",
    ))
    .ok()
});

/// Go: `rxUUID`.
static RX_UUID: LazyLock<Option<Regex>> = LazyLock::new(|| {
    goregex::compile("^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$").ok()
});

/// The formats gojsonschema's `FormatCheckers` checks, but `hostname` and `uuid`.
const UNPORTED: [&str; 15] = [
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
    "regex",
    "json-pointer",
    "relative-json-pointer",
];

/// A `format` the port checks (Go: the checker `FormatCheckers` holds for the name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    /// `HostnameChecker`.
    Hostname,
    /// `UUIDFormatChecker`.
    Uuid,
}

impl Format {
    /// The checker for `name`, `None` for a name gojsonschema has no checker for (the empty
    /// name among them), which passes every value in Go.
    ///
    /// # Errors
    ///
    /// Refuses the other formats gojsonschema checks: their checkers (`time.Parse`,
    /// `net/mail`, `net/url`, ...) are not ported, and no schema Prebid ships names one.
    pub(super) fn parse(name: &str) -> Result<Option<Self>, String> {
        let (format, rx) = match name {
            "hostname" => (Self::Hostname, &RX_HOSTNAME),
            "uuid" => (Self::Uuid, &RX_UUID),
            other if UNPORTED.contains(&other) => {
                return Err(format!(
                    "format {other:?} is not supported by the Rust port of gojsonschema, which \
                     checks only uuid and hostname"
                ));
            }
            _ => return Ok(None),
        };
        if rx.is_none() {
            return Err(format!("the {name} checker's pattern does not compile"));
        }
        Ok(Some(format))
    }

    /// The format's name, as `Does not match format '{{.format}}'` prints it.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Hostname => "hostname",
            Self::Uuid => "uuid",
        }
    }

    /// Go: `FormatCheckers.IsFormat(format, value)` for a string value.
    pub(super) fn is_format(self, s: &str) -> bool {
        match self {
            Self::Hostname => matches(&RX_HOSTNAME, s) && s.len() < 256,
            Self::Uuid => matches(&RX_UUID, s),
        }
    }
}

fn matches(rx: &LazyLock<Option<Regex>>, s: &str) -> bool {
    rx.as_ref().is_some_and(|r| r.is_match(s))
}

#[cfg(test)]
mod tests {
    use super::Format;

    #[test]
    fn checkers_match_go() -> Result<(), String> {
        let uuid = Format::parse("uuid")?.ok_or("uuid")?;
        let host = Format::parse("hostname")?.ok_or("hostname")?;
        let cases = [
            (uuid, "123e4567-e89b-12d3-a456-426614174000", true),
            (uuid, "123E4567-E89B-12D3-A456-426614174000", false),
            (uuid, "123e4567e89b12d3a456426614174000", false),
            (uuid, "123e4567-e89b-12d3-a456-426614174000\n", false),
            (host, "example.com", true),
            (host, "-example.com", false),
            (host, "a..b", false),
            (host, "localhost", true),
        ];
        for (format, input, want) in cases {
            if format.is_format(input) != want {
                return Err(format!("{} on {input:?}: want {want}", format.name()));
            }
        }
        for name in ["email", "date-time", "ipv4", "relative-json-pointer"] {
            if Format::parse(name).is_ok() {
                return Err(format!("{name:?} must be refused"));
            }
        }
        // Go passes every value for a name it has no checker for.
        for name in ["custom", "UUID", "hostname ", "idn-hostname", ""] {
            if Format::parse(name) != Ok(None) {
                return Err(format!("{name:?} must check nothing"));
            }
        }
        Ok(())
    }
}
