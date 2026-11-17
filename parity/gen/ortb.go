package gen

import (
	"encoding/json"
	"fmt"
	"reflect"
	"strconv"
	"strings"

	"github.com/prebid/openrtb/v20/adcom1"
	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// Base requests are built from the Go openrtb2 types: a structured skeleton
// that Go validation accepts, plus a reflection pass that sets a random subset
// of every other optional field from the pools. Because the field list comes
// from the Go types, generated requests carry fields the Rust model may not
// know, which exposes dropped fields and omitempty differences.

// Reflection-fill densities: the chance of setting each eligible field.
const (
	densityImp     = 0.10
	densityMedia   = 0.15
	densityChannel = 0.15
	densityDevice  = 0.20
	densityUser    = 0.20
	densityNested  = 0.35
	maxFillDepth   = 2
)

// fillSkip lists, per struct, the fields the skeleton owns or that Go
// validation constrains; reflection never sets them.
var fillSkip = map[string]map[string]bool{
	"Imp":       set("id", "metric", "banner", "video", "audio", "native", "pmp", "instl", "bidfloor", "bidfloorcur", "rwdd", "exp", "qty", "refresh", "ext"),
	"Banner":    set("format", "w", "h", "wmax", "hmax", "wmin", "hmin", "ext"),
	"Video":     set("mimes", "minduration", "maxduration", "protocols", "w", "h", "companionad", "durfloors", "ext"),
	"Audio":     set("mimes", "minduration", "maxduration", "companionad", "durfloors", "ext"),
	"Native":    set("request", "ver", "ext"),
	"Site":      set("id", "domain", "page", "ref", "cat", "publisher", "ext"),
	"App":       set("id", "name", "bundle", "storeurl", "ver", "cat", "publisher", "ext"),
	"DOOH":      set("id", "name", "venuetype", "publisher", "ext"),
	"Device":    set("geo", "dnt", "lmt", "ua", "ip", "ipv6", "ifa", "w", "h", "devicetype", "os", "ext"),
	"Geo":       set("country", "ext"),
	"User":      set("consent", "eids", "ext"),
	"Publisher": set("id", "ext"),
}

// fillStructs are the nested struct types reflection may create.
var fillStructs = set("Geo", "Content", "Producer", "Network", "Channel", "Data", "Segment", "UserAgent", "BrandVersion")

func set(xs ...string) map[string]bool { return stringSet(xs) }

// requestShape constrains a base request.
type requestShape struct {
	channel string
	imps    int
	media   []string
}

// baseAuction builds a valid /openrtb2/auction body.
func (g *Generator) baseAuction(r *RNG, d *draft) error {
	req, err := g.ortbRequest(r, requestShape{})
	if err != nil {
		return err
	}
	d.body = req
	return nil
}

// ortbRequest builds a valid OpenRTB 2.6 request tree.
func (g *Generator) ortbRequest(r *RNG, shape requestShape) (obj, error) {
	channel := shape.channel
	if channel == "" {
		channel = []string{channelSite, channelApp, channelDOOH}[r.Weighted([]int{60, 32, 8})]
	}
	bidders := g.catalog.pickBidders(r, channel, 1+r.Weighted([]int{55, 30, 15}))
	if len(bidders) == 0 {
		return nil, fmt.Errorf("gen: no bidder supports channel %s", channel)
	}
	nImps := shape.imps
	if nImps == 0 {
		nImps = 1 + r.Weighted([]int{50, 30, 15, 5})
	}
	br := openrtb2.BidRequest{ID: requestIDPrefix + r.Hex(12)}
	g.fillChannel(r, &br, channel)
	br.Device = g.device(r, channel)
	br.User = g.user(r)
	if r.Chance(0.3) {
		br.Source = &openrtb2.Source{TID: "tid-" + r.Hex(10)}
	}
	if r.Chance(0.4) {
		br.Cur = []string{"USD"}
	}
	if r.Chance(0.3) {
		br.TMax = int64(Pick(r, g.pools.Values.ValidTMax))
	}
	if r.Chance(0.2) {
		br.AT = int64(r.Range(1, 2))
	}
	if r.Chance(0.15) {
		br.BCat = []string{Pick(r, g.pools.ORTB.IABCategories)}
		br.BAdv = []string{Pick(r, g.pools.ORTB.ADomains)}
	}
	imps := make([]any, 0, nImps)
	usedIDs := map[string]bool{}
	for i := 0; i < nImps; i++ {
		imp, err := g.impFor(r, i, channel, bidders, shape.media, usedIDs)
		if err != nil {
			return nil, err
		}
		imps = append(imps, imp)
	}
	req, err := toObject(br)
	if err != nil {
		return nil, err
	}
	req["imp"] = imps
	return req, nil
}

// impFor builds one imp with a non-empty bidder subset and media they support.
func (g *Generator) impFor(r *RNG, i int, channel string, bidders []*bidderSpec, media []string, used map[string]bool) (obj, error) {
	var chosen []*bidderSpec
	for _, b := range bidders {
		if r.Chance(0.7) {
			chosen = append(chosen, b)
		}
	}
	if len(chosen) == 0 {
		chosen = []*bidderSpec{Pick(r, bidders)}
	}
	if len(media) == 0 {
		media = g.drawMedia(r, channel, chosen)
	}
	imp := g.imp(r, impID(r, i, used), media)
	o, err := toObject(imp)
	if err != nil {
		return nil, err
	}
	ext := obj{}
	params := obj{}
	for _, b := range chosen {
		p, _ := g.catalog.params(r, b, ParamsValid)
		params[b.name] = p
	}
	ext["prebid"] = obj{"bidder": params}
	if r.Chance(0.2) {
		ext["gpid"] = "/1234/parity/" + strconv.Itoa(i)
	}
	if r.Chance(0.15) {
		ext["data"] = obj{"pbadslot": "/1234/parity/slot-" + strconv.Itoa(i)}
	}
	o["ext"] = ext
	return o, nil
}

// impID returns a unique imp ID in one of the shapes fixtures use.
func impID(r *RNG, i int, used map[string]bool) string {
	for {
		var id string
		switch r.Weighted([]int{6, 2, 2}) {
		case 0:
			id = "imp-" + strconv.Itoa(i+1)
		case 1:
			id = strconv.Itoa(i + 1)
		default:
			id = "div-gpt-ad-" + r.Hex(4) + "-" + strconv.Itoa(i)
		}
		if !used[id] {
			used[id] = true
			return id
		}
	}
}

// drawMedia picks the imp media: one type every bidder supports on channel,
// sometimes two (multi-format), preferring the template fixture's type.
func (g *Generator) drawMedia(r *RNG, channel string, bidders []*bidderSpec) []string {
	var common []string
	for _, m := range allMedia {
		ok := true
		for _, b := range bidders {
			ok = ok && b.supports(channel, m)
		}
		if ok {
			common = append(common, m)
		}
	}
	if len(common) == 0 {
		for _, m := range allMedia {
			if bidders[0].supports(channel, m) {
				common = append(common, m)
			}
		}
	}
	if len(common) == 0 {
		return []string{mediaBanner}
	}
	first := common[weightedMedia(r, common)]
	if len(bidders) == 1 && r.Chance(0.6) && bidders[0].templateMedia != "" && contains(common, bidders[0].templateMedia) {
		first = bidders[0].templateMedia
	}
	out := []string{first}
	if len(common) > 1 && r.Chance(0.18) {
		second := Pick(r, common)
		if second != first {
			out = append(out, second)
		}
	}
	return out
}

func weightedMedia(r *RNG, media []string) int {
	weights := make([]int, len(media))
	for i, m := range media {
		weights[i] = map[string]int{mediaBanner: 45, mediaVideo: 20, mediaNative: 15, mediaAudio: 5}[m]
	}
	return r.Weighted(weights)
}

func contains(xs []string, x string) bool {
	for _, v := range xs {
		if v == x {
			return true
		}
	}
	return false
}

// imp builds the typed imp skeleton for media.
func (g *Generator) imp(r *RNG, id string, media []string) openrtb2.Imp {
	imp := openrtb2.Imp{ID: id}
	p := &g.pools.ORTB
	for _, m := range media {
		switch m {
		case mediaBanner:
			imp.Banner = g.banner(r)
		case mediaVideo:
			imp.Video = g.video(r)
		case mediaNative:
			imp.Native = &openrtb2.Native{Request: Pick(r, p.NativeRequests), Ver: "1.2"}
		case mediaAudio:
			d := Pick(r, p.VideoDurations)
			imp.Audio = &openrtb2.Audio{MIMEs: Pick(r, p.AudioMimes), MinDuration: int64(d[0]), MaxDuration: int64(d[1])}
		}
	}
	g.fill(r, reflect.ValueOf(&imp).Elem(), densityImp, 0)
	return imp
}

func (g *Generator) banner(r *RNG) *openrtb2.Banner {
	sizes := Sample(r, g.pools.ORTB.BannerSizes, r.Range(1, 3))
	b := &openrtb2.Banner{}
	for _, s := range sizes {
		b.Format = append(b.Format, openrtb2.Format{W: int64(s[0]), H: int64(s[1])})
	}
	if r.Chance(0.2) {
		w, h := int64(sizes[0][0]), int64(sizes[0][1])
		b.W, b.H = &w, &h
	}
	g.fill(r, reflect.ValueOf(b).Elem(), densityMedia, 0)
	return b
}

func (g *Generator) video(r *RNG) *openrtb2.Video {
	p := &g.pools.ORTB
	size := Pick(r, p.VideoSizes)
	dur := Pick(r, p.VideoDurations)
	w, h := int64(size[0]), int64(size[1])
	v := &openrtb2.Video{
		MIMEs:       Pick(r, p.VideoMimes),
		MinDuration: int64(dur[0]),
		MaxDuration: int64(dur[1]),
		W:           &w,
		H:           &h,
		Linearity:   adcom1.LinearityLinear,
	}
	for _, proto := range Pick(r, p.VideoProtocols) {
		v.Protocols = append(v.Protocols, adcom1.MediaCreativeSubtype(proto))
	}
	if r.Chance(0.5) {
		v.Plcmt = adcom1.VideoPlcmtSubtype(r.Range(1, 4))
	}
	g.fill(r, reflect.ValueOf(v).Elem(), densityMedia, 0)
	return v
}

// fillChannel sets exactly one of site, app and dooh.
func (g *Generator) fillChannel(r *RNG, br *openrtb2.BidRequest, channel string) {
	p := &g.pools.ORTB
	switch channel {
	case channelApp:
		a := Pick(r, p.Apps)
		app := &openrtb2.App{ID: a.ID, Name: a.Name, Bundle: a.Bundle, StoreURL: a.StoreURL, Ver: a.Ver, Cat: a.Cat, Publisher: &openrtb2.Publisher{ID: a.Publisher}}
		g.fill(r, reflect.ValueOf(app).Elem(), densityChannel, 0)
		br.App = app
	case channelDOOH:
		v := Pick(r, p.DOOH)
		br.DOOH = &openrtb2.DOOH{ID: v.ID, Name: v.Name, VenueType: v.VenueType, Publisher: &openrtb2.Publisher{ID: v.Publisher}}
	default:
		s := Pick(r, p.Sites)
		site := &openrtb2.Site{Page: s.Page, Domain: s.Domain, Ref: s.Ref, Cat: s.Cat, Publisher: &openrtb2.Publisher{ID: s.Publisher}}
		if r.Chance(0.4) {
			site.ID = "site-" + r.Hex(6)
		}
		g.fill(r, reflect.ValueOf(site).Elem(), densityChannel, 0)
		br.Site = site
	}
}

// device builds a device with a pool user agent and address.
func (g *Generator) device(r *RNG, channel string) *openrtb2.Device {
	p := &g.pools.ORTB
	dev := &openrtb2.Device{UA: Pick(r, p.UserAgents), DeviceType: adcom1.DeviceType(r.Range(1, 7))}
	if r.Chance(0.8) {
		dev.IP = Pick(r, p.IPv4)
	} else {
		dev.IPv6 = Pick(r, p.IPv6)
	}
	if r.Chance(0.6) {
		dev.OS = Pick(r, p.Fields["Device.os"])
	}
	if r.Chance(0.3) {
		size := Pick(r, p.BannerSizes)
		dev.W, dev.H = int64(size[0]), int64(size[1])
	}
	if channel == channelApp && r.Chance(0.6) {
		dev.IFA = r.UUID()
	}
	if r.Chance(0.6) {
		dev.Geo = &openrtb2.Geo{Country: Pick(r, p.Countries), Type: adcom1.LocationType(r.Range(1, 3))}
		g.fill(r, reflect.ValueOf(dev.Geo).Elem(), densityNested, 1)
	}
	g.fill(r, reflect.ValueOf(dev).Elem(), densityDevice, 0)
	return dev
}

// user builds an optional user.
func (g *Generator) user(r *RNG) *openrtb2.User {
	if !r.Chance(0.6) {
		return nil
	}
	u := &openrtb2.User{}
	if r.Chance(0.5) {
		u.ID = "user-" + r.Hex(8)
	}
	g.fill(r, reflect.ValueOf(u).Elem(), densityUser, 0)
	return u
}

// fill sets a random subset of v's zero-valued optional fields.
func (g *Generator) fill(r *RNG, v reflect.Value, density float64, depth int) {
	t := v.Type()
	skip := fillSkip[t.Name()]
	for i := 0; i < t.NumField(); i++ {
		f := t.Field(i)
		name := jsonName(f)
		if name == "" || skip[name] || !f.IsExported() || !v.Field(i).IsZero() {
			continue
		}
		if !r.Chance(density) {
			continue
		}
		if val, ok := g.fieldValue(r, t.Name(), name, f.Type, depth); ok {
			v.Field(i).Set(val)
		}
	}
}

func jsonName(f reflect.StructField) string {
	name, _, _ := strings.Cut(f.Tag.Get("json"), ",")
	if name == "-" || name == "ext" {
		return ""
	}
	return name
}

var rawMessageType = reflect.TypeOf(json.RawMessage{})

// fieldValue draws a value of type t for field owner.name.
func (g *Generator) fieldValue(r *RNG, owner, name string, t reflect.Type, depth int) (reflect.Value, bool) {
	if t == rawMessageType {
		return reflect.Value{}, false
	}
	switch t.Kind() {
	case reflect.Ptr:
		inner, ok := g.fieldValue(r, owner, name, t.Elem(), depth)
		if !ok {
			return reflect.Value{}, false
		}
		p := reflect.New(t.Elem())
		p.Elem().Set(inner)
		return p, true
	case reflect.String:
		return reflect.ValueOf(g.stringValue(r, owner, name)).Convert(t), true
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return reflect.ValueOf(intValue(r, name, t)).Convert(t), true
	case reflect.Float64, reflect.Float32:
		return reflect.ValueOf(floatValue(r, name)).Convert(t), true
	case reflect.Slice:
		return g.sliceValue(r, owner, name, t, depth)
	case reflect.Struct:
		if depth >= maxFillDepth || !fillStructs[t.Name()] {
			return reflect.Value{}, false
		}
		s := reflect.New(t).Elem()
		g.fill(r, s, densityNested, depth+1)
		return s, true
	}
	return reflect.Value{}, false
}

func (g *Generator) sliceValue(r *RNG, owner, name string, t reflect.Type, depth int) (reflect.Value, bool) {
	n := r.Range(1, 2)
	out := reflect.MakeSlice(t, 0, n)
	for i := 0; i < n; i++ {
		v, ok := g.fieldValue(r, owner, name, t.Elem(), depth)
		if !ok {
			return reflect.Value{}, false
		}
		out = reflect.Append(out, v)
	}
	return out, true
}

func (g *Generator) stringValue(r *RNG, owner, name string) string {
	if pool := g.pools.ORTB.Fields[owner+"."+name]; len(pool) > 0 {
		return Pick(r, pool)
	}
	switch name {
	case "cat", "sectioncat", "pagecat", "bcat", "acat":
		return Pick(r, g.pools.ORTB.IABCategories)
	case "language":
		return Pick(r, g.pools.ORTB.Languages)
	case "domain", "inventorypartnerdomain":
		return Pick(r, g.pools.ORTB.ADomains)
	case "keywords", "kwarray":
		return Pick(r, g.pools.ORTB.Keywords)
	}
	return "parity-" + name + "-" + strconv.Itoa(r.Range(1, 99))
}

// intValue draws a positive value; flags (int8) are 0 or 1, enums 1 to 3.
func intValue(r *RNG, name string, t reflect.Type) int64 {
	switch {
	case name == "yob":
		return int64(r.Range(1960, 2005))
	case t.PkgPath() != "":
		return int64(r.Range(1, 3))
	case t.Kind() == reflect.Int8:
		return int64(r.Range(0, 1))
	}
	return int64(Pick(r, []int{1, 2, 3, 5, 10, 15, 30, 60, 120, 300}))
}

func floatValue(r *RNG, name string) float64 {
	switch name {
	case "lat":
		return Pick(r, []float64{37.7749, 52.52, 48.8566, -23.5505})
	case "lon":
		return Pick(r, []float64{-122.4194, 13.405, 2.3522, -46.6333})
	case "pxratio":
		return Pick(r, []float64{1, 1.5, 2, 3})
	}
	return Pick(r, []float64{0.1, 0.5, 1, 1.25, 2.5})
}

// baseAMP builds an AMP request: GET with tag_id and size parameters, and a
// one-imp site request stored under the tag.
func (g *Generator) baseAMP(r *RNG, d *draft) error {
	media := []string{mediaBanner}
	if r.Chance(0.15) {
		media = []string{mediaVideo}
	}
	req, err := g.ortbRequest(r, requestShape{channel: channelSite, imps: 1, media: media})
	if err != nil {
		return err
	}
	d.amp = req
	d.query = []queryParam{{key: "tag_id", value: "pending", hasValue: true}}
	if r.Chance(0.5) {
		size := Pick(r, g.pools.ORTB.BannerSizes)
		d.setQuery("w", strconv.Itoa(size[0]))
		d.setQuery("h", strconv.Itoa(size[1]))
	}
	if r.Chance(0.3) {
		d.setQuery("curl", Pick(r, g.pools.ORTB.Sites).Page)
	}
	return nil
}

// baseVideo builds a /openrtb2/video request whose pods reference stored imps
// and whose storedrequestid references a stored partial video request.
func (g *Generator) baseVideo(r *RNG, d *draft) error {
	channel := []string{channelSite, channelApp}[r.Weighted([]int{60, 40})]
	shape, err := g.ortbRequest(r, requestShape{channel: channel, imps: 1, media: []string{mediaVideo}})
	if err != nil {
		return err
	}
	p := &g.pools.ORTB
	durations := Pick(r, [][]int{{15, 30}, {15}, {5, 15, 30}, {30, 60}})
	body := obj{
		"podconfig": obj{"durationrangesec": intList(durations...), "pods": []any{}},
		"video": obj{
			"mimes":     strList(Pick(r, p.VideoMimes)...),
			"protocols": intList(Pick(r, p.VideoProtocols)...),
			"w":         num(int64(Pick(r, p.VideoSizes)[0])),
			"h":         num(int64(Pick(r, p.VideoSizes)[1])),
		},
	}
	if r.Chance(0.3) {
		path(body, "podconfig")["requireexactduration"] = true
	}
	for _, key := range []string{"device", "user", "source"} {
		if v, ok := shape[key]; ok && key != "source" {
			body[key] = v
		}
	}
	if err := g.videoPods(r, d, body, shape, r.Range(1, 2)); err != nil {
		return err
	}
	stored := obj{channel: shape[channel]}
	if r.Chance(0.4) {
		stored["tmax"] = num(int64(Pick(r, g.pools.Values.ValidTMax)))
	}
	id, err := d.storeAs("req", stored)
	if err != nil {
		return err
	}
	body["storedrequestid"] = id
	d.body = body
	return nil
}

// videoPods appends n pods whose configids are stored imps carrying the
// shape request's bidders.
func (g *Generator) videoPods(r *RNG, d *draft, body, shape obj, n int) error {
	pods, _ := lookup(body, "podconfig.pods")
	list, _ := pods.([]any)
	imps := impsOf(shape)
	if len(imps) == 0 {
		return fmt.Errorf("gen: video shape request has no imp")
	}
	bidders, _ := lookupObj(imps[0], "ext.prebid.bidder")
	for i := 0; i < n; i++ {
		storedImp := obj{"ext": obj{"prebid": obj{"bidder": deepCopy(bidders)}}}
		if r.Chance(0.3) {
			storedImp["tagid"] = "pod-tag-" + strconv.Itoa(len(list)+1)
		}
		id, err := d.storeAs("imp", storedImp)
		if err != nil {
			return err
		}
		list = append(list, obj{
			"podid":            num(int64(len(list) + 1)),
			"adpoddurationsec": num(int64(Pick(r, []int{30, 60, 90, 120}))),
			"configid":         id,
		})
	}
	path(body, "podconfig")["pods"] = list
	return nil
}

// videoImpIDs mirrors createImpressions: "<podid>_<i>" for i below
// adpoddurationsec / min(durationrangesec), raised to the duration count when
// requireexactduration is set.
func videoImpIDs(req obj) []string {
	if req == nil {
		return nil
	}
	cfg, _ := lookupObj(req, "podconfig")
	durs, _ := cfg["durationrangesec"].([]any)
	minDur := 0
	for _, d := range durs {
		if v, ok := intOf(d); ok && v > 0 && (minDur == 0 || v < minDur) {
			minDur = v
		}
	}
	if minDur == 0 {
		return nil
	}
	exact, _ := cfg["requireexactduration"].(bool)
	pods, _ := cfg["pods"].([]any)
	var out []string
	for _, p := range pods {
		po, _ := p.(obj)
		podID, ok1 := intOf(po["podid"])
		dur, ok2 := intOf(po["adpoddurationsec"])
		if !ok1 || !ok2 {
			continue
		}
		n := dur / minDur
		if exact && n < len(durs) {
			n = len(durs)
		}
		for i := 0; i < n && i < 64; i++ {
			out = append(out, strconv.Itoa(podID)+"_"+strconv.Itoa(i))
		}
	}
	return out
}

func intOf(v any) (int, bool) {
	n, ok := v.(json.Number)
	if !ok {
		return 0, false
	}
	i, err := n.Int64()
	return int(i), err == nil
}

// channelOf returns which of site, app and dooh a request carries.
func channelOf(req obj) string {
	for _, ch := range []string{channelSite, channelApp, channelDOOH} {
		if _, ok := req[ch]; ok {
			return ch
		}
	}
	return channelSite
}

// requestBidders lists the bidder names of every imp, sorted and unique.
func requestBidders(req obj) []string {
	seen := map[string]bool{}
	for _, imp := range impsOf(req) {
		if b, ok := lookupObj(imp, "ext.prebid.bidder"); ok {
			for k := range b {
				seen[k] = true
			}
		}
	}
	return sortedKeys(seen)
}

// isVideoEndpoint reports whether d targets /openrtb2/video.
func (d *draft) isVideoEndpoint() bool { return d.endpoint == corpus.EndpointVideo }
