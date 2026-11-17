package main

import (
	"bytes"
	"errors"
	"fmt"
	"math"
	"reflect"
	"strconv"
	"strings"

	"github.com/cespare/xxhash/v2"
	"github.com/coocood/freecache"
)

// The operations a script draws from, by index into scriptSpec.weights.
const (
	opSet = iota
	opGet
	opGetWithExpiration
	opPeek
	opDel
	opTouch
	opTTL
	opGetOrSet
	opSetAndGet
	opGetFn
	opPeekFn
	opGetWithBuf
	opSetInt
	opGetInt
	opGetIntWithExpiration
	opDelInt
	opTick
	opClear
	opResetStatistics
	opIterate
	opIterStep
	opCount
)

// scriptSpec describes one random operation script. Every field is written to the
// "script" golden line, so the Rust tests rebuild the same script from it.
type scriptSpec struct {
	name string
	seed uint64
	// cacheSize is passed to NewCacheCustomTimer.
	cacheSize int
	ops       int
	// batch is the number of operations per "batch" golden line.
	batch int
	// stateEvery digests the whole cache state every stateEvery batches (and after the last).
	stateEvery int
	clockStart uint32
	// keys random keys of 0 to keyMaxLen bytes make up the pool, after the colliding keys
	// (collide) and before the 65535 and 65536 byte keys (huge).
	keys      int
	keyMaxLen int
	smallVal  int
	ttlMax    int
	tickMax   int
	// backPermille is the share of ticks, in thousandths, that move the clock backwards.
	backPermille int
	collide      bool
	huge         bool
	weights      [opCount]int
	// deepKeys keys from the deep pool (findDeepKeys), which all share one segment and one
	// slot, join the pool after the random keys.
	deepKeys int
	// smallMix draws 97% of the value lengths from 0 to smallVal.
	smallMix bool
	// tickEvery, when positive, advances the clock by one second every tickEvery calls to
	// Now, so that any difference in how often the cache reads its clock changes the results.
	tickEvery int
}

// w builds a weight table from (operation, weight) pairs.
func w(pairs ...int) [opCount]int {
	var weights [opCount]int
	for i := 0; i+1 < len(pairs); i += 2 {
		weights[pairs[i]] = pairs[i+1]
	}
	return weights
}

// mixedWeights exercises every operation, in ten-thousandths.
var mixedWeights = w(
	opSet, 3000, opGet, 2500, opGetWithExpiration, 400, opPeek, 300, opDel, 400, opTouch, 400,
	opTTL, 300, opGetOrSet, 300, opSetAndGet, 300, opGetFn, 300, opPeekFn, 200,
	opGetWithBuf, 200, opSetInt, 100, opGetInt, 100, opGetIntWithExpiration, 100, opDelInt, 100,
	opTick, 980, opClear, 1, opResetStatistics, 2, opIterate, 1,
)

