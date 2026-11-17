//! Go: `Time.Format`, `Time.AppendFormat` and the layout scanner `nextStdChunk`.

use super::{AbsDate, RFC3339, RFC3339_NANO, Time};
use crate::utf8::lossy_string;

/// Go: `longDayNames`.
pub(super) const LONG_DAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Go: `shortDayNames`.
pub(super) const SHORT_DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Go: `shortMonthNames`.
pub(super) const SHORT_MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Go: `longMonthNames`.
pub(super) const LONG_MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A fractional-second element: its digit count (Go keeps the low 12 bits) and separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Frac {
    pub(super) digits: usize,
    pub(super) comma: bool,
}

/// Go: the `std*` layout element codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Std {
    /// "January"
    LongMonth,
    /// "Jan"
    Month,
    /// "1"
    NumMonth,
    /// "01"
    ZeroMonth,
    /// "Monday"
    LongWeekDay,
    /// "Mon"
    WeekDay,
    /// "2"
    Day,
    /// "_2"
    UnderDay,
    /// "02"
    ZeroDay,
    /// "__2"
    UnderYearDay,
    /// "002"
    ZeroYearDay,
    /// "15"
    Hour,
    /// "3"
    Hour12,
    /// "03"
    ZeroHour12,
    /// "4"
    Minute,
    /// "04"
    ZeroMinute,
    /// "5"
    Second,
    /// "05"
    ZeroSecond,
    /// "2006"
    LongYear,
    /// "06"
    Year,
    /// "PM"
    Pm,
    /// "pm"
    LowerPm,
    /// "MST"
    Tz,
    /// "Z0700"
    Iso8601Tz,
    /// "Z070000"
    Iso8601SecondsTz,
    /// "Z07"
    Iso8601ShortTz,
    /// "Z07:00"
    Iso8601ColonTz,
    /// "Z07:00:00"
    Iso8601ColonSecondsTz,
    /// "-0700"
    NumTz,
    /// "-070000"
    NumSecondsTz,
    /// "-07"
    NumShortTz,
    /// "-07:00"
    NumColonTz,
    /// "-07:00:00"
    NumColonSecondsTz,
    /// ".0", ".00", ...: trailing zeros included.
    FracSecond0(Frac),
    /// ".9", ".99", ...: trailing zeros omitted.
    FracSecond9(Frac),
}

impl Std {
    /// The `Z07...` elements, which print `Z` for a zero offset.
    pub(super) const fn is_iso8601(self) -> bool {
        matches!(
            self,
            Self::Iso8601Tz
                | Self::Iso8601SecondsTz
                | Self::Iso8601ShortTz
                | Self::Iso8601ColonTz
                | Self::Iso8601ColonSecondsTz
        )
    }

    pub(super) const fn is_frac(self) -> bool {
        matches!(self, Self::FracSecond0(_) | Self::FracSecond9(_))
    }
}

/// One step of Go's `nextStdChunk`: the literal text before the element, the element (`None`
/// at the end of the layout) and the rest of the layout.
pub(super) struct Chunk<'a> {
    pub(super) prefix: &'a [u8],
    pub(super) std: Option<Std>,
    pub(super) suffix: &'a [u8],
}

fn has(layout: &[u8], i: usize, pattern: &[u8]) -> bool {
    layout.get(i..i + pattern.len()) == Some(pattern)
}

fn byte_at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

/// Go: `isDigit`.
pub(super) fn is_digit(s: &[u8], i: usize) -> bool {
    s.get(i).is_some_and(u8::is_ascii_digit)
}

/// Go: `startsWithLowerCase`.
fn starts_with_lower_case(s: &[u8]) -> bool {
    s.first().is_some_and(u8::is_ascii_lowercase)
}

/// The first matching zone element at `i` among `(pattern, element)` pairs, in Go's order.
fn zone_at(layout: &[u8], i: usize, table: &[(&[u8], Std)]) -> Option<(usize, Std, usize)> {
    table
        .iter()
        .find(|(pattern, _)| has(layout, i, pattern))
        .map(|&(pattern, std)| (i, std, i + pattern.len()))
}

