package oracle

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"sync"

	"github.com/prebid/prebid-server/v4/parity"
)

// Static fixture locations, relative to Options.FixturesDir.
const (
	currencyFixture = "currency/latest.json"
	gvlFixtureFmt   = "gvl/v%d/vendor-list.json"
)

// Static URL paths the oracle serves.
const (
	// CurrencyPath is the default currency file path (currency_converter.fetch_url).
	CurrencyPath = "/currency/latest.json"
	// currencyProfilePrefix prefixes per-profile currency files.
	currencyProfilePrefix = "/currency/profiles/"
	// VendorListHost is the GVL host the Go and Rust servers fetch from.
	VendorListHost = "vendor-list.consensu.org"
)

// gvlSpecs are the GVL specification versions with snapshots.
var gvlSpecs = []uint16{2, 3}

// gvlPathRE matches /v{spec}/vendor-list.json and
// /v{spec}/archives/vendor-list-v{N}.json.
var gvlPathRE = regexp.MustCompile(`^/v(\d+)/(?:vendor-list\.json|archives/vendor-list-v(\d+)\.json)$`)

// staticFiles serves the fixed currency rates and GVL snapshots.
type staticFiles struct {
	currency []byte
	gvl      map[uint16]*gvlSnapshot

	mu       sync.RWMutex
	profiles map[string][]byte
}

// gvlSnapshot is one spec version's latest vendor list.
type gvlSnapshot struct {
	latest  uint16
	raw     []byte
	members map[string]json.RawMessage

	mu       sync.Mutex
	archives map[uint16][]byte
}

// loadStatic reads the currency and GVL fixtures from dir.
func loadStatic(dir string) (*staticFiles, error) {
	s := &staticFiles{gvl: map[uint16]*gvlSnapshot{}, profiles: map[string][]byte{}}
	currency, err := os.ReadFile(filepath.Join(dir, filepath.FromSlash(currencyFixture)))
	if err != nil {
		return nil, fmt.Errorf("oracle: reading currency fixture: %w", err)
	}
	if err := validateRates(currency); err != nil {
		return nil, fmt.Errorf("oracle: %s: %w", currencyFixture, err)
	}
	s.currency = currency
	for _, spec := range gvlSpecs {
		snap, err := loadGVL(dir, spec)
		if err != nil {
			return nil, err
		}
		s.gvl[spec] = snap
	}
	return s, nil
}

// validateRates checks the Go currency file shape: {"dataAsOf", "conversions"}.
func validateRates(raw []byte) error {
	var rates struct {
		DataAsOf    string                        `json:"dataAsOf"`
		Conversions map[string]map[string]float64 `json:"conversions"`
	}
	if err := json.Unmarshal(raw, &rates); err != nil {
		return fmt.Errorf("currency rates: %w", err)
	}
	if rates.DataAsOf == "" || len(rates.Conversions) == 0 {
		return fmt.Errorf("currency rates need dataAsOf and conversions")
	}
	return nil
}

func loadGVL(dir string, spec uint16) (*gvlSnapshot, error) {
	rel := fmt.Sprintf(gvlFixtureFmt, spec)
	raw, err := os.ReadFile(filepath.Join(dir, filepath.FromSlash(rel)))
	if err != nil {
		return nil, fmt.Errorf("oracle: reading GVL fixture: %w", err)
	}
	var members map[string]json.RawMessage
	if err := json.Unmarshal(raw, &members); err != nil {
		return nil, fmt.Errorf("oracle: %s: %w", rel, err)
	}
	var head struct {
		Spec    uint16 `json:"gvlSpecificationVersion"`
		Version uint16 `json:"vendorListVersion"`
	}
	if err := json.Unmarshal(raw, &head); err != nil {
		return nil, fmt.Errorf("oracle: %s: %w", rel, err)
	}
	if head.Spec != spec || head.Version == 0 {
		return nil, fmt.Errorf("oracle: %s: gvlSpecificationVersion %d / vendorListVersion %d are invalid", rel, head.Spec, head.Version)
	}
	return &gvlSnapshot{latest: head.Version, raw: raw, members: members, archives: map[uint16][]byte{}}, nil
}

