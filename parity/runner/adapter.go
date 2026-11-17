package runner

import (
	"context"
	"encoding/json"
	"fmt"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/diff"
	"github.com/prebid/prebid-server/v4/parity/report"
)

// adapterOutputs are one side's adapter endpoint results for a case.
type adapterOutputs struct {
	requests *parity.AdapterRequestsOutput
	bids     []*parity.AdapterBidsOutput
	raw      [][]byte
	err      error
	what     string
}

// execAdapter runs an adapter fixture against both servers' pure adapter
// endpoints: MakeRequests once, then MakeBids once per fixture httpCall with
// the fixture's own request data and mock response (adapterstest inputs), so
// the two sides always receive identical inputs.
func (p *pair) execAdapter(ctx context.Context, rc *runCase) caseResult {
	ac := rc.Case.Adapter
	for _, s := range p.sides {
		if rc.Case.HasTag(corpus.TagNeedsFake) && !s.srv.Health.Fakes {
			return notInstrumented(fmt.Sprintf("%s (%s) has no deterministic fakes installed and the case is tagged needs_fake", s.impl, s.name))
		}
	}
	var outs [2]adapterOutputs
	p.runBothAdapter(func(i int) { outs[i] = callAdapter(ctx, p.sides[i], ac) })
	res := caseResult{files: adapterInputFiles(ac)}
	if p.r.opts.FixtureSelfTest {
		res.fixture = fixtureOutcome(rc, outs[0])
	}
	for i, o := range outs {
		for j, raw := range o.raw {
			res.files = append(res.files, caseFile{name: fmt.Sprintf("%s.%s.adapter-%d.json", p.sides[i].name, p.sides[i].impl, j), role: roleExtra, raw: raw})
			res.notes = append(res.notes, validationNotes(p.sides[i], raw)...)
		}
		if o.err != nil {
			out := sideFailure(p.sides[i], o.what, o.err)
			out.files, out.notes, out.fixture = res.files, res.notes, res.fixture
			return out
		}
	}
	doc, err := p.compareAdapter(rc, outs)
	if err != nil {
		out := failed("diff: %v", err)
		out.files, out.notes, out.fixture = res.files, res.notes, res.fixture
		return out
	}
	res.stagesA, res.stagesB = adapterStages(ac), adapterStages(ac)
	return withDiff(res, doc)
}

// runBothAdapter runs fn for both sides concurrently: adapter calls never
// reach the oracle, so two servers of one implementation cannot mix captures.
func (p *pair) runBothAdapter(fn func(i int)) {
	done := make(chan struct{}, len(p.sides))
	for i := range p.sides {
		go func(i int) {
			fn(i)
			done <- struct{}{}
		}(i)
	}
	for range p.sides {
		<-done
	}
}

// callAdapter calls one server's adapter endpoints for a case.
func callAdapter(ctx context.Context, s *side, ac *corpus.AdapterCase) adapterOutputs {
	bidder := ac.Requests.Build.Bidder
	var o adapterOutputs
	req, raw, err := s.srv.Client.AdapterRequests(ctx, bidder, &ac.Requests)
	o.raw = appendRaw(o.raw, raw)
	if err != nil {
		o.err, o.what = err, "adapter requests"
		return o
	}
	o.requests = req
	for i := range ac.Bids {
		bids, raw, err := s.srv.Client.AdapterBids(ctx, bidder, &ac.Bids[i])
		o.raw = appendRaw(o.raw, raw)
		if err != nil {
			o.err, o.what = err, fmt.Sprintf("adapter bids[%d]", i)
			return o
		}
		o.bids = append(o.bids, bids)
	}
	return o
}

func appendRaw(list [][]byte, raw []byte) [][]byte {
	if raw == nil {
		return list
	}
	return append(list, raw)
}

// compareAdapter diffs the MakeRequests output, then each MakeBids output
// (keyed bidder#i), into one document.
func (p *pair) compareAdapter(rc *runCase, outs [2]adapterOutputs) (parity.DiffDocument, error) {
	dc := diff.Case{CaseID: rc.ID, FixturePath: rc.Case.FixturePath, Endpoint: rc.Case.Endpoint, Profile: p.b.Label()}
	res, err := p.r.engine.CompareAdapterRequests(dc, outs[0].requests, outs[1].requests)
	if err != nil {
		return parity.DiffDocument{}, err
	}
	docs := []parity.DiffDocument{res.Doc}
	for i := range outs[0].bids {
		bres, err := p.r.engine.CompareAdapterBids(dc, outs[0].bids[i], outs[1].bids[i])
		if err != nil {
			return parity.DiffDocument{}, err
		}
		for j := range bres.Doc.Mismatches {
			bres.Doc.Mismatches[j].Key = fmt.Sprintf("%s#%d", rc.Case.Adapter.Requests.Build.Bidder, i)
		}
		docs = append(docs, bres.Doc)
	}
	return mergeDocs(docHead(rc, p.b.Label()), docs), nil
}

// adapterStages are the stages an adapter case captures on a side.
func adapterStages(ac *corpus.AdapterCase) map[parity.StageID]parity.StageStatus {
	st := map[parity.StageID]parity.StageStatus{parity.StageAdapterRequests: parity.StageCaptured}
	if len(ac.Bids) > 0 {
		st[parity.StageAdapterBids] = parity.StageCaptured
	}
	return st
}

// adapterInputFiles are the inputs both servers received.
func adapterInputFiles(ac *corpus.AdapterCase) []caseFile {
	files := []caseFile{{name: "adapter.requests.input.json", role: roleExtra, v: ac.Requests}}
	for i := range ac.Bids {
		files = append(files, caseFile{name: fmt.Sprintf("adapter.bids-%d.input.json", i), role: roleExtra, v: ac.Bids[i]})
	}
	return files
}

