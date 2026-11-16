package schema

import (
	"bytes"
	"encoding/json"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestSchemaFilesCompile(t *testing.T) {
	for _, file := range schemaFiles {
		t.Run(file, func(t *testing.T) {
			_, err := compile(BaseURI + file)
			require.NoError(t, err)
		})
	}
}

func TestSchemaFilesCarryVersionAndID(t *testing.T) {
	for _, file := range schemaFiles {
		t.Run(file, func(t *testing.T) {
			var doc map[string]any
			require.NoError(t, json.Unmarshal(mustFile(t, file), &doc))
			assert.Equal(t, "http://json-schema.org/draft-07/schema#", doc["$schema"])
			assert.Equal(t, BaseURI+file, doc["$id"])
			assert.NotEmpty(t, doc["description"])
		})
	}
}

// exampleCase binds an example to the way it is validated and the Go type that
// mirrors it.
type exampleCase struct {
	file string
	// definition, when set, validates against FileCommon/FileIntrospection
	// definitions instead of dispatching on the "schema" member.
	definitionFile string
	definition     string
	newGoValue     func() any
}

func exampleCases() []exampleCase {
	return []exampleCase{
		{file: "envelope.oracle.json", newGoValue: func() any { return &parity.Envelope{} }},
		{file: "envelope.inline.json", newGoValue: func() any { return &parity.Envelope{} }},
		{file: "envelope.bidder_response.json", newGoValue: func() any { return &parity.Envelope{} }},
		{file: "trace.json", newGoValue: func() any { return &parity.Trace{} }},
		{file: "adapter_requests_input.json", newGoValue: func() any { return &parity.AdapterRequestsInput{} }},
		{file: "adapter_requests_output.json", newGoValue: func() any { return &parity.AdapterRequestsOutput{} }},
		{file: "adapter_bids_input.json", newGoValue: func() any { return &parity.AdapterBidsInput{} }},
		{file: "adapter_bids_output.json", newGoValue: func() any { return &parity.AdapterBidsOutput{} }},
		{file: "diff.json", newGoValue: func() any { return &parity.DiffDocument{} }},
		{file: "index.json", newGoValue: func() any { return &parity.AuctionIndex{} }},
		{file: "health.json", newGoValue: func() any { return &parity.Health{} }},
		{file: "config.json", newGoValue: func() any { return &parity.ConfigDump{} }},
		{file: "bidders.json", newGoValue: func() any { return &parity.BiddersDump{} }},
		{file: "reset.json", newGoValue: func() any { return &parity.ResetResult{} }},
		{file: "error.json", definitionFile: FileIntrospection, definition: "Error", newGoValue: func() any {
			return &struct {
				Error string `json:"error"`
			}{}
		}},
		{file: "outbound_meta.json", definitionFile: FileCommon, definition: "OutboundMeta", newGoValue: func() any { return &parity.OutboundMeta{} }},
	}
}

func (c exampleCase) validate(doc []byte) error {
	if c.definition != "" {
		return ValidateDefinition(c.definitionFile, c.definition, doc)
	}
	return Validate(doc)
}

func TestEveryExampleIsCovered(t *testing.T) {
	names, err := Examples()
	require.NoError(t, err)
	covered := map[string]bool{}
	for _, c := range exampleCases() {
		covered[c.file] = true
	}
	for _, name := range names {
		assert.True(t, covered[name], "example %s has no test case", name)
	}
	assert.Len(t, names, len(exampleCases()))
}

func TestExamplesValidate(t *testing.T) {
	for _, c := range exampleCases() {
		t.Run(c.file, func(t *testing.T) {
			doc, err := Example(c.file)
			require.NoError(t, err)
			require.NoError(t, c.validate(doc))
		})
	}
}

// TestExamplesRoundTripThroughGoTypes proves the Go mirror types accept every
// example without unknown members and marshal back to a schema-valid document.
func TestExamplesRoundTripThroughGoTypes(t *testing.T) {
	for _, c := range exampleCases() {
		t.Run(c.file, func(t *testing.T) {
			doc, err := Example(c.file)
			require.NoError(t, err)
			v := c.newGoValue()
			dec := json.NewDecoder(bytes.NewReader(doc))
			dec.DisallowUnknownFields()
			require.NoError(t, dec.Decode(v))
			out, err := json.Marshal(v)
			require.NoError(t, err)
			require.NoError(t, c.validate(out), "re-marshaled: %s", out)
		})
	}
}

func TestInvalidDocumentsRejected(t *testing.T) {
	tests := []struct {
		name    string
		example string
		mutate  func(doc map[string]any)
		wantErr string
	}{
		{
			name:    "envelope unknown member",
			example: "envelope.oracle.json",
			mutate:  func(doc map[string]any) { doc["surprise"] = true },
			wantErr: "surprise",
		},
		{
			name:    "envelope bad mock mode",
			example: "envelope.oracle.json",
			mutate:  func(doc map[string]any) { doc["mock"].(map[string]any)["mode"] = "live" },
			wantErr: "mode",
		},
		{
			name:    "envelope binary decision out of domain",
			example: "envelope.oracle.json",
			mutate:  func(doc map[string]any) { doc["decisions"].(map[string]any)["floors.skip"] = 2 },
			wantErr: "floors.skip",
		},
		{
			name:    "envelope unknown decision",
			example: "envelope.oracle.json",
			mutate:  func(doc map[string]any) { doc["decisions"].(map[string]any)["coin.flip"] = 1 },
			wantErr: "coin.flip",
		},
		{
			name:    "envelope unknown capture stage",
			example: "envelope.inline.json",
			mutate: func(doc map[string]any) {
				doc["capture"].(map[string]any)["stages"] = []any{"not.a.stage"}
			},
			wantErr: "stages",
		},
		{
			name:    "envelope wrong schema version major",
			example: "envelope.oracle.json",
			mutate:  func(doc map[string]any) { doc["schema_version"] = "2.0.0" },
			wantErr: "schema_version",
		},
		{
			name:    "trace missing http_out",
			example: "trace.json",
			mutate:  func(doc map[string]any) { delete(doc, "http_out") },
			wantErr: "http_out",
		},
		{
			name:    "trace unregistered stage",
			example: "trace.json",
			mutate: func(doc map[string]any) {
				doc["stages"].(map[string]any)["made.up"] = map[string]any{}
			},
			wantErr: "made.up",
		},
		{
			name:    "trace bad stage status",
			example: "trace.json",
			mutate: func(doc map[string]any) {
				doc["stage_status"].(map[string]any)["http.in"] = "maybe"
			},
			wantErr: "http.in",
		},
		{
			name:    "lexical mismatch without class",
			example: "diff.json",
			mutate: func(doc map[string]any) {
				delete(doc["mismatches"].([]any)[1].(map[string]any), "class")
			},
			wantErr: "class",
		},
		{
			name:    "wire mismatch with lexical class",
			example: "diff.json",
			mutate: func(doc map[string]any) {
				doc["mismatches"].([]any)[2].(map[string]any)["class"] = "html_escape"
			},
			wantErr: "class",
		},
		{
			name:    "primary mismatch with consequential_of",
			example: "diff.json",
			mutate: func(doc map[string]any) {
				doc["mismatches"].([]any)[0].(map[string]any)["consequential_of"] = "x"
			},
			wantErr: "consequential_of",
		},
		{
			name:    "index unknown domain",
			example: "index.json",
			mutate: func(doc map[string]any) {
				doc["cases"].([]any)[0].(map[string]any)["domains"] = []any{"payments"}
			},
			wantErr: "domains",
		},
		{
			name:    "config dump leaks parity subtree",
			example: "config.json",
			mutate: func(doc map[string]any) {
				doc["config"].(map[string]any)["parity"] = map[string]any{"token": "secret"}
			},
			wantErr: "config",
		},
		{
			name:    "adapter output missing errors",
			example: "adapter_bids_output.json",
			mutate:  func(doc map[string]any) { delete(doc, "errors") },
			wantErr: "errors",
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			doc := mustExampleMap(t, tt.example)
			tt.mutate(doc)
			raw, err := json.Marshal(doc)
			require.NoError(t, err)
			err = Validate(raw)
			require.Error(t, err)
			assert.Contains(t, err.Error(), tt.wantErr)
		})
	}
}

