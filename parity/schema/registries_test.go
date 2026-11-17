package schema

import (
	"encoding/json"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/prebid/prebid-server/v4/errortypes"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestStagesRegistryMatchesGoConstants(t *testing.T) {
	reg, err := Stages()
	require.NoError(t, err)

	ids := make([]parity.StageID, 0, len(reg.Stages))
	for i, s := range reg.Stages {
		ids = append(ids, s.ID)
		assert.Equal(t, i+1, s.Order, "stage %s order", s.ID)
	}
	assert.Equal(t, parity.AllStages(), ids, "stages.json must list parity.AllStages() in order")
	assert.Equal(t, stageStrings(parity.AllStages()), schemaEnum(t, FileCommon, "StageID"))
}

func TestStagesRegistryDomains(t *testing.T) {
	reg, err := Stages()
	require.NoError(t, err)

	domains := make([]parity.Domain, 0, len(reg.Domains))
	requirementOwner := map[string]parity.Domain{}
	for i, d := range reg.Domains {
		domains = append(domains, d.ID)
		assert.Equal(t, i+1, d.Order, "domain %s order", d.ID)
		require.NotEmpty(t, d.Requirements, "domain %s", d.ID)
		for _, req := range d.Requirements {
			owner, dup := requirementOwner[req]
			assert.False(t, dup, "requirement %s owned by %s and %s", req, owner, d.ID)
			requirementOwner[req] = d.ID
		}
	}
	assert.Equal(t, parity.AllDomains(), domains)
	assert.Equal(t, domainStrings(parity.AllDomains()), schemaEnum(t, FileCommon, "Domain"))

	stagesPerDomain := map[parity.Domain]int{}
	for _, s := range reg.Stages {
		assert.True(t, s.Domain.Valid(), "stage %s has invalid domain %q", s.ID, s.Domain)
		stagesPerDomain[s.Domain]++
		require.NotEmpty(t, s.Requirements, "stage %s", s.ID)
		for _, req := range s.Requirements {
			_, known := requirementOwner[req]
			assert.True(t, known, "stage %s cites unknown requirement %s", s.ID, req)
		}
		assert.NotEmpty(t, s.GoCapture, "stage %s must cite its Go capture site", s.ID)
	}
	for _, d := range parity.AllDomains() {
		assert.Positive(t, stagesPerDomain[d], "domain %s has no stage", d)
	}
}

func TestStagesRegistryLookup(t *testing.T) {
	reg, err := Stages()
	require.NoError(t, err)
	tests := []struct {
		id         parity.StageID
		wantFound  bool
		wantDomain parity.Domain
		wantKey    string
	}{
		{id: parity.StageHTTPResponse, wantFound: true, wantDomain: parity.DomainEndpoints, wantKey: parity.UnkeyedStage},
		{id: parity.StageAdapterBids, wantFound: true, wantDomain: parity.DomainBiddersAdapters, wantKey: "bidder#call"},
		{id: parity.StagePrivacyBidder, wantFound: true, wantDomain: parity.DomainPolicyPrivacyIdentity, wantKey: "bidder"},
		{id: parity.StageID("nope"), wantFound: false},
	}
	for _, tt := range tests {
		t.Run(string(tt.id), func(t *testing.T) {
			spec, found := reg.Stage(tt.id)
			assert.Equal(t, tt.wantFound, found)
			if found {
				assert.Equal(t, tt.wantDomain, spec.Domain)
				assert.Equal(t, tt.wantKey, spec.KeyedBy)
			}
		})
	}
}

