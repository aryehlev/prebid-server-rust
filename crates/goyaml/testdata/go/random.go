package main

import (
	"fmt"
	"math"
	"math/rand"
	"strconv"
	"strings"
)

// yamlGen writes random YAML documents from a small grammar: block and flow collections at
// varying indentation, scalars in every style, anchors and aliases, merge keys, tags, comments
// and document markers, with now and then a tab, a BOM or CRLF line breaks. Keys come mostly
// from Kitchen and config.BidderInfo, so that the typed decoders see their fields.
type yamlGen struct {
	rnd *rand.Rand
	// anchors are the anchors defined so far, and mapAnchors those on block mappings.
	anchors, mapAnchors []string
	next                int
}

// randomKeys are mapping keys besides Kitchen's and BidderInfo's.
var randomKeys = []string{
	"a", "b", "c", "x", "y", "key with spaces", "1", "-1", "1.5", "true", "null", "~", "''",
	`""`, `"a"`, "'b'", "2001-12-14", "!!str 1", "!!int 1", "0x1F", "yes", "a:b", "[a, b]",
	"{a: b}", "é", "-a", "?a", ".inf",
}

// bidderKeys are config.BidderInfo's yaml keys, at every level.
var bidderKeys = []string{
	"aliasOf", "whiteLabelOnly", "disabled", "endpoint", "extra_info", "openrtb", "maintainer",
	"capabilities", "modifyingVastXmlAllowed", "debug", "geoscope", "gvlVendorID", "userSync",
	"experiment", "xapi", "platform_id", "app_secret", "endpointCompression", "email", "app",
	"site", "dooh", "mediaTypes", "allow", "username", "password", "tracker", "version",
	"gpp-supported", "multiformat-supported", "key", "supports", "iframe", "redirect",
	"externalUrl", "formatOverride", "enabled", "skipwhen", "gdpr", "gpp_sid", "url",
	"redirectUrl", "userMacro", "adsCert",
}

// randomScalars are plain scalars besides sampleScalars: indicators inside and around words,
// and near-numbers, near-timestamps and near-booleans.
var randomScalars = []string{
	"a b", "a:b", "a,b", "a#b", "a #b", "-a", "?a", ":a", "::", "-", "--", "?", "%a", "a%",
	"a!", "!a", "a&", "a*", "a]", "a}", "a[b]", "a{b}", "a'b", `a"b`, `a\b`, "~~", "1:20",
	"12:30:00", "1-2", "0.1.2", "1e", "e1", ".", "..5", "+.inf", ".Inf", "+.INF", "-.NAN",
	"NaN", "inf", "+1", "+0", "-0", "00", "08", "09", "0o8", "0x", "0xG", "0b2", "1__0", "_1",
	"1_", "0_1", "0x_1", "0b_1", "0o_1", "-0x1F", "-0o17", "-0b101", "+0x1F", "0X1F", "0O17",
	"0B1", "1.5e+3", "1.5E-3", "+1.5", "-.5e2", "1.e2", "1e+400", "-1e400", "4.9e-324",
	"2001-12-14 21:59:43", "2001-12-14T21:59:43", "2001-12-14t21:59:43.123456789123Z",
	"2001-1-1", "2001-12-14T", "20011214", "2001-12-14 21:59", "2001-12-14 21:59:43 +01:00",
	"2001-12-14T21:59:43-01", "2001-12-14 21:59:43.5 Z", "2001-12-14\t21:59:43", "Yes", "YES",
	"yEs", "NO", "On", "oN", "Off", "Y", "N", "True", "TRUE", "tRUE", "False", "Null", "NULL",
	"nULL", "中文", "😀", "a\u00a0b", "http://x.com/a?b=c&d", "$UID", "{{.GDPR}}", "%%", "@a",
	"5s", "1h30m", "-1.5ms", "300ms", "2h45m0.5s", "1d", "a,b,c", "x,", ",",
}

// randomTags are the tags of random nodes.
var randomTags = []string{
	"!!str", "!!int", "!!float", "!!bool", "!!null", "!!timestamp", "!!map",
	"!!seq", "!foo", "!", "!<tag:yaml.org,2002:str>", "!e!x", "!!merge",
}

func newYAMLGen(seed int64) *yamlGen {
	return &yamlGen{rnd: rand.New(rand.NewSource(seed))}
}

func (g *yamlGen) chance(p float64) bool { return g.rnd.Float64() < p }

func (g *yamlGen) pick(list []string) string { return list[g.rnd.Intn(len(list))] }

