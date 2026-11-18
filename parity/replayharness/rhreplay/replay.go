// Package rhreplay reads ReplayHarness replay files and recordings and
// rebuilds the requests ReplayHarness sends for them, byte for byte.
//
// It follows github.com/aryehlev/ReplayHarness at PinnedCommit, the commit
// parity/scripts/replayharness.sh builds: internal/replay/replay.go (Parse,
// validate, normalize and Load), internal/replay/run.go (BuildRequest,
// substitute, substituteValue, scalar, statusMatcher, extract, lookup and
// headerOf), internal/runner/runner.go (Client.Do sets the User-Agent and
// then every step header with http.Header.Set) and the recording format of
// internal/discover/traffic.go. Moving the pin means reading those files
// again.
package rhreplay

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"path"
	"sort"
	"strconv"
	"strings"

	"gopkg.in/yaml.v3"
)

// PinnedCommit is the ReplayHarness commit this package mirrors.
const PinnedCommit = "f61ab1997f27b0044eeb801caea62810d75e2d76"

// PinnedVersion is the version that a ReplayHarness CLI built from an
// unmodified checkout of PinnedCommit reports: the Go pseudo-version of the
// commit without its leading "v" (cmd/replayharness/main.go reads it from
// the build info). Its requests carry the User-Agent "ReplayHarness/" +
// PinnedVersion; a build with local changes appends "+dirty" to it.
// parity/replayharness/traffic.jsonl and saved.jsonl are recordings of that
// build. Moving the pin means updating both constants and recording both
// files again.
const PinnedVersion = "0.1.1-0.20261116212744-f61ab1997f27"

// Errors the package reports.
var (
	ErrBadReplay    = errors.New("rhreplay: invalid replay")
	ErrBadRecording = errors.New("rhreplay: invalid recording")
	ErrNotSendable  = errors.New("rhreplay: ReplayHarness cannot send the step")
)

// Replay is one named sequence of steps.
type Replay struct {
	Name  string `json:"name"`
	Why   string `json:"why,omitempty"`
	Steps []Step `json:"steps"`
	// File is the slash path of the file the replay came from, relative to
	// the directory LoadDir read.
	File string `json:"-"`
}

// Step is one request of a replay and what to check and save from its
// answer.
type Step struct {
	Name    string            `json:"name,omitempty"`
	Method  string            `json:"method"`
	Path    string            `json:"path"`
	Query   map[string]any    `json:"query,omitempty"`
	Headers map[string]string `json:"headers,omitempty"`
	JSON    any               `json:"json,omitempty"`
	Body    *string           `json:"body,omitempty"`
	NoAuth  bool              `json:"no_auth,omitempty"`
	Expect  Expect            `json:"expect,omitzero"`
	Save    map[string]string `json:"save,omitempty"`
}

// Expect is what a step checks in its answer.
type Expect struct {
	Status      any               `json:"status,omitempty"`
	JSON        map[string]any    `json:"json,omitempty"`
	Headers     map[string]string `json:"headers,omitempty"`
	Contains    []string          `json:"contains,omitempty"`
	NotContains []string          `json:"not_contains,omitempty"`
	MaxMS       float64           `json:"max_ms,omitempty"`
}

// Parse reads replays from YAML or JSON: {replays: [...]}, {flows: [...]}, a
// list of replays or a single replay. Like ReplayHarness it decodes the
// document generically, then re-encodes it as JSON and decodes that strictly,
// so numbers inside a step's json body become float64.
func Parse(data []byte) ([]Replay, error) {
	doc, err := decodeDocument(data)
	if err != nil {
		return nil, err
	}

	list, err := replayList(normalize(doc))
	if err != nil {
		return nil, err
	}

	replays, err := decodeStrict(list)
	if err != nil {
		return nil, err
	}

	for i := range replays {
		if err := replays[i].validate(); err != nil {
			name := replays[i].Name
			if name == "" {
				name = "#" + strconv.Itoa(i+1)
			}

			return nil, fmt.Errorf("replay %s: %w", name, err)
		}
	}

	return replays, nil
}

// decodeDocument decodes a JSON document, or else YAML, generically.
func decodeDocument(data []byte) (any, error) {
	trimmed := bytes.TrimSpace(data)

	var doc any

	var err error
	if len(trimmed) > 0 && (trimmed[0] == '{' || trimmed[0] == '[') {
		err = json.Unmarshal(trimmed, &doc)
	} else {
		err = yaml.Unmarshal(trimmed, &doc)
	}

	if err != nil {
		return nil, fmt.Errorf("%w: %w", ErrBadReplay, err)
	}

	return doc, nil
}

