package diff

import (
	"crypto/sha1"
	"encoding/hex"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
)

// Signature returns the cluster signature of a mismatch:
// sha1(stage|domain|json_path with indices replaced by [*]|kind), lowercase hex
// (mismatch.schema.json). Records with equal signatures are the same defect.
// SHA-1 is the contract's clustering hash, not a security control.
func Signature(stage parity.StageID, domain parity.Domain, jsonPath string, kind parity.MismatchKind) string {
	sum := sha1.Sum([]byte(string(stage) + "|" + string(domain) + "|" + PathPattern(jsonPath) + "|" + string(kind)))
	return hex.EncodeToString(sum[:])
}

// endpointRequirements maps product routes to their endpoint requirement.
var endpointRequirements = []struct {
	prefix      string
	requirement string
}{
	{"/openrtb2/auction", "ENDP-01"},
	{"/openrtb2/amp", "ENDP-02"},
	{"/openrtb2/video", "ENDP-03"},
	{"/cookie_sync", "ENDP-04"},
	{"/setuid", "ENDP-04"},
	{"/getuids", "ENDP-04"},
	{"/event", "ENDP-05"},
	{"/vtrack", "ENDP-05"},
	{"/status", "ENDP-06"},
	{"/info/", "ENDP-06"},
}

func endpointRequirement(endpoint string) string {
	for _, er := range endpointRequirements {
		if strings.HasPrefix(endpoint, er.prefix) {
			return er.requirement
		}
	}
	return ""
}

// outboundDomains refines http.out mismatches by the outbound call kind.
var outboundDomains = map[string]struct {
	domain      parity.Domain
	requirement string
}{
	parity.OutboundCache:      {parity.DomainCacheSupportFlows, "CACH-01"},
	parity.OutboundFloors:     {parity.DomainPolicyPrivacyIdentity, "AUCT-03"},
	parity.OutboundCurrency:   {parity.DomainPolicyPrivacyIdentity, "AUCT-03"},
	parity.OutboundVendorList: {parity.DomainPolicyPrivacyIdentity, "PRIV-01"},
	parity.OutboundStored:     {parity.DomainStoredData, "STRD-01"},
}

// pathRule refines a mismatch's domain when its JSON path contains the
// segments (contiguous, anywhere in the path; "*" matches an index). A
// trailing "*" in the last segment matches any name with that prefix.
type pathRule struct {
	segments    []string
	domain      parity.Domain
	requirement string
}

// pathRules are checked in order; the first match wins.
var pathRules = []pathRule{
	{[]string{"ext", "prebid", "modules"}, parity.DomainHooksModules, "MODL-01"},
	{[]string{"prebid", "cache"}, parity.DomainCacheSupportFlows, "CACH-01"},
	{[]string{"hb_cache*"}, parity.DomainCacheSupportFlows, "CACH-01"},
	{[]string{"hb_uuid*"}, parity.DomainCacheSupportFlows, "CACH-01"},
	{[]string{"ext", "prebid", "floors"}, parity.DomainPolicyPrivacyIdentity, "AUCT-03"},
	{[]string{"imp", "*", "bidfloor"}, parity.DomainPolicyPrivacyIdentity, "AUCT-03"},
	{[]string{"imp", "*", "bidfloorcur"}, parity.DomainPolicyPrivacyIdentity, "AUCT-03"},
	{[]string{"user", "eids"}, parity.DomainPolicyPrivacyIdentity, "IDEN-01"},
	{[]string{"user", "ext", "eids"}, parity.DomainPolicyPrivacyIdentity, "IDEN-01"},
	{[]string{"regs"}, parity.DomainPolicyPrivacyIdentity, "PRIV-01"},
	{[]string{"user", "consent"}, parity.DomainPolicyPrivacyIdentity, "PRIV-01"},
	{[]string{"user", "ext", "consent"}, parity.DomainPolicyPrivacyIdentity, "PRIV-01"},
	{[]string{"device", "ip"}, parity.DomainPolicyPrivacyIdentity, "PRIV-01"},
	{[]string{"device", "ipv6"}, parity.DomainPolicyPrivacyIdentity, "PRIV-01"},
	{[]string{"device", "ifa"}, parity.DomainPolicyPrivacyIdentity, "PRIV-01"},
	{[]string{"device", "geo"}, parity.DomainPolicyPrivacyIdentity, "PRIV-01"},
	{[]string{"device", "lmt"}, parity.DomainPolicyPrivacyIdentity, "PRIV-01"},
}

func (r pathRule) matches(segments []string) bool {
	n := len(r.segments)
	for start := 0; start+n <= len(segments); start++ {
		ok := true
		for i, want := range r.segments {
			if !segmentMatches(want, segments[start+i]) {
				ok = false
				break
			}
		}
		if ok {
			return true
		}
	}
	return false
}

func segmentMatches(want, got string) bool {
	if prefix, ok := strings.CutSuffix(want, "*"); ok && prefix != "" {
		return strings.HasPrefix(got, prefix)
	}
	return want == got
}

// refinable reports whether path rules may override a stage's domain: they
// apply to the stages that carry requests, responses or outbound bodies.
// Config, metrics and analytics stay in their operational domains.
func refinable(domain parity.Domain) bool {
	return domain != parity.DomainConfigBootstrap && domain != parity.DomainCutoverProof
}

