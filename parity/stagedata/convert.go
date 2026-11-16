package stagedata

import (
	"encoding/json"
	"fmt"
	"sort"

	"github.com/prebid/prebid-server/v4/adapters"
	"github.com/prebid/prebid-server/v4/exchange/entities"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/transport"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// RequestDataOf projects one adapter request. Headers are flattened in
// canonical-name order (http.Header has no wire order) and the body is recorded
// with the request's own Content-Encoding. A nil request yields an empty
// projection.
func RequestDataOf(req *adapters.RequestData) parity.AdapterRequestData {
	out := parity.AdapterRequestData{Headers: parity.HeaderList{}, ImpIDs: []string{}}
	if req == nil {
		return out
	}
	out.Method = req.Method
	out.URI = req.Uri
	out.Headers = transport.HeaderListOf(req.Headers)
	out.Body = parity.NewBody(req.Body, req.Headers.Get("Content-Encoding"))
	if req.ImpIDs != nil {
		out.ImpIDs = append(out.ImpIDs, req.ImpIDs...)
	}
	return out
}

// RequestsOf projects MakeRequests output in request order.
func RequestsOf(reqs []*adapters.RequestData) AdapterRequests {
	out := AdapterRequests{Requests: make([]parity.AdapterRequestData, 0, len(reqs))}
	for _, req := range reqs {
		out.Requests = append(out.Requests, RequestDataOf(req))
	}
	return out
}

// ResponseDataOf projects the HTTP response handed to MakeBids (nil for none).
func ResponseDataOf(resp *adapters.ResponseData) *parity.AdapterHTTPResponse {
	if resp == nil {
		return nil
	}
	return &parity.AdapterHTTPResponse{
		Status:  resp.StatusCode,
		Headers: transport.HeaderListOf(resp.Headers),
		Body:    parity.NewBody(resp.Body, ""),
	}
}

// BidderResponseOf projects MakeBids' bidder response (nil for none). Bids keep
// their order; each bid object is marshaled with jsonutil.Marshal.
func BidderResponseOf(resp *adapters.BidderResponse) (*parity.AdapterBidderResponse, error) {
	if resp == nil {
		return nil, nil
	}
	out := &parity.AdapterBidderResponse{Currency: resp.Currency, Bids: make([]parity.TypedBid, 0, len(resp.Bids))}
	for i, bid := range resp.Bids {
		typed, err := typedBidOf(bid)
		if err != nil {
			return nil, fmt.Errorf("stagedata: bidder response bids[%d]: %w", i, err)
		}
		out.Bids = append(out.Bids, typed)
	}
	if resp.FledgeAuctionConfigs != nil {
		raw, err := jsonutil.Marshal(resp.FledgeAuctionConfigs)
		if err != nil {
			return nil, fmt.Errorf("stagedata: bidder response fledge auction configs: %w", err)
		}
		out.FledgeAuctionConfigs = raw
	}
	return out, nil
}

func typedBidOf(bid *adapters.TypedBid) (parity.TypedBid, error) {
	if bid == nil {
		return parity.TypedBid{Bid: json.RawMessage("null")}, nil
	}
	out := parity.TypedBid{Type: string(bid.BidType), Seat: string(bid.Seat), DealPriority: bid.DealPriority}
	raw, err := jsonutil.Marshal(bid.Bid)
	if err != nil {
		return out, fmt.Errorf("bid: %w", err)
	}
	out.Bid = raw
	if bid.BidVideo != nil {
		if out.Video, err = jsonutil.Marshal(bid.BidVideo); err != nil {
			return out, fmt.Errorf("video: %w", err)
		}
	}
	if bid.BidMeta != nil {
		if out.Meta, err = jsonutil.Marshal(bid.BidMeta); err != nil {
			return out, fmt.Errorf("meta: %w", err)
		}
	}
	return out, nil
}

// BidOf projects one exchange bid.
func BidOf(bid *entities.PbsOrtbBid) Bid {
	if bid == nil {
		return Bid{}
	}
	return Bid{
		Bid:               bid.Bid,
		Type:              string(bid.BidType),
		Meta:              bid.BidMeta,
		Video:             bid.BidVideo,
		Events:            bid.BidEvents,
		Floors:            bid.BidFloors,
		Targets:           bid.BidTargets,
		DealPriority:      bid.DealPriority,
		DealTierSatisfied: bid.DealTierSatisfied,
		GeneratedBidID:    bid.GeneratedBidID,
		OriginalBidCPM:    bid.OriginalBidCPM,
		OriginalBidCur:    bid.OriginalBidCur,
		TargetBidderCode:  bid.TargetBidderCode,
		AdapterCode:       string(bid.AdapterCode),
	}
}

// SeatBidOf projects one exchange seat bid; seat overrides an empty Seat field
// (stored auction responses leave it empty and key the seat by map key).
func SeatBidOf(seat string, sb *entities.PbsOrtbSeatBid) SeatBid {
	out := SeatBid{Seat: seat, Bids: []Bid{}}
	if sb == nil {
		return out
	}
	if sb.Seat != "" {
		out.Seat = sb.Seat
	}
	out.Currency = sb.Currency
	out.FledgeAuctionConfigs = sb.FledgeAuctionConfigs
	out.HTTPCalls = sb.HttpCalls
	for _, bid := range sb.Bids {
		out.Bids = append(out.Bids, BidOf(bid))
	}
	return out
}

// SeatBidsSorted projects a bidder's seat bids sorted by seat (requestBid builds
// the list from map iteration). Nil entries are skipped.
func SeatBidsSorted(seatBids []*entities.PbsOrtbSeatBid) []SeatBid {
	out := make([]SeatBid, 0, len(seatBids))
	for _, sb := range seatBids {
		if sb != nil {
			out = append(out, SeatBidOf("", sb))
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Seat < out[j].Seat })
	return out
}

// SeatBidMapOf projects the exchange's per-seat bids map.
func SeatBidMapOf(seatBids map[openrtb_ext.BidderName]*entities.PbsOrtbSeatBid) SeatBids {
	out := make(SeatBids, len(seatBids))
	for seat, sb := range seatBids {
		out[string(seat)] = SeatBidOf(string(seat), sb)
	}
	return out
}

// NonBidsOf copies a seat non-bid map so that a nil map records as {}.
func NonBidsOf(nonBids map[string][]openrtb_ext.NonBid) map[string][]openrtb_ext.NonBid {
	out := make(map[string][]openrtb_ext.NonBid, len(nonBids))
	for seat, list := range nonBids {
		out[seat] = list
	}
	return out
}

// BoolPtr returns a pointer to b (for the nullable decision members).
func BoolPtr(b bool) *bool {
	return &b
}
