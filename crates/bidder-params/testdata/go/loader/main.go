// Command loader writes the golden of how openrtb_ext.NewBidderParamsValidator finds, opens and
// reports the schema files of a directory, from the real Go code. The Rust pbs-bidder-params
// crate opens each file at its absolute path with std instead (jsonschema.schema-files):
//
//   - Prebid makes the path absolute with filepath.Abs: os.Getwd, which gives $PWD when $PWD
//     names the working directory, then filepath.Clean, which drops "..";
//   - it loads the schema through gojsonschema's reference loader for "file:///" + path:
//     url.Parse refuses a control character and a bad % escape (in the fragment too), a '#'
//     starts a fragment that gojsonschema resolves as a JSON pointer into the file before it,
//     and url.QueryUnescape decodes %XX and turns '+' into a space before os.Open;
//   - os's errors print as *fs.PathError ("open <path>: no such file or directory").
//
// Run it from the directory of this module (crates/bidder-params/testdata/go):
//
//	go run ./loader ../golden
//
// It writes loader.jsonl, one line per case: the files of the case's root directory, the schema
// directory Prebid is given, the working directory and $PWD it runs with, and Go's outcome: the
// error text, or, when the validator loads, the appnexus schema's contents and the error of
// validating the document 1 against the schema Go compiled for appnexus, and the same two for
// appnexusalias, an alias of appnexus every case runs with. The case's root directory is
// spelled $ROOT.
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"log"
	"os"
	"path/filepath"
	"strings"

	"github.com/prebid/prebid-server/v4/openrtb_ext"
)

// rootToken stands for the root directory of a case in the golden.
const rootToken = "$ROOT"

// alias is the alias of appnexus registered before every case, as Prebid registers the aliases
// of static/bidder-info before it loads the schemas: a file of its name loads, then the alias
// takes its parent's schema.
const alias = "appnexusalias"

// entry is one file system entry of a case, under its root.
type entry struct {
	// Path is relative to the root.
	Path string `json:"path"`
	// Kind is "file", "dir" or "symlink".
	Kind string `json:"kind"`
	// Contents is a file's contents.
	Contents string `json:"contents,omitempty"`
	// Target is a symlink's target, relative to the symlink's directory.
	Target string `json:"target,omitempty"`
}

// loaderCase is one call of NewBidderParamsValidator and its outcome.
type loaderCase struct {
	Name    string  `json:"name"`
	Entries []entry `json:"entries"`
	// Dir is the schema directory Prebid is given.
	Dir string `json:"dir"`
	// Cwd, when set, is the working directory of the call, under the root.
	Cwd string `json:"cwd,omitempty"`
	// Pwd, when set, is the $PWD of the call, under the root; $PWD is unset otherwise.
	Pwd string `json:"pwd,omitempty"`
	// Loaded says whether the validator loads.
	Loaded bool `json:"loaded"`
	// Error is the error text when it does not.
	Error string `json:"error,omitempty"`
	// Schema is Schema("appnexus") when it loads.
	Schema string `json:"schema,omitempty"`
	// Probe is the text of Validate("appnexus", `1`) when it loads (see probe): it tells which
	// schema Go compiled.
	Probe string `json:"probe,omitempty"`
	// AliasSchema and AliasProbe are Schema and Probe for alias.
	AliasSchema string `json:"aliasSchema,omitempty"`
	AliasProbe  string `json:"aliasProbe,omitempty"`
}

func fileEntry(path, contents string) entry {
	return entry{Path: path, Kind: "file", Contents: contents}
}

func dirEntry(path string) entry {
	return entry{Path: path, Kind: "dir"}
}

func symlinkEntry(path, target string) entry {
	return entry{Path: path, Kind: "symlink", Target: target}
}

// The schemas of the cases: anything goes, objects only, and one that does not load.
const (
	anySchema    = `{}`
	objectSchema = `{"type":"object"}`
	badSchema    = `{"type":1}`
)

