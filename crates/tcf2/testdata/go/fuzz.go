package main

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"math/rand"
	"sort"
	"strconv"
	"strings"
)

// This file builds the review corpora (consent_fuzz_goldens.json and vendorlist_fuzz_goldens.json).
// They are generated programmatically from fixed seeds and aim at the shapes the hand-written
// corpora sample only sparsely: every base64 prefix of range-heavy strings, systematic sweeps
// over the bit alignment of the legitimate-interest header, random edits of section headers,
// mixed alphabets and segments, and grammar-generated vendor list documents that stay valid JSON
// while varying types, nulls, duplicate keys, key spelling and escapes.

// buildFuzzConsentCases returns the consent review corpus.
func buildFuzzConsentCases() []consentCase {
	b := &caseBuilder{seen: map[string]bool{}}
	r := rand.New(rand.NewSource(77001))

	addPrefixSweeps(b, r)
	addAlignmentSweeps(b)
	addHeaderEdits(b, r)
	addRangeShapes(b, r)
	addRestrictionShapes(b, r)
	addAlphabetAndSegments(b, r)
	addTCF1Sweeps(b, r)
	return b.cases
}

// rangeHeavyTCF2 is a random valid TCF2 spec whose sections favour range encodings.
func rangeHeavyTCF2(r *rand.Rand) *tcf2Spec {
	s := randomTCF2(r)
	section := func() vendorSection {
		if r.Intn(4) == 0 {
			max := uint64(r.Intn(70))
			return vendorSection{maxVendorID: max, bitfield: randomBits(r, max), numEntries: -1}
		}
		max := []uint64{0, 1, 2, 3, 17, 100, 65535, uint64(1 + r.Intn(3000))}[r.Intn(8)]
		return vendorSection{maxVendorID: max, isRange: true, entries: randomEntries(r, max, r.Intn(6)), numEntries: -1}
	}
	s.consents = section()
	s.legitInts = section()
	s.restrictions = randomRestrictions(r)
	return s
}

// addPrefixSweeps adds every base64 prefix (6-bit granularity, so the one-character remainder
// that base64 rejects is reached too) of range-heavy strings.
func addPrefixSweeps(b *caseBuilder, r *rand.Rand) {
	for i := 0; i < 24; i++ {
		s := enc(rangeHeavyTCF2(r).encode())
		if len(s) > 150 {
			s = s[:150]
		}
		for n := 1; n <= len(s); n++ {
			b.add(fmt.Sprintf("fuzz tcf2 b64 prefix %d/%d #%d", n, len(s), i), s[:n])
		}
	}
}

// addAlignmentSweeps truncates strings at every byte around the legitimate-interest header for
// each consent section length, so the header starts at every bit offset and the data ends at
// every byte before, inside and after it.
func addAlignmentSweeps(b *caseBuilder) {
	liVariants := []vendorSection{
		{maxVendorID: 0, numEntries: -1},
		{maxVendorID: 5, bitfield: []bool{true, false, true, true, false}, numEntries: -1},
		{maxVendorID: 9, isRange: true, entries: []rangeEntry{{true, 2, 4}, {false, 9, 9}}, numEntries: -1},
		{maxVendorID: 0, isRange: true, numEntries: -1},
	}
	for m := uint64(0); m <= 33; m++ {
		for li, liSection := range liVariants {
			s := baseTCF2()
			s.consents = vendorSection{maxVendorID: m, bitfield: everyThird(m), numEntries: -1}
			s.legitInts = liSection
			if li == 2 {
				s.restrictions = []restriction{{purpose: 3, rtype: 1, entries: []rangeEntry{{false, 4, 4}}, numEntries: -1}}
			}
			data := s.encode()
			header := (230 + int(m)) / 8
			for n := header - 2; n <= len(data)+1; n++ {
				if n < 0 {
					continue
				}
				cut := make([]byte, n)
				copy(cut, data)
				b.add(fmt.Sprintf("fuzz align bitfield max %d li %d len %d", m, li, n), enc(cut))
			}
		}
	}
	// Range-encoded consents with k entries of mixed kinds move the header through every offset.
	for k := 0; k <= 9; k++ {
		for mask := 0; mask < 1<<uint(minInt(k, 4)); mask++ {
			s := baseTCF2()
			var entries []rangeEntry
			for i := 0; i < k; i++ {
				id := uint64(2*i + 1)
				if mask&(1<<uint(i%4)) != 0 {
					entries = append(entries, rangeEntry{true, id, id + 1})
				} else {
					entries = append(entries, rangeEntry{false, id, id})
				}
			}
			s.consents = vendorSection{maxVendorID: 40, isRange: true, entries: entries, numEntries: -1}
			s.legitInts = vendorSection{maxVendorID: 2, bitfield: []bool{true, true}, numEntries: -1}
			data := s.encode()
			bits := 230 + 12
			for _, e := range entries {
				bits += 17
				if e.isRange {
					bits += 16
				}
			}
			for n := bits/8 - 1; n <= len(data)+1; n++ {
				cut := make([]byte, n)
				copy(cut, data)
				b.add(fmt.Sprintf("fuzz align range k %d mask %d len %d", k, mask, n), enc(cut))
			}
		}
	}
}

