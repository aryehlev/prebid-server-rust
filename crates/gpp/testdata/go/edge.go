package main

// edge.txt: cases aimed at go-gpp corners that the other golden files filter out or rarely
// reach. They cover Fibonacci codes of 25 to 58 bits, whose values wrap in uint16. They cover
// headers whose wrapping offsets produce section IDs that go backwards, repeat, or are 0. They
// cover inputs and bit streams of 8 to 128 KiB, where go-gpp's uint16(len(data)) bounds checks
// truncate and its uint16 bit position wraps at 65535. And they cover GPC sub-segment shapes and
// header and section counts in the tens of thousands.
//
// Each input is a recipe: prefix + unit repeated count times + suffix. That keeps inputs of
// many kilobytes small on disk. Strings longer than shortLimit bytes are written as
// "#<length>:<FNV-1a 64 hex>" instead of in full, and so is a whole result longer than
// longLimit bytes.
//
//	P \t <esc prefix> \t <count> \t <esc unit> \t <esc suffix> \t <dumpParseQ with short>
//	B \t <op> \t <hex prefix> \t <count> \t <hex unit> \t <hex suffix> \t <bit offset> \t <runOp>

import (
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"hash/fnv"
	"math/rand"
	"strconv"
	"strings"

	gpp "github.com/prebid/go-gpp"
	"github.com/prebid/go-gpp/constants"
	"github.com/prebid/go-gpp/util"
)

const (
	edgeSeed   = 20261117
	shortLimit = 200
	longLimit  = 4096
	// maxFibIndex bounds the largest fibonacci(i) go-gpp may compute in one case. Its
	// recursion takes about 0.4s at i = 58.
	maxFibIndex = 58
)

func fnvHash(s string) string {
	h := fnv.New64a()
	if _, err := h.Write([]byte(s)); err != nil {
		panic(err)
	}
	return fmt.Sprintf("#%d:%016x", len(s), h.Sum64())
}

// short is esc for strings of at most shortLimit bytes and fnvHash for longer ones.
func short(s string) string {
	if len(s) <= shortLimit {
		return esc(s)
	}
	return fnvHash(s)
}

func capped(s string) string {
	if len(s) <= longLimit {
		return s
	}
	return fnvHash(s)
}

func edgeGolden(r *rand.Rand) []string {
	e := &edgeCases{}
	addIDSequenceCases(e, r, 700)
	addLongFibBitCases(e, r, 400)
	addHugeHeaderCases(e, r)
	addHugeSectionCases(e, r)
	addManySectionCases(e, r)
	addWrapBitCases(e, r)
	addSubSegmentCases(e, r)
	addMiscCases(e)
	fmt.Printf("edge.txt: skipped %d cases go-gpp would not finish\n", e.skipped)
	return e.lines
}

type edgeCases struct {
	lines   []string
	seen    map[string]bool
	skipped int
}

func (e *edgeCases) parse(prefix string, count int, unit, suffix string) {
	key := "P\t" + prefix + "\t" + strconv.Itoa(count) + "\t" + unit + "\t" + suffix
	if e.seen == nil {
		e.seen = map[string]bool{}
	}
	if e.seen[key] {
		return
	}
	e.seen[key] = true
	input := prefix + strings.Repeat(unit, count) + suffix
	if !parseSafe(input) {
		e.skipped++
		return
	}
	e.lines = append(e.lines, strings.Join([]string{"P", esc(prefix), strconv.Itoa(count), esc(unit),
		esc(suffix), capped(dumpParseQ(input, short))}, "\t"))
}

func (e *edgeCases) bits(op string, prefix []byte, count int, unit, suffix []byte, offset uint16) {
	data := append(append(append([]byte{}, prefix...), repeatBytes(unit, count)...), suffix...)
	if (op == "fib" && !fibSafe(data, offset)) || (op == "fibr" && !rangeSafe(data, offset)) {
		e.skipped++
		return
	}
	e.lines = append(e.lines, strings.Join([]string{"B", op, hex.EncodeToString(prefix),
		strconv.Itoa(count), hex.EncodeToString(unit), hex.EncodeToString(suffix),
		strconv.Itoa(int(offset)), capped(runOp(op, data, offset))}, "\t"))
}