func cases() []loaderCase {
	return []loaderCase{
		{Name: "loads", Entries: []entry{fileEntry("d/appnexus.json", anySchema)}, Dir: "$ROOT/d"},
		{Name: "missing directory", Dir: "$ROOT/missing"},
		{Name: "a file, not a directory", Entries: []entry{fileEntry("f", "")}, Dir: "$ROOT/f"},
		{Name: "file name is not a bidder", Entries: []entry{fileEntry("d/anyBidder.json", anySchema)}, Dir: "$ROOT/d"},
		{Name: "schema does not compile", Entries: []entry{fileEntry("d/appnexus.json", badSchema)}, Dir: "$ROOT/d"},
		{Name: "schema is not JSON", Entries: []entry{fileEntry("d/appnexus.json", "{")}, Dir: "$ROOT/d"},
		{Name: "schema is a directory", Entries: []entry{dirEntry("d/appnexus.json")}, Dir: "$ROOT/d"},
		{Name: "schema is a dangling symlink", Entries: []entry{dirEntry("d"), symlinkEntry("d/appnexus.json", "gone.json")}, Dir: "$ROOT/d"},
		{Name: "space", Entries: []entry{fileEntry("a b/appnexus.json", objectSchema)}, Dir: "$ROOT/a b"},
		{Name: "question mark", Entries: []entry{fileEntry("a?b/appnexus.json", objectSchema)}, Dir: "$ROOT/a?b"},
		{Name: "dot-dot", Entries: []entry{dirEntry("sub"), fileEntry("d/appnexus.json", badSchema)}, Dir: "$ROOT/sub/../d"},
		{Name: "plus", Entries: []entry{fileEntry("a+b/appnexus.json", objectSchema)}, Dir: "$ROOT/a+b"},
		{
			Name:    "plus beside its decoded twin",
			Entries: []entry{fileEntry("a+b/appnexus.json", anySchema), fileEntry("a b/appnexus.json", objectSchema)},
			Dir:     "$ROOT/a+b",
		},
		{Name: "percent escape", Entries: []entry{fileEntry("a%41/appnexus.json", objectSchema)}, Dir: "$ROOT/a%41"},
		{
			Name:    "percent escape beside its decoded twin",
			Entries: []entry{fileEntry("a%41/appnexus.json", anySchema), fileEntry("aA/appnexus.json", objectSchema)},
			Dir:     "$ROOT/a%41",
		},
		{Name: "bad percent escape", Entries: []entry{fileEntry("a%zz/appnexus.json", objectSchema)}, Dir: "$ROOT/a%zz"},
		{
			Name:    "multi-byte text before a bad percent escape",
			Entries: []entry{fileEntry("é%zz/appnexus.json", objectSchema)},
			Dir:     "$ROOT/é%zz",
		},
		{Name: "fragment", Entries: []entry{fileEntry("a#b/appnexus.json", objectSchema)}, Dir: "$ROOT/a#b"},
		{
			Name:    "bad percent escape after a fragment",
			Entries: []entry{fileEntry("a#b%zz/appnexus.json", objectSchema)},
			Dir:     "$ROOT/a#b%zz",
		},
		{
			Name: "fragment after a JSON file",
			Entries: []entry{
				fileEntry("doc", `{"definitions":{"appnexus.json":{"type":"object"}}}`),
				fileEntry("doc#/definitions/appnexus.json", anySchema),
			},
			Dir: "$ROOT/doc#/definitions",
		},
		{Name: "control character", Entries: []entry{fileEntry("a\tb/appnexus.json", objectSchema)}, Dir: "$ROOT/a\tb"},
		{
			Name:    "relative, $PWD names the working directory",
			Entries: []entry{fileEntry("real/d/appnexus.json", badSchema), symlinkEntry("link", "real")},
			Dir:     "d",
			Cwd:     "real",
			Pwd:     "link",
		},
		{
			Name:    "relative, $PWD unset",
			Entries: []entry{fileEntry("real/d/appnexus.json", badSchema), symlinkEntry("link", "real")},
			Dir:     "d",
			Cwd:     "real",
		},
		{
			// os.ReadDir sorts the entries by name, and the first that fails stops the load.
			Name: "files fail in the order of their names",
			Entries: []entry{
				fileEntry("d/m.json", anySchema), fileEntry("d/x.json", anySchema),
				fileEntry("d/f.json", anySchema), fileEntry("d/b.json", anySchema),
				fileEntry("d/q.json", anySchema), fileEntry("d/e.json", anySchema),
				fileEntry("d/t.json", anySchema), fileEntry("d/k.json", anySchema),
			},
			Dir: "$ROOT/d",
		},
		{
			Name:    "a schema fails before a later file name",
			Entries: []entry{fileEntry("d/zz.json", anySchema), fileEntry("d/appnexus.json", badSchema)},
			Dir:     "$ROOT/d",
		},
		{Name: "a file without .json", Entries: []entry{fileEntry("d/appnexus", objectSchema)}, Dir: "$ROOT/d"},
		{
			Name:    "an alias's own schema gives way to its parent's",
			Entries: []entry{fileEntry("d/appnexus.json", objectSchema), fileEntry("d/appnexusalias.json", anySchema)},
			Dir:     "$ROOT/d",
		},
		{
			Name:    "an alias's own schema without its parent's",
			Entries: []entry{fileEntry("d/appnexusalias.json", objectSchema)},
			Dir:     "$ROOT/d",
		},
	}
}

