package rhgate_test

import (
	"bytes"
	"errors"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity/replayharness/rhgate"
)

func result(id, endpoint string, b, a int, findings ...rhgate.Finding) rhgate.Result {
	r := rhgate.Result{
		Scenario: rhgate.Scenario{ID: id, Endpoint: endpoint, Name: "probe " + id},
		Observed: []rhgate.Observed{{Status: b}},
		Findings: findings,
	}
	if a >= 0 {
		r.Baseline = &rhgate.Observed{Status: a}
	}

	return r
}

func report(baseline string, results ...rhgate.Result) *rhgate.Report {
	return &rhgate.Report{Tool: "replayharness", Version: "v1", Target: "http://b", Baseline: baseline, Results: results}
}

func serverError() rhgate.Finding {
	return rhgate.Finding{Check: "server-error", Severity: "error", Message: "500 on extreme input"}
}

func TestJudgeCountsDivergences(t *testing.T) {
	t.Parallel()

	shape := rhgate.Finding{Check: rhgate.CheckDivergence, Severity: "warning", Message: "response shape differs from the baseline: $.x is gone"}
	base := report("http://a",
		result("1", "GET /status", 200, 200),
		result("2", "GET /info/bidders", 200, 200, shape),
		result("3", rhgate.CookieSync, 200, 200, shape),
		result("4", "POST /openrtb2/auction", 0, 200),
	)

	v := rhgate.Judge(base, nil)
	if !v.Failed() || len(v.Divergences) != 2 || len(v.NotCompared) != 1 || len(v.Incomplete) != 0 {
		t.Fatalf("verdict %+v", v)
	}

	if v.Divergences[0].Endpoint != "GET /info/bidders" || !strings.Contains(v.Divergences[1].What, "side B did not answer") {
		t.Errorf("divergences %+v", v.Divergences)
	}

	var out bytes.Buffer
	if err := v.Write(&out); err != nil {
		t.Fatal(err)
	}

	if !strings.Contains(out.String(), "not compared: "+rhgate.NotComparedReason(rhgate.CookieSync)) {
		t.Errorf("the verdict does not give the reason a difference is not counted:\n%s", out.String())
	}
}

func findingsBase() *rhgate.Report {
	slow := rhgate.Finding{Check: "slow", Severity: "warning", Message: "slower than 2s"}

	return report("http://a",
		result("1", "GET /status", 500, 500, serverError()),
		result("2", "GET /static/{filepath}", 500, 500, serverError(), slow),
		result("3", "GET /info/bidders", 200, 200),
	)
}

func TestJudgeComparesFindings(t *testing.T) {
	t.Parallel()

	ref := report("", result("1", "GET /status", 500, -1, serverError()), result("2", "GET /static/{filepath}", 500, -1),
		result("3", "GET /info/bidders", 200, -1, serverError()))

	v := rhgate.Judge(findingsBase(), ref)
	if !v.Failed() || v.Shared != 1 || len(v.OnlyB) != 1 || len(v.OnlyA) != 1 || len(v.Divergences) != 0 {
		t.Fatalf("verdict %+v", v)
	}

	if v.OnlyB[0].Scenario != "2" || v.OnlyA[0].Scenario != "3" {
		t.Errorf("only B %+v, only A %+v", v.OnlyB, v.OnlyA)
	}
}

func TestJudgeLeavesTimingOut(t *testing.T) {
	t.Parallel()

	ref := report("", result("1", "", 500, -1, serverError()), result("2", "", 500, -1, serverError()), result("3", "", 200, -1))

	if v := rhgate.Judge(findingsBase(), ref); v.Failed() || v.Shared != 2 || len(v.Incomplete) != 0 {
		t.Errorf("verdict %+v", v)
	}
}

func TestJudgeIncomplete(t *testing.T) {
	t.Parallel()

	for name, tc := range map[string]struct{ base, ref *rhgate.Report }{
		"not a baseline run": {base: report("", result("1", "GET /status", 200, -1))},
		"no probes":          {base: report("http://a")},
		"reference lacks a probe": {
			base: report("http://a", result("1", "GET /status", 200, 200)),
			ref:  report("", result("2", "GET /status", 200, -1)),
		},
	} {
		if v := rhgate.Judge(tc.base, tc.ref); len(v.Incomplete) == 0 {
			t.Errorf("%s: verdict %+v is complete", name, v)
		}
	}
}

func TestRead(t *testing.T) {
	t.Parallel()

	rep, err := rhgate.Read(strings.NewReader(`{"tool":"replayharness","version":"v1","target":"t","baseline":"b",` +
		`"results":[{"scenario":{"id":"1","endpoint":"GET /status"},"observed":[{"status":200}],"baseline":{"status":200}}]}`))
	if err != nil || len(rep.Results) != 1 || rep.Results[0].Baseline.Status != 200 {
		t.Fatalf("Read: %+v, %v", rep, err)
	}

	for _, doc := range []string{`{"tool":"other"}`, `not json`} {
		if _, err := rhgate.Read(strings.NewReader(doc)); !errors.Is(err, rhgate.ErrBadReport) {
			t.Errorf("Read(%s): err %v, want ErrBadReport", doc, err)
		}
	}
}
