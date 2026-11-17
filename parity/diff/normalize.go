package diff

import (
	"errors"
	"fmt"
	"math/big"
	"regexp"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/schema"
)

// Normalization rule IDs. parity/schema/normalization.json is the allowlist:
// the differ applies a rule only when the file lists it, with the stages,
// sort keys, dropped members and shapes the file gives. A rule in the file
// without an implementation here is a load error, so the file can never claim
// a normalization the differ does not perform.
const (
	RuleSeatbidOrder         = "seatbid_order"
	RuleWarningsErrorsOrder  = "warnings_errors_order"
	RuleHTTPCallsOrder       = "httpcalls_order"
	RuleHTTPOutOrder         = "http_out_order"
	RuleAdapterBidsCallKey   = "adapter_bids_call_key"
	RuleSeatNonBidOrder      = "seatnonbid_order"
	RuleCachePutsOrder       = "cache_puts_order"
	RuleBidArrivalOrder      = "bid_arrival_order"
	RuleAdapterRequestsOrder = "adapter_requests_order"
	RuleResponseTimeMillis   = "responsetimemillis"
	RuleAuctionTimestamp     = "auctiontimestamp"
	RuleEventsTSParam        = "events_ts_param"
	RuleAnalyticsStartTime   = "analytics_start_time"
	RuleRequestIDUUID        = "request_id_uuid"
	RuleTIDUUID              = "tid_uuid"
	RuleGeneratedBidID       = "generated_bid_id"
	RuleCacheUUID            = "cache_uuid"
	RuleHookExecutionTime    = "hook_execution_time"
	RuleTimingHistograms     = "timing_histograms"
	RuleGoRuntimeMetrics     = "go_runtime_metrics"
	RuleHeaderNames          = "header_names"
	RuleDateHeader           = "date_header"
	RuleJSONMemberOrder      = "json_member_order"
	RuleStageEntrySeq        = "stage_entry_seq"
)

// Normalization actions (normalization.json "actions").
const (
	actionSort            = "sort"
	actionDrop            = "drop"
	actionKeySet          = "key_set"
	actionShape           = "shape"
	actionUUIDPlaceholder = "uuid_placeholder"
	actionCountOnly       = "count_only"
	actionIgnore          = "ignore"
	actionCaseInsensitive = "case_insensitive_names"
)

// implementation is the differ's side of one allowlisted rule. LoadRules
// checks it against the file so code and allowlist cannot drift apart.
type implementation struct {
	action string
	// keys are the sort keys the implementation uses; they must equal the
	// file's keys as a set.
	keys []string
	// drop must equal the file's drop list.
	drop []string
	// shape is true when the file must give a shape.
	shape bool
	// mentions are strings the file's target must contain, tying the
	// implementation's selectors to the target the file cites.
	mentions []string
}

