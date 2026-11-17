package fakes

import (
	"sync"
	"testing"

	"github.com/gofrs/uuid"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestSequenceUUID(t *testing.T) {
	tests := []struct {
		name string
		seed string
		n    uint64
		want string
	}{
		{name: "first of the default seed", seed: DefaultUUID, n: 1, want: "0b1b705d-5a17-494f-872e-a858a0fc3200"},
		{name: "second of the default seed", seed: DefaultUUID, n: 2, want: "2329f45a-e73e-4f17-87bb-876e809c0196"},
		{name: "third of the default seed", seed: DefaultUUID, n: 3, want: "f4115b74-ceda-46eb-94dc-5b099ad1d7e6"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := SequenceUUID(tt.seed, tt.n)
			assert.Equal(t, tt.want, got.String())
			assert.Equal(t, byte(uuid.V4), got.Version())
			assert.Equal(t, uuid.VariantRFC4122, got.Variant())
		})
	}
	assert.NotEqual(t, SequenceUUID("a", 1), SequenceUUID("b", 1), "the seed selects the stream")
}

func TestSourceNewV4Restarts(t *testing.T) {
	tests := []struct {
		name  string
		set   *parity.FakeValues
		seed  string
		draws int
	}{
		{name: "defaults", set: nil, seed: DefaultUUID, draws: 3},
		{name: "envelope uuid seeds the stream", set: &parity.FakeValues{UUID: "case-seed"}, seed: "case-seed", draws: 2},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := &Source{}
			for round := 0; round < 2; round++ {
				s.Set(tt.set)
				for n := 1; n <= tt.draws; n++ {
					assert.Equal(t, SequenceUUID(tt.seed, uint64(n)), s.NewV4(), "round %d draw %d", round, n)
				}
			}
		})
	}
}

func TestSourceNewV4WithoutSet(t *testing.T) {
	s := &Source{}
	assert.Equal(t, SequenceUUID(DefaultUUID, 1), s.NewV4())
	assert.Equal(t, SequenceUUID(DefaultUUID, 2), s.NewV4())
}

func TestSourceNewV4Concurrent(t *testing.T) {
	const draws = 64
	s := &Source{}
	s.Set(nil)
	got := make([]uuid.UUID, draws)
	var wg sync.WaitGroup
	for i := range got {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			got[i] = s.NewV4()
		}(i)
	}
	wg.Wait()
	want := map[uuid.UUID]bool{}
	for n := 1; n <= draws; n++ {
		want[SequenceUUID(DefaultUUID, uint64(n))] = true
	}
	seen := map[uuid.UUID]bool{}
	for _, u := range got {
		assert.True(t, want[u], "%s is not one of the first %d stream values", u, draws)
		assert.False(t, seen[u], "%s drawn twice", u)
		seen[u] = true
	}
}

func TestInstallRoutesNewV4(t *testing.T) {
	Uninstall()
	t.Cleanup(Uninstall)

	src := Install()
	src.Set(nil)
	for n := 1; n <= 2; n++ {
		u, err := uuid.NewV4()
		require.NoError(t, err)
		assert.Equal(t, SequenceUUID(DefaultUUID, uint64(n)), u)
	}

	Uninstall()
	a, err := uuid.NewV4()
	require.NoError(t, err)
	b, err := uuid.NewV4()
	require.NoError(t, err)
	assert.NotEqual(t, a, b, "uninstalled NewV4 must be random again")
	assert.Equal(t, byte(uuid.V4), a.Version())
}
