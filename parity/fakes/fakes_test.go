package fakes

import (
	"testing"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestDefaultsMatchFixtures(t *testing.T) {
	d := Defaults()
	now := time.UnixMilli(d.NowUnixMS).UTC()
	_, week := now.ISOWeek()
	assert.Equal(t, "2021-08-20T15:28:50.823Z", now.Format("2006-01-02T15:04:05.000Z07:00"))
	assert.Equal(t, 33, week)
	assert.Equal(t, int64(10), d.Int63)
	assert.Equal(t, "30470a14-2949-4110-abce-b62d57304ad5", d.UUID)
}

func TestSourceSet(t *testing.T) {
	tests := []struct {
		name string
		in   *parity.FakeValues
		want parity.FakeValues
	}{
		{name: "nil takes defaults", in: nil, want: Defaults()},
		{name: "zero fields take defaults", in: &parity.FakeValues{}, want: Defaults()},
		{
			name: "overrides",
			in:   &parity.FakeValues{NowUnixMS: 1000, Int63: 7, UUID: "u"},
			want: parity.FakeValues{NowUnixMS: 1000, Int63: 7, UUID: "u"},
		},
		{
			name: "partial override",
			in:   &parity.FakeValues{Int63: 99},
			want: parity.FakeValues{NowUnixMS: DefaultNowUnixMS, Int63: 99, UUID: DefaultUUID},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := &Source{}
			s.Set(tt.in)
			assert.Equal(t, tt.want, s.Values())
		})
	}
}

func TestSourceValues(t *testing.T) {
	s := &Source{}
	s.Set(&parity.FakeValues{NowUnixMS: 1629473330823, Int63: 10, UUID: "id"})
	uuid, err := s.Generate()
	require.NoError(t, err)

	tests := []struct {
		name string
		got  any
		want any
	}{
		{name: "now is UTC", got: s.Now().Location(), want: time.UTC},
		{name: "now instant", got: s.Now().UnixMilli(), want: int64(1629473330823)},
		{name: "int63", got: s.GenerateInt63(), want: int64(10)},
		{name: "int", got: s.Int(), want: 10},
		{name: "intn reduces", got: s.Intn(3), want: 1},
		{name: "intn zero", got: s.Intn(0), want: 0},
		{name: "intn negative", got: s.Intn(-5), want: 0},
		{name: "uuid", got: uuid, want: "id"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assert.Equal(t, tt.want, tt.got)
		})
	}
}

// TestInstallLifecycle checks the package-level helpers fall back to the real
// sources when nothing is installed (production) and return the fakes after
// Install (parity mode).
func TestInstallLifecycle(t *testing.T) {
	Uninstall()
	t.Cleanup(Uninstall)

	require.Nil(t, Active())
	before := time.Now()
	assert.False(t, Now().Before(before), "uninstalled Now must be the real clock")

	src := Install()
	require.NotNil(t, src)
	assert.Same(t, src, Active())
	assert.Same(t, src, Install(), "Install is idempotent")
	assert.Equal(t, DefaultNowUnixMS, Now().UnixMilli())
	assert.Equal(t, int(DefaultInt63), Int())

	src.Set(&parity.FakeValues{Int63: 5})
	assert.Equal(t, 5, Int())

	Uninstall()
	assert.Nil(t, Active())
	assert.NotEqual(t, DefaultNowUnixMS, Now().UnixMilli())
}