var implementations = map[string]implementation{
	RuleSeatbidOrder:         {action: actionSort, keys: []string{"seat", "impid", "id"}, mentions: []string{"$.seatbid[*]", "$.seatbid[*].bid[*]"}},
	RuleWarningsErrorsOrder:  {action: actionSort, keys: []string{"code", "message"}, mentions: []string{"$.ext.warnings.*[*]", "$.ext.errors.*[*]"}},
	RuleHTTPCallsOrder:       {action: actionSort, keys: []string{"uri", "requestbody", "status"}, mentions: []string{"$.ext.debug.httpcalls.*[*]"}},
	RuleHTTPOutOrder:         {action: actionSort, keys: []string{"kind", "bidder", "orig_url", "request.body"}, drop: []string{"seq"}, mentions: []string{"trace.http_out"}},
	RuleAdapterBidsCallKey:   {action: actionSort, keys: []string{"request_data.uri", "request_data.body"}, mentions: []string{"bidder#call"}},
	RuleSeatNonBidOrder:      {action: actionSort, keys: []string{"seat", "impid", "statuscode"}, mentions: []string{"$.ext.prebid.seatnonbid[*]", "nonbid[*]"}},
	RuleCachePutsOrder:       {action: actionSort, keys: []string{"type", "value"}, mentions: []string{"$.puts[*]", "$.responses[*]"}},
	RuleBidArrivalOrder:      {action: actionSort, keys: bidArrivalKeys, mentions: []string{"$.bids[*]", "$.seat_bids[*].bids[*]", "$.*.bids[*]", "$.bids.*[*]", "$.conversion_errors[*]", "the entry's errors"}},
	RuleAdapterRequestsOrder: {action: actionSort, keys: adapterRequestKeys, mentions: []string{"$.requests[*]"}},
	RuleResponseTimeMillis:   {action: actionKeySet, mentions: []string{"$.ext.responsetimemillis"}},
	RuleAuctionTimestamp:     {action: actionShape, shape: true, mentions: []string{"$.ext.prebid.auctiontimestamp"}},
	RuleEventsTSParam:        {action: actionShape, shape: true, mentions: []string{"ts="}},
	RuleAnalyticsStartTime:   {action: actionShape, shape: true, mentions: []string{"start_time_set"}},
	RuleRequestIDUUID:        {action: actionUUIDPlaceholder, mentions: []string{"$.id", "{{UUID}}"}},
	RuleTIDUUID:              {action: actionUUIDPlaceholder, mentions: []string{"$.source.tid", "$.imp[*].ext.tid"}},
	RuleGeneratedBidID:       {action: actionUUIDPlaceholder, mentions: []string{"$.seatbid[*].bid[*].ext.prebid.bidid"}},
	RuleCacheUUID:            {action: actionUUIDPlaceholder, mentions: []string{"hb_cache_id", "hb_uuid", "cacheId"}},
	RuleHookExecutionTime:    {action: actionShape, shape: true, mentions: []string{"execution_time_millis", "ExecutionTimeMillis", "$.ext.prebid.modules.trace"}},
	RuleTimingHistograms:     {action: actionCountOnly, mentions: []string{"_time", "_time_seconds", "_duration", "_duration_seconds"}},
	RuleGoRuntimeMetrics:     {action: actionIgnore, mentions: []string{goRuntimeMetricPrefix}},
	RuleHeaderNames:          {action: actionCaseInsensitive, mentions: []string{"headers"}},
	RuleDateHeader:           {action: actionIgnore, mentions: []string{"Date"}},
	RuleJSONMemberOrder:      {action: actionIgnore, mentions: []string{"member order"}},
	RuleStageEntrySeq:        {action: actionDrop, mentions: []string{".seq"}},
}

// ImplementedRules returns the IDs of every rule the differ can apply, sorted.
func ImplementedRules() []string {
	out := make([]string, 0, len(implementations))
	for id := range implementations {
		out = append(out, id)
	}
	sort.Strings(out)
	return out
}

// Rule is one active allowlist entry.
type Rule struct {
	Spec   schema.NormalizationRule
	all    bool
	stages map[parity.StageID]bool
	shape  *shapeCheck
}

// AppliesTo reports whether the rule normalizes values of stage.
func (r *Rule) AppliesTo(stage parity.StageID) bool {
	return r != nil && (r.all || r.stages[stage])
}

// Rules is the active normalization allowlist.
type Rules struct {
	byID map[string]*Rule
	ids  []string
}

// LoadRules loads the embedded parity/schema/normalization.json.
func LoadRules() (*Rules, error) {
	reg, err := schema.Normalization()
	if err != nil {
		return nil, fmt.Errorf("parity diff: loading normalization allowlist: %w", err)
	}
	return NewRules(reg)
}

// NewRules validates an allowlist against the differ's implementations and
// returns the active rules. Every rule must cite a Go nondeterminism source
// and a justification, name a registered stage, and match its implementation.
func NewRules(reg schema.NormalizationRegistry) (*Rules, error) {
	if reg.Schema != parity.SchemaNormalization {
		return nil, fmt.Errorf("parity diff: normalization allowlist has schema %q, want %q", reg.Schema, parity.SchemaNormalization)
	}
	registered := make(map[parity.StageID]bool)
	for _, s := range parity.AllStages() {
		registered[s] = true
	}
	rules := &Rules{byID: make(map[string]*Rule, len(reg.Rules))}
	var errs []error
	for _, spec := range reg.Rules {
		rule, err := newRule(spec, registered)
		if err != nil {
			errs = append(errs, fmt.Errorf("rule %q: %w", spec.ID, err))
			continue
		}
		if _, dup := rules.byID[spec.ID]; dup {
			errs = append(errs, fmt.Errorf("rule %q: listed twice", spec.ID))
			continue
		}
		rules.byID[spec.ID] = rule
		rules.ids = append(rules.ids, spec.ID)
	}
	if len(errs) > 0 {
		return nil, fmt.Errorf("parity diff: invalid normalization allowlist: %w", errors.Join(errs...))
	}
	return rules, nil
}

