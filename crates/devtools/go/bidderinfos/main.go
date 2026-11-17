// Command bidderinfos dumps static/bidder-info as the Go server loads it at startup to
// testdata/go_bidder_infos.json.
//
// It follows main.go: config.LoadBidderInfoFromDisk on the absolute bidder-info path, then
// config.SetupViper and config.New, which merges the (empty) adapters.* host config into the infos
// and validates them. The resulting cfg.BidderInfos is encoded with encoding/json (BidderInfo has
// no json tags, so keys are the Go field names).
//
// Run it through `pbs-devtools go-dump bidder-infos`.
package main

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"sort"
	"strings"

	jsoniter "github.com/json-iterator/go"
	"github.com/spf13/viper"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/errortypes"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/util/jsonutil"

	"github.com/aryehlev/prebid-server-rust/crates/devtools/go/internal/dump"
)

func init() {
	// Same as main.go, so the process behaves like the server binary.
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

type output struct {
	Generator string   `json:"generator"`
	GoCommit  string   `json:"go_commit"`
	Notes     []string `json:"notes"`
	// Files lists every directory entry of static/bidder-info and whether Go loads it.
	Files []file `json:"files"`
	// Aliases maps each aliasOf bidder to its parent (openrtb_ext.GetAliasBidderToParent()).
	Aliases map[string]string `json:"aliases"`
	// ConfigNewErrors are the validation errors config.New reported with gdpr.default_value set.
	ConfigNewErrors []string `json:"config_new_errors"`
	// BidderInfos is cfg.BidderInfos after config.New.
	BidderInfos map[string]config.BidderInfo `json:"bidder_infos"`
}

type file struct {
	Name   string `json:"name"`
	Loaded bool   `json:"loaded"`
}

func main() {
	opts := dump.ParseFlags()
	dump.ClearPBSEnv()

	infoPath := filepath.Join(opts.Root, "static", "bidder-info")
	loaded, err := config.LoadBidderInfoFromDisk(infoPath)
	if err != nil {
		dump.Fatalf("LoadBidderInfoFromDisk(%s): %v", infoPath, err)
	}
	loadedJSON := dump.Raw(loaded)

	// config.SetupViper reads pbs.* from "." and /etc/config; run from an empty directory so no
	// host config file can leak into the golden.
	empty, err := os.MkdirTemp("", "pbs-devtools-bidderinfos-")
	if err != nil {
		dump.Fatalf("creating temp dir: %v", err)
	}
	defer os.RemoveAll(empty)
	if err := os.Chdir(empty); err != nil {
		dump.Fatalf("chdir %s: %v", empty, err)
	}

	v := viper.New()
	config.SetupViper(v, "pbs", loaded)
	if used := v.ConfigFileUsed(); used != "" {
		dump.Fatalf("viper picked up a config file (%s); remove it or run elsewhere", used)
	}
	// gdpr.default_value has no default and is required; set it so that only bidder-info
	// problems would show up as validation errors.
	v.Set("gdpr.default_value", "1")
	cfg, err := config.New(v, loaded, openrtb_ext.NormalizeBidderName)
	if cfg == nil {
		dump.Fatalf("config.New: %v", err)
	}
	errs := errorStrings(err)

	if startup := dump.Raw(cfg.BidderInfos); !bytes.Equal(startup, loadedJSON) {
		dump.Fatalf("cfg.BidderInfos differs from LoadBidderInfoFromDisk; the golden format must record both")
	}

	aliases := make(map[string]string)
	for alias, parent := range openrtb_ext.GetAliasBidderToParent() {
		aliases[string(alias)] = string(parent)
	}

	out := output{
		Generator: "pbs-devtools go-dump bidder-infos (rust/crates/devtools/go/bidderinfos)",
		GoCommit:  opts.GoCommit,
		Notes: []string{
			"bidder_infos is cfg.BidderInfos after main.go's startup path with no host config; it equals LoadBidderInfoFromDisk's result (checked).",
			"Encoded with encoding/json: keys are Go field names, nil pointers/slices are null, alias infos share the parent's Capabilities/Debug/Maintainer/OpenRTB pointers.",
			"files: a directory entry is loaded when it is not a directory and strings.Split(name, \".\") is exactly [bidder, \"yaml\"].",
		},
		Files:           listFiles(infoPath),
		Aliases:         aliases,
		ConfigNewErrors: errs,
		BidderInfos:     cfg.BidderInfos,
	}
	dump.WriteJSON(opts.Out, out)
}

func errorStrings(err error) []string {
	out := []string{}
	if err == nil {
		return out
	}
	var agg errortypes.AggregateError
	if errors.As(err, &agg) {
		for _, e := range agg.Errors {
			out = append(out, e.Error())
		}
		sort.Strings(out)
		return out
	}
	return append(out, err.Error())
}

func listFiles(dir string) []file {
	entries, err := os.ReadDir(dir)
	if err != nil {
		dump.Fatalf("reading %s: %v", dir, err)
	}
	out := make([]file, 0, len(entries))
	for _, e := range entries {
		out = append(out, file{Name: e.Name(), Loaded: !e.IsDir() && isLoaded(e.Name())})
	}
	return out
}

// isLoaded mirrors the file-name filter in config.processBidderInfos (config/bidderinfo.go).
func isLoaded(name string) bool {
	parts := strings.Split(name, ".")
	return len(parts) == 2 && parts[1] == "yaml"
}
