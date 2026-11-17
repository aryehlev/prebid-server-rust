package gen

import (
	"errors"
	"fmt"
	"strconv"
	"strings"
)

// Case ID grammar.
//
//	generated := "gen:" seed ":" index ":" base { "+" step }
//	derived   := source "#mut:" seed ":" index ":" step { "+" step }
//	step      := name [ "@" variant ]
//	name      := [a-z][a-z0-9_]* { "." [a-z0-9_]+ }
//
// seed, index and variant are unsigned decimal integers. A generated case
// starts from a base request built from (seed, index); a derived case starts
// from the corpus case whose ID is source. Steps then apply in order, each with
// its own random stream keyed by the case, the step name, the variant and how
// often the name occurred before it, so a step's choices do not depend on the
// other steps of the chain. The ID therefore regenerates the identical case on
// the same checkout.
const (
	// GeneratedPrefix starts the ID of a case built from scratch.
	GeneratedPrefix = "gen:"
	// MutationMarker separates a corpus case ID from its mutation chain.
	MutationMarker = "#mut:"
	// stepSeparator joins chain steps.
	stepSeparator = "+"
	// variantSeparator separates a step name from its variant.
	variantSeparator = "@"
)

// ErrInvalidID is wrapped by every case ID parse error.
var ErrInvalidID = errors.New("gen: invalid case id")

// Step is one element of a mutation chain.
type Step struct {
	Name    string
	Variant uint32
}

// String formats the step as it appears in a case ID.
func (s Step) String() string {
	if s.Variant == 0 {
		return s.Name
	}
	return s.Name + variantSeparator + strconv.FormatUint(uint64(s.Variant), 10)
}

// ID is a parsed case ID.
type ID struct {
	// Source is the corpus case ID a derived case starts from; empty for a
	// generated case.
	Source string
	Seed   uint64
	Index  uint64
	// Chain holds the steps. For a generated case Chain[0] is the base.
	Chain []Step
}

// Generated reports whether the case is built from scratch.
func (id ID) Generated() bool { return id.Source == "" }

// String formats the ID.
func (id ID) String() string {
	steps := make([]string, len(id.Chain))
	for i, s := range id.Chain {
		steps[i] = s.String()
	}
	tail := fmt.Sprintf("%d:%d:%s", id.Seed, id.Index, strings.Join(steps, stepSeparator))
	if id.Generated() {
		return GeneratedPrefix + tail
	}
	return id.Source + MutationMarker + tail
}

// WithSteps returns a copy of id with steps appended.
func (id ID) WithSteps(steps ...Step) ID {
	out := id
	out.Chain = append(append([]Step(nil), id.Chain...), steps...)
	return out
}

// ParseID parses a generated or derived case ID.
func ParseID(s string) (ID, error) {
	var id ID
	var tail string
	switch {
	case strings.HasPrefix(s, GeneratedPrefix):
		tail = strings.TrimPrefix(s, GeneratedPrefix)
	default:
		i := strings.LastIndex(s, MutationMarker)
		if i <= 0 {
			return ID{}, fmt.Errorf("%w: %q is neither %q... nor <case>%s...", ErrInvalidID, s, GeneratedPrefix, MutationMarker)
		}
		id.Source = s[:i]
		tail = s[i+len(MutationMarker):]
	}
	parts := strings.SplitN(tail, ":", 3)
	if len(parts) != 3 {
		return ID{}, fmt.Errorf("%w: %q needs seed:index:chain", ErrInvalidID, s)
	}
	var err error
	if id.Seed, err = strconv.ParseUint(parts[0], 10, 64); err != nil {
		return ID{}, fmt.Errorf("%w: %q: seed: %v", ErrInvalidID, s, err)
	}
	if id.Index, err = strconv.ParseUint(parts[1], 10, 64); err != nil {
		return ID{}, fmt.Errorf("%w: %q: index: %v", ErrInvalidID, s, err)
	}
	if id.Chain, err = parseChain(parts[2]); err != nil {
		return ID{}, fmt.Errorf("%w: %q: %v", ErrInvalidID, s, err)
	}
	return id, nil
}

func parseChain(s string) ([]Step, error) {
	if s == "" {
		return nil, errors.New("empty chain")
	}
	var out []Step
	for _, tok := range strings.Split(s, stepSeparator) {
		step, err := parseStep(tok)
		if err != nil {
			return nil, err
		}
		out = append(out, step)
	}
	return out, nil
}

func parseStep(tok string) (Step, error) {
	name, variant, hasVariant := strings.Cut(tok, variantSeparator)
	if !validStepName(name) {
		return Step{}, fmt.Errorf("bad step name %q", name)
	}
	step := Step{Name: name}
	if hasVariant {
		v, err := strconv.ParseUint(variant, 10, 32)
		if err != nil || v == 0 {
			return Step{}, fmt.Errorf("bad step variant %q", tok)
		}
		step.Variant = uint32(v)
	}
	return step, nil
}

// validStepName checks name := [a-z][a-z0-9_]* { "." [a-z0-9_]+ }.
func validStepName(name string) bool {
	for i, seg := range strings.Split(name, ".") {
		if seg == "" {
			return false
		}
		for j, c := range seg {
			lower := c >= 'a' && c <= 'z'
			if i == 0 && j == 0 && !lower {
				return false
			}
			if !lower && !(c >= '0' && c <= '9') && c != '_' {
				return false
			}
		}
	}
	return true
}
