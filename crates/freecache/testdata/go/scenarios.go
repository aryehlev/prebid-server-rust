package main

import (
	"bytes"
	"fmt"
	"math"
	"strconv"
	"strings"

	"github.com/coocood/freecache"
)

// The scenarios replay freecache's own tests (cache_test.go) on a fake clock, advancing it
// where the tests sleep, and record the counters the tests only log or check loosely.

// scenarioClockStart is the fake clock's starting time.
const scenarioClockStart = 1700000000

func scenarioStats(c *freecache.Cache) string {
	return fmt.Sprintf("hr:%016x ev:%d e:%d aat:%d ex:%d", math.Float64bits(c.HitRate()),
		c.EvacuateCount(), c.EntryCount(), c.AverageAccessTime(), c.ExpiredCount())
}

// scenarioFreeCache is TestFreeCache.
func scenarioFreeCache() string {
	clock := &fakeTimer{now: scenarioClockStart}
	cache := freecache.NewCacheCustomTimer(1024, clock)
	var out []string
	out = append(out, fmt.Sprintf("initial:%s", scenarioStats(cache)))
	key := []byte("abcd")
	val := []byte("efghijkl")
	out = append(out, "set:"+errStr(cache.Set(key, val, 0)))
	out = append(out, "get:"+valStr(cache.Get(key)))
	out = append(out, fmt.Sprintf("del:%v", cache.Del(key)))
	out = append(out, "get:"+valStr(cache.Get(key)))
	out = append(out, fmt.Sprintf("del:%v", cache.Del(key)))

	cache.Clear()
	n := 5000
	setErrors := 0
	for i := 0; i < n; i++ {
		keyStr := fmt.Sprintf("key%v", i)
		if cache.Set([]byte(keyStr), []byte(strings.Repeat(keyStr, 10)), 0) != nil {
			setErrors++
		}
	}
	clock.now++
	for i := 1; i < n; i += 2 {
		_, _ = cache.Get([]byte(fmt.Sprintf("key%v", i)))
	}
	for i := 1; i < n; i += 8 {
		cache.Del([]byte(fmt.Sprintf("key%v", i)))
	}
	for i := 0; i < n; i += 2 {
		keyStr := fmt.Sprintf("key%v", i)
		if cache.Set([]byte(keyStr), []byte(strings.Repeat(keyStr, 10)), 0) != nil {
			setErrors++
		}
	}
	hits, mismatches, fnHits := 0, 0, 0
	for i := 1; i < n; i += 2 {
		keyStr := fmt.Sprintf("key%v", i)
		expected := strings.Repeat(keyStr, 10)
		value, err := cache.Get([]byte(keyStr))
		if err == nil {
			hits++
			if string(value) != expected {
				mismatches++
			}
		}
		if cache.GetFn([]byte(keyStr), func(v []byte) error {
			if string(v) != expected {
				mismatches++
			}
			return nil
		}) == nil {
			fnHits++
		}
	}
	out = append(out, fmt.Sprintf("seterr:%d hits:%d fnhits:%d mismatch:%d", setErrors, hits, fnHits, mismatches))
	out = append(out, "logged:"+scenarioStats(cache))
	cache.ResetStatistics()
	out = append(out, "reset:"+scenarioStats(cache))
	return strings.Join(out, " ")
}

// scenarioEvacuateCount is TestEvacuateCount.
func scenarioEvacuateCount() string {
	clock := &fakeTimer{now: scenarioClockStart}
	cache := freecache.NewCacheCustomTimer(1024*1024, clock)
	n := 100000
	for i := 0; i < n; i++ {
		if err := cache.Set([]byte(strconv.Itoa(i)), []byte("A"), 0); err != nil {
			panic(err)
		}
	}
	missing := 0
	for i := 0; i < n; i++ {
		res, err := cache.Get([]byte(strconv.Itoa(i)))
		if err == freecache.ErrNotFound || (err == nil && string(res) != "A") {
			missing++
		}
	}
	return fmt.Sprintf("missing:%d entries:%d evacuates:%d", missing, cache.EntryCount(), cache.EvacuateCount())
}

// scenarioIterator is TestIterator.
func scenarioIterator() string {
	clock := &fakeTimer{now: scenarioClockStart}
	cache := freecache.NewCacheCustomTimer(1024, clock)
	count := 10000
	for i := 0; i < count; i++ {
		if err := cache.Set([]byte(fmt.Sprintf("%d", i)), []byte(fmt.Sprintf("val%d", i)), 0); err != nil {
			panic(err)
		}
	}
	// Set some value that expires to make sure expired entry is not returned.
	if err := cache.Set([]byte("abc"), []byte("def"), 1); err != nil {
		panic(err)
	}
	clock.now += 2
	it := cache.NewIterator()
	f := newFnv()
	n, mismatches := 0, 0
	for e := it.Next(); e != nil; e = it.Next() {
		n++
		if !bytes.Equal(e.Value, append([]byte("val"), e.Key...)) {
			mismatches++
		}
		f.i64(int64(len(e.Key)))
		f.bytes(e.Key)
	}
	return fmt.Sprintf("n:%d mismatch:%d order:%s", n, mismatches, f.hex())
}

func writeScenarios(g *golden) {
	defer g.close()
	g.line("scenario", "TestFreeCache", scenarioFreeCache())
	g.line("scenario", "TestEvacuateCount", scenarioEvacuateCount())
	g.line("scenario", "TestIterator", scenarioIterator())
}