func repeatBytes(unit []byte, count int) []byte {
	out := make([]byte, 0, len(unit)*count)
	for i := 0; i < count; i++ {
		out = append(out, unit...)
	}
	return out
}

// ---------------------------------------------------------------------------------------------
// Safety: only cases that go-gpp finishes quickly are recorded.

// fib16 is go-gpp's fibonacci(i) (uint16, wrapping), computed in linear time.
var fib16 = func() []uint16 {
	f := make([]uint16, 70010)
	f[1] = 1
	for i := 2; i < len(f); i++ {
		f[i] = f[i-1] + f[i-2]
	}
	return f
}()

// scanFib mirrors util.ReadFibonacciInt without its exponential recursion. It returns the
// value, the number of bits read, the largest fibonacci(i) index go-gpp would compute, whether
// the read succeeded, and whether it terminates at all.
func scanFib(data []byte, pos uint16) (value uint16, read uint16, maxIdx int, ok, finite bool) {
	last, err := util.ParseByte1(data, pos)
	if err != nil {
		return 0, 0, 0, false, true
	}
	next, err := util.ParseByte1(data, pos+1)
	if err != nil {
		return 0, 1, 0, false, true
	}
	value = uint16(last)
	i := 3
	for ; last == 0 || next == 0; i++ {
		if i > 70000 {
			return 0, 0, maxIdx, false, false
		}
		last = next
		next, err = util.ParseByte1(data, pos+uint16(i-1))
		if err != nil {
			return 0, uint16(i - 1), maxIdx, false, true
		}
		if last == 1 {
			value += fib16[i]
			maxIdx = i
		}
	}
	return value, uint16(i - 1), maxIdx, true, true
}

func fibSafe(data []byte, pos uint16) bool {
	_, _, maxIdx, _, finite := scanFib(data, pos)
	return finite && maxIdx <= maxFibIndex
}

// rangeSafe mirrors util.ReadFibonacciRange at pos and then gpp.Parse's ID expansion: it rejects
// slow Fibonacci codes, ranges ending at 65535 (Parse never terminates) and huge ID counts.
func rangeSafe(data []byte, pos uint16) bool {
	n, err := util.ParseUInt12(data, pos)
	if err != nil {
		return true
	}
	pos += 12
	var maxValue uint16
	total := 0
	readInt := func() (uint16, bool, bool) {
		v, read, maxIdx, ok, finite := scanFib(data, pos)
		pos += read
		return v, ok, finite && maxIdx <= maxFibIndex
	}
	for i := 0; i < int(n); i++ {
		flag, err := util.ParseByte1(data, pos)
		if err != nil {
			return true
		}
		pos++
		first, ok, safe := readInt()
		if !safe {
			return false
		}
		if !ok {
			return true
		}
		if flag == 0 {
			entry := first + maxValue
			if entry == 0xffff {
				return false
			}
			total++
			if entry > maxValue {
				maxValue = entry
			}
			continue
		}
		second, ok, safe := readInt()
		if !safe {
			return false
		}
		if !ok {
			return true
		}
		start := maxValue + first
		end := start + second
		if end == 0xffff {
			return false
		}
		if start <= end {
			total += int(end-start) + 1
		}
		if end > maxValue {
			maxValue = end
		}
	}
	return total <= 300000
}

// decodeLikeGo decodes s exactly like util.NewBitStreamFromBase64, but keeps all bytes (the
// exported BitStream API can only reach the first 8 KiB).
func decodeLikeGo(s string) ([]byte, error) {
	if len(s)%4 > 0 {
		s += "A"
	}
	return base64.RawURLEncoding.DecodeString(s)
}

func parseSafe(input string) bool {
	header := strings.SplitN(input, "~", 2)[0]
	data, err := decodeLikeGo(header)
	if err != nil || uint16(len(data)) < gpp.MaxHeaderLength || header[0] != gpp.SectionGPPByte {
		return true
	}
	return rangeSafe(data, 12)
}

// ---------------------------------------------------------------------------------------------
// Header construction.

type bitWriter struct{ bits []byte }

func (w *bitWriter) put(v uint64, n int) {
	for i := n - 1; i >= 0; i-- {
		w.bits = append(w.bits, byte(v>>uint(i))&1)
	}
}

func (w *bitWriter) code(c []byte) { w.bits = append(w.bits, c...) }

