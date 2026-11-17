package gen

import (
	"bytes"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"sync"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/oracle"
)

// Channels and media types the generator draws.
const (
	channelSite = "site"
	channelApp  = "app"
	channelDOOH = "dooh"

	mediaBanner = "banner"
	mediaVideo  = "video"
	mediaNative = "native"
	mediaAudio  = "audio"
)

// allMedia lists the media types in a fixed order.
var allMedia = []string{mediaBanner, mediaVideo, mediaNative, mediaAudio}

// maxExamplesPerBidder caps the fixture params kept per bidder.
const maxExamplesPerBidder = 6

// ParamsMode selects how bidder params are drawn.
type ParamsMode int

const (
	// ParamsValid draws params the production validator accepts.
	ParamsValid ParamsMode = iota
	// ParamsNearMiss draws params that break exactly one schema rule.
	ParamsNearMiss
)

// bidderSpec is what the generator knows about one bidder.
type bidderSpec struct {
	name  string
	gvlID uint16
	// media maps a channel to the media types bidder-info declares for it.
	media  map[string]map[string]bool
	schema *paramSchema
	// examples are validator-approved params from the adapter fixtures, the
	// template fixture's params first.
	examples []json.RawMessage
	// template is true when the oracle answers this bidder with an OpenRTB bid.
	template bool
	// templateMedia and templateChannel describe the request the template
	// answered, so requests built for it resemble that fixture.
	templateMedia   string
	templateChannel string
}

// supports reports whether the bidder declares media on channel.
func (b *bidderSpec) supports(channel, media string) bool {
	return b.media[channel][media]
}

// catalog is the set of bidders the generator can put in a request.
type catalog struct {
	bidders   []*bidderSpec
	byName    map[string]*bidderSpec
	deep      []*bidderSpec
	disabled  []string
	validator openrtb_ext.BidderParamValidator
}

// bidderInfoCache holds the parsed static/bidder-info per directory.
// config.LoadBidderInfoFromDisk registers bidder-info aliases in package-level
// openrtb_ext state, so it runs at most once per directory and process.
var bidderInfoCache = struct {
	sync.Mutex
	byDir map[string]config.BidderInfos
}{byDir: map[string]config.BidderInfos{}}

func loadBidderInfos(dir string) (config.BidderInfos, error) {
	bidderInfoCache.Lock()
	defer bidderInfoCache.Unlock()
	if infos, ok := bidderInfoCache.byDir[dir]; ok {
		return infos, nil
	}
	infos, err := config.LoadBidderInfoFromDisk(dir)
	if err != nil {
		return nil, fmt.Errorf("gen: loading bidder info: %w", err)
	}
	bidderInfoCache.byDir[dir] = infos
	return infos, nil
}

// newCatalog builds the bidder catalog from static/bidder-info,
// static/bidder-params and the adapter corpus.
func newCatalog(repo *corpus.Repo, c *corpus.Corpus) (*catalog, error) {
	infoDir, err := repo.Resolve(corpus.RootBidderInfo)
	if err != nil {
		return nil, err
	}
	infos, err := loadBidderInfos(infoDir)
	if err != nil {
		return nil, err
	}
	paramsDir, err := repo.Resolve(corpus.RootBidderParams)
	if err != nil {
		return nil, err
	}
	validator, err := openrtb_ext.NewBidderParamsValidator(paramsDir)
	if err != nil {
		return nil, fmt.Errorf("gen: building params validator: %w", err)
	}
	cat := &catalog{byName: map[string]*bidderSpec{}, validator: validator}
	aliases := map[string]string{}
	for name, info := range infos {
		if info.AliasOf != "" {
			aliases[name] = info.AliasOf
		}
	}
	templates := oracle.TemplatesFromCorpus(c.Adapters, aliases)
	examples := fixtureExamples(c)
	fixtures := fixturesByPath(c)
	for _, name := range sortedKeys(infos) {
		info := infos[name]
		if !info.IsEnabled() {
			cat.disabled = append(cat.disabled, name)
			continue
		}
		spec, ok, err := cat.spec(name, info, templates, fixtures)
		if err != nil {
			return nil, err
		}
		if !ok {
			continue
		}
		spec.examples = append(spec.examples, examples[name]...)
		if len(spec.examples) == 0 && info.AliasOf != "" {
			spec.examples = append(spec.examples, examples[info.AliasOf]...)
		}
		spec.examples = dedupeRaw(spec.examples, maxExamplesPerBidder)
		cat.bidders = append(cat.bidders, spec)
		cat.byName[name] = spec
		if spec.template && len(spec.examples) > 0 {
			cat.deep = append(cat.deep, spec)
		}
	}
	if len(cat.deep) == 0 {
		return nil, fmt.Errorf("gen: no bidder has both an OpenRTB template and valid params")
	}
	return cat, nil
}

