//! Replays the `time_*.txt` goldens written by `testdata/go/golden/time.go`.

use std::{
    fmt::Display,
    slice,
    time::{Duration as StdDuration, UNIX_EPOCH},
};

use super::{
    ANSIC, DATE_ONLY, DATE_TIME, DateFields, Duration, KITCHEN, LAYOUT, Location, RFC822, RFC822Z,
    RFC850, RFC1123, RFC1123Z, RFC3339, RFC3339_NANO, RUBY_DATE, STAMP, STAMP_MICRO, STAMP_MILLI,
    STAMP_NANO, TIME_ONLY, Time, UNIX_DATE, duration::parse_duration_bytes, parse::parse_bytes,
};
use crate::{
    golden::{self, SplitMix64, Sweep, err_text},
    strconv::format_float,
    testutil::ensure_eq,
    utf8::lossy_string,
};

/// Go: `timeLayouts`.
const TIME_LAYOUTS: [&str; 41] = [
    LAYOUT,
    ANSIC,
    UNIX_DATE,
    RUBY_DATE,
    RFC822,
    RFC822Z,
    RFC850,
    RFC1123,
    RFC1123Z,
    RFC3339,
    RFC3339_NANO,
    KITCHEN,
    STAMP,
    STAMP_MILLI,
    STAMP_MICRO,
    STAMP_NANO,
    DATE_TIME,
    DATE_ONLY,
    TIME_ONLY,
    "15:04:05.000",
    "2006-01-02 15:04:05.999999999 -0700 MST",
    "2006-01-02T15:04:05.000Z07:00",
    "2006-01-02T15:04:05Z0700",
    "2006-01-02T15:04:05-07:00",
    "Monday, January 2, 2006 at 3:04:05 pm",
    "Mon Jan 2 3 03 4 04 5 05 PM pm",
    "_2 __2 002 2 02",
    "06 2006 _2006 __2006",
    "Z0700|Z070000|Z07|Z07:00|Z07:00:00",
    "-0700|-070000|-07|-07:00|-07:00:00|MST",
    "05.0 05.00 05.9 05.99 05,000 05,999 .000000000 .999999999 .0000000000 .9999999999",
    "15.000000 15,999 .99a .00 .91",
    "Janx January Januaryx Monx Mondays Mo Ja J M P p",
    "0102 1504 05 2006 20060102150405",
    "literal \u{e9} \u{b5} 2006-01-02",
    "",
    "no elements here",
    "2006-002 __2/2006",
    "01/02/06 03:04:05PM",
    ".9.0,9,0",
    "3PM 3pm 3AM",
];

/// Go: `layoutPieces`.
const LAYOUT_PIECES: [&str; 60] = [
    "2006",
    "06",
    "01",
    "1",
    "Jan",
    "January",
    "02",
    "2",
    "_2",
    "__2",
    "002",
    "Mon",
    "Monday",
    "15",
    "03",
    "3",
    "04",
    "4",
    "05",
    "5",
    "PM",
    "pm",
    "MST",
    "Z0700",
    "Z070000",
    "Z07",
    "Z07:00",
    "Z07:00:00",
    "-0700",
    "-070000",
    "-07",
    "-07:00",
    "-07:00:00",
    ".000",
    ".999",
    ",000",
    ",999",
    ".0",
    ".9",
    ".000000000",
    ".999999999",
    ".0000000000",
    " ",
    "  ",
    ":",
    "-",
    "T",
    "/",
    "x",
    "\u{e9}",
    "Janu",
    "Mo",
    "Z",
    "_",
    "0",
    ",",
    ".",
    "J",
    "M",
    "P",
];

/// Go: `mutateBytes`.
const MUTATE_BYTES: &[u8] = b"0123456789 :-+.,TZzAPMamp_/\xc3\xa9GMTUC";

/// Go: `zoneNames`.
const ZONE_NAMES: [&str; 6] = ["", "UTC", "MST", "CEST", "GMT+3", "X"];

