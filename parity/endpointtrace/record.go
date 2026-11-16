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
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/shadow"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/stored_responses"
)

// Short names for the stages this package records.
const (
	stageEntrypoint       = parity.StageHooksEntrypoint
	stageStoredFetch      = parity.StageStoredFetch
	stageAccount          = parity.StageAccountResolved
	stageRawAuction       = parity.StageHooksRawAuction
	stageMerged           = parity.StageRequestMerged
	stageNormalized       = parity.StageRequestNormalized
	stageStoredResponses  = parity.StageStoredResponses
	stageValidated        = parity.StageRequestValidated
	stageGDPR             = parity.StageGDPRContext
	stageExchangeResponse = parity.StageExchangeResponse
	stageSeatNonBids      = parity.StageSeatNonBids
	stageAuctionResponse  = parity.StageHooksAuctionResponse
	stageAMPTargeting     = parity.StageAMPTargetingResponse
	stageExitpoint        = parity.StageHooksExitpoint
	stageHookOutcomes     = parity.StageHookOutcomes
	stageVideoPods        = parity.StageVideoPods
	stageVideoCache       = parity.StageVideoCache
)

// unkeyed is the key of every stage recorded here.
const unkeyed = parity.UnkeyedStage

// jsonNull is recorded where a value is absent altogether, such as the AMP
// entrypoint body or a request snapshot that failed.
var jsonNull = []byte("null")

func recordEntrypoint(s *trace.Session, body []byte, reject *hookexecution.RejectError) {
	recordBody(s, stageEntrypoint, body, reject)
}

func recordRawAuction(s *trace.Session, body []byte, reject *hookexecution.RejectError) {
	recordBody(s, stageRawAuction, body, reject)
}

// recordBody records a request body as a hook stage returned it, plus the
// stage's reject error ([] when it did not reject).
func recordBody(s *trace.Session, stage parity.StageID, body []byte, reject *hookexecution.RejectError) {
	if !s.Wants(stage) {
		return
	}
	recordRawOrNull(s, stage, body)
	var errs []error
	if reject != nil {
		errs = []error{reject}
	}
	s.Errors(stage, unkeyed, errs)
}

func recordStoredFetch(s *trace.Session, storedRequestID string, hasStoredRequest bool, requests, imps map[string]json.RawMessage, errs []error) {
	if !s.Wants(stageStoredFetch) {
		return
	}
	s.Record(stageStoredFetch, unkeyed, storedFetchPayload{
		StoredRequestID:  storedRequestID,
		HasStoredRequest: hasStoredRequest,
		Requests:         bodies(requests),
		Imps:             bodies(imps),
	})
	s.Errors(stageStoredFetch, unkeyed, errs)
}

func recordStoredRequestFetch(s *trace.Session, storedRequestID string, request json.RawMessage, errs []error) {
	requests := map[string]json.RawMessage{}
	if request != nil {
		requests[storedRequestID] = request
	}
	recordStoredFetch(s, storedRequestID, true, requests, nil, errs)
}

func recordAccount(s *trace.Session, accountID string, account *config.Account, errs []error) {
	if !s.Wants(stageAccount) {
		return
	}
	dump, err := accountDump(account)
	if err != nil {
		s.Note("%s: %v", stageAccount, err)
	}
	s.Record(stageAccount, unkeyed, accountPayload{ID: accountID, Account: dump})
	s.Errors(stageAccount, unkeyed, errs)
}

func recordMerged(s *trace.Session, requestJSON []byte) {
	if !s.Wants(stageMerged) {
		return
	}
	recordRawOrNull(s, stageMerged, requestJSON)
	s.Errors(stageMerged, unkeyed, nil)
}

func recordMergedRequest(s *trace.Session, req *openrtb2.BidRequest, errs []error) {
	if !s.Wants(stageMerged) {
		return
	}
	s.Record(stageMerged, unkeyed, req)
	s.Errors(stageMerged, unkeyed, errs)
}

