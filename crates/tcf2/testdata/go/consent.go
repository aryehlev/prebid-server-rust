package main

import (
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"math/rand"
	"reflect"
	"sort"
	"strconv"
	"strings"

	"github.com/prebid/go-gdpr/api"
	"github.com/prebid/go-gdpr/consentconstants"
	"github.com/prebid/go-gdpr/vendorconsent"
	tcf2 "github.com/prebid/go-gdpr/vendorconsent/tcf2"
)

// consentCase records every observable result for one input string.
type consentCase struct {
	Label    string  `json:"label"`
	InputHex string  `json:"input_hex,omitempty"`
	Input    *string `json:"input,omitempty"`
	// VendorConsent is vendorconsent.ParseString(input), used by gdpr/consent_parser.go,
	// exchange/utils.go, analytics/agma and privacy/gdpr/validate.go.
	VendorConsent *parseResult `json:"vendorconsent"`
	// TCF2 is tcf2.ParseString(input) (amp/parse.go). It is only recorded when the input does not
	// start with 'C', since otherwise vendorconsent.ParseString already is tcf2.ParseString.
	TCF2 *parseResult `json:"tcf2,omitempty"`
	// RawURLDecode is base64.RawURLEncoding.DecodeString(input) (adapters/flipp).
	RawURLDecode *bytesResult `json:"raw_url_decode"`
	// TCF1Parse is vendorconsent.Parse(decoded), the TCF1 parser flipp applies to the decoded bytes.
	TCF1Parse *parseResult `json:"tcf1_parse,omitempty"`
}

type bytesResult struct {
	Err string `json:"err,omitempty"`
	Hex string `json:"hex,omitempty"`
	Len int    `json:"len"`
}

type parseResult struct {
	Err   string `json:"err,omitempty"`
	Panic string `json:"panic,omitempty"`
	// Kind is "tcf1-bitfield", "tcf1-range" or "tcf2" (informational).
	Kind string `json:"kind,omitempty"`
	Meta *meta  `json:"meta,omitempty"`
}

type meta struct {
	Version           uint8    `json:"version"`
	Created           [2]int64 `json:"created"`
	LastUpdated       [2]int64 `json:"last_updated"`
	CmpID             uint16   `json:"cmp_id"`
	CmpVersion        uint16   `json:"cmp_version"`
	ConsentScreen     uint8    `json:"consent_screen"`
	ConsentLanguage   string   `json:"consent_language_hex"`
	VendorListVersion uint16   `json:"vendor_list_version"`
	TCFPolicyVersion  uint8    `json:"tcf_policy_version"`
	MaxVendorID       uint16   `json:"max_vendor_id"`
	// PurposeAllowed holds '0'/'1' for purpose ids 0.. up to (excluding) PurposePanicFrom, or up to
	// 31 for TCF2 (ids above 24 are always false there).
	PurposeAllowed   string `json:"purpose_allowed"`
	PurposePanicFrom int    `json:"purpose_panic_from"`
	// VendorConsent lists the vendor ids in [0, 65535] with VendorConsent(id) == true.
	VendorConsent string `json:"vendor_consent"`

	// TCF2 only.
	PurposeLI           string            `json:"purpose_li,omitempty"`
	PurposeOneTreatment *bool             `json:"purpose_one_treatment,omitempty"`
	SpecialFeatures     string            `json:"special_feature_opt_in,omitempty"`
	VendorLIMaxID       *uint16           `json:"vendor_li_max_id,omitempty"`
	VendorLI            *string           `json:"vendor_li,omitempty"`
	ConsentEncoding     string            `json:"consent_encoding,omitempty"`
	LIEncoding          string            `json:"li_encoding,omitempty"`
	PubDump             []pubDump         `json:"pub_dump,omitempty"`
	PubSamples          []int             `json:"pub_samples,omitempty"`
	PubChecks           map[string]string `json:"pub_checks,omitempty"`
	PubWrap             string            `json:"pub_wrap,omitempty"`
}

type pubDump struct {
	Key     uint64      `json:"key"`
	Purpose uint64      `json:"purpose"`
	Type    uint64      `json:"type"`
	Vendors [][2]uint64 `json:"vendors"`
}

