// Package pools embeds the value pools of the parity request generator
// (parity/gen): realistic OpenRTB values, privacy signal strings and the
// boundary, unicode and malformed-input catalogs. The data lives in the JSON
// files next to this file so it can be reviewed and extended without touching
// generator code; parity/gen decodes it and its tests check every entry.
//
// The package is harness-only: product code must not import it.
package pools

import "embed"

// FS holds ortb.json, privacy.json and values.json.
//
//go:embed *.json
var FS embed.FS

// File names inside FS.
const (
	ORTBFile    = "ortb.json"
	PrivacyFile = "privacy.json"
	ValuesFile  = "values.json"
)
