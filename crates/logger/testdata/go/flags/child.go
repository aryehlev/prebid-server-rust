package main

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"os"
	"os/exec"
	"reflect"
)

// childEnv makes the program parse its arguments as Prebid Server does instead of writing the
// goldens.
const childEnv = "PBS_LOGGER_FLAGS_CHILD"

// program is os.Args[0] in the child: the binary's name, as in the image's ENTRYPOINT
// (Dockerfile:49) without its directory.
const program = "prebid-server"

// glogFlagNames are the flags glog v1.2.5 registers (glog_flags.go:388-397,
// glog_file.go:44-46), in flag.VisitAll's order.
var glogFlagNames = []string{
	"alsologtostderr", "log_backtrace_at", "log_dir", "log_link", "logbuflevel",
	"logtostderr", "stderrthreshold", "v", "vmodule",
}

// childOutput is what the child prints once flag.Parse returns: every flag's String() and the
// arguments left, hex-encoded so that bytes that are not UTF-8 survive.
type childOutput struct {
	Values map[string]string `json:"values"`
	Rest   []string          `json:"rest"`
}

// child is Prebid Server's main up to and including its first statement.
func child() {
	flag.Parse() // main.go:28: flag.CommandLine is ExitOnError, so -h exits 0 and an error 2
	out := childOutput{Values: map[string]string{}, Rest: []string{}}
	flag.VisitAll(func(f *flag.Flag) {
		out.Values[f.Name] = hex.EncodeToString([]byte(f.Value.String()))
	})
	for _, arg := range flag.Args() {
		out.Rest = append(out.Rest, hex.EncodeToString([]byte(arg)))
	}
	if err := json.NewEncoder(os.Stdout).Encode(out); err != nil {
		os.Exit(1)
	}
}

// result is what one command line did.
type result struct {
	// Outcome is "parsed" (flag.Parse returned), "help" (it exited 0) or "error" (it exited 2).
	Outcome string
	// Stderr is everything flag.Parse wrote.
	Stderr string
	// Values maps every flag to its String() once parsed.
	Values map[string]string
	// Rest is flag.Args() once parsed.
	Rest []string
}

// runChild runs args through the child's flag.Parse and returns what it did with its exit code.
func runChild(args []string) (result, int) {
	exe, err := os.Executable()
	if err != nil {
		fail(err.Error())
	}
	var stdout, stderr bytes.Buffer
	cmd := &exec.Cmd{
		Path:   exe,
		Args:   append([]string{program}, args...),
		Env:    append(os.Environ(), childEnv+"=1"),
		Stdout: &stdout,
		Stderr: &stderr,
	}
	err = cmd.Run()
	if err == nil {
		// flag.Parse exits 0 after -h without returning; the child prints the values only when
		// it returns.
		if stdout.Len() == 0 {
			return result{Outcome: "help", Stderr: stderr.String()}, 0
		}
		var out childOutput
		if err := json.Unmarshal(stdout.Bytes(), &out); err != nil {
			fail("the child's values: " + err.Error())
		}
		r := result{Outcome: "parsed", Stderr: stderr.String(), Values: map[string]string{}, Rest: []string{}}
		for name, value := range out.Values {
			r.Values[name] = unhex(value)
		}
		for _, arg := range out.Rest {
			r.Rest = append(r.Rest, unhex(arg))
		}
		return r, 0
	}
	var exit *exec.ExitError
	if errors.As(err, &exit) && exit.ExitCode() == 2 {
		return result{Outcome: "error", Stderr: stderr.String()}, 2
	}
	fail("the child did not exit 0 or 2: " + err.Error())
	return result{}, -1
}

// glogFlags are glog's flags as flag.CommandLine holds them. alone says that it holds nothing
// else, as in Prebid Server when no argument starts with -httptest.serve= (the generator's own
// arguments never do); a test binary adds the testing package's flags.
func glogFlags(alone bool) []*flag.Flag {
	var flags []*flag.Flag
	var names, all []string
	flag.CommandLine.VisitAll(func(f *flag.Flag) {
		all = append(all, f.Name)
		for _, name := range glogFlagNames {
			if f.Name == name {
				flags = append(flags, f)
				names = append(names, f.Name)
			}
		}
	})
	if !reflect.DeepEqual(names, glogFlagNames) || alone && !reflect.DeepEqual(all, glogFlagNames) {
		fail("flag.CommandLine does not hold glog's nine flags alone")
	}
	return flags
}

// resetFlags sets every glog flag back to its default.
func resetFlags(flags []*flag.Flag) {
	for _, f := range flags {
		if err := f.Value.Set(f.DefValue); err != nil {
			fail(err.Error())
		}
	}
}

// emulate parses args in this process through a ContinueOnError set named like the binary that
// shares glog's flag values; the matrix shows that it does what the child's flag.Parse does.
func emulate(flags []*flag.Flag, args []string) result {
	// Reset first: fs.Var takes each flag's default for the usage listing from its value.
	resetFlags(flags)
	var out bytes.Buffer
	fs := flag.NewFlagSet(program, flag.ContinueOnError)
	fs.SetOutput(&out)
	for _, f := range flags {
		fs.Var(f.Value, f.Name, f.Usage)
	}
	err := fs.Parse(args)
	switch {
	case errors.Is(err, flag.ErrHelp):
		return result{Outcome: "help", Stderr: out.String()}
	case err != nil:
		return result{Outcome: "error", Stderr: out.String()}
	}
	r := result{Outcome: "parsed", Stderr: out.String(), Values: map[string]string{}, Rest: []string{}}
	fs.VisitAll(func(f *flag.Flag) { r.Values[f.Name] = f.Value.String() })
	r.Rest = append(r.Rest, fs.Args()...)
	return r
}

// sameResult reports whether the child and the emulation agree.
func sameResult(a, b result) bool {
	return a.Outcome == b.Outcome && a.Stderr == b.Stderr && reflect.DeepEqual(a.Values, b.Values) &&
		reflect.DeepEqual(a.Rest, b.Rest)
}

func unhex(s string) string {
	b, err := hex.DecodeString(s)
	if err != nil {
		fail(err.Error())
	}
	return string(b)
}