func TestDecisionsRegistryMatchesGoConstants(t *testing.T) {
	reg, err := Decisions()
	require.NoError(t, err)
	ids := make([]parity.DecisionID, 0, len(reg.Decisions))
	for _, d := range reg.Decisions {
		ids = append(ids, d.ID)
		assert.NotEmpty(t, d.GoSite, "decision %s", d.ID)
		switch d.Domain {
		case DecisionDomainBinary:
			assert.Equal(t, []string{"0", "1"}, sortedKeys(d.Values), "decision %s", d.ID)
		case DecisionDomainEnum:
			assert.Equal(t, []string{parity.UsersyncShuffleIdentity}, sortedKeys(d.Values), "decision %s", d.ID)
		}
	}
	assert.Equal(t, parity.AllDecisions(), ids)
	assert.Equal(t, decisionStrings(parity.AllDecisions()), schemaEnum(t, FileCommon, "DecisionID"))

	var envelope struct {
		Properties struct {
			Decisions struct {
				Properties map[string]json.RawMessage `json:"properties"`
			} `json:"decisions"`
		} `json:"properties"`
	}
	require.NoError(t, json.Unmarshal(mustFile(t, FileEnvelope), &envelope))
	assert.ElementsMatch(t, decisionStrings(parity.AllDecisions()), keysOf(envelope.Properties.Decisions.Properties),
		"envelope.decisions must constrain every decision point")
}

func TestNormalizationRules(t *testing.T) {
	reg, err := Normalization()
	require.NoError(t, err)
	require.NotEmpty(t, reg.Rules)

	registered := map[string]bool{"*": true}
	for _, s := range parity.AllStages() {
		registered[string(s)] = true
	}
	seen := map[string]bool{}
	for _, rule := range reg.Rules {
		t.Run(rule.ID, func(t *testing.T) {
			assert.False(t, seen[rule.ID], "duplicate rule id")
			seen[rule.ID] = true
			assert.NotEmpty(t, rule.GoSource, "every rule must cite its Go nondeterminism source")
			for _, src := range rule.GoSource {
				assert.NotEmpty(t, src)
			}
			assert.NotEmpty(t, rule.Justification)
			assert.Contains(t, reg.Actions, rule.Action, "action must be documented")
			for _, stage := range rule.AppliesTo {
				assert.True(t, registered[stage], "rule applies to unregistered stage %q", stage)
			}
		})
	}
}

func TestNormalizationNeverHidesGatingClasses(t *testing.T) {
	reg, err := Normalization()
	require.NoError(t, err)

	assert.Equal(t, parity.KindWire, reg.NotNormalized.Wire.Kind)
	assert.Equal(t, parity.KindLexical, reg.NotNormalized.Lexical.Kind)
	assert.ElementsMatch(t, wireClasses(), keysOf(reg.NotNormalized.Wire.Classes))
	assert.ElementsMatch(t, lexicalClasses(), keysOf(reg.NotNormalized.Lexical.Classes))
	assert.ElementsMatch(t, wireClasses(), schemaEnum(t, FileMismatch, "WireClass"))
	assert.ElementsMatch(t, lexicalClasses(), schemaEnum(t, FileMismatch, "LexicalClass"))

	fakesReg, err := Fakes()
	require.NoError(t, err)
	assert.ElementsMatch(t, fakeBidders(fakesReg), reg.NotNormalized.AdapterFakes.Adapters)
}

func TestMismatchKindsMatchGoConstants(t *testing.T) {
	kinds := make([]string, 0, len(parity.AllMismatchKinds()))
	for _, k := range parity.AllMismatchKinds() {
		kinds = append(kinds, string(k))
	}
	assert.Equal(t, kinds, schemaEnum(t, FileMismatch, "MismatchKind"))
	assert.Equal(t, parity.AllOutboundKinds(), schemaEnum(t, FileCommon, "OutboundKind"))
}

