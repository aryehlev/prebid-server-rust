package corpus_test

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/replayharness/rhreplay"
)

const (
	rhUA      = "ReplayHarness/test"
	rhNotePfx = "from "
	rhTraffic = "traffic.jsonl"
	rhBodyCap = 64 << 10
	rhSandbox = `{"id":"rh-sandbox","privacy":{"privacysandbox":{"cookiedeprecation":{"enabled":true,"ttl_sec":86400}}}}`
	rhSetUID  = "/setuid"
	rhCustom  = "X-Custom"
	rhAuction = "/openrtb2/auction"
	rhStatus  = "/status"
	rhInfo    = "/info/bidders"
	rhReplays = `replays:
  - name: flow
    steps:
      - method: GET
        path: /setuid
        query: {bidder: appnexus, uid: rh}
        expect: {status: 200}
        save: {cookie: "header:Set-Cookie"}
      - method: GET
        path: /getuids
        headers: {Cookie: "{{cookie}}"}
  - name: dup
    steps:
      - {method: GET, path: /status}
  - name: unique
    steps:
      - {method: GET, path: "/x/{{unique}}"}
`
	rhAPI = `endpoints:
  - {method: GET, path: /status}
  - method: GET
    path: /debug/pprof/
    rules: ["admin port only (admin_port), any method"]
  - {method: GET, path: "/info/bidders/{bidderName}"}
`
)

var rhCaseID = regexp.MustCompile(`^parity/replayharness#[a-z]+:/[^:]*:[0-9a-f]{16}$`)

type rhLine struct {
	Request  rhreplay.Request  `json:"request"`
	Response rhreplay.Response `json:"response"`
}

func rhReq(method, path, query, body string, headers map[string]string) rhLine {
	h := map[string]string{"User-Agent": rhUA}
	maps.Copy(h, headers)

	return rhLine{Request: rhreplay.Request{Method: method, Path: path, Query: query, Headers: h, Body: body}, Response: rhreplay.Response{Status: 200}}
}

func rhJSONLines(t *testing.T, lines []rhLine) string {
	t.Helper()

	var b strings.Builder

	for _, l := range lines {
		raw, err := json.Marshal(l)
		if err != nil {
			t.Fatal(err)
		}

		b.Write(raw)
		b.WriteByte('\n')
	}

	return b.String()
}

// rhTrafficLines is the synthetic traffic.jsonl: one request of each kind the
// loader sorts out.
func rhTrafficLines(custom string) []rhLine {
	binary := rhReq("POST", "/event", "", base64.StdEncoding.EncodeToString([]byte{0xff, 0xfe}), nil)
	binary.Request.BodyEncoding = "base64"

	return []rhLine{
		rhReq("GET", rhStatus, "", "", nil),
		rhReq("GET", rhStatus, "", "", nil),
		rhReq("GET", rhInfo, "x=1", "", map[string]string{
			"Connection": "close", "Accept-Encoding": "gzip", parity.TokenHeader: "secret", rhCustom: custom,
		}),
		rhReq("GET", rhSetUID, "bidder=appnexus&uid=a", "", nil),
		rhReq("POST", "/cookie_sync", "", `{}`, nil),
		rhReq("POST", "/cookie_sync", "", `{"bidders":["appnexus"]}`, nil),
		rhReq("POST", "/cookie_sync", "", `{"Account":"rh-sandbox","bidders":["appnexus"]}`, nil),
		rhReq("POST", rhAuction, "", strings.Repeat("a", 262145), nil),
		rhReq("GET", "/a b", "", "", nil),
		rhReq("POST", rhAuction, "", strings.Repeat("b", rhreplay.RecorderBodyLimit), nil),
		binary,
		rhReq("POST", rhAuction, "", `{"id":"r","imp":[{"id":"imp-1"}]}`, nil),
	}
}

