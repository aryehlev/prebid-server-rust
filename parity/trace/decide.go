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

// enumDecisions lists the allowed envelope values of the enum decision points
// (decisions.json "enum" domain).
var enumDecisions = map[parity.DecisionID][]string{
	parity.DecisionUsersyncShuffle: {parity.UsersyncShuffleIdentity},
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

// DecideBool is Decide for the binary decision points whose Go site computes a
// boolean: draw is the original random computation and true maps to the
// decision value 1. A nil session simply returns draw().
func (s *Session) DecideBool(point parity.DecisionID, draw func() bool) bool {
	if s == nil {
		return draw()
	}
	return s.Decide(point, func() int { return boolValue(draw()) }) == 1
}

// DecideChance is DecideBool for a binary point whose Go site draws against a
// configured rate (floors.skip, floors.data_rate, floors.enforce). uncertain
// reports whether draw can return both outcomes for the configured rates. When
// it cannot (a rate of 0 or 100 fixes the outcome), the envelope value is
// ignored: Go never draws differently there, so honouring a pinned value would
// make parity-mode Go behave unlike production (observer effect). The fixed
// outcome is then recorded with source rng, plus a note when the envelope had
// pinned the point. A nil session simply returns draw().
func (s *Session) DecideChance(point parity.DecisionID, uncertain bool, draw func() bool) bool {
	if s == nil {
		return draw()
	}
	if uncertain {
		return s.DecideBool(point, draw)
	}
	got := boolValue(draw())
	var notes []string
	if _, pinned := s.env.Decisions[string(point)]; pinned {
		notes = []string{"envelope override ignored: the configured rate fixes the outcome"}
	}
	s.recordDecision(point, strconv.Itoa(got), parity.DecisionSourceRNG, notes)
	return got == 1
}

// DecideString is the string-valued decision point (usersync.shuffle). It
// returns the envelope override when it is one of the point's enum values,
// otherwise def() (an out-of-domain override is noted and ignored).
func (s *Session) DecideString(point parity.DecisionID, def func() string) string {
	if s == nil {
		return def()
	}
	if v, ok := s.env.DecisionString(point); ok {
		if inEnum(point, v) {
			s.recordDecision(point, quote(v), parity.DecisionSourceEnvelope, nil)
			return v
		}
		got := def()
		s.recordDecision(point, quote(got), parity.DecisionSourceRNG,
			[]string{"envelope override " + quote(v) + " is not an allowed value"})
		return got
	}
	got := def()
	s.recordDecision(point, quote(got), parity.DecisionSourceRNG, nil)
	return got
}

// inEnum reports whether v is an allowed envelope value of point. Points
// without a registered enum accept any value.
func inEnum(point parity.DecisionID, v string) bool {
	allowed, ok := enumDecisions[point]
	if !ok {
		return true
	}
	for _, a := range allowed {
		if a == v {
			return true
		}
	}
	return false
}

func boolValue(b bool) int {
	if b {
		return 1
	}
	return 0
}

// LastDecision returns the value most recently recorded for point, and false
// when the point was not reached in this case (or for a nil session).
func (s *Session) LastDecision(point parity.DecisionID) (json.RawMessage, bool) {
	if s == nil {
		return nil, false
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := len(s.decisions) - 1; i >= 0; i-- {
		if s.decisions[i].Point == point {
			return append(json.RawMessage(nil), s.decisions[i].Value...), true
		}
	}
	return nil, false
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
