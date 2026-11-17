// Command gppgolden writes the differential golden files replayed by the pbs-gpp Rust tests
// (crates/gpp/src/golden_tests.rs).
//
// It runs the real github.com/prebid/go-gpp at the version pinned in Prebid Server's go.mod
// (this module's go.mod requires the same version), so run it from the directory of this module
// (crates/gpp/testdata/go):
//
//	go run .
//
// Every file has one case per line with tab-separated fields. Strings are written with esc (a
// quoted form where bytes outside printable ASCII, backslash and double quote are escaped as
// \xHH, \\ and \"), so lines never contain raw tabs or newlines. The corpus is generated from a
// fixed seed, so the output is deterministic.
package main

import (
	"bufio"
	"encoding/base64"
	"encoding/hex"
	"flag"
	"fmt"
	"log"
	"math/rand"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"unicode/utf8"

	gpp "github.com/prebid/go-gpp"
	"github.com/prebid/go-gpp/constants"
	"github.com/prebid/go-gpp/sections"
	"github.com/prebid/go-gpp/sections/uspca"
	"github.com/prebid/go-gpp/sections/uspco"
	"github.com/prebid/go-gpp/sections/uspct"
	"github.com/prebid/go-gpp/sections/uspnat"
	"github.com/prebid/go-gpp/sections/usput"
	"github.com/prebid/go-gpp/sections/uspva"
	"github.com/prebid/go-gpp/util"
)

const seed = 20261116

func main() {
	out := flag.String("out", "../golden", "output directory")
	flag.Parse()

	r := rand.New(rand.NewSource(seed))
	write(*out, "parse.txt", parseGolden(r))
	write(*out, "parse_bytes.txt", parseBytesGolden(r))
	write(*out, "bitstream.txt", bitstreamGolden(r))
	write(*out, "base64.txt", base64Golden(r))
	write(*out, "write.txt", writeGolden(r))
	write(*out, "encode.txt", encodeGolden(r))
	// edge.txt has its own random source so that adding to it never changes the files above.
	write(*out, "edge.txt", edgeGolden(rand.New(rand.NewSource(edgeSeed))))
}

func write(dir, name string, lines []string) {
	f, err := os.Create(filepath.Join(dir, name))
	if err != nil {
		log.Fatal(err)
	}
	w := bufio.NewWriter(f)
	for _, line := range lines {
		if _, err := w.WriteString(line + "\n"); err != nil {
			log.Fatal(err)
		}
	}
	if err := w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := f.Close(); err != nil {
		log.Fatal(err)
	}
	fmt.Printf("%s: %d cases\n", name, len(lines))
}

// esc quotes s so that it fits on one tab-separated line.
func esc(s string) string {
	var b strings.Builder
	b.WriteByte('"')
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c == '\\' || c == '"':
			b.WriteByte('\\')
			b.WriteByte(c)
		case c >= 0x20 && c <= 0x7e:
			b.WriteByte(c)
		default:
			fmt.Fprintf(&b, "\\x%02x", c)
		}
	}
	b.WriteByte('"')
	return b.String()
}

// ---------------------------------------------------------------------------------------------
// parse.txt: <esc input> \t <dump of gpp.Parse and gpp.Encode of the parsed sections>

func parseGolden(r *rand.Rand) []string {
	c := newCorpus()
	addRealWorld(c)
	addValid(c, r, 600)
	addMutations(c, r, 2200)
	addRandomHeaders(c, r, 600)
	addCraftedHeaders(c, r, 500)
	addSectionFocused(c, r)
	addStaleErrorCases(c, r, 300)

	lines := make([]string, 0, len(c.list))
	for _, s := range c.list {
		lines = append(lines, esc(s)+"\t"+dumpParse(s))
	}
	return lines
}

func dumpParse(s string) string {
	return dumpParseQ(s, esc)
}

// dumpParseQ is dumpParse with every string written through q.
func dumpParseQ(s string, q func(string) string) string {
	container, errs := gpp.Parse(s)
	var b strings.Builder
	fmt.Fprintf(&b, "v=%d t=[", container.Version)
	for i, id := range container.SectionTypes {
		if i > 0 {
			b.WriteByte(',')
		}
		b.WriteString(strconv.Itoa(int(id)))
	}
	b.WriteString("] s=[")
	for i, sec := range container.Sections {
		if i > 0 {
			b.WriteByte(';')
		}
		b.WriteString(dumpSection(sec, q))
	}
	b.WriteString("] e=[")
	for i, err := range errs {
		if i > 0 {
			b.WriteByte(',')
		}
		b.WriteString(q(err.Error()))
	}
	b.WriteString("] x=")
	// Encode sorts the slice in place, so it runs after the sections were dumped.
	encoded, err := gpp.Encode(container.Sections)
	if err != nil {
		b.WriteString("!" + q(err.Error()))
	} else {
		b.WriteString(q(encoded))
	}
	return b.String()
}

