//! ISO 4217 currency codes exactly as `golang.org/x/text/currency` parses them.
//!
//! Go Prebid Server validates every currency code with `currency.ParseISO` from
//! `golang.org/x/text` (v0.37.0 in go.mod), whose table comes from CLDR 32. That table keeps
//! historic codes (`ADP`, `DEM`, ...) and lacks codes introduced after CLDR 32 (`VES`, `MRU`,
//! `SLE`, `ZWG`, ...), so those are "not a recognized currency" in Go and here.

use std::{error::Error, fmt};

/// The recognized codes, sorted, pinned from `golang.org/x/text@v0.37.0/currency/tables.go`.
///
/// Position `i` corresponds to Go's table index `i + 1` (Go's index 0 is a null entry).
const CODES: [&str; 300] = [
    "ADP", "AED", "AFA", "AFN", "ALK", "ALL", "AMD", "ANG", "AOA", "AOK", "AON", "AOR", "ARA",
    "ARL", "ARM", "ARP", "ARS", "ATS", "AUD", "AWG", "AZM", "AZN", "BAD", "BAM", "BAN", "BBD",
    "BDT", "BEC", "BEF", "BEL", "BGL", "BGM", "BGN", "BGO", "BHD", "BIF", "BMD", "BND", "BOB",
    "BOL", "BOP", "BOV", "BRB", "BRC", "BRE", "BRL", "BRN", "BRR", "BRZ", "BSD", "BTN", "BUK",
    "BWP", "BYB", "BYN", "BYR", "BZD", "CAD", "CDF", "CHE", "CHF", "CHW", "CLE", "CLF", "CLP",
    "CNH", "CNX", "CNY", "COP", "COU", "CRC", "CSD", "CSK", "CUC", "CUP", "CVE", "CYP", "CZK",
    "DDM", "DEM", "DJF", "DKK", "DOP", "DZD", "ECS", "ECV", "EEK", "EGP", "ERN", "ESA", "ESB",
    "ESP", "ETB", "EUR", "FIM", "FJD", "FKP", "FRF", "GBP", "GEK", "GEL", "GHC", "GHS", "GIP",
    "GMD", "GNF", "GNS", "GQE", "GRD", "GTQ", "GWE", "GWP", "GYD", "HKD", "HNL", "HRD", "HRK",
    "HTG", "HUF", "IDR", "IEP", "ILP", "ILR", "ILS", "INR", "IQD", "IRR", "ISJ", "ISK", "ITL",
    "JMD", "JOD", "JPY", "KES", "KGS", "KHR", "KMF", "KPW", "KRH", "KRO", "KRW", "KWD", "KYD",
    "KZT", "LAK", "LBP", "LKR", "LRD", "LSL", "LTL", "LTT", "LUC", "LUF", "LUL", "LVL", "LVR",
    "LYD", "MAD", "MAF", "MCF", "MDC", "MDL", "MGA", "MGF", "MKD", "MKN", "MLF", "MMK", "MNT",
    "MOP", "MRO", "MTL", "MTP", "MUR", "MVP", "MVR", "MWK", "MXN", "MXP", "MXV", "MYR", "MZE",
    "MZM", "MZN", "NAD", "NGN", "NIC", "NIO", "NLG", "NOK", "NPR", "NZD", "OMR", "PAB", "PEI",
    "PEN", "PES", "PGK", "PHP", "PKR", "PLN", "PLZ", "PTE", "PYG", "QAR", "RHD", "ROL", "RON",
    "RSD", "RUB", "RUR", "RWF", "SAR", "SBD", "SCR", "SDD", "SDG", "SDP", "SEK", "SGD", "SHP",
    "SIT", "SKK", "SLL", "SOS", "SRD", "SRG", "SSP", "STD", "STN", "SUR", "SVC", "SYP", "SZL",
    "THB", "TJR", "TJS", "TMM", "TMT", "TND", "TOP", "TPE", "TRL", "TRY", "TTD", "TWD", "TZS",
    "UAH", "UAK", "UGS", "UGX", "USD", "USN", "USS", "UYI", "UYP", "UYU", "UZS", "VEB", "VEF",
    "VND", "VNN", "VUV", "WST", "XAF", "XAG", "XAU", "XBA", "XBB", "XBC", "XBD", "XCD", "XDR",
    "XEU", "XFO", "XFU", "XOF", "XPD", "XPF", "XPT", "XRE", "XSU", "XTS", "XUA", "XXX", "YDD",
    "YER", "YUD", "YUM", "YUN", "YUR", "ZAL", "ZAR", "ZMK", "ZMW", "ZRN", "ZRZ", "ZWD", "ZWL",
    "ZWR",
];

