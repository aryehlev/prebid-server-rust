package main

import (
	"math/rand"
)

// bitWriter builds consent strings bit by bit (most significant bit first).
type bitWriter struct {
	bits []byte
}

func (w *bitWriter) put(v uint64, n int) {
	for i := n - 1; i >= 0; i-- {
		w.bits = append(w.bits, byte((v>>uint(i))&1))
	}
}

func (w *bitWriter) flag(b bool) {
	if b {
		w.put(1, 1)
	} else {
		w.put(0, 1)
	}
}

func (w *bitWriter) bytes() []byte {
	out := make([]byte, (len(w.bits)+7)/8)
	for i, b := range w.bits {
		if b == 1 {
			out[i/8] |= 0x80 >> uint(i%8)
		}
	}
	return out
}

type rangeEntry struct {
	isRange    bool
	start, end uint64
}

func putEntries(w *bitWriter, entries []rangeEntry) {
	for _, e := range entries {
		w.flag(e.isRange)
		w.put(e.start, 16)
		if e.isRange {
			w.put(e.end, 16)
		}
	}
}

// vendorSection is a TCF2 vendor consent or vendor legitimate interest section.
type vendorSection struct {
	maxVendorID uint64
	isRange     bool
	bitfield    []bool
	entries     []rangeEntry
	// numEntries overrides the encoded entry count when >= 0.
	numEntries int
}

func (v *vendorSection) encode(w *bitWriter) {
	w.put(v.maxVendorID, 16)
	w.flag(v.isRange)
	if v.isRange {
		n := uint64(len(v.entries))
		if v.numEntries >= 0 {
			n = uint64(v.numEntries)
		}
		w.put(n, 12)
		putEntries(w, v.entries)
		return
	}
	for _, b := range v.bitfield {
		w.flag(b)
	}
}

type restriction struct {
	purpose, rtype uint64
	entries        []rangeEntry
	numEntries     int
}

// tcf2Spec describes every field of a TCF 2 core segment.
type tcf2Spec struct {
	version, created, updated   uint64
	cmpID, cmpVersion, screen   uint64
	lang, vlv, policy           uint64
	serviceSpecific, nonStd     bool
	specialFeatures, purposes   uint64
	purposesLI                  uint64
	p1Treatment                 bool
	publisherCC                 uint64
	consents, legitInts         vendorSection
	restrictions                []restriction
	numRestrictions             int
	extraBits                   []byte
	omitLegitInts, omitPubRestr bool
}

func (s *tcf2Spec) encode() []byte {
	w := &bitWriter{}
	w.put(s.version, 6)
	w.put(s.created, 36)
	w.put(s.updated, 36)
	w.put(s.cmpID, 12)
	w.put(s.cmpVersion, 12)
	w.put(s.screen, 6)
	w.put(s.lang, 12)
	w.put(s.vlv, 12)
	w.put(s.policy, 6)
	w.flag(s.serviceSpecific)
	w.flag(s.nonStd)
	w.put(s.specialFeatures, 12)
	w.put(s.purposes, 24)
	w.put(s.purposesLI, 24)
	w.flag(s.p1Treatment)
	w.put(s.publisherCC, 12)
	s.consents.encode(w)
	if !s.omitLegitInts {
		s.legitInts.encode(w)
	}
	if !s.omitPubRestr {
		n := uint64(len(s.restrictions))
		if s.numRestrictions >= 0 {
			n = uint64(s.numRestrictions)
		}
		w.put(n, 12)
		for _, r := range s.restrictions {
			w.put(r.purpose, 6)
			w.put(r.rtype, 2)
			ne := uint64(len(r.entries))
			if r.numEntries >= 0 {
				ne = uint64(r.numEntries)
			}
			w.put(ne, 12)
			putEntries(w, r.entries)
		}
	}
	w.bits = append(w.bits, s.extraBits...)
	return w.bytes()
}

// tcf1Spec describes a TCF 1.1 consent string.
type tcf1Spec struct {
	version, created, updated uint64
	cmpID, cmpVersion, screen uint64
	lang, vlv, purposes       uint64
	maxVendorID               uint64
	isRange                   bool
	bitfield                  []bool
	defaultConsent            bool
	entries                   []rangeEntry
	numEntries                int
	extraBits                 []byte
}

