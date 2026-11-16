package shadow

import (
	"encoding/json"
	"testing"
	"time"

	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
	"github.com/prebid/prebid-server/v4/util/ptrutil"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

const baseRequest = `{
  "id": "req-1",
  "imp": [{"id": "imp-1", "banner": {"format": [{"w": 300, "h": 250}]}, "ext": {"prebid": {"bidder": {"appnexus": {"placementId": 1}}}, "tid": "t-in"}}],
  "site": {"page": "https://example.com", "ext": {"custom": 1}},
  "device": {"ua": "UA", "ext": {"keep": true}},
  "user": {"ext": {"consent": "OLD", "other": [1, 2]}},
  "regs": {"ext": {"gdpr": 0}},
  "source": {"tid": "src", "ext": {"unknown": "x"}},
  "ext": {"prebid": {"debug": true, "unknownfield": {"a": 1}}}
}`

func newWrapper(t *testing.T) *openrtb_ext.RequestWrapper {
	t.Helper()
	req := &openrtb2.BidRequest{}
	require.NoError(t, jsonutil.UnmarshalValid([]byte(baseRequest), req))
	return &openrtb_ext.RequestWrapper{BidRequest: req}
}

func mustMarshal(t *testing.T, v any) string {
	t.Helper()
	raw, err := jsonutil.Marshal(v)
	require.NoError(t, err)
	return string(raw)
}

// TestRebuiltRequestMatchesLiveRebuild checks the two properties the endpoint
// hooks rely on: the snapshot equals what Go's own rebuild produces later, and
// taking it changes nothing (the later live rebuild still applies every dirty
// change, so no dirty flag or ext cache was touched).
func TestRebuiltRequestMatchesLiveRebuild(t *testing.T) {
	tests := []struct {
		name   string
		mutate func(t *testing.T, rw *openrtb_ext.RequestWrapper)
	}{
		{
			name:   "untouched wrapper",
			mutate: func(*testing.T, *openrtb_ext.RequestWrapper) {},
		},
		{
			name: "request ext prebid with fields RequestWrapper.Clone drops",
			mutate: func(t *testing.T, rw *openrtb_ext.RequestWrapper) {
				reqExt, err := rw.GetRequestExt()
				require.NoError(t, err)
				prebid := reqExt.GetPrebid()
				prebid.Targeting = &openrtb_ext.ExtRequestTargeting{
					AlwaysIncludeDeals:        true,
					MediaTypePriceGranularity: &openrtb_ext.MediaTypePriceGranularity{Banner: &openrtb_ext.PriceGranularity{Precision: ptrutil.ToPtr(2)}},
				}
				reqExt.SetPrebid(prebid)
			},
		},
		{
			name: "user ext consent and device ext prebid",
			mutate: func(t *testing.T, rw *openrtb_ext.RequestWrapper) {
				userExt, err := rw.GetUserExt()
				require.NoError(t, err)
				userExt.SetConsent(ptrutil.ToPtr("NEW"))
				deviceExt, err := rw.GetDeviceExt()
				require.NoError(t, err)
				deviceExt.SetPrebid(&openrtb_ext.ExtDevicePrebid{Interstitial: &openrtb_ext.ExtDeviceInt{MinWidthPerc: 50, MinHeightPerc: 40}})
			},
		},
		{
			name: "imp ext tid through accessed imp wrappers",
			mutate: func(t *testing.T, rw *openrtb_ext.RequestWrapper) {
				impExt, err := rw.GetImp()[0].GetImpExt()
				require.NoError(t, err)
				impExt.SetTid("t-new")
			},
		},
		{
			name: "regs, site and source ext plus plain field edits",
			mutate: func(t *testing.T, rw *openrtb_ext.RequestWrapper) {
				regExt, err := rw.GetRegExt()
				require.NoError(t, err)
				regExt.SetGDPR(ptrutil.ToPtr[int8](1))
				siteExt, err := rw.GetSiteExt()
				require.NoError(t, err)
				siteExt.SetAmp(ptrutil.ToPtr[int8](1))
				sourceExt, err := rw.GetSourceExt()
				require.NoError(t, err)
				sourceExt.SetSChain(&openrtb2.SupplyChain{Ver: "1.0", Complete: 1})
				rw.TMax = 500
				rw.Cur = []string{"EUR"}
			},
		},
		{
			name: "ext objects created where the request had none",
			mutate: func(t *testing.T, rw *openrtb_ext.RequestWrapper) {
				rw.App = &openrtb2.App{ID: "app"}
				appExt, err := rw.GetAppExt()
				require.NoError(t, err)
				appExt.SetPrebid(&openrtb_ext.ExtAppPrebid{Source: "s", Version: "1"})
				doohExt, err := rw.GetDOOHExt()
				require.NoError(t, err)
				doohExt.SetExt(map[string]json.RawMessage{"k": json.RawMessage(`"v"`)})
			},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			live := newWrapper(t)
			tt.mutate(t, live)
			before := mustMarshal(t, live.BidRequest)

			snap, err := RebuiltRequest(live)
			require.NoError(t, err)

			assert.Equal(t, before, mustMarshal(t, live.BidRequest), "the live BidRequest is unchanged")
			require.NoError(t, live.RebuildRequest())
			assert.JSONEq(t, mustMarshal(t, live.BidRequest), mustMarshal(t, snap), "snapshot equals the live rebuild")
			assert.Equal(t, mustMarshal(t, live.BidRequest), mustMarshal(t, snap), "lexically equal too")
		})
	}
}