/// The code of Go's zero `Unit`, which `ParseISO("XXX")` returns.
const XXX: &str = "XXX";

/// An ISO 4217 currency designator (Go: `currency.Unit`).
///
/// Two units are equal exactly when their [`Unit::as_str`] codes are equal, which is how Go's
/// callers compare them (`fromUnit.String() == toUnit.String()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Unit {
    /// Go's table index; 0 is `XXX`.
    index: u16,
}

impl Unit {
    /// The canonical upper-case ISO code (Go: `Unit.String()`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        if self.index == 0 {
            return XXX;
        }
        usize::from(self.index)
            .checked_sub(1)
            .and_then(|position| CODES.get(position))
            .copied()
            .unwrap_or(XXX)
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The two errors of Go's `currency.ParseISO`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParseIsoError {
    /// Go: `errSyntax`, the input is not exactly three ASCII letters.
    NotWellFormed,
    /// Go: `errValue`, three letters that are not in the CLDR 32 currency table.
    NotRecognized,
}

impl fmt::Display for ParseIsoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotWellFormed => "currency: tag is not well-formed",
            Self::NotRecognized => "currency: tag is not a recognized currency",
        })
    }
}

impl Error for ParseIsoError {}

/// Parses a 3-letter ISO 4217 code, case-insensitively (Go: `currency.ParseISO`).
///
/// # Errors
///
/// [`ParseIsoError::NotWellFormed`] unless the input is exactly three ASCII letters, and
/// [`ParseIsoError::NotRecognized`] for letters that are not a code in the pinned table.
pub fn parse_iso(s: &str) -> Result<Unit, ParseIsoError> {
    // Go copies at most 4 bytes and tag.FixCase rejects anything that is not 3 bytes long.
    let [first, second, third] = s.as_bytes() else {
        return Err(ParseIsoError::NotWellFormed);
    };
    let key = [fix_case(*first)?, fix_case(*second)?, fix_case(*third)?];
    let code = str::from_utf8(&key).map_err(|_utf8| ParseIsoError::NotWellFormed)?;
    if code == XXX {
        return Ok(Unit { index: 0 });
    }
    let position = CODES
        .binary_search(&code)
        .map_err(|_missing| ParseIsoError::NotRecognized)?;
    let index = position
        .checked_add(1)
        .and_then(|index| u16::try_from(index).ok())
        .ok_or(ParseIsoError::NotRecognized)?;
    Ok(Unit { index })
}

