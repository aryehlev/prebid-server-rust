// Package parity holds the language-neutral contract of the Go-vs-Rust live
// differential parity harness.
//
// Every type in this package mirrors a JSON Schema under parity/schema. The Go
// server, the Rust server and the Go runner all exchange these documents, so the
// JSON field names, enum values and identifier strings defined here are a
// cross-language contract: change them only together with the schema files and a
// schema_version bump (see parity/README.md).
//
// The package depends on the standard library only so that product code (the
// trace hooks, the transport and the deterministic fakes) can import it without
// pulling harness dependencies into the server binary.
package parity

import "encoding/json"

// SchemaVersion is the semantic version shared by every parity document. A
// minor bump adds optional fields; a major bump changes every Schema* name below
// and readers must reject documents whose major version they do not know.
const SchemaVersion = "1.0.0"

// Document type identifiers carried in the "schema" member of every document.
const (
	SchemaEnvelope              = "pbs-parity-envelope/1"
	SchemaTrace                 = "pbs-parity-trace/1"
	SchemaAdapterRequestsInput  = "pbs-parity-adapter-requests-input/1"
	SchemaAdapterRequestsOutput = "pbs-parity-adapter-requests-output/1"
	SchemaAdapterBidsInput      = "pbs-parity-adapter-bids-input/1"
	SchemaAdapterBidsOutput     = "pbs-parity-adapter-bids-output/1"
	SchemaDiff                  = "pbs-parity-diff/1"
	SchemaIndex                 = "pbs-parity-index/1"
	SchemaHealth                = "pbs-parity/1"
	SchemaConfigDump            = "pbs-parity-config/1"
	SchemaBiddersDump           = "pbs-parity-bidders/1"
	SchemaReset                 = "pbs-parity-reset/1"
	SchemaStages                = "pbs-parity-stages/1"
	SchemaDecisions             = "pbs-parity-decisions/1"
	SchemaNormalization         = "pbs-parity-normalization/1"
	SchemaFakes                 = "pbs-parity-fakes/1"
	SchemaErrorCodes            = "pbs-parity-error-codes/1"
)

// Implementation names used in the "impl" member of traces and in X-Parity-Meta.
const (
	ImplGo   = "go"
	ImplRust = "rust"
)

// HTTP surface shared by both servers.
const (
	// PathPrefix is the prefix of every parity introspection route.
	PathPrefix = "/__parity/"
	// TokenHeader carries the shared secret configured as parity.token.
	TokenHeader = "X-Parity-Token"
	// MetaHeader is added to every outbound request sent to the oracle.
	MetaHeader = "X-Parity-Meta"
)

// SampleRequestRoot is the only directory endpoint sample-request fixtures may
// be loaded from (Phase-1 threat T-01-01). Loaders must clean paths and reject
// anything that escapes the approved corpus roots.
const SampleRequestRoot = "endpoints/openrtb2/sample-requests"

// Domain is one of the nine locked parity matrix domains (Phase-1 D-05/D-06).
type Domain string

// The locked domain IDs. Their order is the matrix row order.
const (
	DomainEndpoints             Domain = "endpoints"
	DomainConfigBootstrap       Domain = "config_bootstrap"
	DomainStoredData            Domain = "stored_data"
	DomainAuctionLifecycle      Domain = "auction_lifecycle"
	DomainPolicyPrivacyIdentity Domain = "policy_privacy_identity"
	DomainBiddersAdapters       Domain = "bidders_adapters"
	DomainHooksModules          Domain = "hooks_modules"
	DomainCacheSupportFlows     Domain = "cache_support_flows"
	DomainCutoverProof          Domain = "cutover_proof"
)

// AllDomains returns the locked domains in matrix row order. It returns a new
// slice on every call so callers cannot mutate the canonical order.
func AllDomains() []Domain {
	return []Domain{
		DomainEndpoints,
		DomainConfigBootstrap,
		DomainStoredData,
		DomainAuctionLifecycle,
		DomainPolicyPrivacyIdentity,
		DomainBiddersAdapters,
		DomainHooksModules,
		DomainCacheSupportFlows,
		DomainCutoverProof,
	}
}

// Valid reports whether d is one of the locked domains.
func (d Domain) Valid() bool {
	for _, known := range AllDomains() {
		if d == known {
			return true
		}
	}
	return false
}

// SurfaceStatus is the status of one coverage-matrix row.
type SurfaceStatus string

// The only allowed matrix statuses.
const (
	StatusPassing            SurfaceStatus = "passing"
	StatusMismatch           SurfaceStatus = "mismatch"
	StatusNotYetInstrumented SurfaceStatus = "not-yet-instrumented"
)

// EvidenceLink points from a matrix row to the artifacts that justify its
// status. It never embeds request or response payloads.
type EvidenceLink struct {
	CaseID       string `json:"case_id"`
	FixturePath  string `json:"fixture_path"`
	ArtifactPath string `json:"artifact_path"`
	SummaryPath  string `json:"summary_path"`
	Cluster      string `json:"cluster,omitempty"`
}

// MatrixRow is one domain row of the coverage matrix.
type MatrixRow struct {
	Domain        Domain         `json:"domain"`
	Status        SurfaceStatus  `json:"status"`
	Summary       string         `json:"summary"`
	EvidenceLinks []EvidenceLink `json:"evidence_links"`
}

