//! Go `time`: [`Time`] with `Format`, `Parse`, `String`, `MarshalJSON`/`UnmarshalJSON` and the
//! layout constants, and [`Duration`] with `String`, `Seconds` and `ParseDuration`.
//!
//! Go: `time/time.go`, `time/format.go`, `time/format_rfc3339.go`.
//!
//! A [`Time`] holds Go's representation (seconds since January 1, year 1 UTC, plus nanoseconds)
//! and a [`Location`]. Calendar arithmetic is Go's own absolute-day algorithm, wrapping on
//! overflow as Go's integers do, so dates match Go across the whole `int64` range. Every layout
//! element formats and parses as in Go, quirks included: `_2`, `__2`, `002`, `.000`/`,999`,
//! `Z07:00:00`, `MST`, the fraction accepted after seconds, two-digit years, the `GMT+3`
//! abbreviation (labelled `GMT+3` while the clock is read as UTC), the offset `-00:00:01` (which
//! Go reads as no offset), the strict `MarshalJSON` year and zone checks, and [`ParseError`]
//! texts such as `parsing time "2024-13-01": month out of range`.
//!
//! Locations are UTC and fixed zones. Go's `Local` is UTC in Prebid's container (no `TZ`
//! variable and no `/etc/localtime`, so Go names it `UTC` and gives it no zones), and this port
//! treats `Local` as UTC: [`Time::from_unix`] and [`Time::now`] return UTC times, [`Time::local`]
//! is [`Time::utc`], and [`parse`] returns UTC where Go returns `Local` (a zero offset with no
//! zone name). Monotonic clock readings are not modelled, so [`Time`]'s `Display` never prints
//! Go's ` m=+...` suffix. Not ported: `ParseInLocation`, `LoadLocation` and zone databases,
//! `Truncate`/`Round`, ISO weeks and the binary encodings.
//!
//! ```
//! use pbs_gocompat::time::{self, DateFields, Duration, Location, Time};
//!
//! let fields = DateFields {
//!     year: 2024,
//!     month: 2,
//!     day: 29,
//!     hour: 13,
//!     minute: 4,
//!     second: 5,
//!     nanosecond: 120_000_000,
//! };
//! let t = Time::date(&fields, Location::Utc);
//! assert_eq!(t.format(time::RFC3339), "2024-02-29T13:04:05Z");
//! assert_eq!(t.format(time::RFC822Z), "29 Feb 24 13:04 +0000");
//! assert_eq!(t.format("15:04:05.000"), "13:04:05.120");
//! assert_eq!(
//!     t.marshal_json().map_err(|e| e.to_string())?,
//!     r#""2024-02-29T13:04:05.12Z""#
//! );
//!
//! let parsed =
//!     time::parse(time::RFC3339, "2024-02-29T15:04:05+02:00").map_err(|e| e.to_string())?;
//! assert_eq!(parsed.to_string(), "2024-02-29 15:04:05 +0200 +0200");
//! assert!(parsed.equal(&Time::date(
//!     &DateFields {
//!         nanosecond: 0,
//!         ..fields
//!     },
//!     Location::Utc
//! )));
//! assert_eq!(
//!     time::parse(time::DATE_ONLY, "2024-02-30")
//!         .map_err(|e| e.to_string())
//!         .err(),
//!     Some(r#"parsing time "2024-02-30": day out of range"#.to_owned()),
//! );
//!
//! assert_eq!(
//!     time::parse_duration("1h15m30.5s").map(Duration::string),
//!     Ok("1h15m30.5s".to_owned())
//! );
//! assert_eq!(
//!     time::parse_duration("10 s").map_err(|e| e.to_string()),
//!     Err(r#"time: unknown unit " s" in duration "10 s""#.to_owned()),
//! );
//! # Ok::<(), String>(())
//! ```

mod duration;
mod format;
mod parse;
#[cfg(test)]
mod tests;

use std::{
    error::Error as StdError,
    fmt as stdfmt,
    time::{SystemTime, UNIX_EPOCH},
};