// The wrap-around probes exercise CheckPubRestriction's uint8 arithmetic for purpose ids >= 64
// and restriction types >= 4. The Rust test uses the same lists.
var (
	wrapPurposes = []uint8{64, 65, 66, 100, 128, 129, 200, 255}
	wrapTypes    = []uint8{0, 1, 2, 3, 4, 5, 7, 255}
)

const wrapVendorSamples = 4

// defaultPubSamples are probed for every consent; restriction range endpoints are added to them.
var defaultPubSamples = []int{0, 1, 2, 3, 32766, 32767, 32768, 65535}

func newCase(label, input string) consentCase {
	c := consentCase{Label: label}
	c.Input, c.InputHex = encodeInput([]byte(input))
	c.VendorConsent = record(func() (api.VendorConsents, error) { return vendorconsent.ParseString(input) })
	if !tcf2.IsConsentV2(input) {
		c.TCF2 = record(func() (api.VendorConsents, error) { return tcf2.ParseString(input) })
	}
	decoded, err := base64.RawURLEncoding.DecodeString(input)
	// base64_goldens.json covers the decoded bytes; here only the outcome matters.
	c.RawURLDecode = &bytesResult{}
	if err != nil {
		c.RawURLDecode.Err = err.Error()
	} else {
		c.RawURLDecode.Len = len(decoded)
		c.TCF1Parse = record(func() (api.VendorConsents, error) { return vendorconsent.Parse(decoded) })
	}
	return c
}

func record(parse func() (api.VendorConsents, error)) *parseResult {
	var vc api.VendorConsents
	var err error
	if msg, panicked := catch(func() { vc, err = parse() }); panicked {
		return &parseResult{Panic: msg}
	}
	if err != nil {
		return &parseResult{Err: err.Error()}
	}
	res := &parseResult{Meta: commonMeta(vc)}
	switch t := fmt.Sprintf("%T", vc); t {
	case "*vendorconsent.consentBitField":
		res.Kind = "tcf1-bitfield"
	case "*vendorconsent.rangeSection":
		res.Kind = "tcf1-range"
	case "vendorconsent.ConsentMetadata":
		res.Kind = "tcf2"
		addTCF2Meta(res.Meta, vc.(tcf2.ConsentMetadata))
	default:
		fail(fmt.Errorf("unexpected consent type %s", t))
	}
	return res
}

func commonMeta(vc api.VendorConsents) *meta {
	created := vc.Created()
	updated := vc.LastUpdated()
	m := &meta{
		Version:           vc.Version(),
		Created:           [2]int64{created.Unix(), int64(created.Nanosecond())},
		LastUpdated:       [2]int64{updated.Unix(), int64(updated.Nanosecond())},
		CmpID:             vc.CmpID(),
		CmpVersion:        vc.CmpVersion(),
		ConsentScreen:     vc.ConsentScreen(),
		ConsentLanguage:   hex.EncodeToString([]byte(vc.ConsentLanguage())),
		VendorListVersion: vc.VendorListVersion(),
		TCFPolicyVersion:  vc.TCFPolicyVersion(),
		MaxVendorID:       vc.MaxVendorID(),
		PurposePanicFrom:  256,
	}
	var purposes strings.Builder
	limit := 256
	if _, ok := vc.(tcf2.ConsentMetadata); ok {
		limit = 32
	}
	for id := 0; id < limit; id++ {
		var allowed bool
		if _, panicked := catch(func() { allowed = vc.PurposeAllowed(consentconstants.Purpose(id)) }); panicked {
			m.PurposePanicFrom = id
			break
		}
		purposes.WriteByte(bit(allowed))
	}
	m.PurposeAllowed = purposes.String()
	m.VendorConsent = idRanges(vc.VendorConsent)
	return m
}

