package trace

import (
	"encoding/json"
	"strconv"

	"github.com/prebid/prebid-server/v4/parity"
)

// binaryDecisions are the decision points whose envelope value must be 0 or 1
// (decisions.json "binary" domain).
var binaryDecisions = map[parity.DecisionID]bool{
	parity.DecisionFloorsSkip:                true,
	parity.DecisionFloorsDataRate:            true,
	parity.DecisionFloorsEnforce:             true,
	parity.DecisionCategoryDedupe:            true,
	parity.DecisionPriceGranularityTimesplit: true,
}

// Intn is the index-valued decision point: it returns the envelope override for
// point when it is an integer in [0, n), otherwise def(n). The outcome and its
// source are recorded. A nil session simply returns def(n).
func (s *Session) Intn(point parity.DecisionID, n int, def func(int) int) int {
	if s == nil {
		return def(n)
	}
	if v, ok := s.env.DecisionInt(point); ok {
		if v >= 0 && v < int64(n) {
			s.recordDecision(point, strconv.FormatInt(v, 10), parity.DecisionSourceEnvelope, nil)
			return int(v)
		}
		got := def(n)
		s.recordDecision(point, strconv.Itoa(got), parity.DecisionSourceRNG,
			[]string{"envelope override " + strconv.FormatInt(v, 10) + " outside [0," + strconv.Itoa(n) + ")"})
		return got
	}
	got := def(n)
	s.recordDecision(point, strconv.Itoa(got), parity.DecisionSourceRNG, nil)
	return got
}

// Decide is the semantic decision point: it returns the envelope's integer
// override for point (validated against the decision's value domain), otherwise
// the value def computes from the real random source. A nil session simply
// returns def().
func (s *Session) Decide(point parity.DecisionID, def func() int) int {
	if s == nil {
		return def()
	}
	if v, ok := s.env.DecisionInt(point); ok {
		if !binaryDecisions[point] || v == 0 || v == 1 {
			s.recordDecision(point, strconv.FormatInt(v, 10), parity.DecisionSourceEnvelope, nil)
			return int(v)
		}
		got := def()
		s.recordDecision(point, strconv.Itoa(got), parity.DecisionSourceRNG,
			[]string{"envelope override " + strconv.FormatInt(v, 10) + " is not 0 or 1"})
		return got
	}
	got := def()
	s.recordDecision(point, strconv.Itoa(got), parity.DecisionSourceRNG, nil)
	return got
}

// DecideString is the string-valued decision point (usersync.shuffle). It
// returns the envelope override when present, otherwise def().
func (s *Session) DecideString(point parity.DecisionID, def func() string) string {
	if s == nil {
		return def()
	}
	if v, ok := s.env.DecisionString(point); ok {
		s.recordDecision(point, quote(v), parity.DecisionSourceEnvelope, nil)
		return v
	}
	got := def()
	s.recordDecision(point, quote(got), parity.DecisionSourceRNG, nil)
	return got
}

func (s *Session) recordDecision(point parity.DecisionID, rawValue, source string, notes []string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.decisions = append(s.decisions, parity.Decision{
		Point:  point,
		Value:  json.RawMessage(rawValue),
		Source: source,
		Notes:  notes,
	})
}

func quote(v string) string {
	raw, err := json.Marshal(v)
	if err != nil {
		return `""`
	}
	return string(raw)
}
