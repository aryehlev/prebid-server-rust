// Package endpointtrace records the endpoint stages of /openrtb2/auction,
// /openrtb2/amp and /openrtb2/video into the in-flight parity trace session.
//
// Every exported function is a one-line hook for endpoints/openrtb2. It loads
// trace.Current() and returns at once when no parity case is in flight or the
// stage is not captured, so the disabled cost is a call, one atomic load and a
// nil check. Its arguments are values the handler already holds, passed without
// interface conversions (AMPTargeting and VideoPods are generic for that), so
// the disabled path never allocates. Hooks only read: request snapshots go
// through parity/shadow, which rebuilds an independent deep copy and never the
// live RequestWrapper (observer-effect rule).
//
// # Stage payloads
//
// The stage entry json member holds the payload below; errors and warnings are
// the stage entry's own lists (ErrorRecord, split by errortypes.IsWarning), and
// a stage that recorded no error carries [] for both. The Rust server records
// the same payloads at the equivalent sites.
//
//	hooks.entrypoint    request body bytes as the entrypoint hook stage returned them
//	                    (raw is exact; json is set when they parse). AMP has no body: null.
//	                    errors/warnings: the reject error, if any.
//	stored.fetch        {stored_request_id, has_stored_request, requests{id: Body}, imps{id: Body}}
//	                    with the fetcher errors. Auction records it again when the
//	                    raw-auction hook changed the payload (writes: 2).
//	account.resolved    {id, account}: account is the resolved config.Account dumped
//	                    with mapstructure.Decode into a map (config keys, as the
//	                    /__parity/config dump), or null when resolution failed.
//	                    errors: account-ID and fetch errors.
//	hooks.raw_auction   request body bytes after the raw-auction hook stage (auction only).
//	request.merged      auction/video: the request bytes after the stored-request,
//	                    default-request and stored-imp merge; AMP: the stored request
//	                    after the AMP query overrides. errors: merge and parse errors.
//	request.normalized  the BidRequest after ConvertUpTo26, mergeBidderParams,
//	                    setFieldsImplicitly, SetDefaults, processInterstitials and
//	                    lmt.ModifyForIOS (the steps each endpoint runs), as rebuilding
//	                    the wrapper would produce it. errors: what the steps returned;
//	                    a failing step leaves json null.
//	stored.responses    {auction{imp: Body}, bid{imp: {bidder: Body}}, replace_impid{bidder: {imp: bool}}}
//	                    with the ProcessStoredResponses errors.
//	request.validated   the BidRequest after validateRequest (rebuilt as above); its
//	                    errors and warnings.
//	gdpr.context        {signal, enforced, channel, tcf2{...}}: see gdprPayload.
//	exchange.response   errors only: the error HoldAuction returned ([] for none).
//	                    The exchange records the json.
//	seat_non_bids       errors only: the error of setSeatNonBidRaw (auction, video test).
//	hooks.auction_response  the BidResponse after the auction-response hook stage.
//	amp.targeting_response  the AMP response {targeting, ortb2{ext}} before the exitpoint stage.
//	hooks.exitpoint     the response object after the exitpoint stage; errors: the
//	                    response encoding error, if any.
//	hook_outcomes       every hook stage outcome in execution order, with the stage
//	                    name and hook errors and warnings that Go's own JSON omits.
//	video.pods          {pod_errors[{PodId, PodIndex, ErrMsgs}], response}: pod
//	                    errors as Go marshals PodError and the simplified video
//	                    response; errors: the pod or response-building error.
//	video.cache         {cache_key, cache_type, ttl, data}: the debug log put to
//	                    Prebid Cache; errors: the put error.
package endpointtrace