func addTCF2Meta(m *meta, cm tcf2.ConsentMetadata) {
	var li, sf strings.Builder
	for id := 0; id < 32; id++ {
		li.WriteByte(bit(cm.PurposeLITransparency(consentconstants.Purpose(id))))
	}
	for id := uint16(0); id < 16; id++ {
		sf.WriteByte(bit(cm.SpecialFeatureOptIn(id)))
	}
	m.PurposeLI = li.String()
	m.SpecialFeatures = sf.String()
	p1 := cm.PurposeOneTreatment()
	m.PurposeOneTreatment = &p1
	liMax := cm.VendorLegitInterestMaxID()
	m.VendorLIMaxID = &liMax
	vli := idRanges(cm.VendorLegitInterest)
	m.VendorLI = &vli

	rv := reflect.ValueOf(cm)
	m.ConsentEncoding = encodingName(rv.FieldByName("vendorConsents"))
	m.LIEncoding = encodingName(rv.FieldByName("vendorLegitimateInterests"))
	m.PubDump = pubInternals(rv.FieldByName("publisherRestrictions"))
	if len(m.PubDump) == 0 {
		// Without restrictions every probe is false; the Rust test checks that against
		// defaultPubSamples without the golden spelling it out.
		for key := 0; key < 256; key++ {
			for _, v := range defaultPubSamples {
				if cm.CheckPubRestriction(uint8(key>>2), uint8(key&3), uint16(v)) {
					fail(fmt.Errorf("restriction without dump for key %d", key))
				}
			}
		}
		m.PubDump = nil
		return
	}

	samples := map[int]bool{}
	for _, v := range defaultPubSamples {
		samples[v] = true
	}
	for _, d := range m.PubDump {
		for _, v := range d.Vendors {
			for _, s := range []int{int(v[0]) - 1, int(v[0]), int(v[0]) + 1, int(v[1]) - 1, int(v[1]), int(v[1]) + 1} {
				if s >= 0 && s <= 65535 {
					samples[s] = true
				}
			}
		}
	}
	for s := range samples {
		m.PubSamples = append(m.PubSamples, s)
	}
	sort.Ints(m.PubSamples)

	m.PubChecks = map[string]string{}
	for key := 0; key < 256; key++ {
		purpose, rtype := uint8(key>>2), uint8(key&3)
		var b strings.Builder
		anySet := false
		for _, v := range m.PubSamples {
			ok := cm.CheckPubRestriction(purpose, rtype, uint16(v))
			anySet = anySet || ok
			b.WriteByte(bit(ok))
		}
		if anySet {
			m.PubChecks[strconv.Itoa(key)] = b.String()
		}
	}
	if len(m.PubChecks) == 0 {
		m.PubChecks = nil
	}

	var wrap strings.Builder
	for _, p := range wrapPurposes {
		for _, t := range wrapTypes {
			for i, v := range m.PubSamples {
				if i >= wrapVendorSamples {
					break
				}
				wrap.WriteByte(bit(cm.CheckPubRestriction(p, t, uint16(v))))
			}
		}
	}
	m.PubWrap = wrap.String()
}

func encodingName(v reflect.Value) string {
	if v.IsNil() {
		return "nil"
	}
	switch t := v.Elem().Type().String(); t {
	case "*vendorconsent.consentBitField":
		return "bitfield"
	case "*vendorconsent.rangeSection":
		return "range"
	default:
		return t
	}
}

func pubInternals(v reflect.Value) []pubDump {
	if v.IsNil() {
		return nil
	}
	restrictions := v.Elem().Elem().FieldByName("restrictions")
	keys := restrictions.MapKeys()
	sort.Slice(keys, func(i, j int) bool { return keys[i].Uint() < keys[j].Uint() })
	out := []pubDump{}
	for _, k := range keys {
		r := restrictions.MapIndex(k)
		d := pubDump{
			Key:     k.Uint(),
			Purpose: r.FieldByName("purposeID").Uint(),
			Type:    r.FieldByName("restrictType").Uint(),
			Vendors: [][2]uint64{},
		}
		vendors := r.FieldByName("vendors")
		for i := 0; i < vendors.Len(); i++ {
			e := vendors.Index(i)
			d.Vendors = append(d.Vendors, [2]uint64{e.FieldByName("startID").Uint(), e.FieldByName("endID").Uint()})
		}
		out = append(out, d)
	}
	return out
}

func bit(b bool) byte {
	if b {
		return '1'
	}
	return '0'
}