/// Go: `tag.FixCase("XXX", key)` for one byte, upper-casing ASCII letters.
const fn fix_case(byte: u8) -> Result<u8, ParseIsoError> {
    let upper = if byte >= b'a' {
        byte - (b'z' - b'Z')
    } else {
        byte
    };
    if upper.is_ascii_uppercase() {
        Ok(upper)
    } else {
        Err(ParseIsoError::NotWellFormed)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::{CODES, ParseIsoError, Unit, parse_iso};
    use crate::golden::{TestResult, check, check_eq, fnv64a_hex, hex_decode, read_golden};

    fn render(s: &str) -> String {
        match parse_iso(s) {
            Ok(unit) => format!("ok:{unit}"),
            Err(err) => format!("err:{err}"),
        }
    }

    #[test]
    fn table_is_sorted_and_matches_go() -> TestResult {
        check(CODES.is_sorted(), "CODES must be sorted for binary search")?;
        let go_codes: Vec<String> = read_golden("iso4217.golden")?
            .into_iter()
            .filter(|fields| fields.first().map(String::as_str) == Some("code"))
            .filter_map(|fields| fields.get(1).cloned())
            .collect();
        let rust_codes: Vec<String> = CODES.iter().map(|code| (*code).to_owned()).collect();
        check_eq(&rust_codes, &go_codes, "recognized codes")
    }

    #[test]
    fn explicit_cases_match_go() -> TestResult {
        let mut cases = 0_usize;
        for fields in read_golden("iso4217.golden")? {
            match fields.as_slice() {
                [kind, input, canonical] if kind == "code" => {
                    check_eq(&render(input), &format!("ok:{canonical}"), input)?;
                    check_eq(
                        &render(&input.to_lowercase()),
                        &format!("ok:{canonical}"),
                        input,
                    )?;
                }
                [kind, input_hex, expected] if kind == "case" => {
                    let input = String::from_utf8(hex_decode(input_hex)?)?;
                    check_eq(&render(&input), expected, &format!("ParseISO({input:?})"))?;
                    cases += 1;
                }
                _ => {}
            }
        }
        check(cases > 40, "the golden must hold the explicit cases")
    }

    fn digest_of(alphabet: &[u8]) -> Result<(usize, String), Box<dyn Error>> {
        let mut input = Vec::new();
        let mut count = 0_usize;
        for &a in alphabet {
            for &b in alphabet {
                for &c in alphabet {
                    let s = str::from_utf8(&[a, b, c])?.to_owned();
                    input.extend_from_slice(s.as_bytes());
                    input.push(b'\t');
                    input.extend_from_slice(render(&s).as_bytes());
                    input.push(b'\n');
                    count += 1;
                }
            }
        }
        Ok((count, fnv64a_hex(&input)))
    }

    #[test]
    fn every_ascii_triple_matches_go() -> TestResult {
        let ascii: Vec<u8> = (0_u8..128).collect();
        let mut letters: Vec<u8> = (b'A'..=b'Z').collect();
        letters.extend(b'a'..=b'z');
        let mut checked = 0;
        for fields in read_golden("iso4217.golden")? {
            if let [kind, name, count, digest] = fields.as_slice()
                && kind == "digest"
            {
                let alphabet = if name == "ascii3" { &ascii } else { &letters };
                let (rust_count, rust_digest) = digest_of(alphabet)?;
                check_eq(&rust_count.to_string(), count, name)?;
                check_eq(&rust_digest, digest, name)?;
                checked += 1;
            }
        }
        check_eq(&checked, &2, "digest lines")
    }

    #[test]
    fn go_x_text_parse_iso_test() -> TestResult {
        // Go: golang.org/x/text/currency TestParseISO ("\xff\xff\xff" is not a Rust &str; the
        // byte-level behaviour is the same as for every other non-letter byte).
        let cases: [(&str, Result<&str, ParseIsoError>); 14] = [
            ("USD", Ok("USD")),
            ("xxx", Ok("XXX")),
            ("xts", Ok("XTS")),
            ("XX", Err(ParseIsoError::NotWellFormed)),
            ("XXXX", Err(ParseIsoError::NotWellFormed)),
            ("", Err(ParseIsoError::NotWellFormed)),
            ("UUU", Err(ParseIsoError::NotRecognized)),
            ("\u{22A9}", Err(ParseIsoError::NotWellFormed)),
            ("aaa", Err(ParseIsoError::NotRecognized)),
            ("zzz", Err(ParseIsoError::NotRecognized)),
            ("000", Err(ParseIsoError::NotWellFormed)),
            ("999", Err(ParseIsoError::NotWellFormed)),
            ("---", Err(ParseIsoError::NotWellFormed)),
            ("\0\0\0", Err(ParseIsoError::NotWellFormed)),
        ];
        for (input, expected) in cases {
            check_eq(&parse_iso(input).map(Unit::as_str), &expected, input)?;
        }
        Ok(())
    }
}