// doc writes a random document, now and then followed by more.
func (g *yamlGen) doc() string {
	g.anchors, g.mapAnchors = nil, nil
	var b strings.Builder
	switch g.rnd.Intn(20) {
	case 0:
		b.WriteString("%YAML 1.1\n---\n")
	case 1:
		b.WriteString("%TAG !e! tag:example.com,2000:\n---\n")
	case 2, 3:
		b.WriteString("---\n")
	case 4:
		b.WriteString("--- # doc\n")
	}
	switch r := g.rnd.Intn(10); {
	case r < 6:
		g.mapping(&b, 0, 0)
	case r < 8:
		g.sequence(&b, 0, 0)
	case r < 9:
		b.WriteString(g.flow(0) + "\n")
	default:
		b.WriteString(g.props(false) + g.inline(0) + "\n")
	}
	switch g.rnd.Intn(20) {
	case 0:
		b.WriteString("...\n")
	case 1:
		b.WriteString("---\n")
	case 2:
		b.WriteString("--- " + g.scalar() + "\n")
	case 3:
		b.WriteString("---\n")
		g.mapping(&b, 0, 0)
	}
	s := b.String()
	switch g.rnd.Intn(40) {
	case 0:
		s = strings.ReplaceAll(s, "\n", "\r\n")
	case 1:
		s = strings.TrimSuffix(s, "\n")
	case 2:
		s = "\ufeff" + s
	}
	return s
}

// noise writes, now and then, a comment line or a blank line.
func (g *yamlGen) noise(b *strings.Builder, pad string) {
	switch g.rnd.Intn(40) {
	case 0:
		b.WriteString(pad + "# comment\n")
	case 1:
		b.WriteString("\n")
	case 2:
		b.WriteString(pad + "  \n")
	}
}

// step is the extra indentation of a nested block collection.
func (g *yamlGen) step() int {
	return []int{1, 2, 2, 2, 3, 4}[g.rnd.Intn(6)]
}

// mapping writes a block mapping at indentation indent.
func (g *yamlGen) mapping(b *strings.Builder, indent, depth int) {
	pad := strings.Repeat(" ", indent)
	for n := 1 + g.rnd.Intn(4); n > 0; n-- {
		g.noise(b, pad)
		lead := pad
		if g.chance(0.01) {
			lead += "\t"
		}
		switch r := g.rnd.Intn(40); {
		case r == 0 && depth < 4:
			b.WriteString(lead + "? " + g.inline(depth+1) + "\n" + pad + ":")
		case r <= 3:
			b.WriteString(lead + "<<:")
			g.merge(b, indent, depth)
			continue
		default:
			b.WriteString(lead + g.key() + ":")
		}
		g.value(b, indent, depth+1, false)
	}
}

// compact writes a block mapping whose first key follows a sequence item's dash.
func (g *yamlGen) compact(b *strings.Builder, indent, depth int) {
	pad := strings.Repeat(" ", indent)
	for i, n := 0, 1+g.rnd.Intn(3); i < n; i++ {
		if i > 0 {
			b.WriteString(pad)
		}
		b.WriteString(g.key() + ":")
		g.value(b, indent, depth+1, false)
	}
}

// sequence writes a block sequence at indentation indent.
func (g *yamlGen) sequence(b *strings.Builder, indent, depth int) {
	pad := strings.Repeat(" ", indent)
	for n := 1 + g.rnd.Intn(4); n > 0; n-- {
		g.noise(b, pad)
		b.WriteString(pad + "-")
		switch r := g.rnd.Intn(10); {
		case r < 2 && depth < 4:
			b.WriteString(" ")
			g.compact(b, indent+2, depth+1)
		case r < 3 && depth < 4:
			b.WriteString(" ")
			g.compactSequence(b, indent+2, depth+1)
		default:
			g.value(b, indent, depth+1, true)
		}
	}
}

// compactSequence writes a block sequence whose first item follows a sequence item's dash.
func (g *yamlGen) compactSequence(b *strings.Builder, indent, depth int) {
	pad := strings.Repeat(" ", indent)
	for i, n := 0, 1+g.rnd.Intn(3); i < n; i++ {
		if i > 0 {
			b.WriteString(pad)
		}
		b.WriteString("-")
		g.value(b, indent, depth+1, true)
	}
}