func dumpSection(sec gpp.Section, q func(string) string) string {
	var kind string
	var fields []string
	var gpc *sections.CommonUSGPCSegment
	switch v := sec.(type) {
	case uspnat.USPNAT:
		kind, gpc = "nat", &v.GPCSegment
		c := v.CoreSegment
		fields = nums(c.Version, c.SharingNotice, c.SaleOptOutNotice, c.SharingOptOutNotice,
			c.TargetedAdvertisingOptOutNotice, c.SensitiveDataProcessingOptOutNotice,
			c.SensitiveDataLimitUseNotice, c.SaleOptOut, c.SharingOptOut, c.TargetedAdvertisingOptOut)
		fields = append(fields, list(c.SensitiveDataProcessing), list(c.KnownChildSensitiveDataConsents))
		fields = append(fields, nums(c.PersonalDataConsents, c.MspaCoveredTransaction,
			c.MspaOptOutOptionMode, c.MspaServiceProviderMode)...)
	case uspca.USPCA:
		kind, gpc = "ca", &v.GPCSegment
		c := v.CoreSegment
		fields = nums(c.Version, c.SaleOptOutNotice, c.SharingOptOutNotice,
			c.SensitiveDataLimitUseNotice, c.SaleOptOut, c.SharingOptOut)
		fields = append(fields, list(c.SensitiveDataProcessing), list(c.KnownChildSensitiveDataConsents))
		fields = append(fields, nums(c.PersonalDataConsents, c.MspaCoveredTransaction,
			c.MspaOptOutOptionMode, c.MspaServiceProviderMode)...)
	case uspva.USPVA:
		kind, fields = "va", common(v.CoreSegment)
	case uspco.USPCO:
		kind, gpc, fields = "co", &v.GPCSegment, common(v.CoreSegment)
	case usput.USPUT:
		kind = "ut"
		c := v.CoreSegment
		fields = nums(c.Version, c.SharingNotice, c.SaleOptOutNotice,
			c.TargetedAdvertisingOptOutNotice, c.SensitiveDataProcessingOptOutNotice, c.SaleOptOut,
			c.TargetedAdvertisingOptOut)
		fields = append(fields, list(c.SensitiveDataProcessing))
		fields = append(fields, nums(c.KnownChildSensitiveDataConsents, c.MspaCoveredTransaction,
			c.MspaOptOutOptionMode, c.MspaServiceProviderMode)...)
	case uspct.USPCT:
		kind, gpc, fields = "ct", &v.GPCSegment, common(v.CoreSegment)
	default:
		return fmt.Sprintf("g:%d:%s", int(sec.GetID()), q(sec.GetValue()))
	}
	if gpc != nil {
		fields = append(fields, fmt.Sprintf("gpc=%d/%t", gpc.SubsectionType, gpc.Gpc))
	}
	return fmt.Sprintf("%s:%d:%s:%s:%s:%s", kind, int(sec.GetID()), q(sec.GetValue()),
		strings.Join(fields, ","), q(string(sec.Encode(true))), q(string(sec.Encode(false))))
}

func common(c sections.CommonUSCoreSegment) []string {
	fields := nums(c.Version, c.SharingNotice, c.SaleOptOutNotice,
		c.TargetedAdvertisingOptOutNotice, c.SaleOptOut, c.TargetedAdvertisingOptOut)
	fields = append(fields, list(c.SensitiveDataProcessing), list(c.KnownChildSensitiveDataConsents))
	return append(fields, nums(c.MspaCoveredTransaction, c.MspaOptOutOptionMode,
		c.MspaServiceProviderMode)...)
}

func nums(values ...byte) []string {
	out := make([]string, len(values))
	for i, v := range values {
		out[i] = strconv.Itoa(int(v))
	}
	return out
}

func list(values []byte) string {
	return "[" + strings.Join(nums(values...), " ") + "]"
}

// corpus is an ordered set of GPP strings that are safe to run through go-gpp.
type corpus struct {
	seen map[string]bool
	list []string
}

func newCorpus() *corpus {
	return &corpus{seen: map[string]bool{}}
}

func (c *corpus) add(s string) {
	if c.seen[s] || !utf8.ValidString(s) || !safeForGo(s) {
		return
	}
	c.seen[s] = true
	c.list = append(c.list, s)
}

// safeForGo filters out headers that would make go-gpp hang: Fibonacci integers long enough for
// its exponential fibonacci() recursion to take seconds, ranges ending at 65535 (its uint16 loop
// never terminates), ID ranges too large to materialise, and streams long enough for the
// uint16 bit position to wrap. The Rust port handles all of these itself.
func safeForGo(s string) bool {
	header := strings.SplitN(s, "~", 2)[0]
	bs, err := util.NewBitStreamFromBase64(header)
	if err != nil || bs.Len() < gpp.MaxHeaderLength || header[0] != gpp.SectionGPPByte {
		return true
	}
	data := rawData(bs)
	if len(data) >= 8192 {
		return false
	}
	pos := uint16(12)
	n, err := util.ParseUInt12(data, pos)
	if err != nil {
		return true
	}
	pos += 12
	for i := 0; i < int(n); i++ {
		flagBit, err := util.ParseByte1(data, pos)
		if err != nil {
			return true
		}
		pos++
		ints := 1 + int(flagBit)
		for j := 0; j < ints; j++ {
			length, ok := fibLength(data, pos)
			if !ok {
				return true // the read fails before any long integer is summed
			}
			if length > 40 {
				return false
			}
			pos += length
		}
	}
	bs.SetPosition(12)
	ir, err := bs.ReadFibonacciRange()
	if err != nil {
		return true
	}
	total := 0
	for _, rg := range ir.Range {
		if rg.EndID == 0xffff {
			return false
		}
		if rg.StartID <= rg.EndID {
			total += int(rg.EndID-rg.StartID) + 1
		}
	}
	return total <= 100000
}

// fibLength returns the number of bits of the Fibonacci integer at pos, or false if the data ends
// first.
func fibLength(data []byte, pos uint16) (uint16, bool) {
	var prev byte
	for length := uint16(1); ; length++ {
		bit, err := util.ParseByte1(data, pos+length-1)
		if err != nil {
			return 0, false
		}
		if length > 1 && prev == 1 && bit == 1 {
			return length, true
		}
		prev = bit
	}
}

// rawData reads the stream's bytes through its exported API.
func rawData(bs *util.BitStream) []byte {
	saved := bs.GetPosition()
	defer bs.SetPosition(saved)
	out := make([]byte, 0, bs.Len())
	bs.SetPosition(0)
	for i := uint16(0); i < bs.Len(); i++ {
		b, err := bs.ReadByte8()
		if err != nil {
			break
		}
		out = append(out, b)
	}
	return out
}

