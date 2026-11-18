package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/replayharness/rhreplay"
)

// replaysDir holds the replays, relative to the repository.
const replaysDir = "parity/replayharness/replays"

type savedOptions struct {
	repo      string
	tokenFile string
	out       string
	input     string
}

func parseSaved(args []string, stderr io.Writer) (savedOptions, error) {
	var o savedOptions

	fs := flag.NewFlagSet("saved", flag.ContinueOnError)
	fs.SetOutput(stderr)
	fs.StringVar(&o.repo, "repo", ".", "a directory inside the Prebid Server checkout")
	fs.StringVar(&o.tokenFile, "token-file", "", "file holding the parity token, which must not appear in the output (required)")
	fs.StringVar(&o.out, "o", "", "the file of saved answers to write (required)")

	if err := fs.Parse(args); err != nil {
		return o, fmt.Errorf("parsing flags: %w", err)
	}

	switch {
	case o.tokenFile == "", o.out == "":
		return o, usageError{"-token-file and -o are required"}
	case fs.NArg() != 1:
		return o, usageError{"name one recording, made with replayharness record --keep-secrets"}
	}

	o.input = fs.Arg(0)

	return o, nil
}

// savedLine is one exchange of the saved answers file.
type savedLine struct {
	Request  rhreplay.Request  `json:"request"`
	Response rhreplay.Response `json:"response"`
}

// cmdSaved keeps, from a recording of the replays made with --keep-secrets,
// Go's answer to every replay step that saves a value: the request as
// recorded, the status, the headers the step saves from and, when it saves
// a JSON path, the body. The corpus loader builds the steps that use the
// saved values from them.
func cmdSaved(args []string, stdout, stderr io.Writer) error {
	opts, err := parseSaved(args, stderr)
	if err != nil {
		return err
	}

	secret, err := readToken(opts.tokenFile)
	if err != nil {
		return err
	}

	raw, err := os.ReadFile(filepath.Clean(opts.input))
	if err != nil {
		return fmt.Errorf("reading %s: %w", opts.input, err)
	}

	exs, err := rhreplay.ReadRecording(bytes.NewReader(raw))
	if err != nil {
		return fmt.Errorf("reading %s: %w", opts.input, err)
	}

	replays, err := loadReplays(opts.repo)
	if err != nil {
		return err
	}

	out, kept, err := savedAnswers(replays, exs)
	if err != nil {
		return err
	}

	if strings.Contains(string(out), secret) {
		return errSecretInSaved
	}

	if err := os.WriteFile(filepath.Clean(opts.out), out, privatePerm); err != nil {
		return fmt.Errorf("writing %s: %w", opts.out, err)
	}

	_, _ = fmt.Fprintf(stdout, "rhtool saved: %d exchanges read, %d answers kept in %s\n", len(exs), kept, opts.out)

	return nil
}

func readToken(file string) (string, error) {
	token, err := os.ReadFile(filepath.Clean(file))
	if err != nil {
		return "", fmt.Errorf("reading the token: %w", err)
	}

	secret := strings.TrimSpace(string(token))
	if secret == "" {
		return "", errEmptyToken
	}

	return secret, nil
}

func loadReplays(start string) ([]rhreplay.Replay, error) {
	repo, err := corpus.FindRepo(start)
	if err != nil {
		return nil, fmt.Errorf("finding the repository: %w", err)
	}

	root, err := os.OpenRoot(filepath.Join(repo.Root(), filepath.FromSlash(replaysDir)))
	if err != nil {
		return nil, fmt.Errorf("opening %s: %w", replaysDir, err)
	}

	replays, err := rhreplay.LoadDir(root.FS(), ".")

	return replays, errors.Join(err, root.Close())
}

// savedAnswers rebuilds the replays with the recorded answers and returns
// the minimal exchanges of the steps that save values, in replay order.
func savedAnswers(replays []rhreplay.Replay, exs []rhreplay.Exchange) ([]byte, int, error) {
	byKey := map[string]rhreplay.Exchange{}

	for _, ex := range exs {
		if _, ok := byKey[ex.Request.Key()]; !ok {
			byKey[ex.Request.Key()] = ex
		}
	}

	answers := func(w rhreplay.Wire) (rhreplay.Response, bool) {
		ex, ok := byKey[w.Key()]

		return ex.Response, ok
	}

	var out bytes.Buffer

	written := map[string]bool{}

	for _, b := range rhreplay.BuildAll(replays, "ReplayHarness/dev", answers) {
		st := replays[b.Replay].Steps[b.Step]
		if len(st.Save) == 0 || b.NotSent != "" {
			continue
		}

		key := b.Wire.Key()

		ex, ok := byKey[key]
		if !ok {
			return nil, 0, fmt.Errorf("%w: %s replay %q step %d", errNoAnswer, replays[b.Replay].File, replays[b.Replay].Name, b.Step+1)
		}

		if written[key] {
			continue
		}

		written[key] = true

		line, err := json.Marshal(savedLine{Request: ex.Request, Response: minimal(ex.Response, st.Save)})
		if err != nil {
			return nil, 0, fmt.Errorf("encoding an answer: %w", err)
		}

		out.Write(line)
		out.WriteByte('\n')
	}

	return out.Bytes(), len(written), nil
}

// minimal keeps what the saves read: the status, the headers named by
// header: sources and, for a JSON path, the body.
func minimal(resp rhreplay.Response, saves map[string]string) rhreplay.Response {
	out := rhreplay.Response{Status: resp.Status}

	names := make([]string, 0, len(saves))
	for name := range saves {
		names = append(names, name)
	}

	sort.Strings(names)

	for _, name := range names {
		src := saves[name]

		switch {
		case src == "status":
		case strings.HasPrefix(strings.ToLower(src), "header:"):
			want := strings.TrimSpace(src[len("header:"):])
			for k, v := range resp.Headers {
				if strings.EqualFold(k, want) {
					if out.Headers == nil {
						out.Headers = map[string]string{}
					}

					out.Headers[k] = v
				}
			}
		default:
			out.Body, out.BodyEncoding = resp.Body, resp.BodyEncoding
		}
	}

	return out
}

var (
	errNoAnswer      = errors.New("the recording has no answer to a step that saves a value")
	errSecretInSaved = errors.New("the saved answers hold the parity token")
)