// MismatchKind classifies one difference between the Go and Rust observations.
// Every kind gates parity; there is no advisory kind.
type MismatchKind string

// Mismatch kinds. See mismatch.schema.json for the exact semantics.
const (
	KindValue              MismatchKind = "value"
	KindType               MismatchKind = "type"
	KindMissingInRust      MismatchKind = "missing_in_rust"
	KindExtraInRust        MismatchKind = "extra_in_rust"
	KindOrder              MismatchKind = "order"
	KindLexical            MismatchKind = "lexical"
	KindStatus             MismatchKind = "status"
	KindHeader             MismatchKind = "header"
	KindWire               MismatchKind = "wire"
	KindStageMissing       MismatchKind = "stage_missing"
	KindStageExtra         MismatchKind = "stage_extra"
	KindErrorCode          MismatchKind = "error_code"
	KindErrorText          MismatchKind = "error_text"
	KindMetric             MismatchKind = "metric"
	KindTiming             MismatchKind = "timing"
	KindDecision           MismatchKind = "decision"
	KindUnexpectedOutbound MismatchKind = "unexpected_outbound"
)

// AllMismatchKinds returns every mismatch kind in schema order.
func AllMismatchKinds() []MismatchKind {
	return []MismatchKind{
		KindValue, KindType, KindMissingInRust, KindExtraInRust, KindOrder,
		KindLexical, KindStatus, KindHeader, KindWire, KindStageMissing,
		KindStageExtra, KindErrorCode, KindErrorText, KindMetric, KindTiming,
		KindDecision, KindUnexpectedOutbound,
	}
}

// Lexical classes refine KindLexical: the two sides are semantically equal JSON
// but differ byte-wise. None of them is normalized away.
const (
	LexicalHTMLEscape      = "html_escape"
	LexicalUnicodeEscape   = "unicode_escape"
	LexicalNumberFormat    = "number_format"
	LexicalTrailingNewline = "trailing_newline"
	LexicalWhitespace      = "whitespace"
	LexicalMemberOrder     = "member_order"
	// LexicalUnrecorded: one side recorded only the semantic json of a
	// document the other recorded as exact bytes, so its lexical form could
	// not be compared.
	LexicalUnrecorded = "unrecorded"
	LexicalOther      = "other"
)

// Wire classes refine KindWire: transport-level differences on the HTTP wire.
// None of them is normalized away.
const (
	WireHeaderCase       = "header_case"
	WireHeaderOrder      = "header_order"
	WireDefaultHeader    = "default_header"
	WireHTTPVersion      = "http_version"
	WireContentLength    = "content_length"
	WireTransferEncoding = "transfer_encoding"
	WireOther            = "other"
)

// MismatchRecord is one attributed difference (Phase-1 MismatchRecord extended
// with stage, key, requirement, primary/consequential marking and signature).
type MismatchRecord struct {
	CaseID          string          `json:"case_id"`
	FixturePath     string          `json:"fixture_path"`
	Endpoint        string          `json:"endpoint"`
	Profile         string          `json:"profile"`
	Stage           StageID         `json:"stage"`
	Key             string          `json:"key"`
	Domain          Domain          `json:"domain"`
	Requirement     string          `json:"requirement"`
	Kind            MismatchKind    `json:"kind"`
	Class           string          `json:"class,omitempty"`
	JSONPath        string          `json:"json_path"`
	Go              json.RawMessage `json:"go"`
	Rust            json.RawMessage `json:"rust"`
	Primary         bool            `json:"primary"`
	ConsequentialOf *string         `json:"consequential_of"`
	Signature       string          `json:"signature"`
}

// DiffDocument is the per-case diff.json written by the runner.
type DiffDocument struct {
	Schema        string           `json:"schema"`
	SchemaVersion string           `json:"schema_version"`
	CaseID        string           `json:"case_id"`
	FixturePath   string           `json:"fixture_path"`
	Endpoint      string           `json:"endpoint"`
	Profile       string           `json:"profile"`
	Mismatches    []MismatchRecord `json:"mismatches"`
}

// AuctionIndexEntry is one case of parity/out/<run>/index.json. The first seven
// fields are the exact Phase-1 contract; Stage, Cluster and PrimaryDomain are
// the optional design additions.
type AuctionIndexEntry struct {
	CaseID        string   `json:"case_id"`
	FixturePath   string   `json:"fixture_path"`
	Endpoint      string   `json:"endpoint"`
	Domains       []Domain `json:"domains"`
	MismatchKinds []string `json:"mismatch_kinds"`
	SummaryPath   string   `json:"summary_path"`
	ArtifactPath  string   `json:"artifact_path"`
	Stage         StageID  `json:"stage,omitempty"`
	Cluster       string   `json:"cluster,omitempty"`
	PrimaryDomain Domain   `json:"primary_domain,omitempty"`
}

// AuctionIndex is the top-level index.json document.
type AuctionIndex struct {
	Schema        string              `json:"schema"`
	SchemaVersion string              `json:"schema_version"`
	Cases         []AuctionIndexEntry `json:"cases"`
}