func addRealWorld(c *corpus) {
	for _, s := range []string{
		// go-gpp tests.
		"DBABM~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA",
		"DBABMA~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA",
		"DBACNY~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA~1YNN",
		"DBABjw~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA~1YNN",
		"DBABBgA~xlgWEYCZAA",
		"DBABRgA~bSFgmiU",
		"DBGBM~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA",
		"DBABBgA~xlgWE",
		"DBABh4A~BlgWEYCY.QA~BSFgmiU",
		"DBABRg~bSFgmiU",
		"DBABJg~bSFgmJQ.YA",
		"DBABVg~bSFgmSZQ.YA",
		"DBABLA~DSJgmkoZJSA.YA",
		"DBADLO8~BSJgmkoZJSA.YA~BSFgmiU~BWJYJllA~BSFgmSZQ.YA",
		"DBABrGA~DSJgmkoZJSA.YA~BlgWEYCY.QA~BSFgmiU~bSFgmJQ.YA~BWJYJllA~bSFgmSZQ.YA",
		"DBACLMA~BAAAAAAAAAA.QA~BaAAAAA",
		"DBACTjw~1YYN~BSZZYgkA~BaRlkCSA.QA",
		"DBACMYA~CPpcCoAPpcCoAPoABABGCyCUACAAACAAAAAAAVQAQAVABZABABYAAAAA.QADgIAAA.IABE~" +
			"CPpcCoAPpcCoAPoABABGCyCQAEAAAEAAAAEFABAEEAN8AEAN4A.YAAAAAAAAAA",
		"DBABYA~test_minimum",
		// Prebid Server tests and sample requests.
		"DBABMA~1YNN",
		"DBABMA~valid",
		"DBACNYA~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA~1NYN",
		"DBACNYA~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA~1YNN",
		"DBACNYA~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA~present",
		"malformed",
		"malformedGPPString",
		"D",
		// IAB-style examples.
		"DBABzw~1YNN~BVQqAAAAAgA.QA",
		"DBABLA~BVQqAAAAAgA.QA",
		"DBABLA~BVQVAAAAAgA.QA",
		"DBABLA~BVVqAAAAAgA",
		"DBABBg~BVoYYZoI",
		"DBABBg~BVoYYYI",
		"DBABRg~BVoYYYI",
		"DBABJg~BVoYYRA.QA",
		"DBABVg~BVoYYZoI.QA",
		"DBABLg~BVoYYZo",
		"DBACOeA~CQBGUIAQBGUIAEyAAAENA-FgAAAAAAAAAAAAAAAAAAAA~1---",
		"DBABMA~CQBGUIAQBGUIAEyAAAENA-FgAAAAAAAAAAAAAAAAAAAA.YAAAAAAAAAAA",
		// Edge shapes.
		"", "~", "~~", "DB", "DBA", "DBAA", "DBAAAA", "DBAAAA~", "DBABMA~", "DBABMA~~",
		"DBACNYA~~", "AAAA", "ZZZZ", "CBABMA~x", "dBABMA~x", "D=AB", "DBA=", "DB\nABMA~x",
		"\nDBABMA~x", "DBABMA\n~x", "DBABMA\r\n~x", "DBABM A~x", "DBABMA~1YNN~", "DBABBgA~",
		"DBABBgA~.", "DBABBgA~xlgWEYCZAA.", "DBABBgA~xlgWEYCZAA..", "DBABBgA~xlgWEYCZAA.YA.YA",
		"DBABBgA~xlgWEYCZAA.wA", "DBABBgA~xlgWEYCZAA.gA", "DBABBgA~xlgWEYCZAA.A",
		"DBABBgA~xlgWEYCZAA.Q", "DBABBgA~xlgWEYCZAA.!", "DBABRgA~bSFgmiU.YA",
		"DBABBgA~xlgWEYCZAA\u00e9", "D\u00e9", "DBABMA~\u00e9", "DBAB\u00e9A~x",
	} {
		c.add(s)
	}
}

// rawSection is a gpp.Section carrying an arbitrary ID and string, used to build GPP strings
// with gpp.Encode.
type rawSection struct {
	id    constants.SectionID
	value string
}

func (s rawSection) GetID() constants.SectionID { return s.id }
func (s rawSection) GetValue() string           { return s.value }
func (s rawSection) Encode(bool) []byte         { return []byte(s.value) }

func two(r *rand.Rand) byte { return byte(r.Intn(4)) }

func twos(r *rand.Rand, n int) []byte {
	out := make([]byte, n)
	for i := range out {
		out[i] = two(r)
	}
	return out
}

func randomGPC(r *rand.Rand) sections.CommonUSGPCSegment {
	return sections.CommonUSGPCSegment{SubsectionType: 1, Gpc: r.Intn(2) == 1}
}

func randomCommon(r *rand.Rand, sdp, kc int) sections.CommonUSCoreSegment {
	return sections.CommonUSCoreSegment{
		Version: byte(r.Intn(64)), SharingNotice: two(r), SaleOptOutNotice: two(r),
		TargetedAdvertisingOptOutNotice: two(r), SaleOptOut: two(r),
		TargetedAdvertisingOptOut: two(r), SensitiveDataProcessing: twos(r, sdp),
		KnownChildSensitiveDataConsents: twos(r, kc), MspaCoveredTransaction: two(r),
		MspaOptOutOptionMode: two(r), MspaServiceProviderMode: two(r),
	}
}