func newRule(spec schema.NormalizationRule, registered map[parity.StageID]bool) (*Rule, error) {
	impl, ok := implementations[spec.ID]
	if !ok {
		return nil, errors.New("the differ has no implementation for it")
	}
	if len(spec.GoSource) == 0 || strings.TrimSpace(spec.Justification) == "" {
		return nil, errors.New("every rule must cite its Go nondeterminism source and a justification")
	}
	for _, src := range spec.GoSource {
		if strings.TrimSpace(src) == "" {
			return nil, errors.New("empty go_source entry")
		}
	}
	if spec.Action != impl.action {
		return nil, fmt.Errorf("action %q, the implementation performs %q", spec.Action, impl.action)
	}
	if !sameSet(spec.Keys, impl.keys) {
		return nil, fmt.Errorf("keys %v, the implementation sorts by %v", spec.Keys, impl.keys)
	}
	if !sameSet(spec.Drop, impl.drop) {
		return nil, fmt.Errorf("drop %v, the implementation drops %v", spec.Drop, impl.drop)
	}
	for _, m := range impl.mentions {
		if !strings.Contains(spec.Target, m) {
			return nil, fmt.Errorf("target %q does not mention %q, which the implementation selects", spec.Target, m)
		}
	}
	rule := &Rule{Spec: spec, stages: make(map[parity.StageID]bool)}
	if len(spec.AppliesTo) == 0 {
		return nil, errors.New("applies_to is empty")
	}
	for _, s := range spec.AppliesTo {
		switch {
		case s == "*":
			rule.all = true
		case registered[parity.StageID(s)]:
			rule.stages[parity.StageID(s)] = true
		default:
			return nil, fmt.Errorf("applies_to names unregistered stage %q", s)
		}
	}
	if impl.shape {
		shape, err := newShapeCheck(spec.Shape)
		if err != nil {
			return nil, err
		}
		rule.shape = shape
	}
	return rule, nil
}

func sameSet(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	as, bs := append([]string(nil), a...), append([]string(nil), b...)
	sort.Strings(as)
	sort.Strings(bs)
	return equalStrings(as, bs)
}

// IDs returns the active rule IDs in file order.
func (rs *Rules) IDs() []string {
	return append([]string(nil), rs.ids...)
}

// Rule returns the active rule id, or nil.
func (rs *Rules) Rule(id string) *Rule {
	if rs == nil {
		return nil
	}
	return rs.byID[id]
}

// Active reports whether rule id is in the allowlist and applies to stage.
func (rs *Rules) Active(id string, stage parity.StageID) bool {
	return rs.Rule(id).AppliesTo(stage)
}

// shapeCheck is the file-given shape of a shape rule. A shape is as narrow as
// the Go value it stands for (a millisecond timestamp, a non-negative
// duration), so a Rust value of the right JSON type but the wrong unit or sign
// is still reported.
type shapeCheck struct {
	typ      string
	pattern  *regexp.Regexp
	min, max *big.Rat
}

// shapeMembers are the members a shape may have.
var shapeMembers = map[string]bool{"type": true, "pattern": true, "minimum": true, "maximum": true}

func newShapeCheck(shape map[string]any) (*shapeCheck, error) {
	for k := range shape {
		if !shapeMembers[k] {
			return nil, fmt.Errorf("shape member %q is not one of type, pattern, minimum, maximum", k)
		}
	}
	typ, _ := shape["type"].(string)
	switch typ {
	case "integer", "number", "string", "boolean":
	default:
		return nil, fmt.Errorf("shape type %q is not one of integer, number, string, boolean", typ)
	}
	sc := &shapeCheck{typ: typ}
	if p, ok := shape["pattern"].(string); ok {
		re, err := regexp.Compile(p)
		if err != nil {
			return nil, fmt.Errorf("shape pattern: %w", err)
		}
		sc.pattern = re
	}
	var err error
	if sc.min, err = shapeBound(shape, "minimum", typ); err != nil {
		return nil, err
	}
	if sc.max, err = shapeBound(shape, "maximum", typ); err != nil {
		return nil, err
	}
	return sc, nil
}

// shapeBound reads an optional numeric bound of an integer or number shape.
func shapeBound(shape map[string]any, name, typ string) (*big.Rat, error) {
	v, ok := shape[name]
	if !ok {
		return nil, nil
	}
	if typ != "integer" && typ != "number" {
		return nil, fmt.Errorf("shape %s applies to integer and number shapes only", name)
	}
	f, ok := v.(float64)
	if !ok {
		return nil, fmt.Errorf("shape %s %v is not a number", name, v)
	}
	r := new(big.Rat)
	if r.SetFloat64(f) == nil {
		return nil, fmt.Errorf("shape %s %v is not finite", name, v)
	}
	return r, nil
}