/// Go: `durSigns`, `durNumbers` and `durUnits`.
const DUR_SIGNS: [&str; 4] = ["", "", "-", "+"];
const DUR_NUMBERS: [&str; 20] = [
    "0",
    "1",
    "7",
    "12",
    "100",
    "999",
    "1000000",
    "9223372036854775807",
    "9223372036854775808",
    "18446744073709551616",
    "1.5",
    ".5",
    "5.",
    ".",
    "",
    "0.000000001",
    "123.456789012345678901",
    "2562047",
    "00.10",
    "1e3",
];
const DUR_UNITS: [&str; 14] = [
    "ns", "us", "\u{b5}s", "\u{3bc}s", "ms", "s", "m", "h", "", "d", "x", "S", "mss", " s",
];

/// Go: `timeZones`.
fn time_zones() -> Vec<Location> {
    let fixed = Location::fixed_zone;
    vec![
        Location::Utc,
        fixed("", 0),
        fixed("IST", 5 * 3600 + 30 * 60),
        fixed("", -7 * 3600),
        fixed("MST", -7 * 3600),
        fixed("", -30),
        fixed("", 45),
        fixed("", 14 * 3600),
        fixed("", -12 * 3600 - 1),
        fixed("ODD", 3723),
        fixed("", 100 * 3600),
        fixed("", -25 * 3600),
        fixed("UTC", 0),
    ]
}

fn int(s: &str) -> Result<i64, String> {
    s.parse().map_err(|e| format!("{s:?}: {e}"))
}

/// The time described by Go's `timeFields`.
fn time_of(fields: &[String]) -> Result<Time, String> {
    let [sec, nsec, kind, name, offset] = fields else {
        return Err(format!("time fields: {fields:?}"));
    };
    let loc = if kind == "U" {
        Location::Utc
    } else {
        Location::fixed_zone(name, int(offset)?)
    };
    Ok(Time::from_unix(int(sec)?, int(nsec)?).in_location(loc))
}

/// Go: `timeFields`, tab-joined.
fn time_fields(t: &Time) -> String {
    let (name, offset) = t.zone();
    let kind = if matches!(t.location(), Location::Utc) {
        "U"
    } else {
        "F"
    };
    format!("{}\t{}\t{kind}\t{name}\t{offset}", t.unix(), t.nanosecond())
}

/// Go: `parseResult`.
fn parse_result<E: Display>(result: &Result<Time, E>) -> String {
    result
        .as_ref()
        .map_or_else(|_| err_text(result), |t| format!("ok\t{}", time_fields(t)))
}

/// Go: `dateResult`.
fn date_result(t: &Time) -> String {
    [
        time_fields(t),
        t.year().to_string(),
        t.month().to_string(),
        t.day().to_string(),
        t.hour().to_string(),
        t.minute().to_string(),
        t.second().to_string(),
        t.weekday().to_string(),
        t.year_day().to_string(),
        t.unix_milli().to_string(),
        t.unix_nano().to_string(),
        t.is_zero().to_string(),
        t.to_string(),
    ]
    .join("\t")
}

/// Go: `durationResult`.
fn duration_result(d: Duration) -> String {
    [
        d.0.to_string(),
        d.string(),
        format_float(d.seconds(), b'g', -1, 64),
        format_float(d.minutes(), b'g', -1, 64),
        format_float(d.hours(), b'g', -1, 64),
        d.milliseconds().to_string(),
        d.microseconds().to_string(),
    ]
    .join("\t")
}

/// Go: `parseDurationResult`.
fn parse_duration_result(s: &[u8]) -> String {
    let result = parse_duration_bytes(s);
    result.as_ref().map_or_else(
        |_| err_text(&result),
        |&d| format!("ok\t{}", duration_result(d)),
    )
}

#[test]
fn format_matches_go() -> Result<(), String> {
    let rows = golden::rows(include_str!("../../testdata/go/golden/time_format.txt"))?;
    ensure_eq(&rows.len(), &(63 * TIME_LAYOUTS.len()), "format rows")?;
    for row in &rows {
        let [layout, want] = row.get(5..).unwrap_or_default() else {
            return Err(format!("format row: {row:?}"));
        };
        let t = time_of(row.get(..5).unwrap_or_default())?;
        ensure_eq(&t.format(layout), want, &format!("{row:?}"))?;
        let appended = t.append_format(b"x".to_vec(), layout);
        ensure_eq(
            &lossy_string(&appended),
            &format!("x{want}"),
            "append_format",
        )?;
    }
    Ok(())
}

