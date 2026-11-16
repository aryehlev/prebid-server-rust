package corpus

import (
	"bytes"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
)

// DefaultProfile is the name of the profile with no fixture-specific settings.
const DefaultProfile = "default"

// Profile is the host configuration a case needs on top of the runner's
// default parity profile. It is derived from a sample request's "config" block
// or from exchangetest flags. Two cases with equal profiles can share one pair
// of server processes; Name is a stable identifier of the content.
//
// Fields map to host config keys as Overrides documents. Settings the Go test
// injects into internals with no config equivalent are not represented here;
// the loaders record them in Case.Notes instead.
type Profile struct {
	// AccountRequired -> account_required.
	AccountRequired bool `json:"account_required,omitempty"`
	// BlockedApps -> blocked_apps.
	BlockedApps []string `json:"blocked_apps,omitempty"`
	// DisabledAdapters -> adapters.<bidder>.disabled = true.
	DisabledAdapters []string `json:"disabled_adapters,omitempty"`
	// CurrencyRates are the rates the oracle serves as the currency file
	// (the servers fetch them through currency_converter.fetch_url).
	CurrencyRates map[string]map[string]float64 `json:"currency_rates,omitempty"`
	// DefaultRequest is the default request JSON (default_request.type=file);
	// the runner writes it to a file.
	DefaultRequest json.RawMessage `json:"default_request,omitempty"`
	// BidderInfoOverrides are per-bidder bidder-info overrides
	// (adapters.<bidder>.*), as the sample-request "bidderInfoOverrides".
	BidderInfoOverrides map[string]json.RawMessage `json:"bidder_info_overrides,omitempty"`
	// PreferredMediaType -> account_defaults.preferredmediatype.
	PreferredMediaType json.RawMessage `json:"preferred_media_type,omitempty"`
	// GDPREnabled -> gdpr.enabled (nil keeps the default profile's value).
	GDPREnabled *bool `json:"gdpr_enabled,omitempty"`
	// GDPRDefaultValue -> gdpr.default_value.
	GDPRDefaultValue string `json:"gdpr_default_value,omitempty"`
	// GDPREEACountries -> gdpr.eea_countries.
	GDPREEACountries []string `json:"gdpr_eea_countries,omitempty"`
	// CCPAEnforce -> ccpa.enforce.
	CCPAEnforce bool `json:"ccpa_enforce,omitempty"`
	// LMTEnforce -> lmt.enforce (nil keeps the default profile's value).
	LMTEnforce *bool `json:"lmt_enforce,omitempty"`
	// FloorsEnabled -> price_floors.enabled.
	FloorsEnabled bool `json:"floors_enabled,omitempty"`
	// AccountFloorsEnabled -> account_defaults.price_floors.enabled.
	AccountFloorsEnabled bool `json:"account_floors_enabled,omitempty"`
	// AccountEnforceDealFloors -> account_defaults.price_floors.enforce_deal_floors.
	AccountEnforceDealFloors bool `json:"account_enforce_deal_floors,omitempty"`
	// EventsEnabled -> account_defaults.events.enabled.
	EventsEnabled bool `json:"events_enabled,omitempty"`
	// HostSChainNode -> host_schain_node.
	HostSChainNode json.RawMessage `json:"host_schain_node,omitempty"`
	// HostValidations -> validations.
	HostValidations json.RawMessage `json:"host_validations,omitempty"`
	// AccountValidations -> account_defaults.validations.
	AccountValidations json.RawMessage `json:"account_validations,omitempty"`
	// AccountDefaultBidLimit -> account_defaults.default_bid_limit.
	AccountDefaultBidLimit int `json:"account_default_bid_limit,omitempty"`
	// AccountPrivacy -> account_defaults.privacy.
	AccountPrivacy json.RawMessage `json:"account_privacy,omitempty"`
	// AccountEEACountries -> account_defaults.gdpr.eea_countries.
	AccountEEACountries []string `json:"account_eea_countries,omitempty"`
	// ExternalURL -> external_url.
	ExternalURL string `json:"external_url,omitempty"`
	// HostVendorID -> gdpr.host_vendor_id.
	HostVendorID int `json:"host_vendor_id,omitempty"`
	// DataCenter -> datacenter.
	DataCenter string `json:"datacenter,omitempty"`
	// ORTBVersions -> adapters.<bidder>.openrtb.version.
	ORTBVersions map[string]string `json:"ortb_versions,omitempty"`
}