// TestErrorCodesMatchErrortypes pins every errortypes constant to its kind name
// in error_codes.json and in parity.ErrorCodes().
func TestErrorCodesMatchErrortypes(t *testing.T) {
	want := []struct {
		code int
		kind string
	}{
		{errortypes.TimeoutErrorCode, "Timeout"},
		{errortypes.BadInputErrorCode, "BadInput"},
		{errortypes.BlockedAppErrorCode, "BlockedApp"},
		{errortypes.BadServerResponseErrorCode, "BadServerResponse"},
		{errortypes.FailedToRequestBidsErrorCode, "FailedToRequestBids"},
		{errortypes.BidderTemporarilyDisabledErrorCode, "BidderTemporarilyDisabled"},
		{errortypes.AccountDisabledErrorCode, "AccountDisabled"},
		{errortypes.AcctRequiredErrorCode, "AcctRequired"},
		{errortypes.NoConversionRateErrorCode, "NoConversionRate"},
		{errortypes.MalformedAcctErrorCode, "MalformedAcct"},
		{errortypes.ModuleRejectionErrorCode, "ModuleRejection"},
		{errortypes.TmaxTimeoutErrorCode, "TmaxTimeout"},
		{errortypes.FailedToMarshalErrorCode, "FailedToMarshal"},
		{errortypes.FailedToUnmarshalErrorCode, "FailedToUnmarshal"},
		{errortypes.InvalidImpFirstPartyDataErrorCode, "InvalidImpFirstPartyData"},
		{errortypes.BidderTemporarilyThrottledErrorCode, "BidderTemporarilyThrottled"},
		{errortypes.UnknownErrorCode, "UnknownError"},
		{errortypes.InvalidPrivacyConsentWarningCode, "InvalidPrivacyConsentWarning"},
		{errortypes.AccountLevelDebugDisabledWarningCode, "AccountLevelDebugDisabledWarning"},
		{errortypes.BidderLevelDebugDisabledWarningCode, "BidderLevelDebugDisabledWarning"},
		{errortypes.DisabledCurrencyConversionWarningCode, "DisabledCurrencyConversionWarning"},
		{errortypes.AlternateBidderCodeWarningCode, "AlternateBidderCodeWarning"},
		{errortypes.MultiBidWarningCode, "MultiBidWarning"},
		{errortypes.AdServerTargetingWarningCode, "AdServerTargetingWarning"},
		{errortypes.BidAdjustmentWarningCode, "BidAdjustmentWarning"},
		{errortypes.FloorBidRejectionWarningCode, "FloorBidRejectionWarning"},
		{errortypes.InvalidBidResponseDSAWarningCode, "InvalidBidResponseDSAWarning"},
		{errortypes.SecCookieDeprecationLenWarningCode, "SecCookieDeprecationLenWarning"},
		{errortypes.SecBrowsingTopicsWarningCode, "SecBrowsingTopicsWarning"},
		{errortypes.InvalidUserEIDsWarningCode, "InvalidUserEIDsWarning"},
		{errortypes.InvalidUserUIDsWarningCode, "InvalidUserUIDsWarning"},
		{errortypes.TooLongTargetingPrefixWarningCode, "TooLongTargetingPrefixWarning"},
		{errortypes.TooShortTargetingPrefixWarningCode, "TooShortTargetingPrefixWarning"},
		{errortypes.BidderBlockedByPrivacySettings, "BidderBlockedByPrivacySettings"},
		{errortypes.UnknownWarningCode, "UnknownWarning"},
	}
	reg, err := ErrorCodes()
	require.NoError(t, err)
	fromJSON := map[int]string{}
	for _, spec := range reg.All() {
		_, dup := fromJSON[spec.Code]
		assert.False(t, dup, "duplicate code %d", spec.Code)
		fromJSON[spec.Code] = spec.Kind
	}
	for _, spec := range reg.Errors {
		assert.Less(t, spec.Code, 10000, "error %s", spec.Kind)
	}
	for _, spec := range reg.Warnings {
		assert.Greater(t, spec.Code, 10000, "warning %s", spec.Kind)
	}
	fromGo := parity.ErrorCodes()
	for _, w := range want {
		assert.Equal(t, w.kind, fromJSON[w.code], "error_codes.json code %d", w.code)
		assert.Equal(t, w.kind, fromGo[w.code], "parity.ErrorCodes() code %d", w.code)
		assert.Equal(t, w.kind, parity.ErrorKindForCode(w.code))
	}
	assert.Len(t, fromJSON, len(want))
	assert.Len(t, fromGo, len(want))
	assert.Equal(t, parity.ErrorKindUnknown, parity.ErrorKindForCode(424242))
}