func minInt(a, b int) int {
	if a < b {
		return a
	}
	return b
}

// setBits overwrites n bits at bit offset off (most significant bit first), growing data.
func setBits(data []byte, off, n int, v uint64) []byte {
	for len(data)*8 < off+n {
		data = append(data, 0)
	}
	for i := 0; i < n; i++ {
		pos := off + i
		mask := byte(0x80 >> uint(pos%8))
		if (v>>uint(n-1-i))&1 == 1 {
			data[pos/8] |= mask
		} else {
			data[pos/8] &^= mask
		}
	}
	return data
}

// addHeaderEdits overwrites the section headers of valid strings with edge values.
func addHeaderEdits(b *caseBuilder, r *rand.Rand) {
	maxes := []uint64{0, 1, 2, 7, 8, 9, 255, 256, 4095, 4096, 32767, 32768, 65534, 65535}
	counts := []uint64{0, 1, 2, 3, 4095}
	for i := 0; i < 40; i++ {
		spec := rangeHeavyTCF2(r)
		data := spec.encode()
		// MaxVendorId of the consent section and its encoding bit.
		for _, max := range maxes {
			d := setBits(append([]byte(nil), data...), 213, 16, max)
			b.add(fmt.Sprintf("fuzz edit consent max %d #%d", max, i), enc(d))
		}
		for _, flag := range []uint64{0, 1} {
			d := setBits(append([]byte(nil), data...), 229, 1, flag)
			b.add(fmt.Sprintf("fuzz edit consent encoding %d #%d", flag, i), enc(d))
		}
		if spec.consents.isRange {
			for _, c := range counts {
				d := setBits(append([]byte(nil), data...), 230, 12, c)
				b.add(fmt.Sprintf("fuzz edit consent numEntries %d #%d", c, i), enc(d))
			}
		}
		// The legitimate-interest header follows the consent section.
		w := &bitWriter{}
		spec.consents.encode(w)
		liStart := 213 + len(w.bits)
		for _, max := range maxes {
			d := setBits(append([]byte(nil), data...), liStart, 16, max)
			b.add(fmt.Sprintf("fuzz edit li max %d #%d", max, i), enc(d))
		}
		for _, flag := range []uint64{0, 1} {
			d := setBits(append([]byte(nil), data...), liStart+16, 1, flag)
			b.add(fmt.Sprintf("fuzz edit li encoding %d #%d", flag, i), enc(d))
		}
		w2 := &bitWriter{}
		spec.legitInts.encode(w2)
		pubStart := liStart + len(w2.bits)
		for _, c := range counts {
			d := setBits(append([]byte(nil), data...), pubStart, 12, c)
			b.add(fmt.Sprintf("fuzz edit numRestrictions %d #%d", c, i), enc(d))
		}
		// Random bit flips within 40 bits of a section boundary.
		for f := 0; f < 6; f++ {
			d := append([]byte(nil), data...)
			anchor := []int{213, liStart, pubStart}[r.Intn(3)]
			pos := anchor - 8 + r.Intn(48)
			if pos/8 < len(d) {
				d[pos/8] ^= 0x80 >> uint(pos%8)
			}
			b.add(fmt.Sprintf("fuzz boundary flip #%d.%d", i, f), enc(d))
		}
	}
}

