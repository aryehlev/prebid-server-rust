package diff

import (
	"regexp"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
)

// Message rules normalize error texts whose content Go assembles from map
// iteration. They rewrite string values in place, keeping the source spelling
// of every original character of an exact value, so lexical comparison is
// unaffected.

// textPiece is one piece of a rewritten string: the original value's bytes
// [start, end), or literal text when lit is set.
type textPiece struct {
	start, end int
	lit        string
	isLit      bool
}

func spanPiece(start, end int) textPiece { return textPiece{start: start, end: end} }

func literalPiece(s string) textPiece { return textPiece{lit: s, isLit: true} }

// rewriteText replaces n's string value with pieces. An exact node keeps the
// source spelling of each original rune; literal pieces are spelled as JSON
// quoting spells them.
func rewriteText(n *node, pieces []textPiece) {
	var str strings.Builder
	for _, p := range pieces {
		if p.isLit {
			str.WriteString(p.lit)
		} else {
			str.WriteString(n.str[p.start:p.end])
		}
	}
	raw, ok := respell(n, pieces)
	if !ok {
		raw = jsonQuote(str.String())
	}
	n.str, n.raw = str.String(), raw
}

// respell builds the raw literal of a rewritten exact string from the source
// spellings of its runes. It fails when n is not exact or its raw literal does
// not decode to its value.
func respell(n *node, pieces []textPiece) (string, bool) {
	if !n.exact {
		return "", false
	}
	units := stringUnits(n.raw)
	offsets := make([]int, len(units)+1)
	var decoded strings.Builder
	for i, u := range units {
		offsets[i] = decoded.Len()
		decoded.WriteRune(u.r)
	}
	offsets[len(units)] = decoded.Len()
	if decoded.String() != n.str {
		return "", false
	}
	var raw strings.Builder
	raw.WriteByte('"')
	for _, p := range pieces {
		if p.isLit {
			raw.WriteString(strings.TrimSuffix(strings.TrimPrefix(jsonQuote(p.lit), `"`), `"`))
			continue
		}
		for i, u := range units {
			if offsets[i] >= p.start && offsets[i] < p.end {
				raw.WriteString(u.spelling)
			}
		}
	}
	raw.WriteByte('"')
	return raw.String(), true
}

// messageRoots are the nodes of a view whose string values the message rules
// rewrite for stage: entry values with their error and warning lists, the
// analytics records, and the HTTP response body.
func (e *Engine) messageRoots(v *view, stage parity.StageID) []*node {
	switch stage {
	case parity.StageAnalytics:
		return []*node{v.analytics}
	case parity.StageHTTPResponse:
		if v.httpResp != nil {
			return []*node{v.httpResp.body}
		}
		return nil
	}
	var out []*node
	for _, k := range sortedEntryKeys(v.entries[stage]) {
		en := v.entries[stage][k]
		out = append(out, en.value, en.errors, en.warnings)
	}
	return out
}

// messageView applies the message and candidate rules to every stage they
// apply to.
func (e *Engine) messageView(v *view) {
	e.maskResponseCur(v)
	e.maskAMPTargeting(v)
	storedFailures := storedAuctionFailures(v)
	for _, stage := range parity.AllStages() {
		sortParams := e.rules.Active(RuleBidderParamsErrorOrder, stage)
		maskStored := e.rules.Active(RuleStoredAuctionErrorImp, stage) && len(storedFailures) >= minStoredCandidates
		if !sortParams && !maskStored {
			continue
		}
		for _, root := range e.messageRoots(v, stage) {
			walk(root, func(n *node) {
				if n.kind != kindString {
					return
				}
				if sortParams {
					sortBidderParamsErrors(n)
				}
				if maskStored {
					maskStoredAuctionImp(n, storedFailures)
				}
			})
		}
	}
}

// bidder_params_error_order.
//
// ortb/request_validator.go:137 reports invalid bidder params as
// "request.imp[N].ext.prebid.bidder.<bidder> failed validation.\n" followed by
// every gojsonschema result error, concatenated without a separator
// (openrtb_ext/bidders.go:865-867). gojsonschema validates an object's
// properties in the order it stored them while parsing the schema, and it
// stores them by ranging over a Go map (gojsonschema@v1.2.0 schema.go:1020), so
// every process lists the errors of different properties in its own order. The
// rule splits the error list into its "<field>: <description>" items and sorts
// them; a list that does not split into template items exactly one way is left
// as recorded.

// bidderParamsPrefix ends where the error list of one validation message starts.
var bidderParamsPrefix = regexp.MustCompile(`request\.imp\[\d+\]\.ext\.prebid\.bidder\.\S+ failed validation\.\n`)

// schemaField is a gojsonschema field: "(root)" or a property path
// (ResultErrorFields.Field). Only paths that start with a letter, "_" or "$"
// are recognized, so a field cannot start inside a preceding number, quote or
// "(root)" ("... equal to 0.01(root): ..." splits one way only).
const schemaField = `(?:\(root\)|[A-Za-z_$][^\s:]*)`

