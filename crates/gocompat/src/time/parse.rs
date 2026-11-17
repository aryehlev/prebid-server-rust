//! Go: `time.Parse` (`parse`, `parseRFC3339`, `parseStrictRFC3339`) and its helpers.

use std::{error::Error as StdError, fmt as stdfmt};

use super::{
    DateFields, Location, RFC3339, RFC3339_NANO, Time, days_before, days_in,
    format::{
        LONG_DAY_NAMES, LONG_MONTH_NAMES, SHORT_DAY_NAMES, SHORT_MONTH_NAMES, Std, is_digit,
        next_std_chunk,
    },
    is_leap,
};

/// Go: `time.ParseError`.
///
/// The fields are Go strings: the value element can split a UTF-8 sequence of the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    layout: Vec<u8>,
    value: Vec<u8>,
    layout_elem: Vec<u8>,
    value_elem: Vec<u8>,
    message: String,
}

impl ParseError {
    fn new(
        layout: &[u8],
        value: &[u8],
        layout_elem: &[u8],
        value_elem: &[u8],
        message: String,
    ) -> Self {
        Self {
            layout: layout.to_vec(),
            value: value.to_vec(),
            layout_elem: layout_elem.to_vec(),
            value_elem: value_elem.to_vec(),
            message,
        }
    }

    /// Go: `ParseError.Layout`.
    #[must_use]
    pub fn layout(&self) -> &[u8] {
        &self.layout
    }

    /// Go: `ParseError.Value`.
    #[must_use]
    pub fn value(&self) -> &[u8] {
        &self.value
    }

    /// Go: `ParseError.LayoutElem`.
    #[must_use]
    pub fn layout_elem(&self) -> &[u8] {
        &self.layout_elem
    }

    /// Go: `ParseError.ValueElem`.
    #[must_use]
    pub fn value_elem(&self) -> &[u8] {
        &self.value_elem
    }

    /// Go: `ParseError.Message`.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl stdfmt::Display for ParseError {
    /// Go: `ParseError.Error`.
    fn fmt(&self, f: &mut stdfmt::Formatter<'_>) -> stdfmt::Result {
        f.write_str("parsing time ")?;
        f.write_str(&quote(&self.value))?;
        if !self.message.is_empty() {
            return f.write_str(&self.message);
        }
        f.write_str(" as ")?;
        f.write_str(&quote(&self.layout))?;
        f.write_str(": cannot parse ")?;
        f.write_str(&quote(&self.value_elem))?;
        f.write_str(" as ")?;
        f.write_str(&quote(&self.layout_elem))
    }
}

impl StdError for ParseError {}

/// Go: the `time` package's `quote`: control bytes and every byte of a non-ASCII sequence
/// become `\xNN`, and `"` and `\` are escaped.
pub(super) fn quote(s: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for &b in s {
        if (b' '..0x80).contains(&b) {
            if b == b'"' || b == b'\\' {
                out.push('\\');
            }
            out.push(char::from(b));
        } else {
            out.push_str("\\x");
            out.push(char::from(
                HEX.get(usize::from(b >> 4)).copied().unwrap_or(b'0'),
            ));
            out.push(char::from(
                HEX.get(usize::from(b & 0xF)).copied().unwrap_or(b'0'),
            ));
        }
    }
    out.push('"');
    out
}

/// Go: `errBad` and friends: the step failed.
#[derive(Debug, Clone, Copy)]
pub(super) struct Bad;

/// Go: `leadingInt`: the leading decimal digits of `s` and the rest.
pub(super) fn leading_int(s: &[u8]) -> Result<(u64, &[u8]), Bad> {
    let mut x: u64 = 0;
    let mut i = 0;
    for &c in s {
        if !c.is_ascii_digit() {
            break;
        }
        if x > (1 << 63) / 10 {
            return Err(Bad);
        }
        x = x * 10 + u64::from(c - b'0');
        if x > 1 << 63 {
            return Err(Bad);
        }
        i += 1;
    }
    Ok((x, s.get(i..).unwrap_or_default()))
}

