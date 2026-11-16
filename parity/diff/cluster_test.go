package diff

import (
	"crypto/sha1"
	"encoding/hex"
	"reflect"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
)

func TestPathPattern(t *testing.T) {
	tests := []struct{ in, want string }{
		{"$", "$"},
		{"$.imp[3].ext", "$.imp[*].ext"},
		{"$[12].request.headers['User-Agent']", "$[*].request.headers['User-Agent']"},
		{"$.a['b[1]'].c[0][1]", "$.a['b[*]'].c[*][*]"},
		{"$['@errors'][0].code", "$['@errors'][*].code"},
	}
	for _, tt := range tests {
		if got := PathPattern(tt.in); got != tt.want {
			t.Errorf("PathPattern(%q) = %q, want %q", tt.in, got, tt.want)
		}
	}
}

func TestPathBuilding(t *testing.T) {
	tests := []struct {
		got, want string
	}{
		{child(Root, "imp"), "$.imp"},
		{child(Root, "X-Prebid"), "$['X-Prebid']"},
		{child(Root, "floors.skip"), "$['floors.skip']"},
		{child(Root, `it's\`), `$['it\'s\\']`},
		{child(Root, "1st"), "$['1st']"},
		{meta(Root, MetaHeaders), "$['@headers']"},
		{index(child(Root, "imp"), 2), "$.imp[2]"},
	}
	for _, tt := range tests {
		if tt.got != tt.want {
			t.Errorf("got %q, want %q", tt.got, tt.want)
		}
	}
}

func TestPathSegments(t *testing.T) {
	tests := []struct {
		in   string
		want []string
	}{
		{"$", nil},
		{"$.imp[0].ext.prebid", []string{"imp", "*", "ext", "prebid"}},
		{`$['a b']['it\'s'].x[1]`, []string{"a b", "it's", "x", "*"}},
		{"$['@headers']['Content-Type']", []string{"@headers", "Content-Type"}},
		{"$[3].request.body.user.eids", []string{"*", "request", "body", "user", "eids"}},
	}
	for _, tt := range tests {
		if got := pathSegments(tt.in); !reflect.DeepEqual(got, tt.want) {
			t.Errorf("pathSegments(%q) = %q, want %q", tt.in, got, tt.want)
		}
	}
}

func TestSignature(t *testing.T) {
	sum := sha1.Sum([]byte("bidder.request|bidders_adapters|$.imp[*].banner.w|value"))
	want := hex.EncodeToString(sum[:])
	if got := Signature(parity.StageBidderRequest, parity.DomainBiddersAdapters, "$.imp[3].banner.w", parity.KindValue); got != want {
		t.Fatalf("Signature = %s, want %s", got, want)
	}
	other := Signature(parity.StageBidderRequest, parity.DomainBiddersAdapters, "$.imp[3].banner.w", parity.KindType)
	if other == want {
		t.Fatal("kind does not change the signature")
	}
}

// TestAttribution covers domain and requirement refinement.
func TestAttribution(t *testing.T) {
	e := newTestEngine(t)
	tests := []struct {
		name        string
		f           finding
		endpoint    string
		domain      parity.Domain
		requirement string
	}{
		{"stage domain", finding{stage: parity.StageBidderRequest, path: "$.imp[0].banner.w", kind: parity.KindValue},
			"/openrtb2/auction", parity.DomainBiddersAdapters, "BIDD-01"},
		{"privacy path", finding{stage: parity.StageBidderRequest, path: "$.user.ext.consent", kind: parity.KindValue},
			"/openrtb2/auction", parity.DomainPolicyPrivacyIdentity, "PRIV-01"},
		{"eids path", finding{stage: parity.StageBidderRequest, path: "$.user.eids[0].uids[0].id", kind: parity.KindValue},
			"/openrtb2/auction", parity.DomainPolicyPrivacyIdentity, "IDEN-01"},
		{"floors path", finding{stage: parity.StageHTTPResponse, path: "$.ext.prebid.floors.enforcement", kind: parity.KindValue},
			"/openrtb2/auction", parity.DomainPolicyPrivacyIdentity, "AUCT-03"},
		{"modules path", finding{stage: parity.StageHTTPResponse, path: "$.ext.prebid.modules.trace", kind: parity.KindValue},
			"/openrtb2/auction", parity.DomainHooksModules, "MODL-01"},
		{"cache targeting", finding{stage: parity.StageTargeting, path: "$.seatbid[0].bid[0].ext.prebid.targeting.hb_cache_id_appnexus", kind: parity.KindValue},
			"/openrtb2/auction", parity.DomainCacheSupportFlows, "CACH-01"},
		{"endpoint requirement", finding{stage: parity.StageHTTPResponse, path: "$.id", kind: parity.KindValue},
			"/openrtb2/amp", parity.DomainEndpoints, "ENDP-02"},
		{"cookie sync endpoint", finding{stage: parity.StageHTTPResponse, path: "$['@status']", kind: parity.KindStatus},
			"/cookie_sync", parity.DomainEndpoints, "ENDP-04"},
		{"timing", finding{stage: parity.StageBidderResult, path: "$.errors", kind: parity.KindTiming},
			"/openrtb2/auction", parity.DomainAuctionLifecycle, "AUCT-02"},
		{"cache outbound", finding{stage: parity.StageHTTPOut, path: "$[0].request.body", kind: parity.KindValue, outbound: parity.OutboundCache},
			"/openrtb2/auction", parity.DomainCacheSupportFlows, "CACH-01"},
		{"currency outbound", finding{stage: parity.StageHTTPOut, path: "$[0].request.url", kind: parity.KindValue, outbound: parity.OutboundCurrency},
			"/openrtb2/auction", parity.DomainPolicyPrivacyIdentity, "AUCT-03"},
		{"config keeps its domain", finding{stage: parity.StageConfigResolved, path: "$.gdpr.default_value", kind: parity.KindValue},
			"", parity.DomainConfigBootstrap, "CONF-01"},
		{"metrics keep their domain", finding{stage: parity.StageMetricsDelta, path: "$.regs", kind: parity.KindMetric},
			"/openrtb2/auction", parity.DomainCutoverProof, "OPER-01"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			d, r := e.attribution(tt.f, tt.endpoint)
			if d != tt.domain || r != tt.requirement {
				t.Fatalf("attribution = %s/%s, want %s/%s", d, r, tt.domain, tt.requirement)
			}
		})
	}
}

func record(caseID string, stage parity.StageID, path string, kind parity.MismatchKind, class string, primary bool) parity.MismatchRecord {
	return parity.MismatchRecord{
		CaseID: caseID, Stage: stage, Domain: parity.DomainBiddersAdapters, Requirement: "BIDD-01",
		Kind: kind, Class: class, JSONPath: path, Primary: primary,
		Signature: Signature(stage, parity.DomainBiddersAdapters, path, kind),
	}
}

func TestBuildClusters(t *testing.T) {
	docs := []parity.DiffDocument{
		{CaseID: "b", Mismatches: []parity.MismatchRecord{
			record("b", parity.StageBidderRequest, "$.imp[1].banner.w", parity.KindValue, "", true),
			record("b", parity.StageHTTPOut, "$[0].request.body.imp[1].banner.w", parity.KindValue, "", false),
		}},
		{CaseID: "a", Mismatches: []parity.MismatchRecord{
			record("a", parity.StageBidderRequest, "$.imp[0].banner.w", parity.KindValue, "", true),
			record("a", parity.StageBidderRequest, "$.imp[2].banner.w", parity.KindValue, "", true),
			record("a", parity.StageAdapterBids, "$.bidder_response.currency", parity.KindLexical, parity.LexicalHTMLEscape, true),
			record("a", parity.StageAdapterBids, "$.bidder_response.currency", parity.KindLexical, parity.LexicalUnicodeEscape, true),
		}},
	}
	clusters := BuildClusters(docs)
	if len(clusters) != 3 {
		t.Fatalf("got %d clusters", len(clusters))
	}
	first := clusters[0]
	if first.Stage != parity.StageBidderRequest || first.Count != 3 || first.PrimaryCount != 3 ||
		!equalStrings(first.Cases, []string{"a", "b"}) || first.PathPattern != "$.imp[*].banner.w" {
		t.Fatalf("first cluster = %+v", first)
	}
	if first.Example.CaseID != "a" || first.Example.JSONPath != "$.imp[0].banner.w" {
		t.Fatalf("example = %s %s", first.Example.CaseID, first.Example.JSONPath)
	}
	if lex := clusters[1]; lex.Kind != parity.KindLexical || !equalStrings(lex.Classes, []string{parity.LexicalHTMLEscape, parity.LexicalUnicodeEscape}) {
		t.Fatalf("lexical cluster = %+v", lex)
	}
	if last := clusters[2]; last.Primary() || last.Stage != parity.StageHTTPOut {
		t.Fatalf("last cluster = %+v", last)
	}
	if got := PrimaryClusters(clusters); len(got) != 2 {
		t.Fatalf("primary clusters = %d", len(got))
	}
	if got := CountByKind(docs); got[parity.KindValue] != 4 || got[parity.KindLexical] != 2 {
		t.Fatalf("CountByKind = %v", got)
	}
	sigs := PrimarySignatures(docs[1])
	if len(sigs) != 2 {
		t.Fatalf("PrimarySignatures = %v", sigs)
	}
	if again := BuildClusters(docs); !reflect.DeepEqual(again, clusters) {
		t.Fatal("clustering is not deterministic")
	}
}
