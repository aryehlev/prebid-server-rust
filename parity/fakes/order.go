package fakes

import (
	"sort"

	"github.com/prebid/prebid-server/v4/openrtb_ext"
)

// Map iteration order.
//
// Some adapters group a bid request's imps in a map and, while ranging over
// it, overwrite the shared request's Imp with each group before marshaling
// it. MakeBids later reads the same request object, so it sees the imps of
// whichever group Go's map iteration visited last, and that changes which bids
// survive or which media type they get (adtelligent, datablocks, trafficgate,
// zeroclickfraud). Two Go processes then disagree on the same case. In parity
// mode those adapters range over MapKeys instead, which visits the groups in
// the ascending order the contract (fakes.json orders) names for each one. The
// same holds for the server code whose outcome follows a map's iteration
// order: first party data, the request validation errors, the auction
// winners, category dedupe and the user sync chooser.

// MapKeys returns the keys of m in the order an adapter ranges over them.
// While the fakes are installed (parity mode) the keys are sorted ascending by
// less; otherwise they come in Go's map iteration order, exactly what ranging
// over m gives.
func MapKeys[K comparable, V any](m map[K]V, less func(a, b K) bool) []K {
	keys := make([]K, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	if active.Load() != nil {
		sort.Slice(keys, func(i, j int) bool { return less(keys[i], keys[j]) })
	}
	return keys
}

// The orders of the adapters that range over a map of imp groups (fakes.json
// orders). adtelligent orders its sourceId groups with cmp.Less[int].

// LessDatablocks orders datablocks imp groups by sourceId.
func LessDatablocks(a, b openrtb_ext.ExtImpDatablocks) bool { return a.SourceId < b.SourceId }

// LessZeroClickFraud orders zeroclickfraud imp groups by (sourceId, host).
func LessZeroClickFraud(a, b openrtb_ext.ExtImpZeroClickFraud) bool {
	if a.SourceId != b.SourceId {
		return a.SourceId < b.SourceId
	}
	return a.Host < b.Host
}

// LessTrafficGate orders trafficgate imp groups by (placementId, host).
func LessTrafficGate(a, b openrtb_ext.ExtImpTrafficGate) bool {
	if a.PlacementID != b.PlacementID {
		return a.PlacementID < b.PlacementID
	}
	return a.Host < b.Host
}