/// Go: `atoi`.
fn atoi(s: &[u8]) -> Result<i64, Bad> {
    let (neg, digits) = match s.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        Some(_) | None => (false, s),
    };
    let (q, rem) = leading_int(digits)?;
    if !rem.is_empty() {
        return Err(Bad);
    }
    let x = q.cast_signed();
    Ok(if neg { x.wrapping_neg() } else { x })
}

/// Go: `match`: ASCII case-insensitive equality of `s1` with the same-length prefix of `s2`.
fn matches_fold(s1: &[u8], s2: &[u8]) -> bool {
    s1.iter().zip(s2).all(|(&c1, &c2)| {
        c1 == c2 || {
            let (l1, l2) = (c1 | 0x20, c2 | 0x20);
            l1 == l2 && l1.is_ascii_lowercase()
        }
    })
}

/// Go: `lookup`: the index of the name `val` starts with, and the rest of `val`.
fn lookup<'v>(table: &[&str], val: &'v [u8]) -> Result<(i64, &'v [u8]), Bad> {
    for (i, name) in (0_i64..).zip(table) {
        let name = name.as_bytes();
        if let Some(head) = val.get(..name.len())
            && matches_fold(head, name)
        {
            return Ok((i, val.get(name.len()..).unwrap_or_default()));
        }
    }
    Err(Bad)
}

/// Go: `getnum`: one or two digits (exactly two when `fixed`).
fn getnum(s: &[u8], fixed: bool) -> Result<(i64, &[u8]), Bad> {
    let first = s.first().filter(|c| c.is_ascii_digit()).ok_or(Bad)?;
    let tens = i64::from(first - b'0');
    match s.get(1).filter(|c| c.is_ascii_digit()) {
        Some(second) => Ok((
            tens * 10 + i64::from(second - b'0'),
            s.get(2..).unwrap_or_default(),
        )),
        None if fixed => Err(Bad),
        None => Ok((tens, s.get(1..).unwrap_or_default())),
    }
}

/// Go: `getnum3`: one to three digits (exactly three when `fixed`).
fn getnum3(s: &[u8], fixed: bool) -> Result<(i64, &[u8]), Bad> {
    let digits = s.iter().take(3).take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || fixed && digits != 3 {
        return Err(Bad);
    }
    let n = s
        .iter()
        .take(digits)
        .fold(0, |n, &c| n * 10 + i64::from(c - b'0'));
    Ok((n, s.get(digits..).unwrap_or_default()))
}

/// Go: `cutspace`.
fn cutspace(s: &[u8]) -> &[u8] {
    let n = s.iter().take_while(|&&c| c == b' ').count();
    s.get(n..).unwrap_or_default()
}

/// Go: `skip`: removes the literal `prefix` from `value`, where a run of spaces matches any
/// run of spaces. On failure, the value from the mismatch.
fn skip<'v>(value: &'v [u8], prefix: &[u8]) -> Result<&'v [u8], &'v [u8]> {
    let (mut value, mut prefix) = (value, prefix);
    while let Some((&p, prefix_rest)) = prefix.split_first() {
        if p == b' ' {
            if value.first().is_some_and(|&c| c != b' ') {
                return Err(value);
            }
            prefix = cutspace(prefix);
            value = cutspace(value);
            continue;
        }
        match value.split_first() {
            Some((&c, value_rest)) if c == p => {
                prefix = prefix_rest;
                value = value_rest;
            }
            _ => return Err(value),
        }
    }
    Ok(value)
}

/// Go: `commaOrPeriod`.
const fn comma_or_period(b: u8) -> bool {
    b == b'.' || b == b','
}

