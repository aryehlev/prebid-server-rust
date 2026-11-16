package parity

import (
	"encoding/json"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestCaptureStageFilter(t *testing.T) {
	tests := []struct {
		name    string
		stages  string
		want    map[StageID]bool
		wantErr string
	}{
		{name: "omitted means all", stages: "", want: nil},
		{name: "null means all", stages: "null", want: nil},
		{name: "star means all", stages: `"*"`, want: nil},
		{name: "list", stages: `["http.in","http.response"]`, want: map[StageID]bool{StageHTTPIn: true, StageHTTPResponse: true}},
		{name: "empty list captures nothing", stages: `[]`, want: map[StageID]bool{}},
		{name: "other string", stages: `"all"`, wantErr: `must be "*"`},
		{name: "wrong type", stages: `{"a":1}`, wantErr: "capture.stages"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			c := CaptureSpec{}
			if tt.stages != "" {
				c.Stages = json.RawMessage(tt.stages)
			}
			got, err := c.StageFilter()
			if tt.wantErr != "" {
				require.Error(t, err)
				assert.Contains(t, err.Error(), tt.wantErr)
				return
			}
			require.NoError(t, err)
			assert.Equal(t, tt.want, got)
		})
	}
}

func TestCaptureDefaults(t *testing.T) {
	no, yes := false, true
	tests := []struct {
		name          string
		spec          CaptureSpec
		wantMetrics   bool
		wantAnalytics bool
	}{
		{name: "defaults on", spec: CaptureSpec{}, wantMetrics: true, wantAnalytics: true},
		{name: "explicit off", spec: CaptureSpec{Metrics: &no, Analytics: &no}, wantMetrics: false, wantAnalytics: false},
		{name: "explicit on", spec: CaptureSpec{Metrics: &yes, Analytics: &yes}, wantMetrics: true, wantAnalytics: true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assert.Equal(t, tt.wantMetrics, tt.spec.WantsMetrics())
			assert.Equal(t, tt.wantAnalytics, tt.spec.WantsAnalytics())
		})
	}
}

func TestMockSpecDefaults(t *testing.T) {
	tests := []struct {
		name      string
		spec      MockSpec
		wantMode  string
		wantLevel string
	}{
		{name: "zero value", spec: MockSpec{}, wantMode: MockModeOracle, wantLevel: MockLevelHTTP},
		{name: "explicit", spec: MockSpec{Mode: MockModeInline, Level: MockLevelBidderResp}, wantMode: MockModeInline, wantLevel: MockLevelBidderResp},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assert.Equal(t, tt.wantMode, tt.spec.EffectiveMode())
			assert.Equal(t, tt.wantLevel, tt.spec.EffectiveLevel())
		})
	}
}

func TestEnvelopeDecisions(t *testing.T) {
	env := &Envelope{Decisions: map[string]json.RawMessage{
		string(DecisionFloorsSkip):      json.RawMessage(`1`),
		string(DecisionUsersyncShuffle): json.RawMessage(`"identity"`),
		string(DecisionFloorsEnforce):   json.RawMessage(`"oops"`),
	}}
	tests := []struct {
		name       string
		env        *Envelope
		point      DecisionID
		wantInt    int64
		wantIntOK  bool
		wantStr    string
		wantStrOK  bool
		checkIsStr bool
	}{
		{name: "int override", env: env, point: DecisionFloorsSkip, wantInt: 1, wantIntOK: true},
		{name: "string override", env: env, point: DecisionUsersyncShuffle, wantStr: "identity", wantStrOK: true, checkIsStr: true},
		{name: "wrong type is no override", env: env, point: DecisionFloorsEnforce, wantIntOK: false},
		{name: "absent", env: env, point: DecisionCategoryDedupe, wantIntOK: false},
		{name: "nil envelope", env: nil, point: DecisionFloorsSkip, wantIntOK: false},
		{name: "nil envelope string", env: nil, point: DecisionUsersyncShuffle, wantStrOK: false, checkIsStr: true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if tt.checkIsStr {
				got, ok := tt.env.DecisionString(tt.point)
				assert.Equal(t, tt.wantStrOK, ok)
				assert.Equal(t, tt.wantStr, got)
				return
			}
			got, ok := tt.env.DecisionInt(tt.point)
			assert.Equal(t, tt.wantIntOK, ok)
			assert.Equal(t, tt.wantInt, got)
		})
	}
}

func TestHeaderListValues(t *testing.T) {
	h := HeaderList{{"X-A", "1"}, {"Content-Type", "json"}, {"x-a", "2"}}
	tests := []struct {
		name string
		key  string
		want []string
	}{
		{name: "case-insensitive in order", key: "X-a", want: []string{"1", "2"}},
		{name: "single", key: "content-type", want: []string{"json"}},
		{name: "absent", key: "Accept", want: nil},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assert.Equal(t, tt.want, h.Values(tt.key))
		})
	}
}

func TestDomains(t *testing.T) {
	tests := []struct {
		domain Domain
		want   bool
	}{
		{domain: DomainEndpoints, want: true},
		{domain: DomainCutoverProof, want: true},
		{domain: Domain("payments"), want: false},
		{domain: Domain(""), want: false},
	}
	for _, tt := range tests {
		t.Run(string(tt.domain), func(t *testing.T) {
			assert.Equal(t, tt.want, tt.domain.Valid())
		})
	}
	all := AllDomains()
	all[0] = "mutated"
	assert.Equal(t, DomainEndpoints, AllDomains()[0], "AllDomains must return a fresh slice")
	assert.Len(t, AllDomains(), 9)
}

func TestRegistriesHaveNoDuplicates(t *testing.T) {
	tests := []struct {
		name string
		ids  []string
	}{
		{name: "stages", ids: stageIDs()},
		{name: "decisions", ids: decisionIDs()},
		{name: "outbound kinds", ids: AllOutboundKinds()},
		{name: "mismatch kinds", ids: mismatchKinds()},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			seen := map[string]bool{}
			for _, id := range tt.ids {
				assert.False(t, seen[id], "duplicate %s", id)
				seen[id] = true
			}
			assert.NotEmpty(t, tt.ids)
		})
	}
}

func stageIDs() []string {
	out := []string{}
	for _, s := range AllStages() {
		out = append(out, string(s))
	}
	return out
}

func decisionIDs() []string {
	out := []string{}
	for _, d := range AllDecisions() {
		out = append(out, string(d))
	}
	return out
}

func mismatchKinds() []string {
	out := []string{}
	for _, k := range AllMismatchKinds() {
		out = append(out, string(k))
	}
	return out
}
