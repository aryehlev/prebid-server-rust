package main

import (
	"math"
	"strconv"
	"strings"
	"time"
)

// timeLayouts are the layouts formatted for every sample time: Go's constants, the layouts
// Prebid uses, and layouts that exercise every element and the scanner's edge cases.
var timeLayouts = []string{
	time.Layout,
	time.ANSIC,
	time.UnixDate,
	time.RubyDate,
	time.RFC822,
	time.RFC822Z,
	time.RFC850,
	time.RFC1123,
	time.RFC1123Z,
	time.RFC3339,
	time.RFC3339Nano,
	time.Kitchen,
	time.Stamp,
	time.StampMilli,
	time.StampMicro,
	time.StampNano,
	time.DateTime,
	time.DateOnly,
	time.TimeOnly,
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
	"literal \xc3\xa9 \xc2\xb5 2006-01-02",
	"",
	"no elements here",
	"2006-002 __2/2006",
	"01/02/06 03:04:05PM",
	".9.0,9,0",
	"3PM 3pm 3AM",
}

// timeZones are the fixed zones the sample times are shown in.
var timeZones = []*time.Location{
	time.UTC,
	time.FixedZone("", 0),
	time.FixedZone("IST", 5*3600+30*60),
	time.FixedZone("", -7*3600),
	time.FixedZone("MST", -7*3600),
	time.FixedZone("", -30),
	time.FixedZone("", 45),
	time.FixedZone("", 14*3600),
	time.FixedZone("", -12*3600-1),
	time.FixedZone("ODD", 3723),
	time.FixedZone("", 100*3600),
	time.FixedZone("", -25*3600),
	time.FixedZone("UTC", 0),
}

// timeSamples are the curated times: each base time in UTC and in two of timeZones.
func timeSamples() []time.Time {
	utc := time.UTC
	base := []time.Time{
		{},
		time.Unix(0, 0),
		time.Date(2024, 2, 29, 13, 4, 5, 120000000, utc),
		time.Date(2006, 1, 2, 15, 4, 5, 999999999, utc),
		time.Date(2001, 9, 9, 1, 46, 40, 1, utc),
		time.Date(1999, 12, 31, 23, 59, 59, 100, utc),
		time.Date(2000, 1, 1, 0, 0, 0, 0, utc),
		time.Date(2023, 7, 4, 12, 0, 0, 500000, utc),
		time.Date(2023, 1, 9, 0, 30, 0, 0, utc),
		time.Date(2100, 3, 1, 9, 5, 7, 7000, utc),
		time.Date(1900, 12, 31, 23, 0, 0, 0, utc),
		time.Date(-1, 1, 1, 0, 0, 0, 0, utc),
		time.Date(-2024, 6, 15, 18, 0, 0, 0, utc),
		time.Date(10000, 1, 1, 0, 0, 0, 0, utc),
		time.Date(999, 4, 10, 4, 4, 4, 4, utc),
		time.Date(12345, 11, 30, 22, 10, 0, 0, utc),
		time.Unix(math.MaxInt64, 999999999),
		time.Unix(math.MinInt64, 0),
		time.Unix(1<<40, 123456789),
		time.Unix(-1<<40, 1),
		time.Unix(1700000000, 0),
	}
	var out []time.Time
	for i, t := range base {
		out = append(out, t, t.In(timeZones[i%len(timeZones)]), t.In(timeZones[(i*7+3)%len(timeZones)]))
	}
	return out
}

// timeFields is how the golden files describe a time: Unix seconds, nanoseconds, "U" for UTC
// or "F" for a fixed zone, and the zone's name and offset.
func timeFields(t time.Time) []string {
	name, offset := t.Zone()
	kind := "F"
	if t.Location() == time.UTC {
		kind = "U"
	}
	return []string{strconv.FormatInt(t.Unix(), 10), strconv.Itoa(t.Nanosecond()), kind, name, strconv.Itoa(offset)}
}

// parseResult renders a Parse result: "ok" and the time's fields, or the error.
func parseResult(t time.Time, err error) string {
	if err != nil {
		return errText(err)
	}
	return "ok\t" + strings.Join(timeFields(t), "\t")
}

