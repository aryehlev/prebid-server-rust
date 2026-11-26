package arbiter

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"testing/synctest"
	"time"
)

var update = flag.Bool("update", false, "rewrite ../../golden/gometrics/ticks.txt")

// arbiterBusy is how long each pass of the arbiter's loop ticks the meters: none, just under, at
// and over the five-second period, over several periods, and a nanosecond.
var arbiterBusy = []time.Duration{
	0, 4999 * time.Millisecond, 5 * time.Second, 12 * time.Second, 0, 15 * time.Second, 1,
	27500 * time.Millisecond, 4 * time.Second, 5*time.Second + 1, 0,
}

// TestArbiterTicks runs the loop of the go-metrics meter arbiter (meterArbiter.tick:
// "for { <-ticker.C; tickMeters() }" over time.NewTicker(5e9)) with tickMeters taking the
// arbiterBusy durations, and compares when it takes its ticks with ticks.txt: "<busy> <taken
// at>" rows in nanoseconds, the second since the ticker was made. A tick that falls due while
// the loop is busy is held and taken at once; the ones after it are dropped. Run with
// -args -update to rewrite the file.
func TestArbiterTicks(t *testing.T) {
	var b strings.Builder
	synctest.Test(t, func(t *testing.T) {
		start := time.Now()
		ticker := time.NewTicker(5 * time.Second)
		defer ticker.Stop()
		for _, busy := range arbiterBusy {
			<-ticker.C
			fmt.Fprintf(&b, "%d\t%d\n", int64(busy), int64(time.Since(start)))
			time.Sleep(busy)
		}
	})
	path := filepath.Join("..", "..", "golden", "gometrics", "ticks.txt")
	if *update {
		if err := os.WriteFile(path, []byte(b.String()), 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if got := b.String(); got != string(want) {
		t.Errorf("the ticks differ from %s:\n%s", path, got)
	}
}
