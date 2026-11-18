package main

import (
	"flag"
	"os"
	"os/user"
	"path/filepath"
	"sort"
	"strings"

	"github.com/golang/glog"
)

// glogHost and glogUser are glog's host and user name parts of log file names.
func glogHost() string {
	h, err := os.Hostname()
	if err != nil {
		return "unknownhost"
	}
	if i := strings.Index(h, "."); i >= 0 {
		h = h[:i]
	}
	return h
}

func glogUser() string {
	name := "unknownuser"
	if u, err := user.Current(); err == nil && u.Username != "" {
		name = u.Username
	}
	return strings.Map(func(r rune) rune {
		switch {
		case r >= 'a' && r <= 'z', r >= 'A' && r <= 'Z', r >= '0' && r <= '9':
			return r
		}
		return '_'
	}, name)
}

// normalizeName replaces the host, user, time and pid parts of a glog file name
// (program.host.user.log.SEVERITY.yyyymmdd-hhmmss.pid) with placeholders.
func normalizeName(name string) string {
	parts := strings.Split(name, ".")
	if len(parts) != 7 {
		return name
	}
	parts[1], parts[2], parts[5], parts[6] = "HOST", "USER", "TIME", "PID"
	return strings.Join(parts, ".")
}

// normalizeContent replaces the machine- and time-dependent parts of a glog log file: the
// header's creation time, host and binary, and each line's time, pid and file:line.
func normalizeContent(content, dir string) string {
	lines := strings.SplitAfter(content, "\n")
	for i, l := range lines {
		switch {
		case strings.HasPrefix(l, "Log file created at: "):
			lines[i] = "Log file created at: TIME\n"
		case strings.HasPrefix(l, "Running on machine: "):
			lines[i] = "Running on machine: HOST\n"
		case strings.HasPrefix(l, "Binary: "):
			lines[i] = "Binary: BINARY\n"
		case strings.HasPrefix(l, "Previous log: "), strings.HasPrefix(l, "Next log: "):
			k := strings.Index(l, ": ") + 2
			lines[i] = l[:k] + normalizePath(strings.TrimSuffix(l[k:], "\n"), dir) + "\n"
		case strings.HasPrefix(l, "Log line format: "):
		case len(l) > 30 && strings.Contains(l, "] "):
			lines[i] = l[:1] + " SITE" + l[strings.Index(l, "] "):]
		}
	}
	return strings.Join(lines, "")
}

// normalizePath rewrites a path under dir as DIR/<normalized name>.
func normalizePath(p, dir string) string {
	if rel, ok := strings.CutPrefix(p, dir+"/"); ok {
		return "DIR/" + normalizeName(rel)
	}
	if !strings.Contains(p, "/") {
		return normalizeName(p)
	}
	return p
}

// listDir renders a directory of glog files and links.
func listDir(t *table, kind, dir, logDir string) {
	entries, err := os.ReadDir(dir)
	check(err)
	var rows [][3]string
	for _, e := range entries {
		p := filepath.Join(dir, e.Name())
		info, err := os.Lstat(p)
		check(err)
		if info.Mode()&os.ModeSymlink != 0 {
			target, err := os.Readlink(p)
			check(err)
			rows = append(rows, [3]string{normalizeName(e.Name()), "link", normalizePath(target, logDir)})
			continue
		}
		b, err := os.ReadFile(p)
		check(err)
		rows = append(rows, [3]string{normalizeName(e.Name()), "file", normalizeContent(string(b), logDir)})
	}
	sort.Slice(rows, func(i, j int) bool { return rows[i][0] < rows[j][0] })
	for _, r := range rows {
		t.row(kind, r[0], r[1], r[2])
	}
}

// genFiles writes files.txt: the log files, links and contents glog writes to -log_dir.
func genFiles() {
	logDir, err := os.MkdirTemp("", "glog-dir")
	check(err)
	linkDir, err := os.MkdirTemp("", "glog-link")
	check(err)
	defer os.RemoveAll(logDir)
	defer os.RemoveAll(linkDir)
	logDir, err = filepath.EvalSymlinks(logDir)
	check(err)
	linkDir, err = filepath.EvalSymlinks(linkDir)
	check(err)
	check(flag.Set("log_dir", logDir))
	check(flag.Set("log_link", linkDir))
	check(flag.Set("stderrthreshold", "FATAL"))
	defer resetFlags()

	glog.InfoDepthf(0, "one")
	glog.WarningDepthf(0, "two %d", 2)
	glog.ErrorDepthf(0, "three\nlines")
	glog.InfoDepthf(0, "four\n")
	glog.Flush()

	var t table
	listDir(&t, "dir", logDir, logDir)
	listDir(&t, "link", linkDir, logDir)
	t.write("files.txt")
}
