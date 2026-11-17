package gen

import (
	"strconv"
	"strings"
	"sync"

	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// String names the phase.
func (p Phase) String() string {
	switch p {
	case PhaseFeature:
		return "feature"
	case PhaseStructure:
		return "structure"
	case PhaseRaw:
		return "raw"
	case PhaseHTTP:
		return "http"
	case PhaseEncoding:
		return "encoding"
	}
	return "base"
}

// endpointSet is the set of endpoints a step applies to.
type endpointSet uint8

const (
	onAuction endpointSet = 1 << iota
	onAMP
	onVideo

	// onORTB are the endpoints whose request is an OpenRTB BidRequest (the
	// auction body, the AMP stored request).
	onORTB = onAuction | onAMP
	// onBody are the endpoints that take a request body.
	onBody = onAuction | onVideo
	onAll  = onAuction | onAMP | onVideo
)

var endpointBits = []struct {
	bit      endpointSet
	endpoint string
}{
	{onAuction, corpus.EndpointAuction},
	{onAMP, corpus.EndpointAMP},
	{onVideo, corpus.EndpointVideo},
}

func (s endpointSet) has(endpoint string) bool {
	for _, e := range endpointBits {
		if e.endpoint == endpoint {
			return s&e.bit != 0
		}
	}
	return false
}

func (s endpointSet) list() []string {
	var out []string
	for _, e := range endpointBits {
		if s&e.bit != 0 {
			out = append(out, e.endpoint)
		}
	}
	return out
}

// Step weights: how often drawMutator picks a step relative to the others.
const (
	weightCommon = 10
	weightRare   = 3
)

// maxImps caps imp.add.
const maxImps = 8

// mutator is one registered step.
type mutator struct {
	name      string
	doc       string
	phase     Phase
	validity  Validity
	weight    int
	endpoints endpointSet
	// when is the precondition; nil means always.
	when func(d *draft) bool
	run  func(d *draft, r *RNG) error
}

// appliesTo reports whether the step can run on d now.
func (m *mutator) appliesTo(d *draft) bool {
	return m.endpoints.has(d.endpoint) && (m.when == nil || m.when(d))
}

func (m *mutator) apply(d *draft, r *RNG) error { return m.run(d, r) }

// rare lowers the step's draw weight.
func (m *mutator) rare() *mutator {
	m.weight = weightRare
	return m
}

// MutatorInfo describes one registered step.
type MutatorInfo struct {
	Name      string   `json:"name"`
	Doc       string   `json:"doc"`
	Phase     string   `json:"phase"`
	Validity  string   `json:"validity"`
	Endpoints []string `json:"endpoints"`
	Weight    int      `json:"weight"`
}

func (m *mutator) info() MutatorInfo {
	return MutatorInfo{Name: m.name, Doc: m.doc, Phase: m.phase.String(), Validity: m.validity.String(), Endpoints: m.endpoints.list(), Weight: m.weight}
}

// newStep builds a mutator with the common weight.
func newStep(phase Phase, name string, v Validity, eps endpointSet, doc string, when func(*draft) bool, run func(*draft, *RNG) error) *mutator {
	return &mutator{name: name, doc: doc, phase: phase, validity: v, weight: weightCommon, endpoints: eps, when: when, run: run}
}

// feat builds a request-feature step.
func feat(name string, v Validity, eps endpointSet, doc string, when func(*draft) bool, run func(*draft, *RNG) error) *mutator {
	return newStep(PhaseFeature, name, v, eps, doc, when, run)
}

// registry lists every step in a fixed order. Step functions never call
// registry or lookupMutator, which keeps package initialisation acyclic.
var registry = sync.OnceValue(func() []*mutator {
	groups := [][]*mutator{
		impMutators(), bidderMutators(), requestExtMutators(), storedMutators(),
		accountMutators(), mockMutators(), graftMutators(), videoMutators(),
		ampMutators(), privacyMutators(), structureMutators(), rawMutators(),
		httpMutators(),
	}
	var all []*mutator
	for _, g := range groups {
		all = append(all, g...)
	}
	return all
})

var mutatorIndex = sync.OnceValue(func() map[string]*mutator {
	out := map[string]*mutator{}
	for _, m := range registry() {
		out[m.name] = m
	}
	return out
})

func lookupMutator(name string) (*mutator, bool) {
	m, ok := mutatorIndex()[name]
	return m, ok
}

// ortbReq returns the OpenRTB request feature steps edit, or nil (video bodies
// are not OpenRTB requests).
func ortbReq(d *draft) obj {
	if d.isVideoEndpoint() {
		return nil
	}
	return d.req()
}

func hasORTB(d *draft) bool { return ortbReq(d) != nil }

func hasReq(d *draft) bool { return d.req() != nil }

func hasImps(d *draft) bool {
	req := ortbReq(d)
	return req != nil && len(impsOf(req)) > 0
}

// hasChannel reports whether the request carries site, app or dooh.
func hasChannel(d *draft) bool {
	req := ortbReq(d)
	if req == nil {
		return false
	}
	_, s := req[channelSite].(obj)
	_, a := req[channelApp].(obj)
	_, o := req[channelDOOH].(obj)
	return s || a || o
}

// impsWith returns the imps that carry an object under key.
func impsWith(req obj, key string) []obj {
	var out []obj
	for _, imp := range impsOf(req) {
		if _, ok := imp[key].(obj); ok {
			out = append(out, imp)
		}
	}
	return out
}

func hasImpWith(key string) func(*draft) bool {
	return func(d *draft) bool {
		req := ortbReq(d)
		return req != nil && len(impsWith(req, key)) > 0
	}
}

// impsWithBidders returns the imps that name at least one bidder.
func impsWithBidders(req obj) []obj {
	var out []obj
	for _, imp := range impsOf(req) {
		if len(bidderNames(imp)) > 0 {
			out = append(out, imp)
		}
	}
	return out
}

func hasBidderImp(d *draft) bool {
	req := ortbReq(d)
	return req != nil && len(impsWithBidders(req)) > 0
}

// bidderNames lists imp.ext.prebid.bidder keys, sorted.
func bidderNames(imp obj) []string {
	b, _ := lookupObj(imp, "ext.prebid.bidder")
	return sortedKeys(b)
}

// prebid returns request.ext.prebid, creating it.
func prebid(req obj) obj { return path(req, "ext.prebid") }

// spec returns the catalog entry of a bidder named in the request, if any.
func (d *draft) spec(name string) *bidderSpec { return d.g.catalog.byName[name] }

// bidders lists the bidders the case's auction will call: imp bidders of the
// OpenRTB request, or the bidders of the video pods' stored imps.
func (d *draft) bidders() []string {
	if req := ortbReq(d); req != nil {
		return requestBidders(req)
	}
	seen := map[string]bool{}
	for _, raw := range d.stored.Imps {
		imp, err := decodeObject(raw)
		if err != nil {
			continue
		}
		for _, b := range bidderNames(imp) {
			seen[b] = true
		}
	}
	return sortedKeys(seen)
}

func hasCaseBidders(d *draft) bool { return len(d.bidders()) > 0 }

// worsen lowers the case's validity class to at least v.
func (d *draft) worsen(v Validity) {
	if v > d.validity {
		d.validity = v
	}
}

func impMutators() []*mutator {
	return []*mutator{
		feat("imp.add", Preserves, onAuction, "appends an imp for the request's bidders (multi-imp)", func(d *draft) bool { return hasORTB(d) && len(impsOf(ortbReq(d))) < maxImps }, addImp),
		feat("imp.multiformat", Preserves, onORTB, "adds a second media type every bidder of the imp supports", hasBidderImp, addMediaType),
		feat("imp.bidfloor", Preserves, onORTB, "sets imp.bidfloor and imp.bidfloorcur", hasImps, setBidFloor),
		feat("imp.pmp", Preserves, onORTB, "adds a private marketplace deal", hasImps, addPMP),
		feat("imp.instl", Preserves, onORTB, "marks an imp interstitial", hasImps, onImp(func(_ *RNG, imp obj) { imp["instl"] = num(1) })),
		feat("imp.exp", Preserves, onORTB, "sets imp.exp (cache TTL)", hasImps, onImp(func(r *RNG, imp obj) { imp["exp"] = num(int64(Pick(r, []int{30, 300, 3600}))) })),
		feat("imp.rewarded", Preserves, onORTB, "marks an imp rewarded in the 2.6 (rwdd) or 2.5 (ext.prebid.is_rewarded_inventory) location", hasImps, setRewarded),
		feat("imp.fledge", Preserves, onORTB, "sets imp.ext.ae", hasImps, onImp(func(_ *RNG, imp obj) { child(imp, "ext")["ae"] = num(1) })),
		feat("imp.echo_video_attrs", Preserves, onORTB, "sets imp.ext.prebid.options.echovideoattrs", hasImpWith(mediaVideo), echoVideoAttrs),
		feat("imp.metric", Invalidates, onORTB, "adds imp.metric, which Go rejects", hasImps, onImp(func(_ *RNG, imp obj) {
			imp["metric"] = []any{obj{"type": "viewability", "value": fnum(0.85), "vendor": "parity"}}
		})).rare(),
		feat("imp.none", Invalidates, onORTB, "empties request.imp", hasImps, func(d *draft, _ *RNG) error { ortbReq(d)["imp"] = []any{}; return nil }).rare(),
		feat("imp.drop_ext", Invalidates, onORTB, "removes an imp's ext", hasImps, onImp(func(_ *RNG, imp obj) { delete(imp, "ext") })).rare(),
		feat("imp.no_bidders", Invalidates, onORTB, "empties imp.ext.prebid.bidder", hasBidderImp, onBidderImp(func(_ *RNG, imp obj) { path(imp, "ext.prebid")["bidder"] = obj{} })).rare(),
		feat("imp.no_media", Invalidates, onORTB, "removes every media object of an imp", hasImps, onImp(func(_ *RNG, imp obj) {
			for _, m := range allMedia {
				delete(imp, m)
			}
		})).rare(),
		feat("imp.banner_nosize", Invalidates, onORTB, "removes banner.format, w and h", hasImpWith(mediaBanner), onMedia(mediaBanner, func(_ *RNG, imp, b obj) {
			delete(b, "format")
			delete(b, "w")
			delete(b, "h")
			delete(imp, "instl")
		})).rare(),
		feat("imp.banner_wmax", Invalidates, onORTB, "sets the unsupported banner.wmax", hasImpWith(mediaBanner), onMedia(mediaBanner, func(_ *RNG, _, b obj) { b["wmax"] = num(728) })).rare(),
		feat("imp.video_nomimes", Invalidates, onORTB, "removes video.mimes", hasImpWith(mediaVideo), onMedia(mediaVideo, func(_ *RNG, _, v obj) { delete(v, "mimes") })).rare(),
		feat("imp.native_bad", Invalidates, onORTB, "makes native.request invalid JSON", hasImpWith(mediaNative), onMedia(mediaNative, func(_ *RNG, _, n obj) { n["request"] = "{not json" })).rare(),
		feat("ids.dup_imp", Invalidates, onAuction, "gives two imps the same id", hasImps, dupImpID).rare(),
		feat("ids.empty_imp", Invalidates, onORTB, "empties an imp id", hasImps, onImp(func(_ *RNG, imp obj) { imp["id"] = "" })).rare(),
		feat("ids.empty_request", Invalidates, onAuction, "empties request.id", hasORTB, func(d *draft, _ *RNG) error { ortbReq(d)["id"] = ""; return nil }).rare(),
		feat("ids.request_id", Preserves, onAuction, "uses an unusual but valid request.id", hasORTB, func(d *draft, r *RNG) error {
			ortbReq(d)["id"] = Pick(r, []string{r.UUID(), strings.Repeat("r", 256), "réq-ünïcode-✓", "id with spaces", "0"})
			return nil
		}),
		feat("channel.both", Invalidates, onAuction, "adds a second distribution channel", hasChannel, addSecondChannel).rare(),
		feat("channel.none", Invalidates, onAuction, "removes site, app and dooh", hasChannel, func(d *draft, _ *RNG) error {
			req := ortbReq(d)
			for _, ch := range []string{channelSite, channelApp, channelDOOH} {
				delete(req, ch)
			}
			return nil
		}).rare(),
		feat("channel.site_noid", Invalidates, onAuction, "removes site.id and site.page (AMP refills site.page)", func(d *draft) bool {
			_, ok := lookupObj(ortbReq(d), channelSite)
			return ok
		}, func(d *draft, _ *RNG) error {
			site, _ := lookupObj(ortbReq(d), channelSite)
			delete(site, "id")
			delete(site, "page")
			return nil
		}).rare(),
		feat("device.bad_size", Invalidates, onORTB, "sets a negative device.w", hasORTB, func(d *draft, _ *RNG) error { path(ortbReq(d), "device")["w"] = num(-1); return nil }).rare(),
		feat("device.geo_accuracy", Invalidates, onORTB, "sets a negative device.geo.accuracy", hasORTB, func(d *draft, _ *RNG) error {
			path(ortbReq(d), "device.geo")["accuracy"] = num(-5)
			return nil
		}).rare(),
	}
}

// onImp runs edit on a random imp.
func onImp(edit func(r *RNG, imp obj)) func(*draft, *RNG) error {
	return func(d *draft, r *RNG) error {
		edit(r, Pick(r, impsOf(ortbReq(d))))
		return nil
	}
}

// onBidderImp runs edit on a random imp that names bidders.
func onBidderImp(edit func(r *RNG, imp obj)) func(*draft, *RNG) error {
	return func(d *draft, r *RNG) error {
		edit(r, Pick(r, impsWithBidders(ortbReq(d))))
		return nil
	}
}

// onMedia runs edit on the media object of a random imp that has it.
func onMedia(media string, edit func(r *RNG, imp, m obj)) func(*draft, *RNG) error {
	return func(d *draft, r *RNG) error {
		imp := Pick(r, impsWith(ortbReq(d), media))
		if m, ok := imp[media].(obj); ok {
			edit(r, imp, m)
		}
		return nil
	}
}

func addImp(d *draft, r *RNG) error {
	req := ortbReq(d)
	channel := channelOf(req)
	var specs []*bidderSpec
	for _, name := range requestBidders(req) {
		if b := d.spec(name); b != nil && len(b.media[channel]) > 0 {
			specs = append(specs, b)
		}
	}
	if len(specs) == 0 {
		specs = d.g.catalog.pickBidders(r, channel, 1)
	}
	if len(specs) == 0 {
		d.note("imp.add: no bidder supports %s", channel)
		return nil
	}
	used := map[string]bool{}
	for _, imp := range impsOf(req) {
		if id, ok := imp["id"].(string); ok {
			used[id] = true
		}
	}
	list, _ := req["imp"].([]any)
	imp, err := d.g.impFor(r, len(list), channel, specs, nil, used)
	if err != nil {
		return err
	}
	req["imp"] = append(list, imp)
	return nil
}

func addMediaType(d *draft, r *RNG) error {
	req := ortbReq(d)
	imp := Pick(r, impsWithBidders(req))
	channel := channelOf(req)
	var options []string
	for _, m := range allMedia {
		if _, ok := imp[m]; !ok && d.allSupport(bidderNames(imp), channel, m) {
			options = append(options, m)
		}
	}
	if len(options) == 0 {
		d.note("imp.multiformat: no further media type is supported by every bidder")
		return nil
	}
	m := Pick(r, options)
	media, err := d.g.mediaTree(r, m)
	if err != nil {
		return err
	}
	imp[m] = media
	return nil
}

// allSupport reports whether every named catalog bidder supports media.
func (d *draft) allSupport(names []string, channel, media string) bool {
	for _, name := range names {
		if b := d.spec(name); b == nil || !b.supports(channel, media) {
			return false
		}
	}
	return true
}

// mediaTree builds one media object as a tree.
func (g *Generator) mediaTree(r *RNG, media string) (any, error) {
	o, err := toObject(g.imp(r, "parity", []string{media}))
	if err != nil {
		return nil, err
	}
	return o[media], nil
}

func setBidFloor(d *draft, r *RNG) error {
	imp := Pick(r, impsOf(ortbReq(d)))
	imp["bidfloor"] = fnum(Pick(r, []float64{0.01, 0.1, 0.5, 1, 2.5}))
	imp["bidfloorcur"] = Pick(r, []string{"USD", "USD", "EUR"})
	return nil
}

func addPMP(d *draft, r *RNG) error {
	imp := Pick(r, impsOf(ortbReq(d)))
	deal := obj{"id": "deal-" + r.Hex(6), "bidfloor": fnum(1.5), "bidfloorcur": "USD", "at": num(1)}
	imp["pmp"] = obj{"private_auction": num(int64(r.Range(0, 1))), "deals": []any{deal}}
	return nil
}

func setRewarded(d *draft, r *RNG) error {
	imp := Pick(r, impsOf(ortbReq(d)))
	if r.Chance(0.5) {
		imp["rwdd"] = num(1)
	} else {
		path(imp, "ext.prebid")["is_rewarded_inventory"] = num(1)
	}
	return nil
}

func echoVideoAttrs(d *draft, r *RNG) error {
	imp := Pick(r, impsWith(ortbReq(d), mediaVideo))
	path(imp, "ext.prebid.options")["echovideoattrs"] = true
	return nil
}

func dupImpID(d *draft, r *RNG) error {
	req := ortbReq(d)
	imps := impsOf(req)
	if len(imps) == 1 {
		list, _ := req["imp"].([]any)
		req["imp"] = append(list, deepCopy(imps[0]))
		return nil
	}
	imps[1]["id"] = imps[0]["id"]
	return nil
}

func addSecondChannel(d *draft, r *RNG) error {
	req := ortbReq(d)
	if _, ok := req[channelApp]; ok {
		req[channelSite] = obj{"page": Pick(r, d.g.pools.ORTB.Sites).Page}
		return nil
	}
	a := Pick(r, d.g.pools.ORTB.Apps)
	req[channelApp] = obj{"id": a.ID, "bundle": a.Bundle}
	return nil
}

func bidderMutators() []*mutator {
	return []*mutator{
		feat("bidders.add", Preserves, onORTB, "adds another bidder with valid params to an imp", hasImps, addBidder),
		feat("bidders.alias", Preserves, onORTB, "defines a request alias (ext.prebid.aliases, sometimes aliasgvlids) and bids through it", hasBidderImp, addAlias),
		feat("bidders.alias_noop", Invalidates, onORTB, "aliases a bidder to itself", hasBidderImp, func(d *draft, r *RNG) error {
			name := Pick(r, requestBidders(ortbReq(d)))
			path(ortbReq(d), "ext.prebid.aliases")[name] = name
			return nil
		}).rare(),
		feat("bidders.alias_unknown", Invalidates, onORTB, "aliases to an unknown core bidder", hasBidderImp, func(d *draft, r *RNG) error {
			imp := Pick(r, impsWithBidders(ortbReq(d)))
			path(ortbReq(d), "ext.prebid.aliases")["paritydangling"] = "paritynosuchbidder"
			impBidders(imp)["paritydangling"] = obj{}
			return nil
		}).rare(),
		feat("bidders.unknown", Invalidates, onORTB, "adds an unknown bidder to an imp", hasImps, onImp(func(_ *RNG, imp obj) { impBidders(imp)["paritynosuchbidder"] = obj{} })).rare(),
		feat("bidders.disabled", Uncertain, onORTB, "adds a bidder bidder-info disables", func(d *draft) bool { return hasImps(d) && len(d.g.catalog.disabled) > 0 }, func(d *draft, r *RNG) error {
			imp := Pick(r, impsOf(ortbReq(d)))
			impBidders(imp)[Pick(r, d.g.catalog.disabled)] = obj{}
			return nil
		}),
		feat("bidders.case", Uncertain, onORTB, "changes the letter case of a bidder name", hasBidderImp, recaseBidder),
		feat("bidders.legacy_location", Uncertain, onORTB, "moves bidder params from imp.ext.prebid.bidder to imp.ext.<bidder>", hasBidderImp, onBidderImp(legacyParams)),
		feat("bidders.near_miss", Invalidates, onORTB, "replaces one bidder's params with params that break one schema rule", hasCatalogBidder, nearMissParams),
		feat("bidders.schema_params", Preserves, onORTB, "replaces one bidder's params with a schema-generated instance", hasCatalogBidder, schemaParams),
		feat("bidders.adjustment_factor", Preserves, onORTB, "sets ext.prebid.bidadjustmentfactors", hasBidderImp, adjustmentFactor),
		feat("bidders.adjustment_bad", Invalidates, onORTB, "sets a zero bid adjustment factor", hasBidderImp, func(d *draft, r *RNG) error {
			path(ortbReq(d), "ext.prebid.bidadjustmentfactors")[Pick(r, requestBidders(ortbReq(d)))] = num(0)
			return nil
		}).rare(),
		feat("bidders.adjustments", Preserves, onORTB, "sets ext.prebid.bidadjustments (v2 rules)", hasBidderImp, bidAdjustments),
		feat("bidders.alternate_codes", Preserves, onORTB, "allows alternate bidder codes and makes the mock bid with one", hasBidderImp, alternateCodes),
	}
}

// hasCatalogBidder reports whether some imp names a catalog bidder.
func hasCatalogBidder(d *draft) bool {
	req := ortbReq(d)
	if req == nil {
		return false
	}
	for _, name := range requestBidders(req) {
		if d.spec(name) != nil {
			return true
		}
	}
	return false
}

// catalogImpBidder picks an imp and one of its catalog bidders.
func catalogImpBidder(d *draft, r *RNG) (obj, *bidderSpec) {
	type pair struct {
		imp obj
		b   *bidderSpec
	}
	var pairs []pair
	for _, imp := range impsOf(ortbReq(d)) {
		for _, name := range bidderNames(imp) {
			if b := d.spec(name); b != nil {
				pairs = append(pairs, pair{imp, b})
			}
		}
	}
	p := Pick(r, pairs)
	return p.imp, p.b
}

func addBidder(d *draft, r *RNG) error {
	req := ortbReq(d)
	imp := Pick(r, impsOf(req))
	exclude := stringSet(bidderNames(imp))
	b := d.g.catalog.otherBidder(r, channelOf(req), exclude)
	if b == nil {
		d.note("bidders.add: no further bidder supports %s", channelOf(req))
		return nil
	}
	params, label := d.g.catalog.params(r, b, ParamsValid)
	impBidders(imp)[b.name] = params
	d.note("bidders.add: %s params from %s", b.name, label)
	return nil
}

func addAlias(d *draft, r *RNG) error {
	req := ortbReq(d)
	imp := Pick(r, impsWithBidders(req))
	core := Pick(r, bidderNames(imp))
	alias := "paritygen" + strconv.Itoa(r.Range(1, 99))
	path(req, "ext.prebid.aliases")[alias] = core
	bidders := impBidders(imp)
	bidders[alias] = deepCopy(bidders[core])
	if r.Chance(0.5) {
		delete(bidders, core)
	}
	if b := d.spec(core); b != nil && r.Chance(0.4) {
		gvl := int64(b.gvlID)
		if gvl < 1 {
			gvl = 1
		}
		path(req, "ext.prebid.aliasgvlids")[alias] = num(gvl)
	}
	return nil
}

func recaseBidder(d *draft, r *RNG) error {
	imp := Pick(r, impsWithBidders(ortbReq(d)))
	name := Pick(r, bidderNames(imp))
	recased := strings.ToUpper(name)
	if r.Chance(0.5) {
		recased = strings.ToUpper(name[:1]) + name[1:]
	}
	if recased == name {
		d.note("bidders.case: %s has no letters to recase", name)
		return nil
	}
	bidders := impBidders(imp)
	bidders[recased] = bidders[name]
	delete(bidders, name)
	return nil
}

func legacyParams(_ *RNG, imp obj) {
	ext := child(imp, "ext")
	for name, params := range impBidders(imp) {
		ext[name] = params
	}
	p := path(imp, "ext.prebid")
	delete(p, "bidder")
	if len(p) == 0 {
		delete(ext, "prebid")
	}
}

func nearMissParams(d *draft, r *RNG) error {
	imp, b := catalogImpBidder(d, r)
	params, label := d.g.catalog.params(r, b, ParamsNearMiss)
	impBidders(imp)[b.name] = params
	d.note("bidders.near_miss: %s %s", b.name, label)
	return nil
}

func schemaParams(d *draft, r *RNG) error {
	imp, b := catalogImpBidder(d, r)
	params, verified := d.g.catalog.schemaParams(r, b)
	impBidders(imp)[b.name] = params
	if !verified {
		d.worsen(Uncertain)
		d.note("bidders.schema_params: %s instance not accepted by the validator", b.name)
	}
	return nil
}

func adjustmentFactor(d *draft, r *RNG) error {
	req := ortbReq(d)
	name := Pick(r, requestBidders(req))
	factor := fnum(Pick(r, []float64{0.5, 0.9, 1.1, 2}))
	path(req, "ext.prebid.bidadjustmentfactors")[name] = factor
	if r.Chance(0.3) {
		path(req, "ext.prebid.bidadjustmentfactors.mediatypes."+Pick(r, allMedia))[name] = fnum(0.8)
	}
	return nil
}

func bidAdjustments(d *draft, r *RNG) error {
	req := ortbReq(d)
	name := Pick(r, requestBidders(req))
	rule := obj{"adjtype": "multiplier", "value": fnum(Pick(r, []float64{0.8, 1.1, 1.5}))}
	switch r.Intn(3) {
	case 1:
		rule = obj{"adjtype": "cpm", "value": fnum(0.1), "currency": "USD"}
	case 2:
		rule = obj{"adjtype": "static", "value": fnum(1.25), "currency": "USD"}
	}
	media := Pick(r, append([]string{"*"}, allMedia...))
	path(req, "ext.prebid.bidadjustments.mediatype."+media+"."+name)["*"] = []any{rule}
	return nil
}

func alternateCodes(d *draft, r *RNG) error {
	req := ortbReq(d)
	name := Pick(r, requestBidders(req))
	alt := path(req, "ext.prebid.alternatebiddercodes")
	alt["enabled"] = true
	child(alt, "bidders")[name] = obj{"enabled": true, "allowedbiddercodes": strList("*")}
	d.mockBidder(name, func(m *corpus.MockBidder) { m.Seat = "paritygenseat" })
	return nil
}

// mockBidder edits the template parameters of one bidder.
func (d *draft) mockBidder(name string, edit func(m *corpus.MockBidder)) {
	if d.mocks.Bidders == nil {
		d.mocks.Bidders = map[string]corpus.MockBidder{}
	}
	m := d.mocks.Bidders[name]
	edit(&m)
	d.mocks.Bidders[name] = m
}

// customRates are request and host conversion rates covering every pool
// currency.
func customRates() obj {
	return obj{"USD": obj{"EUR": fnum(0.92), "GBP": fnum(0.79), "JPY": fnum(150.5)}}
}

func hostRates() map[string]map[string]float64 {
	return map[string]map[string]float64{"USD": {"EUR": 0.9, "GBP": 0.8, "JPY": 151}}
}

func requestExtMutators() []*mutator {
	return []*mutator{
		feat("currency.custom_rates", Preserves, onORTB, "sets ext.prebid.currency rates, usepbsrates and request.cur", hasORTB, func(d *draft, r *RNG) error {
			req := ortbReq(d)
			prebid(req)["currency"] = obj{"rates": customRates(), "usepbsrates": r.Chance(0.5)}
			req["cur"] = strList(Pick(r, []string{"USD", "EUR", "GBP"}))
			return nil
		}),
		feat("currency.host_rates", Preserves, onAll, "serves host currency rates", nil, func(d *draft, _ *RNG) error { d.profile.CurrencyRates = hostRates(); return nil }),
		feat("currency.bid", Preserves, onAll, "makes a bidder bid in another currency", hasCaseBidders, func(d *draft, r *RNG) error {
			cur := Pick(r, []string{"EUR", "GBP", "JPY"})
			d.mockBidder(Pick(r, d.bidders()), func(m *corpus.MockBidder) { m.Currency = cur })
			if r.Chance(0.6) {
				d.profile.CurrencyRates = hostRates()
			}
			return nil
		}),
		feat("currency.multi_cur", Preserves, onORTB, "lists several request currencies", hasORTB, func(d *draft, _ *RNG) error { ortbReq(d)["cur"] = strList("EUR", "USD"); return nil }),
		feat("currency.bad_code", Invalidates, onORTB, "uses an unknown currency code in custom rates", hasORTB, func(d *draft, _ *RNG) error {
			prebid(ortbReq(d))["currency"] = obj{"rates": obj{"USD": obj{"XYZ1": fnum(1.5)}}}
			return nil
		}).rare(),
		feat("floors.request", Preserves, onORTB, "sends ext.prebid.floors and enables floors in host and account config", hasORTB, requestFloors),
		feat("floors.high", Preserves, onORTB, "sets a floor above every mock bid with floors enabled", hasImps, func(d *draft, r *RNG) error {
			imp := Pick(r, impsOf(ortbReq(d)))
			imp["bidfloor"], imp["bidfloorcur"] = fnum(500), "USD"
			d.profile.FloorsEnabled, d.profile.AccountFloorsEnabled = true, true
			return nil
		}),
		feat("floors.bad", Uncertain, onORTB, "sends floors data that fails floors validation", hasORTB, func(d *draft, _ *RNG) error {
			group := obj{"modelweight": num(-1), "schema": obj{"fields": strList("unknownField")}, "values": obj{"x": fnum(-2)}}
			prebid(ortbReq(d))["floors"] = obj{"enabled": true, "data": obj{"modelgroups": []any{group}}}
			d.profile.FloorsEnabled = true
			return nil
		}).rare(),
		feat("multibid.request", Preserves, onORTB, "sets ext.prebid.multibid", hasBidderImp, multibid),
		feat("targeting.basic", Preserves, onORTB, "sets ext.prebid.targeting with a named price granularity", hasORTB, basicTargeting),
		feat("targeting.custom", Preserves, onORTB, "sets a custom price granularity", hasORTB, func(d *draft, _ *RNG) error {
			prebid(ortbReq(d))["targeting"] = obj{"pricegranularity": customGranularity(2), "includewinners": true}
			return nil
		}),
		feat("targeting.mediatype", Preserves, onORTB, "sets mediatypepricegranularity", hasORTB, func(d *draft, r *RNG) error {
			pg := obj{}
			for _, m := range Sample(r, []string{mediaBanner, mediaVideo, mediaNative}, r.Range(1, 2)) {
				pg[m] = customGranularity(int64(r.Range(0, 3)))
			}
			prebid(ortbReq(d))["targeting"] = obj{"mediatypepricegranularity": pg, "includebidderkeys": true}
			return nil
		}),
		feat("targeting.prefix", Preserves, onORTB, "sets a targeting prefix, deal preferences and format keys", hasORTB, func(d *draft, _ *RNG) error {
			prebid(ortbReq(d))["targeting"] = obj{"prefix": "pbs", "includewinners": true, "includebidderkeys": true, "includeformat": true, "preferdeals": true, "alwaysincludedeals": true}
			return nil
		}),
		feat("targeting.bad_precision", Invalidates, onORTB, "sets a price granularity precision above 15", hasORTB, func(d *draft, _ *RNG) error {
			prebid(ortbReq(d))["targeting"] = obj{"pricegranularity": customGranularity(16), "includewinners": true}
			return nil
		}).rare(),
		feat("targeting.bad_ranges", Invalidates, onORTB, "sets overlapping price granularity ranges", hasORTB, func(d *draft, _ *RNG) error {
			ranges := []any{obj{"min": fnum(0), "max": fnum(10), "increment": fnum(0.1)}, obj{"min": fnum(0), "max": fnum(5), "increment": fnum(0)}}
			prebid(ortbReq(d))["targeting"] = obj{"pricegranularity": obj{"precision": num(2), "ranges": ranges}, "includewinners": true}
			return nil
		}).rare(),
		feat("targeting.no_keys", Uncertain, onORTB, "disables both includewinners and includebidderkeys", hasORTB, func(d *draft, _ *RNG) error {
			prebid(ortbReq(d))["targeting"] = obj{"pricegranularity": "med", "includewinners": false, "includebidderkeys": false}
			return nil
		}).rare(),
		feat("cache.bids", Preserves, onORTB, "requests bid caching", hasORTB, func(d *draft, r *RNG) error {
			path(ortbReq(d), "ext.prebid.cache")["bids"] = obj{"returnCreative": r.Chance(0.5)}
			return nil
		}),
		feat("cache.vast", Preserves, onORTB, "requests VAST caching", hasORTB, func(d *draft, r *RNG) error {
			path(ortbReq(d), "ext.prebid.cache")["vastxml"] = obj{"returnCreative": r.Chance(0.5)}
			return nil
		}),
		feat("cache.empty", Invalidates, onAuction, "sends an empty ext.prebid.cache (AMP sets its own cache)", hasORTB, func(d *draft, _ *RNG) error { prebid(ortbReq(d))["cache"] = obj{}; return nil }).rare(),
		feat("events.enabled", Preserves, onORTB, "requests events and enables them for the account", hasORTB, func(d *draft, _ *RNG) error {
			prebid(ortbReq(d))["events"] = obj{}
			d.profile.EventsEnabled = true
			return nil
		}),
		feat("debug.test", Preserves, onAuction, "sets request.test=1", hasORTB, func(d *draft, _ *RNG) error { ortbReq(d)["test"] = num(1); return nil }),
		feat("debug.ext", Preserves, onAuction, "sets ext.prebid.debug", hasORTB, func(d *draft, _ *RNG) error { prebid(ortbReq(d))["debug"] = true; return nil }),
		feat("debug.trace", Preserves, onAuction, "sets ext.prebid.trace", hasORTB, func(d *draft, r *RNG) error {
			prebid(ortbReq(d))["trace"] = Pick(r, []string{"verbose", "basic"})
			return nil
		}),
		feat("tmax.valid", Preserves, onAll, "sets a common tmax", hasReq, func(d *draft, r *RNG) error {
			d.req()["tmax"] = num(int64(Pick(r, d.g.pools.Values.ValidTMax)))
			return nil
		}),
		feat("tmax.edge", Uncertain, onAll, "sets an edge tmax (0, 1, very large)", hasReq, func(d *draft, r *RNG) error {
			d.req()["tmax"] = num(int64(Pick(r, d.g.pools.Values.TMax)))
			return nil
		}),
		feat("tmax.negative", Invalidates, onAuction, "sets a negative tmax", hasORTB, func(d *draft, _ *RNG) error { ortbReq(d)["tmax"] = num(-1); return nil }).rare(),
		feat("ext.channel", Preserves, onORTB, "sets ext.prebid.channel", hasORTB, func(d *draft, r *RNG) error {
			prebid(ortbReq(d))["channel"] = obj{"name": Pick(r, []string{"web", "app", "pbjs", "amp"}), "version": "9.1"}
			return nil
		}),
		feat("ext.channel_empty", Invalidates, onORTB, "sets an empty ext.prebid.channel.name", hasORTB, func(d *draft, _ *RNG) error {
			prebid(ortbReq(d))["channel"] = obj{"name": ""}
			return nil
		}).rare(),
		feat("ext.returnallbidstatus", Preserves, onORTB, "asks for seat non-bids in the response", hasORTB, func(d *draft, _ *RNG) error {
			prebid(ortbReq(d))["returnallbidstatus"] = true
			return nil
		}),
		feat("ext.integration", Preserves, onORTB, "sets ext.prebid.integration", hasORTB, func(d *draft, r *RNG) error {
			prebid(ortbReq(d))["integration"] = Pick(r, []string{"pbjs", "amp", "parity"})
			return nil
		}),
		feat("ext.passthrough", Preserves, onORTB, "sets ext.prebid.passthrough", hasORTB, func(d *draft, _ *RNG) error {
			prebid(ortbReq(d))["passthrough"] = obj{"parity": "echo", "n": num(1)}
			return nil
		}),
		feat("ext.bidderparams", Uncertain, onORTB, "sets request-level ext.prebid.bidderparams", hasBidderImp, func(d *draft, r *RNG) error {
			path(ortbReq(d), "ext.prebid.bidderparams")[Pick(r, requestBidders(ortbReq(d)))] = obj{"parityParam": "x"}
			return nil
		}),
	}
}

func requestFloors(d *draft, r *RNG) error {
	group := obj{
		"currency":     "USD",
		"modelweight":  num(100),
		"modelversion": "parity-v1",
		"schema":       obj{"fields": strList("mediaType"), "delimiter": "|"},
		"values":       obj{"banner": fnum(1), "video": fnum(2), "native": fnum(0.5), "*": fnum(0.25)},
		"default":      fnum(0.1),
	}
	floors := obj{
		"enabled":     true,
		"floormin":    fnum(0.05),
		"data":        obj{"currency": "USD", "floorprovider": "parity", "modelgroups": []any{group}},
		"enforcement": obj{"enforcepbs": r.Chance(0.7), "floordeals": r.Chance(0.3), "enforcerate": num(100)},
	}
	prebid(ortbReq(d))["floors"] = floors
	d.profile.FloorsEnabled, d.profile.AccountFloorsEnabled = true, true
	return nil
}

func multibid(d *draft, r *RNG) error {
	req := ortbReq(d)
	bidders := requestBidders(req)
	entries := []any{obj{"bidder": bidders[0], "maxbids": num(int64(r.Range(2, 9))), "targetbiddercodeprefix": "pg" + r.Hex(3)}}
	if len(bidders) > 1 && r.Chance(0.5) {
		entries = append(entries, obj{"bidders": strList(bidders[1:]...), "maxbids": num(2)})
	}
	prebid(req)["multibid"] = entries
	return nil
}

func basicTargeting(d *draft, r *RNG) error {
	t := obj{"pricegranularity": Pick(r, []string{"low", "med", "medium", "high", "auto", "dense"})}
	winners := r.Chance(0.7)
	t["includewinners"] = winners
	t["includebidderkeys"] = !winners || r.Chance(0.5)
	if r.Chance(0.3) {
		t["includeformat"] = true
	}
	prebid(ortbReq(d))["targeting"] = t
	return nil
}

// customGranularity is a two-range granularity with the given precision.
func customGranularity(precision int64) obj {
	ranges := []any{
		obj{"min": fnum(0), "max": fnum(5), "increment": fnum(0.05)},
		obj{"min": fnum(5), "max": fnum(20), "increment": fnum(0.5)},
	}
	return obj{"precision": num(precision), "ranges": ranges}
}
