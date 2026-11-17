//! Go: `logsink.textPrintf` (the line format) and glog's `defaultFormat` and `lnFormat`.

use pbs_gocompat::{
    fmt::{Value, sprintf_bytes},
    time::Time,
};

use super::{Severity, Site};

/// Go: `logsink.MaxLogMessageLen`: lines are cut to one byte less.
pub(super) const MAX_LOG_MESSAGE_LEN: usize = 15000;

/// Go: `twoDigits`.
fn two_digits(buf: &mut Vec<u8>, d: i64) {
    let d = u8::try_from(d.rem_euclid(100)).unwrap_or_default();
    buf.push(b'0' + d / 10);
    buf.push(b'0' + d % 10);
}

/// Go: `nDigits`: `d` right-aligned in `n` columns padded with `pad`; zero prints no digit.
fn n_digits(buf: &mut Vec<u8>, n: usize, d: u64, pad: u8) {
    let digits = if d == 0 { String::new() } else { d.to_string() };
    buf.resize(buf.len() + n.saturating_sub(digits.len()), pad);
    buf.extend_from_slice(digits.as_bytes());
}

/// Go: the header `logsink.textPrintf` writes before the message.
pub(super) fn header(buf: &mut Vec<u8>, severity: Severity, t: &Time, pid: u32, site: Site<'_>) {
    buf.push(severity.letter());
    two_digits(buf, t.month());
    two_digits(buf, t.day());
    buf.push(b' ');
    two_digits(buf, t.hour());
    buf.push(b':');
    two_digits(buf, t.minute());
    buf.push(b':');
    two_digits(buf, t.second());
    buf.push(b'.');
    n_digits(
        buf,
        6,
        u64::try_from(t.nanosecond() / 1000).unwrap_or_default(),
        b'0',
    );
    buf.push(b' ');
    n_digits(buf, 7, u64::from(pid), b' ');
    buf.push(b' ');
    buf.extend_from_slice(site.base_name().as_bytes());
    buf.push(b':');
    buf.extend_from_slice(site.line.to_string().as_bytes());
    buf.extend_from_slice(b"] ");
}

/// Go: `logsink.textPrintf`: the header, the message formatted by Go's `fmt.Sprintf`, cut to
/// 14999 bytes, and a final newline unless the cut text ends in one.
pub(super) fn line(
    severity: Severity,
    t: &Time,
    pid: u32,
    site: Site<'_>,
    format: &str,
    args: &[Value],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64 + format.len());
    header(&mut buf, severity, t, pid, site);
    buf.extend_from_slice(&sprintf_bytes(format, args));
    buf.truncate(MAX_LOG_MESSAGE_LEN - 1);
    if buf.last() != Some(&b'\n') {
        buf.push(b'\n');
    }
    buf
}

/// Go: `reflect.TypeOf(arg).Kind() == reflect.String`.
fn is_string_kind(v: &Value) -> bool {
    if let Value::Named { value, .. } | Value::Method { value, .. } = v {
        return is_string_kind(value);
    }
    matches!(v, Value::String(_))
}

/// Go: `defaultFormat`: `%v` per operand, with a space between two that are not strings.
pub(super) fn default_format(args: &[Value]) -> String {
    match args {
        [] => String::new(),
        [_] => "%v".to_owned(),
        _ => {
            let mut b = String::with_capacity(args.len() * 3);
            let mut was_string = true;
            for arg in args {
                let is_string = is_string_kind(arg);
                b.push_str(if was_string || is_string { "%v" } else { " %v" });
                was_string = is_string;
            }
            b
        }
    }
}

/// Go: `lnFormat`: `%v` per operand separated by spaces, then a newline.
pub(super) fn ln_format(args: &[Value]) -> String {
    if args.is_empty() {
        return "\n".to_owned();
    }
    let mut b = vec!["%v"; args.len()].join(" ");
    b.push('\n');
    b
}
