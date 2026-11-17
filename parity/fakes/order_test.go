package fakes

import (
	"cmp"
	"sort"
	"testing"

	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/stretchr/testify/assert"
)

func TestMapKeys(t *testing.T) {
	type group struct {
		SourceID int
		Host     string
	}
	byGroup := func(a, b group) bool {
		if a.SourceID != b.SourceID {
			return a.SourceID < b.SourceID
		}
		return a.Host < b.Host
	}
	tests := []struct {
		name      string
		installed bool
		m         map[group]int
		want      []group
	}{
		{name: "empty", installed: true, m: map[group]int{}, want: []group{}},
		{name: "installed sorts by less", installed: true,
			m:    map[group]int{{3, "a"}: 1, {1, "b"}: 2, {1, "a"}: 3, {2, "z"}: 4},
			want: []group{{1, "a"}, {1, "b"}, {2, "z"}, {3, "a"}}},
		{name: "not installed keeps every key", installed: false,
			m:    map[group]int{{3, "a"}: 1, {1, "b"}: 2, {1, "a"}: 3, {2, "z"}: 4},
			want: []group{{1, "a"}, {1, "b"}, {2, "z"}, {3, "a"}}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			Uninstall()
			t.Cleanup(Uninstall)
			if tt.installed {
				Install()
			}
			got := MapKeys(tt.m, byGroup)
			if !tt.installed {
				sort.Slice(got, func(i, j int) bool { return byGroup(got[i], got[j]) })
			}
			assert.Equal(t, tt.want, got)
		})
	}
}

func TestMapKeysInstalledIsStable(t *testing.T) {
	Uninstall()
	t.Cleanup(Uninstall)
	Install()
	m := map[int]string{}
	for i := 0; i < 64; i++ {
		m[i*7%64] = "x"
	}
	first := MapKeys(m, cmp.Less[int])
	for i := 0; i < 20; i++ {
		assert.Equal(t, first, MapKeys(m, cmp.Less[int]))
	}
	assert.True(t, sort.IntsAreSorted(first))
}

func TestAdapterGroupOrders(t *testing.T) {
	type dbk = openrtb_ext.ExtImpDatablocks
	type zcf = openrtb_ext.ExtImpZeroClickFraud
	type tg = openrtb_ext.ExtImpTrafficGate
	tests := []struct {
		name string
		less bool
		want bool
	}{
		{"datablocks by sourceId", LessDatablocks(dbk{SourceId: 1}, dbk{SourceId: 2}), true},
		{"datablocks equal", LessDatablocks(dbk{SourceId: 2}, dbk{SourceId: 2}), false},
		{"zeroclickfraud by sourceId first", LessZeroClickFraud(zcf{SourceId: 1, Host: "z"}, zcf{SourceId: 2, Host: "a"}), true},
		{"zeroclickfraud then host", LessZeroClickFraud(zcf{SourceId: 1, Host: "b"}, zcf{SourceId: 1, Host: "a"}), false},
		{"trafficgate by placement first", LessTrafficGate(tg{PlacementID: "a", Host: "z"}, tg{PlacementID: "b", Host: "a"}), true},
		{"trafficgate then host", LessTrafficGate(tg{PlacementID: "a", Host: "a"}, tg{PlacementID: "a", Host: "b"}), true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assert.Equal(t, tt.want, tt.less)
		})
	}
}
