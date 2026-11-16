// Package schema embeds the parity JSON Schemas, the registries (stages,
// decisions, normalization, fakes, error codes) and the hand-written examples,
// and validates parity documents against the schemas.
//
// The JSON files in this directory are the cross-language contract; the Go
// types in package parity mirror them. This package is harness-only (runner,
// tests) and must not be imported by product code.
package schema

import (
	"bytes"
	"embed"
	"encoding/json"
	"fmt"
	"io/fs"
	"path"
	"sort"
	"strings"
	"sync"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/xeipuuv/gojsonschema"
)

// BaseURI is the $id prefix shared by every schema file.
const BaseURI = "https://prebid.org/pbs-parity/schema/v1/"

// Schema and registry file names.
const (
	FileCommon        = "common.schema.json"
	FileEnvelope      = "envelope.schema.json"
	FileTrace         = "trace.schema.json"
	FileAdapterIO     = "adapter_io.schema.json"
	FileMismatch      = "mismatch.schema.json"
	FileIndex         = "index.schema.json"
	FileIntrospection = "introspection.schema.json"
	FileRegistries    = "registries.schema.json"

	FileStages        = "stages.json"
	FileDecisions     = "decisions.json"
	FileNormalization = "normalization.json"
	FileFakes         = "fakes.json"
	FileErrorCodes    = "error_codes.json"
)

// ExamplesDir is the embedded directory holding the hand-written examples.
const ExamplesDir = "examples"

//go:embed *.json examples/*.json
var files embed.FS

// schemaFiles lists every schema file; all of them are added to each compiled
// schema's pool so cross-file $refs resolve without network access.
var schemaFiles = []string{
	FileCommon, FileEnvelope, FileTrace, FileAdapterIO, FileMismatch,
	FileIndex, FileIntrospection, FileRegistries,
}

// documentSchemas maps the "schema" member of a document to its schema file.
var documentSchemas = map[string]string{
	parity.SchemaEnvelope:              FileEnvelope,
	parity.SchemaTrace:                 FileTrace,
	parity.SchemaAdapterRequestsInput:  FileAdapterIO,
	parity.SchemaAdapterRequestsOutput: FileAdapterIO,
	parity.SchemaAdapterBidsInput:      FileAdapterIO,
	parity.SchemaAdapterBidsOutput:     FileAdapterIO,
	parity.SchemaDiff:                  FileMismatch,
	parity.SchemaIndex:                 FileIndex,
	parity.SchemaHealth:                FileIntrospection,
	parity.SchemaConfigDump:            FileIntrospection,
	parity.SchemaBiddersDump:           FileIntrospection,
	parity.SchemaReset:                 FileIntrospection,
	parity.SchemaStages:                FileRegistries,
	parity.SchemaDecisions:             FileRegistries,
	parity.SchemaNormalization:         FileRegistries,
	parity.SchemaFakes:                 FileRegistries,
	parity.SchemaErrorCodes:            FileRegistries,
}

var (
	compiledMu sync.Mutex
	compiled   = map[string]*gojsonschema.Schema{}
)

// File returns the raw bytes of an embedded schema or registry file.
func File(name string) ([]byte, error) {
	raw, err := files.ReadFile(name)
	if err != nil {
		return nil, fmt.Errorf("parity schema: reading %s: %w", name, err)
	}
	return raw, nil
}

// Examples returns the names of the embedded example documents, sorted.
func Examples() ([]string, error) {
	entries, err := fs.ReadDir(files, ExamplesDir)
	if err != nil {
		return nil, fmt.Errorf("parity schema: listing examples: %w", err)
	}
	names := make([]string, 0, len(entries))
	for _, e := range entries {
		if !e.IsDir() && strings.HasSuffix(e.Name(), ".json") {
			names = append(names, e.Name())
		}
	}
	sort.Strings(names)
	return names, nil
}