#[test]
fn parse_matches_go() -> Result<(), String> {
    let rows = golden::rows_bytes(include_str!("../../testdata/go/golden/time_parse.txt"))?;
    for row in &rows {
        let [layout, value, want] = row.as_slice() else {
            return Err(format!("parse row: {row:?}"));
        };
        let got = parse_result(&parse_bytes(layout, value));
        ensure_eq(
            &got,
            &lossy_string(want),
            &format!("parse {}", lossy_string(value)),
        )?;
    }
    // The public API, on a row with valid UTF-8.
    let t =
        super::parse(RFC3339, "2024-02-29T13:04:05.123456789+05:30").map_err(|e| e.to_string())?;
    ensure_eq(
        &time_fields(&t),
        &"1709192045\t123456789\tF\t\t19800".to_owned(),
        "parse",
    )
}

#[test]
fn parse_errors_expose_go_fields() -> Result<(), String> {
    let Err(e) = super::parse(TIME_ONLY, "25:00:00") else {
        return Err("25:00:00 parsed".to_owned());
    };
    ensure_eq(e.layout(), TIME_ONLY.as_bytes(), "layout")?;
    ensure_eq(e.value(), b"25:00:00".as_slice(), "value")?;
    ensure_eq(e.layout_elem(), b"15".as_slice(), "layout elem")?;
    ensure_eq(e.value_elem(), b":00:00".as_slice(), "value elem")?;
    ensure_eq(e.message(), ": hour out of range", "message")
}

#[test]
fn date_matches_go() -> Result<(), String> {
    let zones = time_zones();
    let rows = golden::rows(include_str!("../../testdata/go/golden/time_date.txt"))?;
    for row in &rows {
        let [
            zone,
            year,
            month,
            day,
            hour,
            minute,
            second,
            nanosecond,
            want,
        ] = row.as_slice()
        else {
            return Err(format!("date row: {row:?}"));
        };
        let fields = DateFields {
            year: int(year)?,
            month: int(month)?,
            day: int(day)?,
            hour: int(hour)?,
            minute: int(minute)?,
            second: int(second)?,
            nanosecond: int(nanosecond)?,
        };
        let index = usize::try_from(int(zone)?).map_err(|e| e.to_string())?;
        let loc = zones.get(index).ok_or("zone")?.clone();
        ensure_eq(
            &date_result(&Time::date(&fields, loc)),
            want,
            &format!("{row:?}"),
        )?;
    }
    ensure_eq(
        &Time::date(&DateFields::default(), Location::Utc),
        &Time::default(),
        "zero",
    )
}

#[test]
fn duration_matches_go() -> Result<(), String> {
    let rows = golden::rows_bytes(include_str!("../../testdata/go/golden/time_duration.txt"))?;
    for row in &rows {
        let [kind, input, want] = row.as_slice() else {
            return Err(format!("duration row: {row:?}"));
        };
        let got = if kind == b"parse" {
            parse_duration_result(input)
        } else {
            duration_result(Duration(int(&lossy_string(input))?))
        };
        ensure_eq(&got, &lossy_string(want), &lossy_string(input))?;
    }
    ensure_eq(
        &super::parse_duration("1h15m30.5s").map(Duration::string),
        &Ok("1h15m30.5s".to_owned()),
        "public parse_duration",
    )?;
    ensure_eq(
        &Duration(1500).to_string(),
        &"1.5\u{b5}s".to_owned(),
        "display",
    )
}