// matches reports whether n has the shape.
func (s *shapeCheck) matches(n *node) bool {
	switch s.typ {
	case "integer":
		return n.kind == kindNumber && n.num.IsInt() && s.inBounds(n.num)
	case "number":
		return n.kind == kindNumber && s.inBounds(n.num)
	case "boolean":
		return n.kind == kindBool
	default:
		return n.kind == kindString && s.matchesString(n.str)
	}
}

func (s *shapeCheck) inBounds(v *big.Rat) bool {
	return (s.min == nil || v.Cmp(s.min) >= 0) && (s.max == nil || v.Cmp(s.max) <= 0)
}

func (s *shapeCheck) matchesString(v string) bool {
	return s.pattern == nil || s.pattern.MatchString(v)
}

// mask replaces n in place with a masked value that compares equal to any
// other value masked by the same rule.
func mask(n *node, label string) {
	exact := n.exact
	*n = node{kind: kindMasked, mask: label, exact: exact}
}

// selectNodes returns the nodes at a relative selector. Segments are member
// names separated by dots; "*" selects every member value and a "[*]" suffix
// selects every element: "ext.warnings.*[*]", "[*].object.response".
func selectNodes(root *node, selector string) []*node {
	current := []*node{root}
	if selector == "" {
		return nonNil(current)
	}
	for _, seg := range strings.Split(selector, ".") {
		name, wildcards := strings.CutSuffix(seg, "[*]")
		var next []*node
		for _, n := range current {
			next = append(next, memberNodes(n, name)...)
		}
		if wildcards {
			next = elementNodes(next)
		}
		current = next
	}
	return nonNil(current)
}

func memberNodes(n *node, name string) []*node {
	switch {
	case n == nil:
		return nil
	case name == "":
		return []*node{n}
	case name == "*" && n.kind == kindObject:
		out := make([]*node, 0, len(n.members))
		for _, k := range n.sortedKeys() {
			out = append(out, n.get(k))
		}
		return out
	default:
		return []*node{n.get(name)}
	}
}

func elementNodes(list []*node) []*node {
	var out []*node
	for _, n := range list {
		if n != nil && n.kind == kindArray {
			out = append(out, n.elems...)
		}
	}
	return out
}

func nonNil(list []*node) []*node {
	out := list[:0]
	for _, n := range list {
		if n != nil {
			out = append(out, n)
		}
	}
	return out
}

// anchorsOf returns every node selected by any of the selectors.
func anchorsOf(root *node, selectors []string) []*node {
	var out []*node
	for _, sel := range selectors {
		out = append(out, selectNodes(root, sel)...)
	}
	return out
}