pub use duration::{Duration, DurationError, parse_duration};
pub use parse::{ParseError, parse};

use crate::utf8::lossy_string;

/// Go: `time.Layout`, the reference time in numerical order.
pub const LAYOUT: &str = "01/02 03:04:05PM '06 -0700";
/// Go: `time.ANSIC`.
pub const ANSIC: &str = "Mon Jan _2 15:04:05 2006";
/// Go: `time.UnixDate`.
pub const UNIX_DATE: &str = "Mon Jan _2 15:04:05 MST 2006";
/// Go: `time.RubyDate`.
pub const RUBY_DATE: &str = "Mon Jan 02 15:04:05 -0700 2006";
/// Go: `time.RFC822`.
pub const RFC822: &str = "02 Jan 06 15:04 MST";
/// Go: `time.RFC822Z`.
pub const RFC822Z: &str = "02 Jan 06 15:04 -0700";
/// Go: `time.RFC850`.
pub const RFC850: &str = "Monday, 02-Jan-06 15:04:05 MST";
/// Go: `time.RFC1123`.
pub const RFC1123: &str = "Mon, 02 Jan 2006 15:04:05 MST";
/// Go: `time.RFC1123Z`.
pub const RFC1123Z: &str = "Mon, 02 Jan 2006 15:04:05 -0700";
/// Go: `time.RFC3339`.
pub const RFC3339: &str = "2006-01-02T15:04:05Z07:00";
/// Go: `time.RFC3339Nano`.
pub const RFC3339_NANO: &str = "2006-01-02T15:04:05.999999999Z07:00";
/// Go: `time.Kitchen`.
pub const KITCHEN: &str = "3:04PM";
/// Go: `time.Stamp`.
pub const STAMP: &str = "Jan _2 15:04:05";
/// Go: `time.StampMilli`.
pub const STAMP_MILLI: &str = "Jan _2 15:04:05.000";
/// Go: `time.StampMicro`.
pub const STAMP_MICRO: &str = "Jan _2 15:04:05.000000";
/// Go: `time.StampNano`.
pub const STAMP_NANO: &str = "Jan _2 15:04:05.000000000";
/// Go: `time.DateTime`.
pub const DATE_TIME: &str = "2006-01-02 15:04:05";
/// Go: `time.DateOnly`.
pub const DATE_ONLY: &str = "2006-01-02";
/// Go: `time.TimeOnly`.
pub const TIME_ONLY: &str = "15:04:05";

const SECONDS_PER_MINUTE: i64 = 60;
const SECONDS_PER_HOUR: i64 = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: i64 = 24 * SECONDS_PER_HOUR;
const NANOS_PER_SECOND: i64 = 1_000_000_000;
/// Go: `absoluteYears`.
const ABSOLUTE_YEARS: u64 = 292_277_022_400;
/// Go: `marchThruDecember`.
const MARCH_THRU_DECEMBER: u32 = 31 + 30 + 31 + 30 + 31 + 31 + 30 + 31 + 30 + 31;
/// Go: `unixToInternal`.
const UNIX_TO_INTERNAL: i64 = 62_135_596_800;
/// Go: `absoluteToUnix`.
const ABSOLUTE_TO_UNIX: i64 = -9_223_372_028_741_760_000;
/// Go: `unixToAbsolute`.
const UNIX_TO_ABSOLUTE: i64 = 9_223_372_028_741_760_000;

/// Go: a `*time.Location`: UTC (which is also Go's `Local` in Prebid's deployment) or a fixed
/// zone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Location {
    /// Go: `time.UTC`.
    #[default]
    Utc,
    /// Go: `time.FixedZone(name, offset)`.
    Fixed {
        /// The zone abbreviation, which may be empty.
        name: String,
        /// Seconds east of UTC.
        offset: i64,
    },
}