var scriptSpecs = []scriptSpec{
	{
		name: "small_evict", seed: 1, cacheSize: 0, ops: 60000, batch: 1000, stateEvery: 1,
		clockStart: 1700000000, keys: 6000, keyMaxLen: 24, smallVal: 120, ttlMax: 30,
		tickMax: 2, collide: true, huge: true, weights: mixedWeights,
	},
	{
		name: "near_limits", seed: 2, cacheSize: 524288 + 12345, ops: 40000, batch: 1000,
		stateEvery: 1, clockStart: 1700000000, keys: 300, keyMaxLen: 8, smallVal: 40,
		ttlMax: 10, tickMax: 1, collide: true, huge: true,
		weights: w(opSet, 400, opGet, 300, opSetAndGet, 50, opGetOrSet, 50, opDel, 50,
			opTouch, 30, opTick, 100, opIterate, 1),
	},
	{
		name: "ttl_wrap", seed: 3, cacheSize: 600000, ops: 40000, batch: 1000, stateEvery: 1,
		clockStart: 0xFFFFFF00, keys: 2000, keyMaxLen: 16, smallVal: 64, ttlMax: 20,
		tickMax: 5, backPermille: 50, collide: true, weights: mixedWeights,
	},
	{
		name: "clock_back", seed: 4, cacheSize: 524288, ops: 40000, batch: 1000, stateEvery: 1,
		clockStart: 1000000000, keys: 3000, keyMaxLen: 20, smallVal: 100, ttlMax: 15,
		tickMax: 10, backPermille: 300, collide: true, weights: mixedWeights,
	},
	{
		name: "hot_keys_lru", seed: 5, cacheSize: 1 << 20, ops: 60000, batch: 1000,
		stateEvery: 2, clockStart: 1600000000, keys: 20000, keyMaxLen: 12, smallVal: 48,
		ttlMax: 100, tickMax: 1,
		weights: w(opSet, 350, opGet, 450, opGetFn, 50, opTick, 150),
	},
	{
		name: "big_cache_huge_keys", seed: 6, cacheSize: 70 << 20, ops: 3000, batch: 500,
		stateEvery: 1, clockStart: 1700000000, keys: 200, keyMaxLen: 64, smallVal: 2000,
		ttlMax: 50, tickMax: 3, huge: true,
		weights: w(opSet, 400, opGet, 300, opPeek, 50, opDel, 50, opTouch, 50, opTTL, 50,
			opTick, 99, opIterate, 1),
	},
	{
		name: "clear_reset_iterate", seed: 7, cacheSize: 524288, ops: 30000, batch: 1000,
		stateEvery: 1, clockStart: 1700000000, keys: 500, keyMaxLen: 10, smallVal: 80,
		ttlMax: 8, tickMax: 2, collide: true,
		weights: w(opSet, 400, opGet, 300, opDel, 50, opTick, 150, opClear, 5,
			opResetStatistics, 10, opIterate, 5, opTouch, 40, opTTL, 40),
	},
	{
		name: "int_keys", seed: 8, cacheSize: 524288, ops: 20000, batch: 1000, stateEvery: 1,
		clockStart: 1700000000, keys: 50, keyMaxLen: 8, smallVal: 300, ttlMax: 5,
		tickMax: 1,
		weights: w(opSetInt, 400, opGetInt, 300, opGetIntWithExpiration, 100, opDelInt, 100,
			opTick, 100),
	},
	{
		name: "empty_values", seed: 9, cacheSize: 524288, ops: 20000, batch: 1000,
		stateEvery: 1, clockStart: 1700000000, keys: 400, keyMaxLen: 3, smallVal: 0,
		ttlMax: 4, tickMax: 1, collide: true, weights: mixedWeights,
	},
	{
		name: "negative_size", seed: 10, cacheSize: -5, ops: 5000, batch: 1000, stateEvery: 1,
		clockStart: 5, keys: 100, keyMaxLen: 30, smallVal: 400, ttlMax: 3, tickMax: 2,
		backPermille: 100, weights: mixedWeights,
	},
}

// collisionSeed seeds the search for keys whose hashes share the low 32 bits, which select the
// segment, the slot and the 16-bit tag.
const collisionSeed = 0xC011151011

// collisionPairs is the number of colliding key pairs scripts with collide use.
const collisionPairs = 8

// findCollisions returns collisionPairs pairs of distinct 6-byte keys whose Sum64 share the
// low 32 bits, in the order the search finds them.
func findCollisions() [][]byte {
	rng := splitmix64{state: collisionSeed}
	seen := map[uint32][]byte{}
	var found [][]byte
	for len(found) < 2*collisionPairs {
		key := make([]byte, 6)
		rng.fill(key)
		low := uint32(xxhash.Sum64(key))
		if prev, ok := seen[low]; ok {
			if !bytes.Equal(prev, key) {
				found = append(found, prev, key)
				delete(seen, low)
			}
			continue
		}
		seen[low] = key
	}
	return found
}

// specialTTLs are expiration arguments whose uint32 conversion truncates or wraps.
var specialTTLs = []int{1 << 32, 1<<32 + 1, 1<<32 - 1, 1 << 31, math.MaxInt64, math.MaxUint32 + 5}