// Where BidResponse-shaped and BidRequest-shaped values live inside each
// stage value. Rules whose targets are written relative to a response or a
// request ($.seatbid, $.source.tid) apply at these anchors.
var (
	responseAnchors = map[parity.StageID][]string{
		parity.StageHTTPResponse:         {""},
		parity.StageExchangeResponse:     {"", "response", "bid_response"},
		parity.StageHooksAuctionResponse: {"", "response", "bid_response"},
		parity.StageHooksExitpoint:       {"", "response", "bid_response"},
		parity.StageAnalytics:            {"[*].object.response"},
	}
	requestAnchors = map[parity.StageID][]string{
		parity.StageHooksEntrypoint:       {"", "request", "bid_request"},
		parity.StageHooksRawAuction:       {"", "request", "bid_request"},
		parity.StageRequestMerged:         {"", "request", "bid_request"},
		parity.StageRequestNormalized:     {"", "request", "bid_request"},
		parity.StageRequestValidated:      {"", "request", "bid_request"},
		parity.StageHooksProcessedAuction: {"", "request", "bid_request"},
		parity.StageAuctionSetup:          {"resolved_request"},
		parity.StageFloorsEnrich:          {"", "request_after"},
		parity.StageBidderRequest:         {"", "request", "bid_request"},
		parity.StageHooksBidderRequest:    {"", "request", "bid_request"},
		parity.StageAdapterRequests:       {"requests[*].body"},
		parity.StageAdapterRequestsFinal:  {"requests[*].body"},
		parity.StageAnalytics:             {"[*].object.request"},
	}
	// responseExtAnchors are where a stage records a response's ext
	// (openrtb_ext.ExtBidResponse) beside the BidResponse instead of inside
	// it (stagedata.ExchangeResponse). Rules whose targets start at $.ext
	// apply there too.
	responseExtAnchors = map[parity.StageID][]string{
		parity.StageExchangeResponse: {"ext_bid_response"},
	}
	// bidListSelectors are the per-seat bid lists that Go fills in response
	// arrival order (bid_arrival_order rule).
	bidListSelectors = map[parity.StageID][]string{
		parity.StageBidsAdjusted:                  {"bids", "conversion_errors"},
		parity.StageBidderResult:                  {"seat_bids[*].bids"},
		parity.StageHooksAllProcessedBidResponses: {"*.bids"},
		parity.StageEventsTracking:                {"bids.*"},
	}
	// adapterRequestLists are the MakeRequests outputs (adapter_requests_order).
	adapterRequestLists = map[parity.StageID][]string{
		parity.StageAdapterRequests:      {"requests"},
		parity.StageAdapterRequestsFinal: {"requests"},
	}
	// seatNonBidMaps are NonBid lists keyed by seat (stagedata.SeatNonBids).
	seatNonBidMaps = map[parity.StageID][]string{
		parity.StageSeatNonBids: {"*"},
	}
	// seatNonBidLists are SeatNonBid arrays outside a response's ext.
	seatNonBidLists = map[parity.StageID][]string{
		parity.StageSeatNonBids: {"", "seatnonbid", "seat_non_bid"},
		parity.StageAnalytics:   {"[*].object.seat_non_bid"},
	}
	// hookTraceRoots hold hook outcomes whose execution times are dropped.
	hookTraceRoots = map[parity.StageID][]string{
		parity.StageHookOutcomes:     {""},
		parity.StageHTTPResponse:     {"ext.prebid.modules.trace"},
		parity.StageAnalytics:        {"[*].object.hook_outcomes", "[*].object.response.ext.prebid.modules.trace"},
		parity.StageExchangeResponse: {"ext.prebid.modules.trace", "ext_bid_response.prebid.modules.trace"},
	}
	// headerLists are [[name, value], ...] lists inside stage values.
	headerLists = map[parity.StageID][]string{
		parity.StageAdapterRequests:      {"requests[*].headers"},
		parity.StageAdapterRequestsFinal: {"requests[*].headers"},
	}
	// bodyMembers are parity Body objects inside stage values; they are
	// decoded so their content is compared semantically and lexically.
	bodyMembers = map[parity.StageID][]string{
		parity.StageAdapterRequests:      {"requests[*].body"},
		parity.StageAdapterRequestsFinal: {"requests[*].body"},
		parity.StageAdapterBids:          {"request_data.body"},
	}
)

// hookTimeMembers are the hook execution-time members (hookexecution
// ExecutionTime json tag, and its Go field name).
var hookTimeMembers = []string{"execution_time_millis", "ExecutionTimeMillis"}

func isRequestShaped(n *node) bool {
	return n != nil && n.kind == kindObject && n.get("imp") != nil
}

func isResponseShaped(n *node) bool {
	if n == nil || n.kind != kindObject || n.get("id") == nil {
		return false
	}
	for _, k := range []string{"seatbid", "cur", "nbr", "ext", "bidid"} {
		if n.get(k) != nil {
			return true
		}
	}
	return false
}

// structural applies the rules that do not depend on generated values: drop,
// key_set and shape rules, and the header-list form of the header_names rule.
func (rs *Rules) structural(stage parity.StageID, root *node) {
	if root == nil {
		return
	}
	if rs.Active(RuleResponseTimeMillis, stage) {
		for _, n := range selectResponseExt(root, stage, "responsetimemillis") {
			if n.kind == kindObject {
				for _, m := range n.members {
					if isMillis(m.val) {
						mask(m.val, "key_set:"+RuleResponseTimeMillis)
					}
				}
			}
		}
	}
	if rule := rs.Rule(RuleAuctionTimestamp); rule.AppliesTo(stage) {
		nodes := selectResponseExt(root, stage, "prebid.auctiontimestamp")
		if stage == parity.StageEventsTracking {
			nodes = selectNodes(root, eventsAuctionTimestamp)
		}
		maskShapes(rule, nodes)
	}
	if rule := rs.Rule(RuleAnalyticsStartTime); rule.AppliesTo(stage) && stage == parity.StageAnalytics {
		maskShapes(rule, selectNodes(root, "[*].object.start_time_set"))
	}
	if rule := rs.Rule(RuleEventsTSParam); rule.AppliesTo(stage) {
		maskEventTimestamps(rule, root)
	}
	if rule := rs.Rule(RuleHookExecutionTime); rule.AppliesTo(stage) {
		for _, n := range anchorsOf(root, hookTraceRoots[stage]) {
			maskMemberShapes(rule, n, hookTimeMembers)
		}
	}
	if rs.Active(RuleHeaderNames, stage) {
		for _, n := range anchorsOf(root, headerLists[stage]) {
			if semantic := headerListNode(n); semantic != nil {
				*n = *semantic
			}
		}
	}
}

