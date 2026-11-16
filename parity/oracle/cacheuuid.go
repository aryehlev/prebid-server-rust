package oracle

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"unicode/utf8"
)

// CanonicalJSON re-encodes a JSON document in the parity canonical form that
// both implementations' cache UUIDs depend on:
//
//   - object members sorted by key, compared byte-wise on their UTF-8 form;
//   - no insignificant whitespace;
//   - numbers copied verbatim (1.50 stays 1.50, so lexical differences in what
//     a server sends still change the UUID);
//   - strings re-escaped minimally: \" and \\, \b \f \n \r \t, other control
//     characters as \u00xx (lowercase hex); everything else, including
//     non-ASCII and U+2028/U+2029, as raw UTF-8.
//
// Invalid JSON or invalid UTF-8 inside strings is an error.
func CanonicalJSON(raw []byte) ([]byte, error) {
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	var v any
	if err := dec.Decode(&v); err != nil {
		return nil, fmt.Errorf("oracle: canonical json: %w", err)
	}
	if dec.More() {
		return nil, errors.New("oracle: canonical json: trailing data after the JSON value")
	}
	var buf bytes.Buffer
	if err := writeCanonical(&buf, v); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

func writeCanonical(buf *bytes.Buffer, v any) error {
	switch x := v.(type) {
	case nil:
		buf.WriteString("null")
	case bool:
		if x {
			buf.WriteString("true")
		} else {
			buf.WriteString("false")
		}
	case json.Number:
		buf.WriteString(x.String())
	case string:
		return writeCanonicalString(buf, x)
	case []any:
		buf.WriteByte('[')
		for i, item := range x {
			if i > 0 {
				buf.WriteByte(',')
			}
			if err := writeCanonical(buf, item); err != nil {
				return err
			}
		}
		buf.WriteByte(']')
	case map[string]any:
		return writeCanonicalObject(buf, x)
	default:
		return fmt.Errorf("oracle: canonical json: unexpected value of type %T", v)
	}
	return nil
}

func writeCanonicalObject(buf *bytes.Buffer, obj map[string]any) error {
	keys := make([]string, 0, len(obj))
	for k := range obj {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	buf.WriteByte('{')
	for i, k := range keys {
		if i > 0 {
			buf.WriteByte(',')
		}
		if err := writeCanonicalString(buf, k); err != nil {
			return err
		}
		buf.WriteByte(':')
		if err := writeCanonical(buf, obj[k]); err != nil {
			return err
		}
	}
	buf.WriteByte('}')
	return nil
}

const lowerHex = "0123456789abcdef"

func writeCanonicalString(buf *bytes.Buffer, s string) error {
	if !utf8.ValidString(s) {
		return errors.New("oracle: canonical json: string is not valid UTF-8")
	}
	buf.WriteByte('"')
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch c {
		case '"':
			buf.WriteString(`\"`)
		case '\\':
			buf.WriteString(`\\`)
		case '\b':
			buf.WriteString(`\b`)
		case '\f':
			buf.WriteString(`\f`)
		case '\n':
			buf.WriteString(`\n`)
		case '\r':
			buf.WriteString(`\r`)
		case '\t':
			buf.WriteString(`\t`)
		default:
			if c < 0x20 {
				buf.WriteString(`\u00`)
				buf.WriteByte(lowerHex[c>>4])
				buf.WriteByte(lowerHex[c&0xf])
			} else {
				buf.WriteByte(c)
			}
		}
	}
	buf.WriteByte('"')
	return nil
}

// ContentUUID returns the content-addressed cache UUID of one canonical put:
// the first 16 bytes of sha256(canonical), with the RFC 9562 version 8 and
// variant bits set, formatted 8-4-4-4-12 in lowercase hex.
func ContentUUID(canonical []byte) string {
	sum := sha256.Sum256(canonical)
	b := sum[:16]
	b[6] = (b[6] & 0x0f) | 0x80
	b[8] = (b[8] & 0x3f) | 0x80
	h := hex.EncodeToString(b)
	return h[0:8] + "-" + h[8:12] + "-" + h[12:16] + "-" + h[16:20] + "-" + h[20:32]
}

// cachePutRequest is the Prebid Cache POST body.
type cachePutRequest struct {
	Puts []json.RawMessage `json:"puts"`
}

// cacheResponse is the Prebid Cache POST response.
type cacheResponse struct {
	Responses []cacheUUID `json:"responses"`
}

type cacheUUID struct {
	UUID string `json:"uuid"`
}

// CacheUUIDs returns the UUID of every put of a Prebid Cache request, in
// input order. A put that carries a non-empty string "key" gets that key, as
// Prebid Cache does for custom keys; every other put gets ContentUUID of its
// canonical JSON.
func CacheUUIDs(body []byte) ([]string, error) {
	var req cachePutRequest
	if err := json.Unmarshal(body, &req); err != nil {
		return nil, fmt.Errorf("oracle: cache request: %w", err)
	}
	out := make([]string, 0, len(req.Puts))
	for i, put := range req.Puts {
		var head struct {
			Key string `json:"key"`
		}
		if err := json.Unmarshal(put, &head); err != nil {
			return nil, fmt.Errorf("oracle: cache request puts[%d]: %w", i, err)
		}
		if head.Key != "" {
			out = append(out, head.Key)
			continue
		}
		canonical, err := CanonicalJSON(put)
		if err != nil {
			return nil, fmt.Errorf("oracle: cache request puts[%d]: %w", i, err)
		}
		out = append(out, ContentUUID(canonical))
	}
	return out, nil
}

// cacheResponseBody encodes the Prebid Cache response for uuids.
func cacheResponseBody(uuids []string) ([]byte, error) {
	resp := cacheResponse{Responses: make([]cacheUUID, 0, len(uuids))}
	for _, id := range uuids {
		resp.Responses = append(resp.Responses, cacheUUID{UUID: id})
	}
	raw, err := json.Marshal(resp)
	if err != nil {
		return nil, fmt.Errorf("oracle: encoding cache response: %w", err)
	}
	return raw, nil
}
