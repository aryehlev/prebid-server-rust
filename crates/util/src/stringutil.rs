//! Go `util/stringutil`.

use std::num::ParseIntError;

/// Go: `stringutil.StrToInt8Slice`: the comma-separated `int8` values of `s`.
///
/// `None` (Go: nil) for an empty string. Each value is parsed as Go's
/// `strconv.ParseInt(part, 10, 8)` parses it: an optional sign and decimal digits.
///
/// # Errors
///
/// The error of the first element that is not an `int8`; an empty element is one. Its text is
/// std's, not Go's `strconv.ParseInt: parsing "128": value out of range`, which the cookie sync
/// endpoint returns to the caller (the usersync.gpp-sid-error-text ledger entry).
pub fn str_to_int8_slice(s: &str) -> Result<Option<Vec<i8>>, ParseIntError> {
    if s.is_empty() {
        return Ok(None);
    }
    s.split(',')
        .map(str::parse::<i8>)
        .collect::<Result<Vec<i8>, ParseIntError>>()
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::str_to_int8_slice;
    use crate::golden::{TestResult, check, check_eq, err_text, fields, read_golden, text};

    #[test]
    fn str_to_int8_slice_go_cases() -> TestResult {
        // Go: TestStrToInt8Slice, with std's error texts in place of strconv's
        // (`strconv.ParseInt: parsing "malformed": invalid syntax`, `... "128": value out of
        // range`), as the usersync.gpp-sid-error-text ledger entry records.
        let cases: [(&str, Option<Vec<i8>>, &str); 6] = [
            ("", None, ""),
            ("malformed", None, "error: invalid digit found in string"),
            (
                "malformed,2,malformed",
                None,
                "error: invalid digit found in string",
            ),
            ("128", None, "error: number too large to fit in target type"),
            ("127", Some(vec![127]), ""),
            ("127,2,-127", Some(vec![127, 2, -127]), ""),
        ];
        for (input, want, want_err) in cases {
            let got = str_to_int8_slice(input);
            check_eq(&got.clone().ok().flatten(), &want, input)?;
            check_eq(err_text(&got).as_str(), want_err, input)?;
        }
        Ok(())
    }

    #[test]
    fn str_to_int8_slice_golden() -> TestResult {
        // Go: StrToInt8Slice's values and whether it fails; the error texts are std's.
        let rows = read_golden("stringutil.txt")?;
        check(rows.len() > 20, "stringutil.txt is complete")?;
        for row in &rows {
            let [s, is_nil, joined, err] = fields::<4>(row)?;
            let s = text(s)?;
            let got = str_to_int8_slice(s);
            let values = got.clone().ok().flatten();
            let nil: &[u8] = if values.is_none() { b"true" } else { b"false" };
            check_eq(nil, is_nil, s)?;
            let got_joined = values
                .unwrap_or_default()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ");
            check_eq(got_joined.as_bytes(), joined, s)?;
            check_eq(&got.is_err(), &!err.is_empty(), s)?;
        }
        Ok(())
    }
}