func main() {
	if len(os.Args) != 2 {
		log.Fatal("usage: loader <output directory>")
	}
	out, err := filepath.Abs(os.Args[1])
	if err != nil {
		log.Fatal(err)
	}
	start, err := os.Getwd()
	if err != nil {
		log.Fatal(err)
	}
	pwd, hadPwd := os.LookupEnv("PWD")
	if err := openrtb_ext.SetAliasBidderName(alias, openrtb_ext.BidderAppnexus); err != nil {
		log.Fatal(err)
	}

	var lines bytes.Buffer
	for _, c := range cases() {
		run(&c)
		if err := os.Chdir(start); err != nil {
			log.Fatal(err)
		}
		restorePwd(pwd, hadPwd)
		enc := json.NewEncoder(&lines)
		enc.SetEscapeHTML(false)
		if err := enc.Encode(c); err != nil {
			log.Fatal(err)
		}
	}

	f, err := os.Create(filepath.Join(out, "loader.jsonl"))
	if err != nil {
		log.Fatal(err)
	}
	w := bufio.NewWriter(f)
	if _, err := w.Write(lines.Bytes()); err != nil {
		log.Fatal(err)
	}
	if err := w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := f.Close(); err != nil {
		log.Fatal(err)
	}
}

// run makes the case's files under a new root, calls NewBidderParamsValidator as the case says
// and records the outcome in c.
func run(c *loaderCase) {
	temp, err := os.MkdirTemp("", "pbs-bidder-params-loader-")
	if err != nil {
		log.Fatal(err)
	}
	defer func() {
		if err := os.RemoveAll(temp); err != nil {
			log.Fatal(err)
		}
	}()
	// The physical spelling, so that os.Getwd gives the same root whatever $PWD says.
	root, err := filepath.EvalSymlinks(temp)
	if err != nil {
		log.Fatal(err)
	}
	for _, e := range c.Entries {
		create(root, e)
	}
	if c.Cwd != "" {
		if err := os.Chdir(filepath.Join(root, c.Cwd)); err != nil {
			log.Fatal(err)
		}
	}
	if c.Pwd != "" {
		if err := os.Setenv("PWD", filepath.Join(root, c.Pwd)); err != nil {
			log.Fatal(err)
		}
	} else if err := os.Unsetenv("PWD"); err != nil {
		log.Fatal(err)
	}

	validator, err := openrtb_ext.NewBidderParamsValidator(strings.ReplaceAll(c.Dir, rootToken, root))
	if err != nil {
		c.Error = strings.ReplaceAll(err.Error(), root, rootToken)
		return
	}
	c.Loaded = true
	c.Schema = validator.Schema(openrtb_ext.BidderAppnexus)
	c.Probe = probe(validator, openrtb_ext.BidderAppnexus)
	c.AliasSchema = validator.Schema(alias)
	c.AliasProbe = probe(validator, alias)
}

// probe is the text of validating the document 1 against name's schema: empty when valid, the
// error's text, or "panic" where Go panics, as it does on the nil schema of a bidder without
// one.
func probe(validator openrtb_ext.BidderParamValidator, name openrtb_ext.BidderName) (text string) {
	defer func() {
		if recover() != nil {
			text = "panic"
		}
	}()
	if err := validator.Validate(name, []byte("1")); err != nil {
		return err.Error()
	}
	return ""
}

// create makes one entry under root.
func create(root string, e entry) {
	path := filepath.Join(root, e.Path)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		log.Fatal(err)
	}
	switch e.Kind {
	case "file":
		if err := os.WriteFile(path, []byte(e.Contents), 0o644); err != nil {
			log.Fatal(err)
		}
	case "dir":
		if err := os.MkdirAll(path, 0o755); err != nil {
			log.Fatal(err)
		}
	case "symlink":
		if err := os.Symlink(e.Target, path); err != nil {
			log.Fatal(err)
		}
	default:
		log.Fatalf("unknown entry kind %q", e.Kind)
	}
}

func restorePwd(pwd string, had bool) {
	if had {
		if err := os.Setenv("PWD", pwd); err != nil {
			log.Fatal(err)
		}
	} else if err := os.Unsetenv("PWD"); err != nil {
		log.Fatal(err)
	}
}