// eventsAuctionTimestamp is the events.tracking member holding the auction
// timestamp (stagedata.EventsTracking.AuctionTimestampMS).
const eventsAuctionTimestamp = "auction_timestamp_ms"

// selectResponseExt applies a selector relative to a response ext
// ("responsetimemillis") at the ext of every response anchor of stage and at
// every response ext anchor.
func selectResponseExt(root *node, stage parity.StageID, selector string) []*node {
	out := selectAll(root, responseAnchors[stage], "ext."+selector)
	return append(out, selectAll(root, responseExtAnchors[stage], selector)...)
}

// selectAll applies one relative selector at every anchor.
func selectAll(root *node, anchors []string, selector string) []*node {
	var out []*node
	for _, a := range anchorsOf(root, anchors) {
		out = append(out, selectNodes(a, selector)...)
	}
	return out
}

func maskShapes(rule *Rule, nodes []*node) {
	for _, n := range nodes {
		if rule.shape.matches(n) {
			mask(n, "shape:"+rule.Spec.ID)
		}
	}
}

// eventTimestamp matches the ts= query parameter of an event URL, however its
// separator is spelled (raw JSON may escape & as \u0026; VAST may use &amp;).
// The value ends at the next separator, quote, tag or CDATA delimiter.
var eventTimestamp = regexp.MustCompile(`((?:\?|&amp;|&|\\u0026)ts=)([^&"'\s<>\]\\]*)`)

// eventURLMarker identifies event URLs (endpoints/events/event.go /event).
const eventURLMarker = "/event?"

// maskEventTimestamps replaces the ts parameter of every event URL in string
// values with <ts> when it has the file's shape.
func maskEventTimestamps(rule *Rule, root *node) {
	walk(root, func(n *node) {
		if n.kind == kindString && strings.Contains(n.str, eventURLMarker) {
			n.str = replaceEventTimestamps(rule, n.str)
			n.raw = replaceEventTimestamps(rule, n.raw)
		}
	})
}

func replaceEventTimestamps(rule *Rule, s string) string {
	return eventTimestamp.ReplaceAllStringFunc(s, func(m string) string {
		parts := eventTimestamp.FindStringSubmatch(m)
		if !rule.shape.matchesString(parts[2]) {
			return m
		}
		return parts[1] + "<ts>"
	})
}

// maskMemberShapes masks the value of every member named in names anywhere
// below n when it has the rule's shape. The member itself stays, so a side
// that leaves it out, or writes a value of another shape, is still reported.
func maskMemberShapes(rule *Rule, n *node, names []string) {
	walk(n, func(v *node) {
		if v.kind != kindObject {
			return
		}
		for _, name := range names {
			if m := v.get(name); m != nil && rule.shape.matches(m) {
				mask(m, "shape:"+rule.Spec.ID)
			}
		}
	})
}

// isMillis reports whether n is a non-negative integer, the only value Go
// writes for an elapsed time in milliseconds.
func isMillis(n *node) bool {
	return n != nil && n.kind == kindNumber && n.num.IsInt() && n.num.Sign() >= 0
}

