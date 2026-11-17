package server

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"

	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/adapters"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/currency"
	"github.com/prebid/prebid-server/v4/exchange"
	"github.com/prebid/prebid-server/v4/metrics"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/stagedata"
	"github.com/prebid/prebid-server/v4/parity/trace"
)

// Adapter route operations.
const (
	adapterOpRequests = "requests"
	adapterOpBids     = "bids"
)

// adapterRoute returns the route of POST /__parity/adapter/{bidder}/{op}.
func (s *Server) adapterRoute(bidder, op string) route {
	if op == adapterOpBids {
		return route{http.MethodPost: func(w http.ResponseWriter, r *http.Request) { s.adapterBids(w, r, bidder) }}
	}
	return route{http.MethodPost: func(w http.ResponseWriter, r *http.Request) { s.adapterRequests(w, r, bidder) }}
}

// adapterRequests serves POST /__parity/adapter/{bidder}/requests: it builds the
// bidder and calls MakeRequests only. requests and errors are always present;
// errors keep MakeRequests' order.
func (s *Server) adapterRequests(w http.ResponseWriter, r *http.Request, pathBidder string) {
	out, err := s.makeRequests(r, pathBidder)
	if err != nil {
		writeFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) makeRequests(r *http.Request, pathBidder string) (*parity.AdapterRequestsOutput, error) {
	var in parity.AdapterRequestsInput
	if err := readAdapterInput(r, &in); err != nil {
		return nil, err
	}
	if err := checkAdapterInput(in.Schema, in.SchemaVersion, parity.SchemaAdapterRequestsInput, pathBidder, in.Build, in.ExtraInfo); err != nil {
		return nil, err
	}
	bidder, err := s.buildBidder(in.Build)
	if err != nil {
		return nil, err
	}
	req, err := decodeBidRequest(in.Request)
	if err != nil {
		return nil, err
	}
	extra := extraRequestInfo(in.ExtraInfo)
	var reqs []*adapters.RequestData
	var errs []error
	err = runAdapter(r.Context(), in.Fakes, in.Build.Bidder+" MakeRequests", func() {
		reqs, errs = bidder.MakeRequests(req, &extra)
	})
	if err != nil {
		return nil, err
	}
	return &parity.AdapterRequestsOutput{
		Schema:        parity.SchemaAdapterRequestsOutput,
		SchemaVersion: parity.SchemaVersion,
		Impl:          parity.ImplGo,
		CaseID:        in.CaseID,
		Bidder:        in.Build.Bidder,
		Requests:      stagedata.RequestsOf(reqs).Requests,
		Errors:        errorList(errs),
	}, nil
}

// adapterBids serves POST /__parity/adapter/{bidder}/bids: it builds the bidder
// and calls MakeBids only. bidder_response (null when MakeBids returns none)
// and errors are always both present.
func (s *Server) adapterBids(w http.ResponseWriter, r *http.Request, pathBidder string) {
	out, err := s.makeBids(r, pathBidder)
	if err != nil {
		writeFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) makeBids(r *http.Request, pathBidder string) (*parity.AdapterBidsOutput, error) {
	var in parity.AdapterBidsInput
	if err := readAdapterInput(r, &in); err != nil {
		return nil, err
	}
	if err := checkAdapterInput(in.Schema, in.SchemaVersion, parity.SchemaAdapterBidsInput, pathBidder, in.Build, in.ExtraInfo); err != nil {
		return nil, err
	}
	bidder, err := s.buildBidder(in.Build)
	if err != nil {
		return nil, err
	}
	req, err := decodeBidRequest(in.Request)
	if err != nil {
		return nil, err
	}
	reqData, respData, err := bidsCall(in.RequestData, in.Response)
	if err != nil {
		return nil, err
	}
	var resp *adapters.BidderResponse
	var errs []error
	err = runAdapter(r.Context(), in.Fakes, in.Build.Bidder+" MakeBids", func() {
		resp, errs = bidder.MakeBids(req, reqData, respData)
	})
	if err != nil {
		return nil, err
	}
	projected, err := stagedata.BidderResponseOf(resp)
	if err != nil {
		return nil, fmt.Errorf("parity: bidder %s: %w", in.Build.Bidder, err)
	}
	return &parity.AdapterBidsOutput{
		Schema:         parity.SchemaAdapterBidsOutput,
		SchemaVersion:  parity.SchemaVersion,
		Impl:           parity.ImplGo,
		CaseID:         in.CaseID,
		Bidder:         in.Build.Bidder,
		BidderResponse: projected,
		Errors:         errorList(errs),
	}, nil
}

func readAdapterInput(r *http.Request, v any) error {
	raw, err := readDocument(r)
	if err != nil {
		return err
	}
	return decodeStrict(raw, v)
}

// checkAdapterInput validates the members shared by both adapter inputs. The
// bidder in the path must be build.bidder.
func checkAdapterInput(schema, version, want, pathBidder string, build parity.AdapterBuild, extra parity.ExtraRequestInfo) error {
	if err := checkHeader(schema, version, want); err != nil {
		return err
	}
	if build.Bidder != pathBidder {
		return badDocument("build.bidder %q does not match the route bidder %q", build.Bidder, pathBidder)
	}
	if build.Wrap != parity.WrapRaw && build.Wrap != parity.WrapInfoAware {
		return badDocument("build.wrap %q is not %s or %s", build.Wrap, parity.WrapRaw, parity.WrapInfoAware)
	}
	switch openrtb_ext.BidType(extra.PreferredMediaType) {
	case "", openrtb_ext.BidTypeBanner, openrtb_ext.BidTypeVideo, openrtb_ext.BidTypeAudio, openrtb_ext.BidTypeNative:
		return nil
	}
	return badDocument("extra_info.preferred_media_type %q is not a media type", extra.PreferredMediaType)
}

// buildBidder builds the adapter under test the way the exchange does
// (exchange.BuildParityBidder). Omitted adapter_config and server default to
// what BuildAdapters derives from the bidder info and host config. The bidder
// must have a bidder info; disabled bidders can still be built.
func (s *Server) buildBidder(build parity.AdapterBuild) (adapters.Bidder, error) {
	info, ok := s.deps.BidderInfos[build.Bidder]
	if !ok {
		return nil, badDocument("unknown bidder %q", build.Bidder)
	}
	adapterCfg := exchange.ParityAdapterConfig(info)
	if build.AdapterConfig != nil {
		adapterCfg = adapterConfigOf(*build.AdapterConfig)
	}
	server := config.Server{}
	if s.deps.Cfg != nil {
		server = exchange.ParityServerConfig(s.deps.Cfg)
	}
	if build.Server != nil {
		server = config.Server{ExternalUrl: build.Server.ExternalURL, GvlID: build.Server.GvlID, DataCenter: build.Server.DataCenter}
	}
	bidder, err := exchange.BuildParityBidder(openrtb_ext.BidderName(build.Bidder), adapterCfg, server, info, build.Wrap == parity.WrapInfoAware)
	if err != nil {
		return nil, badDocument("%v", err)
	}
	return bidder, nil
}

func adapterConfigOf(in parity.AdapterConfig) config.Adapter {
	out := config.Adapter{
		Endpoint:         in.Endpoint,
		ExtraAdapterInfo: in.ExtraInfo,
		PlatformID:       in.PlatformID,
		AppSecret:        in.AppSecret,
	}
	if in.XAPI != nil {
		out.XAPI = config.AdapterXAPI{Username: in.XAPI.Username, Password: in.XAPI.Password, Tracker: in.XAPI.Tracker}
	}
	return out
}

// decodeBidRequest decodes the OpenRTB request the way adapterstest decodes a
// fixture's mockBidRequest, with encoding/json
// (adapters/adapterstest/test_json.go:112), so the adapter gets the fixture's
// inputs exactly: a member spelled null, such as "ext": null, reaches it as
// json.RawMessage("null"), where jsonutil would leave it empty.
func decodeBidRequest(raw []byte) (*openrtb2.BidRequest, error) {
	if len(raw) == 0 || string(raw) == "null" {
		return nil, badDocument("request is required")
	}
	req := &openrtb2.BidRequest{}
	if err := json.Unmarshal(raw, req); err != nil {
		return nil, badDocument("request: %v", err)
	}
	return req, nil
}

// extraRequestInfo converts extra_info. Without currency_rates the adapter gets
// no conversions, exactly like adapterstest without request currency rates.
func extraRequestInfo(in parity.ExtraRequestInfo) adapters.ExtraRequestInfo {
	out := adapters.ExtraRequestInfo{
		PbsEntryPoint:              metrics.RequestType(in.PbsEntryPoint),
		GlobalPrivacyControlHeader: in.GlobalPrivacyControlHeader,
		PreferredMediaType:         openrtb_ext.BidType(in.PreferredMediaType),
	}
	if in.CurrencyRates != nil {
		out.CurrencyConversions = currency.NewRates(in.CurrencyRates.Conversions)
	}
	return out
}

// bidsCall converts request_data and response into MakeBids arguments. Bodies
// are their wire bytes; an empty header list is a nil http.Header, as in
// adapterstest.
func bidsCall(reqIn parity.AdapterRequestData, respIn parity.AdapterHTTPResponse) (*adapters.RequestData, *adapters.ResponseData, error) {
	reqBody, err := reqIn.Body.WireBytes()
	if err != nil {
		return nil, nil, badDocument("request_data.body: %v", err)
	}
	respBody, err := respIn.Body.WireBytes()
	if err != nil {
		return nil, nil, badDocument("response.body: %v", err)
	}
	reqData := &adapters.RequestData{
		Method:  reqIn.Method,
		Uri:     reqIn.URI,
		Body:    reqBody,
		Headers: headerOrNil(reqIn.Headers),
		ImpIDs:  reqIn.ImpIDs,
	}
	respData := &adapters.ResponseData{StatusCode: respIn.Status, Body: respBody, Headers: headerOrNil(respIn.Headers)}
	return reqData, respData, nil
}

// headerOrNil keeps each name exactly as given (HeaderList names keep their
// case), like adapterstest decoding fixture headers straight into an
// http.Header: a non-canonical name is not found by Header.Get, in both.
func headerOrNil(list parity.HeaderList) http.Header {
	if len(list) == 0 {
		return nil
	}
	h := make(http.Header, len(list))
	for _, kv := range list {
		h[kv[0]] = append(h[kv[0]], kv[1])
	}
	return h
}

// runAdapter runs call under the exec lock with the deterministic fakes set to
// fakes, so adapter calls never interleave with an exec case. The session it
// publishes has no case ID, so background calls in the meantime are not
// attributed to the adapter case. A panic in the adapter becomes an error
// naming label.
func runAdapter(ctx context.Context, fakes *parity.FakeValues, label string, call func()) (err error) {
	_, end, err := trace.BeginContext(ctx, &parity.Envelope{Fakes: fakes})
	if err != nil {
		return &errDocument{status: http.StatusServiceUnavailable, msg: err.Error()}
	}
	defer end()
	defer func() {
		if recovered := recover(); recovered != nil {
			err = fmt.Errorf("parity: bidder %s panicked: %v", label, recovered)
		}
	}()
	call()
	return nil
}

// errorList records errors in their original order (never nil).
func errorList(errs []error) []parity.ErrorRecord {
	out := make([]parity.ErrorRecord, 0, len(errs))
	for _, err := range errs {
		out = append(out, trace.ErrorRecordOf(err))
	}
	return out
}
