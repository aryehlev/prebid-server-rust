package main

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/cespare/xxhash/v2"
	"github.com/coocood/freecache"
)

// The adversarial goldens target what the first scripts reach rarely: one slot holding
// hundreds of entries (slotCap growth, long sorted runs, equal 16-bit tags deep inside a slot),
// a clock that advances while the cache runs, iteration interleaved with writes, the entry
// size limit across many cache sizes, and RingBuf calls outside the cache's access pattern.

// deepSeed seeds the search for deep keys.
const deepSeed = 0xD33950175107

// deepTarget is the low 16 bits every deep key hashes to: segment 0x07, slot 0x2A.
const deepTarget = 0x2A07

// deepCount is the number of deep keys; among this many, several pairs share their 16-bit
// tag as well.
const deepCount = 1500

// findDeepKeys returns deepCount distinct keys of 1 to 40 bytes whose Sum64 has the low 16
// bits deepTarget, so that they all land in the same segment and slot.
func findDeepKeys() [][]byte {
	rng := splitmix64{state: deepSeed}
	seen := map[string]bool{}
	var found [][]byte
	for len(found) < deepCount {
		key := make([]byte, 1+rng.below(40))
		rng.fill(key)
		if xxhash.Sum64(key)&0xFFFF != deepTarget || seen[string(key)] {
			continue
		}
		seen[string(key)] = true
		found = append(found, key)
	}
	return found
}

// adversarialWeights is mixedWeights with partial iteration steps.
var adversarialWeights = func() [opCount]int {
	weights := mixedWeights
	weights[opIterStep] = 60
	return weights
}()

// adversarialSpecs are appended to scriptSpecs.
var adversarialSpecs = []scriptSpec{
	{
		name: "deep_slot", seed: 11, cacheSize: 8 << 20, ops: 40000, batch: 1000, stateEvery: 2,
		clockStart: 1700000000, keys: 50, keyMaxLen: 16, smallVal: 60, ttlMax: 20, tickMax: 2,
		collide: true, deepKeys: deepCount, smallMix: true, weights: adversarialWeights,
	},
	{
		name: "deep_slot_tiny", seed: 12, cacheSize: 0, ops: 30000, batch: 1000, stateEvery: 1,
		clockStart: 1700000000, keyMaxLen: 1, smallVal: 30, ttlMax: 6, tickMax: 1,
		backPermille: 20, deepKeys: deepCount, smallMix: true, weights: adversarialWeights,
	},
	{
		name: "ticking_small", seed: 13, cacheSize: 0, ops: 40000, batch: 1000, stateEvery: 1,
		clockStart: 1700000000, keys: 3000, keyMaxLen: 20, smallVal: 100, ttlMax: 10,
		tickMax: 1, collide: true, huge: true, tickEvery: 3, weights: adversarialWeights,
	},
	{
		name: "ticking_deep", seed: 14, cacheSize: 1 << 20, ops: 30000, batch: 1000,
		stateEvery: 1, clockStart: 0xFFFFFFF0, keys: 20, keyMaxLen: 8, smallVal: 40, ttlMax: 5,
		tickMax: 1, backPermille: 100, deepKeys: 800, smallMix: true, tickEvery: 5,
		weights: adversarialWeights,
	},
	{
		name: "iter_mutate", seed: 15, cacheSize: 524288, ops: 30000, batch: 1000, stateEvery: 1,
		clockStart: 1700000000, keys: 400, keyMaxLen: 10, smallVal: 80, ttlMax: 8, tickMax: 2,
		collide: true,
		weights: w(opSet, 300, opGet, 200, opDel, 100, opTouch, 50, opIterStep, 300, opClear, 3,
			opIterate, 5, opTick, 100, opGetOrSet, 30, opSetAndGet, 30),
	},
}

func init() {
	scriptSpecs = append(scriptSpecs, adversarialSpecs...)
}

// limitSizes are NewCache sizes around the 512 KiB minimum, the 256 segments, the factor 1024
// and the 65535-byte key limit.
var limitSizes = []int{
	-1, 0, 1, 524287, 524288, 524289, 524288 + 255, 524288 + 256, 524288 + 1023, 524288 + 1024,
	524288 + 1025, 600000, 1<<20 - 1, 1 << 20, 1<<20 + 1, 3000001, 64<<20 - 1, 64 << 20,
	64<<20 + 24*1024, 70 << 20, 70<<20 + 12345,
}

// limitKeyLens are the key lengths probed for every size.
var limitKeyLens = []int{0, 1, 8, 100, 487, 488, 489, 4096, 65511, 65512, 65535, 65536}

// maxAccepted returns the longest value Set accepts with a key of keyLen bytes, found by
// bisection, or -1 with the error for an empty value.
func maxAccepted(c *freecache.Cache, keyLen int) string {
	key := make([]byte, keyLen)
	if err := c.Set(key, nil, 0); err != nil {
		return "-1 " + errStr(err)
	}
	lo, hi := 0, 1<<17
	for lo < hi {
		mid := (lo + hi + 1) / 2
		if c.Set(key, make([]byte, mid), 0) == nil {
			lo = mid
		} else {
			hi = mid - 1
		}
	}
	return strconv.Itoa(lo) + " " + errStr(c.Set(key, make([]byte, lo+1), 0))
}

