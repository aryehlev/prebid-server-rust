package schema

import (
	"github.com/prebid/prebid-server/v4/parity"
)

// StageRegistry is stages.json.
type StageRegistry struct {
	Schema        string       `json:"schema"`
	SchemaVersion string       `json:"schema_version"`
	Description   string       `json:"description"`
	Domains       []DomainSpec `json:"domains"`
	Stages        []StageSpec  `json:"stages"`
}

// DomainSpec is one matrix domain with the requirement IDs it owns.
type DomainSpec struct {
	ID           parity.Domain `json:"id"`
	Order        int           `json:"order"`
	Requirements []string      `json:"requirements"`
	Description  string        `json:"description"`
}

// StageSpec is one registered capture point.
type StageSpec struct {
	ID           parity.StageID `json:"id"`
	Order        int            `json:"order"`
	Domain       parity.Domain  `json:"domain"`
	Requirements []string       `json:"requirements"`
	KeyedBy      string         `json:"keyed_by"`
	TraceField   string         `json:"trace_field"`
	Endpoints    []string       `json:"endpoints"`
	GoCapture    []string       `json:"go_capture"`
	Captures     string         `json:"captures"`
}

// Stage returns the spec of id, if registered.
func (r StageRegistry) Stage(id parity.StageID) (StageSpec, bool) {
	for _, s := range r.Stages {
		if s.ID == id {
			return s, true
		}
	}
	return StageSpec{}, false
}

// DecisionRegistry is decisions.json.
type DecisionRegistry struct {
	Schema        string            `json:"schema"`
	SchemaVersion string            `json:"schema_version"`
	Description   string            `json:"description"`
	Decisions     []DecisionSpec    `json:"decisions"`
	NotDecisions  []NotDecisionSpec `json:"not_decisions"`
}

// DecisionSpec is one decision point and its value domain.
type DecisionSpec struct {
	ID            parity.DecisionID `json:"id"`
	Domain        string            `json:"domain"`
	Values        map[string]string `json:"values"`
	GoSite        string            `json:"go_site"`
	ConsultedWhen string            `json:"consulted_when"`
	RNGEquivalent string            `json:"rng_equivalent"`
	// OutsideCase is the value a parity-mode server applies at the site
	// for a request outside an exec case, or "" when it draws as in
	// production.
	OutsideCase string `json:"outside_case,omitempty"`
}

// NotDecisionSpec documents a random source that deliberately is not a decision point.
type NotDecisionSpec struct {
	Site   string `json:"site"`
	Policy string `json:"policy"`
}

// Decision value domains.
const (
	DecisionDomainBinary = "binary"
	DecisionDomainIndex  = "index"
	DecisionDomainEnum   = "enum"
)

// NormalizationRegistry is normalization.json.
type NormalizationRegistry struct {
	Schema        string              `json:"schema"`
	SchemaVersion string              `json:"schema_version"`
	Description   string              `json:"description"`
	Actions       map[string]string   `json:"actions"`
	Rules         []NormalizationRule `json:"rules"`
	NotNormalized NotNormalized       `json:"not_normalized"`
}

// NormalizationRule is one allowlisted normalization.
type NormalizationRule struct {
	ID            string         `json:"id"`
	AppliesTo     []string       `json:"applies_to"`
	Target        string         `json:"target"`
	Action        string         `json:"action"`
	Keys          []string       `json:"keys,omitempty"`
	Drop          []string       `json:"drop,omitempty"`
	Shape         map[string]any `json:"shape,omitempty"`
	Justification string         `json:"justification"`
	GoSource      []string       `json:"go_source"`
}

// NotNormalized lists the difference classes that always gate parity.
type NotNormalized struct {
	Description  string       `json:"description"`
	Wire         ClassPolicy  `json:"wire"`
	Lexical      ClassPolicy  `json:"lexical"`
	AdapterFakes AdapterFakes `json:"adapter_fakes"`
}

// ClassPolicy names the mismatch kind and the classes reported under it.
type ClassPolicy struct {
	Kind    parity.MismatchKind `json:"kind"`
	Classes map[string]string   `json:"classes"`
}

// AdapterFakes lists the adapters compared exactly thanks to deterministic fakes.
type AdapterFakes struct {
	Description string   `json:"description"`
	Adapters    []string `json:"adapters"`
}