// intKeys are the SetInt/GetInt/DelInt keys.
var intKeys = []int64{0, 1, 2, 3, 7, 42, -1, -2, math.MinInt64, math.MaxInt64, 1 << 40, -(1 << 33)}

// errFn is the error the GetFn/PeekFn callbacks return when the script asks them to fail.
var errFn = errors.New("fn error")

// fakeTimer is the script clock. With tickEvery set it also advances by itself, one second
// every tickEvery calls to Now.
type fakeTimer struct {
	now       uint32
	calls     uint64
	tickEvery uint64
}

func (t *fakeTimer) Now() uint32 {
	t.calls++
	if t.tickEvery > 0 && t.calls%t.tickEvery == 0 {
		t.now++
	}
	return t.now
}

// runner executes one script.
type runner struct {
	spec  *scriptSpec
	rng   splitmix64
	vrng  splitmix64
	clock *fakeTimer
	cache *freecache.Cache
	keys  [][]byte
	maxKV int
	// it is the iterator opIterStep advances across operations, nil before the first step
	// and after the end.
	it *freecache.Iterator
}

func newRunner(spec *scriptSpec, collisions, deep [][]byte) *runner {
	r := &runner{
		spec:  spec,
		rng:   splitmix64{state: spec.seed},
		vrng:  splitmix64{state: spec.seed ^ 0x76616C756573},
		clock: &fakeTimer{now: spec.clockStart, tickEvery: uint64(spec.tickEvery)},
	}
	r.cache = freecache.NewCacheCustomTimer(spec.cacheSize, r.clock)
	size := spec.cacheSize
	if size < 512*1024 {
		size = 512 * 1024
	}
	r.maxKV = (size/256)/4 - freecache.ENTRY_HDR_SIZE
	if spec.collide {
		r.keys = append(r.keys, collisions...)
	}
	for i := 0; i < spec.keys; i++ {
		key := make([]byte, r.rng.below(uint64(spec.keyMaxLen)+1))
		r.rng.fill(key)
		r.keys = append(r.keys, key)
	}
	r.keys = append(r.keys, deep[:spec.deepKeys]...)
	if spec.huge {
		r.keys = append(r.keys, bytes.Repeat([]byte("h"), 65535), bytes.Repeat([]byte("h"), 65536))
	}
	return r
}

// pickOp draws an operation by weight.
func (r *runner) pickOp() int {
	total := 0
	for _, weight := range r.spec.weights {
		total += weight
	}
	x := int(r.rng.below(uint64(total)))
	for op, weight := range r.spec.weights {
		if x < weight {
			return op
		}
		x -= weight
	}
	return opTick
}

// pickKey draws a key index: half the time from the first tenth of the pool (hot keys).
func (r *runner) pickKey() int {
	n := len(r.keys)
	if r.rng.below(2) == 0 {
		hot := n / 10
		if hot < 1 {
			hot = 1
		}
		return int(r.rng.below(uint64(hot)))
	}
	return int(r.rng.below(uint64(n)))
}

// pickValLen draws a value length for a key of keyLen bytes: small, around the largest
// accepted, anything accepted, or anything up to slightly over the limit.
func (r *runner) pickValLen(keyLen int) int {
	lim := r.maxKV - keyLen
	if r.spec.smallMix && r.rng.below(100) < 97 {
		return int(r.rng.below(uint64(r.spec.smallVal) + 1))
	}
	var n int
	switch x := r.rng.below(100); {
	case x < 60:
		n = int(r.rng.below(uint64(r.spec.smallVal) + 1))
	case x < 75:
		n = lim + int(r.rng.below(6)) - 3
	case x < 92:
		if lim > 0 {
			n = int(r.rng.below(uint64(lim) + 1))
		} else {
			n = int(r.rng.below(uint64(r.spec.smallVal) + 1))
		}
	default:
		n = int(r.rng.below(uint64(r.maxKV) + 64))
	}
	if n < 0 {
		n = 0
	}
	return n
}