func TestValidateRejectsUnknownDocuments(t *testing.T) {
	tests := []struct {
		name    string
		doc     string
		wantErr string
	}{
		{name: "not json", doc: `{`, wantErr: "schema member"},
		{name: "no schema member", doc: `{"a":1}`, wantErr: "no \"schema\" member"},
		{name: "unknown schema", doc: `{"schema":"pbs-parity-nope/1"}`, wantErr: "unknown document schema"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := Validate([]byte(tt.doc))
			require.Error(t, err)
			assert.Contains(t, err.Error(), tt.wantErr)
		})
	}
}

func TestEveryDocumentSchemaIsDispatched(t *testing.T) {
	names := []string{
		parity.SchemaEnvelope, parity.SchemaTrace, parity.SchemaAdapterRequestsInput,
		parity.SchemaAdapterRequestsOutput, parity.SchemaAdapterBidsInput,
		parity.SchemaAdapterBidsOutput, parity.SchemaDiff, parity.SchemaIndex,
		parity.SchemaHealth, parity.SchemaConfigDump, parity.SchemaBiddersDump,
		parity.SchemaReset, parity.SchemaStages, parity.SchemaDecisions,
		parity.SchemaNormalization, parity.SchemaFakes, parity.SchemaErrorCodes,
	}
	for _, name := range names {
		t.Run(name, func(t *testing.T) {
			file, ok := SchemaFileFor(name)
			require.True(t, ok)
			raw := mustFile(t, file)
			assert.Contains(t, string(raw), `"const": "`+name+`"`, "schema file %s must pin %s", file, name)
		})
	}
	assert.Len(t, documentSchemas, len(names))
}

func TestRegistriesValidate(t *testing.T) {
	for _, file := range []string{FileStages, FileDecisions, FileNormalization, FileFakes, FileErrorCodes} {
		t.Run(file, func(t *testing.T) {
			raw := mustFile(t, file)
			require.NoError(t, Validate(raw))
			docSchema, err := DocumentSchema(raw)
			require.NoError(t, err)
			assert.True(t, strings.HasPrefix(docSchema, "pbs-parity-"))
			var head struct {
				SchemaVersion string `json:"schema_version"`
			}
			require.NoError(t, json.Unmarshal(raw, &head))
			assert.Equal(t, parity.SchemaVersion, head.SchemaVersion)
		})
	}
}

func TestExamplesUseCurrentSchemaVersion(t *testing.T) {
	for _, c := range exampleCases() {
		if c.definition != "" {
			continue
		}
		t.Run(c.file, func(t *testing.T) {
			doc := mustExampleMap(t, c.file)
			assert.Equal(t, parity.SchemaVersion, doc["schema_version"])
		})
	}
}

func mustFile(t *testing.T, name string) []byte {
	t.Helper()
	raw, err := File(name)
	require.NoError(t, err)
	return raw
}

func mustExampleMap(t *testing.T, name string) map[string]any {
	t.Helper()
	raw, err := Example(name)
	require.NoError(t, err)
	var doc map[string]any
	require.NoError(t, json.Unmarshal(raw, &doc))
	return doc
}