// FakesRegistry is fakes.json.
type FakesRegistry struct {
	Schema        string            `json:"schema"`
	SchemaVersion string            `json:"schema_version"`
	Description   string            `json:"description"`
	Defaults      parity.FakeValues `json:"defaults"`
	Derived       FakesDerived      `json:"derived"`
	Provenance    map[string]string `json:"provenance"`
	// GeneratedUUIDs documents the deterministic stream that replaces every
	// random v4 UUID the server generates in parity mode.
	GeneratedUUIDs GeneratedUUIDs `json:"generated_uuids"`
	// Orders are the deterministic orders parity mode imposes where Go's
	// outcome depends on response completion order or map iteration.
	Orders []FakeOrder `json:"orders"`
	// ServerSites are where code outside the adapters reads a fake value.
	ServerSites []FakeSite `json:"server_sites"`
	Adapters    []FakeSeam `json:"adapters"`
}

// FakeSite documents where code outside the adapters reads a fake value.
type FakeSite struct {
	Name   string `json:"name"`
	Value  string `json:"value"`
	Use    string `json:"use"`
	GoSeam string `json:"go_seam"`
	GoSite string `json:"go_site"`
}

// FakeOrder is one fakes.json orders entry.
type FakeOrder struct {
	Name   string `json:"name"`
	Order  string `json:"order"`
	GoSeam string `json:"go_seam"`
	GoSite string `json:"go_site"`
}

// GeneratedUUIDs is the fakes.json generated_uuids member.
type GeneratedUUIDs struct {
	Description string `json:"description"`
	Algorithm   string `json:"algorithm"`
	// Seed names the fake value the stream is seeded with ("uuid").
	Seed string `json:"seed"`
	// First are the first values of the stream for the default seed.
	First   []string `json:"first"`
	GoSeam  string   `json:"go_seam"`
	GoSites []string `json:"go_sites"`
}

// FakesDerived are values computed from the default clock, kept for readers.
type FakesDerived struct {
	NowRFC3339     string `json:"now_rfc3339"`
	NowUnixSeconds int64  `json:"now_unix_seconds"`
	NowISOWeek     int    `json:"now_iso_week"`
}

// FakeSeam documents where one adapter reads a fake value.
type FakeSeam struct {
	Bidder string `json:"bidder"`
	Value  string `json:"value"`
	Use    string `json:"use"`
	GoSeam string `json:"go_seam"`
	GoSite string `json:"go_site"`
}

// ErrorCodeRegistry is error_codes.json.
type ErrorCodeRegistry struct {
	Schema        string          `json:"schema"`
	SchemaVersion string          `json:"schema_version"`
	Description   string          `json:"description"`
	Errors        []ErrorCodeSpec `json:"errors"`
	Warnings      []ErrorCodeSpec `json:"warnings"`
}

// ErrorCodeSpec is one errortypes code.
type ErrorCodeSpec struct {
	Code       int    `json:"code"`
	Kind       string `json:"kind"`
	GoConstant string `json:"go_constant"`
}

// All returns errors followed by warnings.
func (r ErrorCodeRegistry) All() []ErrorCodeSpec {
	out := make([]ErrorCodeSpec, 0, len(r.Errors)+len(r.Warnings))
	out = append(out, r.Errors...)
	return append(out, r.Warnings...)
}

// Stages loads stages.json.
func Stages() (StageRegistry, error) {
	var r StageRegistry
	err := decodeStrict(FileStages, &r)
	return r, err
}

// Decisions loads decisions.json.
func Decisions() (DecisionRegistry, error) {
	var r DecisionRegistry
	err := decodeStrict(FileDecisions, &r)
	return r, err
}

// Normalization loads normalization.json.
func Normalization() (NormalizationRegistry, error) {
	var r NormalizationRegistry
	err := decodeStrict(FileNormalization, &r)
	return r, err
}

// Fakes loads fakes.json.
func Fakes() (FakesRegistry, error) {
	var r FakesRegistry
	err := decodeStrict(FileFakes, &r)
	return r, err
}

// ErrorCodes loads error_codes.json.
func ErrorCodes() (ErrorCodeRegistry, error) {
	var r ErrorCodeRegistry
	err := decodeStrict(FileErrorCodes, &r)
	return r, err
}