// addRangeShapes covers overlapping, unordered, adjacent and boundary range entries in both
// vendor sections, with probes of every vendor id.
func addRangeShapes(b *caseBuilder, r *rand.Rand) {
	shapes := [][]rangeEntry{
		{{true, 1, 65535}},
		{{true, 65534, 65535}, {false, 1, 1}},
		{{false, 65535, 65535}},
		{{true, 1, 2}, {true, 1, 2}, {true, 1, 2}},
		{{true, 10, 20}, {true, 15, 25}, {true, 5, 12}, {false, 30, 30}, {false, 30, 30}},
		{{true, 100, 200}, {true, 201, 300}, {false, 99, 99}, {false, 301, 301}},
		{{false, 7, 7}, {false, 6, 6}, {false, 5, 5}, {true, 1, 3}},
		{{true, 1, 65535}, {true, 2, 3}, {false, 4, 4}},
		{{true, 32767, 32769}, {false, 32768, 32768}},
	}
	for i, shape := range shapes {
		for _, max := range []uint64{65535, 300, 32768} {
			s := baseTCF2()
			s.consents = vendorSection{maxVendorID: max, isRange: true, entries: shape, numEntries: -1}
			s.legitInts = vendorSection{maxVendorID: max, isRange: true, entries: shape, numEntries: -1}
			b.add(fmt.Sprintf("fuzz range shape %d max %d", i, max), enc(s.encode()))
			s2 := baseTCF2()
			s2.legitInts = vendorSection{maxVendorID: max, isRange: true, entries: shape, numEntries: -1}
			b.add(fmt.Sprintf("fuzz li range shape %d max %d", i, max), enc(s2.encode()))
		}
	}
	// Random entry lists that may be invalid at a random position (zero, inverted, above max).
	for i := 0; i < 160; i++ {
		max := []uint64{1, 2, 3, 10, 255, 4096, 65535}[r.Intn(7)]
		entries := randomEntries(r, max, 1+r.Intn(8))
		if r.Intn(2) == 0 && len(entries) > 0 {
			bad := []rangeEntry{{false, 0, 0}, {true, 0, 1}, {true, 3, 3}, {true, 5, 4}, {false, max + 1, max + 1}, {true, 1, max + 1}, {true, max, max}}
			entries[r.Intn(len(entries))] = bad[r.Intn(len(bad))]
		}
		s := baseTCF2()
		if r.Intn(2) == 0 {
			s.consents = vendorSection{maxVendorID: max, isRange: true, entries: entries, numEntries: -1}
		} else {
			s.legitInts = vendorSection{maxVendorID: max, isRange: true, entries: entries, numEntries: -1}
		}
		if r.Intn(3) == 0 {
			ne := len(entries) + r.Intn(3) - 1
			if ne < 0 {
				ne = 0
			}
			if s.consents.isRange {
				s.consents.numEntries = ne
			} else {
				s.legitInts.numEntries = ne
			}
		}
		b.add(fmt.Sprintf("fuzz random entries #%d", i), enc(s.encode()))
	}
}