/// Go: `parseNanoseconds`: the fraction in `value[..nbytes]`, which starts with its separator.
/// The second result is Go's range error name.
fn parse_nanoseconds(value: &[u8], nbytes: usize) -> (Result<i64, Bad>, &'static str) {
    if !value.first().is_some_and(|&c| comma_or_period(c)) {
        return (Err(Bad), "");
    }
    let nbytes = nbytes.min(10);
    let ns = match atoi(value.get(1..nbytes).unwrap_or_default()) {
        Ok(ns) => ns,
        Err(bad) => return (Err(bad), ""),
    };
    if ns < 0 {
        return (Ok(ns), "fractional second");
    }
    let scale = (nbytes..10).fold(ns, |ns, _| ns.wrapping_mul(10));
    (Ok(scale), "")
}

/// Go: `parseSignedOffset`: the length of a `+hh`/`-hh` offset (hour at most 23), or 0.
fn parse_signed_offset(value: &[u8]) -> usize {
    let Some((&sign, digits)) = value.split_first() else {
        return 0;
    };
    if sign != b'-' && sign != b'+' {
        return 0;
    }
    match leading_int(digits) {
        Ok((x, rem)) if rem.len() != digits.len() && x <= 23 => value.len() - rem.len(),
        Ok(_) | Err(Bad) => 0,
    }
}

/// Go: `parseTimeZone`: the length of a zone abbreviation at the start of `value`.
fn parse_time_zone(value: &[u8]) -> Option<usize> {
    if value.len() < 3 {
        return None;
    }
    if value.starts_with(b"ChST") || value.starts_with(b"MeST") {
        return Some(4);
    }
    if let Some(rest) = value.strip_prefix(b"GMT") {
        // Go: parseGMT.
        return Some(if rest.is_empty() {
            3
        } else {
            3 + parse_signed_offset(rest)
        });
    }
    if value.first().is_some_and(|&c| c == b'+' || c == b'-') {
        let n = parse_signed_offset(value);
        return (n > 0).then_some(n);
    }
    let upper = value
        .iter()
        .take(6)
        .take_while(|c| c.is_ascii_uppercase())
        .count();
    match upper {
        5 if value.get(4) == Some(&b'T') => Some(5),
        4 if value.get(3) == Some(&b'T') || value.starts_with(b"WITA") => Some(4),
        3 => Some(3),
        _ => None,
    }
}

/// The result of one layout element: the rest of the value, Go's `err` and its range error.
struct Step<'v> {
    value: &'v [u8],
    bad: bool,
    range: &'static str,
}

impl<'v> Step<'v> {
    const fn ok(value: &'v [u8]) -> Self {
        Self {
            value,
            bad: false,
            range: "",
        }
    }

    const fn bad(value: &'v [u8]) -> Self {
        Self {
            value,
            bad: true,
            range: "",
        }
    }
}

/// Go: `parse`'s local variables.
struct Fields {
    year: i64,
    month: i64,
    day: i64,
    yday: i64,
    hour: i64,
    min: i64,
    sec: i64,
    nsec: i64,
    /// Go: `z`, which is only ever `UTC`.
    z_utc: bool,
    zone_offset: i64,
    zone_name: Vec<u8>,
    am_set: bool,
    pm_set: bool,
}

impl Default for Fields {
    fn default() -> Self {
        Self {
            year: 0,
            month: -1,
            day: -1,
            yday: -1,
            hour: 0,
            min: 0,
            sec: 0,
            nsec: 0,
            z_utc: false,
            zone_offset: -1,
            zone_name: Vec::new(),
            am_set: false,
            pm_set: false,
        }
    }
}

/// Splits `value` after `n` bytes, or `None` when it is shorter.
fn split(value: &[u8], n: usize) -> Option<(&[u8], &[u8])> {
    (value.len() >= n).then(|| value.split_at(n))
}

/// Go: `getnum`'s results as `parse` assigns them: 0 and the unchanged value on failure.
fn num_step<'v>(
    target: &mut i64,
    value: &'v [u8],
    result: Result<(i64, &'v [u8]), Bad>,
) -> Step<'v> {
    *target = result.map_or(0, |(n, _)| n);
    result.map_or_else(|Bad| Step::bad(value), |(_, rest)| Step::ok(rest))
}