// pickTTL draws an expireSeconds argument.
func (r *runner) pickTTL() int {
	switch x := r.rng.below(100); {
	case x < 45:
		return 0
	case x < 50:
		return -int(r.rng.below(5)) - 1
	case x < 88:
		return 1 + int(r.rng.below(uint64(r.spec.ttlMax)))
	case x < 96:
		return int(r.rng.below(1 << 20))
	default:
		return specialTTLs[r.rng.below(uint64(len(specialTTLs)))]
	}
}

func (r *runner) pickInt() int64 {
	return intKeys[r.rng.below(uint64(len(intKeys)))]
}

func (r *runner) value(n int) []byte {
	b := make([]byte, n)
	r.vrng.fill(b)
	return b
}

func errStr(err error) string {
	if err == nil {
		return "nil"
	}
	return "err:" + err.Error()
}

// valStr renders a value result, keeping Go's nil apart from an empty slice.
func valStr(v []byte, err error) string {
	if err != nil {
		return "err:" + err.Error()
	}
	if v == nil {
		return "nil"
	}
	return fmt.Sprintf("len:%d fnv:%s", len(v), bytesDigest(v))
}

// counters renders every counter the cache exposes.
func counters(c *freecache.Cache) string {
	return fmt.Sprintf("e:%d h:%d m:%d ev:%d ex:%d ow:%d to:%d aat:%d lk:%d hr:%016x",
		c.EntryCount(), c.HitCount(), c.MissCount(), c.EvacuateCount(), c.ExpiredCount(),
		c.OverwriteCount(), c.TouchedCount(), c.AverageAccessTime(), c.LookupCount(),
		math.Float64bits(c.HitRate()))
}

// fnRecorder is the GetFn/PeekFn callback.
func fnRecorder(desc *string, fail bool) func([]byte) error {
	return func(b []byte) error {
		*desc = fmt.Sprintf("len:%d fnv:%s", len(b), bytesDigest(b))
		if fail {
			return errFn
		}
		return nil
	}
}

