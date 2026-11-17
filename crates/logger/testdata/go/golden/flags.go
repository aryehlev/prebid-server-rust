package main

import (
	"bytes"
	"errors"
	"flag"
	"fmt"
	"strings"
)

// glogFlags copies glog's flags from flag.CommandLine, where glog registers them, into a
// ContinueOnError set named like the server binary, sharing glog's flag values.
func glogFlags(out *bytes.Buffer) *flag.FlagSet {
	fs := flag.NewFlagSet("prebid-server", flag.ContinueOnError)
	fs.SetOutput(out)
	n := 0
	flag.CommandLine.VisitAll(func(f *flag.Flag) {
		fs.Var(f.Value, f.Name, f.Usage)
		n++
	})
	if n != 9 {
		check(fmt.Errorf("expected glog's 9 flags, found %d", n))
	}
	return fs
}

// flagValues lists name=String() for every flag in flag order.
func flagValues(fs *flag.FlagSet) string {
	var vals []string
	fs.VisitAll(func(f *flag.Flag) {
		vals = append(vals, f.Name+"="+f.Value.String())
	})
	return strings.Join(vals, "\n")
}

// resetFlags restores every glog flag to its default.
func resetFlags() {
	flag.CommandLine.VisitAll(func(f *flag.Flag) {
		check(f.Value.Set(f.DefValue))
	})
}

var flagCases = [][]string{
	{},
	{"-v", "1", "-logtostderr"},
	{"--stderrthreshold=WARNING", "-v=2"},
	{"--alsologtostderr"},
	{"-logtostderr=false", "-alsologtostderr=1", "-stderrthreshold", "info"},
	{"-stderrthreshold=fatal"},
	{"-stderrthreshold=3"},
	{"-stderrthreshold=256"},
	{"-stderrthreshold=259"},
	{"-stderrthreshold=-1"},
	{"-stderrthreshold=4"},
	{"-stderrthreshold=x"},
	{"-stderrthreshold="},
	{"-stderrthreshold=Warning"},
	{"-stderrthreshold=ınfo"},
	{"-v=x"},
	{"-v="},
	{"-v=-3"},
	{"-v=4294967297"},
	{"-v=99999999999999999999"},
	{"-v", "+5"},
	{"-v=0x10"},
	{"-v"},
	{"-v", "-logtostderr"},
	{"-vmodule=recordio=2,foo/bar/baz=1,gfs*=3"},
	{"-vmodule=a=1,,b=2,"},
	{"-vmodule=a"},
	{"-vmodule=a=b"},
	{"-vmodule==1"},
	{"-vmodule=a=1=2"},
	{"-vmodule=a=4294967296"},
	{"-vmodule="},
	{"-log_backtrace_at=recordio.go:234,sstable.go:456"},
	{"-log_backtrace_at=x:1"},
	{"-log_backtrace_at=x.go"},
	{"-log_backtrace_at=x.go:-1"},
	{"-log_backtrace_at=x.go:y"},
	{"-log_backtrace_at=a.go:1:2"},
	{"-log_backtrace_at=,a.go:3,"},
	{"-log_backtrace_at=a.go:+7"},
	{"-log_dir=/var/log/pbs", "-log_link", "/tmp/links"},
	{"-log_dir"},
	{"-log_dir="},
	{"-logbuflevel=-1"},
	{"-logbuflevel=0x7f"},
	{"-logbuflevel=1_0"},
	{"-logbuflevel=x"},
	{"-logbuflevel=9223372036854775808"},
	{"-logtostderr=x"},
	{"-logtostderr=T"},
	{"-logtostderr=FALSE"},
	{"-logtostderr="},
	{"-logtostderr", "true"},
	{"-logtostderr", "-logtostderr=x"},
	{"-alsologtostderr", "-alsologtostderr=2"},
	{"-logbuflevel=5", "-logbuflevel=x"},
	{"-logbuflevel=-9223372036854775809"},
	{"-logbuflevel", "0b101"},
	{"-v=7", "-v=x"},
	{"-stderrthreshold=0", "-stderrthreshold=9"},
	{"-vmodule=a=1", "-vmodule=b"},
	{"-log_backtrace_at=a.go:1", "-log_backtrace_at=b"},
	{"-h"},
	{"-help"},
	{"--help"},
	{"-help=1"},
	{"-nope"},
	{"-v=1", "-nope"},
	{"---v"},
	{"-=x"},
	{"-"},
	{"--"},
	{"--", "-v=1"},
	{"arg", "-v=1"},
	{"-v=1", "arg", "-logtostderr"},
	{"-v=1", "-v=2", "-logtostderr", "-logtostderr=0"},
}

// genFlags writes flags.txt: for each argument list, Parse's error, what it printed, the
// remaining arguments and every flag's value afterwards; and flagset.txt for FlagSet.Set.
func genFlags() {
	var out bytes.Buffer
	fs := glogFlags(&out)
	var t table
	for _, args := range flagCases {
		out.Reset()
		resetFlags()
		err := fs.Parse(args)
		if errors.Is(err, flag.ErrHelp) {
			err = errors.New("help")
		}
		t.row(strings.Join(args, "\n"), errText(err), out.String(), strings.Join(fs.Args(), "\n"), flagValues(fs))
	}
	t.write("flags.txt")

	var s table
	sets := [][2]string{
		{"v", "3"}, {"v", "x"}, {"logtostderr", "true"}, {"logtostderr", "yes"},
		{"stderrthreshold", "INFO"}, {"stderrthreshold", "7"}, {"logbuflevel", "x"},
		{"vmodule", "x"}, {"log_backtrace_at", "x"}, {"nope", "1"}, {"log_dir", "/x"},
	}
	for _, kv := range sets {
		resetFlags()
		err := fs.Set(kv[0], kv[1])
		s.row(kv[0], kv[1], errText(err), flagValues(fs))
	}
	resetFlags()
	s.write("flagset.txt")
}
