//! Go `util/mathutil`.

/// Go: `mathutil.RoundTo4Decimals`: `math.Round(amount*10000) / 10000`, rounding halves away
/// from zero after the multiplication (so `123.45675` rounds down, being stored as
/// `123.456749...`).
#[must_use]
pub fn round_to_4_decimals(amount: f64) -> f64 {
    (amount * 10000.0).round() / 10000.0
}

#[cfg(test)]
mod tests {
    use super::round_to_4_decimals;
    use crate::golden::{TestResult, check, check_eq, fields, read_golden, text};

    #[test]
    fn round_to_4_decimals_go_cases() -> TestResult {
        // Go: TestRoundTo4Decimals.
        let cases = [
            (123.456_789, 123.4568),
            (-123.456_789, -123.4568),
            (123.4567, 123.4567),
            (123.456_75, 123.4568),
            (123.456_74, 123.4567),
            (0.000_05, 0.0001),
            (-0.000_05, -0.0001),
            (0.0, 0.0),
        ];
        for (input, want) in cases {
            check_eq(
                &round_to_4_decimals(input).to_bits(),
                &f64::to_bits(want),
                &input.to_string(),
            )?;
        }
        Ok(())
    }

    #[test]
    fn round_to_4_decimals_golden() -> TestResult {
        // Go: mathutil.RoundTo4Decimals (util/mathutil/mathutil.go:7), bit-exact for curated
        // values, random bit patterns and near-halves. NaN payloads are not compared: they
        // depend on the CPU, and JSON cannot carry a NaN.
        let rows = read_golden("mathutil.txt")?;
        check(rows.len() > 2000, "mathutil.txt is complete")?;
        for row in &rows {
            let [input, want] = fields::<2>(row)?;
            let input = f64::from_bits(u64::from_str_radix(text(input)?, 16)?);
            let want = f64::from_bits(u64::from_str_radix(text(want)?, 16)?);
            let got = round_to_4_decimals(input);
            if got.is_nan() && want.is_nan() {
                continue;
            }
            check_eq(
                &format!("{:x}", got.to_bits()),
                &format!("{:x}", want.to_bits()),
                &format!("{input:e}"),
            )?;
        }
        Ok(())
    }
}