// addRestrictionShapes covers publisher restrictions with repeated keys, boundary vendors,
// invalid entries at random positions and truncation inside the section.
func addRestrictionShapes(b *caseBuilder, r *rand.Rand) {
	for i := 0; i < 160; i++ {
		s := baseTCF2()
		n := 1 + r.Intn(6)
		for j := 0; j < n; j++ {
			key := uint64(r.Intn(256))
			if r.Intn(3) == 0 && len(s.restrictions) > 0 {
				prev := s.restrictions[r.Intn(len(s.restrictions))]
				key = prev.purpose<<2 | prev.rtype
			}
			var entries []rangeEntry
			for k := r.Intn(5); k > 0; k-- {
				switch r.Intn(6) {
				case 0:
					entries = append(entries, rangeEntry{false, 32767, 32767})
				case 1:
					entries = append(entries, rangeEntry{true, 1, 32767})
				case 2:
					start := uint64(1 + r.Intn(32000))
					entries = append(entries, rangeEntry{true, start, start + 1 + uint64(r.Intn(700))})
				default:
					entries = append(entries, rangeEntry{false, uint64(1 + r.Intn(32767)), 0})
				}
			}
			for k := range entries {
				if !entries[k].isRange {
					entries[k].end = entries[k].start
				}
			}
			if r.Intn(8) == 0 && len(entries) > 0 {
				bad := []rangeEntry{{false, 0, 0}, {false, 32768, 32768}, {true, 2, 32768}, {true, 9, 9}, {true, 0, 3}, {false, 65535, 65535}}
				entries[r.Intn(len(entries))] = bad[r.Intn(len(bad))]
			}
			s.restrictions = append(s.restrictions, restriction{purpose: key >> 2, rtype: key & 3, entries: entries, numEntries: -1})
		}
		data := s.encode()
		b.add(fmt.Sprintf("fuzz restrictions #%d", i), enc(data))
		if i%4 == 0 {
			for cut := 34; cut < len(data); cut++ {
				b.add(fmt.Sprintf("fuzz restrictions #%d cut %d", i, cut), enc(data[:cut]))
			}
		}
	}
}

// addAlphabetAndSegments mixes in the standard alphabet, padding, whitespace and segments.
func addAlphabetAndSegments(b *caseBuilder, r *rand.Rand) {
	for i := 0; i < 60; i++ {
		data := rangeHeavyTCF2(r).encode()
		s := enc(data)
		std := base64.StdEncoding.EncodeToString(data)
		b.add(fmt.Sprintf("fuzz std padded #%d", i), std)
		b.add(fmt.Sprintf("fuzz std raw #%d", i), strings.TrimRight(std, "="))
		b.add(fmt.Sprintf("fuzz url padded #%d", i), base64.URLEncoding.EncodeToString(data))
		pos := r.Intn(len(s))
		for _, rep := range []string{"+", "/", "=", " ", "\n", "\r", "\t", "%2B", "-", "_", "A"} {
			b.add(fmt.Sprintf("fuzz replace %q at %d #%d", rep, pos, i), s[:pos]+rep+s[pos+1:])
		}
		segments := []string{"", ".", "..", ".A", ".AA", ".YAAAAAAAAAA", ".IFoEUQQgAIQwgIwQABAEAAAAOIAACAIAAAAAAAAAAA", ". ", ".==", "." + s}
		seg := segments[r.Intn(len(segments))] + segments[r.Intn(len(segments))]
		b.add(fmt.Sprintf("fuzz segments #%d", i), s+seg)
		b.add(fmt.Sprintf("fuzz segments newline #%d", i), s+"\n"+seg)
		// A dot inside the core segment cuts it short.
		cut := 1 + r.Intn(len(s)-1)
		b.add(fmt.Sprintf("fuzz early dot %d #%d", cut, i), s[:cut]+"."+s[cut:])
	}
}

// addTCF1Sweeps covers TCF 1.1 strings: every byte prefix of range and bitfield strings, and
// versions other than 1 and 2 that vendorconsent.ParseString sends to the TCF 1 parser.
func addTCF1Sweeps(b *caseBuilder, r *rand.Rand) {
	for i := 0; i < 16; i++ {
		s := randomTCF1(r)
		if i%2 == 0 {
			s.maxVendorID = uint64(1 + r.Intn(400))
			s.isRange, s.bitfield = true, nil
			s.defaultConsent = i%4 == 0
			s.entries = randomEntries(r, s.maxVendorID, 1+r.Intn(5))
		}
		data := s.encode()
		if len(data) > 60 {
			data = data[:60]
		}
		for n := 18; n <= len(data); n++ {
			b.add(fmt.Sprintf("fuzz tcf1 prefix %d/%d #%d", n, len(data), i), enc(data[:n]))
		}
	}
	for v := uint64(0); v < 64; v++ {
		s := baseTCF1()
		s.version = v
		b.add(fmt.Sprintf("fuzz tcf1 version %d", v), enc(s.encode()))
		s2 := baseTCF2()
		s2.version = v
		b.add(fmt.Sprintf("fuzz tcf2 layout version %d", v), enc(s2.encode()))
	}
	for i := 0; i < 40; i++ {
		s := randomTCF1(r)
		s.isRange = true
		s.bitfield = nil
		s.maxVendorID = []uint64{1, 2, 3, 65535, uint64(1 + r.Intn(500))}[r.Intn(5)]
		s.defaultConsent = r.Intn(2) == 0
		s.entries = randomEntries(r, s.maxVendorID, r.Intn(7))
		if r.Intn(3) == 0 && len(s.entries) > 0 {
			bad := []rangeEntry{{false, 0, 0}, {true, 0, 1}, {true, 4, 4}, {true, 5, 4}, {false, s.maxVendorID + 1, s.maxVendorID + 1}}
			s.entries[r.Intn(len(s.entries))] = bad[r.Intn(len(bad))]
		}
		b.add(fmt.Sprintf("fuzz tcf1 range #%d", i), enc(s.encode()))
	}
}

