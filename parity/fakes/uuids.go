package fakes

import (
	"crypto/sha256"
	"strconv"
	"sync"
	"sync/atomic"

	"github.com/gofrs/uuid"
)

// Generated UUIDs.
//
// Go generates random v4 UUIDs on the request path with uuid.NewV4: the
// request id that replaces {{UUID}} (endpoints/openrtb2/auction.go:2055 through
// util/uuidutil), source.tid and imp[].ext.tid (endpoints/openrtb2/auction.go:976,
// :984), generated bid ids (exchange/exchange.go:128), and the cache keys of
// exchange/auction.go:82 and :220. The differ numbers them with <uuid#n>
// placeholders, but a placeholder cannot reach inside a gzip-compressed bidder
// request: a random tid changes the compressed length, so two Go processes send
// different Content-Length headers for the same case (a wire difference, which
// is never normalized). In parity mode every uuid.NewV4 therefore draws from a
// deterministic stream instead, restarted whenever the fake values are set (at
// the start and end of every case and on /__parity/reset).
//
// The stream is part of the cross-language contract (fakes.json
// generated_uuids): the n-th UUID (n = 1, 2, ...) is the first 16 bytes of
// SHA-256(seed + ":" + decimal n) with the RFC 4122 version 4 and variant bits
// set, where seed is the case's fakes.uuid value.

// uuidStream is one run of the deterministic UUID sequence.
type uuidStream struct {
	seed string
	n    atomic.Uint64
}

// SequenceUUID returns the n-th UUID of the deterministic stream seeded by
// seed (n counts from 1).
func SequenceUUID(seed string, n uint64) uuid.UUID {
	sum := sha256.Sum256([]byte(seed + ":" + strconv.FormatUint(n, 10)))
	var u uuid.UUID
	copy(u[:], sum[:uuid.Size])
	u.SetVersion(uuid.V4)
	u.SetVariant(uuid.VariantRFC4122)
	return u
}

// NewV4 returns the next UUID of the source's deterministic stream. It is safe
// for concurrent use; concurrent callers get distinct UUIDs.
func (s *Source) NewV4() uuid.UUID {
	st := s.uuids.Load()
	if st == nil {
		s.uuids.CompareAndSwap(nil, &uuidStream{seed: DefaultUUID})
		st = s.uuids.Load()
	}
	return SequenceUUID(st.seed, st.n.Add(1))
}

// restartUUIDs starts a new deterministic stream from seed.
func (s *Source) restartUUIDs(seed string) {
	s.uuids.Store(&uuidStream{seed: seed})
}

// generator is the uuid.Generator Install puts in place of
// uuid.DefaultGenerator: while a Source is active NewV4 draws from its
// deterministic stream; otherwise, and for every other UUID version, it
// delegates to the generator it replaced.
type generator struct {
	uuid.Generator
}

// NewV4 implements uuid.Generator.
func (g generator) NewV4() (uuid.UUID, error) {
	if src := active.Load(); src != nil {
		return src.NewV4(), nil
	}
	return g.Generator.NewV4()
}

var installGenerator sync.Once

// installUUIDGenerator replaces uuid.DefaultGenerator once per process, before
// the first Source becomes active. After Uninstall the replacement delegates
// every call to the original generator.
func installUUIDGenerator() {
	installGenerator.Do(func() {
		uuid.DefaultGenerator = generator{Generator: uuid.DefaultGenerator}
	})
}