// randomUSSection returns a valid section string for one of the decoded US section IDs.
func randomUSSection(r *rand.Rand, id constants.SectionID) string {
	gpcIncluded := r.Intn(3) > 0
	switch id {
	case constants.SectionUSPNAT:
		return string(uspnat.USPNAT{CoreSegment: uspnat.USPNATCoreSegment{
			Version: byte(r.Intn(64)), SharingNotice: two(r), SaleOptOutNotice: two(r),
			SharingOptOutNotice: two(r), TargetedAdvertisingOptOutNotice: two(r),
			SensitiveDataProcessingOptOutNotice: two(r), SensitiveDataLimitUseNotice: two(r),
			SaleOptOut: two(r), SharingOptOut: two(r), TargetedAdvertisingOptOut: two(r),
			SensitiveDataProcessing: twos(r, 12), KnownChildSensitiveDataConsents: twos(r, 2),
			PersonalDataConsents: two(r), MspaCoveredTransaction: two(r),
			MspaOptOutOptionMode: two(r), MspaServiceProviderMode: two(r),
		}, GPCSegment: randomGPC(r)}.Encode(gpcIncluded))
	case constants.SectionUSPCA:
		return string(uspca.USPCA{CoreSegment: uspca.USPCACoreSegment{
			Version: byte(r.Intn(64)), SaleOptOutNotice: two(r), SharingOptOutNotice: two(r),
			SensitiveDataLimitUseNotice: two(r), SaleOptOut: two(r), SharingOptOut: two(r),
			SensitiveDataProcessing: twos(r, 9), KnownChildSensitiveDataConsents: twos(r, 2),
			PersonalDataConsents: two(r), MspaCoveredTransaction: two(r),
			MspaOptOutOptionMode: two(r), MspaServiceProviderMode: two(r),
		}, GPCSegment: randomGPC(r)}.Encode(gpcIncluded))
	case constants.SectionUSPVA:
		return string(uspva.USPVA{CoreSegment: randomCommon(r, 8, 1)}.Encode(gpcIncluded))
	case constants.SectionUSPCO:
		return string(uspco.USPCO{CoreSegment: randomCommon(r, 7, 1),
			GPCSegment: randomGPC(r)}.Encode(gpcIncluded))
	case constants.SectionUSPUT:
		return string(usput.USPUT{CoreSegment: usput.USPUTCoreSegment{
			Version: byte(r.Intn(64)), SharingNotice: two(r), SaleOptOutNotice: two(r),
			TargetedAdvertisingOptOutNotice: two(r), SensitiveDataProcessingOptOutNotice: two(r),
			SaleOptOut: two(r), TargetedAdvertisingOptOut: two(r),
			SensitiveDataProcessing: twos(r, 8), KnownChildSensitiveDataConsents: two(r),
			MspaCoveredTransaction: two(r), MspaOptOutOptionMode: two(r),
			MspaServiceProviderMode: two(r),
		}}.Encode(gpcIncluded))
	default:
		return string(uspct.USPCT{CoreSegment: randomCommon(r, 8, 3),
			GPCSegment: randomGPC(r)}.Encode(gpcIncluded))
	}
}

var usIDs = []constants.SectionID{7, 8, 9, 10, 11, 12}

var genericIDs = []constants.SectionID{1, 2, 3, 4, 5, 6, 13, 14, 15, 16, 20, 33, 100, 1000, 6000}

const b64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"

func randomB64(r *rand.Rand, n int) string {
	b := make([]byte, n)
	for i := range b {
		b[i] = b64[r.Intn(len(b64))]
	}
	return string(b)
}

func randomGenericValue(r *rand.Rand) string {
	switch r.Intn(6) {
	case 0:
		return "CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA"
	case 1:
		return []string{"1YNN", "1NYN", "1---", "1YYY"}[r.Intn(4)]
	case 2:
		return ""
	case 3:
		return randomB64(r, 1+r.Intn(10)) + "." + randomB64(r, r.Intn(6))
	default:
		return randomB64(r, r.Intn(40))
	}
}

// randomSections picks 1..6 distinct IDs, mostly decoded US sections.
func randomSections(r *rand.Rand) []gpp.Section {
	n := 1 + r.Intn(6)
	used := map[constants.SectionID]bool{}
	var out []gpp.Section
	for len(out) < n {
		var id constants.SectionID
		if r.Intn(3) > 0 {
			id = usIDs[r.Intn(len(usIDs))]
		} else {
			id = genericIDs[r.Intn(len(genericIDs))]
		}
		if used[id] {
			continue
		}
		used[id] = true
		value := randomGenericValue(r)
		if id >= 7 && id <= 12 {
			value = randomUSSection(r, id)
		}
		out = append(out, rawSection{id: id, value: value})
	}
	return out
}

func mustEncode(secs []gpp.Section) string {
	s, err := gpp.Encode(secs)
	if err != nil {
		log.Fatal(err)
	}
	return s
}

func addValid(c *corpus, r *rand.Rand, n int) {
	for i := 0; i < n; i++ {
		c.add(mustEncode(randomSections(r)))
	}
}

// mutationTokens are inserted whole, so mutations keep the string valid UTF-8.
var mutationTokens = append(strings.Split(b64, ""), "=", "+", "/", ".", "~", " ", "\n", "\r", "!",
	"\x00", "\x7f", "\u00e9", "\u20ac")

func randomToken(r *rand.Rand) string {
	return mutationTokens[r.Intn(len(mutationTokens))]
}

func mutate(r *rand.Rand, s string) string {
	parts := strings.Split(s, "~")
	switch r.Intn(10) {
	case 0: // truncate
		return s[:r.Intn(len(s)+1)]
	case 1: // delete a byte
		if len(s) == 0 {
			return s
		}
		i := r.Intn(len(s))
		return s[:i] + s[i+1:]
	case 2: // replace a byte
		if len(s) == 0 {
			return s
		}
		i := r.Intn(len(s))
		return s[:i] + randomToken(r) + s[i+1:]
	case 3: // insert a byte
		i := r.Intn(len(s) + 1)
		return s[:i] + randomToken(r) + s[i:]
	case 4: // drop a section
		if len(parts) < 2 {
			return s + "~"
		}
		i := 1 + r.Intn(len(parts)-1)
		return strings.Join(append(append([]string{}, parts[:i]...), parts[i+1:]...), "~")
	case 5: // duplicate a section
		i := r.Intn(len(parts))
		return strings.Join(append(append([]string{}, parts...), parts[i]), "~")
	case 6: // swap two sections
		if len(parts) < 3 {
			return s
		}
		i, j := 1+r.Intn(len(parts)-1), 1+r.Intn(len(parts)-1)
		parts[i], parts[j] = parts[j], parts[i]
		return strings.Join(parts, "~")
	case 7: // append garbage section
		return s + "~" + randomB64(r, r.Intn(12))
	case 8: // swap the header for another valid one
		other := strings.Split(mustEncode(randomSections(r)), "~")[0]
		parts[0] = other
		return strings.Join(parts, "~")
	default: // truncate one section
		i := r.Intn(len(parts))
		parts[i] = parts[i][:r.Intn(len(parts[i])+1)]
		return strings.Join(parts, "~")
	}
}

func addMutations(c *corpus, r *rand.Rand, n int) {
	for i := 0; i < n; i++ {
		s := mutate(r, mustEncode(randomSections(r)))
		if r.Intn(3) == 0 {
			s = mutate(r, s)
		}
		c.add(s)
	}
}

