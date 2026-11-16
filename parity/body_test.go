package parity

import (
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func gzipBytes(t *testing.T, content string) []byte {
	t.Helper()
	var buf bytes.Buffer
	zw := gzip.NewWriter(&buf)
	_, err := zw.Write([]byte(content))
	require.NoError(t, err)
	require.NoError(t, zw.Close())
	return buf.Bytes()
}

func strPtr(s string) *string { return &s }

func TestNewBody(t *testing.T) {
	gz := gzipBytes(t, `{"a":1}`)
	tests := []struct {
		name         string
		raw          []byte
		encoding     string
		wantNil      bool
		wantText     *string
		wantB64      *string
		wantJSON     string
		wantEncoding string
	}{
		{name: "empty body is nil", raw: nil, wantNil: true},
		{name: "json text", raw: []byte(`{"a":1}`), wantText: strPtr(`{"a":1}`), wantJSON: `{"a":1}`},
		{name: "plain text keeps bytes", raw: []byte("hello"), wantText: strPtr("hello")},
		{name: "trailing newline kept in text", raw: []byte("{}\n"), wantText: strPtr("{}\n"), wantJSON: "{}\n"},
		{name: "invalid utf8 becomes b64", raw: []byte{0xff, 0x00}, wantB64: strPtr(base64.StdEncoding.EncodeToString([]byte{0xff, 0x00}))},
		{name: "gzip is b64 plus decoded json", raw: gz, encoding: "GZIP", wantB64: strPtr(base64.StdEncoding.EncodeToString(gz)), wantJSON: `{"a":1}`, wantEncoding: "gzip"},
		{name: "identity encoding is dropped", raw: []byte(`[]`), encoding: "identity", wantText: strPtr(`[]`), wantJSON: `[]`},
		{name: "unsupported encoding has no json", raw: []byte("xx"), encoding: "br", wantB64: strPtr(base64.StdEncoding.EncodeToString([]byte("xx"))), wantEncoding: "br"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			b := NewBody(tt.raw, tt.encoding)
			if tt.wantNil {
				assert.Nil(t, b)
				return
			}
			require.NotNil(t, b)
			assert.Equal(t, tt.wantText, b.Text)
			assert.Equal(t, tt.wantB64, b.B64)
			assert.Equal(t, tt.wantJSON, string(b.JSON))
			assert.Equal(t, tt.wantEncoding, b.ContentEncoding)

			wire, err := b.WireBytes()
			require.NoError(t, err)
			assert.Equal(t, tt.raw, wire, "recorded bodies must reproduce the exact wire bytes")
		})
	}
}

func TestBodyWireAndDecodedBytes(t *testing.T) {
	tests := []struct {
		name        string
		body        *Body
		wantDecoded string
		wantErr     bool
	}{
		{name: "nil body", body: nil, wantDecoded: ""},
		{name: "text", body: TextBody("abc"), wantDecoded: "abc"},
		{name: "json only", body: &Body{JSON: []byte(`{"x":true}`)}, wantDecoded: `{"x":true}`},
		{name: "text with gzip input is encoded by receiver", body: &Body{Text: strPtr("zip me"), ContentEncoding: "gzip"}, wantDecoded: "zip me"},
		{name: "bad b64", body: &Body{B64: strPtr("!!")}, wantErr: true},
		{name: "unsupported input encoding", body: &Body{Text: strPtr("x"), ContentEncoding: "br"}, wantErr: true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			wire, err := tt.body.WireBytes()
			if tt.wantErr {
				assert.Error(t, err)
				return
			}
			require.NoError(t, err)
			if tt.body != nil && tt.body.ContentEncoding == "gzip" {
				roundTrip := NewBody(wire, "gzip")
				decoded, err := roundTrip.DecodedBytes()
				require.NoError(t, err)
				assert.Equal(t, tt.wantDecoded, string(decoded))
				return
			}
			decoded, err := tt.body.DecodedBytes()
			require.NoError(t, err)
			assert.Equal(t, tt.wantDecoded, string(decoded))
		})
	}
}