// idRanges renders the ids in [0, 65535] for which f is true as "a-b,c,...".
func idRanges(f func(uint16) bool) string {
	var b strings.Builder
	start := -1
	flush := func(end int) {
		if start < 0 {
			return
		}
		if b.Len() > 0 {
			b.WriteByte(',')
		}
		if start == end {
			b.WriteString(strconv.Itoa(start))
		} else {
			b.WriteString(strconv.Itoa(start) + "-" + strconv.Itoa(end))
		}
		start = -1
	}
	for id := 0; id <= 65535; id++ {
		var ok bool
		if msg, panicked := catch(func() { ok = f(uint16(id)) }); panicked {
			flush(id - 1)
			if b.Len() > 0 {
				b.WriteByte(',')
			}
			b.WriteString("panic@" + strconv.Itoa(id) + ":" + msg)
			continue
		}
		if ok {
			if start < 0 {
				start = id
			}
		} else {
			flush(id - 1)
		}
	}
	flush(65535)
	return b.String()
}

func enc(data []byte) string {
	return base64.RawURLEncoding.EncodeToString(data)
}

type caseBuilder struct {
	cases []consentCase
	seen  map[string]bool
}

func (b *caseBuilder) add(label, input string) {
	if b.seen[input] {
		return
	}
	b.seen[input] = true
	b.cases = append(b.cases, newCase(label, input))
}

func buildConsentCases(gogdpr, repo []string) []consentCase {
	b := &caseBuilder{seen: map[string]bool{}}
	for _, s := range gogdpr {
		b.add("go-gdpr test vector", s)
	}
	// The go-gdpr tests also parse these after stripping the bytes.
	b.add("go-gdpr TestParseInvalidString", "/")
	b.add("go-gdpr TestParseEmptyString", "")
	for _, s := range repo {
		b.add("prebid-server repository string", s)
	}

	addSpecialStrings(b)
	addTCF2EdgeCases(b)
	addTCF1EdgeCases(b)

	r := rand.New(rand.NewSource(20240611))
	var valid2, valid1 [][]byte
	for i := 0; i < 520; i++ {
		data := randomTCF2(r).encode()
		valid2 = append(valid2, data)
		s := enc(data)
		if r.Intn(8) == 0 {
			s += []string{".", ".AAAA", "..", ".!not base64!", ".IF5EX2S5OI2tho2YdF7BEYY", ".YAAAAAAAAAA"}[r.Intn(6)]
		}
		b.add(fmt.Sprintf("random tcf2 #%d", i), s)
	}
	for i := 0; i < 250; i++ {
		data := randomTCF1(r).encode()
		valid1 = append(valid1, data)
		b.add(fmt.Sprintf("random tcf1 #%d", i), enc(data))
	}

	// Every byte-prefix of a selection of valid strings.
	for i := 0; i < 9; i++ {
		data := valid2[i*37%len(valid2)]
		if len(data) > 260 {
			data = data[:260]
		}
		for n := 0; n <= len(data); n++ {
			b.add(fmt.Sprintf("tcf2 truncation %d/%d", n, len(data)), enc(data[:n]))
		}
	}
	for i := 0; i < 4; i++ {
		data := valid1[i*29%len(valid1)]
		if len(data) > 120 {
			data = data[:120]
		}
		for n := 0; n <= len(data); n++ {
			b.add(fmt.Sprintf("tcf1 truncation %d/%d", n, len(data)), enc(data[:n]))
		}
	}

	// Random bit flips.
	for i := 0; i < 220; i++ {
		src := valid2
		if i%4 == 3 {
			src = valid1
		}
		data := append([]byte(nil), src[r.Intn(len(src))]...)
		for f := 1 + r.Intn(3); f > 0; f-- {
			pos := r.Intn(len(data) * 8)
			data[pos/8] ^= 0x80 >> uint(pos%8)
		}
		b.add(fmt.Sprintf("bit flip #%d", i), enc(data))
	}

	// Base64-level corruption of valid strings.
	inserts := []string{"=", "==", "+", "/", " ", "\n", "\r", "\r\n", "\t", "\x00", "\xff", "\xc3\xa9", "%3D", ".", "A", "_", "-"}
	for i := 0; i < 24; i++ {
		var s string
		if i%3 == 2 {
			s = enc(valid1[i%len(valid1)])
		} else {
			s = enc(valid2[(i*13)%len(valid2)])
		}
		if len(s) > 200 {
			s = s[:200]
		}
		positions := []int{0, 1, 2, 3, 4, 5, len(s) / 2, len(s) - 2, len(s) - 1, len(s)}
		for _, p := range positions {
			if p < 0 || p > len(s) {
				continue
			}
			ins := inserts[r.Intn(len(inserts))]
			b.add("base64 insert", s[:p]+ins+s[p:])
			if p < len(s) {
				b.add("base64 delete", s[:p]+s[p+1:])
			}
		}
		for _, ins := range inserts {
			p := r.Intn(len(s) + 1)
			b.add("base64 insert", s[:p]+ins+s[p:])
		}
	}

	// Random garbage strings.
	alphabet := "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
	for i := 0; i < 220; i++ {
		n := 1 + r.Intn(70)
		var sb strings.Builder
		for j := 0; j < n; j++ {
			if r.Intn(40) == 0 {
				sb.WriteString(inserts[r.Intn(len(inserts))])
			} else {
				sb.WriteByte(alphabet[r.Intn(len(alphabet))])
			}
		}
		s := sb.String()
		if i%2 == 0 {
			s = "C" + s
		}
		b.add("random garbage", s)
	}
	return b.cases
}

