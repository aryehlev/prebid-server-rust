package main

import (
	"strconv"
	"time"

	"github.com/benbjohnson/clock"
)

// timeLayoutCount is the number of seeded instants of the layout sweep.
const timeLayoutCount = 50000

// maxUnixSecond is 9999-12-31T23:59:59Z, the last second the time crate's default range holds.
const maxUnixSecond = 253402300799

// seededInstant draws an instant and an offset: the Unix second (uniform over years 1970 to
// 9999, or within a day of the epoch, of 2000-01-01 or of the end of 9999), the nanoseconds
// (zero, whole milliseconds, whole microseconds, or any, so that trailing zeros vary) and the
// offset in seconds east of UTC (zero, whole minutes within 18 hours, or whole hours).
func seededInstant(r *splitmix64) (sec int64, nsec int64, offset int) {
	switch r.intn(4) {
	case 0:
		sec = int64(r.intn(86400))
	case 1:
		sec = 946684800 + int64(r.intn(2*86400)) - 86400
	case 2:
		sec = maxUnixSecond - int64(r.intn(86400))
	default:
		sec = int64(r.next() % (maxUnixSecond + 1))
	}
	switch r.intn(4) {
	case 0:
		nsec = 0
	case 1:
		nsec = int64(r.intn(1000)) * 1000000
	case 2:
		nsec = int64(r.intn(1000000)) * 1000
	default:
		nsec = int64(r.intn(1000000000))
	}
	switch r.intn(3) {
	case 0:
		offset = 0
	case 1:
		offset = (r.intn(2*18*60+1) - 18*60) * 60
	default:
		offset = (r.intn(37) - 18) * 3600
	}
	return sec, nsec, offset
}

type timeEdgeJSON struct {
	Sec     int64  `json:"sec"`
	Nsec    int64  `json:"nsec"`
	Offset  int    `json:"offset"`
	RFC3339 string `json:"rfc3339"`
	Nano    string `json:"rfc3339_nano"`
}

type timeLayoutsJSON struct {
	Seed          uint64         `json:"seed"`
	Count         int            `json:"count"`
	InputsSHA256  string         `json:"inputs_sha256"`
	OutputsSHA256 string         `json:"outputs_sha256"`
	Edges         []timeEdgeJSON `json:"edges"`
}

// genTimeLayouts writes timeutil_layouts.json: Format(time.RFC3339) and
// Format(time.RFC3339Nano) of 50,000 seeded instants in a fixed zone of the seeded offset (an
// input is "sec nsec offset"; an output RFC3339, a tab and RFC3339Nano), and of edge instants.
func genTimeLayouts() {
	out := timeLayoutsJSON{Seed: 0x5EED_0201, Count: timeLayoutCount}
	r := &splitmix64{state: out.Seed}
	m := newManifest()
	for i := 0; i < out.Count; i++ {
		sec, nsec, offset := seededInstant(r)
		t := time.Unix(sec, nsec).In(time.FixedZone("", offset))
		m.add(strconv.FormatInt(sec, 10)+" "+strconv.FormatInt(nsec, 10)+" "+strconv.Itoa(offset),
			t.Format(time.RFC3339)+"\t"+t.Format(time.RFC3339Nano))
	}
	out.InputsSHA256, out.OutputsSHA256 = m.inputs(), m.outputs()
	for _, e := range []timeEdgeJSON{
		{Sec: 0}, {Sec: 0, Nsec: 1}, {Sec: 0, Nsec: 100000000}, {Sec: 0, Nsec: 120000000},
		{Sec: 0, Nsec: 999999999}, {Sec: 1763461230, Nsec: 0}, {Sec: 1763461230, Nsec: 500},
		{Sec: 1763461230, Offset: 3600}, {Sec: 1763461230, Offset: -5 * 3600},
		{Sec: 1763461230, Offset: 5*3600 + 30*60}, {Sec: 1763461230, Offset: -(3*3600 + 30*60)},
		{Sec: 1763461230, Offset: 14 * 3600}, {Sec: -62135596800}, {Sec: maxUnixSecond, Nsec: 999999999},
		{Sec: 951782400, Nsec: 7000}, {Sec: 1709164800, Offset: -60},
	} {
		t := time.Unix(e.Sec, e.Nsec).In(time.FixedZone("", e.Offset))
		e.RFC3339, e.Nano = t.Format(time.RFC3339), t.Format(time.RFC3339Nano)
		out.Edges = append(out.Edges, e)
	}
	writeJSON("timeutil_layouts.json", out)
}