/// The field layout of a numeric zone element.
#[derive(Debug, Clone, Copy)]
enum OffsetForm {
    /// `-0700`, `Z0700`.
    Plain,
    /// `-07`, `Z07`.
    Short,
    /// `-07:00`, `Z07:00`.
    Colon,
    /// `-070000`, `Z070000`.
    Seconds,
    /// `-07:00:00`, `Z07:00:00`.
    ColonSeconds,
}

/// Sets Go's `rangeErrString` when `out_of_range`.
const fn with_range<'v>(mut step: Step<'v>, out_of_range: bool, name: &'static str) -> Step<'v> {
    if out_of_range {
        step.range = name;
    }
    step
}

/// Go: `parse`'s `stdWeekDay` and `stdLongWeekDay` cases.
fn weekday_step<'v>(table: &[&str], value: &'v [u8]) -> Step<'v> {
    lookup(table, value).map_or_else(|Bad| Step::bad(value), |(_, rest)| Step::ok(rest))
}

/// Removes one leading space (Go: the `_2` and `__2` padding).
fn strip_space(value: &[u8]) -> &[u8] {
    value.strip_prefix(b" ").unwrap_or(value)
}

/// Go: `fracSecond9`'s test for a fraction: a separator and a digit.
fn starts_fraction(value: &[u8]) -> bool {
    value.first().is_some_and(|&c| comma_or_period(c)) && is_digit(value, 1)
}

/// The number of digits at the start of `s`.
fn digit_run(s: &[u8]) -> usize {
    s.iter().take_while(|c| c.is_ascii_digit()).count()
}