// profileFeature pairs a name token with a presence test.
type profileFeature struct {
	token string
	set   func(p *Profile) bool
}

var profileFeatures = []profileFeature{
	{"account-required", func(p *Profile) bool { return p.AccountRequired }},
	{"blocked-apps", func(p *Profile) bool { return len(p.BlockedApps) > 0 }},
	{"disabled-adapters", func(p *Profile) bool { return len(p.DisabledAdapters) > 0 }},
	{"currency", func(p *Profile) bool { return len(p.CurrencyRates) > 0 }},
	{"default-request", func(p *Profile) bool { return len(p.DefaultRequest) > 0 }},
	{"bidder-info", func(p *Profile) bool { return len(p.BidderInfoOverrides) > 0 || len(p.ORTBVersions) > 0 }},
	{"preferred-media-type", func(p *Profile) bool { return len(p.PreferredMediaType) > 0 }},
	{"gdpr", func(p *Profile) bool {
		return p.GDPREnabled != nil || p.GDPRDefaultValue != "" || len(p.GDPREEACountries) > 0 || len(p.AccountEEACountries) > 0
	}},
	{"ccpa", func(p *Profile) bool { return p.CCPAEnforce }},
	{"lmt", func(p *Profile) bool { return p.LMTEnforce != nil }},
	{"floors", func(p *Profile) bool { return p.FloorsEnabled || p.AccountFloorsEnabled || p.AccountEnforceDealFloors }},
	{"events", func(p *Profile) bool { return p.EventsEnabled }},
	{"schain", func(p *Profile) bool { return len(p.HostSChainNode) > 0 }},
	{"validations", func(p *Profile) bool { return len(p.HostValidations) > 0 || len(p.AccountValidations) > 0 }},
	{"multibid", func(p *Profile) bool { return p.AccountDefaultBidLimit != 0 }},
	{"privacy", func(p *Profile) bool { return len(p.AccountPrivacy) > 0 }},
	{"server", func(p *Profile) bool { return p.ExternalURL != "" || p.HostVendorID != 0 || p.DataCenter != "" }},
}

// Tokens returns the feature tokens of the settings p carries, in a fixed order.
func (p Profile) Tokens() []string {
	var out []string
	for _, f := range profileFeatures {
		if f.set(&p) {
			out = append(out, f.token)
		}
	}
	return out
}

// IsDefault reports whether p carries no settings.
func (p Profile) IsDefault() bool {
	return len(p.Tokens()) == 0
}

// Name returns "default" for an empty profile and otherwise
// "<token>+<token>-<hash8>", where the hash covers the canonical JSON of p, so
// equal settings always get the same name and different settings never do.
func (p Profile) Name() string {
	tokens := p.Tokens()
	if len(tokens) == 0 {
		return DefaultProfile
	}
	canonical, err := json.Marshal(p)
	if err != nil {
		// Profile only holds JSON-safe values; fall back to the token list.
		return strings.Join(tokens, "+")
	}
	return strings.Join(tokens, "+") + "-" + shortHash(8, string(canonical))
}