// execIntrospection compares GET /__parity/config and /__parity/bidders.
// Listener ports differ per process by construction; a port member is
// replaced by a placeholder only where it holds exactly the port the runner
// assigned to that process, so a wrong value still shows.
func (p *pair) execIntrospection(ctx context.Context, rc *runCase) caseResult {
	type dumps struct {
		config  *parity.ConfigDump
		bidders *parity.BiddersDump
		raw     [][]byte
		err     error
		what    string
	}
	var outs [2]dumps
	p.runBothAdapter(func(i int) {
		s := p.sides[i]
		cfg, raw, err := s.srv.Client.Config(ctx)
		outs[i].raw = appendRaw(outs[i].raw, raw)
		if err != nil {
			outs[i].err, outs[i].what = err, "config"
			return
		}
		outs[i].config = cfg
		bidders, raw, err := s.srv.Client.Bidders(ctx)
		outs[i].raw = appendRaw(outs[i].raw, raw)
		outs[i].err, outs[i].what, outs[i].bidders = err, "bidders", bidders
	})
	res := caseResult{}
	for i, o := range outs {
		for j, raw := range o.raw {
			res.files = append(res.files, caseFile{name: fmt.Sprintf("%s.%s.introspection-%d.json", p.sides[i].name, p.sides[i].impl, j), role: roleExtra, raw: raw})
			res.notes = append(res.notes, validationNotes(p.sides[i], raw)...)
		}
		if o.err != nil {
			out := sideFailure(p.sides[i], o.what, o.err)
			out.files, out.notes = res.files, res.notes
			return out
		}
		maskPorts(o.config, p.sides[i].srv.Ports)
	}
	dc := diff.Case{CaseID: rc.ID, FixturePath: rc.Case.FixturePath, Endpoint: rc.Case.Endpoint, Profile: p.b.Label()}
	cres, err := p.r.engine.CompareConfig(dc, outs[0].config, outs[1].config)
	if err != nil {
		return failed("diff config: %v", err)
	}
	bres, err := p.r.engine.CompareBidders(dc, outs[0].bidders, outs[1].bidders)
	if err != nil {
		return failed("diff bidders: %v", err)
	}
	stages := map[parity.StageID]parity.StageStatus{parity.StageConfigResolved: parity.StageCaptured, parity.StageBiddersResolved: parity.StageCaptured}
	res.stagesA, res.stagesB = stages, stages
	return withDiff(res, mergeDocs(docHead(rc, p.b.Label()), []parity.DiffDocument{cres.Doc, bres.Doc}))
}

// portPaths are the config dump members that hold the listener ports.
var portPaths = []struct {
	path []string
	port func(Ports) int
}{
	{[]string{"port"}, func(p Ports) int { return p.Main }},
	{[]string{"admin_port"}, func(p Ports) int { return p.Admin }},
	{[]string{"metrics", "prometheus", "port"}, func(p Ports) int { return p.Prometheus }},
}

// portPlaceholder replaces an assigned port in config dumps.
const portPlaceholder = "<runner-assigned-port>"

// maskPorts replaces the runner-assigned ports in a config dump. The dump is
// re-encoded whether or not a port matched, so both sides always get the
// same treatment (top-level key order, compaction) and the masking itself
// can never produce a difference. The config dump is a harness document,
// not product output, so its member order is not a parity property.
func maskPorts(dump *parity.ConfigDump, ports Ports) {
	if dump == nil || len(dump.Config) == 0 {
		return
	}
	var root map[string]json.RawMessage
	if json.Unmarshal(dump.Config, &root) != nil {
		return
	}
	for _, pp := range portPaths {
		replacePort(root, pp.path, pp.port(ports))
	}
	if raw, err := json.Marshal(root); err == nil {
		dump.Config = raw
	}
}

// replacePort replaces the number at path when it equals port.
func replacePort(obj map[string]json.RawMessage, path []string, port int) bool {
	raw, ok := obj[path[0]]
	if !ok {
		return false
	}
	if len(path) > 1 {
		var child map[string]json.RawMessage
		if json.Unmarshal(raw, &child) != nil || !replacePort(child, path[1:], port) {
			return false
		}
		enc, err := json.Marshal(child)
		if err != nil {
			return false
		}
		obj[path[0]] = enc
		return true
	}
	var n json.Number
	if json.Unmarshal(raw, &n) != nil || n.String() != fmt.Sprint(port) {
		return false
	}
	obj[path[0]] = json.RawMessage(`"` + portPlaceholder + `"`)
	return true
}

// fixtureOutcome is the fixture self-test of implementation A's outputs, or
// not-run when A did not serve the adapter endpoints.
func fixtureOutcome(rc *runCase, a adapterOutputs) *report.FixtureResult {
	f := &report.FixtureResult{CaseID: rc.ID, FixturePath: rc.Case.FixturePath, Bidder: rc.Bidder, Failures: []string{}}
	if a.requests == nil {
		f.Status = report.FixtureNotRun
		if a.err != nil {
			f.Failures = append(f.Failures, a.err.Error())
		}
		return f
	}
	f.Failures = CheckFixture(rc.Case.Adapter, a.requests, a.bids)
	f.Status = report.FixturePassing
	if len(f.Failures) > 0 {
		f.Status = report.FixtureFailing
		for _, tag := range []string{corpus.TagNeedsFake, corpus.TagUnresolvedBuild} {
			if rc.Case.HasTag(tag) {
				f.Status, f.Excuse = report.FixtureExcused, tag
				break
			}
		}
	}
	return f
}