/// Go: the elements starting with a letter at `i`: `Jan`, `January`, `Mon`, `Monday`, `MST`,
/// `PM` and `pm`.
fn word_at(layout: &[u8], i: usize, c: u8) -> Option<(usize, Std, usize)> {
    let rest = |n: usize| layout.get(i + n..).unwrap_or_default();
    match c {
        b'J' if has(layout, i, b"Jan") => {
            if has(layout, i, b"January") {
                return Some((i, Std::LongMonth, i + 7));
            }
            (!starts_with_lower_case(rest(3))).then_some((i, Std::Month, i + 3))
        }
        b'M' => {
            if has(layout, i, b"Mon") {
                if has(layout, i, b"Monday") {
                    return Some((i, Std::LongWeekDay, i + 6));
                }
                if !starts_with_lower_case(rest(3)) {
                    return Some((i, Std::WeekDay, i + 3));
                }
            }
            has(layout, i, b"MST").then_some((i, Std::Tz, i + 3))
        }
        b'P' => (byte_at(layout, i + 1) == b'M').then_some((i, Std::Pm, i + 2)),
        b'p' => (byte_at(layout, i + 1) == b'm').then_some((i, Std::LowerPm, i + 2)),
        _ => None,
    }
}

/// Go: the elements starting with a digit or `_` at `i`.
fn number_at(layout: &[u8], i: usize, c: u8) -> Option<(usize, Std, usize)> {
    const STD0X: [Std; 6] = [
        Std::ZeroMonth,
        Std::ZeroDay,
        Std::ZeroHour12,
        Std::ZeroMinute,
        Std::ZeroSecond,
        Std::Year,
    ];
    let next = byte_at(layout, i + 1);
    match c {
        b'0' => {
            if (b'1'..=b'6').contains(&next) {
                return STD0X
                    .get(usize::from(next - b'1'))
                    .map(|&std| (i, std, i + 2));
            }
            has(layout, i, b"002").then_some((i, Std::ZeroYearDay, i + 3))
        }
        b'1' if next == b'5' => Some((i, Std::Hour, i + 2)),
        b'1' => Some((i, Std::NumMonth, i + 1)),
        b'2' if has(layout, i, b"2006") => Some((i, Std::LongYear, i + 4)),
        b'2' => Some((i, Std::Day, i + 1)),
        b'_' => {
            if next == b'2' {
                if has(layout, i + 1, b"2006") {
                    return Some((i + 1, Std::LongYear, i + 5));
                }
                return Some((i, Std::UnderDay, i + 2));
            }
            has(layout, i, b"__2").then_some((i, Std::UnderYearDay, i + 3))
        }
        b'3' => Some((i, Std::Hour12, i + 1)),
        b'4' => Some((i, Std::Minute, i + 1)),
        b'5' => Some((i, Std::Second, i + 1)),
        _ => None,
    }
}

/// Go: `.000`, `,000`, `.999` and `,999` at `i`, when no digit follows the run.
fn frac_at(layout: &[u8], i: usize, c: u8) -> Option<(usize, Std, usize)> {
    let ch = byte_at(layout, i + 1);
    if ch != b'0' && ch != b'9' {
        return None;
    }
    let run = layout
        .get(i + 1..)
        .unwrap_or_default()
        .iter()
        .take_while(|&&b| b == ch)
        .count();
    let j = i + 1 + run;
    if is_digit(layout, j) {
        return None;
    }
    let frac = Frac {
        digits: run & 0xFFF,
        comma: c == b',',
    };
    let std = if ch == b'9' {
        Std::FracSecond9(frac)
    } else {
        Std::FracSecond0(frac)
    };
    Some((i, std, j))
}

/// Go: `nextStdChunk`.
pub(super) fn next_std_chunk(layout: &[u8]) -> Chunk<'_> {
    const DASH: [(&[u8], Std); 5] = [
        (b"-070000", Std::NumSecondsTz),
        (b"-07:00:00", Std::NumColonSecondsTz),
        (b"-0700", Std::NumTz),
        (b"-07:00", Std::NumColonTz),
        (b"-07", Std::NumShortTz),
    ];
    const ZED: [(&[u8], Std); 5] = [
        (b"Z070000", Std::Iso8601SecondsTz),
        (b"Z07:00:00", Std::Iso8601ColonSecondsTz),
        (b"Z0700", Std::Iso8601Tz),
        (b"Z07:00", Std::Iso8601ColonTz),
        (b"Z07", Std::Iso8601ShortTz),
    ];
    for (i, &c) in layout.iter().enumerate() {
        let found = match c {
            b'J' | b'M' | b'P' | b'p' => word_at(layout, i, c),
            b'0'..=b'5' | b'_' => number_at(layout, i, c),
            b'-' => zone_at(layout, i, &DASH),
            b'Z' => zone_at(layout, i, &ZED),
            b'.' | b',' => frac_at(layout, i, c),
            _ => None,
        };
        if let Some((end, std, start)) = found {
            return Chunk {
                prefix: layout.get(..end).unwrap_or_default(),
                std: Some(std),
                suffix: layout.get(start..).unwrap_or_default(),
            };
        }
    }
    Chunk {
        prefix: layout,
        std: None,
        suffix: &[],
    }
}

