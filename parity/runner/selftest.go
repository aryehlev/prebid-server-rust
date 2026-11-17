package runner

import (
	"context"
	"fmt"

	"github.com/prebid/prebid-server/v4/parity/report"
)

// Verdict is the outcome of a self-test, or of a run judged without a
// baseline.
type Verdict struct {
	// OK is true when the comparison found no mismatch and no harness error,
	// no fixture failed (excused ones aside) and something was actually
	// compared.
	OK bool `json:"ok"`
	// Incomplete is true when nothing could be compared because the server
	// is not parity-capable yet.
	Incomplete bool     `json:"incomplete"`
	Reasons    []string `json:"reasons"`
}

// SelfTest runs the A/A self-test (design section 8.3): implementation A
// against a second process of itself across the selected corpora, plus the
// adapter fixture self-test of A's adapter endpoints. Any Go-vs-Go mismatch
// is missing normalization or cross-case state in the harness itself.
func SelfTest(ctx context.Context, opts Options) (*Result, Verdict, error) {
	opts.Mode = report.ModeSelfTest
	opts.B = opts.A
	opts.FixtureSelfTest = true
	res, err := Run(ctx, opts)
	if err != nil {
		return nil, Verdict{}, err
	}
	return res, Judge(res.Run), nil
}

// Judge evaluates a self-test run, or a run that has no baseline to ratchet
// against: any mismatch fails it.
func Judge(run *report.Run) Verdict {
	v := Verdict{Reasons: []string{}}
	c := run.Summary.Counts
	if c.Mismatch > 0 {
		between := "two processes of " + run.Summary.ImplA
		if run.Summary.Mode != report.ModeSelfTest {
			between = run.Summary.ImplA + " and " + run.Summary.ImplB
		}
		v.Reasons = append(v.Reasons, fmt.Sprintf("%d cases mismatch between %s (%d clusters)", c.Mismatch, between, run.Summary.Clusters))
	}
	if c.Error > 0 {
		v.Reasons = append(v.Reasons, fmt.Sprintf("%d cases failed in the harness", c.Error))
	}
	if f := run.Summary.Fixtures; f != nil && f.Failing > 0 {
		v.Reasons = append(v.Reasons, fmt.Sprintf("%d adapter fixtures fail their own expectations (%d excused)", f.Failing, f.Excused))
	}
	if c.Compared() == 0 {
		v.Incomplete = true
		v.Reasons = append(v.Reasons, fmt.Sprintf("no case was compared: %d not-yet-instrumented, %d skipped", c.NotYetInstrumented, c.Skipped))
	}
	v.OK = len(v.Reasons) == 0
	return v
}