impl Location {
    /// Go: `time.FixedZone`.
    #[must_use]
    pub fn fixed_zone(name: &str, offset: i64) -> Self {
        Self::Fixed {
            name: name.to_owned(),
            offset,
        }
    }

    /// Go: `Location.String`.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Utc => "UTC",
            Self::Fixed { name, .. } => name,
        }
    }

    /// Go: `Location.lookup`: the zone abbreviation and offset in effect.
    fn lookup(&self) -> (&str, i64) {
        match self {
            Self::Utc => ("UTC", 0),
            Self::Fixed { name, offset } => (name, *offset),
        }
    }
}

/// The fields of Go's `time.Date(year, month, day, hour, min, sec, nsec, loc)`.
///
/// Any field may be outside its usual range: [`Time::date`] normalizes them as Go does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateFields {
    /// The year.
    pub year: i64,
    /// The month, 1 (January) to 12.
    pub month: i64,
    /// The day of the month, from 1.
    pub day: i64,
    /// The hour, 0 to 23.
    pub hour: i64,
    /// The minute, 0 to 59.
    pub minute: i64,
    /// The second, 0 to 59.
    pub second: i64,
    /// The nanosecond, 0 to 999999999.
    pub nanosecond: i64,
}

impl Default for DateFields {
    /// January 1, year 1, 00:00:00: Go's zero `Time`.
    fn default() -> Self {
        Self {
            year: 1,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            nanosecond: 0,
        }
    }
}

/// Go: a `time.Time` without a monotonic clock reading.
///
/// The derived equality compares representation and location like Go's `==`; [`Time::equal`]
/// compares instants. The default is Go's zero `Time`, January 1, year 1, 00:00:00 UTC.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Time {
    /// Seconds since January 1, year 1 00:00:00 UTC (Go: `ext`).
    ext: i64,
    /// Nanoseconds within the second, 0 to 999999999.
    nsec: u32,
    loc: Location,
}

/// Go: `norm`: `hi * base + lo == nhi * base + nlo` with `0 <= nlo < base` (wrapping).
const fn norm(mut hi: i64, mut lo: i64, base: i64) -> (i64, i64) {
    if lo < 0 {
        let n = (lo.wrapping_neg().wrapping_sub(1) / base).wrapping_add(1);
        hi = hi.wrapping_sub(n);
        lo = lo.wrapping_add(n.wrapping_mul(base));
    }
    if lo >= base {
        let n = lo / base;
        hi = hi.wrapping_add(n);
        lo = lo.wrapping_sub(n.wrapping_mul(base));
    }
    (hi, lo)
}

/// Go: `isLeap`.
const fn is_leap(year: i64) -> bool {
    let mask = if year % 25 == 0 { 0xF } else { 3 };
    year & mask == 0
}

/// Go: `daysIn`.
const fn days_in(month: i64, year: i64) -> i64 {
    if month == 2 {
        return if is_leap(year) { 29 } else { 28 };
    }
    30 + ((month + (month >> 3)) & 1)
}

/// Go: `daysBefore`: the days of the year before the first of `month`.
const fn days_before(month: i64) -> i64 {
    let adj = if month >= 3 { -2 } else { 0 };
    (214 * month - 211) / 7 + adj
}

/// Go: `uint32(x)`.
fn low_u32(x: u64) -> u32 {
    u32::try_from(x & 0xFFFF_FFFF).unwrap_or(0)
}

/// Go: `dateToAbsDays`, for `month` in [1, 12].
fn date_to_abs_days(year: i64, month: u32, day: i64) -> u64 {
    let jan_feb = u32::from(month < 3);
    let amonth = month + 12 * jan_feb;
    let y = year
        .cast_unsigned()
        .wrapping_sub(u64::from(jan_feb))
        .wrapping_add(ABSOLUTE_YEARS);
    let ayday = (979 * amonth - 2919) >> 5;
    let century = y / 100;
    let cyear = low_u32(y % 100);
    let cday = 1461 * cyear / 4;
    let century_days = 146_097_u64.wrapping_mul(century) / 4;
    let rest = i64::from(cday + ayday).wrapping_add(day).wrapping_sub(1);
    century_days.wrapping_add(rest.cast_unsigned())
}