// fuzzContract mirrors go-gdpr's vendorListContract field for field (same types and tags), so
// encoding/json decodes a document into it exactly as ParseEagerly does.
type fuzzContract struct {
	GVLSpecificationVersion uint16                    `json:"gvlSpecificationVersion"`
	Version                 uint16                    `json:"vendorListVersion"`
	Vendors                 map[string]fuzzVendorWire `json:"vendors"`
}

type fuzzVendorWire struct {
	ID                  uint16  `json:"id"`
	Purposes            []uint8 `json:"purposes"`
	LegitimateInterests []uint8 `json:"legIntPurposes"`
	FlexiblePurposes    []uint8 `json:"flexiblePurposes"`
	SpecialPurposes     []uint8 `json:"specialPurposes"`
	SpecialFeatures     []uint8 `json:"specialFeatures"`
}

func idSet(ids []uint8) string {
	seen := map[uint8]bool{}
	for _, id := range ids {
		seen[id] = true
	}
	var out []int
	for id := range seen {
		out = append(out, int(id))
	}
	sort.Ints(out)
	return fmt.Sprint(out)
}

// deterministicVendorList reports whether ParseEagerly's result cannot depend on Go's random map
// iteration order: that is the case unless two map entries carry the same id with different
// contents.
func deterministicVendorList(doc []byte) bool {
	var c fuzzContract
	if err := json.Unmarshal(doc, &c); err != nil || c.Version == 0 {
		return true
	}
	byID := map[uint16]string{}
	for _, v := range c.Vendors {
		sig := idSet(v.Purposes) + idSet(v.LegitimateInterests) + idSet(v.FlexiblePurposes) + idSet(v.SpecialPurposes) + idSet(v.SpecialFeatures)
		if prev, ok := byID[v.ID]; ok && prev != sig {
			return false
		}
		byID[v.ID] = sig
	}
	return true
}

// jsonGen writes random vendor list documents that are valid JSON.
type jsonGen struct {
	r *rand.Rand
	// typeErrors is the per-value chance (1 in typeErrors) of a value of the wrong type; 0 never.
	typeErrors int
}

func (g *jsonGen) chance(n int) bool { return n > 0 && g.r.Intn(n) == 0 }

func (g *jsonGen) ws() string {
	return []string{"", "", "", " ", "\n", "\t", "\r\n  ", "  "}[g.r.Intn(8)]
}

// key spells a field name the ways encoding/json still matches (or nearly matches) it.
func (g *jsonGen) key(name string) string {
	var sb strings.Builder
	mode := g.r.Intn(10)
	for _, c := range name {
		switch {
		case mode == 0 && g.r.Intn(2) == 0:
			sb.WriteString(strings.ToUpper(string(c)))
		case mode == 1 && g.r.Intn(3) == 0:
			fmt.Fprintf(&sb, `\u%04x`, c)
		case mode == 2 && (c == 's' || c == 'S') && g.r.Intn(2) == 0:
			sb.WriteString("ſ")
		case mode == 3 && (c == 'k' || c == 'K'):
			sb.WriteString("K")
		case mode == 4 && c == 'i' && g.r.Intn(2) == 0:
			sb.WriteString("ı")
		default:
			sb.WriteRune(c)
		}
	}
	if mode == 5 && g.r.Intn(3) == 0 {
		sb.WriteString("x")
	}
	// The names are plain ASCII, so the only escapes are the ones written above.
	return `"` + sb.String() + `"`
}