// decodeStrict re-encodes the replay list as JSON and decodes it into
// replays, refusing unknown fields.
func decodeStrict(list any) ([]Replay, error) {
	raw, err := json.Marshal(list)
	if err != nil {
		return nil, fmt.Errorf("%w: %w", ErrBadReplay, err)
	}

	var replays []Replay

	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.DisallowUnknownFields()

	if err := dec.Decode(&replays); err != nil {
		return nil, fmt.Errorf("%w: %w", ErrBadReplay, err)
	}

	return replays, nil
}

func replayList(doc any) (any, error) {
	switch t := doc.(type) {
	case map[string]any:
		if list, ok := t["replays"]; ok {
			return list, nil
		}

		if list, ok := t["flows"]; ok {
			return list, nil
		}

		return []any{t}, nil
	case []any:
		return t, nil
	}

	return nil, fmt.Errorf("%w: expected replays: [...], a list of replays or one replay", ErrBadReplay)
}

// validate checks a replay the way ReplayHarness does and upper-cases each
// step's method.
func (r *Replay) validate() error {
	if strings.TrimSpace(r.Name) == "" {
		return fmt.Errorf("%w: name is required", ErrBadReplay)
	}

	if len(r.Steps) == 0 {
		return fmt.Errorf("%w: at least one step is required", ErrBadReplay)
	}

	for i := range r.Steps {
		if err := r.Steps[i].validate(); err != nil {
			return fmt.Errorf("step %d: %w", i+1, err)
		}
	}

	return nil
}

func (s *Step) validate() error {
	s.Method = strings.ToUpper(strings.TrimSpace(s.Method))

	switch {
	case s.Method == "":
		return fmt.Errorf("%w: method is required", ErrBadReplay)
	case !strings.HasPrefix(s.Path, "/") && !strings.HasPrefix(s.Path, "{{"):
		return fmt.Errorf("%w: path must start with /, got %q", ErrBadReplay, s.Path)
	case s.JSON != nil && s.Body != nil:
		return fmt.Errorf("%w: use json or body, not both", ErrBadReplay)
	}

	if err := checkStatus(s.Expect.Status); err != nil {
		return err
	}

	return checkSaves(s.Save)
}

func checkSaves(saves map[string]string) error {
	for name, src := range saves {
		if src != "status" && !strings.HasPrefix(src, "$") && !strings.HasPrefix(strings.ToLower(src), "header:") {
			return fmt.Errorf("%w: save %s: use a JSON path like $.id, header:Location or status", ErrBadReplay, name)
		}
	}

	return nil
}

// checkStatus accepts what statusMatcher accepts: nothing, a code like 404,
// a class like 4xx, or a list of them.
func checkStatus(spec any) error {
	if spec == nil {
		return nil
	}

	items, ok := spec.([]any)
	if !ok {
		items = []any{spec}
	}

	for _, item := range items {
		if !validStatus(strings.ToLower(strings.TrimSpace(Scalar(item)))) {
			return fmt.Errorf("%w: expect.status %v: use a code like 404, a class like 4xx, or a list of them", ErrBadReplay, item)
		}
	}

	return nil
}

func validStatus(s string) bool {
	if len(s) == 3 && s[1:] == "xx" && s[0] >= '1' && s[0] <= '5' {
		return true
	}

	n, err := strconv.Atoi(s)

	return err == nil && n >= 100 && n <= 599
}

// normalize turns YAML maps with non-string keys into string-keyed maps.
func normalize(v any) any {
	switch t := v.(type) {
	case map[string]any:
		for k, val := range t {
			t[k] = normalize(val)
		}

		return t
	case map[any]any:
		m := make(map[string]any, len(t))
		for k, val := range t {
			m[fmt.Sprint(k)] = normalize(val)
		}

		return m
	case []any:
		for i := range t {
			t[i] = normalize(t[i])
		}

		return t
	}

	return v
}

// LoadDir reads the replay files of dir in fsys the way `replayharness
// replay <dir>` does: the .yaml, .yml and .json files directly in dir, in
// name order. Each replay's File is its file's path relative to dir.
func LoadDir(fsys fs.FS, dir string) ([]Replay, error) {
	entries, err := fs.ReadDir(fsys, dir)
	if err != nil {
		return nil, fmt.Errorf("rhreplay: reading %s: %w", dir, err)
	}

	var names []string

	for _, e := range entries {
		switch strings.ToLower(path.Ext(e.Name())) {
		case ".yaml", ".yml", ".json":
			if !e.IsDir() {
				names = append(names, e.Name())
			}
		}
	}

	sort.Strings(names)

	var all []Replay

	for _, name := range names {
		data, err := fs.ReadFile(fsys, path.Join(dir, name))
		if err != nil {
			return nil, fmt.Errorf("rhreplay: reading %s: %w", name, err)
		}

		replays, err := Parse(data)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", name, err)
		}

		for i := range replays {
			replays[i].File = name
		}

		all = append(all, replays...)
	}

	return all, nil
}