#[test]
fn json_matches_go() -> Result<(), String> {
    let marshal = golden::rows(include_str!("../../testdata/go/golden/time_marshal.txt"))?;
    ensure_eq(&marshal.len(), &63, "marshal rows")?;
    for row in &marshal {
        let [json, text] = row.get(5..).unwrap_or_default() else {
            return Err(format!("marshal row: {row:?}"));
        };
        let t = time_of(row.get(..5).unwrap_or_default())?;
        let got_json = t.marshal_json().unwrap_or_else(|e| format!("error: {e}"));
        ensure_eq(&got_json, json, &format!("marshal json {row:?}"))?;
        let got_text = t.marshal_text().unwrap_or_else(|e| format!("error: {e}"));
        ensure_eq(&got_text, text, &format!("marshal text {row:?}"))?;
    }
    let unmarshal = golden::rows(include_str!("../../testdata/go/golden/time_unmarshal.txt"))?;
    for row in &unmarshal {
        let [input, json, text] = row.as_slice() else {
            return Err(format!("unmarshal row: {row:?}"));
        };
        let result = Time::unmarshal_json(input);
        let got = match &result {
            Ok(None) => "null".to_owned(),
            Ok(Some(t)) => format!("ok\t{}", time_fields(t)),
            Err(_) => err_text(&result),
        };
        ensure_eq(&got, json, &format!("unmarshal json {input}"))?;
        ensure_eq(
            &parse_result(&Time::unmarshal_text(input)),
            text,
            &format!("unmarshal text {input}"),
        )?;
    }
    Ok(())
}

#[test]
fn arithmetic_saturates_like_go() -> Result<(), String> {
    // 400 years is beyond the Duration range in either direction.
    let early = Time::from_unix(0, 0);
    let late = Time::from_unix(400 * 365 * 86_400, 0);
    ensure_eq(&late.sub(&early), &Duration(i64::MAX), "late - early")?;
    ensure_eq(&early.sub(&late), &Duration(i64::MIN), "early - late")?;
    let t = Time::from_unix_milli(-1500);
    ensure_eq(
        &(t.unix(), t.nanosecond()),
        &(-2, 500_000_000),
        "unix milli",
    )?;
    let later = t.add(Duration::SECOND);
    ensure_eq(&later.sub(&t), &Duration::SECOND, "add then sub")?;
    ensure_eq(
        &(later.after(&t), later.before(&t), later.equal(&t)),
        &(true, false, false),
        "order",
    )?;
    let utc = Time::from_system_time(UNIX_EPOCH);
    ensure_eq(&utc, &Time::from_unix(0, 0), "epoch")?;
    let before = UNIX_EPOCH - StdDuration::from_millis(1500);
    ensure_eq(
        &Time::from_system_time(before),
        &Time::from_unix(-2, 500_000_000),
        "before epoch",
    )?;
    let zoned = Time::from_unix(0, 0).in_location(Location::fixed_zone("X", 3600));
    ensure_eq(&zoned.local(), &zoned.utc(), "local is utc")?;
    ensure_eq(&zoned.location().name(), &"X", "location name")?;
    ensure_eq(&Location::Utc.name(), &"UTC", "utc name")?;
    ensure_eq(&Time::now().after(&utc), &true, "now")
}

/// Go: `r.next() % n` as an index.
fn pick(r: &mut SplitMix64, n: usize) -> Result<usize, String> {
    let n = u64::try_from(n).map_err(|e| e.to_string())?;
    usize::try_from(r.next_u64() % n).map_err(|e| e.to_string())
}

/// Go: `int64(r.next() % n)` for a small `n`.
fn below(r: &mut SplitMix64, n: u64) -> i64 {
    (r.next_u64() % n).cast_signed()
}

/// Go: `randUnix`.
fn rand_unix(r: &mut SplitMix64) -> i64 {
    match r.next_u64() % 4 {
        0 => r.next_u64().cast_signed(),
        1 => below(r, 1 << 41) - (1 << 40),
        2 => below(r, 1 << 34) - (1 << 33),
        _ => 1_600_000_000 + below(r, 400_000_000),
    }
}

/// Go: `randNsec`.
fn rand_nsec(r: &mut SplitMix64) -> i64 {
    match r.next_u64() % 4 {
        0 => 0,
        1 => below(r, 1000) * 1_000_000,
        2 => below(r, 1_000_000) * 1000,
        _ => below(r, 1_000_000_000),
    }
}