// clockScenarioCount is the number of seeded Mock scenarios.
const clockScenarioCount = 400

// clockMaxTickers keeps every scenario at 12 tickers or fewer, where sort.Sort is an insertion
// sort and so keeps tickers that are due at the same time in the order they were created.
const clockMaxTickers = 12

// clockPeriods are the ticker periods and Add steps of the scenarios, in milliseconds.
var clockPeriods = []int{1, 2, 3, 5, 7, 10, 15, 30, 60, 1000}

type clockScenarioJSON struct {
	Ops     []string `json:"ops"`
	Results []string `json:"results"`
}

type clockJSON struct {
	Seed      uint64              `json:"seed"`
	Scenarios []clockScenarioJSON `json:"scenarios"`
}

// genClock writes timeutil_clock.json: seeded scenarios run on benbjohnson/clock v1.3.0's
// Mock, the test clock of Prebid's analytics modules. Each op is one of "ticker d" (Ticker(d)),
// "add d" (Add), "set t" (Set to the epoch plus t), "read i" (a receive from ticker i's C that
// does not wait), "stop i", "reset i d" and "now", with d and t in milliseconds and tickers
// numbered from 0 in creation order. A read's result is the received time in milliseconds
// since the epoch, or "none"; now's is Now in milliseconds; the other ops' results are "".
func genClock() {
	out := clockJSON{Seed: 0x5EED_0301}
	r := &splitmix64{state: out.Seed}
	for i := 0; i < clockScenarioCount; i++ {
		out.Scenarios = append(out.Scenarios, clockScenario(r))
	}
	writeJSON("timeutil_clock.json", out)
}

func clockScenario(r *splitmix64) clockScenarioJSON {
	mock := clock.NewMock()
	var tickers []*clock.Ticker
	var sc clockScenarioJSON
	ms := func(d int) time.Duration { return time.Duration(d) * time.Millisecond }
	since := func(t time.Time) string { return strconv.FormatInt(t.Sub(time.Unix(0, 0)).Milliseconds(), 10) }
	n := 4 + r.intn(40)
	for j := 0; j < n; j++ {
		var op, result string
		k := r.intn(20)
		switch {
		case k < 3 || len(tickers) == 0:
			if len(tickers) == clockMaxTickers {
				op = "now"
				result = since(mock.Now())
				break
			}
			d := pick(r, clockPeriods)
			tickers = append(tickers, mock.Ticker(ms(d)))
			op = "ticker " + strconv.Itoa(d)
		case k < 8:
			d := pick(r, clockPeriods) * (1 + r.intn(4))
			mock.Add(ms(d))
			op = "add " + strconv.Itoa(d)
		case k < 9:
			now := int(mock.Now().Sub(time.Unix(0, 0)).Milliseconds())
			t := now - 20 + r.intn(80)
			if t < 0 {
				t = 0
			}
			mock.Set(time.Unix(0, 0).Add(ms(t)))
			op = "set " + strconv.Itoa(t)
		case k < 15:
			i := r.intn(len(tickers))
			op = "read " + strconv.Itoa(i)
			select {
			case t := <-tickers[i].C:
				result = since(t)
			default:
				result = "none"
			}
		case k < 16:
			i := r.intn(len(tickers))
			tickers[i].Stop()
			op = "stop " + strconv.Itoa(i)
		case k < 18:
			i := r.intn(len(tickers))
			d := pick(r, clockPeriods)
			tickers[i].Reset(ms(d))
			op = "reset " + strconv.Itoa(i) + " " + strconv.Itoa(d)
		default:
			op = "now"
			result = since(mock.Now())
		}
		sc.Ops = append(sc.Ops, op)
		sc.Results = append(sc.Results, result)
	}
	return sc
}
