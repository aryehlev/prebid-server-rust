package gen

import (
	"crypto/sha256"
	"encoding/binary"
	"math/bits"
	"math/rand/v2"
	"strconv"
)

// RNG is the generator's deterministic random source. It wraps the PCG-DXSM
// source of math/rand/v2, whose output for a given seed is fixed, and
// implements every bounded draw itself, so the bytes of a case never depend on
// math/rand helper algorithms that a Go release could change.
type RNG struct {
	src *rand.PCG
}

// NewRNG returns the stream for the PCG state (seed, stream).
func NewRNG(seed, stream uint64) *RNG {
	return &RNG{src: rand.NewPCG(seed, stream)}
}

// Derive returns the stream keyed by seed and labels. The key is hashed with
// SHA-256, so every distinct (seed, labels) list gets an independent stream and
// the stream of one label list never depends on how other streams were used.
func Derive(seed uint64, labels ...string) *RNG {
	h := sha256.New()
	var buf [8]byte
	binary.BigEndian.PutUint64(buf[:], seed)
	h.Write(buf[:])
	for _, l := range labels {
		h.Write([]byte{0})
		h.Write([]byte(l))
	}
	sum := h.Sum(nil)
	return NewRNG(binary.BigEndian.Uint64(sum[0:8]), binary.BigEndian.Uint64(sum[8:16]))
}

// Uint64 returns the next 64 random bits.
func (r *RNG) Uint64() uint64 {
	return r.src.Uint64()
}

// Uint64n returns a uniform value in [0, n), or 0 when n is 0. It uses
// Lemire's multiply-and-reject method.
func (r *RNG) Uint64n(n uint64) uint64 {
	if n == 0 {
		return 0
	}
	hi, lo := bits.Mul64(r.Uint64(), n)
	if lo < n {
		threshold := -n % n
		for lo < threshold {
			hi, lo = bits.Mul64(r.Uint64(), n)
		}
	}
	return hi
}

// Intn returns a uniform int in [0, n), or 0 when n <= 0.
func (r *RNG) Intn(n int) int {
	if n <= 0 {
		return 0
	}
	return int(r.Uint64n(uint64(n)))
}

// Range returns a uniform int in [lo, hi]; it returns lo when hi < lo.
func (r *RNG) Range(lo, hi int) int {
	if hi <= lo {
		return lo
	}
	return lo + r.Intn(hi-lo+1)
}

// Float64 returns a uniform float64 in [0, 1).
func (r *RNG) Float64() float64 {
	return float64(r.Uint64()>>11) / (1 << 53)
}

// Chance reports true with probability p.
func (r *RNG) Chance(p float64) bool {
	return r.Float64() < p
}

// Hex returns n random lowercase hex digits.
func (r *RNG) Hex(n int) string {
	const digits = "0123456789abcdef"
	out := make([]byte, n)
	for i := range out {
		out[i] = digits[r.Intn(len(digits))]
	}
	return string(out)
}

// UUID returns a random RFC 4122 version 4 UUID.
func (r *RNG) UUID() string {
	var b [16]byte
	binary.BigEndian.PutUint64(b[0:8], r.Uint64())
	binary.BigEndian.PutUint64(b[8:16], r.Uint64())
	b[6] = (b[6] & 0x0f) | 0x40
	b[8] = (b[8] & 0x3f) | 0x80
	const digits = "0123456789abcdef"
	out := make([]byte, 0, 36)
	for i, c := range b {
		if i == 4 || i == 6 || i == 8 || i == 10 {
			out = append(out, '-')
		}
		out = append(out, digits[c>>4], digits[c&0x0f])
	}
	return string(out)
}

// Weighted returns an index drawn with probability proportional to weights.
// Non-positive weights are never drawn; it returns -1 when no weight is
// positive.
func (r *RNG) Weighted(weights []int) int {
	total := 0
	for _, w := range weights {
		if w > 0 {
			total += w
		}
	}
	if total == 0 {
		return -1
	}
	n := r.Intn(total)
	for i, w := range weights {
		if w <= 0 {
			continue
		}
		if n < w {
			return i
		}
		n -= w
	}
	return -1
}

// Pick returns a uniformly chosen element of xs, or the zero value when xs is
// empty.
func Pick[T any](r *RNG, xs []T) T {
	var zero T
	if len(xs) == 0 {
		return zero
	}
	return xs[r.Intn(len(xs))]
}

// Shuffle permutes xs in place (Fisher-Yates).
func Shuffle[T any](r *RNG, xs []T) {
	for i := len(xs) - 1; i > 0; i-- {
		j := r.Intn(i + 1)
		xs[i], xs[j] = xs[j], xs[i]
	}
}

// Sample returns k distinct elements of xs (all of them when k >= len(xs)) in
// their original order.
func Sample[T any](r *RNG, xs []T, k int) []T {
	if k >= len(xs) {
		return append([]T(nil), xs...)
	}
	if k <= 0 {
		return nil
	}
	idx := make([]int, len(xs))
	for i := range idx {
		idx[i] = i
	}
	Shuffle(r, idx)
	chosen := make([]bool, len(xs))
	for _, i := range idx[:k] {
		chosen[i] = true
	}
	out := make([]T, 0, k)
	for i, x := range xs {
		if chosen[i] {
			out = append(out, x)
		}
	}
	return out
}

// u64 formats an unsigned integer for use in stream labels.
func u64(v uint64) string {
	return strconv.FormatUint(v, 10)
}
