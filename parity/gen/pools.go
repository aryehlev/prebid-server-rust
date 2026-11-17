package gen

import (
	"encoding/json"
	"fmt"

	"github.com/prebid/prebid-server/v4/parity/fixtures/pools"
)

// Pools are the decoded value pools of parity/fixtures/pools.
type Pools struct {
	ORTB    ORTBPools
	Privacy PrivacyPools
	Values  ValuePools
}

// ORTBPools are realistic OpenRTB values (ortb.json).
type ORTBPools struct {
	UserAgents     []string            `json:"user_agents"`
	IPv4           []string            `json:"ipv4"`
	PrivateIPv4    []string            `json:"private_ipv4"`
	IPv6           []string            `json:"ipv6"`
	Sites          []SitePool          `json:"sites"`
	Apps           []AppPool           `json:"apps"`
	DOOH           []DOOHPool          `json:"dooh"`
	BannerSizes    [][2]int            `json:"banner_sizes"`
	VideoMimes     [][]string          `json:"video_mimes"`
	VideoProtocols [][]int             `json:"video_protocols"`
	VideoSizes     [][2]int            `json:"video_sizes"`
	VideoDurations [][2]int            `json:"video_durations"`
	AudioMimes     [][]string          `json:"audio_mimes"`
	NativeRequests []string            `json:"native_requests"`
	Countries      []string            `json:"countries"`
	EEACountries   []string            `json:"eea_countries"`
	Regions        []string            `json:"regions"`
	Cities         []string            `json:"cities"`
	Languages      []string            `json:"languages"`
	Currencies     []string            `json:"currencies"`
	IABCategories  []string            `json:"iab_categories"`
	ADomains       []string            `json:"adomains"`
	Keywords       []string            `json:"keywords"`
	EIDSources     []string            `json:"eid_sources"`
	SChainNodes    []json.RawMessage   `json:"schain_nodes"`
	Fields         map[string][]string `json:"fields"`
}

// SitePool is one site template.
type SitePool struct {
	Domain    string   `json:"domain"`
	Page      string   `json:"page"`
	Ref       string   `json:"ref"`
	Publisher string   `json:"publisher"`
	Name      string   `json:"name"`
	Cat       []string `json:"cat"`
}

// AppPool is one app template.
type AppPool struct {
	Bundle    string   `json:"bundle"`
	Name      string   `json:"name"`
	StoreURL  string   `json:"storeurl"`
	ID        string   `json:"id"`
	Publisher string   `json:"publisher"`
	Ver       string   `json:"ver"`
	Cat       []string `json:"cat"`
}

// DOOHPool is one digital-out-of-home template.
type DOOHPool struct {
	ID        string   `json:"id"`
	Name      string   `json:"name"`
	VenueType []string `json:"venuetype"`
	Publisher string   `json:"publisher"`
}

// PrivacyPools are consent and privacy strings (privacy.json).
type PrivacyPools struct {
	TCFValid         []string   `json:"tcf_valid"`
	TCFv1            []string   `json:"tcf_v1"`
	TCFInvalid       []string   `json:"tcf_invalid"`
	GPP              []GPPEntry `json:"gpp"`
	USPrivacyValid   []string   `json:"us_privacy_valid"`
	USPrivacyInvalid []string   `json:"us_privacy_invalid"`
}

// GPPEntry is one GPP string with the section IDs its header lists.
type GPPEntry struct {
	GPP         string `json:"gpp"`
	SIDs        []int  `json:"sids"`
	HeaderValid bool   `json:"header_valid"`
}

// ValuePools are the edge values of the structural and malformed mutators
// (values.json).
type ValuePools struct {
	BoundaryNumbers []string `json:"boundary_numbers"`
	UnicodeStrings  []string `json:"unicode_strings"`
	// TMax are the tmax.edge values. They hold no value below 250ms: with a
	// tmax shorter than a case takes to reach its mocked bidders, whether a
	// bidder answers in time depends on scheduling, and two Go processes
	// disagree on the same case (parity/SELFTEST.md).
	TMax             []int    `json:"tmax"`
	ValidTMax        []int    `json:"valid_tmax"`
	NestingDepths    []int    `json:"nesting_depths"`
	TrailingGarbage  []string `json:"trailing_garbage"`
	AltBodies        []string `json:"alt_bodies"`
	ContentTypes     []string `json:"content_types"`
	ContentEncodings []string `json:"content_encodings"`
	Methods          []string `json:"methods"`
}

// LoadPools decodes the embedded pool files.
func LoadPools() (*Pools, error) {
	p := &Pools{}
	files := []struct {
		name string
		dst  any
	}{
		{pools.ORTBFile, &p.ORTB},
		{pools.PrivacyFile, &p.Privacy},
		{pools.ValuesFile, &p.Values},
	}
	for _, f := range files {
		raw, err := pools.FS.ReadFile(f.name)
		if err != nil {
			return nil, fmt.Errorf("gen: reading pool %s: %w", f.name, err)
		}
		if err := json.Unmarshal(raw, f.dst); err != nil {
			return nil, fmt.Errorf("gen: decoding pool %s: %w", f.name, err)
		}
	}
	if err := p.check(); err != nil {
		return nil, err
	}
	return p, nil
}

// check rejects empty pools the generator draws from unconditionally.
func (p *Pools) check() error {
	required := map[string]int{
		"user_agents":      len(p.ORTB.UserAgents),
		"ipv4":             len(p.ORTB.IPv4),
		"ipv6":             len(p.ORTB.IPv6),
		"private_ipv4":     len(p.ORTB.PrivateIPv4),
		"eid_sources":      len(p.ORTB.EIDSources),
		"schain_nodes":     len(p.ORTB.SChainNodes),
		"iab_categories":   len(p.ORTB.IABCategories),
		"adomains":         len(p.ORTB.ADomains),
		"tcf_v1":           len(p.Privacy.TCFv1),
		"us_privacy_bad":   len(p.Privacy.USPrivacyInvalid),
		"sites":            len(p.ORTB.Sites),
		"apps":             len(p.ORTB.Apps),
		"dooh":             len(p.ORTB.DOOH),
		"banner_sizes":     len(p.ORTB.BannerSizes),
		"video_mimes":      len(p.ORTB.VideoMimes),
		"video_protocols":  len(p.ORTB.VideoProtocols),
		"video_sizes":      len(p.ORTB.VideoSizes),
		"video_durations":  len(p.ORTB.VideoDurations),
		"audio_mimes":      len(p.ORTB.AudioMimes),
		"native_requests":  len(p.ORTB.NativeRequests),
		"countries":        len(p.ORTB.Countries),
		"eea_countries":    len(p.ORTB.EEACountries),
		"currencies":       len(p.ORTB.Currencies),
		"tcf_valid":        len(p.Privacy.TCFValid),
		"tcf_invalid":      len(p.Privacy.TCFInvalid),
		"gpp":              len(p.Privacy.GPP),
		"us_privacy_valid": len(p.Privacy.USPrivacyValid),
		"boundary_numbers": len(p.Values.BoundaryNumbers),
		"unicode_strings":  len(p.Values.UnicodeStrings),
		"valid_tmax":       len(p.Values.ValidTMax),
		"alt_bodies":       len(p.Values.AltBodies),
	}
	for _, name := range sortedKeys(required) {
		if required[name] == 0 {
			return fmt.Errorf("gen: pool %s is empty", name)
		}
	}
	return nil
}