func (w *bitWriter) bytes() []byte {
	out := make([]byte, (len(w.bits)+7)/8)
	for i, b := range w.bits {
		if b == 1 {
			out[i/8] |= 0x80 >> uint(i%8)
		}
	}
	return out
}

func (w *bitWriter) base64() string { return base64.RawURLEncoding.EncodeToString(w.bytes()) }

// fibCode returns the Fibonacci code of v >= 1: bit k stands for fib(k+2), and a final 1
// terminates the code.
func fibCode(v uint64) []byte {
	fibs := []uint64{1, 2}
	for fibs[len(fibs)-1] <= v {
		fibs = append(fibs, fibs[len(fibs)-1]+fibs[len(fibs)-2])
	}
	bits := make([]byte, len(fibs))
	top := -1
	for k := len(fibs) - 1; k >= 0; k-- {
		if fibs[k] <= v {
			bits[k] = 1
			v -= fibs[k]
			if top < 0 {
				top = k
			}
		}
	}
	return append(bits[:top+1], 1)
}

// longCode returns a valid Fibonacci code of exactly n >= 28 bits whose value, summed in uint16
// the way go-gpp does, is want. Bits 25..n-2 are random (the top one set), and bits 0..22 hold
// the Zeckendorf form of the remainder.
func longCode(r *rand.Rand, n int, want uint16) []byte {
	code := make([]byte, n)
	code[n-1], code[n-2] = 1, 1
	for k := 25; k <= n-4; k++ {
		if code[k-1] == 0 && r.Intn(2) == 0 {
			code[k] = 1
		}
	}
	var high uint16
	for k := 25; k <= n-2; k++ {
		if code[k] == 1 {
			high += fib16[k+2]
		}
	}
	if low := want - high; low != 0 {
		lc := fibCode(uint64(low))
		copy(code, lc[:len(lc)-1])
	}
	return code
}

func randomLongLength(r *rand.Rand) int {
	switch r.Intn(10) {
	case 0:
		return 53 + r.Intn(maxFibIndex-52)
	case 1, 2, 3:
		return 41 + r.Intn(12)
	default:
		return 28 + r.Intn(13)
	}
}

// offsetCode encodes an offset whose uint16 value is v: as its own Fibonacci code, as the code of
// v + 65536 (which wraps to v; the only way to write 0), or as a long code.
func offsetCode(r *rand.Rand, v uint16, longP int) []byte {
	if r.Intn(100) < longP {
		return longCode(r, randomLongLength(r), v)
	}
	if v == 0 || r.Intn(6) == 0 {
		return fibCode(uint64(v) + 65536)
	}
	return fibCode(uint64(v))
}

// idHeader writes a header whose Fibonacci range expands to ids, in order. Offsets wrap past
// 65535 to go backwards or to repeat the running maximum. Runs of consecutive IDs sometimes
// become range entries.
func idHeader(r *rand.Rand, version byte, ids []uint16, longP int) string {
	type entry struct {
		isRange    bool
		start, end uint16
	}
	var entries []entry
	for i := 0; i < len(ids); {
		j := i + 1
		for j < len(ids) && ids[j] == ids[j-1]+1 && ids[j] != 0 {
			j++
		}
		switch {
		case j-i > 1 && r.Intn(3) > 0:
			j = i + 2 + r.Intn(j-i-1)
			entries = append(entries, entry{true, ids[i], ids[j-1]})
		case r.Intn(8) == 0:
			j = i + 1
			entries = append(entries, entry{true, ids[i], ids[i]})
		default:
			j = i + 1
			entries = append(entries, entry{false, ids[i], ids[i]})
		}
		i = j
	}
	w := &bitWriter{}
	w.put(3, 6)
	w.put(uint64(version), 6)
	w.put(uint64(len(entries)), 12)
	var maxValue uint16
	for _, en := range entries {
		if !en.isRange {
			w.put(0, 1)
			w.code(offsetCode(r, en.start-maxValue, longP))
			if en.start > maxValue {
				maxValue = en.start
			}
			continue
		}
		w.put(1, 1)
		w.code(offsetCode(r, en.start-maxValue, longP))
		w.code(offsetCode(r, en.end-en.start, longP))
		if en.end > maxValue {
			maxValue = en.end
		}
	}
	return w.base64()
}

