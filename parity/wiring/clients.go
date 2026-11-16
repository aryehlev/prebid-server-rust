package wiring

import (
	"net/http"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/transport"
)

// VendorListURLPrefix is the URL prefix of the GDPR vendor list (gdpr.VendorListURLMaker).
const VendorListURLPrefix = "https://vendor-list.consensu.org/"

// WrapClient routes every call of client through the parity transport, labelled
// kind. It is a no-op unless parity is enabled. The shared general client
// (kind "general") additionally gets a URL classifier that labels vendor-list
// and stored-data calls (see GeneralClassifierRules).
func WrapClient(client *http.Client, kind string, cfg *config.Configuration) {
	if client == nil || cfg == nil || !cfg.Parity.Enabled {
		return
	}
	opts := transport.Options{Kind: kind}
	if kind == parity.OutboundGeneral {
		opts.Classify = transport.PrefixClassifier(GeneralClassifierRules(cfg))
	}
	client.Transport = transport.New(client.Transport, opts, cfg.Parity)
}

// GeneralClassifierRules lists the URL prefixes that refine kind "general" for
// calls without a bidder in their context: the vendor list first, then every
// configured stored-data HTTP endpoint. Calls matching nothing (modules, for
// example) stay "general". The Rust server must apply the same rules.
func GeneralClassifierRules(cfg *config.Configuration) []transport.PrefixRule {
	rules := []transport.PrefixRule{{Prefix: VendorListURLPrefix, Kind: parity.OutboundVendorList}}
	for _, sr := range []config.StoredRequests{
		cfg.StoredRequests, cfg.StoredRequestsAMP, cfg.CategoryMapping,
		cfg.StoredVideo, cfg.Accounts, cfg.StoredResponses,
	} {
		for _, endpoint := range []string{sr.HTTP.Endpoint, sr.HTTP.AmpEndpoint, sr.HTTPEvents.Endpoint, sr.HTTPEvents.AmpEndpoint} {
			if endpoint != "" {
				rules = append(rules, transport.PrefixRule{Prefix: endpoint, Kind: parity.OutboundStored})
			}
		}
	}
	return rules
}