// rhRepo writes a repository with a synthetic parity/replayharness.
func rhRepo(t *testing.T, custom string) *corpus.Repo {
	t.Helper()

	saved := rhReq("GET", rhSetUID, "bidder=appnexus&uid=rh", "", map[string]string{"User-Agent": "ReplayHarness/other"})
	saved.Response.Headers = map[string]string{"Set-Cookie": "uids=abc; Path=/"}

	root := t.TempDir()
	files := map[string]string{
		"go.mod":                                               "module github.com/prebid/prebid-server/v4\n",
		"parity/replayharness/" + rhTraffic:                    rhJSONLines(t, rhTrafficLines(custom)),
		"parity/replayharness/saved.jsonl":                     rhJSONLines(t, []rhLine{saved}),
		"parity/replayharness/replays/flow.yaml":               rhReplays,
		"parity/replayharness/api.yaml":                        rhAPI,
		"parity/replayharness/stored/accounts/rh-sandbox.json": rhSandbox,
	}

	for rel, data := range files {
		p := filepath.Join(root, filepath.FromSlash(rel))
		if err := os.MkdirAll(filepath.Dir(p), 0o750); err != nil {
			t.Fatal(err)
		}

		if err := os.WriteFile(p, []byte(data), 0o600); err != nil {
			t.Fatal(err)
		}
	}

	repo, err := corpus.OpenRepo(root)
	if err != nil {
		t.Fatal(err)
	}

	return repo
}

func rhLoad(t *testing.T, repo *corpus.Repo) *corpus.ReplayHarness {
	t.Helper()

	rh := corpus.LoadReplayHarness(repo)
	for _, err := range rh.Errors {
		t.Errorf("loading: %v", err)
	}

	return rh
}

// rhFind returns the case whose envelope sends method and path.
func rhFind(t *testing.T, rh *corpus.ReplayHarness, method, path string) corpus.Case {
	t.Helper()

	for _, cs := range rh.Cases {
		if r := cs.Envelope.Request; r.Method == method && r.Path == path {
			return cs
		}
	}

	t.Fatalf("no case sends %s %s", method, path)

	return corpus.Case{}
}

func TestReplayHarnessSortsOutTheRequests(t *testing.T) {
	t.Parallel()

	rh := rhLoad(t, rhRepo(t, " v "))

	tags := map[string]int{}

	for _, nc := range rh.NotCompared {
		if nc.Reason == "" || len(nc.Sources) == 0 {
			t.Errorf("not compared without a reason or a source: %+v", nc)
		}

		tags[nc.Tag]++
	}

	want := map[string]int{
		corpus.RHWallClock: 3, corpus.RHMapOrder: 1, corpus.RHExecFraming: 1, corpus.RHRequestLine: 1,
		corpus.RHTruncated: 1, corpus.RHNotRebuilt: 1, corpus.RHSidePort: 1,
	}
	if fmt.Sprint(tags) != fmt.Sprint(want) {
		t.Errorf("not compared by tag %v, want %v", tags, want)
	}

	// /status, /info/bidders, cookie_sync with bidders, /event, the auction
	// and /getuids with the saved cookie.
	if len(rh.Cases) != 6 {
		t.Errorf("%d cases, want 6", len(rh.Cases))
	}
}

func TestReplayHarnessCaseEnvelopes(t *testing.T) {
	t.Parallel()

	for _, cs := range rhLoad(t, rhRepo(t, " v ")).Cases {
		env := cs.Envelope
		if !rhCaseID.MatchString(cs.ID) || env.CaseID != cs.ID || cs.Corpus != corpus.CorpusReplayHarness ||
			cs.FixturePath != corpus.RootReplayHarness || len(env.Decisions) != len(parity.AllDecisions()) {
			t.Errorf("case %s: envelope %+v", cs.ID, env)
		}
	}
}