func addRandomHeaders(c *corpus, r *rand.Rand, n int) {
	for i := 0; i < n; i++ {
		b := make([]byte, 3+r.Intn(10))
		r.Read(b)
		header := base64.RawURLEncoding.EncodeToString(b)
		if r.Intn(4) > 0 {
			header = "D" + header[1:]
		}
		parts := []string{header}
		for j := r.Intn(5); j > 0; j-- {
			if r.Intn(2) == 0 {
				parts = append(parts, randomUSSection(r, usIDs[r.Intn(len(usIDs))]))
			} else {
				parts = append(parts, randomGenericValue(r))
			}
		}
		c.add(strings.Join(parts, "~"))
	}
}

// craftedHeader writes a header bit by bit: version, entry count, then each entry as either a
// single offset or a (start offset, length) pair, which may overlap, repeat or wrap past 65535.
func craftedHeader(r *rand.Rand) (string, int) {
	bs := util.NewBitStreamForWrite()
	bs.WriteByte6(3)
	bs.WriteByte6(byte(r.Intn(64)))
	entries := r.Intn(6)
	if r.Intn(10) == 0 {
		entries = 10 + r.Intn(5)
	}
	bs.WriteUInt12(uint16(entries))
	var maxValue uint16
	count := 0
	for i := 0; i < entries; i++ {
		offset := uint16(1 + r.Intn(12))
		if r.Intn(5) == 0 {
			offset = uint16(1 + r.Intn(6764))
		}
		if r.Intn(2) == 0 {
			bs.WriteByte1(0)
			mustWriteFib(bs, offset)
			entry := maxValue + offset
			if entry > maxValue {
				maxValue = entry
			}
			count++
		} else {
			length := uint16(1 + r.Intn(4))
			bs.WriteByte1(1)
			mustWriteFib(bs, offset)
			mustWriteFib(bs, length)
			start := maxValue + offset
			end := start + length
			if end > maxValue {
				maxValue = end
			}
			if start <= end {
				count += int(end-start) + 1
			}
		}
	}
	// Optionally cut the header short.
	encoded := string(bs.Base64Encode())
	if r.Intn(8) == 0 {
		encoded = encoded[:r.Intn(len(encoded)+1)]
	}
	return encoded, count
}

func mustWriteFib(bs *util.BitStream, v uint16) {
	if err := bs.WriteFibonacciInt(v); err != nil {
		log.Fatal(err)
	}
}

func addCraftedHeaders(c *corpus, r *rand.Rand, n int) {
	for i := 0; i < n; i++ {
		header, count := craftedHeader(r)
		// Usually match the section count, sometimes miss by one.
		count += []int{0, 0, 0, 0, 1, -1}[r.Intn(6)]
		if count < 0 || count > 40 {
			count = r.Intn(3)
		}
		parts := []string{header}
		for j := 0; j < count; j++ {
			if r.Intn(2) == 0 {
				parts = append(parts, randomUSSection(r, usIDs[r.Intn(len(usIDs))]))
			} else {
				parts = append(parts, randomGenericValue(r))
			}
		}
		c.add(strings.Join(parts, "~"))
	}
}

var gpcVariants = []string{"", "A", "QA", "YA", "gA", "wA", "Q", "Y", "QAAA", "!", "Q=", "\n"}

// addSectionFocused builds single-section strings for every decoded US section, truncated at
// every length and with every GPC variant. Decoded streams always end on a byte boundary, so
// this produces the error for every field that starts at a multiple of 8 bits; fields that start
// elsewhere can never fail to read.
func addSectionFocused(c *corpus, r *rand.Rand) {
	for _, id := range usIDs {
		header := strings.Split(mustEncode([]gpp.Section{rawSection{id: id}}), "~")[0]
		for k := 0; k < 6; k++ {
			valid := randomUSSection(r, id)
			core := strings.Split(valid, ".")[0]
			for cut := 0; cut <= len(core); cut++ {
				c.add(header + "~" + core[:cut])
			}
			for _, gpc := range gpcVariants {
				c.add(header + "~" + core + "." + gpc)
			}
			c.add(header + "~" + core + "~extra")
			for j := 0; j < 8; j++ {
				c.add(header + "~" + randomB64(r, r.Intn(16)) + []string{"", ".", ".QA", ".YA"}[r.Intn(4)])
			}
		}
	}
}

// addStaleErrorCases mixes failing US sections with generic sections to exercise go-gpp's stale
// err check in the default branch.
func addStaleErrorCases(c *corpus, r *rand.Rand, n int) {
	for i := 0; i < n; i++ {
		secs := randomSections(r)
		for j, s := range secs {
			raw := s.(rawSection)
			if raw.id >= 7 && raw.id <= 12 && r.Intn(2) == 0 {
				raw.value = raw.value[:r.Intn(len(raw.value)+1)]
				if r.Intn(3) == 0 {
					raw.value = "!" + raw.value
				}
				secs[j] = raw
			}
		}
		c.add(mustEncode(secs))
	}
}

// ---------------------------------------------------------------------------------------------
// parse_bytes.txt: <esc input bytes, not valid UTF-8> \t t=[...] e=[...]
// The Rust API takes &str; these cases check that converting raw bytes with
// String::from_utf8_lossy preserves go-gpp's section IDs and error messages.

func parseBytesGolden(r *rand.Rand) []string {
	var lines []string
	seen := map[string]bool{}
	for len(lines) < 400 {
		s := mustEncode(randomSections(r))
		if r.Intn(2) == 0 {
			s = mutate(r, s)
		}
		b := []byte(s)
		for k := 1 + r.Intn(2); k > 0; k-- {
			i := r.Intn(len(b) + 1)
			bad := []byte{byte(0x80 + r.Intn(128))}
			if r.Intn(3) == 0 {
				bad = append(bad, byte(r.Intn(256)))
			}
			b = append(b[:i], append(bad, b[i:]...)...)
		}
		in := string(b)
		if utf8.ValidString(in) || seen[in] || !safeForGo(in) {
			continue
		}
		seen[in] = true
		container, errs := gpp.Parse(in)
		var d strings.Builder
		d.WriteString("t=[")
		for i, id := range container.SectionTypes {
			if i > 0 {
				d.WriteByte(',')
			}
			d.WriteString(strconv.Itoa(int(id)))
		}
		d.WriteString("] e=[")
		for i, err := range errs {
			if i > 0 {
				d.WriteByte(',')
			}
			d.WriteString(esc(err.Error()))
		}
		d.WriteString("]")
		lines = append(lines, esc(in)+"\t"+d.String())
	}
	return lines
}

