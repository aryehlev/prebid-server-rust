package endpointtrace

import (
	"encoding/json"
	"fmt"

	"github.com/mitchellh/mapstructure"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/stored_responses"
)

// storedFetchPayload is the stored.fetch payload. Stored data is kept as Body
// so stored JSON that does not parse is still recorded exactly.
type storedFetchPayload struct {
	StoredRequestID  string                  `json:"stored_request_id"`
	HasStoredRequest bool                    `json:"has_stored_request"`
	Requests         map[string]*parity.Body `json:"requests"`
	Imps             map[string]*parity.Body `json:"imps"`
}

// accountPayload is the account.resolved payload. Account is the
// mapstructure dump of the resolved config.Account (accountDump), or nil.
type accountPayload struct {
	ID      string         `json:"id"`
	Account map[string]any `json:"account"`
}

// accountDump encodes account the way the design dumps configuration
// (mapstructure.Decode into a map, as GET /__parity/config does), so keys are
// the mapstructure config keys. It returns nil for a nil account.
func accountDump(account *config.Account) (map[string]any, error) {
	if account == nil {
		return nil, nil
	}
	out := map[string]any{}
	if err := mapstructure.Decode(account, &out); err != nil {
		return nil, fmt.Errorf("dumping account %q: %w", account.ID, err)
	}
	return out, nil
}

// storedResponsesPayload is the stored.responses payload.
type storedResponsesPayload struct {
	Auction      map[string]*parity.Body            `json:"auction"`
	Bid          map[string]map[string]*parity.Body `json:"bid"`
	ReplaceImpID map[string]map[string]bool         `json:"replace_impid"`
}

// videoPodsPayload is the video.pods payload. P is the endpoint's PodError,
// recorded as Go marshals it.
type videoPodsPayload[P any] struct {
	PodErrors []P                           `json:"pod_errors"`
	Response  *openrtb_ext.BidResponseVideo `json:"response"`
}

// videoCachePayload is the video.cache payload: the debug log entry put to
// Prebid Cache (data is the cached string).
type videoCachePayload struct {
	CacheKey  string `json:"cache_key"`
	CacheType string `json:"cache_type"`
	TTL       int64  `json:"ttl"`
	Data      string `json:"data"`
}

func newStoredResponsesPayload(auction stored_responses.ImpsWithBidResponses, bid stored_responses.ImpBidderStoredResp, replaceImpID stored_responses.BidderImpReplaceImpID) storedResponsesPayload {
	p := storedResponsesPayload{
		Auction:      bodies(auction),
		Bid:          make(map[string]map[string]*parity.Body, len(bid)),
		ReplaceImpID: make(map[string]map[string]bool, len(replaceImpID)),
	}
	for impID, byBidder := range bid {
		p.Bid[impID] = bodies(byBidder)
	}
	for bidder, byImp := range replaceImpID {
		flags := make(map[string]bool, len(byImp))
		for impID, replace := range byImp {
			flags[impID] = replace
		}
		p.ReplaceImpID[bidder] = flags
	}
	return p
}

// bodies converts stored JSON by ID to Bodies. A nil map becomes an empty one:
// Go's nil-versus-empty distinction is not observable and must not be diffed.
func bodies(raw map[string]json.RawMessage) map[string]*parity.Body {
	out := make(map[string]*parity.Body, len(raw))
	for id, data := range raw {
		out[id] = parity.NewBody(data, "")
	}
	return out
}