// TestRebuiltRequestIsRepeatable takes two snapshots in a row: the first must
// not consume the dirty state the second (and the live rebuild) depend on.
func TestRebuiltRequestIsRepeatable(t *testing.T) {
	live := newWrapper(t)
	reqExt, err := live.GetRequestExt()
	require.NoError(t, err)
	reqExt.SetPrebid(&openrtb_ext.ExtRequestPrebid{Integration: "web"})

	first, err := RebuiltRequest(live)
	require.NoError(t, err)
	second, err := RebuiltRequest(live)
	require.NoError(t, err)
	assert.Equal(t, mustMarshal(t, first), mustMarshal(t, second))
	assert.Contains(t, mustMarshal(t, first), `"integration":"web"`)
	assert.NotSame(t, first, second)
	assert.NotSame(t, live.BidRequest, first)
}

func TestRebuiltRequestWithoutRequest(t *testing.T) {
	tests := []struct {
		name string
		rw   *openrtb_ext.RequestWrapper
	}{
		{name: "nil wrapper", rw: nil},
		{name: "nil BidRequest", rw: &openrtb_ext.RequestWrapper{}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req, err := RebuiltRequest(tt.rw)
			assert.ErrorIs(t, err, ErrNoRequest)
			assert.Nil(t, req)
		})
	}
}

// node exercises unexported fields, aliasing, cycles, maps, interfaces and
// shared standard-library types.
type node struct {
	name     string
	next     *node
	alias    *node
	tags     map[string][]string
	raw      json.RawMessage
	any      any
	when     time.Time
	arr      [2]*int
	callback func() int
}

func TestCopy(t *testing.T) {
	one := 1
	cb := func() int { return 7 }
	root := &node{name: "root", tags: map[string][]string{"k": {"a", "b"}}, raw: json.RawMessage(`{"x":1}`),
		any: &node{name: "boxed"}, when: time.Unix(1629473330, 0).UTC(), arr: [2]*int{&one, nil}, callback: cb}
	child := &node{name: "child", next: root}
	root.next = child
	root.alias = child

	cp, err := Copy(root)
	require.NoError(t, err)

	tests := []struct {
		name  string
		check func(t *testing.T)
	}{
		{name: "values copied", check: func(t *testing.T) {
			assert.Equal(t, "root", cp.name)
			assert.Equal(t, "child", cp.next.name)
			assert.Equal(t, root.tags, cp.tags)
			assert.Equal(t, string(root.raw), string(cp.raw))
			assert.Equal(t, "boxed", cp.any.(*node).name)
			assert.True(t, root.when.Equal(cp.when))
			assert.Equal(t, 1, *cp.arr[0])
			assert.Equal(t, 7, cp.callback())
		}},
		{name: "cycle and aliasing preserved", check: func(t *testing.T) {
			assert.Same(t, cp, cp.next.next)
			assert.Same(t, cp.next, cp.alias)
		}},
		{name: "independent of the original", check: func(t *testing.T) {
			assert.NotSame(t, root, cp)
			assert.NotSame(t, root.next, cp.next)
			assert.NotSame(t, root.any, cp.any)
			assert.NotSame(t, root.arr[0], cp.arr[0])
			cp.tags["k"][0] = "changed"
			cp.raw[2] = 'y'
			cp.next.name = "changed"
			assert.Equal(t, "a", root.tags["k"][0])
			assert.Equal(t, `{"x":1}`, string(root.raw))
			assert.Equal(t, "child", root.next.name)
		}},
	}
	for _, tt := range tests {
		t.Run(tt.name, tt.check)
	}
}

func TestCopyNil(t *testing.T) {
	var n *node
	cp, err := Copy(n)
	assert.NoError(t, err)
	assert.Nil(t, cp)
}