// ---------------------------------------------------------------------------------------------
// bitstream.txt: <op> \t <hex data> \t <bit offset> \t <result>
// result is "=<value>@<position after>" or "!<esc error>".

func bitstreamGolden(r *rand.Rand) []string {
	ops := []string{"b1", "b2", "b4", "b6", "b8", "u12", "u16", "fib", "fibr", "intr",
		"tbf-1", "tbf0", "tbf1", "tbf3", "tbf12"}
	var lines []string
	for _, op := range ops {
		for i := 0; i < 400; i++ {
			maxLen := 9
			if op == "fib" {
				maxLen = 6 // keeps go-gpp's exponential fibonacci() fast
			}
			data := make([]byte, r.Intn(maxLen))
			r.Read(data)
			if r.Intn(4) == 0 {
				for j := range data {
					data[j] &= byte(r.Intn(256))
				}
			}
			offset := uint16(r.Intn(len(data)*8 + 10))
			lines = append(lines, fmt.Sprintf("%s\t%s\t%d\t%s", op, hex.EncodeToString(data),
				offset, runOp(op, data, offset)))
		}
	}
	return lines
}

func runOp(op string, data []byte, offset uint16) string {
	bs := util.NewBitStream(data)
	bs.SetPosition(offset)
	var value string
	var err error
	switch op {
	case "b1":
		var v byte
		v, err = bs.ReadByte1()
		value = strconv.Itoa(int(v))
	case "b2":
		var v byte
		v, err = bs.ReadByte2()
		value = strconv.Itoa(int(v))
	case "b4":
		var v byte
		v, err = bs.ReadByte4()
		value = strconv.Itoa(int(v))
	case "b6":
		var v byte
		v, err = bs.ReadByte6()
		value = strconv.Itoa(int(v))
	case "b8":
		var v byte
		v, err = bs.ReadByte8()
		value = strconv.Itoa(int(v))
	case "u12":
		var v uint16
		v, err = bs.ReadUInt12()
		value = strconv.Itoa(int(v))
	case "u16":
		var v uint16
		v, err = bs.ReadUInt16()
		value = strconv.Itoa(int(v))
	case "fib":
		var v uint16
		v, err = bs.ReadFibonacciInt()
		value = strconv.Itoa(int(v))
	case "fibr", "intr":
		var ir *util.IntRange
		if op == "fibr" {
			ir, err = bs.ReadFibonacciRange()
		} else {
			ir, err = bs.ReadIntRange()
		}
		if err == nil {
			value = dumpIntRange(ir)
		}
	default:
		n, convErr := strconv.Atoi(strings.TrimPrefix(op, "tbf"))
		if convErr != nil {
			log.Fatal(convErr)
		}
		var v []byte
		v, err = bs.ReadTwoBitField(n)
		value = list(v)
	}
	if err != nil {
		return "!" + esc(err.Error())
	}
	return fmt.Sprintf("=%s@%d", value, bs.GetPosition())
}

func dumpIntRange(ir *util.IntRange) string {
	parts := make([]string, len(ir.Range))
	for i, rg := range ir.Range {
		parts[i] = fmt.Sprintf("%d-%d", rg.StartID, rg.EndID)
	}
	return fmt.Sprintf("%d/%d/[%s]", ir.Size, ir.Max, strings.Join(parts, " "))
}

// ---------------------------------------------------------------------------------------------
// base64.txt: <esc input> \t <"=" hex bytes | "!" esc error> from util.NewBitStreamFromBase64.

func base64Golden(r *rand.Rand) []string {
	extra := []string{"=", "+", "/", "\n", "\r", ".", "~", " ", "\x00", "\u00e9"}
	var lines []string
	seen := map[string]bool{}
	add := func(s string) {
		if seen[s] {
			return
		}
		seen[s] = true
		bs, err := util.NewBitStreamFromBase64(s)
		if err != nil {
			lines = append(lines, esc(s)+"\t!"+esc(err.Error()))
			return
		}
		lines = append(lines, esc(s)+"\t="+hex.EncodeToString(rawData(bs)))
	}
	for _, s := range []string{"", "A", "AA", "AAA", "AAAA", "AAAAA", "\n", "\r\n", "A\n",
		"\nA", "AB\n", "AAAA\n", "AAAAA\n", "A=", "AA==", "AAA=", "D_", "D-", "____", "+/+/"} {
		add(s)
	}
	for i := 0; i < 1500; i++ {
		var b strings.Builder
		for j := r.Intn(14); j > 0; j-- {
			if r.Intn(8) == 0 {
				b.WriteString(extra[r.Intn(len(extra))])
			} else {
				b.WriteByte(b64[r.Intn(len(b64))])
			}
		}
		add(b.String())
	}
	return lines
}

// ---------------------------------------------------------------------------------------------
// write.txt: bit writer goldens.
//   fib \t <n> \t <hex | "!" esc error>                 WriteFibonacciInt on an empty stream
//   ops \t <w:v,...> \t <hex>                             WriteByteN / WriteUInt12 / WriteUInt16
//   range \t <s-e ...> \t <hex> \t <"" | "!" esc error>   WriteIntRange (bytes written before an
//                                                         error are kept)

