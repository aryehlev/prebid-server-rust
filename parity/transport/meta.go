package transport

import (
	"encoding/base64"
	"encoding/json"
	"fmt"

	"github.com/prebid/prebid-server/v4/parity"
)

// MetaVersion is the current X-Parity-Meta payload version.
const MetaVersion = 1

// EncodeMeta renders the X-Parity-Meta header value: standard base64 (with
// padding) of the JSON-encoded meta.
func EncodeMeta(meta parity.OutboundMeta) (string, error) {
	raw, err := json.Marshal(meta)
	if err != nil {
		return "", fmt.Errorf("parity: encoding %s: %w", parity.MetaHeader, err)
	}
	return base64.StdEncoding.EncodeToString(raw), nil
}

// DecodeMeta parses an X-Parity-Meta header value. The oracle uses it to
// attribute a captured request and then strips the header from the capture.
func DecodeMeta(value string) (parity.OutboundMeta, error) {
	var meta parity.OutboundMeta
	raw, err := base64.StdEncoding.DecodeString(value)
	if err != nil {
		return meta, fmt.Errorf("parity: decoding %s base64: %w", parity.MetaHeader, err)
	}
	if err := json.Unmarshal(raw, &meta); err != nil {
		return meta, fmt.Errorf("parity: decoding %s JSON: %w", parity.MetaHeader, err)
	}
	if meta.V != MetaVersion {
		return meta, fmt.Errorf("parity: unsupported %s version %d", parity.MetaHeader, meta.V)
	}
	return meta, nil
}