impl Fields {
    /// Go: one iteration of `parse`'s element switch; `layout` is the rest of the layout.
    fn step<'v>(&mut self, std: Std, layout: &[u8], value: &'v [u8]) -> Step<'v> {
        match std {
            Std::Year => self.year_step(value),
            Std::LongYear => self.long_year_step(value),
            Std::Month => self.month_name_step(&SHORT_MONTH_NAMES, value),
            Std::LongMonth => self.month_name_step(&LONG_MONTH_NAMES, value),
            Std::NumMonth => self.num_month_step(value, false),
            Std::ZeroMonth => self.num_month_step(value, true),
            Std::WeekDay => weekday_step(&SHORT_DAY_NAMES, value),
            Std::LongWeekDay => weekday_step(&LONG_DAY_NAMES, value),
            Std::Day => num_step(&mut self.day, value, getnum(value, false)),
            Std::UnderDay => {
                let value = strip_space(value);
                num_step(&mut self.day, value, getnum(value, false))
            }
            Std::ZeroDay => num_step(&mut self.day, value, getnum(value, true)),
            Std::UnderYearDay => {
                let value = strip_space(strip_space(value));
                num_step(&mut self.yday, value, getnum3(value, false))
            }
            Std::ZeroYearDay => num_step(&mut self.yday, value, getnum3(value, true)),
            Std::Hour => {
                let step = num_step(&mut self.hour, value, getnum(value, false));
                with_range(step, !(0..24).contains(&self.hour), "hour")
            }
            Std::Hour12 | Std::ZeroHour12 => {
                let step = num_step(&mut self.hour, value, getnum(value, std == Std::ZeroHour12));
                with_range(step, !(0..=12).contains(&self.hour), "hour")
            }
            Std::Minute | Std::ZeroMinute => {
                let step = num_step(&mut self.min, value, getnum(value, std == Std::ZeroMinute));
                with_range(step, !(0..60).contains(&self.min), "minute")
            }
            Std::Second | Std::ZeroSecond => {
                self.second_step(layout, value, std == Std::ZeroSecond)
            }
            Std::Pm => self.meridiem_step(value, b"PM", b"AM"),
            Std::LowerPm => self.meridiem_step(value, b"pm", b"am"),
            Std::Tz => self.zone_name_step(value),
            Std::Iso8601Tz => self.offset_step(value, OffsetForm::Plain, true),
            Std::Iso8601SecondsTz => self.offset_step(value, OffsetForm::Seconds, true),
            Std::Iso8601ShortTz => self.offset_step(value, OffsetForm::Short, true),
            Std::Iso8601ColonTz => self.offset_step(value, OffsetForm::Colon, true),
            Std::Iso8601ColonSecondsTz => self.offset_step(value, OffsetForm::ColonSeconds, true),
            Std::NumTz => self.offset_step(value, OffsetForm::Plain, false),
            Std::NumSecondsTz => self.offset_step(value, OffsetForm::Seconds, false),
            Std::NumShortTz => self.offset_step(value, OffsetForm::Short, false),
            Std::NumColonTz => self.offset_step(value, OffsetForm::Colon, false),
            Std::NumColonSecondsTz => self.offset_step(value, OffsetForm::ColonSeconds, false),
            Std::FracSecond0(frac) => {
                let ndigit = 1 + frac.digits;
                split(value, ndigit).map_or_else(
                    || Step::bad(value),
                    |(_, rest)| self.nanoseconds(value, ndigit, rest),
                )
            }
            Std::FracSecond9(_) => {
                if value.len() < 2 || !starts_fraction(value) {
                    return Step::ok(value);
                }
                let n = 1 + digit_run(value.get(1..).unwrap_or_default());
                self.nanoseconds(value, n, value.get(n..).unwrap_or_default())
            }
        }
    }

    /// Go: `parse`'s `stdYear` case.
    fn year_step<'v>(&mut self, value: &'v [u8]) -> Step<'v> {
        let Some((p, rest)) = split(value, 2) else {
            return Step::bad(value);
        };
        let Ok(year) = atoi(p) else {
            self.year = 0;
            return Step::bad(rest);
        };
        self.year = year + if year >= 69 { 1900 } else { 2000 };
        Step::ok(rest)
    }

    /// Go: `parse`'s `stdLongYear` case.
    fn long_year_step<'v>(&mut self, value: &'v [u8]) -> Step<'v> {
        match split(value, 4) {
            Some((p, rest)) if is_digit(value, 0) => {
                num_step(&mut self.year, rest, atoi(p).map(|year| (year, rest)))
            }
            Some(_) | None => Step::bad(value),
        }
    }

    /// Go: `parse`'s `stdMonth` and `stdLongMonth` cases.
    fn month_name_step<'v>(&mut self, table: &[&str], value: &'v [u8]) -> Step<'v> {
        let Ok((index, rest)) = lookup(table, value) else {
            self.month = 0;
            return Step::bad(value);
        };
        self.month = index + 1;
        Step::ok(rest)
    }

    /// Go: `parse`'s `stdNumMonth` and `stdZeroMonth` cases.
    fn num_month_step<'v>(&mut self, value: &'v [u8], fixed: bool) -> Step<'v> {
        let step = num_step(&mut self.month, value, getnum(value, fixed));
        let out_of_range = !step.bad && !(1..=12).contains(&self.month);
        with_range(step, out_of_range, "month")
    }

    /// Go: `parse`'s `stdSecond` and `stdZeroSecond` cases, including a fraction directly
    /// after the seconds unless the layout's next element is a fraction.
    fn second_step<'v>(&mut self, layout: &[u8], value: &'v [u8], fixed: bool) -> Step<'v> {
        let step = num_step(&mut self.sec, value, getnum(value, fixed));
        if step.bad {
            return step;
        }
        if !(0..60).contains(&self.sec) {
            return with_range(step, true, "second");
        }
        let after = step.value;
        if after.len() < 2
            || !starts_fraction(after)
            || next_std_chunk(layout).std.is_some_and(Std::is_frac)
        {
            return Step::ok(after);
        }
        let n = 2 + digit_run(after.get(2..).unwrap_or_default());
        self.nanoseconds(after, n, after.get(n..).unwrap_or_default())
    }

    /// Assigns Go's `nsec, rangeErrString, err = parseNanoseconds(value, n)`.
    fn nanoseconds<'v>(&mut self, value: &[u8], n: usize, rest: &'v [u8]) -> Step<'v> {
        let (ns, range) = parse_nanoseconds(value, n);
        self.nsec = ns.as_ref().map_or(0, |&ns| ns);
        Step {
            value: rest,
            bad: ns.is_err(),
            range,
        }
    }

    /// Go: `parse`'s `stdPM` and `stdpm` cases.
    fn meridiem_step<'v>(&mut self, value: &'v [u8], pm: &[u8], am: &[u8]) -> Step<'v> {
        let Some((p, rest)) = split(value, 2) else {
            return Step::bad(value);
        };
        if p == pm {
            self.pm_set = true;
        } else if p == am {
            self.am_set = true;
        } else {
            return Step::bad(rest);
        }
        Step::ok(rest)
    }

    /// Go: `parse`'s `stdTZ` case.
    fn zone_name_step<'v>(&mut self, value: &'v [u8]) -> Step<'v> {
        if let Some(rest) = value.strip_prefix(b"UTC") {
            self.z_utc = true;
            return Step::ok(rest);
        }
        let Some(n) = parse_time_zone(value) else {
            return Step::bad(value);
        };
        let (name, rest) = value.split_at(n.min(value.len()));
        self.zone_name = name.to_vec();
        Step::ok(rest)
    }

    /// Go: `parse`'s numeric zone cases; `iso` for the `Z07...` elements, which accept `Z`.
    fn offset_step<'v>(&mut self, value: &'v [u8], form: OffsetForm, iso: bool) -> Step<'v> {
        if iso && let Some(rest) = value.strip_prefix(b"Z") {
            self.z_utc = true;
            return Step::ok(rest);
        }
        let at = |i: usize| value.get(i).copied().unwrap_or(0);
        let part = |a: usize, b: usize| value.get(a..b).unwrap_or_default();
        let zeros: &[u8] = b"00";
        let (len, hour, min, seconds) = match form {
            OffsetForm::Colon if value.len() >= 6 && at(3) == b':' => {
                (6, part(1, 3), part(4, 6), zeros)
            }
            OffsetForm::Short if value.len() >= 3 => (3, part(1, 3), zeros, zeros),
            OffsetForm::ColonSeconds if value.len() >= 9 && at(3) == b':' && at(6) == b':' => {
                (9, part(1, 3), part(4, 6), part(7, 9))
            }
            OffsetForm::Seconds if value.len() >= 7 => (7, part(1, 3), part(3, 5), part(5, 7)),
            OffsetForm::Plain if value.len() >= 5 => (5, part(1, 3), part(3, 5), zeros),
            OffsetForm::Plain
            | OffsetForm::Short
            | OffsetForm::Colon
            | OffsetForm::Seconds
            | OffsetForm::ColonSeconds => return Step::bad(value),
        };
        let (mut hr, mut mm, mut ss) = (0, 0, 0);
        let mut bad = num_step(&mut hr, hour, getnum(hour, true)).bad
            || num_step(&mut mm, min, getnum(min, true)).bad
            || num_step(&mut ss, seconds, getnum(seconds, true)).bad;
        // Go checks hour, minute and second in turn and keeps the last failure.
        let range = if ss > 60 {
            "time zone offset second"
        } else if mm > 60 {
            "time zone offset minute"
        } else if hr > 24 {
            "time zone offset hour"
        } else {
            ""
        };
        self.zone_offset = (hr * 60 + mm) * 60 + ss;
        match at(0) {
            b'+' => {}
            b'-' => self.zone_offset = -self.zone_offset,
            _ => bad = true,
        }
        Step {
            value: value.get(len..).unwrap_or_default(),
            bad,
            range,
        }
    }

    /// Go: the end of `parse`: the hour adjustment, day-of-year and day checks and the zone.
    fn finish(mut self, layout: &[u8], value: &[u8], rest: &[u8]) -> Result<Time, ParseError> {
        let error = |message: &str| ParseError::new(layout, value, b"", rest, message.to_owned());
        if self.pm_set && self.hour < 12 {
            self.hour += 12;
        } else if self.am_set && self.hour == 12 {
            self.hour = 0;
        }
        if self.yday >= 0 {
            let (mut d, mut m) = (0, 0);
            if is_leap(self.year) {
                if self.yday == 31 + 29 {
                    m = 2;
                    d = 29;
                } else if self.yday > 31 + 29 {
                    self.yday -= 1;
                }
            }
            if self.yday < 1 || self.yday > 365 {
                return Err(error(": day-of-year out of range"));
            }
            if m == 0 {
                m = (self.yday - 1) / 31 + 1;
                if days_before(m + 1) < self.yday {
                    m += 1;
                }
                d = self.yday - days_before(m);
            }
            if self.month >= 0 && self.month != m {
                return Err(error(": day-of-year does not match month"));
            }
            self.month = m;
            if self.day >= 0 && self.day != d {
                return Err(error(": day-of-year does not match day"));
            }
            self.day = d;
        } else {
            if self.month < 0 {
                self.month = 1;
            }
            if self.day < 0 {
                self.day = 1;
            }
        }
        if self.day < 1 || self.day > days_in(self.month, self.year) {
            return Err(error(": day out of range"));
        }
        Ok(self.resolve_zone())
    }

    /// Go: the zone resolution at the end of `parse`, with `Local` and the default location
    /// both UTC.
    fn resolve_zone(self) -> Time {
        let fields = DateFields {
            year: self.year,
            month: self.month,
            day: self.day,
            hour: self.hour,
            minute: self.min,
            second: self.sec,
            nanosecond: self.nsec,
        };
        let mut t = Time::date(&fields, Location::Utc);
        if self.z_utc {
            return t;
        }
        let name = String::from_utf8_lossy(&self.zone_name).into_owned();
        if self.zone_offset != -1 {
            t.add_sec(self.zone_offset.wrapping_neg());
            // Go: Local (UTC) has offset 0 and the name "UTC", which no parsed name equals.
            if self.zone_offset != 0 || !name.is_empty() {
                t.loc = Location::fixed_zone(&name, self.zone_offset);
            }
            return t;
        }
        if !name.is_empty() {
            // Go: Local (UTC) has no zones, so lookupName fails and the offset is 0 except for
            // GMT+h, whose wall clock is still read as UTC.
            let offset = match self.zone_name.strip_prefix(b"GMT") {
                Some(hours) if !hours.is_empty() => atoi(hours).unwrap_or(0).wrapping_mul(3600),
                Some(_) | None => 0,
            };
            t.loc = Location::fixed_zone(&name, offset);
        }
        t
    }
}

