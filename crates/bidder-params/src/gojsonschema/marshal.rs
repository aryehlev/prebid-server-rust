//! Go: gojsonschema/utils.go `marshalWithoutNumber`.
//!
//! `json.Marshal(value)`, `json.Unmarshal` into `interface{}` (numbers become `float64`), and
//! `json.Marshal` again. The result is the canonical text used to compare `enum`, `const` and
//! `uniqueItems` values and to print the allowed values in error messages.

use std::fmt::Write as _;

use super::json_loader::{Document, Node, NodeId};

/// Go: encoding/json `UnmarshalTypeError` text for a number that does not fit `float64`.
fn number_error(literal: &str) -> String {
    format!("json: cannot unmarshal number {literal} into Go value of type float64")
}

/// Go: `marshalWithoutNumber(value)`.
///
/// # Errors
///
/// Returns Go's error text when a number overflows `float64`.
pub(super) fn marshal_without_number(doc: &Document, id: NodeId) -> Result<String, String> {
    // Map keys are sorted (Go sorts map keys when encoding), arrays keep their order. The walk
    // is iterative because Go accepts 10000 levels of nesting.
    enum Task {
        Value(NodeId),
        Text(&'static str),
        Key(String),
    }
    let mut out = String::new();
    let mut first_error: Option<String> = None;
    let mut stack = vec![Task::Value(id)];
    while let Some(task) = stack.pop() {
        match task {
            Task::Text(t) => out.push_str(t),
            Task::Key(k) => {
                write_go_string(&mut out, &k);
                out.push(':');
            }
            Task::Value(v) => match doc.node(v) {
                Node::Null => out.push_str("null"),
                Node::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
                Node::String(s) => write_go_string(&mut out, s),
                Node::Number(n) => match go_float64_json(n) {
                    Some(text) => out.push_str(&text),
                    None => {
                        // Go's decoder keeps going and reports the first error.
                        if first_error.is_none() {
                            first_error = Some(number_error(n));
                        }
                    }
                },
                Node::Array(items) => {
                    out.push('[');
                    stack.push(Task::Text("]"));
                    for (i, &item) in items.iter().enumerate().rev() {
                        stack.push(Task::Value(item));
                        if i > 0 {
                            stack.push(Task::Text(","));
                        }
                    }
                }
                Node::Object(entries) => {
                    out.push('{');
                    stack.push(Task::Text("}"));
                    let mut sorted: Vec<&(String, NodeId)> = entries.iter().collect();
                    sorted.sort_by(|a, b| a.0.cmp(&b.0));
                    for (i, (k, item)) in sorted.iter().enumerate().rev() {
                        stack.push(Task::Value(*item));
                        stack.push(Task::Key(k.clone()));
                        if i > 0 {
                            stack.push(Task::Text(","));
                        }
                    }
                }
            },
        }
    }
    first_error.map_or(Ok(out), Err)
}

/// Go: `strconv.ParseFloat(literal, 64)` followed by encoding/json's `float64` encoder. `None`
/// when the literal overflows `float64` (Go: `ErrRange`).
pub(super) fn go_float64_json(literal: &str) -> Option<String> {
    let f: f64 = literal.parse().ok()?;
    if f.is_infinite() || f.is_nan() {
        return None;
    }
    Some(format_float64_json(f))
}

/// Go: encoding/json `floatEncoder.encode` for `float64`.
fn format_float64_json(f: f64) -> String {
    // `{:e}` prints the shortest round-trip digits, like strconv's `-1` precision.
    let sci = format!("{f:e}");
    let (mantissa, exp_text) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exp: i64 = exp_text.parse().unwrap_or(0);
    let (neg, mantissa) = mantissa
        .strip_prefix('-')
        .map_or((false, mantissa), |rest| (true, rest));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    let abs = f.abs();
    if abs != 0.0 && !(1e-6..1e21).contains(&abs) {
        let (head, tail) = digits.split_at(1.min(digits.len()));
        out.push_str(head);
        if !tail.is_empty() {
            out.push('.');
            out.push_str(tail);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let e = exp.unsigned_abs();
        // strconv writes at least two exponent digits; encoding/json then turns e-07 into e-7.
        if e < 10 && exp >= 0 {
            out.push('0');
        }
        out.push_str(&e.to_string());
        return out;
    }
    if digits == "0" {
        out.push('0');
        return out;
    }
    let n = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    let point = exp + 1; // digits before the decimal point
    if point >= n {
        out.push_str(&digits);
        out.push_str(&"0".repeat(usize::try_from(point - n).unwrap_or(0)));
    } else if point > 0 {
        let (int_part, frac_part) = digits.split_at(usize::try_from(point).unwrap_or(0));
        out.push_str(int_part);
        out.push('.');
        out.push_str(frac_part);
    } else {
        out.push_str("0.");
        out.push_str(&"0".repeat(usize::try_from(-point).unwrap_or(0)));
        out.push_str(&digits);
    }
    out
}

/// Go: encoding/json `appendString` with HTML escaping (what `json.Marshal` does).
pub(super) fn write_go_string(out: &mut String, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '"' => out.push_str("\\\""),
            '\u{8}' => out.push_str(r"\b"),
            '\u{c}' => out.push_str(r"\f"),
            '\n' => out.push_str(r"\n"),
            '\r' => out.push_str(r"\r"),
            '\t' => out.push_str(r"\t"),
            '\u{2028}' => out.push_str(r"\u2028"),
            '\u{2029}' => out.push_str(r"\u2029"),
            '<' | '>' | '&' | '\u{0}'..='\u{1f}' => {
                let b = u32::from(c);
                let hi = HEX
                    .get(usize::try_from(b >> 4).unwrap_or(0))
                    .copied()
                    .unwrap_or(b'0');
                let lo = HEX
                    .get(usize::try_from(b & 0xF).unwrap_or(0))
                    .copied()
                    .unwrap_or(b'0');
                let _ok = write!(out, r"\u00{}{}", char::from(hi), char::from(lo));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::go_float64_json;

    #[test]
    fn floats_match_go_encoding_json() -> Result<(), String> {
        // Go: json.Marshal(float64(x)).
        let cases = [
            ("1", "1"),
            ("1.0", "1"),
            ("-0", "-0"),
            ("0.000001", "0.000001"),
            ("0.0000001", "1e-7"),
            ("1e-10", "1e-10"),
            ("1e20", "100000000000000000000"),
            ("1e21", "1e+21"),
            ("123456789012345678901234567890", "1.2345678901234568e+29"),
            ("1.5", "1.5"),
            ("-2.25e-8", "-2.25e-8"),
            ("9007199254740993", "9007199254740992"),
            ("1e308", "1e+308"),
            ("5e-324", "5e-324"),
        ];
        for (input, want) in cases {
            let got = go_float64_json(input).ok_or_else(|| format!("{input} overflowed"))?;
            if got != want {
                return Err(format!("{input}: got {got}, want {want}"));
            }
        }
        if go_float64_json("1e400").is_some() {
            return Err("1e400 must overflow".to_owned());
        }
        Ok(())
    }
}