// writeLimits records, for every size and key length, the longest value Set accepts and the
// error for one byte more.
func writeLimits(g *golden) {
	defer g.close()
	for _, size := range limitSizes {
		c := freecache.NewCacheCustomTimer(size, &fakeTimer{now: scenarioClockStart})
		var results []string
		for _, keyLen := range limitKeyLens {
			results = append(results, fmt.Sprintf("k%d:%s", keyLen, maxAccepted(c, keyLen)))
		}
		g.line("limit", strconv.Itoa(size), strings.Join(results, ","), counters(c))
	}
}

// ringFuzzRuns is the number of independent RingBuf fuzz runs; each draws its own size.
const ringFuzzRuns = 200

// ringFuzzOps is the number of calls per run.
const ringFuzzOps = 400

// ringFuzzSeed seeds the RingBuf fuzz.
const ringFuzzSeed = 0x52494E47

// ringOff draws a stream offset around [begin, end], a few bytes either side.
func ringOff(rng *splitmix64, rb *freecache.RingBuf) int64 {
	span := rb.End() - rb.Begin() + 7
	return rb.Begin() - 3 + int64(rng.below(uint64(span)))
}

// ringFuzzStep makes one RingBuf call with arguments inside the domain on which Go neither
// panics nor loops (lengths up to the buffer size, Evacuate inside the stream) and renders it.
func ringFuzzStep(rng *splitmix64, rb *freecache.RingBuf) string {
	size := rb.Size()
	switch rng.below(10) {
	case 0, 1:
		p := make([]byte, rng.below(uint64(size)+3))
		rng.fill(p)
		n, err := rb.Write(p)
		return fmt.Sprintf("write %d %d %v", len(p), n, err)
	case 2:
		off := ringOff(rng, rb)
		p := make([]byte, rng.below(uint64(size)+3))
		rng.fill(p)
		n, err := rb.WriteAt(p, off)
		return fmt.Sprintf("writeat %d %d %d %v", off, len(p), n, err)
	case 3:
		off := ringOff(rng, rb)
		p := make([]byte, rng.below(uint64(size)+3))
		n, err := rb.ReadAt(p, off)
		return fmt.Sprintf("readat %d %d %d %v %s", off, len(p), n, err, bytesDigest(p))
	case 4:
		off := ringOff(rng, rb)
		length := int64(rng.below(uint64(size) + 1))
		s, err := rb.Slice(off, length)
		return fmt.Sprintf("slice %d %d %d %v %s", off, length, len(s), err, bytesDigest(s))
	case 5:
		off := ringOff(rng, rb)
		p := make([]byte, rng.below(uint64(size)+3))
		// Half the probes copy the stream, so that some of them match.
		if rng.below(2) == 0 {
			_, _ = rb.ReadAt(p, off)
		} else {
			rng.fill(p)
		}
		return fmt.Sprintf("equalat %d %d %v", off, len(p), rb.EqualAt(p, off))
	case 6, 7:
		var off, length int64
		if rng.below(8) == 0 {
			off = ringOff(rng, rb)
			length = int64(rng.below(uint64(size) + 3))
			if off < rb.Begin() || off+length > rb.End() {
				return fmt.Sprintf("evacuate %d %d %d", off, length, rb.Evacuate(off, int(length)))
			}
		} else {
			off = rb.Begin() + int64(rng.below(uint64(rb.End()-rb.Begin())+1))
			length = int64(rng.below(uint64(rb.End()-off) + 1))
		}
		return fmt.Sprintf("evacuate %d %d %d", off, length, rb.Evacuate(off, int(length)))
	case 8:
		length := int64(rng.below(2*uint64(size) + 1))
		rb.Skip(length)
		return fmt.Sprintf("skip %d", length)
	default:
		if rng.below(4) == 0 {
			begin := int64(rng.below(100))
			rb.Reset(begin)
			return fmt.Sprintf("reset %d", begin)
		}
		newSize := 1 + int(rng.below(48))
		rb.Resize(newSize)
		return fmt.Sprintf("resize %d", newSize)
	}
}

// writeRingFuzz records random RingBuf call sequences: per run, a digest of every call's
// rendering followed by the buffer state and data, and the final state.
func writeRingFuzz(g *golden) {
	defer g.close()
	rng := splitmix64{state: ringFuzzSeed}
	for run := 0; run < ringFuzzRuns; run++ {
		size := 1 + int(rng.below(40))
		begin := int64(rng.below(50))
		rb := freecache.NewRingBuf(size, begin)
		f := newFnv()
		for op := 0; op < ringFuzzOps; op++ {
			f.str(ringFuzzStep(&rng, &rb) + " " + rb.String() + "\n")
			f.bytes(rb.Dump())
		}
		g.line("ringfuzz", strconv.Itoa(run), strconv.Itoa(size), strconv.FormatInt(begin, 10),
			f.hex(), rb.String())
	}
}
