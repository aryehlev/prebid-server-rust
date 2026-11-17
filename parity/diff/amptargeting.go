package diff

import (
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
)

// amp_targeting_merge.
//
// The AMP endpoint builds its targeting by copying the targeting of every bid
// whose ext mentions hb_cache_id into one map, in seatbid order, so a later bid
// overwrites a key an earlier one set (endpoints/openrtb2/amp_auction.go:368-388).
// The seatbid order follows the iteration of the seat map in buildBidResponse
// (exchange/exchange.go:1027), so when the winners of different imps come from
// different seats, the generic keys (hb_pb, hb_bidder, hb_size, hb_cache_id)
// differ between two Go processes. The rule masks a key only when the side's
// own exchange.response bids offer two or more values for it and some order of
// those bids merges into exactly the recorded targeting; every other key, and
// a targeting no order produces, compares exactly.

// ampTargetingAnchors locate the AMP targeting map in each stage that records
// it.
var ampTargetingAnchors = map[parity.StageID][]string{
	parity.StageAMPTargetingResponse: {"targeting"},
	parity.StageHooksExitpoint:       {"targeting"},
	parity.StageHTTPResponse:         {"targeting"},
	parity.StageAnalytics:            {"[*].object.amp_targeting_values"},
}

// ampCacheToken is what amp_auction.go:366 looks for in a bid's raw ext.
const ampCacheToken = "hb_cache_id"

// ampMergedBids returns the string targeting of the bids the AMP endpoint
// merges, from the side's exchange.response.
func (e *Engine) ampMergedBids(v *view) []map[string]string {
	var out []map[string]string
	for _, root := range e.stageRoots(v, parity.StageExchangeResponse) {
		for _, bid := range selectAll(root, responseAnchors[parity.StageExchangeResponse], "seatbid[*].bid[*]") {
			ext := bid.get("ext")
			if !mentionsToken(ext, ampCacheToken) {
				continue
			}
			out = append(out, stringMembers(selectNodes(ext, "prebid.targeting")))
		}
	}
	return out
}

// mentionsToken reports whether a member name or a string value under n
// starts with token (the bytes.Contains(bid.Ext, "\"hb_cache_id") test).
func mentionsToken(n *node, token string) bool {
	if n == nil {
		return false
	}
	switch n.kind {
	case kindString:
		return strings.HasPrefix(n.str, token)
	case kindObject:
		for _, m := range n.members {
			if strings.HasPrefix(m.key, token) || mentionsToken(m.val, token) {
				return true
			}
		}
	case kindArray:
		for _, el := range n.elems {
			if mentionsToken(el, token) {
				return true
			}
		}
	}
	return false
}

// stringMembers returns the string members of the first object in nodes.
func stringMembers(nodes []*node) map[string]string {
	out := map[string]string{}
	if len(nodes) == 0 || nodes[0].kind != kindObject {
		return out
	}
	for _, m := range nodes[0].members {
		if m.val != nil && m.val.kind == kindString {
			out[m.key] = m.val.str
		}
	}
	return out
}

// conflictingKeys are the keys the bids set to two or more values.
func conflictingKeys(bids []map[string]string) map[string]bool {
	values := map[string]map[string]bool{}
	for _, bid := range bids {
		for k, val := range bid {
			if values[k] == nil {
				values[k] = map[string]bool{}
			}
			values[k][val] = true
		}
	}
	out := map[string]bool{}
	for k, vals := range values {
		if len(vals) >= minStoredCandidates {
			out[k] = true
		}
	}
	return out
}

// mergeOrderExists reports whether merging the bids in some order, each
// overwriting the keys before it, yields observed for every key a bid sets.
// It builds the order from the last bid backwards: a bid can come last among
// the remaining ones when every key it sets that no later bid sets has its
// observed value. Taking such a bid only lets more bids qualify, so the greedy
// choice finds an order whenever one exists.
func mergeOrderExists(bids []map[string]string, observed map[string]string) bool {
	remaining := append([]map[string]string(nil), bids...)
	settled := map[string]bool{}
	for len(remaining) > 0 {
		last := -1
		for i, bid := range remaining {
			if canComeLast(bid, observed, settled) {
				last = i
				break
			}
		}
		if last < 0 {
			return false
		}
		for k := range remaining[last] {
			settled[k] = true
		}
		remaining = append(remaining[:last], remaining[last+1:]...)
	}
	return true
}

func canComeLast(bid, observed map[string]string, settled map[string]bool) bool {
	for k, val := range bid {
		if settled[k] {
			continue
		}
		if got, ok := observed[k]; !ok || got != val {
			return false
		}
	}
	return true
}

// maskAMPTargeting applies amp_targeting_merge to every stage it applies to.
func (e *Engine) maskAMPTargeting(v *view) {
	bids := e.ampMergedBids(v)
	if len(bids) < minStoredCandidates {
		return
	}
	conflicts := conflictingKeys(bids)
	if len(conflicts) == 0 {
		return
	}
	for stage, anchors := range ampTargetingAnchors {
		if !e.rules.Active(RuleAMPTargetingMerge, stage) {
			continue
		}
		for _, root := range e.stageRoots(v, stage) {
			for _, targeting := range anchorsOf(root, anchors) {
				maskMergedKeys(targeting, bids, conflicts)
			}
		}
	}
}

// maskMergedKeys masks the conflicting keys of one targeting map when some
// merge order of bids produces it.
func maskMergedKeys(targeting *node, bids []map[string]string, conflicts map[string]bool) {
	if targeting == nil || targeting.kind != kindObject {
		return
	}
	if !mergeOrderExists(bids, stringMembers([]*node{targeting})) {
		return
	}
	for i := range targeting.members {
		m := &targeting.members[i]
		if conflicts[m.key] && m.val != nil && m.val.kind == kindString {
			mask(m.val, "candidate:"+RuleAMPTargetingMerge)
		}
	}
}