// spec builds one bidder's entry; ok is false for bidders the generator
// cannot use (no params schema or no capabilities).
func (cat *catalog) spec(name string, info config.BidderInfo, templates *oracle.TemplateSet, fixtures map[string]*corpus.AdapterFixture) (*bidderSpec, bool, error) {
	schemaText := cat.validator.Schema(openrtb_ext.BidderName(name))
	if schemaText == "" || info.Capabilities == nil {
		return nil, false, nil
	}
	schema, err := parseParamSchema([]byte(schemaText))
	if err != nil {
		return nil, false, fmt.Errorf("gen: bidder %s: %w", name, err)
	}
	spec := &bidderSpec{
		name:   name,
		gvlID:  info.GVLVendorID,
		media:  capabilityMedia(info.Capabilities),
		schema: schema,
	}
	t, ok := templates.Template(name)
	if !ok || !t.OpenRTB {
		return spec, true, nil
	}
	spec.template = true
	fixturePath, _, _ := strings.Cut(t.Source, "#")
	if f, ok := fixtures[fixturePath]; ok {
		spec.templateMedia, spec.templateChannel = fixtureShape(f.Spec.BidRequest)
		if p := firstImpParams(f.Spec.BidRequest); p != nil && cat.valid(name, p) {
			spec.examples = append(spec.examples, p)
		}
	}
	return spec, true, nil
}

// capabilityMedia converts bidder-info capabilities to channel media sets.
func capabilityMedia(c *config.CapabilitiesInfo) map[string]map[string]bool {
	out := map[string]map[string]bool{}
	add := func(channel string, p *config.PlatformInfo) {
		if p == nil {
			return
		}
		set := map[string]bool{}
		for _, m := range p.MediaTypes {
			set[string(m)] = true
		}
		out[channel] = set
	}
	add(channelSite, c.Site)
	add(channelApp, c.App)
	add(channelDOOH, c.DOOH)
	return out
}

// fixtureExamples groups the validator-approved params examples by bidder.
func fixtureExamples(c *corpus.Corpus) map[string][]json.RawMessage {
	out := map[string][]json.RawMessage{}
	if c.Params == nil {
		return out
	}
	for _, ex := range c.Params.Examples {
		if ex.Valid {
			out[ex.Bidder] = append(out[ex.Bidder], ex.Params)
		}
	}
	return out
}

func fixturesByPath(c *corpus.Corpus) map[string]*corpus.AdapterFixture {
	out := map[string]*corpus.AdapterFixture{}
	if c.Adapters == nil {
		return out
	}
	for _, f := range c.Adapters.Fixtures {
		out[f.Path] = f
	}
	return out
}

// fixtureShape returns the first imp's first media type and the channel of a
// fixture request.
func fixtureShape(raw json.RawMessage) (media, channel string) {
	req, err := decodeObject(raw)
	if err != nil {
		return "", ""
	}
	for _, ch := range []string{channelSite, channelApp, channelDOOH} {
		if _, ok := req[ch]; ok {
			channel = ch
			break
		}
	}
	imps := impsOf(req)
	if len(imps) == 0 {
		return "", channel
	}
	for _, m := range allMedia {
		if _, ok := imps[0][m]; ok {
			return m, channel
		}
	}
	return "", channel
}

// firstImpParams returns imp[0].ext.bidder of a fixture request.
func firstImpParams(raw json.RawMessage) json.RawMessage {
	req, err := decodeObject(raw)
	if err != nil {
		return nil
	}
	imps := impsOf(req)
	if len(imps) == 0 {
		return nil
	}
	params, ok := lookup(imps[0], "ext.bidder")
	if !ok || params == nil {
		return nil
	}
	enc, err := encodeTree(params)
	if err != nil {
		return nil
	}
	return enc
}