var edgeIDPool = []uint16{7, 8, 9, 10, 11, 12, 7, 8, 9, 10, 11, 12, 0, 1, 2, 3, 6, 13, 99, 6000,
	6764, 6765, 40000, 65533, 65534}

func randomID(r *rand.Rand) uint16 {
	if r.Intn(10) == 0 {
		return uint16(r.Intn(65535))
	}
	return edgeIDPool[r.Intn(len(edgeIDPool))]
}

// sectionFor returns a section string for id: a valid, broken or foreign string for the decoded
// US IDs, and a generic value otherwise.
func sectionFor(r *rand.Rand, id uint16) string {
	if id < 7 || id > 12 {
		return randomGenericValue(r)
	}
	s := randomUSSection(r, constants.SectionID(id))
	switch r.Intn(6) {
	case 0:
		return s[:r.Intn(len(s)+1)]
	case 1:
		return "!" + s
	case 2:
		return randomUSSection(r, usIDs[r.Intn(len(usIDs))])
	default:
		return s
	}
}

func addIDSequenceCases(e *edgeCases, r *rand.Rand, n int) {
	for i := 0; i < n; i++ {
		ids := make([]uint16, 1+r.Intn(8))
		for j := range ids {
			ids[j] = randomID(r)
			if j > 0 && r.Intn(4) == 0 && ids[j-1] < 65533 {
				ids[j] = ids[j-1] + 1
			}
		}
		longP := []int{0, 10, 40}[r.Intn(3)]
		header := idHeader(r, byte(r.Intn(64)), ids, longP)
		if r.Intn(10) == 0 {
			header = header[:r.Intn(len(header)+1)]
		}
		count := len(ids) + []int{0, 0, 0, 0, 0, 1, -1}[r.Intn(7)]
		parts := []string{header}
		for j := 0; j < count; j++ {
			id := uint16(0)
			if j < len(ids) {
				id = ids[j]
			}
			parts = append(parts, sectionFor(r, id))
		}
		e.parse(strings.Join(parts, "~"), 0, "", "")
	}
}

// ---------------------------------------------------------------------------------------------
// Long Fibonacci codes read directly from a bit stream.

func addLongFibBitCases(e *edgeCases, r *rand.Rand, n int) {
	for i := 0; i < n; i++ {
		w := &bitWriter{}
		lead := r.Intn(20)
		for j := 0; j < lead; j++ {
			w.put(uint64(r.Intn(2)), 1)
		}
		op := "fib"
		if r.Intn(3) == 0 {
			op = "fibr"
			entries := 1 + r.Intn(3)
			w.put(uint64(entries), 12)
			for j := 0; j < entries; j++ {
				isRange := r.Intn(2)
				w.put(uint64(isRange), 1)
				for k := 0; k <= isRange; k++ {
					w.code(offsetCode(r, uint16(r.Intn(65536)), 50))
				}
			}
		} else if n := 26 + r.Intn(maxFibIndex-25); n < 28 {
			w.code(fibCode(uint64(1 + r.Intn(121392))))
		} else {
			w.code(longCode(r, n, uint16(r.Intn(65536))))
		}
		for j := r.Intn(16); j > 0; j-- {
			w.put(uint64(r.Intn(2)), 1)
		}
		data := w.bytes()
		if r.Intn(6) == 0 {
			data = data[:r.Intn(len(data)+1)]
		}
		e.bits(op, data, 0, nil, nil, uint16(lead))
	}
}

// ---------------------------------------------------------------------------------------------
// Inputs of 8 KiB and more.

// charCounts lists base64 lengths around the decoded sizes 8192, 65536, 73728 and 131072 bytes.
func charCounts(lo, hi int) []int {
	var out []int
	for c := lo; c <= hi; c++ {
		out = append(out, c)
	}
	return out
}

var hugeCharCounts = append(append(append(charCounts(10919, 10926), charCounts(87375, 87393)...),
	charCounts(98301, 98307)...), charCounts(174759, 174766)...)