// step draws and runs one operation, returning its description and result.
func (r *runner) step() (string, string) {
	c := r.cache
	switch op := r.pickOp(); op {
	case opSet, opGetOrSet, opSetAndGet:
		k := r.pickKey()
		key := r.keys[k]
		vlen := r.pickValLen(len(key))
		ttl := r.pickTTL()
		val := r.value(vlen)
		desc := fmt.Sprintf("k%d v%d t%d", k, vlen, ttl)
		switch op {
		case opSet:
			return "set " + desc, errStr(c.Set(key, val, ttl))
		case opGetOrSet:
			ret, err := c.GetOrSet(key, val, ttl)
			return "getorset " + desc, valStr(ret, nil) + " " + errStr(err)
		default:
			ret, found, err := c.SetAndGet(key, val, ttl)
			return "setandget " + desc, fmt.Sprintf("%s found:%v %s", valStr(ret, nil), found, errStr(err))
		}
	case opGet:
		k := r.pickKey()
		return fmt.Sprintf("get k%d", k), valStr(c.Get(r.keys[k]))
	case opGetWithExpiration:
		k := r.pickKey()
		v, exp, err := c.GetWithExpiration(r.keys[k])
		return fmt.Sprintf("getexp k%d", k), fmt.Sprintf("%s exp:%d", valStr(v, err), exp)
	case opPeek:
		k := r.pickKey()
		return fmt.Sprintf("peek k%d", k), valStr(c.Peek(r.keys[k]))
	case opDel:
		k := r.pickKey()
		return fmt.Sprintf("del k%d", k), strconv.FormatBool(c.Del(r.keys[k]))
	case opTouch:
		k := r.pickKey()
		ttl := r.pickTTL()
		return fmt.Sprintf("touch k%d t%d", k, ttl), errStr(c.Touch(r.keys[k], ttl))
	case opTTL:
		k := r.pickKey()
		ttl, err := c.TTL(r.keys[k])
		if err != nil {
			return fmt.Sprintf("ttl k%d", k), errStr(err)
		}
		return fmt.Sprintf("ttl k%d", k), fmt.Sprintf("ttl:%d", ttl)
	case opGetFn, opPeekFn:
		k := r.pickKey()
		fail := r.rng.below(4) == 0
		desc := "-"
		if op == opGetFn {
			err := c.GetFn(r.keys[k], fnRecorder(&desc, fail))
			return fmt.Sprintf("getfn k%d f%v", k, fail), desc + " " + errStr(err)
		}
		err := c.PeekFn(r.keys[k], fnRecorder(&desc, fail))
		return fmt.Sprintf("peekfn k%d f%v", k, fail), desc + " " + errStr(err)
	case opGetWithBuf:
		k := r.pickKey()
		bufCap := []int{0, 1, 16, 4096}[r.rng.below(4)]
		v, err := c.GetWithBuf(r.keys[k], make([]byte, 0, bufCap))
		return fmt.Sprintf("getbuf k%d c%d", k, bufCap), valStr(v, err)
	case opSetInt:
		ik := r.pickInt()
		vlen := r.pickValLen(8)
		ttl := r.pickTTL()
		val := r.value(vlen)
		return fmt.Sprintf("setint i%d v%d t%d", ik, vlen, ttl), errStr(c.SetInt(ik, val, ttl))
	case opGetInt:
		ik := r.pickInt()
		return fmt.Sprintf("getint i%d", ik), valStr(c.GetInt(ik))
	case opGetIntWithExpiration:
		ik := r.pickInt()
		v, exp, err := c.GetIntWithExpiration(ik)
		return fmt.Sprintf("getintexp i%d", ik), fmt.Sprintf("%s exp:%d", valStr(v, err), exp)
	case opDelInt:
		ik := r.pickInt()
		return fmt.Sprintf("delint i%d", ik), strconv.FormatBool(c.DelInt(ik))
	case opTick:
		d := uint32(r.rng.below(uint64(r.spec.tickMax) + 1))
		if int(r.rng.below(1000)) < r.spec.backPermille {
			r.clock.now -= d
			return fmt.Sprintf("tick -%d", d), fmt.Sprintf("now:%d", r.clock.now)
		}
		r.clock.now += d
		return fmt.Sprintf("tick +%d", d), fmt.Sprintf("now:%d", r.clock.now)
	case opClear:
		c.Clear()
		return "clear", "ok"
	case opResetStatistics:
		c.ResetStatistics()
		return "reset", "ok"
	case opIterStep:
		k := int(r.rng.below(8)) + 1
		if r.it == nil {
			r.it = c.NewIterator()
		}
		f := newFnv()
		n, end := 0, false
		for n < k {
			e := r.it.Next()
			if e == nil {
				end = true
				r.it = nil
				break
			}
			n++
			f.i64(int64(len(e.Key)))
			f.bytes(e.Key)
			f.i64(int64(len(e.Value)))
			f.bytes(e.Value)
		}
		return fmt.Sprintf("iterstep %d", k), fmt.Sprintf("n:%d end:%v fnv:%s", n, end, f.hex())
	default:
		it := c.NewIterator()
		f := newFnv()
		n := 0
		for e := it.Next(); e != nil; e = it.Next() {
			n++
			f.i64(int64(len(e.Key)))
			f.bytes(e.Key)
			f.i64(int64(len(e.Value)))
			f.bytes(e.Value)
		}
		return "iterate", fmt.Sprintf("n:%d fnv:%s", n, f.hex())
	}
}

