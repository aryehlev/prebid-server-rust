package main

import (
	"bytes"
	"encoding/json"
	"math"

	"github.com/prebid/openrtb/v20/adcom1"
	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/openrtb/v20/openrtb3"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// encodeCase is a value that no JSON document decodes to, by name; the parity tests build the
// same value in Rust (tests/parity/encode.rs).
type encodeCase struct {
	name  string
	value interface{}
}

// encodeCases are values with NaN or infinite floats, at different depths and with fields
// encoded after them (jsoniter names each of those fields in its error), and with invalid
// json.RawMessage bytes (jsonutil.Marshal writes nothing for them, json.Marshal fails).
func encodeCases() []encodeCase {
	nan, inf := math.NaN(), math.Inf(1)
	one := 1
	return []encodeCase{
		{"bid price NaN", &openrtb2.Bid{ID: "b", Price: nan, W: 3}},
		{"bid price -Inf", openrtb2.Bid{Price: math.Inf(-1)}},
		{"second bid price +Inf", &openrtb2.BidResponse{
			ID: "r",
			SeatBid: []openrtb2.SeatBid{{
				Bid:  []openrtb2.Bid{{ID: "1", Price: 1}, {ID: "2", Price: inf}},
				Seat: "s",
			}},
			Cur: "USD",
		}},
		{"imp bidfloor NaN then qty and refresh", &openrtb2.BidRequest{
			ID: "x",
			Imp: []openrtb2.Imp{{
				ID:       "i",
				BidFloor: nan,
				Qty:      &openrtb2.Qty{Multiplier: 2},
				Refresh:  &openrtb2.Refresh{Count: &one},
			}},
		}},
		{"imp bidfloor NaN then a second imp", &openrtb2.BidRequest{
			ID:  "x",
			Imp: []openrtb2.Imp{{ID: "i", BidFloor: nan}, {ID: "j", Banner: &openrtb2.Banner{}}},
		}},
		{"imp bidfloor NaN then qty multiplier +Inf", &openrtb2.BidRequest{
			ID:  "x",
			Imp: []openrtb2.Imp{{ID: "i", BidFloor: nan, Qty: &openrtb2.Qty{Multiplier: inf}}},
		}},
		{"deal durfloors bidfloor NaN", &openrtb2.BidRequest{
			Imp: []openrtb2.Imp{{PMP: &openrtb2.PMP{Deals: []openrtb2.Deal{{
				ID:        "d",
				DurFloors: []openrtb2.DurFloors{{MinDur: 1, BidFloor: nan}},
				Ext:       json.RawMessage(`{"a":1}`),
			}}}}},
		}},
		{"video mincpmpersec +Inf", &openrtb2.Video{MinCPMPerSec: inf, PodDur: 3}},
		{"bid ext invalid", &openrtb2.Bid{ID: "b", Ext: json.RawMessage(`{bad}`), Price: 1}},
		{"bid ext invalid then price NaN", &openrtb2.SeatBid{
			Bid: []openrtb2.Bid{{Ext: json.RawMessage(`{bad}`)}, {Price: nan}},
		}},
		{"bid price NaN then ext invalid", &openrtb2.SeatBid{
			Bid: []openrtb2.Bid{{Price: nan}, {Ext: json.RawMessage(`{bad}`)}},
			Ext: json.RawMessage(`{"a":1}`),
		}},
		{"metric value NaN after banner format", &openrtb2.Imp{
			Banner: &openrtb2.Banner{Format: []openrtb2.Format{{W: 1}}},
			Metric: []openrtb2.Metric{{Value: nan}},
		}},
		{"geo lat NaN then user", &openrtb2.BidRequest{
			Device: &openrtb2.Device{Geo: &openrtb2.Geo{Lat: &nan}, UA: "u"},
			User:   &openrtb2.User{ID: "u"},
		}},
		{"adcom1 geo lon -Inf", &adcom1.Geo{Lat: 1, Lon: math.Inf(-1), Country: "c"}},
		{"openrtb3 bid price NaN", &openrtb3.Body{OpenRTB: openrtb3.OpenRTB{
			Ver: "3.0",
			Response: &openrtb3.Response{
				ID:      "r",
				SeatBid: []openrtb3.SeatBid{{Seat: "s", Bid: []openrtb3.Bid{{ID: "b", Price: nan}}}},
			},
		}}},
		{"openrtb3 item flr +Inf", &openrtb3.Item{ID: "i", Flr: inf, FlrCur: "USD"}},
	}
}

// writeEncodings writes encode.tsv.
func writeEncodings(path string, cases []encodeCase) {
	rw := newRowWriter(path)
	for _, c := range cases {
		cells := []string{c.name}
		data, err := jsonutil.Marshal(c.value)
		cells = append(cells, encoded(data, err))
		data, err = json.Marshal(c.value)
		cells = append(cells, encoded(data, err))
		var buf bytes.Buffer
		enc := json.NewEncoder(&buf)
		enc.SetEscapeHTML(false)
		err = enc.Encode(c.value)
		cells = append(cells, encoded(buf.Bytes(), err))
		rw.row(cells...)
	}
	rw.close()
}

// encoded is an encode.tsv encoding: "'" and the bytes, or "!" and the error text.
func encoded(data []byte, err error) string {
	if err != nil {
		return "!" + err.Error()
	}
	return "'" + string(data)
}