func (s *tcf1Spec) encode() []byte {
	w := &bitWriter{}
	w.put(s.version, 6)
	w.put(s.created, 36)
	w.put(s.updated, 36)
	w.put(s.cmpID, 12)
	w.put(s.cmpVersion, 12)
	w.put(s.screen, 6)
	w.put(s.lang, 12)
	w.put(s.vlv, 12)
	w.put(s.purposes, 24)
	w.put(s.maxVendorID, 16)
	w.flag(s.isRange)
	if s.isRange {
		w.flag(s.defaultConsent)
		n := uint64(len(s.entries))
		if s.numEntries >= 0 {
			n = uint64(s.numEntries)
		}
		w.put(n, 12)
		putEntries(w, s.entries)
	} else {
		for _, b := range s.bitfield {
			w.flag(b)
		}
	}
	w.bits = append(w.bits, s.extraBits...)
	return w.bytes()
}

var interestingMaxIDs = []uint64{0, 1, 2, 3, 4, 5, 7, 8, 9, 10, 11, 15, 16, 17, 18, 23, 24, 25, 31, 32, 33, 63, 64, 65, 100, 127, 128, 129, 255, 256, 257, 511, 512, 1000, 1023, 1024, 1025}

var largeMaxIDs = []uint64{2000, 4095, 4096, 8191, 8192, 16383, 32766, 32767, 32768, 40000, 65534, 65535}

func randomBits(r *rand.Rand, n uint64) []bool {
	bits := make([]bool, n)
	mode := r.Intn(5)
	for i := range bits {
		switch mode {
		case 0:
			bits[i] = false
		case 1:
			bits[i] = true
		case 2:
			bits[i] = r.Intn(20) == 0
		case 3:
			bits[i] = r.Intn(2) == 0
		default:
			bits[i] = (i/7)%2 == 0
		}
	}
	return bits
}

// randomEntries returns valid range entries for vendor ids in [1, max].
func randomEntries(r *rand.Rand, max uint64, count int) []rangeEntry {
	var entries []rangeEntry
	if max == 0 {
		return nil
	}
	for i := 0; i < count; i++ {
		if max >= 2 && r.Intn(2) == 0 {
			start := uint64(r.Int63n(int64(max-1))) + 1
			end := start + 1 + uint64(r.Int63n(int64(max-start)))
			entries = append(entries, rangeEntry{isRange: true, start: start, end: end})
		} else {
			id := uint64(r.Int63n(int64(max))) + 1
			entries = append(entries, rangeEntry{start: id, end: id})
		}
	}
	return entries
}

func pickMax(r *rand.Rand, allowLarge bool) uint64 {
	switch {
	case allowLarge && r.Intn(4) == 0:
		return largeMaxIDs[r.Intn(len(largeMaxIDs))]
	case r.Intn(2) == 0:
		return interestingMaxIDs[r.Intn(len(interestingMaxIDs))]
	default:
		return uint64(r.Intn(1200))
	}
}

func randomSection(r *rand.Rand) vendorSection {
	if r.Intn(2) == 0 {
		max := pickMax(r, false)
		return vendorSection{maxVendorID: max, bitfield: randomBits(r, max), numEntries: -1}
	}
	max := pickMax(r, true)
	return vendorSection{maxVendorID: max, isRange: true, entries: randomEntries(r, max, r.Intn(14)), numEntries: -1}
}

func randomRestrictions(r *rand.Rand) []restriction {
	n := []int{0, 0, 0, 1, 1, 2, 3, 5}[r.Intn(8)]
	var out []restriction
	for i := 0; i < n; i++ {
		max := uint64(32767)
		if r.Intn(2) == 0 {
			max = uint64(1 + r.Intn(300))
		}
		out = append(out, restriction{
			purpose:    uint64(r.Intn(64)),
			rtype:      uint64(r.Intn(4)),
			entries:    randomEntries(r, max, r.Intn(6)),
			numEntries: -1,
		})
	}
	return out
}