/// Go: `parse(layout, value, UTC, Local)` with `Local` UTC.
fn parse_layout(layout: &[u8], value: &[u8]) -> Result<Time, ParseError> {
    let (alayout, avalue) = (layout, value);
    let mut fields = Fields::default();
    let (mut layout, mut value) = (layout, value);
    loop {
        let chunk = next_std_chunk(layout);
        let std_len = layout.len() - chunk.prefix.len() - chunk.suffix.len();
        let stdstr = layout
            .get(chunk.prefix.len()..chunk.prefix.len() + std_len)
            .unwrap_or_default();
        value = skip(value, chunk.prefix)
            .map_err(|at| ParseError::new(alayout, avalue, chunk.prefix, at, String::new()))?;
        let Some(std) = chunk.std else {
            if !value.is_empty() {
                let message = format!(": extra text: {}", quote(value));
                return Err(ParseError::new(alayout, avalue, b"", value, message));
            }
            return fields.finish(alayout, avalue, value);
        };
        layout = chunk.suffix;
        let hold = value;
        let step = fields.step(std, layout, value);
        if !step.range.is_empty() {
            let message = format!(": {} out of range", step.range);
            return Err(ParseError::new(
                alayout, avalue, stdstr, step.value, message,
            ));
        }
        if step.bad {
            return Err(ParseError::new(
                alayout,
                avalue,
                stdstr,
                hold,
                String::new(),
            ));
        }
        value = step.value;
    }
}