func (g *jsonGen) wrongValue() string {
	return []string{`"7"`, `true`, `false`, `[1]`, `{"a":1}`, `-1`, `1.5`, `2e1`, `65536`, `256`, `-0`, `1E0`, `123456789012345678901234567890`, `""`, `[]`, `{}`}[g.r.Intn(16)]
}

func (g *jsonGen) uintValue(limit int) string {
	if g.chance(g.typeErrors) {
		return g.wrongValue()
	}
	if g.r.Intn(12) == 0 {
		return "null"
	}
	return strconv.Itoa(g.r.Intn(limit))
}

func (g *jsonGen) byteArray() string {
	var parts []string
	for n := g.r.Intn(9); n > 0; n-- {
		var v string
		switch {
		case g.chance(g.typeErrors):
			v = g.wrongValue()
		case g.r.Intn(7) == 0:
			v = "null"
		case g.r.Intn(9) == 0:
			v = strconv.Itoa(200 + g.r.Intn(56))
		default:
			v = strconv.Itoa(g.r.Intn(26))
		}
		parts = append(parts, g.ws()+v+g.ws())
	}
	return "[" + strings.Join(parts, ",") + "]"
}

func (g *jsonGen) bytesValue() string {
	switch g.r.Intn(12) {
	case 0:
		return "null"
	case 1, 2:
		raw := make([]byte, g.r.Intn(7))
		for i := range raw {
			raw[i] = byte(g.r.Intn(30))
		}
		s := base64.StdEncoding.EncodeToString(raw)
		switch g.r.Intn(5) {
		case 0:
			if len(s) > 2 {
				s = s[:2] + `\n` + s[2:]
			}
		case 1:
			s = strings.ReplaceAll(s, "=", `=`)
		case 2:
			if g.chance(g.typeErrors) {
				s = strings.TrimRight(s, "=") + "!"
			}
		}
		return `"` + s + `"`
	case 3:
		if g.chance(g.typeErrors) {
			return g.wrongValue()
		}
		return "[]"
	default:
		return g.byteArray()
	}
}

func (g *jsonGen) anyValue(depth int) string {
	if depth > 3 {
		return `"leaf"`
	}
	switch g.r.Intn(8) {
	case 0:
		return "null"
	case 1:
		return `"str \"q\" \\ \/ é é 😀"`
	case 2:
		return []string{"0", "-12.5e+3", "1E-7", "123456789012345678901234567890"}[g.r.Intn(4)]
	case 3:
		return []string{"true", "false"}[g.r.Intn(2)]
	case 4:
		var parts []string
		for n := g.r.Intn(4); n > 0; n-- {
			parts = append(parts, g.anyValue(depth+1))
		}
		return "[" + strings.Join(parts, ",") + "]"
	default:
		var parts []string
		for n := g.r.Intn(4); n > 0; n-- {
			parts = append(parts, strconv.Quote(fmt.Sprintf("k%d}{]", g.r.Intn(5)))+":"+g.anyValue(depth+1))
		}
		return "{" + strings.Join(parts, ",") + "}"
	}
}

var vendorFieldNames = []string{"id", "purposes", "legIntPurposes", "flexiblePurposes", "specialPurposes", "specialFeatures"}

func (g *jsonGen) vendor(id int) string {
	if g.r.Intn(25) == 0 {
		return "null"
	}
	if g.chance(g.typeErrors * 3) {
		return g.wrongValue()
	}
	var members []string
	members = append(members, g.key("id")+":"+g.ws()+strconv.Itoa(id))
	for _, name := range vendorFieldNames[1:] {
		for n := []int{0, 1, 1, 1, 1, 2, 3}[g.r.Intn(7)]; n > 0; n-- {
			members = append(members, g.key(name)+":"+g.ws()+g.bytesValue())
		}
	}
	if g.r.Intn(5) == 0 {
		members = append(members, g.key("id")+":"+g.uintValue(70))
	}
	for n := g.r.Intn(3); n > 0; n-- {
		members = append(members, strconv.Quote([]string{"name", "features", "urls", "deletedDate", "dataRetention"}[g.r.Intn(5)])+":"+g.anyValue(0))
	}
	g.r.Shuffle(len(members), func(i, j int) { members[i], members[j] = members[j], members[i] })
	return "{" + g.ws() + strings.Join(members, ","+g.ws()) + g.ws() + "}"
}

