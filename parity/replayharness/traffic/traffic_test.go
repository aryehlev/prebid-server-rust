package traffic_test

import (
	"bytes"
	"errors"
	"io"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity/replayharness/traffic"
)

const (
	getStatus   = `{"time":"t1","request":{"method":"GET","path":"/status"},"response":{"status":204},"durationMs":1}`
	getStatus2  = `{"time":"t2","request":{"method":"GET","path":"/status"},"response":{"status":204},"durationMs":2}`
	postAuction = `{"request":{"method":"POST","path":"/openrtb2/auction","headers":{"Content-Type":"application/json"},` +
		`"body":"{\"id\":\"x\"}"},"response":{"status":400}}`
)

func compact(t *testing.T, opts traffic.Options, lines ...string) (string, traffic.Stats, error) {
	t.Helper()

	var out bytes.Buffer

	stats, err := traffic.Compact([]io.Reader{strings.NewReader(strings.Join(lines, "\n"))}, &out, opts)

	return out.String(), stats, err
}

func TestCompactKeepsFirstOfEachRequest(t *testing.T) {
	t.Parallel()

	out, stats, err := compact(t, traffic.Options{}, getStatus, "", getStatus2, postAuction)
	if err != nil {
		t.Fatalf("Compact: %v", err)
	}

	if want := getStatus + "\n" + postAuction + "\n"; out != want {
		t.Errorf("output:\n%s\nwant:\n%s", out, want)
	}

	if stats.Read != 3 || stats.Kept != 2 || stats.Duplicates != 1 || stats.Bytes != len(out) {
		t.Errorf("stats %+v", stats)
	}
}

func TestCompactHeaderNamesAreCaseInsensitive(t *testing.T) {
	t.Parallel()

	a := `{"request":{"method":"GET","path":"/status","headers":{"accept":"text/plain"}}}`
	b := `{"request":{"method":"GET","path":"/status","headers":{"Accept":"text/plain"}}}`

	_, stats, err := compact(t, traffic.Options{}, a, b)
	if err != nil {
		t.Fatalf("Compact: %v", err)
	}

	if stats.Kept != 1 || stats.Duplicates != 1 {
		t.Errorf("stats %+v", stats)
	}
}

func TestCompactDropsLargeRequestBodies(t *testing.T) {
	t.Parallel()

	big := `{"request":{"method":"POST","path":"/vtrack","body":"` + strings.Repeat("a", 100) + `"}}`

	out, stats, err := compact(t, traffic.Options{MaxRequestBody: 99}, big, postAuction)
	if err != nil {
		t.Fatalf("Compact: %v", err)
	}

	if out != postAuction+"\n" || stats.LargeRequests != 1 || stats.Kept != 1 {
		t.Errorf("output %q, stats %+v", out, stats)
	}
}

func TestCompactKeepsLargeResponseOncePerEndpoint(t *testing.T) {
	t.Parallel()

	body := strings.Repeat("s", 64)
	first := `{"request":{"method":"GET","path":"/bidders/params"},"response":{"status":200,"body":"` + body + `"}}`
	sameBody := `{"request":{"method":"GET","path":"/bidders/params","query":"x=1"},"response":{"status":200,"body":"` + body + `"}}`
	otherPath := `{"request":{"method":"GET","path":"/info/bidders/all"},"response":{"status":200,"body":"` + body + `"}}`
	small := `{"request":{"method":"GET","path":"/bidders/params","query":"y=1"},"response":{"status":200,"body":"s"}}`

	out, stats, err := compact(t, traffic.Options{LargeResponse: 64}, first, sameBody, otherPath, small)
	if err != nil {
		t.Fatalf("Compact: %v", err)
	}

	if want := first + "\n" + otherPath + "\n" + small + "\n"; out != want {
		t.Errorf("output:\n%s\nwant:\n%s", out, want)
	}

	if stats.RepeatedResponses != 1 || stats.Kept != 3 {
		t.Errorf("stats %+v", stats)
	}
}

func TestCompactRefusesSecrets(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name string
		line string
	}{
		{"token value", `{"request":{"method":"GET","path":"/status?t=s3cret"}}`},
		{"cookie header", `{"request":{"method":"GET","path":"/getuids","headers":{"cookie":"uids=x"}}}`},
		{"parity token header", `{"request":{"method":"GET","path":"/status","headers":{"X-Parity-Token":"x"}}}`},
		{"set-cookie response", `{"request":{"method":"GET","path":"/setuid"},"response":{"headers":{"Set-Cookie":"uids=x"}}}`},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()

			out, _, err := compact(t, traffic.Options{Secrets: []string{"", "s3cret"}}, getStatus, tc.line)
			if !errors.Is(err, traffic.ErrSecret) {
				t.Errorf("err = %v, want ErrSecret", err)
			}

			if out != "" {
				t.Errorf("wrote %q after refusing", out)
			}
		})
	}
}

func TestCompactRefusesBadLines(t *testing.T) {
	t.Parallel()

	for _, line := range []string{`not json`, `{"request":{"path":"/status"}}`, `{"request":{"method":"GET","path":"status"}}`, `{}`} {
		if _, _, err := compact(t, traffic.Options{}, line); !errors.Is(err, traffic.ErrBadLine) {
			t.Errorf("%s: err = %v, want ErrBadLine", line, err)
		}
	}
}

func TestCompactEnforcesTheSizeCap(t *testing.T) {
	t.Parallel()

	out, stats, err := compact(t, traffic.Options{MaxBytes: len(getStatus)}, getStatus, postAuction)
	if !errors.Is(err, traffic.ErrTooLarge) {
		t.Fatalf("err = %v, want ErrTooLarge", err)
	}

	if out != "" || stats.Kept != 2 {
		t.Errorf("output %q, stats %+v", out, stats)
	}
}