// recordRequest records the request rw would rebuild to, plus errs. The live
// wrapper is never rebuilt: shadow rebuilds a deep copy. A failed snapshot is
// recorded as null with a trace note.
func recordRequest(s *trace.Session, stage parity.StageID, rw *openrtb_ext.RequestWrapper, errs []error) {
	if !s.Wants(stage) {
		return
	}
	if req, err := shadow.RebuiltRequest(rw); err != nil {
		s.Note("%s: request snapshot failed: %v", stage, err)
		s.RecordRaw(stage, unkeyed, jsonNull)
	} else {
		s.Record(stage, unkeyed, req)
	}
	s.Errors(stage, unkeyed, errs)
}

func recordStoredResponses(s *trace.Session, auction stored_responses.ImpsWithBidResponses, bid stored_responses.ImpBidderStoredResp, replaceImpID stored_responses.BidderImpReplaceImpID, errs []error) {
	if !s.Wants(stageStoredResponses) {
		return
	}
	s.Record(stageStoredResponses, unkeyed, newStoredResponsesPayload(auction, bid, replaceImpID))
	s.Errors(stageStoredResponses, unkeyed, errs)
}

func recordGDPR(s *trace.Session, tcf2 gdpr.TCF2ConfigReader, signal gdpr.Signal, enforced bool, requestType metrics.RequestType, errs []error) {
	if !s.Wants(stageGDPR) {
		return
	}
	s.Record(stageGDPR, unkeyed, newGDPRPayload(tcf2, signal, enforced, exchange.ChannelTypeMap[requestType]))
	s.Errors(stageGDPR, unkeyed, errs)
}

// recordResultError attaches err to a stage another package records the json
// of, so that record is never replaced. No error records [].
func recordResultError(s *trace.Session, stage parity.StageID, err error) {
	s.Errors(stage, unkeyed, errorList(err))
}

// recordStageError appends err to the stage's errors or warnings.
func recordStageError(s *trace.Session, stage parity.StageID, err error) {
	if err != nil {
		s.Errors(stage, unkeyed, []error{err})
	}
}

// recordStageErrors appends errs to the stage's errors and warnings.
func recordStageErrors(s *trace.Session, stage parity.StageID, errs []error) {
	s.Errors(stage, unkeyed, errs)
}

// recordValue records v as the stage json, as jsonutil marshals it, with no
// errors.
func recordValue(s *trace.Session, stage parity.StageID, v any) {
	if !s.Wants(stage) {
		return
	}
	s.Record(stage, unkeyed, v)
	s.Errors(stage, unkeyed, nil)
}

func recordHookOutcomes(s *trace.Session, executor hookexecution.HookStageExecutor) {
	if !s.Wants(stageHookOutcomes) || executor == nil {
		return
	}
	recordValue(s, stageHookOutcomes, newHookOutcomes(executor.GetOutcomes()))
}

func recordVideoPods[P any](s *trace.Session, podErrors []P, resp *openrtb_ext.BidResponseVideo, err error) {
	if !s.Wants(stageVideoPods) {
		return
	}
	if podErrors == nil {
		podErrors = []P{}
	}
	s.Record(stageVideoPods, unkeyed, videoPodsPayload[P]{PodErrors: podErrors, Response: resp})
	s.Errors(stageVideoPods, unkeyed, errorList(err))
}

func recordVideoCache(s *trace.Session, debugLog *exchange.DebugLog, err error) {
	if !s.Wants(stageVideoCache) || debugLog == nil {
		return
	}
	s.Record(stageVideoCache, unkeyed, videoCachePayload{
		CacheKey:  debugLog.CacheKey,
		CacheType: string(debugLog.CacheType),
		TTL:       debugLog.TTL,
		Data:      debugLog.CacheString,
	})
	s.Errors(stageVideoCache, unkeyed, errorList(err))
}

// recordRawOrNull records raw exactly, or JSON null when there are no bytes at
// all (a nil slice; an empty body is recorded as empty raw text).
func recordRawOrNull(s *trace.Session, stage parity.StageID, raw []byte) {
	if raw == nil {
		raw = jsonNull
	}
	s.RecordRaw(stage, unkeyed, raw)
}

func errorList(err error) []error {
	if err == nil {
		return nil
	}
	return []error{err}
}