/// Go's `absDays.split` fields.
struct AbsDate {
    century: u64,
    cyear: u32,
    ayday: u32,
}

impl AbsDate {
    /// Go: `absDays.split`.
    fn split(days: u64) -> Self {
        let d = days.wrapping_mul(4).wrapping_add(3);
        let century = d / 146_097;
        let cd = low_u32(d % 146_097) | 3;
        let product = 2_939_745_u64 * u64::from(cd);
        Self {
            century,
            cyear: low_u32(product >> 32),
            ayday: low_u32(product) / 2_939_745 / 4,
        }
    }

    const fn jan_feb(&self) -> u32 {
        if self.ayday >= MARCH_THRU_DECEMBER {
            1
        } else {
            0
        }
    }

    /// Go: `absCentury.year`.
    fn year(&self) -> i64 {
        self.century
            .wrapping_mul(100)
            .wrapping_sub(ABSOLUTE_YEARS)
            .cast_signed()
            .wrapping_add(i64::from(self.cyear))
            .wrapping_add(i64::from(self.jan_feb()))
    }

    /// Go: `absDays.date`: year, month (1-12) and day.
    fn date(&self) -> (i64, i64, i64) {
        let d = 2141 * self.ayday + 197_913;
        let amonth = i64::from(d >> 16);
        let day = 1 + i64::from((d & 0xFFFF) / 2141);
        (self.year(), amonth - 12 * i64::from(self.jan_feb()), day)
    }

    /// Go: `absDays.yearYday`'s day of the year, 1 to 366.
    fn yday(&self) -> i64 {
        let leap =
            self.cyear.is_multiple_of(4) && (self.cyear != 0 || self.century.is_multiple_of(4));
        let jan_feb = self.jan_feb();
        let leap_after_feb = i64::from(u32::from(leap) & !jan_feb & 1);
        i64::from(self.ayday) + (1 + 31 + 28) + leap_after_feb - 365 * i64::from(jan_feb)
    }
}

/// Go's `absSeconds`: the wall clock of a time in its zone, from the absolute epoch.
struct Abs(u64);

impl Abs {
    const fn days(&self) -> u64 {
        self.0 / 86_400
    }

    /// Go: `absSeconds.clock`: hour, minute and second.
    fn clock(&self) -> (i64, i64, i64) {
        let secs = i64::try_from(self.0 % 86_400).unwrap_or(0);
        (secs / 3600, secs % 3600 / 60, secs % 60)
    }

    /// Go: `absDays.weekday`, 0 (Sunday) to 6.
    fn weekday(&self) -> i64 {
        // The absolute epoch is a Wednesday.
        i64::try_from((self.days() + 3) % 7).unwrap_or(0)
    }
}

/// `nsec` known to be in [0, 1e9).
fn nanos(nsec: i64) -> u32 {
    u32::try_from(nsec).unwrap_or(0)
}

impl Time {
    /// Go: `time.Unix(sec, nsec)`, in UTC (Go's `Local` here). `nsec` may be outside
    /// 0 to 999999999.
    #[must_use]
    pub fn from_unix(sec: i64, nsec: i64) -> Self {
        let (mut sec, mut nsec) = (sec, nsec);
        if !(0..NANOS_PER_SECOND).contains(&nsec) {
            let n = nsec / NANOS_PER_SECOND;
            sec = sec.wrapping_add(n);
            nsec -= n * NANOS_PER_SECOND;
            if nsec < 0 {
                nsec += NANOS_PER_SECOND;
                sec = sec.wrapping_sub(1);
            }
        }
        Self {
            ext: sec.wrapping_add(UNIX_TO_INTERNAL),
            nsec: nanos(nsec),
            loc: Location::Utc,
        }
    }

