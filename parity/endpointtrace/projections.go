package endpointtrace

import (
	"sort"
	"strconv"

	"github.com/prebid/go-gdpr/consentconstants"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/gdpr"
	"github.com/prebid/prebid-server/v4/hooks/hookexecution"
)

// tcf2Purposes are the TCF2 purposes Prebid Server enforces (config.TCF2
// Purpose1..Purpose10).
const tcf2Purposes = 10

// gdprPayload is the gdpr.context payload: the processGDPR result and the
// effective (host merged with account) TCF2 configuration the request carries
// into the exchange. tcf2 is null when processGDPR returned no reader.
type gdprPayload struct {
	Signal   int          `json:"signal"`
	Enforced bool         `json:"enforced"`
	Channel  string       `json:"channel"`
	TCF2     *tcf2Payload `json:"tcf2"`
}

// tcf2Payload summarises a gdpr.TCF2ConfigReader. channel_enabled is evaluated
// for the request's channel. Per-bidder feature-one vendor exceptions are not
// enumerable through the reader and are left out.
type tcf2Payload struct {
	Enabled                 bool                      `json:"enabled"`
	ChannelEnabled          bool                      `json:"channel_enabled"`
	Purposes                map[string]purposePayload `json:"purposes"`
	FeatureOneEnforced      bool                      `json:"feature_one_enforced"`
	PurposeOneTreatment     purposeOneTreatment       `json:"purpose_one_treatment"`
	BasicEnforcementVendors []string                  `json:"basic_enforcement_vendors"`
}

// purposePayload is the effective enforcement of one purpose. algo is "basic",
// "full" or "" (undefined).
type purposePayload struct {
	Enforced         bool     `json:"enforced"`
	Algo             string   `json:"algo"`
	EnforceVendors   bool     `json:"enforce_vendors"`
	VendorExceptions []string `json:"vendor_exceptions"`
}

type purposeOneTreatment struct {
	Enabled       bool `json:"enabled"`
	AccessAllowed bool `json:"access_allowed"`
}

func newGDPRPayload(tcf2 gdpr.TCF2ConfigReader, signal gdpr.Signal, enforced bool, channel config.ChannelType) gdprPayload {
	p := gdprPayload{Signal: int(signal), Enforced: enforced, Channel: string(channel)}
	if tcf2 == nil {
		return p
	}
	p.TCF2 = &tcf2Payload{
		Enabled:            tcf2.IsEnabled(),
		ChannelEnabled:     tcf2.ChannelEnabled(channel),
		Purposes:           make(map[string]purposePayload, tcf2Purposes),
		FeatureOneEnforced: tcf2.FeatureOneEnforced(),
		PurposeOneTreatment: purposeOneTreatment{
			Enabled:       tcf2.PurposeOneTreatmentEnabled(),
			AccessAllowed: tcf2.PurposeOneTreatmentAccessAllowed(),
		},
		BasicEnforcementVendors: sortedKeys(tcf2.BasicEnforcementVendors()),
	}
	for i := 1; i <= tcf2Purposes; i++ {
		purpose := consentconstants.Purpose(i)
		p.TCF2.Purposes[strconv.Itoa(i)] = purposePayload{
			Enforced:         tcf2.PurposeEnforced(purpose),
			Algo:             enforcementAlgo(tcf2.PurposeEnforcementAlgo(purpose)),
			EnforceVendors:   tcf2.PurposeEnforcingVendors(purpose),
			VendorExceptions: sortedKeys(tcf2.PurposeVendorExceptions(purpose)),
		}
	}
	return p
}

func enforcementAlgo(algo config.TCF2EnforcementAlgo) string {
	switch algo {
	case config.TCF2BasicEnforcement:
		return config.TCF2EnforceAlgoBasic
	case config.TCF2FullEnforcement:
		return config.TCF2EnforceAlgoFull
	default:
		return ""
	}
}

// sortedKeys returns the keys of a set in order, [] for an empty or nil set.
func sortedKeys(set map[string]struct{}) []string {
	keys := make([]string, 0, len(set))
	for k := range set {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

// stageOutcome projects hookexecution.StageOutcome with the stage name, which
// Go's own JSON omits.
type stageOutcome struct {
	Stage string `json:"stage"`
	hookexecution.ExecutionTime
	Entity string         `json:"entity"`
	Groups []groupOutcome `json:"groups"`
}

type groupOutcome struct {
	hookexecution.ExecutionTime
	InvocationResults []hookOutcome `json:"invocation_results"`
}

// hookOutcome is hookexecution.HookOutcome plus its errors and warnings, which
// Go's own JSON omits. The outer fields shadow the embedded json:"-" ones.
type hookOutcome struct {
	hookexecution.HookOutcome
	Errors   []string `json:"errors"`
	Warnings []string `json:"warnings"`
}

func newHookOutcomes(outcomes []hookexecution.StageOutcome) []stageOutcome {
	out := make([]stageOutcome, 0, len(outcomes))
	for _, so := range outcomes {
		stage := stageOutcome{Stage: so.Stage, ExecutionTime: so.ExecutionTime, Entity: string(so.Entity), Groups: make([]groupOutcome, 0, len(so.Groups))}
		for _, g := range so.Groups {
			group := groupOutcome{ExecutionTime: g.ExecutionTime, InvocationResults: make([]hookOutcome, 0, len(g.InvocationResults))}
			for _, h := range g.InvocationResults {
				group.InvocationResults = append(group.InvocationResults, hookOutcome{HookOutcome: h, Errors: nonNil(h.Errors), Warnings: nonNil(h.Warnings)})
			}
			stage.Groups = append(stage.Groups, group)
		}
		out = append(out, stage)
	}
	return out
}

func nonNil(s []string) []string {
	if s == nil {
		return []string{}
	}
	return s
}
