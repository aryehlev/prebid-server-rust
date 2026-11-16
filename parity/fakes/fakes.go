// Package fakes provides the deterministic clock, random and UUID sources that
// parity mode injects into the adapters with internal nondeterminism (adnuntius,
// appnexus, cadent_aperture_mx, eplanning, flipp, huaweiads, tappx, yieldlab).
//
// Nothing is active until Install is called, which only parity bootstrap does
// when parity.enabled is true. While nothing is installed every helper returns
// exactly what the adapter used before (time.Now, math/rand), so the adapter
// seams are behaviour-neutral in production. The values are part of the
// cross-language contract (parity/schema/fakes.json): the Rust server must
// inject the same values in parity mode.
package fakes

import (
	"math/rand"
	"sync/atomic"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
)

// Default values, mirrored by parity/schema/fakes.json.
//
// DefaultNowUnixMS is 2021-08-20T15:28:50.823Z: it reproduces the huaweiads
// fixture nonce (1629473330823) and falls in ISO week 33 like the yieldlab test
// week generator. DefaultInt63 matches appnexus' FakeRandomNumberGenerator and
// DefaultUUID matches flipp's test UUID generator.
const (
	DefaultNowUnixMS int64 = 1629473330823
	DefaultInt63     int64 = 10
	DefaultUUID            = "30470a14-2949-4110-abce-b62d57304ad5"
)

// Defaults returns the fakes.json default values.
func Defaults() parity.FakeValues {
	return parity.FakeValues{NowUnixMS: DefaultNowUnixMS, Int63: DefaultInt63, UUID: DefaultUUID}
}

// Source serves deterministic values. Its methods match timeutil.Time,
// randomutil.RandomGenerator and uuidutil.UUIDGenerator so it can be injected
// directly into the adapters that already have those seams.
type Source struct {
	values atomic.Pointer[parity.FakeValues]
}

var active atomic.Pointer[Source]

// Install activates the deterministic sources process-wide and returns them.
// Calling it again returns the already installed Source.
func Install() *Source {
	src := &Source{}
	src.Set(nil)
	if active.CompareAndSwap(nil, src) {
		return src
	}
	return active.Load()
}

// Uninstall deactivates the deterministic sources. It exists for tests and
// shutdown; production never calls it.
func Uninstall() {
	active.Store(nil)
}

// Active returns the installed Source, or nil when parity fakes are not installed.
func Active() *Source {
	return active.Load()
}

// Set replaces the current values; zero fields (or a nil v) take the defaults.
// The exec handler calls it per case from envelope.fakes.
func (s *Source) Set(v *parity.FakeValues) {
	merged := Defaults()
	if v != nil {
		if v.NowUnixMS != 0 {
			merged.NowUnixMS = v.NowUnixMS
		}
		if v.Int63 != 0 {
			merged.Int63 = v.Int63
		}
		if v.UUID != "" {
			merged.UUID = v.UUID
		}
	}
	s.values.Store(&merged)
}

// Values returns the current values.
func (s *Source) Values() parity.FakeValues {
	return *s.values.Load()
}

// Now returns the fixed instant in UTC.
func (s *Source) Now() time.Time {
	return time.UnixMilli(s.values.Load().NowUnixMS).UTC()
}

// GenerateInt63 returns the fixed 63-bit value.
func (s *Source) GenerateInt63() int64 {
	return s.values.Load().Int63
}

// Intn returns the fixed value reduced modulo n; it returns 0 when n <= 0.
func (s *Source) Intn(n int) int {
	if n <= 0 {
		return 0
	}
	return int(s.values.Load().Int63 % int64(n))
}

// Int returns the fixed value as an int.
func (s *Source) Int() int {
	return int(s.values.Load().Int63)
}

// Generate returns the fixed UUID.
func (s *Source) Generate() (string, error) {
	return s.values.Load().UUID, nil
}

// Now returns the deterministic instant when fakes are installed, else time.Now().
func Now() time.Time {
	if src := active.Load(); src != nil {
		return src.Now()
	}
	return time.Now()
}

// Int returns the deterministic value when fakes are installed, else rand.Int().
func Int() int {
	if src := active.Load(); src != nil {
		return src.Int()
	}
	return rand.Int()
}