// sortRules applies the ordering rules. rewrite masks generated UUIDs so the
// sort keys of two implementations agree although their UUIDs differ.
func (rs *Rules) sortRules(stage parity.StageID, root *node, rewrite func(string) string) {
	if root == nil {
		return
	}
	if rs.Active(RuleSeatbidOrder, stage) {
		for _, seatbid := range selectAll(root, responseAnchors[stage], "seatbid") {
			sortArray(seatbid, []string{"seat"}, rewrite)
			for _, bids := range selectNodes(seatbid, "[*].bid") {
				sortArray(bids, []string{"impid", "id"}, rewrite)
			}
		}
	}
	if rs.Active(RuleWarningsErrorsOrder, stage) {
		for _, sel := range []string{"warnings.*", "errors.*"} {
			for _, list := range selectResponseExt(root, stage, sel) {
				sortArray(list, []string{"code", "message"}, rewrite)
			}
		}
	}
	if rs.Active(RuleHTTPCallsOrder, stage) {
		for _, list := range selectResponseExt(root, stage, "debug.httpcalls.*") {
			sortArray(list, []string{"uri", "requestbody", "status"}, rewrite)
		}
	}
	if rs.Active(RuleBidArrivalOrder, stage) {
		for _, list := range anchorsOf(root, bidListSelectors[stage]) {
			sortArray(list, bidArrivalKeys, rewrite)
		}
	}
	if rs.Active(RuleAdapterRequestsOrder, stage) {
		for _, list := range anchorsOf(root, adapterRequestLists[stage]) {
			sortArray(list, adapterRequestKeys, rewrite)
		}
	}
	if rs.Active(RuleSeatNonBidOrder, stage) {
		lists := selectResponseExt(root, stage, "prebid.seatnonbid")
		lists = append(lists, anchorsOf(root, seatNonBidLists[stage])...)
		for _, list := range lists {
			sortArray(list, []string{"seat"}, rewrite)
			for _, nonbids := range selectNodes(list, "[*].nonbid") {
				sortArray(nonbids, []string{"impid", "statuscode"}, rewrite)
			}
		}
		for _, nonbids := range anchorsOf(root, seatNonBidMaps[stage]) {
			sortArray(nonbids, []string{"impid", "statuscode"}, rewrite)
		}
	}
}

// entryWarningStages are the stages whose entry warnings Go builds from map
// iteration: ortb/request_validator.go ranges over imp.ext.prebid.bidder and
// appends one warning per disabled or unknown bidder (request.validated), and
// exchange/utils.go ranges over impsByBidder and appends one warning per
// bidder blocked by privacy settings (privacy.labels). A fatal validation
// error returns at once, so entry errors keep their order.
var entryWarningStages = map[parity.StageID]bool{parity.StageRequestValidated: true, parity.StagePrivacyLabels: true}

// entryArrivalStages are the stages whose entry errors and warnings Go appends
// per bidder response in completion order (exchange/bidder.go reads the
// response channel as the calls finish; bid_arrival_order).
var entryArrivalStages = map[parity.StageID]bool{parity.StageBidderResult: true}

// sortEntryLists sorts a stage entry's error lists by (code, message) where Go
// builds them from map iteration (warnings_errors_order) or in bidder
// response arrival order (bid_arrival_order).
func (rs *Rules) sortEntryLists(stage parity.StageID, en *entry, rewrite func(string) string) {
	if en == nil {
		return
	}
	keys := []string{"code", "message"}
	if entryWarningStages[stage] && rs.Active(RuleWarningsErrorsOrder, stage) {
		sortArray(en.warnings, keys, rewrite)
	}
	if entryArrivalStages[stage] && rs.Active(RuleBidArrivalOrder, stage) {
		sortArray(en.errors, keys, rewrite)
		sortArray(en.warnings, keys, rewrite)
	}
}

// sortArray sorts an array node by the canonical text of each key (dotted
// member paths), ascending and stable, with the element's full canonical text
// as the final tie-breaker so the order is total.
func sortArray(arr *node, keys []string, rewrite func(string) string) {
	if arr == nil || arr.kind != kindArray || len(arr.elems) < 2 {
		return
	}
	type keyed struct {
		parts []string
		elem  *node
	}
	items := make([]keyed, len(arr.elems))
	for i, e := range arr.elems {
		items[i] = keyed{parts: sortParts(e, keys, rewrite), elem: e}
	}
	sort.SliceStable(items, func(i, j int) bool {
		a, b := items[i].parts, items[j].parts
		for k := range a {
			if a[k] != b[k] {
				return a[k] < b[k]
			}
		}
		return false
	})
	for i := range items {
		arr.elems[i] = items[i].elem
	}
}

func sortParts(e *node, keys []string, rewrite func(string) string) []string {
	parts := make([]string, 0, len(keys)+1)
	for _, k := range keys {
		parts = append(parts, sortKeyText(memberPath(e, k), rewrite))
	}
	return append(parts, canonical(e, rewrite))
}

// sortKeyText renders a sort key: strings sort by their content, numbers by
// value, absent members first.
func sortKeyText(n *node, rewrite func(string) string) string {
	switch {
	case n == nil:
		return ""
	case n.kind == kindNumber:
		return numberSortKey(n.num)
	case n.kind == kindString && rewrite != nil:
		return "s" + rewrite(n.str)
	case n.kind == kindString:
		return "s" + n.str
	default:
		return "v" + canonical(n, rewrite)
	}
}

// numberSortKey orders numbers by value with a fixed-width decimal form.
func numberSortKey(r *big.Rat) string {
	f, _ := r.Float64()
	sign := "1"
	if f < 0 {
		sign = "0"
		f = -f
	}
	return fmt.Sprintf("n%s%030.10f", sign, f)
}