func writeGolden(r *rand.Rand) []string {
	var lines []string
	for n := 0; n <= 6800; n++ {
		bs := util.NewBitStreamForWrite()
		if err := bs.WriteFibonacciInt(uint16(n)); err != nil {
			lines = append(lines, fmt.Sprintf("fib\t%d\t!%s", n, esc(err.Error())))
			continue
		}
		lines = append(lines, fmt.Sprintf("fib\t%d\t%s", n, hex.EncodeToString(rawData(bs))))
	}
	for _, n := range []int{65535, 10000, 7000} {
		bs := util.NewBitStreamForWrite()
		err := bs.WriteFibonacciInt(uint16(n))
		lines = append(lines, fmt.Sprintf("fib\t%d\t!%s", n, esc(err.Error())))
	}
	widths := []int{1, 2, 4, 6, 8, 12, 16}
	for i := 0; i < 500; i++ {
		bs := util.NewBitStreamForWrite()
		var ops []string
		for j := r.Intn(12); j > 0; j-- {
			w := widths[r.Intn(len(widths))]
			v := r.Intn(65536)
			if w <= 8 {
				v = r.Intn(256)
			}
			switch w {
			case 1:
				bs.WriteByte1(byte(v))
			case 2:
				bs.WriteByte2(byte(v))
			case 4:
				bs.WriteByte4(byte(v))
			case 6:
				bs.WriteByte6(byte(v))
			case 8:
				bs.WriteByte8(byte(v))
			case 12:
				bs.WriteUInt12(uint16(v))
			default:
				bs.WriteUInt16(uint16(v))
			}
			ops = append(ops, fmt.Sprintf("%d:%d", w, v))
		}
		lines = append(lines, fmt.Sprintf("ops\t%s\t%s", strings.Join(ops, ","),
			hex.EncodeToString(rawData(bs))))
	}
	for i := 0; i < 500; i++ {
		var ranges []util.IRange
		var parts []string
		prev := 0
		for j := r.Intn(6); j > 0; j-- {
			start := prev + 1 + r.Intn(20) - 3
			if r.Intn(10) == 0 {
				start = prev + 1 + r.Intn(8000)
			}
			end := start + r.Intn(5) - 1
			if start < 0 || end < 0 || start > 65535 || end > 65535 {
				continue
			}
			ranges = append(ranges, util.IRange{StartID: uint16(start), EndID: uint16(end)})
			parts = append(parts, fmt.Sprintf("%d-%d", start, end))
			prev = end
		}
		bs := util.NewBitStreamForWrite()
		err := bs.WriteIntRange(&util.IntRange{Size: uint16(len(ranges)), Range: ranges})
		result := ""
		if err != nil {
			result = "!" + esc(err.Error())
		}
		lines = append(lines, fmt.Sprintf("range\t%s\t%s\t%s", strings.Join(parts, " "),
			hex.EncodeToString(rawData(bs)), result))
	}
	return lines
}

// ---------------------------------------------------------------------------------------------
// encode.txt: <sections> \t <"=" esc gpp string | "!" esc error> from gpp.Encode.
// Sections are separated by ";". A generic section is "g:<id>:<value>" (values are base64url or
// "1YNN"-style, never containing ":" or ";"); a US section is "<kind>:<id>:<fields>" with the
// field values in Go struct order, slices written as "a.b.c" (possibly empty), and the GPC
// segment last as "<subsection type>/<0|1>" for kinds that have one. Field values are arbitrary
// bytes and slice lengths arbitrary, to exercise Go's bit masking.

func encodeGolden(r *rand.Rand) []string {
	var lines []string
	for i := 0; i < 600; i++ {
		n := r.Intn(6)
		var secs []gpp.Section
		var descs []string
		for j := 0; j < n; j++ {
			sec, desc := randomEncodeSection(r)
			secs = append(secs, sec)
			descs = append(descs, desc)
		}
		encoded, err := gpp.Encode(secs)
		result := "=" + esc(encoded)
		if err != nil {
			result = "!" + esc(err.Error())
		}
		lines = append(lines, strings.Join(descs, ";")+"\t"+result)
	}
	return lines
}

func anyByte(r *rand.Rand) byte {
	if r.Intn(3) == 0 {
		return byte(r.Intn(256))
	}
	return two(r)
}

func anyBytes(r *rand.Rand, n int) []byte {
	if r.Intn(4) == 0 {
		n = r.Intn(n + 3)
	}
	out := make([]byte, n)
	for i := range out {
		out[i] = anyByte(r)
	}
	return out
}

func dotted(values []byte) string {
	return strings.Join(nums(values...), ".")
}

func randomEncodeID(r *rand.Rand, kind constants.SectionID) constants.SectionID {
	switch r.Intn(12) {
	case 0:
		return constants.SectionID(r.Intn(3) - 1) // -1, 0, 1
	case 1:
		return constants.SectionID(65530 + r.Intn(10))
	case 2:
		return constants.SectionID(1 + r.Intn(20000))
	default:
		return kind
	}
}