func randomTimestamp(r *rand.Rand) uint64 {
	if r.Intn(4) == 0 {
		return r.Uint64() & (1<<36 - 1)
	}
	// Deciseconds between 2018 and 2026.
	return 15_200_000_000 + uint64(r.Int63n(2_600_000_000))
}

func randomTCF2(r *rand.Rand) *tcf2Spec {
	s := &tcf2Spec{
		version:         2,
		created:         randomTimestamp(r),
		updated:         randomTimestamp(r),
		cmpID:           uint64(r.Intn(4096)),
		cmpVersion:      uint64(r.Intn(4096)),
		screen:          uint64(r.Intn(64)),
		lang:            uint64(r.Intn(4096)),
		vlv:             uint64(1 + r.Intn(4095)),
		policy:          []uint64{2, 2, 4, 4, 5, 0, 1, 3, 63}[r.Intn(9)],
		serviceSpecific: r.Intn(2) == 0,
		nonStd:          r.Intn(2) == 0,
		specialFeatures: uint64(r.Intn(1 << 12)),
		purposes:        uint64(r.Intn(1 << 24)),
		purposesLI:      uint64(r.Intn(1 << 24)),
		p1Treatment:     r.Intn(2) == 0,
		publisherCC:     uint64(r.Intn(4096)),
		consents:        randomSection(r),
		legitInts:       randomSection(r),
		restrictions:    randomRestrictions(r),
		numRestrictions: -1,
	}
	if r.Intn(3) == 0 {
		for i := r.Intn(24); i > 0; i-- {
			s.extraBits = append(s.extraBits, byte(r.Intn(2)))
		}
	}
	return s
}

func randomTCF1(r *rand.Rand) *tcf1Spec {
	s := &tcf1Spec{
		version:    1,
		created:    randomTimestamp(r),
		updated:    randomTimestamp(r),
		cmpID:      uint64(r.Intn(4096)),
		cmpVersion: uint64(r.Intn(4096)),
		screen:     uint64(r.Intn(64)),
		lang:       uint64(r.Intn(4096)),
		vlv:        uint64(1 + r.Intn(4095)),
		purposes:   uint64(r.Intn(1 << 24)),
		numEntries: -1,
	}
	if r.Intn(2) == 0 {
		s.maxVendorID = 1 + pickMax(r, false)
		s.bitfield = randomBits(r, s.maxVendorID)
	} else {
		s.maxVendorID = 1 + pickMax(r, true)
		if s.maxVendorID > 65535 {
			s.maxVendorID = 65535
		}
		s.isRange = true
		s.defaultConsent = r.Intn(2) == 0
		s.entries = randomEntries(r, s.maxVendorID, r.Intn(10))
	}
	if r.Intn(3) == 0 {
		for i := r.Intn(24); i > 0; i-- {
			s.extraBits = append(s.extraBits, byte(r.Intn(2)))
		}
	}
	return s
}

// baseTCF2 is a small valid TCF2 spec used as a template for hand-written edge cases.
func baseTCF2() *tcf2Spec {
	return &tcf2Spec{
		version:         2,
		created:         15_800_000_000,
		updated:         15_800_000_123,
		cmpID:           3,
		cmpVersion:      2,
		screen:          7,
		lang:            4*64 + 13,
		vlv:             14,
		policy:          2,
		specialFeatures: 0x800,
		purposes:        0x2D2D6B,
		purposesLI:      0xFFC000,
		publisherCC:     0x512,
		consents:        vendorSection{maxVendorID: 3, bitfield: []bool{true, false, true}, numEntries: -1},
		legitInts:       vendorSection{maxVendorID: 3, bitfield: []bool{false, true, true}, numEntries: -1},
		numRestrictions: -1,
	}
}

func baseTCF1() *tcf1Spec {
	return &tcf1Spec{
		version:     1,
		created:     15_200_000_000,
		updated:     15_200_000_456,
		cmpID:       3,
		cmpVersion:  2,
		screen:      7,
		lang:        4*64 + 13,
		vlv:         14,
		purposes:    0x2D2D6B,
		maxVendorID: 3,
		bitfield:    []bool{true, false, true},
		numEntries:  -1,
	}
}