// archive returns the snapshot re-labelled as list version n (1 <= n <= latest).
func (g *gvlSnapshot) archive(n uint16) ([]byte, error) {
	if n == g.latest {
		return g.raw, nil
	}
	g.mu.Lock()
	defer g.mu.Unlock()
	if raw, ok := g.archives[n]; ok {
		return raw, nil
	}
	members := make(map[string]json.RawMessage, len(g.members))
	for k, v := range g.members {
		members[k] = v
	}
	members["vendorListVersion"] = json.RawMessage(strconv.Itoa(int(n)))
	raw, err := json.Marshal(members)
	if err != nil {
		return nil, fmt.Errorf("oracle: building GVL archive v%d: %w", n, err)
	}
	g.archives[n] = raw
	return raw, nil
}

// setProfileRates registers the currency file of one profile and returns its path.
func (s *staticFiles) setProfileRates(profile string, rates map[string]map[string]float64, dataAsOf string) (string, error) {
	raw, err := json.Marshal(struct {
		DataAsOf    string                        `json:"dataAsOf"`
		Conversions map[string]map[string]float64 `json:"conversions"`
	}{dataAsOf, rates})
	if err != nil {
		return "", fmt.Errorf("oracle: encoding currency rates for %s: %w", profile, err)
	}
	p := currencyProfilePrefix + profile + "/latest.json"
	s.mu.Lock()
	s.profiles[p] = raw
	s.mu.Unlock()
	return p, nil
}

// currencyFor returns the currency file served at path.
func (s *staticFiles) currencyFor(path string) ([]byte, string, bool) {
	if path == CurrencyPath {
		return s.currency, currencyFixture, true
	}
	s.mu.RLock()
	defer s.mu.RUnlock()
	raw, ok := s.profiles[path]
	return raw, path, ok
}

// gvlFor resolves a vendor-list path. found is false for paths that are not
// GVL paths at all; a GVL path for an unknown version yields a 404 reply.
func (s *staticFiles) gvlFor(path string) (reply, bool) {
	m := gvlPathRE.FindStringSubmatch(path)
	if m == nil {
		return reply{}, false
	}
	spec64, err := strconv.ParseUint(m[1], 10, 16)
	snap, ok := s.gvl[uint16(spec64)]
	if err != nil || !ok {
		return notFound(parity.MockSourceStatic, "gvl:unknown-spec", "gvl_spec_missing"), true
	}
	version := snap.latest
	id := fmt.Sprintf("gvl/v%d/vendor-list.json", spec64)
	if m[2] != "" {
		n, err := strconv.ParseUint(m[2], 10, 16)
		if err != nil || n == 0 || uint16(n) > snap.latest {
			return notFound(parity.MockSourceStatic, fmt.Sprintf("gvl/v%d/archives/vendor-list-v%s.json", spec64, m[2]), "gvl_version_missing"), true
		}
		version = uint16(n)
		id = fmt.Sprintf("gvl/v%d/archives/vendor-list-v%d.json", spec64, n)
	}
	raw, err := snap.archive(version)
	if err != nil {
		return reply{status: 500, mock: parity.MockRef{Source: parity.MockSourceStatic, ID: id}, notes: []string{err.Error()}}, true
	}
	return jsonReply(200, raw, parity.MockRef{Source: parity.MockSourceStatic, ID: id}), true
}

// serve answers static paths: the currency files on any host and the GVL on
// vendor-list.consensu.org.
func (s *staticFiles) serve(host, path string) (reply, bool) {
	if raw, id, ok := s.currencyFor(path); ok {
		return jsonReply(200, raw, parity.MockRef{Source: parity.MockSourceStatic, ID: id}), true
	}
	if host == VendorListHost {
		return s.gvlFor(path)
	}
	return reply{}, false
}
