package runner

import (
	"encoding/json"
	"fmt"
	"path"
	"regexp"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// Synthetic introspection cases: one per batch, comparing GET /__parity/config
// and GET /__parity/bidders.
const (
	CorpusIntrospection = "introspection"
	KindIntrospection   = "introspection"
	endpointConfig      = "/__parity/config"
)

// Reasons recorded for cases that are selected but not run.
const (
	reasonContractGap = "tagged contract_gap: its faithful encoding is not yet accepted by the parity schemas"
	reasonBudget      = "run budget exhausted before the case started"
)

// runCase is one case of a run.
type runCase struct {
	// ID is the run case ID: the corpus case ID, suffixed with @<profile>
	// when the base profile is not the default one.
	ID string
	// Base is the base profile fixture name.
	Base string
	// Case is a copy of the corpus case with its IDs rewritten to ID.
	Case *corpus.Case
	// Intro marks the synthetic introspection case of a batch.
	Intro bool
	// Bidder is the adapter of adapter and lifted cases.
	Bidder string
}

// batch is a set of cases that share one server configuration: the base
// profile, the case profile and non-conflicting stored data.
type batch struct {
	Name    string
	Base    string
	Profile corpus.Profile
	Stored  corpus.Stored
	Cases   []*runCase
}

// Label is the profile label recorded on the batch's cases.
func (b *batch) Label() string { return profileLabel(b.Base, b.Profile) }

// Selection filters corpus cases.
type Selection struct {
	// Corpora are corpus names (adapters, sample-requests, exchangetest,
	// bidder-params, replayharness, introspection, gen, or all); empty means
	// all.
	Corpora []string
	// Kinds are case kinds; empty means all.
	Kinds []string
	// Match keeps case IDs matching the expression; nil keeps all.
	Match *regexp.Regexp
	// Limit caps the selected cases per base profile; <= 0 means no cap.
	Limit int
}

func (s Selection) wantsCorpus(name string) bool {
	return len(s.Corpora) == 0 || containsString(s.Corpora, name) || containsString(s.Corpora, "all")
}

func (s Selection) wants(cs *corpus.Case) bool { return s.wantsEntry(cs.Corpus, cs.Kind, cs.ID) }

// wantsEntry applies the corpus, kind and match filters to a case or a
// not-compared request.
func (s Selection) wantsEntry(corpusName string, kind corpus.Kind, id string) bool {
	if !s.wantsCorpus(corpusName) {
		return false
	}
	if len(s.Kinds) > 0 && !containsString(s.Kinds, string(kind)) {
		return false
	}
	return s.Match == nil || s.Match.MatchString(id)
}

// selectCases returns the cases to run under each base profile, in base
// profile then case ID order, and the selected cases that must be skipped
// (with their reason). Adapter cases do not depend on host configuration, so
// they run under the first base profile only.
func selectCases(c *corpus.Corpus, bases []string, sel Selection) (run []*runCase, skipped []skippedCase) {
	for bi, base := range bases {
		n := 0
		for i := range c.Cases {
			cs := &c.Cases[i]
			if !sel.wants(cs) || (cs.Kind == corpus.KindAdapter && bi > 0) {
				continue
			}
			if sel.Limit > 0 && n == sel.Limit {
				break
			}
			n++
			rc := newRunCase(cs, base)
			if cs.HasTag(corpus.TagContractGap) {
				skipped = append(skipped, skippedCase{rc: rc, reason: reasonContractGap})
				continue
			}
			run = append(run, rc)
		}
	}
	return run, skipped
}

type skippedCase struct {
	rc     *runCase
	reason string
}

// notComparedCase is a request a corpus leaves out of its cases, under one
// base profile.
type notComparedCase struct {
	// ID is the case ID the request would have, suffixed as a case's is.
	ID   string
	Base string
	NC   *corpus.NotCompared
}

// selectNotCompared returns the not-compared requests the selection names,
// under each base profile, in base profile then ID order. The corpus, kind
// and match filters apply as they do to cases; the limit does not, because
// none of them runs.
func selectNotCompared(c *corpus.Corpus, bases []string, sel Selection) []notComparedCase {
	var out []notComparedCase
	for _, base := range bases {
		for i := range c.NotCompared {
			nc := &c.NotCompared[i]
			if !sel.wantsEntry(nc.Corpus, nc.Kind, nc.ID) {
				continue
			}
			id := nc.ID
			if base != DefaultProfile {
				id += "@" + base
			}
			out = append(out, notComparedCase{ID: id, Base: base, NC: nc})
		}
	}
	return out
}

// newRunCase copies cs and rewrites its IDs for the base profile.
func newRunCase(cs *corpus.Case, base string) *runCase {
	cp := cloneCase(cs)
	id := cs.ID
	if base != DefaultProfile {
		id += "@" + base
	}
	rewriteCaseID(cp, id)
	return &runCase{ID: id, Base: base, Case: cp, Bidder: caseBidder(cs)}
}

// cloneCase copies the parts of a case rewriteCaseID changes; everything
// else (stored data, mocks, expectations) is shared and read-only. It does
// not round-trip through JSON: stored payloads may be deliberately malformed.
func cloneCase(cs *corpus.Case) *corpus.Case {
	cp := *cs
	if cs.Envelope != nil {
		env := *cs.Envelope
		cp.Envelope = &env
	}
	if cs.Adapter != nil {
		ad := *cs.Adapter
		ad.Bids = append([]parity.AdapterBidsInput(nil), cs.Adapter.Bids...)
		cp.Adapter = &ad
	}
	return &cp
}

// rewriteCaseID sets the case ID everywhere a server or the oracle sees it.
func rewriteCaseID(cs *corpus.Case, id string) {
	old := cs.ID
	cs.ID = id
	if cs.Envelope != nil {
		cs.Envelope.CaseID = id
	}
	if cs.Adapter != nil {
		cs.Adapter.Requests.CaseID = id
		for i := range cs.Adapter.Bids {
			cs.Adapter.Bids[i].CaseID = strings.Replace(cs.Adapter.Bids[i].CaseID, old, id, 1)
		}
	}
}

// caseBidder is the adapter an adapter or lifted case exercises.
func caseBidder(cs *corpus.Case) string {
	switch cs.Kind {
	case corpus.KindAdapter:
		if cs.Adapter != nil {
			return cs.Adapter.Requests.Build.Bidder
		}
	case corpus.KindLifted:
		for _, m := range cs.Mocks.HTTP {
			if m.Bidder != "" {
				return m.Bidder
			}
		}
		if parts := strings.Split(path.Clean(cs.FixturePath), "/"); len(parts) > 2 && parts[0] == "adapters" {
			return parts[1]
		}
	}
	return ""
}

// planBatches groups cases by base profile and case profile, splitting a
// group when two of its cases use one stored ID with different content.
// Batches keep the order in which their first case appears.
func planBatches(cases []*runCase) []*batch {
	var out []*batch
	byKey := map[string][]*batch{}
	for _, rc := range cases {
		key := rc.Base + "/" + rc.Case.Profile.Name()
		var target *batch
		for _, b := range byKey[key] {
			if !storedConflicts(b.Stored, rc.Case.Stored) {
				target = b
				break
			}
		}
		if target == nil {
			name := key
			if n := len(byKey[key]); n > 0 {
				name = fmt.Sprintf("%s#%d", key, n+1)
			}
			target = &batch{Name: name, Base: rc.Base, Profile: rc.Case.Profile}
			byKey[key] = append(byKey[key], target)
			out = append(out, target)
		}
		addStored(&target.Stored, rc.Case.Stored)
		target.Cases = append(target.Cases, rc)
	}
	return out
}

// addIntrospection appends the introspection case of every batch.
func addIntrospection(batches []*batch, profiles Profiles) {
	for _, b := range batches {
		id := "introspection:" + b.Name
		fixture := ""
		if f, ok := profiles[b.Base]; ok {
			fixture = f.Path
		}
		cs := &corpus.Case{
			ID: id, Kind: corpus.Kind(KindIntrospection), Corpus: CorpusIntrospection, FixturePath: fixture,
			Endpoint: endpointConfig, Profile: b.Profile,
			Description: "GET /__parity/config and /__parity/bidders of the batch's servers",
		}
		b.Cases = append(b.Cases, &runCase{ID: id, Base: b.Base, Case: cs, Intro: true})
	}
}

func storedConflicts(have, add corpus.Stored) bool {
	pairs := [][2]map[string]json.RawMessage{
		{have.Requests, add.Requests}, {have.Imps, add.Imps}, {have.Responses, add.Responses}, {have.Accounts, add.Accounts},
	}
	for _, p := range pairs {
		for id, v := range p[1] {
			if cur, ok := p[0][id]; ok && string(cur) != string(v) {
				return true
			}
		}
	}
	return false
}

func addStored(dst *corpus.Stored, src corpus.Stored) {
	merge := func(d *map[string]json.RawMessage, s map[string]json.RawMessage) {
		for id, v := range s {
			if *d == nil {
				*d = map[string]json.RawMessage{}
			}
			(*d)[id] = v
		}
	}
	merge(&dst.Requests, src.Requests)
	merge(&dst.Imps, src.Imps)
	merge(&dst.Responses, src.Responses)
	merge(&dst.Accounts, src.Accounts)
}

// knownInputs are the caller-supplied documents a case's servers read: its
// stored data and default request (UUIDs in them are never "generated").
func knownInputs(cs *corpus.Case) [][]byte {
	var out [][]byte
	for _, set := range []map[string]json.RawMessage{cs.Stored.Requests, cs.Stored.Imps, cs.Stored.Responses, cs.Stored.Accounts} {
		for _, id := range sortedKeys(set) {
			out = append(out, set[id])
		}
	}
	if len(cs.Profile.DefaultRequest) > 0 {
		out = append(out, cs.Profile.DefaultRequest)
	}
	return out
}

func containsString(list []string, s string) bool {
	for _, v := range list {
		if v == s {
			return true
		}
	}
	return false
}