// schemaItemStart matches where a "<field>: " item may start.
var schemaItemStart = regexp.MustCompile(`^` + schemaField + `: `)

// sortBidderParamsErrors sorts the error items of every bidder-params
// validation message in n.
func sortBidderParamsErrors(n *node) {
	locs := bidderParamsPrefix.FindAllStringIndex(n.str, -1)
	if locs == nil {
		return
	}
	var pieces []textPiece
	last, changed := 0, false
	for _, loc := range locs {
		start := loc[1]
		if start < last {
			continue
		}
		end := len(n.str)
		if i := strings.IndexByte(n.str[start:], '\n'); i >= 0 {
			end = start + i
		}
		items, ok := splitSchemaErrors(n.str[start:end])
		pieces = append(pieces, spanPiece(last, start))
		if !ok || len(items) < 2 {
			pieces = append(pieces, spanPiece(start, end))
		} else {
			pieces = append(pieces, sortedItemPieces(n.str, start, items)...)
			changed = true
		}
		last = end
	}
	if !changed {
		return
	}
	pieces = append(pieces, spanPiece(last, len(n.str)))
	rewriteText(n, pieces)
}

// sortedItemPieces orders the items (relative spans of the list starting at
// base) by (field, description).
func sortedItemPieces(s string, base int, items [][2]int) []textPiece {
	sorted := append([][2]int(nil), items...)
	text := func(it [2]int) string { return s[base+it[0] : base+it[1]] }
	sort.SliceStable(sorted, func(i, j int) bool {
		fi, di, _ := strings.Cut(text(sorted[i]), ": ")
		fj, dj, _ := strings.Cut(text(sorted[j]), ": ")
		if fi != fj {
			return fi < fj
		}
		return di < dj
	})
	out := make([]textPiece, len(sorted))
	for i, it := range sorted {
		out[i] = spanPiece(base+it[0], base+it[1])
	}
	return out
}

// splitSchemaErrors splits a concatenation of gojsonschema "<field>:
// <description>" items into item spans. Every item must match a gojsonschema
// error template completely, and the split must be the only one possible: a
// list that splits in two ways (a free-text value running into the next field)
// is not sorted. It reports false when s does not split exactly one way.
func splitSchemaErrors(s string) ([][2]int, bool) {
	var starts []int
	for p := 0; p < len(s); p++ {
		if schemaItemStart.MatchString(s[p:]) {
			starts = append(starts, p)
		}
	}
	if len(starts) == 0 || starts[0] != 0 {
		return nil, false
	}
	n := len(s)
	ends := append(append([]int(nil), starts[1:]...), n)
	ways := map[int]int{n: 1} // splits of s[p:], capped at 2
	next := map[int]int{}
	for i := len(starts) - 1; i >= 0; i-- {
		p := starts[i]
		for _, q := range ends[i:] {
			if ways[q] == 0 || !schemaItem.MatchString(s[p:q]) {
				continue
			}
			if ways[p] == 0 {
				next[p] = q
			}
			ways[p] = min(ways[p]+ways[q], 2)
		}
	}
	if ways[0] != 1 {
		return nil, false
	}
	var items [][2]int
	for p := 0; p < n; p = next[p] {
		items = append(items, [2]int{p, next[p]})
	}
	return items, true
}

// stored_auction_error_imp.
//
// exchange/exchange.go:1560 builds a stored auction response by ranging over
// the map of stored responses by imp ID and returns the first error, so when
// the stored responses of several imps hold bids whose media type cannot be
// determined (exchange/utils.go:1151-1199 getMediaTypeForBid), the imp ID in
// "Failed to parse bid mediatype for impression \"<imp>\"" is whichever of them
// Go reached first. The rule replaces that imp ID with <stored-auction-imp>, and
// only when the side's own stored.responses stage shows at least two imps whose
// stored bids getMediaTypeForBid rejects and the reported imp is one of them.

// minStoredCandidates is the number of failing imps from which the reported
// imp depends on map iteration.
const minStoredCandidates = 2

// storedAuctionImpPlaceholder replaces the reported imp ID.
const storedAuctionImpPlaceholder = "<stored-auction-imp>"

// storedMediaTypeError matches the two getMediaTypeForBid messages and captures
// the imp ID.
var storedMediaTypeError = regexp.MustCompile(`Failed to parse bid (?:mediatype|mType) for impression "([^"]*)"`)

