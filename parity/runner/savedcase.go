package runner

import (
	"encoding/json"
	"fmt"

	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/report"
)

// SchemaSavedCase names case.json, the runner's copy of a case that did not
// pass: everything needed to run it again (minimization, debugging).
const SchemaSavedCase = "pbs-parity-runner-case/1"

// SavedCase is case.json.
type SavedCase struct {
	Schema    string       `json:"schema"`
	RunCaseID string       `json:"run_case_id"`
	Base      string       `json:"base_profile"`
	Batch     string       `json:"batch"`
	Intro     bool         `json:"introspection,omitempty"`
	Case      *corpus.Case `json:"case"`
	// StoredBytes holds the case's stored payloads, base64-encoded, by kind
	// (requests, imps, responses, accounts) and ID. Case.Stored is empty in
	// the file and restored from here on load: encoding/json would re-indent
	// valid payloads and refuses malformed ones (the malformed-account
	// sample), and the servers must be given the exact fixture bytes.
	StoredBytes map[string]map[string][]byte `json:"stored_bytes,omitempty"`
}

// newSavedCase builds case.json for a run case.
func newSavedCase(rc *runCase, batch string) SavedCase {
	cp := *rc.Case
	var raw map[string]map[string][]byte
	cp.Stored, raw = splitStored(rc.Case.Stored)
	return SavedCase{Schema: SchemaSavedCase, RunCaseID: rc.ID, Base: rc.Base, Batch: batch, Intro: rc.Intro, Case: &cp, StoredBytes: raw}
}

// readSavedCase reads a case.json and restores its stored payloads.
func readSavedCase(path string) (*SavedCase, error) {
	var saved SavedCase
	if err := report.ReadJSON(path, &saved); err != nil {
		return nil, err
	}
	if saved.Schema != SchemaSavedCase || saved.Case == nil {
		return nil, fmt.Errorf("runner: %s is not a saved case", path)
	}
	for _, f := range storedFields(&saved.Case.Stored) {
		for id, raw := range saved.StoredBytes[f.kind] {
			if *f.set == nil {
				*f.set = map[string]json.RawMessage{}
			}
			(*f.set)[id] = json.RawMessage(raw)
		}
	}
	saved.StoredBytes = nil
	return &saved, nil
}

// storedField is one stored-data kind of a corpus.Stored.
type storedField struct {
	kind string
	set  *map[string]json.RawMessage
}

func storedFields(s *corpus.Stored) []storedField {
	return []storedField{{"requests", &s.Requests}, {"imps", &s.Imps}, {"responses", &s.Responses}, {"accounts", &s.Accounts}}
}

// splitStored moves every stored payload out of s into a byte map.
func splitStored(s corpus.Stored) (corpus.Stored, map[string]map[string][]byte) {
	var raw map[string]map[string][]byte
	for _, f := range storedFields(&s) {
		for id, payload := range *f.set {
			if raw == nil {
				raw = map[string]map[string][]byte{}
			}
			if raw[f.kind] == nil {
				raw[f.kind] = map[string][]byte{}
			}
			raw[f.kind][id] = append([]byte(nil), payload...)
		}
	}
	return corpus.Stored{}, raw
}