// dateResult renders every accessor of a time.
func dateResult(t time.Time) string {
	return strings.Join(append(timeFields(t),
		strconv.Itoa(t.Year()),
		strconv.Itoa(int(t.Month())),
		strconv.Itoa(t.Day()),
		strconv.Itoa(t.Hour()),
		strconv.Itoa(t.Minute()),
		strconv.Itoa(t.Second()),
		strconv.Itoa(int(t.Weekday())),
		strconv.Itoa(t.YearDay()),
		strconv.FormatInt(t.UnixMilli(), 10),
		strconv.FormatInt(t.UnixNano(), 10),
		strconv.FormatBool(t.IsZero()),
		t.String(),
	), "\t")
}

// timeParseCases are curated Parse inputs: Prebid's layouts, every element's errors and
// Go's quirks.
var timeParseCases = [][2]string{
	{time.RFC3339, "2024-02-29T13:04:05Z"},
	{time.RFC3339, "2024-02-29T13:04:05.123456789+05:30"},
	{time.RFC3339, "2024-02-29T13:04:05-00:00"},
	{time.RFC3339, "2024-02-29T13:04:05+00:00"},
	{time.RFC3339, "2024-02-29T13:04:05.1234567891Z"},
	{time.RFC3339, "2024-02-29t13:04:05Z"},
	{time.RFC3339, "2024-02-29T13:04:05z"},
	{time.RFC3339, "2024-02-29 13:04:05Z"},
	{time.RFC3339, "2024-02-30T13:04:05Z"},
	{time.RFC3339, "2024-13-01T00:00:00Z"},
	{time.RFC3339, "2024-02-29T24:00:00Z"},
	{time.RFC3339, "2024-02-29T23:60:00Z"},
	{time.RFC3339, "2024-02-29T23:59:60Z"},
	{time.RFC3339, "2024-02-29T13:04:05+24:00"},
	{time.RFC3339, "2024-02-29T13:04:05+23:60"},
	{time.RFC3339, "2024-02-29T13:04:05+23:59"},
	{time.RFC3339, "2024-02-29T13:04:05+0530"},
	{time.RFC3339, "2024-02-29T13:04:05,123Z"},
	{time.RFC3339, "2024-02-29T13:04:05.Z"},
	{time.RFC3339, "2024-02-29T1:04:05Z"},
	{time.RFC3339, "10000-01-01T00:00:00Z"},
	{time.RFC3339, "0000-01-01T00:00:00Z"},
	{time.RFC3339, "-001-01-01T00:00:00Z"},
	{time.RFC3339, "2024-02-29T13:04:05"},
	{time.RFC3339, "2024-02-29T13:04:05Zjunk"},
	{time.RFC3339, ""},
	{time.RFC3339Nano, "2006-01-02T15:04:05.999999999-07:00"},
	{time.RFC3339Nano, "2006-01-02T15:04:05Z"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 MST"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 UTC"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 GMT"},
	{time.RFC1123, "mon, 02 jan 2006 15:04:05 PST"},
	{time.RFC1123, "Tue, 02 Jan 2006 15:04:05 GMT+3"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 GMT-11"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 GMT+24"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 GMT+03"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 ChST"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 WITA"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 AEST"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 ABCDT"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 ABCDE"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 ABCDEF"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 +03"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 -24"},
	{time.RFC1123, "Mon, 02 Jan 2006 15:04:05 Ab"},
	{time.RFC1123Z, "Mon, 02 Jan 2006 15:04:05 -0700"},
	{time.RFC1123Z, "Mon, 02 Jan 2006 15:04:05 +0000"},
	{time.RFC1123Z, "Mon, 02 Jan 2006 15:04:05 +2500"},
	{time.RFC1123Z, "Mon, 02 Jan 2006 15:04:05 +0061"},
	{time.RFC1123Z, "Mon, 02 Jan 2006 15:04:05 *0100"},
	{time.RFC1123Z, "Mon, 02 Jan 2006 15:04:05 +99a0"},
	{"-0700 MST", "+0300 MSK"},
	{"-0700 MST", "+0000 GMT"},
	{"-0700 MST", "+0000 UTC"},
	{"-0700 MST", "+0100 UTC"},
	{"-07:00:00", "-00:00:01"},
	{"-07:00:00", "+01:02:03"},
	{"-07:00:00", "+01:02:61"},
	{"-07:00:00", "+99:99:99"},
	{"Z07:00:00", "Z"},
	{"Z070000", "-010203"},
	{"Z07", "+05"},
	{"-07", "-5"},
	{time.Kitchen, "3:04PM"},
	{time.Kitchen, "12:00AM"},
	{time.Kitchen, "12:00PM"},
	{time.Kitchen, "13:00PM"},
	{time.Kitchen, "0:00AM"},
	{time.Kitchen, "3:04XM"},
	{time.Kitchen, "3:04P"},
	{"3:04pm", "11:59pm"},
	{"3:04pm", "11:59PM"},
	{time.ANSIC, "Mon Jan  2 15:04:05 2006"},
	{time.ANSIC, "Mon Jan 2 15:04:05 2006"},
	{time.ANSIC, "Mon Jan 12 15:04:05 2006"},
	{time.ANSIC, "Mon Jan 32 15:04:05 2006"},
	{time.ANSIC, "Mon Feb 29 15:04:05 2023"},
	{time.ANSIC, "Mon Feb 29 15:04:05 2024"},
	{time.ANSIC, "Fri Feb 30 15:04:05 2024"},
	{time.UnixDate, "Mon Jan  2 15:04:05 MST 2006"},
	{time.RFC850, "Monday, 02-Jan-06 15:04:05 MST"},
	{time.RFC850, "Funday, 02-Jan-06 15:04:05 MST"},
	{time.RFC822, "02 Jan 69 15:04 MST"},
	{time.RFC822, "02 Jan 68 15:04 MST"},
	{time.RFC822, "02 Jan -1 15:04 MST"},
	{time.RFC822, "02 Jan +1 15:04 MST"},
	{time.RFC822, "02 Jan 6 15:04 MST"},
	{time.DateTime, "2024-01-02 03:04:05"},
	{time.DateTime, "2024-01-02   03:04:05"},
	{time.DateTime, "2024-01-02 03:04:05.123456"},
	{time.DateTime, "2024-01-02 03:04:05,5"},
	{time.DateTime, "2024-01-02 03:04:05."},
	{time.DateTime, "2024-01-02 03:04:05.1234567891234"},
	{time.DateTime, "2024-1-02 03:04:05"},
	{time.DateTime, "02024-01-02 03:04:05"},
	{time.DateTime, "2024-01-02 03:04:05 extra"},
	{time.DateTime, "2024-01-02"},
	{time.DateOnly, "2024-02-30"},
	{time.DateOnly, "2023-02-29"},
	{time.DateOnly, "2024-00-10"},
	{time.DateOnly, "2024-13-10"},
	{time.DateOnly, "2024-1x-10"},
	{time.DateOnly, "abcd-01-01"},
	{time.DateOnly, "1abc-01-01"},
	{time.DateOnly, "+024-01-01"},
	{time.TimeOnly, "25:00:00"},
	{time.TimeOnly, "23:61:00"},
	{time.TimeOnly, "23:59:61"},
	{time.TimeOnly, "9:05:07"},
	{"15:04:05.000", "13:04:05.120"},
	{"15:04:05.000", "13:04:05.12"},
	{"15:04:05.000", "13:04:05.1234"},
	{"15:04:05.000", "13:04:05,123"},
	{"15:04:05.000", "13:04:05.-12"},
	{"15:04:05.000", "13:04:05.+12"},
	{"15:04:05.000", "13:04:05x123"},
	{"15:04:05.999", "13:04:05"},
	{"15:04:05.999", "13:04:05.5"},
	{"15:04:05.999", "13:04:05.123456789123"},
	{"15:04:05 .000", "13:04:05 .120"},
	{"15:04:05 .000", "13:04:05.120"},
	{".0000000000", ".1234567891"},
	{"2006-002", "2024-060"},
	{"2006-002", "2024-366"},
	{"2006-002", "2023-366"},
	{"2006-002", "2024-000"},
	{"2006-002", "2024-61"},
	{"2006 __2", "2024  60"},
	{"2006 __2", "2024 7"},
	{"2006 __2", "2024   7"},
	{"2006-01-02 002", "2024-03-01 061"},
	{"2006-01-02 002", "2024-03-02 061"},
	{"2006-01 002", "2024-02 061"},
	{"2006 002", "1900 060"},
	{"2006 002", "2000 366"},
	{"Jan _2", "Feb  3"},
	{"Jan _2", "feb 3"},
	{"January 2", "SEPTEMBER 30"},
	{"January 2", "Sept 30"},
	{"Mon", "Thu"},
	{"Monday", "Thurs"},
	{"_2006", "_2024"},
	{"__2006", "_ 2024"},
	{"", ""},
	{"", "x"},
	{"x", "y"},
	{"2006  01", "2006 01"},
	{"2006 01", "2006   01"},
	{"2006 01", "200601"},
	{"\xc3\xa9 2006", "\xc3\xa9 2024"},
	{"\xc3\xa9 2006", "\xc3\xa8 2024"},
	{"2006 \xc3\xa9", "2024 \xc3"},
	{"-07:00 2006", "-99:\xc3\xa9 2006"},
	{"-07:00", "-99:a\xc3\xa9"},
	{"MST", "\x00\x7f\"\\"},
	{"01/02 03:04:05PM '06 -0700", "01/02 03:04:05PM '06 -0700"},
}

// layoutPieces are concatenated into random layouts.
var layoutPieces = []string{
	"2006", "06", "01", "1", "Jan", "January", "02", "2", "_2", "__2", "002", "Mon", "Monday",
	"15", "03", "3", "04", "4", "05", "5", "PM", "pm", "MST", "Z0700", "Z070000", "Z07",
	"Z07:00", "Z07:00:00", "-0700", "-070000", "-07", "-07:00", "-07:00:00", ".000", ".999",
	",000", ",999", ".0", ".9", ".000000000", ".999999999", ".0000000000", " ", "  ", ":", "-",
	"T", "/", "x", "\xc3\xa9", "Janu", "Mo", "Z", "_", "0", ",", ".", "J", "M", "P",
}

// mutateBytes are the bytes random edits insert or substitute.
const mutateBytes = "0123456789 :-+.,TZzAPMamp_/\xc3\xa9GMTUC"

// randUnix draws Unix seconds from ranges that exercise the whole calendar.
func randUnix(r *splitmix64) int64 {
	switch r.next() % 4 {
	case 0:
		return int64(r.next())
	case 1:
		return int64(r.next()%(1<<41)) - 1<<40
	case 2:
		return int64(r.next()%(1<<34)) - 1<<33
	default:
		return 1600000000 + int64(r.next()%400000000)
	}
}

// randNsec draws nanoseconds that are often whole milliseconds or microseconds.
func randNsec(r *splitmix64) int64 {
	switch r.next() % 4 {
	case 0:
		return 0
	case 1:
		return int64(r.next()%1000) * 1000000
	case 2:
		return int64(r.next()%1000000) * 1000
	default:
		return int64(r.next() % 1000000000)
	}
}

var zoneNames = []string{"", "UTC", "MST", "CEST", "GMT+3", "X"}

// randZone draws UTC or a fixed zone.
func randZone(r *splitmix64) *time.Location {
	switch r.next() % 4 {
	case 0:
		return time.UTC
	case 1:
		return time.FixedZone("", (int(r.next()%27)-12)*3600)
	case 2:
		name := zoneNames[r.next()%uint64(len(zoneNames))]
		return time.FixedZone(name, int(r.next()%200001)-100000)
	default:
		return time.FixedZone("", int(r.next()%7201)-3600)
	}
}

func randTime(r *splitmix64) time.Time {
	sec := randUnix(r)
	nsec := randNsec(r)
	return time.Unix(sec, nsec).In(randZone(r))
}

// randLayout is one of timeLayouts or a concatenation of layoutPieces.
func randLayout(r *splitmix64) string {
	if r.next()%4 == 0 {
		return timeLayouts[r.next()%uint64(len(timeLayouts))]
	}
	n := 1 + r.next()%6
	var b strings.Builder
	for i := uint64(0); i < n; i++ {
		b.WriteString(layoutPieces[r.next()%uint64(len(layoutPieces))])
	}
	return b.String()
}

// mutate applies up to two random byte edits.
func mutate(r *splitmix64, v string) string {
	b := []byte(v)
	n := r.next() % 3
	for i := uint64(0); i < n; i++ {
		pos := int(r.next() % uint64(len(b)+1))
		c := mutateBytes[r.next()%uint64(len(mutateBytes))]
		switch r.next() % 3 {
		case 0:
			if pos < len(b) {
				b = append(b[:pos], b[pos+1:]...)
			}
		case 1:
			if pos < len(b) {
				b[pos] = c
			}
		default:
			b = append(b[:pos], append([]byte{c}, b[pos:]...)...)
		}
	}
	return string(b)
}

// randField is a Date argument: usually within [lo, hi), sometimes any int64.
func randField(r *splitmix64, lo, hi int64) int64 {
	if r.next()%4 == 0 {
		return int64(r.next())
	}
	return lo + int64(r.next()%uint64(hi-lo))
}

// randDuration draws durations of every magnitude.
func randDuration(r *splitmix64) time.Duration {
	v := int64(r.next())
	return time.Duration(v >> (r.next() % 64))
}

var (
	durSigns   = []string{"", "", "-", "+"}
	durNumbers = []string{
		"0", "1", "7", "12", "100", "999", "1000000", "9223372036854775807", "9223372036854775808",
		"18446744073709551616", "1.5", ".5", "5.", ".", "", "0.000000001", "123.456789012345678901",
		"2562047", "00.10", "1e3",
	}
	durUnits = []string{"ns", "us", "\xc2\xb5s", "\xce\xbcs", "ms", "s", "m", "h", "", "d", "x", "S", "mss", " s"}
)

// randDurationText concatenates a sign and one to three number-and-unit terms.
func randDurationText(r *splitmix64) string {
	var b strings.Builder
	b.WriteString(durSigns[r.next()%uint64(len(durSigns))])
	n := 1 + r.next()%3
	for i := uint64(0); i < n; i++ {
		b.WriteString(durNumbers[r.next()%uint64(len(durNumbers))])
		b.WriteString(durUnits[r.next()%uint64(len(durUnits))])
	}
	return b.String()
}

// durationResult renders a duration's String and conversions.
func durationResult(d time.Duration) string {
	return strings.Join([]string{
		strconv.FormatInt(int64(d), 10),
		d.String(),
		strconv.FormatFloat(d.Seconds(), 'g', -1, 64),
		strconv.FormatFloat(d.Minutes(), 'g', -1, 64),
		strconv.FormatFloat(d.Hours(), 'g', -1, 64),
		strconv.FormatInt(d.Milliseconds(), 10),
		strconv.FormatInt(d.Microseconds(), 10),
	}, "\t")
}

func parseDurationResult(s string) string {
	d, err := time.ParseDuration(s)
	if err != nil {
		return errText(err)
	}
	return "ok\t" + durationResult(d)
}

var timeDurationInputs = []string{
	"0", "-0", "+0", "", "-", "+", "1", "1s", "-1s", "+1s", "1.5h", "1h30m", "1h15m30.5s", ".5s",
	"5.s", ".s", "-.s", "1.s", "300ms", "1us", "1\xc2\xb5s", "1\xce\xbcs", "1ns", "1.000000001s",
	"0.1ns", "1e3s", "1d", "10 s", "1s ", " 1s", "1sm", "1m1", "9223372036854775807ns",
	"9223372036854775808ns", "-9223372036854775808ns", "9223372036854775807ns1ns",
	"2562047h47m16.854775807s", "2562047h47m16.854775808s", "-2562047h47m16.854775808s",
	"2562048h", "18446744073709551616ns", "0.0000000000000000000001h",
	"1.0000000000000000000000000001h", "123456789012345678901234567890.5ns", "3.6e12", "1h1h",
	"5m0s", "-0s", "00s", "0.0s", "1..2s", "1.2.3s", "MS", "1MS", "1Ms", "1\xc2\xb5", "1h-1m",
	"1h+1m", "--1s", "-+1s", "1h 1m", "9223372036854775808ns9223372036854775808ns",
	"-9223372036854775808ns9223372036854775808ns", "1\xffs", "\"1s\"",
}

var timeDurationValues = []int64{
	0, 1, -1, 999, 1000, 1001, 1500, -1500, 999999, 1000000, 1500000, 999999999, 1000000000,
	1000000001, -1000000000, 59000000000, 60000000000, 61500000000, 3600000000000, 3599999999999,
	86400000000000, math.MaxInt64, math.MinInt64, math.MinInt64 + 1, 1 << 53, 1<<53 + 1,
}

// timeDateInputs are curated Date arguments, normalization and overflow included.
var timeDateInputs = [][7]int{
	{2024, 2, 29, 13, 4, 5, 120000000},
	{2023, 2, 29, 0, 0, 0, 0},
	{2024, 13, 1, 0, 0, 0, 0},
	{2024, 0, 1, 0, 0, 0, 0},
	{2024, -1, 31, 0, 0, 0, 0},
	{2024, 1, 0, 0, 0, 0, 0},
	{2024, 1, -1, 0, 0, 0, 0},
	{2024, 1, 1, 24, 60, 60, 1000000000},
	{2024, 1, 1, -1, -1, -1, -1},
	{0, 1, 1, 0, 0, 0, 0},
	{-1, 12, 31, 0, 0, 0, 0},
	{1, 1, 1, 0, 0, 0, 0},
	{9999, 12, 31, 23, 59, 59, 999999999},
	{1900, 3, 1, 0, 0, 0, 0},
	{2000, 3, 1, 0, 0, 0, 0},
	{292277026596, 12, 4, 15, 30, 7, 0},
	{-292277022399, 1, 1, 0, 0, 0, 0},
	{math.MaxInt64, 1, 1, 0, 0, 0, 0},
	{math.MinInt64, 1, 1, 0, 0, 0, 0},
	{2024, math.MaxInt64, 1, 0, 0, 0, 0},
	{2024, math.MinInt64, 1, 0, 0, 0, 0},
	{2024, 1, math.MaxInt64, 0, 0, 0, 0},
	{0, 0, 0, math.MinInt64, math.MinInt64, math.MinInt64, math.MinInt64},
	{0, 0, 0, math.MaxInt64, math.MaxInt64, math.MaxInt64, math.MaxInt64},
}

var timeUnmarshalInputs = []string{
	`null`, `"2024-02-29T13:04:05Z"`, `"2024-02-29T13:04:05.123+05:30"`,
	`"2024-02-29T13:04:05+00:00"`, `""`, `"`, `''`, `2024`, `"2024-02-29"`,
	`"2024-02-29T13:04:05,5Z"`, `"2024-02-29T1:04:05Z"`, `"2024-02-29T13:04:05+24:00"`,
	`"2024-02-29T13:04:05+23:61"`, `"0000-01-01T00:00:00Z"`, `"9999-12-31T23:59:59.999999999Z"`,
	`"2024-02-29T13:04:05.1234567891Z"`, `"2024-02-29t13:04:05z"`,
	`"2024-02-29T13:04:05+00:00"`, ` "2024-02-29T13:04:05Z"`, `"2024-02-29T13:04:05Z" `,
	`NULL`, `"null"`, `"Z"`, `"2024-02-29T13:04:05 MST"`,
}

func genTime() {
	// Prebid's container has no zone database, so Go's Local is a zone-less UTC.
	local := time.Local
	time.Local = time.UTC
	defer func() { time.Local = local }()

	samples := timeSamples()
	var format table
	for _, t := range samples {
		for _, layout := range timeLayouts {
			format.row(append(timeFields(t), layout, t.Format(layout))...)
		}
	}
	format.write("time_format.txt")

	var parse table
	for _, c := range timeParseCases {
		parse.row(c[0], c[1], parseResult(time.Parse(c[0], c[1])))
	}
	parse.write("time_parse.txt")

	var date table
	for i, in := range timeDateInputs {
		loc := timeZones[i%len(timeZones)]
		t := time.Date(in[0], time.Month(in[1]), in[2], in[3], in[4], in[5], in[6], loc)
		fields := []string{strconv.Itoa(i % len(timeZones))}
		for _, f := range in {
			fields = append(fields, strconv.Itoa(f))
		}
		date.row(append(fields, dateResult(t))...)
	}
	date.write("time_date.txt")

	var dur table
	for _, s := range timeDurationInputs {
		dur.row("parse", s, parseDurationResult(s))
	}
	for _, v := range timeDurationValues {
		dur.row("value", strconv.FormatInt(v, 10), durationResult(time.Duration(v)))
	}
	dur.write("time_duration.txt")

	var js table
	for _, t := range samples {
		b, err := t.MarshalJSON()
		text, textErr := t.MarshalText()
		js.row(append(timeFields(t), string(b)+errText(err), string(text)+errText(textErr))...)
	}
	js.write("time_marshal.txt")
	var un table
	for _, in := range timeUnmarshalInputs {
		var t time.Time
		err := t.UnmarshalJSON([]byte(in))
		result := parseResult(t, err)
		if in == "null" {
			result = "null"
		}
		var u time.Time
		err = u.UnmarshalText([]byte(in))
		un.row(in, result, parseResult(u, err))
	}
	un.write("time_unmarshal.txt")

	genTimeSweeps()
}

// genTimeSweeps writes random cases: the first 500 of each sweep in full, and a checksum of
// all of them.
func genTimeSweeps() {
	r := splitmix64{6}
	var sums table

	var formatRows table
	sw := newSweep()
	for i := 0; i < 20000; i++ {
		t := randTime(&r)
		layout := randLayout(&r)
		out := t.Format(layout)
		if i < 500 {
			formatRows.row(append(timeFields(t), layout, out)...)
		}
		sw.add(layout + "\t" + out)
	}
	formatRows.write("time_format_random.txt")
	sums.row("time_format", sw.sum())

	var parseRows table
	sw = newSweep()
	for i := 0; i < 30000; i++ {
		t := randTime(&r)
		layout := randLayout(&r)
		value := t.Format(layout)
		if r.next()%2 == 0 {
			value = mutate(&r, value)
		}
		result := parseResult(time.Parse(layout, value))
		if i < 500 {
			parseRows.row(layout, value, result)
		}
		sw.add(layout + "\t" + value + "\t" + result)
	}
	parseRows.write("time_parse_random.txt")
	sums.row("time_parse", sw.sum())

	var dateRows table
	sw = newSweep()
	for i := 0; i < 10000; i++ {
		year := randField(&r, -3000, 5000)
		month := randField(&r, -24, 36)
		day := randField(&r, -40, 70)
		hour := randField(&r, -30, 50)
		minute := randField(&r, -100, 160)
		sec := randField(&r, -100, 160)
		nsec := randField(&r, -2000000000, 3000000000)
		loc := randZone(&r)
		t := time.Date(int(year), time.Month(month), int(day), int(hour), int(minute), int(sec), int(nsec), loc)
		d := randDuration(&r)
		u := randTime(&r)
		result := strings.Join([]string{
			dateResult(t),
			dateResult(t.Add(d)),
			strconv.FormatInt(int64(t.Sub(u)), 10),
			strconv.FormatBool(t.Equal(u)),
			strconv.FormatBool(t.Before(u)),
			strconv.FormatBool(t.After(u)),
		}, "\t")
		if i < 500 {
			dateRows.row(result)
		}
		sw.add(result)
	}
	dateRows.write("time_date_random.txt")
	sums.row("time_date", sw.sum())

	var durRows table
	sw = newSweep()
	for i := 0; i < 20000; i++ {
		value := durationResult(randDuration(&r))
		text := randDurationText(&r)
		parsed := parseDurationResult(text)
		if i < 500 {
			durRows.row(value, text, parsed)
		}
		sw.add(value + "\t" + text + "\t" + parsed)
	}
	durRows.write("time_duration_random.txt")
	sums.row("time_duration", sw.sum())

	sums.write("time_sweeps.txt")
}