func TestFakesRegistryMatchesGoDefaults(t *testing.T) {
	reg, err := Fakes()
	require.NoError(t, err)
	assert.Equal(t, fakes.Defaults(), reg.Defaults)

	now := time.UnixMilli(reg.Defaults.NowUnixMS).UTC()
	_, week := now.ISOWeek()
	assert.Equal(t, now.Format("2006-01-02T15:04:05.000Z07:00"), reg.Derived.NowRFC3339)
	assert.Equal(t, now.Unix(), reg.Derived.NowUnixSeconds)
	assert.Equal(t, week, reg.Derived.NowISOWeek)
	assert.Equal(t, 33, week, "the default clock must stay in ISO week 33 (yieldlab fixtures)")

	assert.ElementsMatch(t, []string{
		"adnuntius", "appnexus", "cadent_aperture_mx", "eplanning",
		"flipp", "huaweiads", "tappx", "yieldlab",
	}, fakeBidders(reg))
}

func TestFakesRegistryGeneratedUUIDs(t *testing.T) {
	reg, err := Fakes()
	require.NoError(t, err)
	gen := reg.GeneratedUUIDs
	require.Equal(t, "uuid", gen.Seed)
	require.NotEmpty(t, gen.First)
	for i, want := range gen.First {
		got := fakes.SequenceUUID(reg.Defaults.UUID, uint64(i+1))
		assert.Equal(t, want, got.String(), "generated_uuids.first[%d]", i)
	}
	assert.NotEmpty(t, gen.GoSites)
}

// TestFakesRegistryOrders: every order names a Go site whose file exists and
// holds the seam the entry names.
func TestFakesRegistryOrders(t *testing.T) {
	reg, err := Fakes()
	require.NoError(t, err)
	seams := map[string]string{
		"bidder_request_phase":              "defer parityTakeTurn(ctx, bidderRequest.BidderName)()",
		"fpd_bidders":                       "fakes.MapKeys(allBiddersTable, cmp.Less[string])",
		"bidder_params_validation":          "fakes.MapKeys(prebid.Bidder, cmp.Less[string])",
		"stored_bid_responses_validation":   "fakes.MapKeys(bidResponses, cmp.Less[string])",
		"alias_validation":                  "fakes.MapKeys(aliases, cmp.Less[string])",
		"alias_gvlid_validation":            "fakes.MapKeys(aliasesGVLIDs, cmp.Less[string])",
		"bid_adjustment_factors_validation": "fakes.MapKeys(adjustmentFactors, cmp.Less[string])",
		"usersync_bidders":                  "fakes.MapKeys(bidderSyncerLookup, cmp.Less[string])",
		"auction_winners":                   "fakes.MapKeys(seatBids, cmp.Less[openrtb_ext.BidderName])",
		"category_dedupe":                   "fakes.MapKeys(seatBids, cmp.Less[openrtb_ext.BidderName])",
		"adtarget":                          "fakes.MapKeys(imp2source, cmp.Less[int])",
		"bidmatic":                          "fakes.MapKeys(imp2source, cmp.Less[int])",
		"bidder_response_order":             "pt.inCallOrder(",
		"adtelligent":                       "fakes.MapKeys(imp2source, cmp.Less[int])",
		"datablocks":                        "fakes.MapKeys(reqImps, fakes.LessDatablocks)",
		"trafficgate":                       "fakes.MapKeys(reqImps, fakes.LessTrafficGate)",
		"zeroclickfraud":                    "fakes.MapKeys(reqImps, fakes.LessZeroClickFraud)",
	}
	require.Len(t, reg.Orders, len(seams))
	for _, o := range reg.Orders {
		t.Run(o.Name, func(t *testing.T) {
			want, ok := seams[o.Name]
			require.True(t, ok, "unknown order %s", o.Name)
			file, _, _ := strings.Cut(o.GoSite, ":")
			src, err := os.ReadFile(filepath.Join("..", "..", file))
			require.NoError(t, err)
			assert.Contains(t, string(src), want)
		})
	}
}

