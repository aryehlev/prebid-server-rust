// Package p holds Containers, a struct of the containers that typegen maps through pbs-openrtb's
// gojson_bridge, in a package of the name typegen's tests give the package they describe, so
// that Go's errors name the type as the generated code does (p.Containers).
package p

import "encoding/json"

// Containers holds, one field each, the containers that typegen maps through pbs-openrtb's
// gojson_bridge and no OpenRTB type has: a Go int, uint or json.RawMessage inside a pointer,
// slice or map. They are the shapes of openrtb_ext fields (ExtImpPrebid.Bidder,
// ExtBidResponse.ResponseTimeMillis, ExtRequestTargeting.DurationRangeSec, ImpExtConcert.Sizes)
// and their siblings. pbs-openrtb's tests/parity/containers.rs holds the struct typegen
// generates for the same declarations (pbs-devtools' maps_containers_of_bridged_types test
// checks that it does) and replays containers.jsonl through it.
type Containers struct {
	Bidder             map[string]json.RawMessage `json:"bidder,omitempty"`
	ResponseTimeMillis map[string]int             `json:"responsetimemillis,omitempty"`
	DurationRangeSec   []int                      `json:"durationrangesec"`
	Sizes              *[][]int                   `json:"sizes,omitempty"`
	Raws               []json.RawMessage          `json:"raws,omitempty"`
	RawPtr             *json.RawMessage           `json:"rawptr,omitempty"`
	Counts             []*int                     `json:"counts,omitempty"`
	Wide               map[string]uint            `json:"wide,omitempty"`
}