func addHugeHeaderCases(e *edgeCases, r *rand.Rand) {
	for _, c := range hugeCharCounts {
		for k := 0; k < 2; k++ {
			secs := randomSections(r)
			encoded := strings.SplitN(mustEncode(secs), "~", 2)
			if k == 1 {
				ids := make([]uint16, len(secs))
				for j := range ids {
					ids[j] = randomID(r)
				}
				encoded[0] = idHeader(r, byte(r.Intn(64)), ids, 10)
			}
			suffix := ""
			if len(encoded) > 1 {
				suffix = "~" + encoded[1]
			}
			if c > len(encoded[0]) {
				e.parse(encoded[0], c-len(encoded[0]), "A", suffix)
			}
		}
	}
}

func addHugeSectionCases(e *edgeCases, r *rand.Rand) {
	gpcUnits := []string{"Y", "Q", "g", "w", "A"}
	for _, id := range usIDs {
		header := strings.Split(mustEncode([]gpp.Section{rawSection{id: id}}), "~")[0] + "~"
		core := strings.Split(randomUSSection(r, id), ".")[0]
		for _, c := range append(charCounts(87376, 87388), charCounts(10920, 10925)...) {
			gpc := []string{"", "", ".YA", ".QA", ".A"}[r.Intn(5)]
			e.parse(header+core, c-len(core), "A", gpc)
			if r.Intn(2) == 0 {
				e.parse(header+core+"."+gpcUnits[r.Intn(len(gpcUnits))], c, "A", "")
			}
		}
		// A huge invalid tail, and a huge generic neighbour after a stale error.
		e.parse(header+core, 70000, "A", "!")
		pair := strings.Split(mustEncode([]gpp.Section{rawSection{id: id}, rawSection{id: 13}}),
			"~")[0] + "~"
		e.parse(pair+core+".YA~", 70000, "B", "")
		e.parse(pair+"!~", 70000, "B", "")
	}
}

func addManySectionCases(e *edgeCases, r *rand.Rand) {
	for _, n := range []int{2, 100, 1000, 4095, 4096, 6765, 10000, 30000, 65533, 65534} {
		for _, start := range []uint16{1, 7, 13} {
			if int(start)+n-1 >= 65535 {
				continue
			}
			w := &bitWriter{}
			w.put(3, 6)
			w.put(1, 6)
			w.put(1, 12)
			w.put(1, 1)
			w.code(offsetCode(r, start, 0))
			w.code(offsetCode(r, uint16(n-1), 0))
			header := w.base64()
			for _, delta := range []int{0, 1, -1} {
				unit := []string{"~", "~1YNN", "~BVVqAAAAAgA.QA"}[r.Intn(3)]
				e.parse(header, n+delta, unit, "")
			}
		}
	}
	// The largest range that still ends (65534) and a single section at 65534.
	for _, ids := range [][]uint16{{65534}, {65533, 65534}, {7, 65534}, {65534, 7}} {
		e.parse(idHeader(r, 1, ids, 0), len(ids), "~BVVqAAAAAgA.QA", "")
	}
}

// addWrapBitCases reads at every interesting offset of streams sized around 8 KiB and 64 KiB,
// where go-gpp compares against uint16(len(data)) and its bit position wraps at 65535.
func addWrapBitCases(e *edgeCases, r *rand.Rand) {
	sizes := []int{8191, 8192, 8193, 8200, 65535, 65536, 65537, 65538, 65539, 65540, 73727,
		73728, 73729}
	ops := []string{"b1", "b2", "b4", "b6", "b8", "u12", "u16", "tbf3", "tbf12", "intr", "fib",
		"fibr"}
	fibUnits := []byte{0x03, 0x06, 0x0c, 0x18, 0x30, 0x60, 0xc0, 0xff, 0x6d, 0xdb, 0x33}
	for _, size := range sizes {
		len16 := int(uint16(size))
		offsets := map[int]bool{0: true, 1: true, 7: true}
		for k := 0; k <= 16; k++ {
			offsets[65535-k] = true
			for _, base := range []int{len16 * 8, 65536} {
				if v := base - 1 - k; v >= 0 && v <= 65535 {
					offsets[v] = true
				}
			}
		}
		for k := 0; k < 4; k++ {
			offsets[r.Intn(65536)] = true
		}
		for _, op := range ops {
			prefix := make([]byte, 16)
			suffix := make([]byte, 16)
			r.Read(prefix)
			r.Read(suffix)
			unit := []byte{byte(r.Intn(256))}
			if op == "fib" || op == "fibr" {
				unit = []byte{fibUnits[r.Intn(len(fibUnits))]}
			} else if r.Intn(3) == 0 {
				unit = []byte{0}
			}
			for offset := 0; offset <= 65535; offset++ {
				if offsets[offset] {
					e.bits(op, prefix, size-32, unit, suffix, uint16(offset))
				}
			}
		}
	}
	// A terminator only after the position wraps: zeros up to bit 65535, then "11" at bit 0.
	for _, offset := range []uint16{65500, 65520, 65534, 65535} {
		e.bits("fib", []byte{0xc0}, 8191, []byte{0}, nil, offset)
		e.bits("fib", []byte{0x40}, 8191, []byte{0}, []byte{0x80}, offset)
		e.bits("fibr", []byte{0xd8}, 8191, []byte{0}, nil, offset)
	}
}