// memberPath resolves a dotted member path (request_data.uri) below n.
func memberPath(n *node, path string) *node {
	for _, seg := range strings.Split(path, ".") {
		n = n.get(seg)
		if n == nil {
			return nil
		}
	}
	return n
}

// metricTimingSuffixes and metricTimingNames identify timing histograms whose
// observed values are wall-clock durations (timing_histograms rule):
// metrics/prometheus/prometheus.go request, adapter, overhead, hook, cache and
// stored-data timers, plus adapter_connection_wait, which records a duration
// without a timing suffix.
var (
	metricTimingSuffixes = []string{"_time", "_time_seconds", "_duration", "_duration_seconds"}
	metricTimingNames    = []string{"adapter_connection_wait"}
)

// isTimingMetric reports whether a histogram or summary series records a
// wall-clock duration. Names may carry a configured namespace/subsystem prefix.
func isTimingMetric(name, typ string) bool {
	if typ != parity.MetricHistogram && typ != parity.MetricSummary {
		return false
	}
	for _, suffix := range metricTimingSuffixes {
		if strings.HasSuffix(name, suffix) {
			return true
		}
	}
	for _, n := range metricTimingNames {
		if name == n || strings.HasSuffix(name, "_"+n) {
			return true
		}
	}
	return false
}

// goRuntimeMetricPrefix starts the name of every series of the Go runtime
// collector (client_golang collectors.NewGoCollector). PBS registers it
// through a registerer that prepends the configured namespace and subsystem
// (metrics/prometheus/prometheus.go), so the prefix may follow "<ns>_<sub>_".
const goRuntimeMetricPrefix = "go_"

// goRuntimeMetric matches the series of the Go runtime collector: go_
// followed by one of the collector's metric families (the memstats collector
// and its runtime/metrics names), optionally after the configured
// "<namespace>_<subsystem>_" prefix. A product metric that merely contains
// "_go_" is not ignored.
var goRuntimeMetric = regexp.MustCompile(`^(?:[A-Za-z0-9_:]*_)?` + goRuntimeMetricPrefix +
	`(?:cgo|config|cpu|gc|godebug|goroutines|info|memory|memstats|sched|sync|threads)(?:_|$)`)

// isGoRuntimeMetric reports whether a series comes from the Go runtime
// collector (go_runtime_metrics rule).
func isGoRuntimeMetric(name string) bool {
	return goRuntimeMetric.MatchString(name)
}

// bidArrivalKeys sort the bid lists of bid_arrival_order: bids.adjusted
// entries carry imp_id and bid_id, bidder.result and
// hooks.all_processed_bid_responses entries a bid object, and events.tracking
// entries a bid_id only. Absent keys sort first.
var bidArrivalKeys = []string{"imp_id", "bid_id", "bid.impid", "bid.id"}

// adapterRequestKeys sort the requests of adapter_requests_order.
var adapterRequestKeys = []string{"uri", "method", "imp_ids", "body"}

// cachePutsKeys are the sort keys of a Prebid Cache request's puts.
var cachePutsKeys = []string{"type", "value"}

// sortCachePuts orders the puts of a Prebid Cache request by content, and the
// response's responses with them: the cache answers responses[i] for puts[i],
// and Go builds the puts from map iteration (exchange/auction.go doCache).
func sortCachePuts(c *call, rewrite func(string) string) {
	puts := memberPath(c.reqBody, "puts")
	if puts == nil || puts.kind != kindArray || len(puts.elems) < 2 {
		return
	}
	order := make([]int, len(puts.elems))
	parts := make([][]string, len(puts.elems))
	for i, e := range puts.elems {
		order[i], parts[i] = i, sortParts(e, cachePutsKeys, rewrite)
	}
	sort.SliceStable(order, func(i, j int) bool {
		return lessParts(parts[order[i]], parts[order[j]])
	})
	permute(puts, order)
	if resps := memberPath(c.respBody, "responses"); resps != nil && resps.kind == kindArray && len(resps.elems) == len(order) {
		permute(resps, order)
	}
}

// lessParts compares two sortParts results element by element.
func lessParts(a, b []string) bool {
	for k := range a {
		if a[k] != b[k] {
			return a[k] < b[k]
		}
	}
	return false
}

// permute reorders arr so that element i becomes the old element order[i].
func permute(arr *node, order []int) {
	old := append([]*node(nil), arr.elems...)
	for i, j := range order {
		arr.elems[i] = old[j]
	}
}