// Example returns the bytes of one embedded example document.
func Example(name string) ([]byte, error) {
	return File(path.Join(ExamplesDir, name))
}

// SchemaFileFor returns the schema file that validates documents whose
// "schema" member is docSchema.
func SchemaFileFor(docSchema string) (string, bool) {
	file, ok := documentSchemas[docSchema]
	return file, ok
}

// DocumentSchema reads the "schema" member of a document.
func DocumentSchema(doc []byte) (string, error) {
	var head struct {
		Schema string `json:"schema"`
	}
	if err := json.Unmarshal(doc, &head); err != nil {
		return "", fmt.Errorf("parity schema: reading document schema member: %w", err)
	}
	if head.Schema == "" {
		return "", fmt.Errorf("parity schema: document has no \"schema\" member")
	}
	return head.Schema, nil
}

// Validate validates doc against the schema selected by its "schema" member.
func Validate(doc []byte) error {
	docSchema, err := DocumentSchema(doc)
	if err != nil {
		return err
	}
	file, ok := SchemaFileFor(docSchema)
	if !ok {
		return fmt.Errorf("parity schema: unknown document schema %q", docSchema)
	}
	return ValidateFile(file, doc)
}

// ValidateFile validates doc against one schema file (for example FileTrace).
func ValidateFile(file string, doc []byte) error {
	return validateRef(BaseURI+file, doc)
}

// ValidateDefinition validates doc against one definition of a schema file,
// for example ValidateDefinition(FileCommon, "OutboundMeta", doc) for a decoded
// X-Parity-Meta payload, or (FileIntrospection, "Error", doc).
func ValidateDefinition(file, definition string, doc []byte) error {
	return validateRef(BaseURI+file+"#/definitions/"+definition, doc)
}

func validateRef(ref string, doc []byte) error {
	s, err := compile(ref)
	if err != nil {
		return err
	}
	result, err := s.Validate(gojsonschema.NewBytesLoader(doc))
	if err != nil {
		return fmt.Errorf("parity schema: validating against %s: %w", ref, err)
	}
	if result.Valid() {
		return nil
	}
	msgs := make([]string, 0, len(result.Errors()))
	for _, e := range result.Errors() {
		msgs = append(msgs, e.String())
	}
	sort.Strings(msgs)
	return fmt.Errorf("parity schema: document does not match %s: %s", ref, strings.Join(msgs, "; "))
}

// compile returns the compiled schema for ref, compiling it once.
func compile(ref string) (*gojsonschema.Schema, error) {
	compiledMu.Lock()
	defer compiledMu.Unlock()
	if s, ok := compiled[ref]; ok {
		return s, nil
	}
	loader := gojsonschema.NewSchemaLoader()
	loader.Draft = gojsonschema.Draft7
	loader.AutoDetect = false
	for _, name := range schemaFiles {
		raw, err := File(name)
		if err != nil {
			return nil, err
		}
		if err := loader.AddSchemas(gojsonschema.NewBytesLoader(raw)); err != nil {
			return nil, fmt.Errorf("parity schema: adding %s: %w", name, err)
		}
	}
	root, err := json.Marshal(map[string]string{"$ref": ref})
	if err != nil {
		return nil, fmt.Errorf("parity schema: building root for %s: %w", ref, err)
	}
	s, err := loader.Compile(gojsonschema.NewBytesLoader(root))
	if err != nil {
		return nil, fmt.Errorf("parity schema: compiling %s: %w", ref, err)
	}
	compiled[ref] = s
	return s, nil
}

// decodeStrict decodes an embedded registry file into v, rejecting unknown
// members so the Go mirror cannot silently drift from the JSON.
func decodeStrict(name string, v any) error {
	raw, err := File(name)
	if err != nil {
		return err
	}
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.DisallowUnknownFields()
	if err := dec.Decode(v); err != nil {
		return fmt.Errorf("parity schema: decoding %s: %w", name, err)
	}
	return nil
}