/// Go: `randZone`.
fn rand_zone(r: &mut SplitMix64) -> Result<Location, String> {
    Ok(match r.next_u64() % 4 {
        0 => Location::Utc,
        1 => Location::fixed_zone("", (below(r, 27) - 12) * 3600),
        2 => {
            let name = ZONE_NAMES
                .get(pick(r, ZONE_NAMES.len())?)
                .ok_or("zone name")?;
            Location::fixed_zone(name, below(r, 200_001) - 100_000)
        }
        _ => Location::fixed_zone("", below(r, 7201) - 3600),
    })
}

/// Go: `randTime`.
fn rand_time(r: &mut SplitMix64) -> Result<Time, String> {
    let sec = rand_unix(r);
    let nsec = rand_nsec(r);
    Ok(Time::from_unix(sec, nsec).in_location(rand_zone(r)?))
}

/// Go: `randLayout`.
fn rand_layout(r: &mut SplitMix64) -> Result<String, String> {
    if r.next_u64().is_multiple_of(4) {
        let layout = TIME_LAYOUTS
            .get(pick(r, TIME_LAYOUTS.len())?)
            .ok_or("layout")?;
        return Ok((*layout).to_owned());
    }
    let mut layout = String::new();
    for _ in 0..=r.next_u64() % 6 {
        layout.push_str(
            LAYOUT_PIECES
                .get(pick(r, LAYOUT_PIECES.len())?)
                .ok_or("piece")?,
        );
    }
    Ok(layout)
}

/// Go: `mutate`.
fn mutate(r: &mut SplitMix64, v: &[u8]) -> Result<Vec<u8>, String> {
    let mut b = v.to_vec();
    for _ in 0..r.next_u64() % 3 {
        let pos = pick(r, b.len() + 1)?;
        let c = *MUTATE_BYTES
            .get(pick(r, MUTATE_BYTES.len())?)
            .ok_or("byte")?;
        match r.next_u64() % 3 {
            0 => {
                if pos < b.len() {
                    let _removed = b.remove(pos);
                }
            }
            1 => {
                if let Some(slot) = b.get_mut(pos) {
                    *slot = c;
                }
            }
            _ => b.insert(pos, c),
        }
    }
    Ok(b)
}

/// Go: `randField`.
fn rand_field(r: &mut SplitMix64, lo: i64, hi: i64) -> i64 {
    if r.next_u64().is_multiple_of(4) {
        return r.next_u64().cast_signed();
    }
    lo + below(r, (hi - lo).cast_unsigned())
}

/// Go: `randDuration`.
fn rand_duration(r: &mut SplitMix64) -> Duration {
    let v = r.next_u64().cast_signed();
    Duration(v >> (r.next_u64() % 64))
}

/// Go: `randDurationText`.
fn rand_duration_text(r: &mut SplitMix64) -> Result<String, String> {
    let mut text = (*DUR_SIGNS.get(pick(r, DUR_SIGNS.len())?).ok_or("sign")?).to_owned();
    for _ in 0..=r.next_u64() % 3 {
        text.push_str(
            DUR_NUMBERS
                .get(pick(r, DUR_NUMBERS.len())?)
                .ok_or("number")?,
        );
        text.push_str(DUR_UNITS.get(pick(r, DUR_UNITS.len())?).ok_or("unit")?);
    }
    Ok(text)
}

fn sweep_sum(name: &str) -> Result<String, String> {
    golden::checksum(
        include_str!("../../testdata/go/golden/time_sweeps.txt"),
        name,
    )
}

/// The random sweeps share one generator, so they run in Go's order.
#[test]
fn random_cases_match_go() -> Result<(), String> {
    let mut r = SplitMix64(6);
    random_format(&mut r)?;
    random_parse(&mut r)?;
    random_date(&mut r)?;
    random_duration(&mut r)
}

