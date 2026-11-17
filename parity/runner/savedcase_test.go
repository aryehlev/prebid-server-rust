package runner

import (
	"encoding/json"
	"path/filepath"
	"reflect"
	"testing"

	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/report"
)

func TestSavedCaseRoundTrip(t *testing.T) {
	tests := []struct {
		name      string
		stored    corpus.Stored
		wantBytes []string
	}{
		{"no stored data", corpus.Stored{}, nil},
		{"payloads keep their exact bytes", corpus.Stored{
			Requests: map[string]json.RawMessage{"r": json.RawMessage(`{"id":"r"}`)},
			Accounts: map[string]json.RawMessage{"bad": json.RawMessage(`{"id":"bad"},}`), "ok": json.RawMessage(`{"id":"ok"}`)},
			Imps:     map[string]json.RawMessage{"spaced": json.RawMessage("{ \"id\": \"i\" }\n")},
		}, []string{"accounts/bad", "accounts/ok", "imps/spaced", "requests/r"}},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			cs := envCase("c", corpus.CorpusSampleRequests, corpus.Profile{}, tc.stored)
			saved := newSavedCase(newRunCase(&cs, DefaultProfile), "default/default")
			var got []string
			for kind, set := range saved.StoredBytes {
				for id := range set {
					got = append(got, kind+"/"+id)
				}
			}
			if len(got) != len(tc.wantBytes) || (len(got) > 0 && !reflect.DeepEqual(sortedStrings(got), tc.wantBytes)) {
				t.Errorf("stored bytes = %v, want %v", got, tc.wantBytes)
			}
			path := filepath.Join(t.TempDir(), report.FileCase)
			if err := report.WriteJSON(path, saved); err != nil {
				t.Fatalf("WriteJSON: %v", err)
			}
			back, err := readSavedCase(path)
			if err != nil {
				t.Fatalf("readSavedCase: %v", err)
			}
			if !saved.Case.Stored.IsEmpty() {
				t.Errorf("case.json carries stored payloads inline: %+v", saved.Case.Stored)
			}
			if !reflect.DeepEqual(back.Case.Stored, tc.stored) {
				t.Errorf("stored = %+v, want %+v", back.Case.Stored, tc.stored)
			}
			if !reflect.DeepEqual(cs.Stored, tc.stored) {
				t.Error("newSavedCase mutated the case")
			}
		})
	}
}

func sortedStrings(in []string) []string {
	set := map[string]bool{}
	for _, s := range in {
		set[s] = true
	}
	return sortedKeys(set)
}

// TestEveryCorpusCaseCanBeSaved selects the whole repository corpus under
// two base profiles and encodes every case.json the runner could write.
func TestEveryCorpusCaseCanBeSaved(t *testing.T) {
	if testing.Short() {
		t.Skip("loads the whole corpus")
	}
	repo, err := corpus.OpenRepo(testRepo(t))
	if err != nil {
		t.Fatalf("OpenRepo: %v", err)
	}
	c, err := corpus.LoadAll(repo)
	if err != nil {
		t.Fatalf("LoadAll: %v", err)
	}
	run, skipped := selectCases(c, []string{DefaultProfile, "floors"}, Selection{})
	if len(run) == 0 {
		t.Fatal("no cases selected")
	}
	for _, s := range skipped {
		run = append(run, s.rc)
	}
	for _, b := range planBatches(run) {
		for _, rc := range b.Cases {
			if _, err := report.EncodeJSON(newSavedCase(rc, b.Name)); err != nil {
				t.Errorf("%s: %v", rc.ID, err)
			}
		}
	}
}