// value writes what follows a key's colon or an item's dash: an inline value and a line
// break, a line break and a block collection, or a block scalar.
func (g *yamlGen) value(b *strings.Builder, indent, depth int, item bool) {
	r := g.rnd.Intn(20)
	if depth >= 4 {
		r = 0
	}
	switch {
	case r < 10:
		b.WriteString(" " + g.props(false) + g.inline(depth) + g.comment() + "\n")
	case r < 11:
		b.WriteString(" " + g.pick(randomScalars) + "\n" + strings.Repeat(" ", indent+2) +
			g.pick(randomScalars) + "\n")
	case r < 15:
		b.WriteString(spaced(g.props(true)) + g.comment() + "\n")
		g.mapping(b, indent+g.step(), depth)
	case r < 18:
		b.WriteString(spaced(g.props(false)) + g.comment() + "\n")
		ind := indent + g.step()
		if !item && g.chance(0.3) {
			ind = indent
		}
		g.sequence(b, ind, depth)
	default:
		g.blockScalar(b, indent)
	}
}

// spaced is a space and props without its trailing space, or nothing.
func spaced(props string) string {
	if props == "" {
		return ""
	}
	return " " + strings.TrimSuffix(props, " ")
}

// props writes, now and then, an anchor (recorded for aliases) and a tag for the next node.
func (g *yamlGen) props(mapping bool) string {
	var p string
	if g.chance(0.12) {
		g.next++
		name := fmt.Sprintf("a%d", g.next)
		if len(g.anchors) > 0 && g.chance(0.1) {
			name = g.pick(g.anchors)
		}
		g.anchors = append(g.anchors, name)
		if mapping {
			g.mapAnchors = append(g.mapAnchors, name)
		}
		p = "&" + name + " "
	}
	if g.chance(0.05) {
		p += g.pick(randomTags) + " "
	}
	return p
}

// merge writes the value of a merge key.
func (g *yamlGen) merge(b *strings.Builder, indent, depth int) {
	alias := func() string {
		if len(g.mapAnchors) > 0 && !g.chance(0.1) {
			return "*" + g.pick(g.mapAnchors)
		}
		if len(g.anchors) > 0 {
			return "*" + g.pick(g.anchors)
		}
		return "{m: 1}"
	}
	switch g.rnd.Intn(8) {
	case 0, 1, 2:
		b.WriteString(" " + alias() + "\n")
	case 3:
		b.WriteString(" [" + alias() + ", " + alias() + "]\n")
	case 4:
		b.WriteString(" " + g.flowMapping(depth+1) + "\n")
	case 5:
		b.WriteString(" [" + g.flowMapping(depth+1) + ", " + alias() + "]\n")
	case 6:
		b.WriteString("\n")
		g.mapping(b, indent+2, depth+1)
	default:
		b.WriteString(" " + g.scalar() + "\n")
	}
}

// key writes a block mapping key.
func (g *yamlGen) key() string {
	switch r := g.rnd.Intn(20); {
	case r < 9:
		return g.pick(kitchenKeys)
	case r < 14:
		return g.pick(bidderKeys)
	case r < 18:
		return g.pick(randomKeys)
	case r < 19 && len(g.anchors) > 0:
		return "*" + g.pick(g.anchors) + " "
	default:
		return g.props(false) + g.pick(kitchenKeys)
	}
}

// inline writes a value that fits on its key's line.
func (g *yamlGen) inline(depth int) string {
	switch r := g.rnd.Intn(20); {
	case r < 2 && len(g.anchors) > 0:
		return "*" + g.pick(g.anchors)
	case r < 5 && depth < 5:
		return g.flow(depth + 1)
	case r < 8:
		return g.quoted()
	default:
		return g.scalar()
	}
}

// comment writes, now and then, a trailing comment.
func (g *yamlGen) comment() string {
	if g.chance(0.08) {
		return g.pick([]string{" # c", " #", "  # x: y", " #\t"})
	}
	return ""
}

// word is a scalar's text.
func (g *yamlGen) word() string {
	switch r := g.rnd.Intn(10); {
	case r < 4:
		return g.pick(sampleScalars)
	case r < 7:
		return g.pick(randomScalars)
	default:
		return g.pick(kitchenKeys)
	}
}