func (g *jsonGen) vendors() string {
	if g.r.Intn(20) == 0 {
		return "null"
	}
	if g.chance(g.typeErrors * 3) {
		return g.wrongValue()
	}
	var members []string
	for n := g.r.Intn(9); n > 0; n-- {
		id := 1 + g.r.Intn(40)
		var key string
		switch g.r.Intn(8) {
		case 0:
			key = fmt.Sprintf(`"\u00%02x"`, 0x30+id%10)
		case 1:
			key = `"v` + strconv.Itoa(id) + `"`
		default:
			key = `"` + strconv.Itoa(id) + `"`
		}
		members = append(members, key+":"+g.ws()+g.vendor(id))
	}
	return "{" + strings.Join(members, ",") + "}"
}

func (g *jsonGen) document() string {
	var members []string
	if g.r.Intn(8) != 0 {
		members = append(members, g.key("gvlSpecificationVersion")+":"+g.uintValue(5))
	}
	for n := []int{0, 1, 1, 1, 1, 2}[g.r.Intn(6)]; n > 0; n-- {
		members = append(members, g.key("vendorListVersion")+":"+g.uintValue(400))
	}
	if g.r.Intn(5) != 0 {
		// Most documents carry a usable version, so that decoding gets to the vendors.
		members = append(members, `"vendorListVersion":`+strconv.Itoa(1+g.r.Intn(400)))
	}
	for n := []int{0, 1, 1, 1, 1, 1, 2}[g.r.Intn(7)]; n > 0; n-- {
		members = append(members, g.key("vendors")+":"+g.ws()+g.vendors())
	}
	for n := g.r.Intn(3); n > 0; n-- {
		members = append(members, strconv.Quote([]string{"tcfPolicyVersion", "lastUpdated", "purposes", "stacks"}[g.r.Intn(4)])+":"+g.anyValue(0))
	}
	g.r.Shuffle(len(members), func(i, j int) { members[i], members[j] = members[j], members[i] })
	return g.ws() + "{" + strings.Join(members, ","+g.ws()) + "}" + g.ws()
}

// buildFuzzVendorListCases returns the vendor list review corpus.
func buildFuzzVendorListCases() []vendorListCase {
	var out []vendorListCase
	// add records the first document from next whose result does not depend on map order.
	add := func(name string, next func() string) {
		for attempt := 0; attempt < 50; attempt++ {
			if doc := next(); deterministicVendorList([]byte(doc)) {
				out = append(out, newVendorListCase(name, []byte(doc)))
				return
			}
		}
		fail(fmt.Errorf("no deterministic document for %s", name))
	}
	r := rand.New(rand.NewSource(77002))
	clean := &jsonGen{r: r}
	dirty := &jsonGen{r: r, typeErrors: 25}
	for i := 0; i < 900; i++ {
		add(fmt.Sprintf("fuzz clean doc #%d", i), clean.document)
	}
	for i := 0; i < 700; i++ {
		add(fmt.Sprintf("fuzz dirty doc #%d", i), dirty.document)
	}
	// Single-byte edits of clean documents reach scanner states inside realistic structure.
	edits := []string{"", "{", "}", "[", "]", ",", ":", "\"", "\\", "0", "-", ".", "e", "n", "t", " ", "\x00", "\x7f", "\xc3", "\xff"}
	edited := func() string {
		doc := clean.document()
		pos := r.Intn(len(doc) + 1)
		ins := edits[r.Intn(len(edits))]
		switch r.Intn(3) {
		case 0:
			return doc[:pos] + ins + doc[pos:]
		case 1:
			if pos < len(doc) {
				return doc[:pos] + doc[pos+1:]
			}
		default:
			if pos < len(doc) {
				return doc[:pos] + ins + doc[pos+1:]
			}
		}
		return doc
	}
	for i := 0; i < 500; i++ {
		add(fmt.Sprintf("fuzz edited doc #%d", i), edited)
	}
	return out
}