    /// Go: `time.UnixMilli(msec)`.
    #[must_use]
    pub fn from_unix_milli(msec: i64) -> Self {
        Self::from_unix(msec / 1000, (msec % 1000) * 1_000_000)
    }

    /// The Go time of a [`SystemTime`], in UTC.
    #[must_use]
    pub fn from_system_time(t: SystemTime) -> Self {
        match t.duration_since(UNIX_EPOCH) {
            Ok(d) => Self::from_unix(
                i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
                i64::from(d.subsec_nanos()),
            ),
            Err(before) => {
                let d = before.duration();
                Self::from_unix(
                    i64::try_from(d.as_secs()).map_or(i64::MIN, i64::wrapping_neg),
                    -i64::from(d.subsec_nanos()),
                )
            }
        }
    }

    /// Go: `time.Now()` without its monotonic reading, in UTC (Go's `Local` here).
    #[must_use]
    pub fn now() -> Self {
        Self::from_system_time(SystemTime::now())
    }

    /// Go: `time.Date(year, month, day, hour, min, sec, nsec, loc)`: out-of-range fields
    /// normalize (October 32 is November 1) and the wall clock is read in `loc`.
    #[must_use]
    pub fn date(fields: &DateFields, loc: Location) -> Self {
        let (year, m) = norm(fields.year, fields.month.wrapping_sub(1), 12);
        let (sec, nsec) = norm(fields.second, fields.nanosecond, NANOS_PER_SECOND);
        let (min, sec) = norm(fields.minute, sec, 60);
        let (hour, min) = norm(fields.hour, min, 60);
        let (day, hour) = norm(fields.day, hour, 24);
        let month = u32::try_from(m + 1).unwrap_or(1);
        let clock = hour
            .wrapping_mul(SECONDS_PER_HOUR)
            .wrapping_add(min.wrapping_mul(SECONDS_PER_MINUTE))
            .wrapping_add(sec);
        let unix = date_to_abs_days(year, month, day)
            .cast_signed()
            .wrapping_mul(SECONDS_PER_DAY)
            .wrapping_add(clock)
            .wrapping_add(ABSOLUTE_TO_UNIX)
            .wrapping_sub(loc.lookup().1);
        Self {
            ext: unix.wrapping_add(UNIX_TO_INTERNAL),
            nsec: nanos(nsec),
            loc,
        }
    }

    /// Go: `Time.UTC`.
    #[must_use]
    pub const fn utc(&self) -> Self {
        self.in_location(Location::Utc)
    }

    /// Go: `Time.Local`, which is [`Time::utc`] where Go's `Local` is UTC.
    #[must_use]
    pub const fn local(&self) -> Self {
        self.utc()
    }

    /// Go: `Time.In`.
    #[must_use]
    pub const fn in_location(&self, loc: Location) -> Self {
        Self {
            ext: self.ext,
            nsec: self.nsec,
            loc,
        }
    }

    /// Go: `Time.Location`.
    #[must_use]
    pub const fn location(&self) -> &Location {
        &self.loc
    }

    /// Go: `Time.Zone`: the zone abbreviation and its offset in seconds east of UTC.
    #[must_use]
    pub fn zone(&self) -> (&str, i64) {
        self.loc.lookup()
    }

    /// Go: `Time.Unix`.
    #[must_use]
    pub const fn unix(&self) -> i64 {
        self.ext.wrapping_sub(UNIX_TO_INTERNAL)
    }

    /// Go: `Time.UnixMilli`.
    #[must_use]
    pub fn unix_milli(&self) -> i64 {
        self.unix()
            .wrapping_mul(1000)
            .wrapping_add(i64::from(self.nsec) / 1_000_000)
    }

    /// Go: `Time.UnixNano`.
    #[must_use]
    pub fn unix_nano(&self) -> i64 {
        self.unix()
            .wrapping_mul(NANOS_PER_SECOND)
            .wrapping_add(i64::from(self.nsec))
    }

