package endpointtrace

import (
	"encoding/json"

	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/exchange"
	"github.com/prebid/prebid-server/v4/gdpr"
	"github.com/prebid/prebid-server/v4/hooks/hookexecution"
	"github.com/prebid/prebid-server/v4/metrics"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/stored_responses"
)

// Every hook is a fast path: one atomic load and a nil check, with no
// allocation (TestHooksDisabledDoNotAllocate). The work happens in the record*
// functions (record.go), which also check that the stage is captured.

// Entrypoint records the request body after the entrypoint hook stage and the
// stage's reject error.
func Entrypoint(body []byte, reject *hookexecution.RejectError) {
	if s := trace.Current(); s != nil {
		recordEntrypoint(s, body, reject)
	}
}

// RawAuction records the request body after the raw-auction hook stage and the
// stage's reject error.
func RawAuction(body []byte, reject *hookexecution.RejectError) {
	if s := trace.Current(); s != nil {
		recordRawAuction(s, body, reject)
	}
}

// StoredFetch records the stored requests and imps a fetch returned.
func StoredFetch(storedRequestID string, hasStoredRequest bool, requests, imps map[string]json.RawMessage, errs []error) {
	if s := trace.Current(); s != nil {
		recordStoredFetch(s, storedRequestID, hasStoredRequest, requests, imps, errs)
	}
}

// StoredRequestFetch records a fetch of exactly one stored request (the video
// endpoint's stored video request).
func StoredRequestFetch(storedRequestID string, request json.RawMessage, errs []error) {
	if s := trace.Current(); s != nil {
		recordStoredRequestFetch(s, storedRequestID, request, errs)
	}
}

// Account records the account the request resolved to (nil when resolution
// failed) and the account-ID and fetch errors.
func Account(accountID string, account *config.Account, errs []error) {
	if s := trace.Current(); s != nil {
		recordAccount(s, accountID, account, errs)
	}
}

// Merged records the request bytes after the stored and default request merge.
func Merged(requestJSON []byte) {
	if s := trace.Current(); s != nil {
		recordMerged(s, requestJSON)
	}
}

// MergedRequest records an already parsed merged request (AMP: the stored
// request after the query overrides) and the errors the merge returned.
func MergedRequest(req *openrtb2.BidRequest, errs []error) {
	if s := trace.Current(); s != nil {
		recordMergedRequest(s, req, errs)
	}
}

// MergeError records an error merging or parsing the merged request.
func MergeError(err error) {
	if s := trace.Current(); s != nil {
		recordStageError(s, stageMerged, err)
	}
}

// MergeErrors records errors merging or parsing the merged request.
func MergeErrors(errs []error) {
	if s := trace.Current(); s != nil {
		recordStageErrors(s, stageMerged, errs)
	}
}

// Normalized records the request after the normalization steps, as rebuilding
// rw would produce it, without rebuilding rw.
func Normalized(rw *openrtb_ext.RequestWrapper) {
	if s := trace.Current(); s != nil {
		recordRequest(s, stageNormalized, rw, nil)
	}
}

// NormalizeError records the error of a failing normalization step.
func NormalizeError(err error) {
	if s := trace.Current(); s != nil {
		recordStageError(s, stageNormalized, err)
	}
}

// NormalizeErrors records the errors a normalization step returned.
func NormalizeErrors(errs []error) {
	if s := trace.Current(); s != nil {
		recordStageErrors(s, stageNormalized, errs)
	}
}

// StoredResponses records the stored auction and bid responses and the imp-ID
// replacement flags ProcessStoredResponses returned.
func StoredResponses(auction stored_responses.ImpsWithBidResponses, bid stored_responses.ImpBidderStoredResp, replaceImpID stored_responses.BidderImpReplaceImpID, errs []error) {
	if s := trace.Current(); s != nil {
		recordStoredResponses(s, auction, bid, replaceImpID, errs)
	}
}

// Validated records the request after validateRequest, as rebuilding rw would
// produce it, and the validation errors and warnings.
func Validated(rw *openrtb_ext.RequestWrapper, errs []error) {
	if s := trace.Current(); s != nil {
		recordRequest(s, stageValidated, rw, errs)
	}
}

// GDPR records the outcome of processGDPR.
func GDPR(tcf2 gdpr.TCF2ConfigReader, signal gdpr.Signal, enforced bool, requestType metrics.RequestType, errs []error) {
	if s := trace.Current(); s != nil {
		recordGDPR(s, tcf2, signal, enforced, requestType, errs)
	}
}

// AuctionResult records the error HoldAuction returned on exchange.response.
func AuctionResult(err error) {
	if s := trace.Current(); s != nil {
		recordResultError(s, stageExchangeResponse, err)
	}
}

// SeatNonBidResult records the error of writing seat non-bids into the
// response ext.
func SeatNonBidResult(err error) {
	if s := trace.Current(); s != nil {
		recordResultError(s, stageSeatNonBids, err)
	}
}

// AuctionResponse records the response after the auction-response hook stage.
func AuctionResponse(resp *openrtb2.BidResponse) {
	if s := trace.Current(); s != nil {
		recordValue(s, stageAuctionResponse, resp)
	}
}

// AMPTargeting records the AMP response before the exitpoint hook stage. It is
// generic so the handler passes its AmpResponse value without an interface
// conversion (which would allocate on the disabled path).
func AMPTargeting[T any](resp T) {
	if s := trace.Current(); s != nil {
		recordValue(s, stageAMPTargeting, resp)
	}
}

// Exitpoint records the response object after the exitpoint hook stage.
func Exitpoint(resp any) {
	if s := trace.Current(); s != nil {
		recordValue(s, stageExitpoint, resp)
	}
}

// ResponseEncodeError records the error of encoding the final response.
func ResponseEncodeError(err error) {
	if s := trace.Current(); s != nil {
		recordStageError(s, stageExitpoint, err)
	}
}

// HookOutcomes records every hook stage outcome collected so far.
func HookOutcomes(executor hookexecution.HookStageExecutor) {
	if s := trace.Current(); s != nil {
		recordHookOutcomes(s, executor)
	}
}

// VideoPods records the video pod errors and the simplified video response,
// plus the error that ended pod processing. It is generic so the handler passes
// its []PodError without an interface conversion.
func VideoPods[P any](podErrors []P, resp *openrtb_ext.BidResponseVideo, err error) {
	if s := trace.Current(); s != nil {
		recordVideoPods(s, podErrors, resp, err)
	}
}

// VideoCache records the debug log the video endpoint put to Prebid Cache.
func VideoCache(debugLog *exchange.DebugLog, err error) {
	if s := trace.Current(); s != nil {
		recordVideoCache(s, debugLog, err)
	}
}