func TestReplayHarnessKeepsRequestsExactly(t *testing.T) {
	t.Parallel()

	rh := rhLoad(t, rhRepo(t, " v "))

	status := rhFind(t, rh, "GET", rhStatus)
	sources := []string{rhNotePfx + rhTraffic + ":1", rhNotePfx + rhTraffic + ":2", rhNotePfx + `replays/flow.yaml: replay "dup", step 1`}

	for _, s := range sources {
		if !slices.Contains(status.Notes, s) {
			t.Errorf("the /status case does not note %q: %v", s, status.Notes)
		}
	}

	info := rhFind(t, rh, "GET", rhInfo)
	if got := fmt.Sprint(info.Envelope.Request.Headers); got != "[[User-Agent "+rhUA+"] ["+rhCustom+" v]]" || info.Envelope.Request.Query != "x=1" {
		t.Errorf("the /info/bidders case sends %s ? %s", got, info.Envelope.Request.Query)
	}

	if !strings.Contains(strings.Join(info.Notes, "\n"), "Accept-Encoding, Connection, "+parity.TokenHeader) {
		t.Errorf("the /info/bidders case does not note the headers it leaves out: %v", info.Notes)
	}
}

func TestReplayHarnessKeepsBodiesAndSavedValues(t *testing.T) {
	t.Parallel()

	rh := rhLoad(t, rhRepo(t, " v "))

	if body := rhFind(t, rh, "POST", "/event").Envelope.Request.Body; body == nil || body.B64 == nil || body.Text != nil ||
		*body.B64 != base64.StdEncoding.EncodeToString([]byte{0xff, 0xfe}) {
		t.Errorf("the binary body is not kept as base64: %+v", body)
	}

	if got := rhFind(t, rh, "GET", "/getuids").Envelope.Request.Headers; !slices.Contains(got, [2]string{"Cookie", "uids=abc; Path=/"}) {
		t.Errorf("the step using the saved cookie sends %v", got)
	}

	if auction := rhFind(t, rh, "POST", rhAuction); !slices.Equal(auction.Mocks.ImpIDs, []string{"imp-1"}) {
		t.Errorf("auction imp IDs %v", auction.Mocks.ImpIDs)
	}
}

func TestReplayHarnessIDsAreStableAndContentAddressed(t *testing.T) {
	t.Parallel()

	ids := func(rh *corpus.ReplayHarness) []string {
		out := make([]string, 0, len(rh.Cases))
		for _, cs := range rh.Cases {
			out = append(out, cs.ID)
		}

		return out
	}

	repo := rhRepo(t, " v ")
	first, again := rhLoad(t, repo), rhLoad(t, repo)

	if !slices.Equal(ids(first), ids(again)) {
		t.Errorf("IDs change between loads: %v, %v", ids(first), ids(again))
	}

	changed := rhLoad(t, rhRepo(t, "w"))
	if rhFind(t, first, "GET", rhStatus).ID != rhFind(t, changed, "GET", rhStatus).ID {
		t.Error("the /status case ID depends on another request")
	}

	if rhFind(t, first, "GET", rhInfo).ID == rhFind(t, changed, "GET", rhInfo).ID {
		t.Error("the /info/bidders case ID does not change with its header")
	}
}

func TestReplayHarnessWithoutRecordingsIsEmpty(t *testing.T) {
	t.Parallel()

	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "go.mod"), []byte("module github.com/prebid/prebid-server/v4\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	repo, err := corpus.OpenRepo(root)
	if err != nil {
		t.Fatal(err)
	}

	if rh := corpus.LoadReplayHarness(repo); len(rh.Cases)+len(rh.NotCompared)+len(rh.Errors) != 0 {
		t.Errorf("corpus %+v", rh)
	}
}

// rhSources maps every source of the real corpus to all the sources of its
// request, and says whether the request is a case.
func rhSources(rh *corpus.ReplayHarness) (map[string][]string, map[string]bool) {
	siblings, isCase := map[string][]string{}, map[string]bool{}

	for _, cs := range rh.Cases {
		var all []string

		for _, n := range cs.Notes {
			if s, ok := strings.CutPrefix(n, rhNotePfx); ok {
				all = append(all, s)
			}
		}

		for _, s := range all {
			siblings[s], isCase[s] = all, true
		}
	}

	for _, nc := range rh.NotCompared {
		for _, s := range nc.Sources {
			siblings[s] = nc.Sources
		}
	}

	return siblings, isCase
}