func randomEncodeSection(r *rand.Rand) (gpp.Section, string) {
	gpc := sections.CommonUSGPCSegment{SubsectionType: anyByte(r), Gpc: r.Intn(2) == 1}
	gpcDesc := fmt.Sprintf("%d/%d", gpc.SubsectionType, map[bool]int{false: 0, true: 1}[gpc.Gpc])
	common := func(sdp, kc int) (sections.CommonUSCoreSegment, string) {
		c := sections.CommonUSCoreSegment{
			Version: anyByte(r), SharingNotice: anyByte(r), SaleOptOutNotice: anyByte(r),
			TargetedAdvertisingOptOutNotice: anyByte(r), SaleOptOut: anyByte(r),
			TargetedAdvertisingOptOut: anyByte(r), SensitiveDataProcessing: anyBytes(r, sdp),
			KnownChildSensitiveDataConsents: anyBytes(r, kc), MspaCoveredTransaction: anyByte(r),
			MspaOptOutOptionMode: anyByte(r), MspaServiceProviderMode: anyByte(r),
		}
		desc := strings.Join(append(nums(c.Version, c.SharingNotice, c.SaleOptOutNotice,
			c.TargetedAdvertisingOptOutNotice, c.SaleOptOut, c.TargetedAdvertisingOptOut),
			dotted(c.SensitiveDataProcessing), dotted(c.KnownChildSensitiveDataConsents),
			strconv.Itoa(int(c.MspaCoveredTransaction)), strconv.Itoa(int(c.MspaOptOutOptionMode)),
			strconv.Itoa(int(c.MspaServiceProviderMode))), ",")
		return c, desc
	}
	switch kind := r.Intn(7); kind {
	case 0:
		id := randomEncodeID(r, 7)
		c := uspnat.USPNATCoreSegment{
			Version: anyByte(r), SharingNotice: anyByte(r), SaleOptOutNotice: anyByte(r),
			SharingOptOutNotice: anyByte(r), TargetedAdvertisingOptOutNotice: anyByte(r),
			SensitiveDataProcessingOptOutNotice: anyByte(r), SensitiveDataLimitUseNotice: anyByte(r),
			SaleOptOut: anyByte(r), SharingOptOut: anyByte(r), TargetedAdvertisingOptOut: anyByte(r),
			SensitiveDataProcessing: anyBytes(r, 12), KnownChildSensitiveDataConsents: anyBytes(r, 2),
			PersonalDataConsents: anyByte(r), MspaCoveredTransaction: anyByte(r),
			MspaOptOutOptionMode: anyByte(r), MspaServiceProviderMode: anyByte(r),
		}
		desc := strings.Join(append(nums(c.Version, c.SharingNotice, c.SaleOptOutNotice,
			c.SharingOptOutNotice, c.TargetedAdvertisingOptOutNotice,
			c.SensitiveDataProcessingOptOutNotice, c.SensitiveDataLimitUseNotice, c.SaleOptOut,
			c.SharingOptOut, c.TargetedAdvertisingOptOut), dotted(c.SensitiveDataProcessing),
			dotted(c.KnownChildSensitiveDataConsents), strconv.Itoa(int(c.PersonalDataConsents)),
			strconv.Itoa(int(c.MspaCoveredTransaction)), strconv.Itoa(int(c.MspaOptOutOptionMode)),
			strconv.Itoa(int(c.MspaServiceProviderMode)), gpcDesc), ",")
		return uspnat.USPNAT{SectionID: id, CoreSegment: c, GPCSegment: gpc},
			fmt.Sprintf("nat:%d:%s", id, desc)
	case 1:
		id := randomEncodeID(r, 8)
		c := uspca.USPCACoreSegment{
			Version: anyByte(r), SaleOptOutNotice: anyByte(r), SharingOptOutNotice: anyByte(r),
			SensitiveDataLimitUseNotice: anyByte(r), SaleOptOut: anyByte(r),
			SharingOptOut: anyByte(r), SensitiveDataProcessing: anyBytes(r, 9),
			KnownChildSensitiveDataConsents: anyBytes(r, 2), PersonalDataConsents: anyByte(r),
			MspaCoveredTransaction: anyByte(r), MspaOptOutOptionMode: anyByte(r),
			MspaServiceProviderMode: anyByte(r),
		}
		desc := strings.Join(append(nums(c.Version, c.SaleOptOutNotice, c.SharingOptOutNotice,
			c.SensitiveDataLimitUseNotice, c.SaleOptOut, c.SharingOptOut),
			dotted(c.SensitiveDataProcessing), dotted(c.KnownChildSensitiveDataConsents),
			strconv.Itoa(int(c.PersonalDataConsents)), strconv.Itoa(int(c.MspaCoveredTransaction)),
			strconv.Itoa(int(c.MspaOptOutOptionMode)), strconv.Itoa(int(c.MspaServiceProviderMode)),
			gpcDesc), ",")
		return uspca.USPCA{SectionID: id, CoreSegment: c, GPCSegment: gpc},
			fmt.Sprintf("ca:%d:%s", id, desc)
	case 2:
		id := randomEncodeID(r, 9)
		c, desc := common(8, 1)
		return uspva.USPVA{SectionID: id, CoreSegment: c}, fmt.Sprintf("va:%d:%s", id, desc)
	case 3:
		id := randomEncodeID(r, 10)
		c, desc := common(7, 1)
		return uspco.USPCO{SectionID: id, CoreSegment: c, GPCSegment: gpc},
			fmt.Sprintf("co:%d:%s,%s", id, desc, gpcDesc)
	case 4:
		id := randomEncodeID(r, 11)
		c := usput.USPUTCoreSegment{
			Version: anyByte(r), SharingNotice: anyByte(r), SaleOptOutNotice: anyByte(r),
			TargetedAdvertisingOptOutNotice:     anyByte(r),
			SensitiveDataProcessingOptOutNotice: anyByte(r), SaleOptOut: anyByte(r),
			TargetedAdvertisingOptOut: anyByte(r), SensitiveDataProcessing: anyBytes(r, 8),
			KnownChildSensitiveDataConsents: anyByte(r), MspaCoveredTransaction: anyByte(r),
			MspaOptOutOptionMode: anyByte(r), MspaServiceProviderMode: anyByte(r),
		}
		desc := strings.Join(append(nums(c.Version, c.SharingNotice, c.SaleOptOutNotice,
			c.TargetedAdvertisingOptOutNotice, c.SensitiveDataProcessingOptOutNotice, c.SaleOptOut,
			c.TargetedAdvertisingOptOut), dotted(c.SensitiveDataProcessing),
			strconv.Itoa(int(c.KnownChildSensitiveDataConsents)),
			strconv.Itoa(int(c.MspaCoveredTransaction)), strconv.Itoa(int(c.MspaOptOutOptionMode)),
			strconv.Itoa(int(c.MspaServiceProviderMode))), ",")
		return usput.USPUT{SectionID: id, CoreSegment: c}, fmt.Sprintf("ut:%d:%s", id, desc)
	case 5:
		id := randomEncodeID(r, 12)
		c, desc := common(8, 3)
		return uspct.USPCT{SectionID: id, CoreSegment: c, GPCSegment: gpc},
			fmt.Sprintf("ct:%d:%s,%s", id, desc, gpcDesc)
	default:
		id := randomEncodeID(r, genericIDs[r.Intn(len(genericIDs))])
		value := randomB64(r, r.Intn(20))
		return rawSection{id: id, value: value}, fmt.Sprintf("g:%d:%s", id, value)
	}
}