// scalar writes a plain scalar.
func (g *yamlGen) scalar() string {
	switch r := g.rnd.Intn(10); {
	case r < 5:
		for {
			if s := g.pick(sampleScalars); s != "*x" {
				return s
			}
		}
	case r < 8:
		return g.pick(randomScalars)
	case r < 9:
		v := g.rnd.Int63n(1 << uint(1+g.rnd.Intn(62)))
		if g.chance(0.5) {
			v = -v
		}
		return strconv.FormatInt(v, 10)
	default:
		v := g.rnd.NormFloat64() * math.Pow(10, float64(g.rnd.Intn(40)-20))
		return strconv.FormatFloat(v, g.pick([]string{"e", "E", "f", "g"})[0], -1, 64)
	}
}

var (
	singleQuote = strings.NewReplacer("'", "''")
	doubleQuote = strings.NewReplacer(`\`, `\\`, `"`, `\"`)
)

// quoted writes a single- or double-quoted scalar, now and then with an escape, a line break
// or a mistake.
func (g *yamlGen) quoted() string {
	if g.chance(0.4) {
		return "'" + singleQuote.Replace(g.word()) +
			g.pick([]string{"", "", "''", " x", "\n  y", "\n\n  z"}) + "'"
	}
	return `"` + doubleQuote.Replace(g.word()) + g.pick([]string{
		"", "", "", `\"`, `\\`, `\t`, `\n`, `\x41`, `\u00e9`, `\U0001F600`, `\0`, `\a`, `\e`,
		`\/`, `\N`, `\_`, `\L`, `\P`, `\ `, `\q`, `\x4`, `\u12`, "\\\n  y", " \n  more", `\'`,
		"\t", `\xc3\xa9`, `\ud83d`,
	}) + `"`
}

// flow writes a flow sequence or mapping.
func (g *yamlGen) flow(depth int) string {
	if g.chance(0.5) {
		return g.flowMapping(depth)
	}
	items := make([]string, g.rnd.Intn(4))
	for i := range items {
		items[i] = g.flowItem(depth)
	}
	return "[" + strings.Join(items, g.separator()) + g.pick([]string{"]", "]", "]", ",]", " ]"})
}

// flowMapping writes a flow mapping.
func (g *yamlGen) flowMapping(depth int) string {
	items := make([]string, g.rnd.Intn(4))
	for i := range items {
		switch g.rnd.Intn(10) {
		case 0:
			items[i] = g.flowKey()
		case 1:
			items[i] = "? " + g.flowItem(depth)
		default:
			items[i] = g.flowKey() + ":" + g.pick([]string{" ", " ", " ", ""}) + g.flowItem(depth)
		}
	}
	return "{" + strings.Join(items, g.separator()) + g.pick([]string{"}", "}", "}", ",}", " }"})
}

func (g *yamlGen) separator() string {
	return g.pick([]string{", ", ", ", ", ", ",", " , ", ",  "})
}

// flowKey writes a flow mapping key.
func (g *yamlGen) flowKey() string {
	switch r := g.rnd.Intn(10); {
	case r < 5:
		return g.pick(kitchenKeys)
	case r < 7:
		return g.pick(bidderKeys)
	case r < 8:
		return g.quoted()
	default:
		return g.pick(randomKeys)
	}
}

// flowItem writes a flow collection's item.
func (g *yamlGen) flowItem(depth int) string {
	if depth < 5 && g.chance(0.25) {
		return g.flow(depth + 1)
	}
	props := g.props(false)
	switch r := g.rnd.Intn(10); {
	case r < 1 && len(g.anchors) > 0:
		return "*" + g.pick(g.anchors)
	case r < 3:
		return props + g.quoted()
	default:
		return props + g.scalar()
	}
}

// blockScalar writes a literal or folded scalar after a key's colon or an item's dash.
func (g *yamlGen) blockScalar(b *strings.Builder, indent int) {
	header := g.pick([]string{"|", "|-", "|+", ">", ">-", ">+", "|1", "|2", ">2", "|-2", ">+1",
		"|0", "|10", "|2-"})
	b.WriteString(" " + g.props(false) + header + g.comment() + "\n")
	content := indent + 1 + g.rnd.Intn(3)
	for n := 1 + g.rnd.Intn(4); n > 0; n-- {
		switch g.rnd.Intn(10) {
		case 0:
			b.WriteString("\n")
		case 1:
			b.WriteString(strings.Repeat(" ", content+2) + g.word() + "\n")
		case 2:
			b.WriteString(strings.Repeat(" ", content) + "\t" + g.word() + "\n")
		default:
			b.WriteString(strings.Repeat(" ", content) + g.word() + " " + g.word() + "\n")
		}
	}
	if g.chance(0.3) {
		b.WriteString(strings.Repeat(" ", g.rnd.Intn(content+1)) + "\n")
	}
}
