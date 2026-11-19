package main

import (
	"bytes"
	"strconv"

	"github.com/coocood/freecache"
)

// cacheEntry is one Set and the Get after it.
type cacheEntry struct {
	Key      string `json:"key"`
	ValueLen int    `json:"value_len"`
	SetErr   string `json:"set_err"`
	GetErr   string `json:"get_err"`
}

type cacheDoc struct {
	Header    header       `json:"header"`
	CacheSize int          `json:"cache_size"`
	Entries   []cacheEntry `json:"entries"`
}

// cacheGolden stores documents of growing size in a freecache sized as the stored-request
// caches size it (stored_requests/caches/memory/cache.go:29). pbsLRUCache.Set logs a failed Set
// and goes on (stored_requests/caches/memory/maps.go:60-64), so a rejected entry is a miss.
// freecache refuses an entry whose key and value together are longer than a quarter of its
// segment's ring buffer less the 24-byte entry header (freecache's segment.go:73-76), which is
// 1000 bytes in a 1 MiB cache: the 993-byte and 994-byte documents, under 7-byte keys, sit on
// either side of that limit.
func cacheGolden() cacheDoc {
	doc := cacheDoc{Header: newHeader("github.com/coocood/freecache"), CacheSize: 1 << 20}
	cache := freecache.NewCache(doc.CacheSize)
	for _, n := range []int{512, 993, 994, 1000, 1024, 64 * 1024} {
		key := "doc-" + strconv.Itoa(n)
		entry := cacheEntry{Key: key, ValueLen: n}
		if err := cache.Set([]byte(key), bytes.Repeat([]byte("x"), n), 60); err != nil {
			entry.SetErr = err.Error()
		}
		if _, err := cache.Get([]byte(key)); err != nil {
			entry.GetErr = err.Error()
		}
		doc.Entries = append(doc.Entries, entry)
	}
	return doc
}
