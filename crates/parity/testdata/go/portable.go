package main

// The portable scripts (sessions_portable.json): each script of sessions.json
// and sessions_fuzz.json that holds an input the Rust port reads otherwise
// than Go, run again without that input. The Rust replay counts the scripts
// that hold one by reason and compares the others in full; it compares these
// in full too, so an input that diverges hides nothing else its script
// records.

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"math"
	"strconv"
	"strings"
	"unicode/utf8"
)

// portable returns script without the inputs the Rust port reads otherwise
// than Go, each ledgered and counted by reason by the Rust replay
// (crates/parity/src/golden_test.rs, Divergence): the record and analytics
// ops of a NaN or an infinity (json.non-finite) or of a raw value that is not
// JSON, as the value or as a regs ext (a RawValue holds only JSON), and the
// record_raw ops of bytes that are JSON but not UTF-8 (json.invalid-utf8),
// are left out, and the envelope's -0 decisions (json.negative-zero-int) and
// null stage IDs (json.null-elements) are written as the 0 and "" Go reads
// them as. changed is false when script holds none of them.
func portable(script sessionScript) (sessionScript, bool, error) {
	out := sessionScript{name: script.name, ops: make([]op, 0, len(script.ops))}
	changed := false
	for _, o := range script.ops {
		diverges, err := readsOtherwise(o)
		if err != nil {
			return sessionScript{}, false, fmt.Errorf("%s: %s: %w", script.name, o.Op, err)
		}
		if diverges {
			changed = true
			continue
		}
		out.ops = append(out.ops, o)
	}
	// The generated envelopes write these members as written here (fuzz.go
	// envelope); a decision value ends at the next member or at the object's
	// end.
	out.envelope = strings.ReplaceAll(script.envelope, `"stages":[null]`, `"stages":[""]`)
	for _, point := range fuzzPoints {
		for _, end := range []string{",", "}"} {
			out.envelope = strings.ReplaceAll(out.envelope, strconv.Quote(point)+":-0"+end, strconv.Quote(point)+":0"+end)
		}
	}
	return out, changed || out.envelope != script.envelope, nil
}

// readsOtherwise reports whether the port reads the value op records or
// reports otherwise than Go.
func readsOtherwise(o op) (bool, error) {
	switch o.Op {
	case "record", "analytics":
		switch o.Type {
		case "nan":
			return true, nil
		case "float_bits":
			bits, err := strconv.ParseUint(o.Text, 10, 64)
			if err != nil {
				return false, err
			}
			v := math.Float64frombits(bits)
			return math.IsNaN(v) || math.IsInf(v, 0), nil
		case "raw":
			return !json.Valid([]byte(o.Text)), nil
		case "regs_record":
			var regs struct {
				Ext string `json:"ext"`
			}
			if err := json.Unmarshal(o.Value, &regs); err != nil {
				return false, err
			}
			return regs.Ext != "" && !json.Valid([]byte(regs.Ext)), nil
		}
	case "record_raw":
		raw, err := base64.StdEncoding.DecodeString(o.RawB64)
		if err != nil {
			return false, err
		}
		return json.Valid(raw) && !utf8.Valid(raw), nil
	}
	return false, nil
}

// portableSessionsGolden runs the portable form of each script of
// sessions.json and sessions_fuzz.json that holds an input the port reads
// otherwise, under the script's name.
func portableSessionsGolden() (any, error) {
	scripts := append(sessionScripts(), fuzzSessions()...)
	cases := make([]map[string]any, 0)
	for _, script := range scripts {
		twin, changed, err := portable(script)
		if err != nil {
			return nil, err
		}
		if !changed {
			continue
		}
		results, doc, err := runScript(twin)
		if err != nil {
			return nil, err
		}
		cases = append(cases, map[string]any{
			"name":      twin.name,
			"envelope":  twin.envelope,
			"ops":       twin.ops,
			"results":   results,
			"trace_b64": base64.StdEncoding.EncodeToString([]byte(doc)),
		})
	}
	return cases, nil
}