// stateDigest reads every segment's unexported state with reflect, in the order the Rust
// SegmentState::digest feeds it.
func stateDigest(c *freecache.Cache) string {
	f := newFnv()
	segs := reflect.ValueOf(c).Elem().FieldByName("segments")
	for i := 0; i < segs.Len(); i++ {
		seg := segs.Index(i)
		rb := seg.FieldByName("rb")
		data := rb.FieldByName("data").Bytes()
		f.i64(rb.FieldByName("begin").Int())
		f.i64(rb.FieldByName("end").Int())
		f.i64(rb.FieldByName("index").Int())
		f.i64(int64(len(data)))
		f.words(data)
		f.i64(seg.FieldByName("vacuumLen").Int())
		slotCap := seg.FieldByName("slotCap").Int()
		f.i64(slotCap)
		for _, name := range []string{"missCount", "hitCount", "entryCount", "totalCount",
			"totalTime", "totalEvacuate", "totalExpired", "overwrites", "touched"} {
			f.i64(seg.FieldByName(name).Int())
		}
		slotLens := seg.FieldByName("slotLens")
		slotsData := seg.FieldByName("slotsData")
		for slot := 0; slot < 256; slot++ {
			n := slotLens.Index(slot).Int()
			f.i64(n)
			for j := int64(0); j < n; j++ {
				ptr := slotsData.Index(int(int64(slot)*slotCap + j))
				f.i64(ptr.FieldByName("offset").Int())
				f.i64(int64(ptr.FieldByName("hash16").Uint()))
				f.i64(int64(ptr.FieldByName("keyLen").Uint()))
			}
		}
	}
	return f.hex()
}

func boolField(b bool) string {
	if b {
		return "1"
	}
	return "0"
}

func specFields(s *scriptSpec) []string {
	weights := make([]string, len(s.weights))
	for i, weight := range s.weights {
		weights[i] = strconv.Itoa(weight)
	}
	return []string{
		"script", s.name, strconv.FormatUint(s.seed, 10), strconv.Itoa(s.cacheSize),
		strconv.Itoa(s.ops), strconv.Itoa(s.batch), strconv.Itoa(s.stateEvery),
		strconv.FormatUint(uint64(s.clockStart), 10), strconv.Itoa(s.keys),
		strconv.Itoa(s.keyMaxLen), strconv.Itoa(s.smallVal), strconv.Itoa(s.ttlMax),
		strconv.Itoa(s.tickMax), strconv.Itoa(s.backPermille), boolField(s.collide),
		boolField(s.huge), strings.Join(weights, ","), strconv.Itoa(s.deepKeys),
		boolField(s.smallMix), strconv.Itoa(s.tickEvery),
	}
}

// record renders an operation's result with every counter and, on a ticking clock, the
// number of clock reads so far.
func (r *runner) record(res string) string {
	out := res + " | " + counters(r.cache)
	if r.spec.tickEvery > 0 {
		out += fmt.Sprintf(" c:%d", r.clock.calls)
	}
	return out
}

func writeScripts(g *golden) {
	defer g.close()
	collisions := findCollisions()
	for _, key := range collisions {
		g.line("collide", fmt.Sprintf("%x", key), fmt.Sprintf("%08x", uint32(xxhash.Sum64(key))))
	}
	deep := findDeepKeys()
	for _, key := range deep {
		g.line("deep", fmt.Sprintf("%x", key))
	}
	for i := range scriptSpecs {
		spec := &scriptSpecs[i]
		g.line(specFields(spec)...)
		r := newRunner(spec, collisions, deep)
		ops, results := newFnv(), newFnv()
		for op := 0; op < spec.ops; op++ {
			desc, res := r.step()
			ops.str(desc + "\n")
			results.str(r.record(res) + "\n")
			if (op+1)%spec.batch != 0 && op+1 != spec.ops {
				continue
			}
			batch := op / spec.batch
			state := "-"
			if batch%spec.stateEvery == 0 || op+1 == spec.ops {
				state = stateDigest(r.cache)
			}
			g.line("batch", spec.name, strconv.Itoa(batch), ops.hex(), results.hex(), state,
				counters(r.cache))
			ops, results = newFnv(), newFnv()
		}
	}
}

// traceScript prints the records of operations [from, to) of one script.
func traceScript(name string, from, to int) {
	for i := range scriptSpecs {
		spec := &scriptSpecs[i]
		if spec.name != name {
			continue
		}
		r := newRunner(spec, findCollisions(), findDeepKeys())
		for op := 0; op < spec.ops && (to < 0 || op < to); op++ {
			desc, res := r.step()
			if op >= from {
				fmt.Printf("%d %s => %s\n", op, desc, r.record(res))
			}
		}
		return
	}
	panic("unknown script " + name)
}