func addSpecialStrings(b *caseBuilder) {
	for _, s := range []string{
		"C", "C.", ".", "..", ".C", "CC", "CCC", "CCCC", "CCCCC", "B", "BB", "BBB", "BBBB",
		"\n", "\r", "C\n", "\nC", "\r\n", "=", "C=", "C==", "CA==", "CAAA=", "A", "AA", "AAA", "AAAA",
		"ab", "C\x00", "\x00", "\xff", "C\xff", "Cé", "é", "   ", " C", "C ",
		"CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "BAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
		"null", "undefined", "0", "1", "true", "false", "{}", "[]", "consent",
		"COzSDo9OzSDo9B9AAAENAiCAALAAAAAAAAAACOQAQCOAAAAA\n", "\nCOzSDo9OzSDo9B9AAAENAiCAALAAAAAAAAAACOQAQCOAAAAA",
		"COzSDo9OzSDo9B9AAAENAiCAALAAAAAAAAAACOQAQCOAAAAA\r\n.x",
		"COzSDo9O\nzSDo9B9AAAEN\rAiCAALAAAAAAAAAACOQAQCOAAAAA",
	} {
		b.add("special string", s)
	}
}

func addTCF2EdgeCases(b *caseBuilder) {
	add := func(label string, mutate func(s *tcf2Spec)) {
		s := baseTCF2()
		mutate(s)
		b.add("tcf2 edge: "+label, enc(s.encode()))
	}
	for _, v := range []uint64{0, 1, 2, 3, 4, 31, 32, 33, 63} {
		v := v
		add(fmt.Sprintf("version %d", v), func(s *tcf2Spec) { s.version = v })
	}
	for _, v := range []uint64{0, 1, 4095} {
		v := v
		add(fmt.Sprintf("vendor list version %d", v), func(s *tcf2Spec) { s.vlv = v })
	}
	for p := uint64(0); p < 64; p++ {
		p := p
		add(fmt.Sprintf("policy version %d", p), func(s *tcf2Spec) { s.policy = p })
	}
	for _, l := range []uint64{0, 25, 26, 31, 32, 57, 58, 62, 63, 64*25 + 25, 64*63 + 63, 64*26 + 0, 64*0 + 63} {
		l := l
		add(fmt.Sprintf("language %d", l), func(s *tcf2Spec) { s.lang = l })
	}
	add("all purposes", func(s *tcf2Spec) { s.purposes = 1<<24 - 1; s.purposesLI = 1<<24 - 1; s.specialFeatures = 1<<12 - 1 })
	add("max fields", func(s *tcf2Spec) {
		s.created, s.updated = 1<<36-1, 1<<36-1
		s.cmpID, s.cmpVersion, s.screen = 4095, 4095, 63
		s.p1Treatment, s.serviceSpecific, s.nonStd = true, true, true
		s.publisherCC = 4095
	})
	add("zero timestamps", func(s *tcf2Spec) { s.created, s.updated = 0, 0 })

	// Vendor consent section variants.
	for _, max := range append(append([]uint64{}, interestingMaxIDs...), 2000, 4096, 8191) {
		max := max
		add(fmt.Sprintf("bitfield consents max %d", max), func(s *tcf2Spec) {
			s.consents = vendorSection{maxVendorID: max, bitfield: everyThird(max), numEntries: -1}
		})
		add(fmt.Sprintf("bitfield li max %d", max), func(s *tcf2Spec) {
			s.legitInts = vendorSection{maxVendorID: max, bitfield: everyThird(max), numEntries: -1}
		})
	}
	add("bitfield consents max 65535 all set", func(s *tcf2Spec) {
		bits := make([]bool, 65535)
		for i := range bits {
			bits[i] = true
		}
		s.consents = vendorSection{maxVendorID: 65535, bitfield: bits, numEntries: -1}
	})
	for _, max := range largeMaxIDs {
		max := max
		add(fmt.Sprintf("range consents max %d full", max), func(s *tcf2Spec) {
			s.consents = vendorSection{maxVendorID: max, isRange: true, entries: []rangeEntry{{true, 1, max}}, numEntries: -1}
		})
		add(fmt.Sprintf("range li max %d last", max), func(s *tcf2Spec) {
			s.legitInts = vendorSection{maxVendorID: max, isRange: true, entries: []rangeEntry{{false, max, max}, {true, 1, 2}}, numEntries: -1}
		})
	}
	add("range consents 4095 entries", func(s *tcf2Spec) {
		var entries []rangeEntry
		for i := uint64(1); i <= 4095; i++ {
			entries = append(entries, rangeEntry{start: i * 2, end: i * 2})
		}
		s.consents = vendorSection{maxVendorID: 9000, isRange: true, entries: entries, numEntries: -1}
	})
	add("range consents zero entries", func(s *tcf2Spec) {
		s.consents = vendorSection{maxVendorID: 50, isRange: true, numEntries: -1}
	})
	add("range consents max 0 zero entries", func(s *tcf2Spec) {
		s.consents = vendorSection{maxVendorID: 0, isRange: true, numEntries: -1}
	})
	add("range li max 0 zero entries", func(s *tcf2Spec) {
		s.legitInts = vendorSection{maxVendorID: 0, isRange: true, numEntries: -1}
	})
	add("range overlapping and unsorted", func(s *tcf2Spec) {
		s.consents = vendorSection{maxVendorID: 100, isRange: true, entries: []rangeEntry{{true, 50, 60}, {true, 10, 55}, {false, 99, 99}, {false, 3, 3}, {false, 3, 3}}, numEntries: -1}
	})

	badEntries := map[string][]rangeEntry{
		"single 0":           {{false, 0, 0}},
		"single above max":   {{false, 11, 11}},
		"single at max":      {{false, 10, 10}},
		"range start 0":      {{true, 0, 5}},
		"range end > max":    {{true, 2, 11}},
		"range end == max":   {{true, 2, 10}},
		"range end == start": {{true, 5, 5}},
		"range end < start":  {{true, 6, 5}},
		"second bad":         {{false, 1, 1}, {true, 9, 2}},
		"range 0-0":          {{true, 0, 0}},
	}
	names := make([]string, 0, len(badEntries))
	for name := range badEntries {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		entries := badEntries[name]
		add("consents "+name, func(s *tcf2Spec) {
			s.consents = vendorSection{maxVendorID: 10, isRange: true, entries: entries, numEntries: -1}
		})
		add("li "+name, func(s *tcf2Spec) {
			s.legitInts = vendorSection{maxVendorID: 10, isRange: true, entries: entries, numEntries: -1}
		})
		add("pub restriction "+name, func(s *tcf2Spec) {
			s.restrictions = []restriction{{purpose: 2, rtype: 1, entries: entries, numEntries: -1}}
		})
	}
	for _, n := range []int{1, 2, 7, 4095} {
		n := n
		add(fmt.Sprintf("consents numEntries %d with no entries", n), func(s *tcf2Spec) {
			s.consents = vendorSection{maxVendorID: 10, isRange: true, numEntries: n}
		})
		add(fmt.Sprintf("li numEntries %d with no entries", n), func(s *tcf2Spec) {
			s.legitInts = vendorSection{maxVendorID: 10, isRange: true, numEntries: n}
		})
		add(fmt.Sprintf("numRestrictions %d with none", n), func(s *tcf2Spec) { s.numRestrictions = n })
		add(fmt.Sprintf("restriction numEntries %d with none", n), func(s *tcf2Spec) {
			s.restrictions = []restriction{{purpose: 1, rtype: 0, numEntries: n}}
		})
	}
	add("pub restriction vendor 32767", func(s *tcf2Spec) {
		s.restrictions = []restriction{{purpose: 7, rtype: 1, entries: []rangeEntry{{false, 32767, 32767}}, numEntries: -1}}
	})
	add("pub restriction vendor 32768", func(s *tcf2Spec) {
		s.restrictions = []restriction{{purpose: 7, rtype: 1, entries: []rangeEntry{{false, 32768, 32768}}, numEntries: -1}}
	})
	add("pub restriction range to 32767", func(s *tcf2Spec) {
		s.restrictions = []restriction{{purpose: 7, rtype: 1, entries: []rangeEntry{{true, 1, 32767}}, numEntries: -1}}
	})
	add("pub restriction range to 40000", func(s *tcf2Spec) {
		s.restrictions = []restriction{{purpose: 7, rtype: 1, entries: []rangeEntry{{true, 1, 40000}}, numEntries: -1}}
	})
	add("pub restriction duplicate keys", func(s *tcf2Spec) {
		s.restrictions = []restriction{
			{purpose: 2, rtype: 0, entries: []rangeEntry{{false, 5, 5}}, numEntries: -1},
			{purpose: 2, rtype: 1, entries: []rangeEntry{{false, 6, 6}}, numEntries: -1},
			{purpose: 2, rtype: 0, entries: []rangeEntry{{false, 7, 7}, {true, 100, 200}}, numEntries: -1},
		}
	})
	add("pub restriction every key", func(s *tcf2Spec) {
		for k := uint64(0); k < 256; k++ {
			s.restrictions = append(s.restrictions, restriction{purpose: k >> 2, rtype: k & 3, entries: []rangeEntry{{false, k + 1, k + 1}}, numEntries: -1})
		}
	})
	add("pub restriction 300 restrictions", func(s *tcf2Spec) {
		for k := uint64(0); k < 300; k++ {
			s.restrictions = append(s.restrictions, restriction{purpose: k % 24, rtype: k % 3, entries: []rangeEntry{{true, k + 1, k + 3}}, numEntries: -1})
		}
	})
	add("pub restriction purpose 0 type 3", func(s *tcf2Spec) {
		s.restrictions = []restriction{{purpose: 0, rtype: 3, entries: []rangeEntry{{false, 1, 1}}, numEntries: -1}}
	})
	add("pub restriction purpose 63 type 3", func(s *tcf2Spec) {
		s.restrictions = []restriction{{purpose: 63, rtype: 3, entries: []rangeEntry{{true, 1, 32767}}, numEntries: -1}}
	})
	add("pub restriction no entries", func(s *tcf2Spec) {
		s.restrictions = []restriction{{purpose: 4, rtype: 2, numEntries: -1}}
	})
	add("no li section", func(s *tcf2Spec) { s.omitLegitInts = true; s.omitPubRestr = true })
	add("no pub restriction section", func(s *tcf2Spec) { s.omitPubRestr = true })

	// The legitimate interest section starts byte aligned: Go reads one byte past a 2-byte
	// ParseUInt16 bounds check and panics when the data ends right there.
	for _, max := range []uint64{2, 10, 18, 26, 34, 2050} {
		max := max
		for extra := 0; extra < 3; extra++ {
			s := baseTCF2()
			s.consents = vendorSection{maxVendorID: max, bitfield: everyThird(max), numEntries: -1}
			s.legitInts = vendorSection{maxVendorID: 0, bitfield: nil, numEntries: -1}
			data := s.encode()
			start := (230 + int(max)) / 8
			n := start + 2 + extra
			if n > len(data) {
				padded := make([]byte, n)
				copy(padded, data)
				data = padded
			}
			b.add(fmt.Sprintf("tcf2 edge: aligned li start max %d len %d", max, n), enc(data[:n]))
		}
	}
	// Same alignment produced by a range section.
	for _, n := range []int{0, 1, 2, 3} {
		s := baseTCF2()
		// 230 + 12 + 17*k must be a multiple of 8: k=2 -> 276 (no), use ranges of 33 bits.
		// 230 + 12 + 33 + 17 = 292 (no); 230+12+17*6 = 344 = 43*8.
		var entries []rangeEntry
		for i := uint64(1); i <= 6; i++ {
			entries = append(entries, rangeEntry{start: i, end: i})
		}
		s.consents = vendorSection{maxVendorID: 10, isRange: true, entries: entries, numEntries: -1}
		s.legitInts = vendorSection{maxVendorID: 0, numEntries: -1}
		data := s.encode()
		want := 43 + 2 + n
		if want > len(data) {
			padded := make([]byte, want)
			copy(padded, data)
			data = padded
		}
		b.add(fmt.Sprintf("tcf2 edge: aligned li start after range len %d", want), enc(data[:want]))
	}
}

