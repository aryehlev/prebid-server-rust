//! Go `util/stringutil`.

use pbs_gocompat::strconv::{self, NumError};

/// Go: `stringutil.StrToInt8Slice`: the comma-separated `int8` values of `s`, `None` (Go: nil)
/// for an empty string.
///
/// # Errors
///
/// The `strconv.ParseInt` error of the first element that is not an `int8`, such as
/// `strconv.ParseInt: parsing "128": value out of range`; an empty element is a syntax error.
pub fn str_to_int8_slice(s: &str) -> Result<Option<Vec<i8>>, NumError> {
    if s.is_empty() {
        return Ok(None);
    }
    s.split(',')
        .map(|part| {
            let v = strconv::parse_int(part, 10, 8)?;
            // ParseInt with bitSize 8 only returns values that fit.
            Ok(i8::try_from(v).unwrap_or_default())
        })
        .collect::<Result<Vec<i8>, NumError>>()
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::str_to_int8_slice;
    use crate::golden::{TestResult, check, check_eq, err_text, fields, read_golden, text};

    #[test]
    fn str_to_int8_slice_go_cases() -> TestResult {
        // Go: TestStrToInt8Slice.
        let cases: [(&str, Option<Vec<i8>>, &str); 6] = [
            ("", None, ""),
            (
                "malformed",
                None,
                r#"error: strconv.ParseInt: parsing "malformed": invalid syntax"#,
            ),
            (
                "malformed,2,malformed",
                None,
                r#"error: strconv.ParseInt: parsing "malformed": invalid syntax"#,
            ),
            (
                "128",
                None,
                r#"error: strconv.ParseInt: parsing "128": value out of range"#,
            ),
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
            check_eq(err_text(&got).as_bytes(), err, s)?;
        }
        Ok(())
    }
}
