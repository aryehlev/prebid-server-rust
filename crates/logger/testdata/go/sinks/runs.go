package main

import (
	"bytes"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

// program is os.Args[0] in the children, which glog names its files after.
const program = "prebid-server"

// Placeholders in a run's arguments: LOGDIR and LINKDIR are empty directories, MISSING a path
// that does not exist.
const (
	logDirArg  = "LOGDIR"
	linkDirArg = "LINKDIR"
	missingArg = "MISSING"
)

// runs are the golden's child runs: the stderr flags with the four facade levels, the file flags,
// the flags glog accepts and Prebid Server never uses, Fatalf, and the standard logger.
var runs = []struct {
	scenario string
	args     []string
}{
	{"levels", []string{}},
	{"levels", []string{"-logtostderr"}},
	{"levels", []string{"-v", "1", "-logtostderr"}},
	{"levels", []string{"-alsologtostderr"}},
	{"levels", []string{"-stderrthreshold=INFO"}},
	{"levels", []string{"-stderrthreshold=WARNING"}},
	{"levels", []string{"-stderrthreshold=ERROR"}},
	{"levels", []string{"-stderrthreshold=FATAL"}},
	{"levels", []string{"-logtostderr", "-stderrthreshold=FATAL"}},
	{"levels", []string{"-alsologtostderr", "-stderrthreshold=FATAL"}},
	{"levels", []string{"-logtostderr=false", "-alsologtostderr=false"}},
	{"levels", []string{"-log_dir=" + logDirArg}},
	{"levels", []string{"-log_dir=" + logDirArg, "-log_link=" + linkDirArg}},
	{"levels", []string{"-log_dir=" + missingArg}},
	{"levels", []string{"-logbuflevel=-1", "-log_dir=" + logDirArg}},
	{"levels", []string{"-v=4", "-vmodule=logger=3,glog=2", "-logtostderr"}},
	{"levels", []string{"-log_backtrace_at=logger.go:17", "-logtostderr"}},
	{"fatal", []string{"-logtostderr"}},
	{"fatal", []string{}},
	{"fatal", []string{"-stderrthreshold=FATAL"}},
	{"stdlog", []string{}},
	{"stdlog", []string{"-logtostderr"}},
}

type sinksDoc struct {
	Header header `json:"header"`
	// Messages are the messages the levels scenario logs, by facade call.
	Messages map[string]string `json:"messages"`
	Fatal    string            `json:"fatal_message"`
	Std      string            `json:"std_message"`
	Runs     []runDoc          `json:"runs"`
}

type runDoc struct {
	Scenario string   `json:"scenario"`
	Args     []string `json:"args"`
	Exit     int      `json:"exit"`
	Stderr   []entry  `json:"stderr"`
	// Dir says where glog made its files: "log_dir", "temp" (os.TempDir) or "" for none.
	Dir   string    `json:"dir"`
	Files []fileDoc `json:"files"`
	// Links are the symlinks glog made in -log_link, to the files in Dir.
	Links []fileDoc `json:"links,omitempty"`
}

// entry is one line of output: a glog line, a standard logger line, a stack dump or the
// runtime's signal line.
type entry struct {
	Kind     string `json:"kind"`
	Severity string `json:"severity,omitempty"`
	FileLine string `json:"file_line,omitempty"`
	Message  string `json:"message,omitempty"`
}

type fileDoc struct {
	Name string `json:"name"`
	// Link is a symlink's target, with the directory as a placeholder.
	Link   string   `json:"link,omitempty"`
	Header []string `json:"header,omitempty"`
	Lines  []entry  `json:"lines,omitempty"`
}

func build(h header) sinksDoc {
	doc := sinksDoc{Header: h, Messages: map[string]string{}, Fatal: fatalMessage, Std: stdMessage}
	for _, m := range levelMessages {
		doc.Messages[m.call] = m.message
	}
	for _, r := range runs {
		doc.Runs = append(doc.Runs, run(r.scenario, r.args))
	}
	return doc
}

// run runs one child in fresh directories and records what it wrote.
func run(scenario string, args []string) runDoc {
	root, err := os.MkdirTemp("", "pbs-logger-sinks-")
	if err != nil {
		fail(err.Error())
	}
	defer os.RemoveAll(root)
	dirs := map[string]string{
		"temp":     filepath.Join(root, "tmp"),
		logDirArg:  filepath.Join(root, "logs"),
		linkDirArg: filepath.Join(root, "links"),
		missingArg: filepath.Join(root, "missing"),
	}
	for _, name := range []string{"temp", logDirArg, linkDirArg} {
		if err := os.Mkdir(dirs[name], 0o755); err != nil {
			fail(err.Error())
		}
	}
	childArgs := []string{}
	for _, arg := range args {
		for _, name := range []string{logDirArg, linkDirArg, missingArg} {
			arg = strings.ReplaceAll(arg, name, dirs[name])
		}
		childArgs = append(childArgs, arg)
	}
	exe, err := os.Executable()
	if err != nil {
		fail(err.Error())
	}
	var stderr bytes.Buffer
	cmd := &exec.Cmd{
		Path:   exe,
		Args:   append([]string{program}, childArgs...),
		Env:    append(os.Environ(), childEnv+"="+scenario, "TMPDIR="+dirs["temp"]),
		Stderr: &stderr,
	}
	doc := runDoc{Scenario: scenario, Args: args, Files: []fileDoc{}}
	err = cmd.Run()
	var exit *exec.ExitError
	switch {
	case err == nil:
	case errors.As(err, &exit):
		doc.Exit = exit.ExitCode()
	default:
		fail(err.Error())
	}
	doc.Stderr = parseLines(stderr.String())
	for _, where := range []struct{ name, dir string }{{"log_dir", dirs[logDirArg]}, {"temp", dirs["temp"]}} {
		files := listDir(where.dir, dirs)
		if len(files) == 0 {
			continue
		}
		if doc.Dir != "" {
			fail("glog wrote files in two directories")
		}
		doc.Dir, doc.Files = where.name, files
	}
	doc.Links = listDir(dirs[linkDirArg], dirs)
	if _, err := os.Lstat(dirs[missingArg]); err == nil {
		fail("glog made the missing -log_dir")
	}
	return doc
}

var (
	glogLine = regexp.MustCompile(`^([IWEF])\d{4} \d{2}:\d{2}:\d{2}\.\d{6} +\d+ ([^ ]+:\d+)\] (.*)$`)
	stdLine  = regexp.MustCompile(`^\d{4}/\d{2}/\d{2} \d{2}:\d{2}:\d{2} (.*)$`)
	fileName = regexp.MustCompile(`^prebid-server\.[^.]+\.[A-Za-z0-9_]+\.log\.(INFO|WARNING|ERROR|FATAL)\.\d{8}-\d{6}\.\d+$`)
	created  = regexp.MustCompile(`^Log file created at: \d{4}/\d{2}/\d{2} \d{2}:\d{2}:\d{2}$`)
	binary   = regexp.MustCompile(`^Binary: Built with gc (go\S+) for \S+/\S+$`)
)

// parseLines turns output into entries. A stack dump becomes one "stack" entry however many
// goroutines it lists: its frames, register lines and goroutine headers name addresses and paths
// of the machine.
func parseLines(text string) []entry {
	entries := []entry{}
	for _, line := range strings.Split(text, "\n") {
		var e entry
		if m := glogLine.FindStringSubmatch(line); m != nil {
			e = entry{Kind: "glog", Severity: m[1], FileLine: m[2], Message: m[3]}
		} else if m := stdLine.FindStringSubmatch(line); m != nil {
			e = entry{Kind: "std", Message: m[1]}
		} else if strings.HasPrefix(line, "SIGABRT: ") {
			e = entry{Kind: "signal", Message: line}
		} else if strings.HasPrefix(line, "goroutine ") {
			e = entry{Kind: "stack"}
		} else {
			continue
		}
		if e.Kind == "stack" && len(entries) > 0 && entries[len(entries)-1].Kind == "stack" {
			continue
		}
		entries = append(entries, e)
	}
	return entries
}

// listDir records what glog made in dir, sorted by normalized name.
func listDir(dir string, dirs map[string]string) []fileDoc {
	items, err := os.ReadDir(dir)
	if err != nil {
		fail(err.Error())
	}
	files := []fileDoc{}
	for _, item := range items {
		path := filepath.Join(dir, item.Name())
		f := fileDoc{Name: normalizeName(item.Name())}
		if item.Type()&os.ModeSymlink != 0 {
			target, err := os.Readlink(path)
			if err != nil {
				fail(err.Error())
			}
			for _, name := range []string{logDirArg, linkDirArg, "temp"} {
				if rest, ok := strings.CutPrefix(target, dirs[name]+string(filepath.Separator)); ok {
					target = strings.ToUpper(name) + "/" + rest
				}
			}
			f.Link = normalizeName(filepath.Base(target))
			if dir := filepath.Dir(target); dir != "." {
				f.Link = dir + "/" + f.Link
			}
		} else {
			data, err := os.ReadFile(path)
			if err != nil {
				fail(err.Error())
			}
			lines := strings.Split(string(data), "\n")
			if len(lines) < 5 {
				fail("a log file without glog's header")
			}
			for _, line := range lines[:5] {
				line = created.ReplaceAllString(line, "Log file created at: YYYY/MM/DD hh:mm:ss")
				line = binary.ReplaceAllString(line, "Binary: Built with gc $1 for GOOS/GOARCH")
				if strings.HasPrefix(line, "Running on machine: ") {
					line = "Running on machine: HOST"
				}
				f.Header = append(f.Header, line)
			}
			f.Lines = parseLines(strings.Join(lines[5:], "\n"))
		}
		files = append(files, f)
	}
	sort.Slice(files, func(i, j int) bool { return files[i].Name < files[j].Name })
	return files
}

func normalizeName(name string) string {
	return fileName.ReplaceAllString(name, "prebid-server.HOST.USER.log.$1.YYYYMMDD-HHMMSS.PID")
}
