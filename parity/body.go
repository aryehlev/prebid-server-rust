package parity

import (
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"strings"
	"unicode/utf8"
)

// Body describes an HTTP body in a parity document.
//
// Rules shared by every implementation:
//   - b64, when present, is always the exact wire bytes.
//   - text, when present and content_encoding is empty, is the exact wire bytes.
//   - json is the semantic JSON of the decoded content when it parses as JSON.
//   - When an implementation records a body (traces, http_out) it emits text for
//     valid UTF-8 identity bodies and b64 otherwise, plus json when the decoded
//     content is JSON. A content-encoded body is always recorded as b64.
//   - On input documents only (envelope, inline mocks, adapter inputs) text or
//     json may be combined with content_encoding; the receiver then encodes that
//     content itself. Use b64 when byte-exact compressed input matters.
//
// A nil *Body means "no body".
type Body struct {
	Text            *string         `json:"text,omitempty"`
	B64             *string         `json:"b64,omitempty"`
	JSON            json.RawMessage `json:"json,omitempty"`
	ContentEncoding string          `json:"content_encoding,omitempty"`
}

// TextBody returns a Body holding s as identity text.
func TextBody(s string) *Body {
	return &Body{Text: &s}
}

// NewBody records raw wire bytes. contentEncoding is the Content-Encoding header
// value; gzip content is decoded to populate JSON. It returns nil for an empty body.
func NewBody(raw []byte, contentEncoding string) *Body {
	if len(raw) == 0 {
		return nil
	}
	enc := normalizeEncoding(contentEncoding)
	b := &Body{ContentEncoding: enc}
	if enc == "" && utf8.Valid(raw) {
		s := string(raw)
		b.Text = &s
	} else {
		s := base64.StdEncoding.EncodeToString(raw)
		b.B64 = &s
	}
	if decoded, err := decodeContent(raw, enc); err == nil && json.Valid(decoded) {
		b.JSON = json.RawMessage(bytes.Clone(decoded))
	}
	return b
}

// WireBytes returns the bytes to put on the wire for an input Body.
func (b *Body) WireBytes() ([]byte, error) {
	if b == nil {
		return nil, nil
	}
	if b.B64 != nil {
		raw, err := base64.StdEncoding.DecodeString(*b.B64)
		if err != nil {
			return nil, fmt.Errorf("body.b64: %w", err)
		}
		return raw, nil
	}
	var content []byte
	switch {
	case b.Text != nil:
		content = []byte(*b.Text)
	case len(b.JSON) > 0:
		content = bytes.Clone(b.JSON)
	default:
		return nil, nil
	}
	return encodeContent(content, normalizeEncoding(b.ContentEncoding))
}

// DecodedBytes returns the decoded content of a recorded or input Body.
func (b *Body) DecodedBytes() ([]byte, error) {
	if b == nil {
		return nil, nil
	}
	if b.B64 == nil {
		if b.Text != nil {
			return []byte(*b.Text), nil
		}
		return bytes.Clone(b.JSON), nil
	}
	raw, err := b.WireBytes()
	if err != nil {
		return nil, err
	}
	return decodeContent(raw, normalizeEncoding(b.ContentEncoding))
}

func normalizeEncoding(enc string) string {
	enc = strings.ToLower(strings.TrimSpace(enc))
	if enc == "identity" {
		return ""
	}
	return enc
}

func decodeContent(raw []byte, enc string) ([]byte, error) {
	switch enc {
	case "":
		return raw, nil
	case "gzip", "x-gzip":
		zr, err := gzip.NewReader(bytes.NewReader(raw))
		if err != nil {
			return nil, fmt.Errorf("gzip decode: %w", err)
		}
		defer zr.Close()
		out, err := io.ReadAll(zr)
		if err != nil {
			return nil, fmt.Errorf("gzip decode: %w", err)
		}
		return out, nil
	default:
		return nil, fmt.Errorf("unsupported content encoding %q", enc)
	}
}

func encodeContent(content []byte, enc string) ([]byte, error) {
	switch enc {
	case "":
		return content, nil
	case "gzip", "x-gzip":
		var buf bytes.Buffer
		zw := gzip.NewWriter(&buf)
		if _, err := zw.Write(content); err != nil {
			return nil, fmt.Errorf("gzip encode: %w", err)
		}
		if err := zw.Close(); err != nil {
			return nil, fmt.Errorf("gzip encode: %w", err)
		}
		return buf.Bytes(), nil
	default:
		return nil, fmt.Errorf("unsupported content encoding %q", enc)
	}
}
