package main

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"

	cglog "github.com/chasex/glog"
)

var (
	dateRE   = regexp.MustCompile(`\d{4}/\d{2}/\d{2}`)
	microRE  = regexp.MustCompile(`\d{2}:\d{2}:\d{2}\.\d{6}`)
	clockRE  = regexp.MustCompile(`\d{2}:\d{2}:\d{2}`)
	siteRE   = regexp.MustCompile(`[A-Za-z0-9_]+\.go:\d+ `)
	sizeRE   = regexp.MustCompile(`-\d{8}-\d{6}$`)
	hourRE   = regexp.MustCompile(`-\d{8}-\d{2}$`)
	dayRE    = regexp.MustCompile(`-\d{8}$`)
	dayInRE  = regexp.MustCompile(`-\d{8}:`)
	modeName = map[cglog.RotateMode]string{cglog.R_None: "none", cglog.R_Size: "size", cglog.R_Hour: "hour", cglog.R_Day: "day"}
)

// normalizeChasexName replaces the time suffix of a chasex log file name.
func normalizeChasexName(name string) string {
	name = sizeRE.ReplaceAllString(name, "-YYYYMMDD-HHMMSS")
	name = hourRE.ReplaceAllString(name, "-YYYYMMDD-HH")
	return dayRE.ReplaceAllString(name, "-YYYYMMDD")
}

// normalizeChasexContent replaces dates, times and call sites in chasex log lines.
func normalizeChasexContent(s string) string {
	s = dateRE.ReplaceAllString(s, "DATE")
	s = microRE.ReplaceAllString(s, "TIME.MICRO")
	s = clockRE.ReplaceAllString(s, "TIME")
	return siteRE.ReplaceAllString(s, "SITE ")
}

type chasexCase struct {
	name  string
	flag  int
	level cglog.Level
	mode  cglog.RotateMode
}

// writeChasex makes the calls every case replays, in order.
func writeChasex(l *cglog.Logger) {
	l.Debug("hello world")
	l.Debugf("hello, %s", "chasex")
	l.Debugln("a", 1, 2, "b")
	l.Debug("a", 1, 2, "b")
	l.Info("info message")
	l.Infof("%d%%", 50)
	l.Infoln()
	l.Warn("warn")
	l.Warnf("")
	l.Warnln("x")
	l.Error("err\n")
	l.Errorf("multi\nline")
	l.Errorln(3.5)
	l.Debug("{\"Request\":{\"id\":\"some-id\"},\"Status\":200}")
}

// genChasex writes chasex.txt: the files and contents chasex/glog loggers write.
func genChasex() {
	cases := []chasexCase{
		{"prebid", cglog.LstdFlags, cglog.Ldebug, cglog.R_Day},
		{"warn level", cglog.LstdFlags, cglog.Lwarn, cglog.R_None},
		{"micro", cglog.Ldate | cglog.Lmicroseconds, cglog.Ldebug, cglog.R_Hour},
		{"level only", cglog.Llevel, cglog.Ldebug, cglog.R_Size},
		{"time info", cglog.Ltime | cglog.Llevel, cglog.Linfo, cglog.R_None},
		{"short file", cglog.Lshortfile | cglog.Llevel, cglog.Lerror, cglog.R_None},
		{"utc", cglog.LstdFlags | cglog.LUTC, cglog.Lfatal, cglog.R_Day},
		{"null", cglog.LstdNull, cglog.Ldebug, cglog.R_None},
	}
	var t table
	for _, c := range cases {
		dir, err := os.MkdirTemp("", "chasex")
		check(err)
		l, err := cglog.New(cglog.LogOptions{
			File:    filepath.Join(dir, "x.log"),
			Flag:    c.flag,
			Level:   c.level,
			Mode:    c.mode,
			Maxsize: 1 << 20,
		})
		check(err)
		writeChasex(l)
		l.Flush()
		entries, err := os.ReadDir(dir)
		check(err)
		var names []string
		for _, e := range entries {
			names = append(names, e.Name())
		}
		sort.Strings(names)
		for _, n := range names {
			b, err := os.ReadFile(filepath.Join(dir, n))
			check(err)
			t.row(c.name, fmt.Sprint(c.flag), fmt.Sprint(int(c.level)), modeName[c.mode], normalizeChasexName(n), normalizeChasexContent(string(b)))
		}
		check(os.RemoveAll(dir))
	}

	dir, err := os.MkdirTemp("", "chasex")
	check(err)
	defer os.RemoveAll(dir)
	dir, err = filepath.EvalSymlinks(dir)
	check(err)
	var e table
	for _, c := range []struct {
		file string
		mode cglog.RotateMode
	}{
		{filepath.Join(dir, "missing", "x.log"), cglog.R_Day},
		{filepath.Join(dir, "missing", "x.log"), cglog.R_None},
		{dir, cglog.R_None},
	} {
		_, err := cglog.New(cglog.LogOptions{File: c.file, Flag: cglog.LstdFlags, Level: cglog.Ldebug, Mode: c.mode})
		text := strings.ReplaceAll(errText(err), dir, "DIR")
		text = dayInRE.ReplaceAllString(text, "-YYYYMMDD:")
		e.row(strings.ReplaceAll(c.file, dir, "DIR"), modeName[c.mode], text)
	}
	t.write("chasex.txt")
	e.write("chasex_errors.txt")
}
