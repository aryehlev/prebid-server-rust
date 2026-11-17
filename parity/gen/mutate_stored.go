package gen

import (
	"net/http"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// missingStoredID is a stored ID no case ever stores.
const missingStoredID = "parity-gen-missing"

func storedMutators() []*mutator {
	return []*mutator{
		feat("stored.request", Preserves, onAuction, "moves top-level request fields into a stored request (ext.prebid.storedrequest)", hasORTB, storeRequestPart),
		feat("stored.imp", Preserves, onAuction, "moves an imp's media into a stored imp (imp.ext.prebid.storedrequest)", hasImps, storeImpPart),
		feat("stored.missing_request", Invalidates, onAuction, "references a stored request that does not exist", hasORTB, func(d *draft, _ *RNG) error {
			prebid(ortbReq(d))["storedrequest"] = obj{"id": missingStoredID}
			return nil
		}).rare(),
		feat("stored.auction_response", Preserves, onORTB, "answers every imp from a stored auction response", hasImps, func(d *draft, r *RNG) error {
			return storedAuctionResponses(d, r, impsOf(ortbReq(d)))
		}),
		feat("stored.partial_auction_response", Invalidates, onAuction, "gives only some imps a stored auction response", func(d *draft) bool {
			return hasORTB(d) && len(impsOf(ortbReq(d))) > 1
		}, func(d *draft, r *RNG) error { return storedAuctionResponses(d, r, impsOf(ortbReq(d))[:1]) }).rare(),
		feat("stored.bid_response", Uncertain, onORTB, "answers an imp's bidders from stored bid responses", hasBidderImp, storedBidResponses),
	}
}

// movableRequestKeys are top-level fields stored.request may move.
var movableRequestKeys = []string{"site", "app", "dooh", "device", "user", "regs", "source", "tmax", "cur", "test"}

func storeRequestPart(d *draft, r *RNG) error {
	req := ortbReq(d)
	var keys []string
	for _, k := range movableRequestKeys {
		if _, ok := req[k]; ok {
			keys = append(keys, k)
		}
	}
	moved := obj{}
	for _, k := range Sample(r, keys, r.Range(1, len(keys))) {
		moved[k] = req[k]
		delete(req, k)
	}
	if r.Chance(0.3) {
		moved["ext"] = obj{"prebid": obj{"targeting": obj{"pricegranularity": "med", "includewinners": true}}}
	}
	id, err := d.storeAs("req", moved)
	if err != nil {
		return err
	}
	prebid(req)["storedrequest"] = obj{"id": id}
	return nil
}

func storeImpPart(d *draft, r *RNG) error {
	imp := Pick(r, impsOf(ortbReq(d)))
	moved := obj{}
	for _, k := range append([]string{"tagid", "pmp"}, allMedia...) {
		if v, ok := imp[k]; ok && (r.Chance(0.8) || isMedia(k)) {
			moved[k] = v
			delete(imp, k)
		}
	}
	id, err := d.storeAs("imp", moved)
	if err != nil {
		return err
	}
	path(imp, "ext.prebid")["storedrequest"] = obj{"id": id}
	return nil
}

func isMedia(key string) bool {
	for _, m := range allMedia {
		if m == key {
			return true
		}
	}
	return false
}

// storedAuctionResponses gives each imp a stored seatbid list.
func storedAuctionResponses(d *draft, r *RNG, imps []obj) error {
	for _, imp := range imps {
		impID, _ := imp["id"].(string)
		seat := Pick(r, bidderNames(imp))
		if seat == "" {
			seat = Pick(r, d.g.catalog.deep).name
		}
		bid := obj{"id": "parity-gen-bid-" + r.Hex(6), "impid": impID, "price": fnum(Pick(r, []float64{0.5, 1.25, 3})), "adm": "<div>parity</div>", "crid": "parity-crid", "w": num(300), "h": num(250)}
		id, err := d.storeAs("resp", []any{obj{"bid": []any{bid}, "seat": seat}})
		if err != nil {
			return err
		}
		path(imp, "ext.prebid")["storedauctionresponse"] = obj{"id": id}
	}
	return nil
}

func storedBidResponses(d *draft, r *RNG) error {
	imp := Pick(r, impsWithBidders(ortbReq(d)))
	impID, _ := imp["id"].(string)
	var refs []any
	for _, b := range bidderNames(imp) {
		bid := obj{"id": "parity-gen-sbid-" + r.Hex(6), "impid": impID, "price": fnum(1.1), "adm": "<div>stored</div>", "crid": "parity-crid", "w": num(300), "h": num(250)}
		resp := obj{"id": "parity-gen-sresp", "cur": "USD", "seatbid": []any{obj{"bid": []any{bid}, "seat": b}}}
		id, err := d.storeAs("resp", resp)
		if err != nil {
			return err
		}
		refs = append(refs, obj{"bidder": b, "id": id})
	}
	path(imp, "ext.prebid")["storedbidresponse"] = refs
	return nil
}

func accountMutators() []*mutator {
	return []*mutator{
		feat("account.events", Preserves, onORTB, "uses a stored account with events enabled", hasChannel, withAccount(func(*RNG) obj { return obj{"events": obj{"enabled": true}} })),
		feat("account.floors", Preserves, onORTB, "uses a stored account with price floors enabled", hasChannel, func(d *draft, r *RNG) error {
			d.profile.FloorsEnabled = true
			return withAccount(func(r *RNG) obj {
				return obj{"price_floors": obj{"enabled": true, "enforce_floors_rate": num(100), "enforce_deal_floors": r.Chance(0.5)}}
			})(d, r)
		}),
		feat("account.privacy", Preserves, onORTB, "uses a stored account with activity controls", hasChannel, accountActivities),
		feat("account.validations", Preserves, onORTB, "uses a stored account with bid validations", hasChannel, withAccount(func(r *RNG) obj {
			mode := func() string { return Pick(r, []string{"skip", "warn", "enforce"}) }
			return obj{"validations": obj{"banner_creative_max_size": mode(), "secure_markup": mode(), "max_creative_width": num(300), "max_creative_height": num(250)}}
		})),
		feat("account.disabled", Invalidates, onORTB, "uses a disabled stored account", hasChannel, withAccount(func(*RNG) obj { return obj{"disabled": true} })).rare(),
		feat("account.malformed", Invalidates, onORTB, "uses a stored account of the wrong shape", hasChannel, withAccount(func(*RNG) obj { return obj{"disabled": "invalid type"} })).rare(),
		feat("account.required", Invalidates, onORTB, "requires an account and sends none", hasORTB, func(d *draft, _ *RNG) error {
			d.profile.AccountRequired = true
			req := ortbReq(d)
			for _, ch := range []string{channelSite, channelApp, channelDOOH} {
				if pub, ok := lookupObj(req, ch+".publisher"); ok {
					delete(pub, "id")
				}
			}
			d.dropQuery("account")
			return nil
		}).rare(),
		feat("account.bid_limit", Preserves, onAll, "sets a default multibid limit for the account", nil, func(d *draft, r *RNG) error {
			d.profile.AccountDefaultBidLimit = r.Range(2, 5)
			return nil
		}),
		feat("account.blocked_app", Invalidates, onORTB, "blocks the request's app id", func(d *draft) bool {
			return hasORTB(d) && lookupString(ortbReq(d), "app.id") != ""
		}, func(d *draft, _ *RNG) error {
			d.profile.BlockedApps = []string{lookupString(ortbReq(d), "app.id")}
			return nil
		}).rare(),
	}
}

// withAccount stores the account content and points the request at it.
func withAccount(content func(r *RNG) obj) func(*draft, *RNG) error {
	return func(d *draft, r *RNG) error {
		id, err := d.storeAs("acct", content(r))
		if err != nil {
			return err
		}
		req := ortbReq(d)
		ch := channelOf(req)
		path(req, ch+".publisher")["id"] = id
		if d.endpoint == corpus.EndpointAMP && r.Chance(0.3) {
			d.setQuery("account", id)
		}
		return nil
	}
}

// activityNames are the account activity controls.
var activityNames = []string{"syncUser", "fetchBids", "enrichUfpd", "reportAnalytics", "transmitUfpd", "transmitPreciseGeo", "transmitUniqueRequestIds", "transmitTid"}

func accountActivities(d *draft, r *RNG) error {
	bidders := requestBidders(ortbReq(d))
	return withAccount(func(r *RNG) obj {
		acts := obj{}
		for _, name := range Sample(r, activityNames, r.Range(1, 3)) {
			act := obj{"default": r.Chance(0.5)}
			if len(bidders) > 0 && r.Chance(0.6) {
				cond := obj{"componentName": strList(Pick(r, bidders)), "componentType": strList("bidder")}
				act["rules"] = []any{obj{"condition": cond, "allow": r.Chance(0.5)}}
			}
			acts[name] = act
		}
		return obj{"privacy": obj{"allowactivities": acts}}
	})(d, r)
}

func mockMutators() []*mutator {
	return []*mutator{
		feat("mock.no_bid", Preserves, onAll, "answers one bidder with 204 No Content", hasCaseBidders, mockResponse(parity.InlineResponse{Status: http.StatusNoContent})),
		feat("mock.error", Preserves, onAll, "answers one bidder with HTTP 500", hasCaseBidders, mockResponse(parity.InlineResponse{Status: http.StatusInternalServerError, Body: parity.TextBody("parity mock error")})),
		feat("mock.bad_body", Preserves, onAll, "answers one bidder with a 200 that is not JSON", hasCaseBidders, mockResponse(parity.InlineResponse{Status: http.StatusOK, Headers: parity.HeaderList{{"Content-Type", "application/json"}}, Body: parity.TextBody("{not json")})),
		feat("mock.timeout", Preserves, onAll, "delays one bidder's answer past tmax", hasCaseBidders, func(d *draft, r *RNG) error {
			if req := d.req(); req != nil {
				req["tmax"] = num(300)
			}
			return mockResponse(parity.InlineResponse{Status: http.StatusNoContent, DelayMS: 1200})(d, r)
		}).rare(),
		feat("mock.price", Preserves, onAll, "sets an edge price on one bidder's mock bid", hasCaseBidders, func(d *draft, r *RNG) error {
			price := Pick(r, []float64{0, 0.001, 0.01, 1.2345, 19.99, 999.99, 10000})
			d.mockBidder(Pick(r, d.bidders()), func(m *corpus.MockBidder) { m.Price = &price })
			return nil
		}),
		feat("mock.deal", Preserves, onAll, "gives one bidder's mock bid a deal id", hasCaseBidders, func(d *draft, r *RNG) error {
			deal := "parity-deal-" + r.Hex(4)
			d.mockBidder(Pick(r, d.bidders()), func(m *corpus.MockBidder) { m.DealID = deal })
			return nil
		}),
		feat("mock.seat", Preserves, onAll, "makes one bidder answer under another seat", hasCaseBidders, func(d *draft, r *RNG) error {
			d.mockBidder(Pick(r, d.bidders()), func(m *corpus.MockBidder) { m.Seat = "paritygenseat" })
			return nil
		}),
	}
}

// mockResponse answers one of the case's bidders with resp.
func mockResponse(resp parity.InlineResponse) func(*draft, *RNG) error {
	return func(d *draft, r *RNG) error {
		bidder := Pick(r, d.bidders())
		id := "gen-" + bidder + "-" + strconv.Itoa(len(d.mocks.HTTP))
		d.mocks.HTTP = append(d.mocks.HTTP, corpus.HTTPMock{ID: id, Bidder: bidder, Response: resp})
		return nil
	}
}

// Seed domains the graft steps draw from.
var cacheSeedDomains = []string{corpus.SeedCache, corpus.SeedCustomCacheKey, corpus.SeedEventsCache, corpus.SeedImpCustomCacheKey}

func graftMutators() []*mutator {
	return []*mutator{
		feat("seed.fpd", Uncertain, onORTB, "grafts first-party data from an FPD seed request", func(d *draft) bool {
			return hasORTB(d) && len(d.g.seeds[corpus.SeedFPD]) > 0
		}, graftFPD),
		feat("seed.cache", Uncertain, onORTB, "grafts cache, targeting and imp.exp settings from a cache seed request", func(d *draft) bool {
			return hasImps(d) && len(d.cacheSeeds()) > 0
		}, graftCache),
		feat("fpd.bidderconfig", Preserves, onORTB, "sets ext.prebid.data.bidders and a matching ext.prebid.bidderconfig", func(d *draft) bool {
			req := ortbReq(d)
			return hasBidderImp(d) && (req[channelSite] != nil || req[channelApp] != nil)
		}, bidderConfig),
		feat("fpd.data", Preserves, onORTB, "adds site/app ext.data, user.data segments and content", hasChannel, fpdData),
	}
}

func (d *draft) cacheSeeds() []corpus.Seed {
	var out []corpus.Seed
	for _, dom := range cacheSeedDomains {
		out = append(out, d.g.seeds[dom]...)
	}
	return out
}

// graft copies src's value at dotted into dst, reporting whether it did.
func graft(dst, src obj, dotted string) bool {
	v, ok := lookup(src, dotted)
	if !ok {
		return false
	}
	parent, key := dst, dotted
	if i := strings.LastIndexByte(dotted, '.'); i >= 0 {
		parent, key = path(dst, dotted[:i]), dotted[i+1:]
	}
	parent[key] = deepCopy(v)
	return true
}

func graftFPD(d *draft, r *RNG) error {
	seed := Pick(r, d.g.seeds[corpus.SeedFPD])
	src, err := decodeObject(seed.Request)
	if err != nil {
		d.note("seed.fpd: %s is not an object", seed.Path)
		return nil
	}
	req := ortbReq(d)
	ch := channelOf(req)
	paths := []string{"ext.prebid.data", "ext.prebid.bidderconfig", "user.data", "user.ext.data", "user.keywords"}
	if _, ok := src[ch]; ok {
		paths = append(paths, ch+".ext.data", ch+".keywords", ch+".content")
	}
	n := 0
	for _, p := range paths {
		if graft(req, src, p) {
			n++
		}
	}
	d.note("seed.fpd: grafted %d fields from %s", n, seed.Path)
	return nil
}

func graftCache(d *draft, r *RNG) error {
	seed := Pick(r, d.cacheSeeds())
	src, err := decodeObject(seed.Request)
	if err != nil {
		d.note("seed.cache: %s is not an object", seed.Path)
		return nil
	}
	req := ortbReq(d)
	n := 0
	for _, p := range []string{"ext.prebid.cache", "ext.prebid.targeting", "ext.prebid.events"} {
		if graft(req, src, p) {
			n++
		}
	}
	if srcImps := impsOf(src); len(srcImps) > 0 {
		if exp, ok := srcImps[0]["exp"]; ok {
			Pick(r, impsOf(req))["exp"] = deepCopy(exp)
			n++
		}
	}
	d.note("seed.cache: grafted %d fields from %s", n, seed.Path)
	return nil
}

func bidderConfig(d *draft, r *RNG) error {
	req := ortbReq(d)
	bidder := Pick(r, requestBidders(req))
	ch := channelSite
	if _, ok := req[channelApp]; ok {
		ch = channelApp
	}
	ortb2 := obj{ch: obj{"keywords": "parity,fpd"}}
	if _, ok := req["user"].(obj); ok {
		ortb2["user"] = obj{"keywords": "parity-user"}
	}
	path(req, "ext.prebid.data")["bidders"] = strList(bidder)
	prebid(req)["bidderconfig"] = []any{obj{"bidders": strList(bidder), "config": obj{"ortb2": ortb2}}}
	return nil
}

func fpdData(d *draft, r *RNG) error {
	req := ortbReq(d)
	ch := channelOf(req)
	path(req, ch+".ext.data")["category"] = Pick(r, []string{"sports", "news", "parity"})
	segment := obj{"id": "seg-" + r.Hex(4), "name": "parity-segment"}
	path(req, "user")["data"] = []any{obj{"id": "pd-" + r.Hex(4), "name": "parity.test", "segment": []any{segment}}}
	if ch != channelDOOH && r.Chance(0.5) {
		path(req, ch)["content"] = obj{"title": "Parity Episode", "genre": "test", "language": "en"}
	}
	return nil
}