// attribution is the domain and requirement of one finding.
func (e *Engine) attribution(f finding, endpoint string) (parity.Domain, string) {
	spec := e.stages[f.stage]
	domain, requirement := spec.Domain, ""
	if f.stage == parity.StageHTTPOut {
		if od, ok := outboundDomains[f.outbound]; ok {
			domain, requirement = od.domain, od.requirement
		}
	}
	if refinable(spec.Domain) {
		segments := pathSegments(f.path)
		for _, rule := range pathRules {
			if rule.matches(segments) {
				domain, requirement = rule.domain, rule.requirement
				break
			}
		}
	}
	if requirement == "" {
		requirement = e.stageRequirement(f.stage, domain, endpoint, f.kind)
	}
	return domain, requirement
}

// stageRequirement chooses among a stage's requirements: AUCT-02 for timing,
// the endpoint's requirement when the stage lists it, else the first one the
// domain owns.
func (e *Engine) stageRequirement(stage parity.StageID, domain parity.Domain, endpoint string, kind parity.MismatchKind) string {
	reqs := e.stages[stage].Requirements
	has := func(id string) bool {
		for _, r := range reqs {
			if r == id {
				return true
			}
		}
		return false
	}
	if kind == parity.KindTiming && has("AUCT-02") {
		return "AUCT-02"
	}
	if er := endpointRequirement(endpoint); er != "" && has(er) {
		return er
	}
	for _, r := range reqs {
		if e.owner[r] == domain {
			return r
		}
	}
	if len(reqs) > 0 {
		return reqs[0]
	}
	if d, ok := e.domains[domain]; ok && len(d.Requirements) > 0 {
		return d.Requirements[0]
	}
	return ""
}

// document attributes findings and builds the diff document, then marks
// primary and consequential records (markPrimary).
func (e *Engine) document(c Case, findings []finding) parity.DiffDocument {
	records := make([]parity.MismatchRecord, 0, len(findings))
	instrumentation := make([]bool, 0, len(findings))
	for _, f := range findings {
		domain, requirement := e.attribution(f, c.Endpoint)
		records = append(records, parity.MismatchRecord{
			CaseID: c.CaseID, FixturePath: c.FixturePath, Endpoint: c.Endpoint, Profile: c.Profile,
			Stage: f.stage, Key: f.key, Domain: domain, Requirement: requirement,
			Kind: f.kind, Class: f.class, JSONPath: f.path, Go: f.goVal, Rust: f.rustVal,
			Signature: Signature(f.stage, domain, f.path, f.kind),
		})
		instrumentation = append(instrumentation, f.instrumentation)
	}
	order := make([]int, len(records))
	for i := range order {
		order[i] = i
	}
	sort.SliceStable(order, func(a, b int) bool { return e.recordLess(records[order[a]], records[order[b]]) })
	sorted := make([]parity.MismatchRecord, len(records))
	gaps := make([]bool, len(records))
	for i, idx := range order {
		sorted[i], gaps[i] = records[idx], instrumentation[idx]
	}
	markPrimary(e, sorted, gaps)
	return parity.DiffDocument{
		Schema: parity.SchemaDiff, SchemaVersion: parity.SchemaVersion,
		CaseID: c.CaseID, FixturePath: c.FixturePath, Endpoint: c.Endpoint, Profile: c.Profile,
		Mismatches: sorted,
	}
}

// plane is a causal chain of differences. Servers parse what earlier stages
// produced, so a lexical difference (escaping, number spelling) never causes a
// semantic one downstream, and a semantic difference is reported instead of,
// not as well as, a lexical one. Wire differences come from independent
// subsystems (the outbound client, the server's framing) and cause nothing.
type plane uint8

const (
	planeSemantic plane = iota
	planeLexical
	planeWire
)

func planeOf(kind parity.MismatchKind) plane {
	switch kind {
	case parity.KindLexical:
		return planeLexical
	case parity.KindWire:
		return planeWire
	}
	return planeSemantic
}

// markPrimary applies first-divergence attribution per plane. In the
// semantic and lexical planes the lowest ordered stage with a difference is
// the first diverging stage: its records are primary and every later record
// of that plane is consequential of its first record's signature. Wire
// records are always primary. Rust stages reported as "unimplemented" are
// capture gaps rather than divergences: their stage_missing records are always
// primary and never make other records consequential. records must be sorted
// by recordLess.
func markPrimary(e *Engine, records []parity.MismatchRecord, gaps []bool) {
	first := map[plane]int{}
	for i, r := range records {
		p := planeOf(r.Kind)
		if _, seen := first[p]; !seen && !gaps[i] {
			first[p] = i
		}
	}
	for i := range records {
		p := planeOf(records[i].Kind)
		head := records[first[p]]
		switch {
		case gaps[i], p == planeWire, e.stages[records[i].Stage].Order == e.stages[head.Stage].Order:
			records[i].Primary = true
		default:
			sig := head.Signature
			records[i].ConsequentialOf = &sig
		}
	}
}

// recordLess orders records by stage order, key, JSON path, kind and class.
func (e *Engine) recordLess(a, b parity.MismatchRecord) bool {
	if oa, ob := e.stages[a.Stage].Order, e.stages[b.Stage].Order; oa != ob {
		return oa < ob
	}
	if a.Key != b.Key {
		return a.Key < b.Key
	}
	if a.JSONPath != b.JSONPath {
		return a.JSONPath < b.JSONPath
	}
	if a.Kind != b.Kind {
		return a.Kind < b.Kind
	}
	return a.Class < b.Class
}