/// Go: `appendInt`: `x` in decimal, zero-padded to `width` digits after any sign.
pub(super) fn append_int(b: &mut Vec<u8>, x: i64, width: usize) {
    if x < 0 {
        b.push(b'-');
    }
    let digits = x.unsigned_abs().to_string();
    b.resize(b.len() + width.saturating_sub(digits.len()), b'0');
    b.extend_from_slice(digits.as_bytes());
}

/// Go: `appendNano`.
fn append_nano(b: &mut Vec<u8>, nanosec: u32, frac: Frac, trim: bool) {
    if trim && (frac.digits == 0 || nanosec == 0) {
        return;
    }
    let dot = if frac.comma { b',' } else { b'.' };
    b.push(dot);
    let start = b.len();
    append_int(b, i64::from(nanosec), 9);
    if frac.digits < 9 {
        b.truncate(start + frac.digits);
    }
    if trim {
        let keep = b.iter().rposition(|&c| c != b'0').map_or(0, |i| i + 1);
        b.truncate(keep);
        if b.last() == Some(&dot) {
            b.truncate(b.len() - 1);
        }
    }
}

fn name_of(table: &[&'static str], index: i64) -> &'static str {
    usize::try_from(index)
        .ok()
        .and_then(|i| table.get(i))
        .copied()
        .unwrap_or_default()
}

/// The calendar fields of a time in its zone.
struct Wall<'a> {
    name: &'a str,
    offset: i64,
    year: i64,
    month: i64,
    day: i64,
    yday: i64,
    hour: i64,
    min: i64,
    sec: i64,
    weekday: i64,
    nsec: u32,
}

impl<'a> Wall<'a> {
    fn of(t: &'a Time) -> Self {
        let (name, offset, abs) = t.locabs();
        let date = AbsDate::split(abs.days());
        let (year, month, day) = date.date();
        let (hour, min, sec) = abs.clock();
        Self {
            name,
            offset,
            year,
            month,
            day,
            yday: date.yday(),
            hour,
            min,
            sec,
            weekday: abs.weekday(),
            nsec: t.nsec,
        }
    }

    /// Go: `appendFormat`'s cases for the numeric zone elements.
    fn append_numeric_zone(&self, b: &mut Vec<u8>, std: Std) {
        if self.offset == 0 && std.is_iso8601() {
            b.push(b'Z');
            return;
        }
        let mut zone = self.offset / 60;
        let mut abs_offset = self.offset;
        if zone < 0 {
            b.push(b'-');
            zone = -zone;
            abs_offset = -abs_offset;
        } else {
            b.push(b'+');
        }
        append_int(b, zone / 60, 2);
        let colon = matches!(
            std,
            Std::Iso8601ColonTz
                | Std::NumColonTz
                | Std::Iso8601ColonSecondsTz
                | Std::NumColonSecondsTz
        );
        if colon {
            b.push(b':');
        }
        if !matches!(std, Std::NumShortTz | Std::Iso8601ShortTz) {
            append_int(b, zone % 60, 2);
        }
        if matches!(
            std,
            Std::Iso8601SecondsTz
                | Std::NumSecondsTz
                | Std::NumColonSecondsTz
                | Std::Iso8601ColonSecondsTz
        ) {
            if matches!(std, Std::NumColonSecondsTz | Std::Iso8601ColonSecondsTz) {
                b.push(b':');
            }
            append_int(b, abs_offset % 60, 2);
        }
    }

    /// Go: `appendFormat`'s `stdTZ` case.
    fn append_zone_name(&self, b: &mut Vec<u8>) {
        if !self.name.is_empty() {
            b.extend_from_slice(self.name.as_bytes());
            return;
        }
        let mut zone = self.offset / 60;
        if zone < 0 {
            b.push(b'-');
            zone = -zone;
        } else {
            b.push(b'+');
        }
        append_int(b, zone / 60, 2);
        append_int(b, zone % 60, 2);
    }

    /// Go: one case of `appendFormat`'s element switch.
    fn append(&self, b: &mut Vec<u8>, std: Std) {
        let hour12 = if self.hour % 12 == 0 {
            12
        } else {
            self.hour % 12
        };
        let month = name_of(&LONG_MONTH_NAMES, self.month - 1);
        let weekday = name_of(&LONG_DAY_NAMES, self.weekday);
        match std {
            Std::Year => append_int(b, self.year.wrapping_abs() % 100, 2),
            Std::LongYear => append_int(b, self.year, 4),
            Std::Month => b.extend_from_slice(month.as_bytes().get(..3).unwrap_or_default()),
            Std::LongMonth => b.extend_from_slice(month.as_bytes()),
            Std::NumMonth => append_int(b, self.month, 0),
            Std::ZeroMonth => append_int(b, self.month, 2),
            Std::WeekDay => b.extend_from_slice(weekday.as_bytes().get(..3).unwrap_or_default()),
            Std::LongWeekDay => b.extend_from_slice(weekday.as_bytes()),
            Std::Day => append_int(b, self.day, 0),
            Std::UnderDay => {
                if self.day < 10 {
                    b.push(b' ');
                }
                append_int(b, self.day, 0);
            }
            Std::ZeroDay => append_int(b, self.day, 2),
            Std::UnderYearDay => {
                if self.yday < 100 {
                    b.push(b' ');
                    if self.yday < 10 {
                        b.push(b' ');
                    }
                }
                append_int(b, self.yday, 0);
            }
            Std::ZeroYearDay => append_int(b, self.yday, 3),
            Std::Hour => append_int(b, self.hour, 2),
            Std::Hour12 => append_int(b, hour12, 0),
            Std::ZeroHour12 => append_int(b, hour12, 2),
            Std::Minute => append_int(b, self.min, 0),
            Std::ZeroMinute => append_int(b, self.min, 2),
            Std::Second => append_int(b, self.sec, 0),
            Std::ZeroSecond => append_int(b, self.sec, 2),
            Std::Pm => b.extend_from_slice(if self.hour >= 12 { b"PM" } else { b"AM" }),
            Std::LowerPm => b.extend_from_slice(if self.hour >= 12 { b"pm" } else { b"am" }),
            Std::Tz => self.append_zone_name(b),
            Std::FracSecond0(frac) => append_nano(b, self.nsec, frac, false),
            Std::FracSecond9(frac) => append_nano(b, self.nsec, frac, true),
            Std::Iso8601Tz
            | Std::Iso8601SecondsTz
            | Std::Iso8601ShortTz
            | Std::Iso8601ColonTz
            | Std::Iso8601ColonSecondsTz
            | Std::NumTz
            | Std::NumSecondsTz
            | Std::NumShortTz
            | Std::NumColonTz
            | Std::NumColonSecondsTz => self.append_numeric_zone(b, std),
        }
    }

    /// Go: `appendFormatRFC3339`.
    fn append_rfc3339(&self, b: &mut Vec<u8>, nanos: bool) {
        append_int(b, self.year, 4);
        b.push(b'-');
        append_int(b, self.month, 2);
        b.push(b'-');
        append_int(b, self.day, 2);
        b.push(b'T');
        append_int(b, self.hour, 2);
        b.push(b':');
        append_int(b, self.min, 2);
        b.push(b':');
        append_int(b, self.sec, 2);
        if nanos {
            let frac = Frac {
                digits: 9,
                comma: false,
            };
            append_nano(b, self.nsec, frac, true);
        }
        if self.offset == 0 {
            b.push(b'Z');
            return;
        }
        let mut zone = self.offset / 60;
        if zone < 0 {
            b.push(b'-');
            zone = -zone;
        } else {
            b.push(b'+');
        }
        append_int(b, zone / 60, 2);
        b.push(b':');
        append_int(b, zone % 60, 2);
    }
}

/// Go: `Time.AppendFormat`.
pub(super) fn append_format(t: &Time, mut b: Vec<u8>, layout: &str) -> Vec<u8> {
    let wall = Wall::of(t);
    if layout == RFC3339 || layout == RFC3339_NANO {
        wall.append_rfc3339(&mut b, layout == RFC3339_NANO);
        return b;
    }
    let mut rest = layout.as_bytes();
    while !rest.is_empty() {
        let chunk = next_std_chunk(rest);
        b.extend_from_slice(chunk.prefix);
        let Some(std) = chunk.std else {
            break;
        };
        rest = chunk.suffix;
        wall.append(&mut b, std);
    }
    b
}

/// Go: `Time.appendStrictRFC3339`: the RFC 3339 form with nanoseconds, or the error text.
pub(super) fn strict_rfc3339(t: &Time) -> Result<String, &'static str> {
    let mut b = Vec::new();
    Wall::of(t).append_rfc3339(&mut b, true);
    if byte_at(&b, 4) != b'-' {
        return Err("year outside of range [0,9999]");
    }
    if b.last() != Some(&b'Z') {
        let n = b.len();
        let c = byte_at(&b, n.wrapping_sub(6));
        let tens = byte_at(&b, n.wrapping_sub(5)).wrapping_sub(b'0');
        let hour = tens
            .wrapping_mul(10)
            .wrapping_add(byte_at(&b, n.wrapping_sub(4)).wrapping_sub(b'0'));
        if c.is_ascii_digit() || hour >= 24 {
            return Err("timezone hour outside of range [0,23]");
        }
    }
    Ok(lossy_string(&b))
}