    /// Go: `Time.IsZero`.
    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.ext == 0 && self.nsec == 0
    }

    /// Go: `Time.locabs`: the zone abbreviation, offset and absolute wall-clock seconds.
    fn locabs(&self) -> (&str, i64, Abs) {
        let (name, offset) = self.loc.lookup();
        let sec = self
            .unix()
            .wrapping_add(offset)
            .wrapping_add(UNIX_TO_ABSOLUTE);
        (name, offset, Abs(sec.cast_unsigned()))
    }

    fn abs_date(&self) -> AbsDate {
        AbsDate::split(self.locabs().2.days())
    }

    /// Go: `Time.Year`.
    #[must_use]
    pub fn year(&self) -> i64 {
        self.abs_date().year()
    }

    /// Go: `Time.Month`, 1 (January) to 12.
    #[must_use]
    pub fn month(&self) -> i64 {
        self.abs_date().date().1
    }

    /// Go: `Time.Day`.
    #[must_use]
    pub fn day(&self) -> i64 {
        self.abs_date().date().2
    }

    /// Go: `Time.Hour`.
    #[must_use]
    pub fn hour(&self) -> i64 {
        self.locabs().2.clock().0
    }

    /// Go: `Time.Minute`.
    #[must_use]
    pub fn minute(&self) -> i64 {
        self.locabs().2.clock().1
    }

    /// Go: `Time.Second`.
    #[must_use]
    pub fn second(&self) -> i64 {
        self.locabs().2.clock().2
    }

    /// Go: `Time.Nanosecond`.
    #[must_use]
    pub fn nanosecond(&self) -> i64 {
        i64::from(self.nsec)
    }

    /// Go: `Time.Weekday`, 0 (Sunday) to 6.
    #[must_use]
    pub fn weekday(&self) -> i64 {
        self.locabs().2.weekday()
    }

    /// Go: `Time.YearDay`, 1 to 366.
    #[must_use]
    pub fn year_day(&self) -> i64 {
        self.abs_date().yday()
    }

    /// Go: `Time.addSec`, which saturates where Go's does.
    const fn add_sec(&mut self, d: i64) {
        let sum = self.ext.wrapping_add(d);
        self.ext = if (sum > self.ext) == (d > 0) {
            sum
        } else if d > 0 {
            i64::MAX
        } else {
            -i64::MAX
        };
    }

    /// Go: `Time.Add`.
    #[must_use]
    pub fn add(&self, d: Duration) -> Self {
        let mut dsec = d.0 / NANOS_PER_SECOND;
        let mut nsec = i64::from(self.nsec) + d.0 % NANOS_PER_SECOND;
        if nsec >= NANOS_PER_SECOND {
            dsec += 1;
            nsec -= NANOS_PER_SECOND;
        } else if nsec < 0 {
            dsec -= 1;
            nsec += NANOS_PER_SECOND;
        }
        let mut t = Self {
            ext: self.ext,
            nsec: nanos(nsec),
            loc: self.loc.clone(),
        };
        t.add_sec(dsec);
        t
    }

    /// Go: `Time.Sub`, saturating at the [`Duration`] range.
    #[must_use]
    pub fn sub(&self, u: &Self) -> Duration {
        let d = Duration(
            self.ext
                .wrapping_sub(u.ext)
                .wrapping_mul(NANOS_PER_SECOND)
                .wrapping_add(i64::from(self.nsec) - i64::from(u.nsec)),
        );
        if u.add(d).equal(self) {
            d
        } else if self.before(u) {
            Duration(i64::MIN)
        } else {
            Duration(i64::MAX)
        }
    }

    /// Go: `Time.Equal`: the same instant, in any location.
    #[must_use]
    pub const fn equal(&self, u: &Self) -> bool {
        self.ext == u.ext && self.nsec == u.nsec
    }

    /// Go: `Time.Before`.
    #[must_use]
    pub const fn before(&self, u: &Self) -> bool {
        self.ext < u.ext || self.ext == u.ext && self.nsec < u.nsec
    }

    /// Go: `Time.After`.
    #[must_use]
    pub const fn after(&self, u: &Self) -> bool {
        self.ext > u.ext || self.ext == u.ext && self.nsec > u.nsec
    }

    /// Go: `Time.MarshalJSON`: the quoted RFC 3339 form with nanoseconds.
    ///
    /// # Errors
    ///
    /// [`MarshalError`] when the year is outside 0 to 9999 or the zone hour outside 0 to 23.
    pub fn marshal_json(&self) -> Result<String, MarshalError> {
        let text = self.strict_rfc3339("Time.MarshalJSON: ")?;
        Ok(format!("\"{text}\""))
    }

    /// Go: `Time.MarshalText`.
    ///
    /// # Errors
    ///
    /// As [`Time::marshal_json`], with Go's `Time.MarshalText: ` prefix.
    pub fn marshal_text(&self) -> Result<String, MarshalError> {
        self.strict_rfc3339("Time.MarshalText: ")
    }

    fn strict_rfc3339(&self, prefix: &str) -> Result<String, MarshalError> {
        format::strict_rfc3339(self).map_err(|message| MarshalError(format!("{prefix}{message}")))
    }

    /// Go: `Time.Format`.
    #[must_use]
    pub fn format(&self, layout: &str) -> String {
        lossy_string(&self.append_format(Vec::new(), layout))
    }

    /// Go: `Time.AppendFormat`.
    #[must_use]
    pub fn append_format(&self, b: Vec<u8>, layout: &str) -> Vec<u8> {
        format::append_format(self, b, layout)
    }

    /// Go: `Time.UnmarshalJSON`. `Ok(None)` is JSON `null`, which leaves a Go time unchanged.
    /// As in Go, the string is not JSON-unescaped.
    ///
    /// # Errors
    ///
    /// [`UnmarshalError`] with Go's text.
    pub fn unmarshal_json(data: &str) -> Result<Option<Self>, UnmarshalError> {
        if data == "null" {
            return Ok(None);
        }
        let inner = data
            .strip_prefix('"')
            .and_then(|d| d.strip_suffix('"'))
            .ok_or(UnmarshalError::NotString)?;
        parse::parse_strict_rfc3339(inner.as_bytes())
            .map(Some)
            .map_err(UnmarshalError::Parse)
    }

    /// Go: `Time.UnmarshalText`.
    ///
    /// # Errors
    ///
    /// The [`ParseError`] of reading `data` as RFC 3339.
    pub fn unmarshal_text(data: &str) -> Result<Self, ParseError> {
        parse::parse_strict_rfc3339(data.as_bytes())
    }
}