// Overrides returns the host config settings of p as dotted config keys (the
// keys of config.SetupViper / pbs.yaml) with JSON-compatible values. Currency
// rates and the default request are not config values; the runner serves the
// former from the oracle and writes the latter to a file.
func (p Profile) Overrides() (map[string]any, error) {
	out := map[string]any{}
	setIf := func(ok bool, key string, v any) {
		if ok {
			out[key] = v
		}
	}
	setIf(p.AccountRequired, "account_required", true)
	setIf(len(p.BlockedApps) > 0, "blocked_apps", p.BlockedApps)
	for _, b := range p.DisabledAdapters {
		out["adapters."+strings.ToLower(b)+".disabled"] = true
	}
	setIf(p.GDPREnabled != nil, "gdpr.enabled", derefBool(p.GDPREnabled))
	setIf(p.GDPRDefaultValue != "", "gdpr.default_value", p.GDPRDefaultValue)
	setIf(len(p.GDPREEACountries) > 0, "gdpr.eea_countries", p.GDPREEACountries)
	setIf(p.CCPAEnforce, "ccpa.enforce", true)
	setIf(p.LMTEnforce != nil, "lmt.enforce", derefBool(p.LMTEnforce))
	setIf(p.FloorsEnabled, "price_floors.enabled", true)
	setIf(p.AccountFloorsEnabled, "account_defaults.price_floors.enabled", true)
	setIf(p.AccountEnforceDealFloors, "account_defaults.price_floors.enforce_deal_floors", true)
	setIf(p.EventsEnabled, "account_defaults.events.enabled", true)
	setIf(p.AccountDefaultBidLimit != 0, "account_defaults.default_bid_limit", p.AccountDefaultBidLimit)
	setIf(len(p.AccountEEACountries) > 0, "account_defaults.gdpr.eea_countries", p.AccountEEACountries)
	setIf(p.ExternalURL != "", "external_url", p.ExternalURL)
	setIf(p.HostVendorID != 0, "gdpr.host_vendor_id", p.HostVendorID)
	setIf(p.DataCenter != "", "datacenter", p.DataCenter)
	for _, b := range sortedKeys(p.ORTBVersions) {
		if p.ORTBVersions[b] != "" {
			out["adapters."+strings.ToLower(b)+".openrtb.version"] = p.ORTBVersions[b]
		}
	}
	jsonValued := []struct {
		key string
		raw json.RawMessage
	}{
		{"account_defaults.preferredmediatype", p.PreferredMediaType},
		{"host_schain_node", p.HostSChainNode},
		{"validations", p.HostValidations},
		{"account_defaults.validations", p.AccountValidations},
		{"account_defaults.privacy", p.AccountPrivacy},
	}
	for _, jv := range jsonValued {
		if len(jv.raw) == 0 {
			continue
		}
		v, err := decodeJSONValue(jv.raw)
		if err != nil {
			return nil, fmt.Errorf("corpus: profile %s: %w", jv.key, err)
		}
		out[jv.key] = v
	}
	if err := bidderInfoOverrides(p.BidderInfoOverrides, out); err != nil {
		return nil, err
	}
	return out, nil
}

// bidderInfoOverrides flattens {"openrtb":{"version":..}, "capabilities":{..}}
// into adapters.<bidder>.<key> entries.
func bidderInfoOverrides(overrides map[string]json.RawMessage, out map[string]any) error {
	for _, bidder := range sortedKeys(overrides) {
		var fields map[string]json.RawMessage
		if err := json.Unmarshal(overrides[bidder], &fields); err != nil {
			return fmt.Errorf("corpus: bidder info overrides for %s: %w", bidder, err)
		}
		for _, field := range sortedKeys(fields) {
			v, err := decodeJSONValue(fields[field])
			if err != nil {
				return fmt.Errorf("corpus: bidder info override %s.%s: %w", bidder, field, err)
			}
			out["adapters."+strings.ToLower(bidder)+"."+field] = v
		}
	}
	return nil
}

func decodeJSONValue(raw json.RawMessage) (any, error) {
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	var v any
	if err := dec.Decode(&v); err != nil {
		return nil, err
	}
	return v, nil
}

func derefBool(b *bool) bool {
	return b != nil && *b
}

func boolPtr(b bool) *bool { return &b }

// sortedStrings returns a sorted copy of in.
func sortedStrings(in []string) []string {
	out := append([]string(nil), in...)
	sort.Strings(out)
	return out
}
