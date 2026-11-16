package stagedata

import (
	"bytes"
	"compress/gzip"
	"encoding/json"
	"net/http"
	"testing"

	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/adapters"
	"github.com/prebid/prebid-server/v4/exchange/entities"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func gzipped(t *testing.T, s string) []byte {
	t.Helper()
	var buf bytes.Buffer
	w := gzip.NewWriter(&buf)
	_, err := w.Write([]byte(s))
	require.NoError(t, err)
	require.NoError(t, w.Close())
	return buf.Bytes()
}

func TestRequestDataOf(t *testing.T) {
	tests := []struct {
		name        string
		in          *adapters.RequestData
		wantMethod  string
		wantHeaders parity.HeaderList
		wantImpIDs  []string
		wantBody    bool
	}{
		{name: "nil request", in: nil, wantHeaders: parity.HeaderList{}, wantImpIDs: []string{}},
		{
			name: "headers sorted and imp ids copied",
			in: &adapters.RequestData{
				Method: http.MethodPost, Uri: "https://b.example/x", Body: []byte(`{"a":1}`), ImpIDs: []string{"i1"},
				Headers: http.Header{"X-Prebid": {"pbs-go/1"}, "Content-Type": {"application/json"}},
			},
			wantMethod:  http.MethodPost,
			wantHeaders: parity.HeaderList{{"Content-Type", "application/json"}, {"X-Prebid", "pbs-go/1"}},
			wantImpIDs:  []string{"i1"},
			wantBody:    true,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := RequestDataOf(tt.in)
			assert.Equal(t, tt.wantMethod, got.Method)
			assert.Equal(t, tt.wantHeaders, got.Headers)
			assert.Equal(t, tt.wantImpIDs, got.ImpIDs)
			assert.Equal(t, tt.wantBody, got.Body != nil)
		})
	}
}

func TestRequestDataOfGzipBody(t *testing.T) {
	raw := gzipped(t, `{"a":1}`)
	got := RequestDataOf(&adapters.RequestData{Body: raw, Headers: http.Header{"Content-Encoding": {"gzip"}}})
	require.NotNil(t, got.Body)
	assert.Equal(t, parity.NewBody(raw, "gzip"), got.Body)
}

func TestBidderResponseOf(t *testing.T) {
	tests := []struct {
		name     string
		in       *adapters.BidderResponse
		wantNil  bool
		wantBids int
		wantJSON string
	}{
		{name: "nil response", in: nil, wantNil: true},
		{
			name: "bids keep order and lexical form",
			in: &adapters.BidderResponse{Currency: "EUR", Bids: []*adapters.TypedBid{
				{Bid: &openrtb2.Bid{ID: "b2", ImpID: "i", Price: 1.5, AdM: "<a>"}, BidType: openrtb_ext.BidTypeBanner, DealPriority: 3},
				{Bid: &openrtb2.Bid{ID: "b1", ImpID: "i", Price: 2}, BidType: openrtb_ext.BidTypeVideo, BidVideo: &openrtb_ext.ExtBidPrebidVideo{Duration: 15}},
			}},
			wantBids: 2,
			wantJSON: `{"currency":"EUR","bids":[{"bid":{"id":"b2","impid":"i","price":1.5,"adm":"<a>"},"type":"banner","deal_priority":3},{"bid":{"id":"b1","impid":"i","price":2},"type":"video","video":{"duration":15,"primary_category":""}}]}`,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := BidderResponseOf(tt.in)
			require.NoError(t, err)
			if tt.wantNil {
				assert.Nil(t, got)
				return
			}
			require.Len(t, got.Bids, tt.wantBids)
			raw, err := json.Marshal(got)
			require.NoError(t, err)
			assert.JSONEq(t, tt.wantJSON, string(raw))
			assert.Contains(t, string(got.Bids[0].Bid), "\\u003ca\\u003e", "jsonutil keeps Go's HTML escaping")
		})
	}
}

func TestSeatBidProjections(t *testing.T) {
	seatBids := []*entities.PbsOrtbSeatBid{
		{Seat: "zeta", Currency: "USD", Bids: []*entities.PbsOrtbBid{{Bid: &openrtb2.Bid{ID: "z"}, BidType: openrtb_ext.BidTypeBanner, AdapterCode: "zeta"}}},
		nil,
		{Seat: "alpha", Currency: "EUR"},
	}
	sorted := SeatBidsSorted(seatBids)
	require.Len(t, sorted, 2)
	assert.Equal(t, "alpha", sorted[0].Seat)
	assert.Equal(t, []Bid{}, sorted[0].Bids)
	assert.Equal(t, "zeta", sorted[1].Seat)
	assert.Equal(t, "banner", sorted[1].Bids[0].Type)
	assert.Equal(t, "zeta", sorted[1].Bids[0].AdapterCode)

	byMap := SeatBidMapOf(map[openrtb_ext.BidderName]*entities.PbsOrtbSeatBid{"stored": {Bids: []*entities.PbsOrtbBid{{Bid: &openrtb2.Bid{ID: "s"}}}}})
	assert.Equal(t, "stored", byMap["stored"].Seat, "empty seat takes the map key")

	assert.Equal(t, Bid{}, BidOf(nil))
}

func TestNonBidsOf(t *testing.T) {
	raw, err := jsonutil.Marshal(NonBidsOf(nil))
	require.NoError(t, err)
	assert.Equal(t, "{}", string(raw))
	in := map[string][]openrtb_ext.NonBid{"a": {{ImpId: "i", StatusCode: 301}}}
	assert.Equal(t, in, NonBidsOf(in))
}