func TestReplayHarnessCoversTheRecording(t *testing.T) {
	t.Parallel()

	repo, err := corpus.FindRepo(".")
	if err != nil {
		t.Fatal(err)
	}

	root, err := os.OpenRoot(filepath.Join(repo.Root(), filepath.FromSlash(corpus.RootReplayHarness)))
	if err != nil {
		t.Fatal(err)
	}

	defer func() { _ = root.Close() }()

	raw, err := root.ReadFile(rhTraffic)
	if err != nil {
		t.Fatal(err)
	}

	exs, err := rhreplay.ReadRecording(strings.NewReader(string(raw)))
	if err != nil {
		t.Fatal(err)
	}

	siblings, _ := rhSources(rhLoad(t, repo))

	for _, ex := range exs {
		if _, ok := siblings[rhTraffic+":"+strconv.Itoa(ex.Line)]; !ok {
			t.Errorf("%s:%d is neither a case nor listed as not compared", rhTraffic, ex.Line)
		}
	}
}

// rhStepSize is the size of a step's body as ReplayHarness sends it.
func rhStepSize(t *testing.T, st rhreplay.Step) int {
	t.Helper()

	if st.Body != nil {
		return len(*st.Body)
	}

	raw, err := json.Marshal(st.JSON)
	if err != nil {
		t.Fatal(err)
	}

	return len(raw)
}

func TestReplayHarnessRebuildsWhatReplayHarnessSent(t *testing.T) {
	t.Parallel()

	repo, err := corpus.FindRepo(".")
	if err != nil {
		t.Fatal(err)
	}

	root, err := os.OpenRoot(filepath.Join(repo.Root(), filepath.FromSlash(corpus.RootReplayHarness)))
	if err != nil {
		t.Fatal(err)
	}

	defer func() { _ = root.Close() }()

	replays, err := rhreplay.LoadDir(root.FS(), "replays")
	if err != nil {
		t.Fatal(err)
	}

	siblings, isCase := rhSources(rhLoad(t, repo))

	for _, r := range replays {
		for i, st := range r.Steps {
			rhCheckStep(t, fmt.Sprintf("replays/%s: replay %q, step %d", r.File, r.Name, i+1), st, siblings, isCase)
		}
	}
}

// rhCheckStep checks that the corpus holds step src and that, when the step
// is a case, rhreplay rebuilt the very request traffic.jsonl recorded for it.
// The recorder drops Cookie headers and traffic.jsonl drops bodies over
// 64 KiB, so only those steps may be missing from the recording.
func rhCheckStep(t *testing.T, src string, st rhreplay.Step, siblings map[string][]string, isCase map[string]bool) {
	t.Helper()

	all, ok := siblings[src]
	if !ok {
		t.Errorf("%s is neither a case nor listed as not compared", src)

		return
	}

	recorded := slices.ContainsFunc(all, func(s string) bool { return strings.HasPrefix(s, rhTraffic+":") })
	_, cookie := st.Headers["Cookie"]

	if isCase[src] && !recorded && !cookie && rhStepSize(t, st) <= rhBodyCap {
		t.Errorf("%s: rhreplay does not rebuild the request traffic.jsonl recorded for it", src)
	}
}

func TestReplayHarnessScriptBuildsThePinnedCommit(t *testing.T) {
	t.Parallel()

	repo, err := corpus.FindRepo(".")
	if err != nil {
		t.Fatal(err)
	}

	root, err := os.OpenRoot(repo.Root())
	if err != nil {
		t.Fatal(err)
	}

	defer func() { _ = root.Close() }()

	script, err := root.ReadFile("parity/scripts/replayharness.sh")
	if err != nil {
		t.Fatal(err)
	}

	if !strings.Contains(string(script), "\nrh_commit="+rhreplay.PinnedCommit+"\n") {
		t.Errorf("parity/scripts/replayharness.sh does not build rhreplay.PinnedCommit %s", rhreplay.PinnedCommit)
	}
}