// TestFakesRegistryServerSites: every server site names Go sites whose
// lines hold the seam, so the line numbers the Rust side reads are current.
func TestFakesRegistryServerSites(t *testing.T) {
	reg, err := Fakes()
	require.NoError(t, err)
	seams := map[string]string{
		"uids_cookie_expires":        "fakes.Now().Add(",
		"uid_expires":                "fakes.Now().Add(uidTTL)",
		"uid_liveness":               "fakes.Now()",
		"cookie_deprecation_expires": "c.time.Now()",
	}
	require.Len(t, reg.ServerSites, len(seams))
	for _, site := range reg.ServerSites {
		t.Run(site.Name, func(t *testing.T) {
			want, ok := seams[site.Name]
			require.True(t, ok, "unknown server site %s", site.Name)
			for _, line := range siteLines(t, site.GoSite) {
				assert.Contains(t, line, want)
			}
		})
	}
}

// siteLines returns the source lines a go_site such as "a/b.go:12 (x),
// :40 (y)" names.
func siteLines(t *testing.T, goSite string) []string {
	t.Helper()
	file, rest, ok := strings.Cut(goSite, ":")
	require.True(t, ok, "go_site %q names no line", goSite)
	src, err := os.ReadFile(filepath.Join("..", "..", file))
	require.NoError(t, err)
	lines := strings.Split(string(src), "\n")
	var out []string
	for _, m := range regexp.MustCompile(`(?:^|:)(\d+)`).FindAllStringSubmatch(rest, -1) {
		n, err := strconv.Atoi(m[1])
		require.NoError(t, err)
		require.LessOrEqual(t, n, len(lines), "%s has no line %d", file, n)
		out = append(out, lines[n-1])
	}
	require.NotEmpty(t, out, "go_site %q names no line", goSite)
	return out
}

// TestDecisionsOutsideCase: a decision's outside_case is one of its values,
// and only usersync.shuffle has one (usersync/shuffler.go).
func TestDecisionsOutsideCase(t *testing.T) {
	reg, err := Decisions()
	require.NoError(t, err)
	for _, d := range reg.Decisions {
		if d.ID == parity.DecisionUsersyncShuffle {
			assert.Equal(t, parity.UsersyncShuffleIdentity, d.OutsideCase)
		} else {
			assert.Empty(t, d.OutsideCase, d.ID)
		}
		if d.OutsideCase != "" {
			assert.Contains(t, d.Values, d.OutsideCase)
		}
	}
}

func schemaEnum(t *testing.T, file, definition string) []string {
	t.Helper()
	var doc struct {
		Definitions map[string]struct {
			Enum []string `json:"enum"`
		} `json:"definitions"`
	}
	require.NoError(t, json.Unmarshal(mustFile(t, file), &doc))
	def, ok := doc.Definitions[definition]
	require.True(t, ok, "%s has no definition %s", file, definition)
	require.NotEmpty(t, def.Enum, "%s#/definitions/%s has no enum", file, definition)
	return def.Enum
}

func stageStrings(in []parity.StageID) []string {
	out := make([]string, 0, len(in))
	for _, v := range in {
		out = append(out, string(v))
	}
	return out
}

func domainStrings(in []parity.Domain) []string {
	out := make([]string, 0, len(in))
	for _, v := range in {
		out = append(out, string(v))
	}
	return out
}

func decisionStrings(in []parity.DecisionID) []string {
	out := make([]string, 0, len(in))
	for _, v := range in {
		out = append(out, string(v))
	}
	return out
}

func wireClasses() []string {
	return []string{
		parity.WireHeaderCase, parity.WireHeaderOrder, parity.WireDefaultHeader,
		parity.WireHTTPVersion, parity.WireContentLength, parity.WireTransferEncoding,
		parity.WireOther,
	}
}

func lexicalClasses() []string {
	return []string{
		parity.LexicalHTMLEscape, parity.LexicalUnicodeEscape, parity.LexicalNumberFormat,
		parity.LexicalTrailingNewline, parity.LexicalWhitespace, parity.LexicalMemberOrder,
		parity.LexicalUnrecorded, parity.LexicalOther,
	}
}

func fakeBidders(reg FakesRegistry) []string {
	out := make([]string, 0, len(reg.Adapters))
	for _, a := range reg.Adapters {
		out = append(out, a.Bidder)
	}
	return out
}

func keysOf[V any](m map[string]V) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

func sortedKeys(m map[string]string) []string {
	return keysOf(m)
}