fn random_format(r: &mut SplitMix64) -> Result<(), String> {
    let rows = golden::rows(include_str!(
        "../../testdata/go/golden/time_format_random.txt"
    ))?;
    let mut sweep = Sweep::default();
    for i in 0..20_000 {
        let t = rand_time(r)?;
        let layout = rand_layout(r)?;
        let out = t.format(&layout);
        if let Some(row) = rows.get(i) {
            let want: Vec<String> = time_fields(&t)
                .split('\t')
                .map(str::to_owned)
                .chain([layout.clone(), out.clone()])
                .collect();
            ensure_eq(&want, row, &format!("random format {i}"))?;
        }
        sweep.add(&format!("{layout}\t{out}"));
    }
    ensure_eq(&sweep.sum(), &sweep_sum("time_format")?, "format checksum")
}

fn random_parse(r: &mut SplitMix64) -> Result<(), String> {
    let rows = golden::rows_bytes(include_str!(
        "../../testdata/go/golden/time_parse_random.txt"
    ))?;
    let mut sweep = Sweep::default();
    for i in 0..30_000 {
        let t = rand_time(r)?;
        let layout = rand_layout(r)?;
        let mut value = t.format(&layout).into_bytes();
        if r.next_u64().is_multiple_of(2) {
            value = mutate(r, &value)?;
        }
        let result = parse_result(&parse_bytes(layout.as_bytes(), &value));
        if let Some(row) = rows.get(i) {
            let want = [layout.as_bytes(), value.as_slice(), result.as_bytes()].map(<[u8]>::to_vec);
            ensure_eq(
                want.as_slice(),
                row.as_slice(),
                &format!("random parse {i}"),
            )?;
        }
        let mut line = layout.into_bytes();
        line.push(b'\t');
        line.extend_from_slice(&value);
        line.push(b'\t');
        line.extend_from_slice(result.as_bytes());
        sweep.add_bytes(&line);
    }
    ensure_eq(&sweep.sum(), &sweep_sum("time_parse")?, "parse checksum")
}

fn random_date(r: &mut SplitMix64) -> Result<(), String> {
    let rows = golden::rows(include_str!(
        "../../testdata/go/golden/time_date_random.txt"
    ))?;
    let mut sweep = Sweep::default();
    for i in 0..10_000 {
        let fields = DateFields {
            year: rand_field(r, -3000, 5000),
            month: rand_field(r, -24, 36),
            day: rand_field(r, -40, 70),
            hour: rand_field(r, -30, 50),
            minute: rand_field(r, -100, 160),
            second: rand_field(r, -100, 160),
            nanosecond: rand_field(r, -2_000_000_000, 3_000_000_000),
        };
        let t = Time::date(&fields, rand_zone(r)?);
        let d = rand_duration(r);
        let u = rand_time(r)?;
        let result = [
            date_result(&t),
            date_result(&t.add(d)),
            t.sub(&u).0.to_string(),
            t.equal(&u).to_string(),
            t.before(&u).to_string(),
            t.after(&u).to_string(),
        ]
        .join("\t");
        if let Some(row) = rows.get(i) {
            let what = format!("random date {i}: {fields:?}");
            ensure_eq(slice::from_ref(&result), row.as_slice(), &what)?;
        }
        sweep.add(&result);
    }
    ensure_eq(&sweep.sum(), &sweep_sum("time_date")?, "date checksum")
}

fn random_duration(r: &mut SplitMix64) -> Result<(), String> {
    let rows = golden::rows(include_str!(
        "../../testdata/go/golden/time_duration_random.txt"
    ))?;
    let mut sweep = Sweep::default();
    for i in 0..20_000 {
        let value = duration_result(rand_duration(r));
        let text = rand_duration_text(r)?;
        let parsed = parse_duration_result(text.as_bytes());
        if let Some(row) = rows.get(i) {
            let want = [value.clone(), text.clone(), parsed.clone()];
            ensure_eq(
                want.as_slice(),
                row.as_slice(),
                &format!("random duration {i}"),
            )?;
        }
        sweep.add(&format!("{value}\t{text}\t{parsed}"));
    }
    ensure_eq(
        &sweep.sum(),
        &sweep_sum("time_duration")?,
        "duration checksum",
    )
}
