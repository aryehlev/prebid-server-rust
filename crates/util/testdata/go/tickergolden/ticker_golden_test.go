// Package tickergolden writes the TickerTask timing golden replayed by the pbs-util Rust tests.
//
// Each scenario runs util/task.TickerTask inside a testing/synctest bubble, whose fake clock
// makes the timing deterministic: a runner sleeps for a scripted duration on every run, Stop is
// called at a scripted time, and the golden records when every run started. Scenarios avoid a
// Stop at the same instant as a tick, where Go's select picks a case at random.
//
// Run from the repository root with the toolchain the Dockerfile builds with:
//
//	GOLDEN_OUT=rust/crates/util/testdata/golden/task_ticker.txt \
//	    go test ./rust/crates/util/testdata/go/tickergolden -run TestWriteGolden -count=1
package tickergolden

import (
	"os"
	"strconv"
	"strings"
	"sync"
	"testing"
	"testing/synctest"
	"time"

	"github.com/prebid/prebid-server/v4/util/task"
)

type scenario struct {
	name     string
	interval time.Duration
	// durations are the run lengths, used in turn.
	durations []time.Duration
	// stopAt is when Stop is called, measured from the call to Start; a negative value calls
	// Stop before Start.
	stopAt time.Duration
	// horizon is how long the scenario is observed.
	horizon time.Duration
}

var scenarios = []scenario{
	{"zero interval runs once", 0, []time.Duration{0}, time.Hour, 2 * time.Hour},
	{"negative interval runs once", -time.Second, []time.Duration{0}, time.Hour, 2 * time.Hour},
	{"currency default", 30 * time.Minute, []time.Duration{0}, 2*time.Hour + 15*time.Minute, 3 * time.Hour},
	{"short runs", 10 * time.Second, []time.Duration{3 * time.Second}, 95 * time.Second, 3 * time.Minute},
	{"runs as long as the interval", 10 * time.Second, []time.Duration{10 * time.Second, 10 * time.Second, 0}, 65 * time.Second, 3 * time.Minute},
	{"runs longer than two intervals", 10 * time.Second, []time.Duration{25 * time.Second, 25 * time.Second, 0, 0}, 140 * time.Second, 4 * time.Minute},
	{"mixed run lengths", 10 * time.Second, []time.Duration{12 * time.Second, time.Second, 35 * time.Second, 0}, 150*time.Second + 500*time.Millisecond, 5 * time.Minute},
	{"stop before the first tick", 10 * time.Second, []time.Duration{0}, 5 * time.Second, time.Minute},
	{"stop during the first run", 10 * time.Second, []time.Duration{15 * time.Second, 0}, 5 * time.Second, time.Minute},
	{"long first run delays the ticker", 10 * time.Second, []time.Duration{15 * time.Second, 0, 0, 0, 0, 0}, 47 * time.Second, time.Minute},
	{"stop before start", 10 * time.Second, []time.Duration{0}, -1, time.Minute},
	{"one second interval", time.Second, []time.Duration{0}, 10*time.Second + 500*time.Millisecond, 20 * time.Second},
	{"alternating overruns", 7 * time.Second, []time.Duration{6 * time.Second, 8 * time.Second, 2 * time.Second, 0}, 52*time.Second + 500*time.Millisecond, 2 * time.Minute},
	{"stop during a periodic run", 10 * time.Second, []time.Duration{0, 8 * time.Second}, 15 * time.Second, time.Minute},
	{"run of exactly two intervals", 10 * time.Second, []time.Duration{0, 20 * time.Second}, 65 * time.Second, 2 * time.Minute},
}

func joinDurations(ds []time.Duration) string {
	parts := make([]string, len(ds))
	for i, d := range ds {
		parts[i] = strconv.FormatInt(int64(d), 10)
	}
	return strings.Join(parts, ",")
}

// run returns the start times of every run of the scenario, relative to the call to Start.
func run(t *testing.T, sc scenario) []time.Duration {
	var starts []time.Duration
	synctest.Test(t, func(t *testing.T) {
		var mu sync.Mutex
		var start time.Time
		calls := 0
		tt := task.NewTickerTaskFromFunc(sc.interval, func() error {
			mu.Lock()
			starts = append(starts, time.Since(start))
			d := sc.durations[calls%len(sc.durations)]
			calls++
			mu.Unlock()
			time.Sleep(d)
			return nil
		})
		start = time.Now()
		if sc.stopAt < 0 {
			tt.Stop()
		} else {
			go func() {
				time.Sleep(sc.stopAt)
				tt.Stop()
			}()
		}
		tt.Start()
		time.Sleep(sc.horizon - time.Since(start))
		synctest.Wait()
	})
	return starts
}

func TestWriteGolden(t *testing.T) {
	out := os.Getenv("GOLDEN_OUT")
	if out == "" {
		t.Skip("GOLDEN_OUT is not set")
	}
	var b strings.Builder
	for _, sc := range scenarios {
		starts := run(t, sc)
		fields := []string{
			sc.name,
			strconv.FormatInt(int64(sc.interval), 10),
			joinDurations(sc.durations),
			strconv.FormatInt(int64(sc.stopAt), 10),
			strconv.FormatInt(int64(sc.horizon), 10),
			joinDurations(starts),
		}
		for i, f := range fields {
			if i > 0 {
				b.WriteByte('\t')
			}
			b.WriteString(strconv.Quote(f))
		}
		b.WriteByte('\n')
	}
	if err := os.WriteFile(out, []byte(b.String()), 0o644); err != nil {
		t.Fatal(err)
	}
}