// ---------------------------------------------------------------------------------------------
// Section strings with every GPC sub-segment shape.

func addSubSegmentCases(e *edgeCases, r *rand.Rand) {
	extras := []string{"", ".", "..", ".YA", ".YA.", ".!", ".=", ".Y\n", "\n.YA", ".Y\r\n", ". YA",
		".YA.QA", ".YA..", ".+", "./", ".YA=", ".é"}
	for _, id := range usIDs {
		header := strings.Split(mustEncode([]gpp.Section{rawSection{id: id}}), "~")[0] + "~"
		for k := 0; k < 3; k++ {
			core := strings.Split(randomUSSection(r, id), ".")[0]
			for subsection := 0; subsection < 4; subsection++ {
				for gpcBit := 0; gpcBit < 2; gpcBit++ {
					for extraBits := 0; extraBits <= 13; extraBits += 1 + r.Intn(5) {
						w := &bitWriter{}
						w.put(uint64(subsection), 2)
						w.put(uint64(gpcBit), 1)
						for j := 0; j < extraBits; j++ {
							w.put(uint64(r.Intn(2)), 1)
						}
						e.parse(header+core+"."+w.base64(), 0, "", "")
					}
				}
			}
			for _, extra := range extras {
				e.parse(header+core+extra, 0, "", "")
			}
			// Core strings one character short or long of a full byte.
			for _, cut := range []int{1, 2, 3} {
				if cut < len(core) {
					e.parse(header+core[:len(core)-cut]+".YA", 0, "", "")
				}
			}
			e.parse(header+core+"A.YA", 0, "", "")
		}
	}
}

// ---------------------------------------------------------------------------------------------
// Whitespace, empty and US Privacy shapes.

func addMiscCases(e *edgeCases) {
	uspv1Header := strings.Split(mustEncode([]gpp.Section{rawSection{id: 6}}), "~")[0]
	tcfUSPHeader := strings.Split(mustEncode([]gpp.Section{rawSection{id: 2}, rawSection{id: 6}}),
		"~")[0]
	for _, v := range []string{"1YNN", "1YN", "1YNNN", "2YNN", "1ZNN", "1yNN", "", " 1YNN", "1YNN ",
		"1Y\x00N", "1YéN", "1---", "----", "1YN-", "1\tNN", "é", "1YNN.YA", "1YNN\n"} {
		e.parse(uspv1Header+"~"+v, 0, "", "")
		e.parse(tcfUSPHeader+"~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA~"+v, 0, "", "")
	}
	for _, s := range []string{"", " ", "~", "~~~", "~DBABMA", " DBABMA~1YNN", "DBABMA ~1YNN",
		"DBABMA~1YNN ", "\tDBABMA~1YNN", "DBABMA\t~1YNN", "DBABMA~ 1YNN", "DBABMA ~1YNN",
		"DBABMA==~1YNN", "DBABMA=~1YNN", "DBABM~", "DBAB~", "DBA~", "DB~", "D~", "D", "Da", "DBABMA~1YNN~",
		"DBABMA\r\n~1YNN", "\r\nDBABMA~1YNN", "DBA\nBMA~1YNN", "DBABMA\n\n\n\n~1YNN",
		"DBABMA\nA~1YNN", "DBABMAA\n~1YNN", "DBABMAAA\n\n~1YNN", "DBABMAAAA\nA~1YNN"} {
		e.parse(s, 0, "", "")
	}
}
