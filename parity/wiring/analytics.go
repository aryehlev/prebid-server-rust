package wiring

import (
	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/analytics"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/hooks/hookexecution"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/privacy"
)

// WrapAnalytics returns runner unchanged unless parity is enabled. In parity
// mode every Log*Object call is first recorded to the in-flight trace session
// (as a language-neutral projection) and then delegated unchanged.
func WrapAnalytics(runner analytics.Runner, cfg *config.Configuration) analytics.Runner {
	if cfg == nil || !cfg.Parity.Enabled {
		return runner
	}
	return &analyticsRecorder{next: runner}
}

type analyticsRecorder struct {
	next analytics.Runner
}

// auctionRecord is the projection of AuctionObject, AmpObject and VideoObject.
// Request is the RequestWrapper's BidRequest exactly as the handler left it: the
// wrapper is never rebuilt for tracing (observer-effect rule).
type auctionRecord struct {
	Status             int                           `json:"status"`
	Errors             []parity.ErrorRecord          `json:"errors"`
	Request            *openrtb2.BidRequest          `json:"request"`
	Response           *openrtb2.BidResponse         `json:"response"`
	AccountID          *string                       `json:"account_id"`
	StartTimeSet       bool                          `json:"start_time_set"`
	HookOutcomes       []hookexecution.StageOutcome  `json:"hook_outcomes"`
	SeatNonBid         []openrtb_ext.SeatNonBid      `json:"seat_non_bid"`
	AmpTargetingValues map[string]string             `json:"amp_targeting_values,omitempty"`
	Origin             string                        `json:"origin,omitempty"`
	VideoRequest       *openrtb_ext.BidRequestVideo  `json:"video_request,omitempty"`
	VideoResponse      *openrtb_ext.BidResponseVideo `json:"video_response,omitempty"`
}

type statusRecord struct {
	Status       int                           `json:"status"`
	Errors       []parity.ErrorRecord          `json:"errors"`
	Bidder       string                        `json:"bidder,omitempty"`
	UID          string                        `json:"uid,omitempty"`
	Success      *bool                         `json:"success,omitempty"`
	BidderStatus []*analytics.CookieSyncBidder `json:"bidder_status,omitempty"`
}

type notificationRecord struct {
	Request   *analytics.EventRequest `json:"request"`
	AccountID *string                 `json:"account_id"`
}

func (r *analyticsRecorder) LogAuctionObject(ao *analytics.AuctionObject, ac privacy.ActivityControl) {
	if s := trace.Current(); s != nil && ao != nil {
		s.AddAnalytics(parity.AnalyticsAuction, auctionRecord{
			Status: ao.Status, Errors: allErrorRecords(ao.Errors), Request: bidRequestOf(ao.RequestWrapper),
			Response: ao.Response, AccountID: accountIDOf(ao.Account), StartTimeSet: !ao.StartTime.IsZero(),
			HookOutcomes: ao.HookExecutionOutcome, SeatNonBid: ao.SeatNonBid,
		})
	}
	r.next.LogAuctionObject(ao, ac)
}

func (r *analyticsRecorder) LogAmpObject(ao *analytics.AmpObject, ac privacy.ActivityControl) {
	if s := trace.Current(); s != nil && ao != nil {
		s.AddAnalytics(parity.AnalyticsAmp, auctionRecord{
			Status: ao.Status, Errors: allErrorRecords(ao.Errors), Request: bidRequestOf(ao.RequestWrapper),
			Response: ao.AuctionResponse, StartTimeSet: !ao.StartTime.IsZero(), HookOutcomes: ao.HookExecutionOutcome,
			SeatNonBid: ao.SeatNonBid, AmpTargetingValues: ao.AmpTargetingValues, Origin: ao.Origin,
		})
	}
	r.next.LogAmpObject(ao, ac)
}

func (r *analyticsRecorder) LogVideoObject(vo *analytics.VideoObject, ac privacy.ActivityControl) {
	if s := trace.Current(); s != nil && vo != nil {
		s.AddAnalytics(parity.AnalyticsVideo, auctionRecord{
			Status: vo.Status, Errors: allErrorRecords(vo.Errors), Request: bidRequestOf(vo.RequestWrapper),
			Response: vo.Response, StartTimeSet: !vo.StartTime.IsZero(), SeatNonBid: vo.SeatNonBid,
			VideoRequest: vo.VideoRequest, VideoResponse: vo.VideoResponse,
		})
	}
	r.next.LogVideoObject(vo, ac)
}

func (r *analyticsRecorder) LogCookieSyncObject(cso *analytics.CookieSyncObject) {
	if s := trace.Current(); s != nil && cso != nil {
		s.AddAnalytics(parity.AnalyticsCookieSync, statusRecord{
			Status: cso.Status, Errors: allErrorRecords(cso.Errors), BidderStatus: cso.BidderStatus,
		})
	}
	r.next.LogCookieSyncObject(cso)
}

func (r *analyticsRecorder) LogSetUIDObject(so *analytics.SetUIDObject) {
	if s := trace.Current(); s != nil && so != nil {
		success := so.Success
		s.AddAnalytics(parity.AnalyticsSetUID, statusRecord{
			Status: so.Status, Errors: allErrorRecords(so.Errors), Bidder: so.Bidder, UID: so.UID, Success: &success,
		})
	}
	r.next.LogSetUIDObject(so)
}

func (r *analyticsRecorder) LogNotificationEventObject(ne *analytics.NotificationEvent, ac privacy.ActivityControl) {
	if s := trace.Current(); s != nil && ne != nil {
		s.AddAnalytics(parity.AnalyticsNotification, notificationRecord{Request: ne.Request, AccountID: accountIDOf(ne.Account)})
	}
	r.next.LogNotificationEventObject(ne, ac)
}

func (r *analyticsRecorder) Shutdown() {
	r.next.Shutdown()
}

// allErrorRecords keeps every error in input order, warnings included.
func allErrorRecords(errs []error) []parity.ErrorRecord {
	out := make([]parity.ErrorRecord, 0, len(errs))
	for _, err := range errs {
		out = append(out, trace.ErrorRecordOf(err))
	}
	return out
}

func bidRequestOf(rw *openrtb_ext.RequestWrapper) *openrtb2.BidRequest {
	if rw == nil {
		return nil
	}
	return rw.BidRequest
}

func accountIDOf(account *config.Account) *string {
	if account == nil {
		return nil
	}
	id := account.ID
	return &id
}
