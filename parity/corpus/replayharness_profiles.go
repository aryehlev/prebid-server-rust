package corpus

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"path"

	"github.com/prebid/prebid-server/v4/parity/replayharness/rhreplay"
)

// Case profiles of RootReplayHarness: profiles/<name>/profile.json is the
// Profile the cases of profiles/<name>/replays run under, on top of the base
// profile, and the replay scripts start a Go server with it to check Go's
// answers to those replays.
const (
	rhProfilesDir = "profiles"
	rhProfileFile = "profile.json"
)

var (
	errRHProfileEmpty       = errors.New("the profile sets nothing; its replays belong in " + rhReplayDir)
	errRHProfileUnsupported = errors.New("currency_rates and default_request are not config settings; rhtool serve cannot apply them")
	errRHProfileTrailing    = errors.New("data after the profile object")
)

// rhProfile is one directory of profiles.
type rhProfile struct {
	name    string
	profile Profile
	replays []rhreplay.Replay
}

// ReadReplayHarnessProfile decodes a profile.json of
// parity/replayharness/profiles: a Profile with no unknown member that sets
// something, and only host config settings, which BuildConfig applies the
// same way for the runner and for rhtool serve.
func ReadReplayHarnessProfile(raw []byte) (Profile, error) {
	var p Profile

	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.DisallowUnknownFields()

	if err := dec.Decode(&p); err != nil {
		return Profile{}, fmt.Errorf("decoding: %w", err)
	}

	if err := dec.Decode(&json.RawMessage{}); !errors.Is(err, io.EOF) {
		return Profile{}, errRHProfileTrailing
	}

	switch {
	case p.IsDefault():
		return Profile{}, errRHProfileEmpty
	case len(p.CurrencyRates) > 0 || len(p.DefaultRequest) > 0:
		return Profile{}, errRHProfileUnsupported
	}

	return p, nil
}

// readRHProfiles reads every directory of profiles, in name order. A missing
// profiles directory reads as none.
func readRHProfiles(fsys fs.FS) ([]rhProfile, error) {
	dirs, err := fs.ReadDir(fsys, rhProfilesDir)

	switch {
	case errors.Is(err, fs.ErrNotExist):
		return nil, nil
	case err != nil:
		return nil, fmt.Errorf("%w: %w", errReplayHarness, err)
	}

	out := make([]rhProfile, 0, len(dirs))

	for _, d := range dirs {
		dir := path.Join(rhProfilesDir, d.Name())
		if !d.IsDir() {
			return nil, fmt.Errorf("%w: %s: not a profile directory", errReplayHarness, dir)
		}

		raw, err := fs.ReadFile(fsys, path.Join(dir, rhProfileFile))
		if err != nil {
			return nil, fmt.Errorf("%w: %w", errReplayHarness, err)
		}

		p, err := ReadReplayHarnessProfile(raw)
		if err != nil {
			return nil, fmt.Errorf("%w: %s: %w", errReplayHarness, path.Join(dir, rhProfileFile), err)
		}

		replays, err := rhreplay.LoadDir(fsys, path.Join(dir, rhReplayDir))
		if err != nil {
			return nil, fmt.Errorf("%w: %w", errReplayHarness, err)
		}

		out = append(out, rhProfile{name: d.Name(), profile: p, replays: replays})
	}

	return out, nil
}

// addProfileReplays adds every step of the profiles' replays. saved.jsonl
// holds the default profile's answers, so a step that sends a saved value is
// not rebuilt.
func (l *rhLoader) addProfileReplays() {
	none := func(rhreplay.Wire) (rhreplay.Response, bool) { return rhreplay.Response{}, false }

	for i := range l.in.profiles {
		p := &l.in.profiles[i]
		l.addSteps(p, p.replays, path.Join(rhProfilesDir, p.name, rhReplayDir), none)
	}
}

// profileKey keys a request of profile p apart from the same request of
// another profile; the default profile's keys stay requestKey's.
func profileKey(p *rhProfile, key string) string {
	if p == nil {
		return key
	}

	return hashHex([]byte(p.name + "\n" + key))
}

// caseProfile is the Profile of an entry's case.
func (e *rhEntry) caseProfile() Profile {
	if e.profile == nil {
		return Profile{}
	}

	return e.profile.profile
}