/// Go: `parseRFC3339`'s `parseUint`.
fn parse_uint(s: &[u8], min: i64, max: i64, ok: &mut bool) -> i64 {
    let mut x: i64 = 0;
    for &c in s {
        if !c.is_ascii_digit() {
            *ok = false;
            return min;
        }
        x = x * 10 + i64::from(c - b'0');
    }
    if x < min || max < x {
        *ok = false;
        return min;
    }
    x
}

/// Go: `parseRFC3339` with `Local` UTC: the fast path, `None` where Go falls back to `parse`.
fn parse_rfc3339(s: &[u8]) -> Option<Time> {
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    let part = |a: usize, b: usize| s.get(a..b).unwrap_or_default();
    if s.len() < "2006-01-02T15:04:05".len() {
        return None;
    }
    let mut ok = true;
    let year = parse_uint(part(0, 4), 0, 9999, &mut ok);
    let month = parse_uint(part(5, 7), 1, 12, &mut ok);
    let day = parse_uint(part(8, 10), 1, days_in(month, year), &mut ok);
    let hour = parse_uint(part(11, 13), 0, 23, &mut ok);
    let minute = parse_uint(part(14, 16), 0, 59, &mut ok);
    let second = parse_uint(part(17, 19), 0, 59, &mut ok);
    if !ok || at(4) != b'-' || at(7) != b'-' || at(10) != b'T' || at(13) != b':' || at(16) != b':' {
        return None;
    }
    let mut s = s.get(19..).unwrap_or_default();
    let mut nanosecond = 0;
    if s.len() >= 2 && s.first() == Some(&b'.') && is_digit(s, 1) {
        let n = 2 + s
            .get(2..)
            .unwrap_or_default()
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        nanosecond = parse_nanoseconds(s, n).0.unwrap_or(0);
        s = s.get(n..).unwrap_or_default();
    }
    let fields = DateFields {
        year,
        month,
        day,
        hour,
        minute,
        second,
        nanosecond,
    };
    let mut t = Time::date(&fields, Location::Utc);
    if s != b"Z" {
        if s.len() != "-07:00".len() {
            return None;
        }
        let hr = parse_uint(s.get(1..3).unwrap_or_default(), 0, 23, &mut ok);
        let mm = parse_uint(s.get(4..6).unwrap_or_default(), 0, 59, &mut ok);
        let sign = s.first().copied().unwrap_or(0);
        if !ok || !(sign == b'-' || sign == b'+') || s.get(3) != Some(&b':') {
            return None;
        }
        let mut zone_offset = (hr * 60 + mm) * 60;
        if sign == b'-' {
            zone_offset = -zone_offset;
        }
        t.add_sec(-zone_offset);
        if zone_offset != 0 {
            t.loc = Location::fixed_zone("", zone_offset);
        }
    }
    Some(t)
}

/// Go: `parseStrictRFC3339`, whose strict checks Go 1.26 leaves disabled.
pub(super) fn parse_strict_rfc3339(b: &[u8]) -> Result<Time, ParseError> {
    parse_rfc3339(b).map_or_else(|| parse_bytes(RFC3339.as_bytes(), b), Ok)
}

/// Go: `time.Parse` over Go strings.
pub(super) fn parse_bytes(layout: &[u8], value: &[u8]) -> Result<Time, ParseError> {
    if (layout == RFC3339.as_bytes() || layout == RFC3339_NANO.as_bytes())
        && let Some(t) = parse_rfc3339(value)
    {
        return Ok(t);
    }
    parse_layout(layout, value)
}

/// Go: `time.Parse(layout, value)`, where Go's `Local` is UTC.
///
/// # Errors
///
/// [`ParseError`] with Go's text, such as
/// `parsing time "10:61" as "15:04": cannot parse "61" as "04"`... or the range form
/// `parsing time "10:61": minute out of range`.
pub fn parse(layout: &str, value: &str) -> Result<Time, ParseError> {
    parse_bytes(layout.as_bytes(), value.as_bytes())
}