func addTCF1EdgeCases(b *caseBuilder) {
	add := func(label string, mutate func(s *tcf1Spec)) {
		s := baseTCF1()
		mutate(s)
		b.add("tcf1 edge: "+label, enc(s.encode()))
	}
	for _, v := range []uint64{0, 1, 2, 3, 63} {
		v := v
		add(fmt.Sprintf("version %d", v), func(s *tcf1Spec) { s.version = v })
	}
	add("vendor list version 0", func(s *tcf1Spec) { s.vlv = 0 })
	add("max vendor 0", func(s *tcf1Spec) { s.maxVendorID = 0; s.bitfield = nil })
	for _, max := range []uint64{1, 2, 3, 4, 5, 11, 12, 13, 100, 65535} {
		max := max
		add(fmt.Sprintf("bitfield max %d", max), func(s *tcf1Spec) { s.maxVendorID = max; s.bitfield = everyThird(max) })
		add(fmt.Sprintf("bitfield max %d short", max), func(s *tcf1Spec) {
			s.maxVendorID = max
			if max > 8 {
				s.bitfield = everyThird(max - 8)
			} else {
				s.bitfield = nil
			}
		})
	}
	for _, def := range []bool{false, true} {
		def := def
		add(fmt.Sprintf("range default %t", def), func(s *tcf1Spec) {
			s.maxVendorID, s.isRange, s.defaultConsent = 40, true, def
			s.entries = []rangeEntry{{true, 2, 5}, {false, 9, 9}, {true, 30, 40}}
		})
		add(fmt.Sprintf("range default %t no entries", def), func(s *tcf1Spec) {
			s.maxVendorID, s.isRange, s.defaultConsent = 40, true, def
		})
		for _, bad := range [][]rangeEntry{{{false, 0, 0}}, {{false, 41, 41}}, {{true, 0, 3}}, {{true, 3, 41}}, {{true, 3, 3}}, {{true, 4, 3}}} {
			bad := bad
			add(fmt.Sprintf("range default %t bad %v", def, bad), func(s *tcf1Spec) {
				s.maxVendorID, s.isRange, s.defaultConsent = 40, true, def
				s.entries = bad
			})
		}
		add(fmt.Sprintf("range default %t numEntries 9 none", def), func(s *tcf1Spec) {
			s.maxVendorID, s.isRange, s.defaultConsent, s.numEntries = 40, true, def, 9
		})
	}
	for _, l := range []uint64{0, 25, 26, 63, 64*63 + 63} {
		l := l
		add(fmt.Sprintf("language %d", l), func(s *tcf1Spec) { s.lang = l })
	}
}

// everyThird sets every third vendor; above 2000 vendors it switches to blocks of 1000 so the
// goldens stay small.
func everyThird(n uint64) []bool {
	bits := make([]bool, n)
	for i := range bits {
		if n > 2000 {
			bits[i] = (i/1000)%2 == 0 || i == len(bits)-1
		} else {
			bits[i] = i%3 == 0
		}
	}
	return bits
}
