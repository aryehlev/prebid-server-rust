package main

// Generated bodies (bodies_fuzz.json): random wire bytes recorded under random
// Content-Encoding values, and the wire and decoded bytes of random input
// bodies, in the shape of bodies.json. Generated X-Parity-Meta header values
// (meta_fuzz.json): damaged base64 of rewritten OutboundMeta JSON, decoded and
// encoded again.

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"reflect"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/transport"
)

// fuzzEncodings are Content-Encoding values: the identity and gzip spellings
// Go's normalizeEncoding accepts (case, Unicode spaces, the Turkish dotted
// capital I that lower-cases to i) and unsupported ones.
var fuzzEncodings = []string{
	"", "identity", "IDENTITY", " Identity ", "\tidentity\n", "identity" + cp(0xa0), cp(0x85) + "identity",
	cp(0x2028) + "identity", cp(0x3000) + "gzip", cp(0x130) + "DENTITY", "GZ" + cp(0x130) + "P", "gzip", "x-gzip",
	"X-GZIP", "gzip, br", "br", "deflate", "zstd", "identity, gzip", cp(0x200b) + "identity",
}

// b64Text is base64 of raw, sometimes damaged the ways base64 input can be.
func (f *fuzzer) b64Text(raw []byte) string {
	s := base64.StdEncoding.EncodeToString(raw)
	if !f.chance(40) {
		return s
	}
	at := 0
	if len(s) > 0 {
		at = f.intn(len(s))
	}
	switch f.intn(6) {
	case 0:
		return s[:at] + f.pick("\n", "\r", "\r\n") + s[at:]
	case 1:
		return strings.TrimRight(s, "=")
	case 2:
		return s[:at] + f.pick(" ", "!", "-", "_", "=", "\t", cp(0xe9)) + s[at:]
	case 3:
		return s + f.pick("=", "==", "A", "\n")
	case 4:
		return base64.URLEncoding.EncodeToString(raw)
	default:
		return base64.RawStdEncoding.EncodeToString(raw)
	}
}

// fuzzBodiesGolden records random wire bytes under random encodings, and the
// wire and decoded bytes of random input bodies.
func fuzzBodiesGolden() (any, error) {
	f := newFuzzer(fuzzSeed + 4)
	recorded := make([]map[string]any, 0, 300)
	for i := 0; i < 300; i++ {
		raw := f.rawBytes()
		if f.chance(30) {
			raw = []byte(f.jsonText(f.chance(50)))
		}
		enc := fuzzEncodings[f.intn(len(fuzzEncodings))]
		body, err := marshalString(parity.NewBody(raw, enc))
		if err != nil {
			return nil, err
		}
		recorded = append(recorded, map[string]any{
			"name": fmt.Sprintf("recorded-%d", i), "raw_b64": base64.StdEncoding.EncodeToString(raw),
			"content_encoding": enc, "body": body, "gzip": strings.Contains(strings.ToLower(enc), "gz"),
			// The exact bytes: a json member can pass invalid UTF-8 through.
			"body_b64": base64.StdEncoding.EncodeToString([]byte(body)),
		})
	}
	inputs := make([]map[string]any, 0, 300)
	for i := 0; i < 300; i++ {
		var members []string
		if f.chance(50) {
			var b strings.Builder
			f.writeString(&b, f.text())
			members = append(members, `"text":`+b.String())
		}
		if f.chance(50) {
			var b strings.Builder
			f.writeString(&b, f.b64Text(f.rawBytes()))
			members = append(members, `"b64":`+b.String())
		}
		if f.chance(50) {
			members = append(members, `"json":`+f.jsonText(f.chance(50)))
		}
		if f.chance(50) {
			var b strings.Builder
			f.writeString(&b, fuzzEncodings[f.intn(len(fuzzEncodings))])
			members = append(members, `"content_encoding":`+b.String())
		}
		f.r.Shuffle(len(members), func(i, j int) { members[i], members[j] = members[j], members[i] })
		doc := "{" + strings.Join(members, ",") + "}"
		var b parity.Body
		if err := json.Unmarshal([]byte(doc), &b); err != nil {
			return nil, fmt.Errorf("input %d: %w", i, err)
		}
		// The decoded encoding: the document may spell it with escapes.
		c := map[string]any{"name": fmt.Sprintf("input-%d", i), "body": doc, "gzip": strings.Contains(strings.ToLower(b.ContentEncoding), "gz")}
		if wire, err := b.WireBytes(); err != nil {
			c["wire_error"] = err.Error()
		} else {
			c["wire_b64"] = base64.StdEncoding.EncodeToString(wire)
		}
		if decoded, err := b.DecodedBytes(); err != nil {
			c["decoded_error"] = err.Error()
		} else {
			c["decoded_b64"] = base64.StdEncoding.EncodeToString(decoded)
		}
		inputs = append(inputs, c)
	}
	return map[string]any{"recorded": recorded, "inputs": inputs}, nil
}

// fuzzMetaGolden decodes generated X-Parity-Meta values with
// transport.DecodeMeta and encodes each decoded meta again with EncodeMeta.
// Header values are valid UTF-8 (they are HTTP header text).
func fuzzMetaGolden() (any, error) {
	f := newFuzzer(fuzzSeed + 5)
	cases := make([]map[string]any, 0, 300)
	for i := 0; i < 300; i++ {
		var header string
		if f.chance(10) {
			header = f.text()
		} else {
			var m parity.OutboundMeta
			if err := f.fill(reflect.ValueOf(&m).Elem(), 0); err != nil {
				return nil, err
			}
			if f.chance(70) {
				m.V = transport.MetaVersion
			}
			raw, err := json.Marshal(m)
			if err != nil {
				return nil, fmt.Errorf("meta %d: %w", i, err)
			}
			if f.chance(50) {
				if raw, err = f.rewrite(raw); err != nil {
					return nil, fmt.Errorf("meta %d: %w", i, err)
				}
			}
			header = f.b64Text(raw)
		}
		c := map[string]any{"header_b64": base64.StdEncoding.EncodeToString([]byte(header))}
		meta, err := transport.DecodeMeta(header)
		if err != nil {
			c["error"] = err.Error()
			cases = append(cases, c)
			continue
		}
		doc, err := marshalString(meta)
		if err != nil {
			return nil, fmt.Errorf("meta %d: %w", i, err)
		}
		again, err := transport.EncodeMeta(meta)
		if err != nil {
			return nil, fmt.Errorf("meta %d: %w", i, err)
		}
		c["meta"], c["header_again"] = doc, again
		cases = append(cases, c)
	}
	return cases, nil
}