func dedupeRaw(in []json.RawMessage, limit int) []json.RawMessage {
	seen := map[string]bool{}
	var out []json.RawMessage
	for _, raw := range in {
		var buf bytes.Buffer
		if err := json.Compact(&buf, raw); err != nil {
			continue
		}
		if seen[buf.String()] {
			continue
		}
		seen[buf.String()] = true
		out = append(out, json.RawMessage(buf.Bytes()))
		if len(out) == limit {
			break
		}
	}
	return out
}

// valid reports whether the production validator accepts params for bidder.
func (cat *catalog) valid(bidder string, params []byte) bool {
	return cat.validator.Validate(openrtb_ext.BidderName(bidder), params) == nil
}

// paramsAttempts bounds schema draws before falling back to an example.
const paramsAttempts = 6

// params draws params for b. The label says where they came from:
// "example", "schema", "schema_unverified", or "near_miss:<violation>".
func (cat *catalog) params(r *RNG, b *bidderSpec, mode ParamsMode) (any, string) {
	valid, label := cat.validParams(r, b)
	if mode == ParamsValid {
		return valid, label
	}
	for _, m := range b.schema.nearMisses(r, valid) {
		raw, err := encodeTree(m.value)
		if err == nil && !cat.valid(b.name, raw) {
			return m.value, "near_miss:" + m.label
		}
	}
	return "parity-params", "near_miss:top_type"
}

// validParams prefers fixture examples, then schema instances the validator
// accepts.
func (cat *catalog) validParams(r *RNG, b *bidderSpec) (any, string) {
	if len(b.examples) > 0 && r.Chance(0.65) {
		if v, err := decodeTree(Pick(r, b.examples)); err == nil {
			return v, "example"
		}
	}
	last, verified := cat.schemaParams(r, b)
	if verified {
		return last, "schema"
	}
	if len(b.examples) > 0 {
		if v, err := decodeTree(b.examples[0]); err == nil {
			return v, "example"
		}
	}
	return last, "schema_unverified"
}

// schemaParams draws schema instances until the validator accepts one;
// verified is false when none of paramsAttempts draws passed.
func (cat *catalog) schemaParams(r *RNG, b *bidderSpec) (any, bool) {
	var last any
	for i := 0; i < paramsAttempts; i++ {
		last = b.schema.instance(r)
		raw, err := encodeTree(last)
		if err == nil && cat.valid(b.name, raw) {
			return last, true
		}
	}
	return last, false
}

// pickBidders draws n distinct bidders usable on channel, preferring bidders
// the oracle answers with bids.
func (cat *catalog) pickBidders(r *RNG, channel string, n int) []*bidderSpec {
	var deep, rest []*bidderSpec
	for _, b := range cat.deep {
		if len(b.media[channel]) > 0 {
			deep = append(deep, b)
		}
		// Bidders whose template answered a request on this channel are
		// listed twice: their auctions are the likeliest to produce bids.
		if b.templateChannel == channel && len(b.media[channel]) > 0 {
			deep = append(deep, b)
		}
	}
	for _, b := range cat.bidders {
		if len(b.media[channel]) > 0 && !(b.template && len(b.examples) > 0) {
			rest = append(rest, b)
		}
	}
	var out []*bidderSpec
	used := map[string]bool{}
	for attempts := 0; len(out) < n && attempts < 8*n; attempts++ {
		pool := deep
		if len(rest) > 0 && (len(deep) == 0 || r.Chance(0.15)) {
			pool = rest
		}
		b := Pick(r, pool)
		if b == nil || used[b.name] {
			continue
		}
		used[b.name] = true
		out = append(out, b)
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].name < out[j].name })
	return out
}

// otherBidder draws an enabled bidder not in exclude that supports channel.
func (cat *catalog) otherBidder(r *RNG, channel string, exclude map[string]bool) *bidderSpec {
	var pool []*bidderSpec
	for _, b := range cat.deep {
		if !exclude[b.name] && len(b.media[channel]) > 0 {
			pool = append(pool, b)
		}
	}
	return Pick(r, pool)
}