impl stdfmt::Display for Time {
    /// Go: `Time.String`: the layout `2006-01-02 15:04:05.999999999 -0700 MST`.
    fn fmt(&self, f: &mut stdfmt::Formatter<'_>) -> stdfmt::Result {
        f.write_str(&self.format("2006-01-02 15:04:05.999999999 -0700 MST"))
    }
}

/// Go: the error of `Time.MarshalJSON` or `Time.MarshalText`, such as
/// `Time.MarshalJSON: year outside of range [0,9999]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarshalError(String);

impl stdfmt::Display for MarshalError {
    fn fmt(&self, f: &mut stdfmt::Formatter<'_>) -> stdfmt::Result {
        f.write_str(&self.0)
    }
}

impl StdError for MarshalError {}

/// Go: the errors of `Time.UnmarshalJSON`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnmarshalError {
    /// `Time.UnmarshalJSON: input is not a JSON string`.
    NotString,
    /// The string is not RFC 3339.
    Parse(ParseError),
}

impl stdfmt::Display for UnmarshalError {
    fn fmt(&self, f: &mut stdfmt::Formatter<'_>) -> stdfmt::Result {
        match self {
            Self::NotString => f.write_str("Time.UnmarshalJSON: input is not a JSON string"),
            Self::Parse(e) => e.fmt(f),
        }
    }
}

impl StdError for UnmarshalError {}