// maskStoredAuctionImp replaces candidate imp IDs in the messages of n.
func maskStoredAuctionImp(n *node, failing map[string]bool) {
	locs := storedMediaTypeError.FindAllStringSubmatchIndex(n.str, -1)
	var pieces []textPiece
	last := 0
	for _, loc := range locs {
		start, end := loc[2], loc[3]
		if !failing[n.str[start:end]] {
			continue
		}
		pieces = append(pieces, spanPiece(last, start), literalPiece(storedAuctionImpPlaceholder))
		last = end
	}
	if pieces == nil {
		return
	}
	rewriteText(n, append(pieces, spanPiece(last, len(n.str))))
}

// storedAuctionFailures returns the imp IDs whose stored auction response, as
// the side recorded it in stored.responses ({auction: {imp: Body}}), holds a bid
// that getMediaTypeForBid rejects.
func storedAuctionFailures(v *view) map[string]bool {
	out := map[string]bool{}
	for _, en := range v.entries[parity.StageStoredResponses] {
		auction := en.value.get("auction")
		if auction == nil || auction.kind != kindObject {
			continue
		}
		for _, imp := range auction.keys() {
			if seatBids := storedBody(auction.get(imp)); seatBids != nil && hasUntypedBid(seatBids) {
				out[imp] = true
			}
		}
	}
	return out
}

// storedBody returns the JSON content of a recorded parity Body.
func storedBody(b *node) *node {
	if b == nil || b.kind != kindObject {
		return nil
	}
	if j := b.get("json"); j != nil {
		return j
	}
	if t := b.get("text"); t != nil && t.kind == kindString {
		if n, err := parseSemantic([]byte(t.str)); err == nil {
			return n
		}
	}
	return nil
}

// bidTypes are the bid.ext.prebid.type values openrtb_ext.ParseBidType accepts.
var bidTypes = map[string]bool{"banner": true, "video": true, "audio": true, "native": true}

// hasUntypedBid reports whether getMediaTypeForBid rejects a bid of a stored
// seatbid list: an mtype above 4, or an mtype of 0 or less (absent) without a
// valid ext.prebid.type. A list with an mtype that does not decode as Go's
// int8 MarkupType fails to unmarshal before any media type is checked, so it
// does not count.
func hasUntypedBid(seatBids *node) bool {
	if seatBids.kind != kindArray {
		return false
	}
	untyped := false
	for _, bid := range elementNodes(selectNodes(seatBids, "[*].bid")) {
		if bid.kind != kindObject {
			continue
		}
		mtype, ok := markupType(bid.get("mtype"))
		switch {
		case !ok:
			return false
		case mtype > 4:
			untyped = true
		case mtype < 1 && !hasBidType(bid):
			untyped = true
		}
	}
	return untyped
}

// markupType decodes bid.mtype as Go's int8 MarkupType; absent or null is 0.
func markupType(n *node) (int64, bool) {
	if n == nil || n.kind == kindNull {
		return 0, true
	}
	if n.kind != kindNumber || !n.num.IsInt() || !n.num.Num().IsInt64() {
		return 0, false
	}
	v := n.num.Num().Int64()
	return v, v >= -128 && v <= 127
}

// hasBidType reports whether bid.ext.prebid.type is a type
// openrtb_ext.ParseBidType accepts.
func hasBidType(bid *node) bool {
	typ := memberPath(bid, "ext.prebid.type")
	return typ != nil && typ.kind == kindString && bidTypes[typ.str]
}

// response_cur_seat.
//
// buildBidResponse sets the response cur to the currency of every seat with
// bids while ranging over the seat map, so the last seat Go's map iteration
// visits wins (exchange/exchange.go:1027-1032). When the seats with bids have
// different currencies, two Go processes can answer with different cur values.
// The rule masks the response cur only when the side's own bidder.result
// stage shows seats with bids in at least two currencies and the cur is one of
// them.

// seatCurrencies returns the currencies of the seats with at least one bid in
// the side's bidder.result entries (stagedata.BidderResult seat_bids).
func seatCurrencies(v *view) map[string]bool {
	out := map[string]bool{}
	for _, en := range v.entries[parity.StageBidderResult] {
		for _, seat := range elementNodes(selectNodes(en.value, "seat_bids")) {
			bids, cur := seat.get("bids"), seat.get("currency")
			if bids != nil && bids.kind == kindArray && len(bids.elems) > 0 && cur != nil && cur.kind == kindString {
				out[cur.str] = true
			}
		}
	}
	return out
}

// maskResponseCur applies response_cur_seat at the response anchors of every
// stage it applies to.
func (e *Engine) maskResponseCur(v *view) {
	currencies := seatCurrencies(v)
	if len(currencies) < minStoredCandidates {
		return
	}
	for _, stage := range parity.AllStages() {
		if !e.rules.Active(RuleResponseCurSeat, stage) {
			continue
		}
		for _, root := range e.stageRoots(v, stage) {
			for _, cur := range selectAll(root, responseAnchors[stage], "cur") {
				if cur.kind == kindString && currencies[cur.str] {
					mask(cur, "candidate:"+RuleResponseCurSeat)
				}
			}
		}
	}
}
