package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"reflect"
	"strconv"
	"strings"
)

type bidderName string

type stringer struct{ id int }

func (s stringer) String() string { return "stringer#" + strconv.Itoa(s.id) }

type imp struct {
	ID       string
	BidFloor float64
	Tags     []string
	Ext      map[string]int
	Ptr      *int
	hidden   bool
}

type holder struct {
	Name string
	Err  error
	err  error
	Any  any
	Nil  any
	S    stringer
	s    stringer
}

type fmtValue struct {
	name string
	v    any
}

// fmtCatalog returns the operands the Rust tests model, by name, and the pointers among them
// whose addresses Go prints.
func fmtCatalog() (values, pointers []fmtValue) {
	n := 42
	pi := &n
	errA := errors.New("boom")
	errB := fmt.Errorf("wrapped: %w", errA)
	im := imp{ID: "imp-1", BidFloor: 0.5, Tags: []string{"a", "b c"}, Ext: map[string]int{"z": 26, "a": 1}, Ptr: pi, hidden: true}
	pim := &im
	var nilImp *imp
	h := holder{Name: "h", Err: errA, err: errA, Any: 3, Nil: nil, S: stringer{1}, s: stringer{2}}
	ph := &h
	ps := &stringer{8}
	pstrs := &[]string{"x"}
	pmap := &map[string]int{"k": 1}
	pointers = []fmtValue{
		{"pint", pi}, {"pimp", pim}, {"pholder", ph}, {"pstringer", ps},
		{"err", errA}, {"errw", errB}, {"pstrs", pstrs}, {"pmap", pmap},
	}
	values = []fmtValue{
		{"nil", nil}, {"true", true}, {"false", false},
		{"int0", 0}, {"int", -42}, {"i2", 2}, {"i6", 6}, {"intbig", 10000000},
		{"int8", int8(-128)}, {"int16", int16(300)}, {"int32", int32(0x1F600)},
		{"int64", int64(math.MinInt64)}, {"uint", uint(7)}, {"uint8", uint8(200)},
		{"uint16", uint16(65535)}, {"uint32", uint32(0xD800)}, {"uint64", uint64(math.MaxUint64)},
		{"uintptr", uintptr(0xbeef)},
		{"f0", 0.0}, {"fneg0", math.Copysign(0, -1)}, {"f", 1.5}, {"fbig", 1e21}, {"fsmall", 1e-7},
		{"fpi", 3.14159265}, {"f123", 123456789.0}, {"fnan", math.NaN()}, {"finf", math.Inf(1)},
		{"fninf", math.Inf(-1)}, {"f32", float32(0.1)}, {"fround", 2.675},
		{"s0", ""}, {"s", "hello"}, {"sutf", "h\u00e9llo w\u00f6rld"}, {"sesc", "tab\tq\"\\"},
		{"sback", "`back`"}, {"sctl", "\x00\x7f"}, {"scjk", "\u65e5\u672c\u8a9e"},
		{"sbom", "\ufeffbom"}, {"snum", "12"},
		{"bytes", []byte("hi\xff")}, {"bnil", []byte(nil)}, {"bempty", []byte{}},
		{"raw", json.RawMessage(`{"a":1}`)}, {"name", bidderName("appnexus")},
		{"strs", []string{"a", "b c"}}, {"strsnil", []string(nil)}, {"ints", []int{1, -2, 3}},
		{"arr", [2]bool{true, false}}, {"anys", []any{1, "x", nil, errA, 2.5}},
		{"u8s", [3]uint8{1, 2, 255}}, {"errs", []error{errA, errB}},
		{"map", map[string]int{"b": 2, "a": 1, "c": 3}}, {"mapnil", map[string]int(nil)},
		{"mapint", map[int]string{3: "c", -1: "z", 0: ""}},
		{"mapf", map[float64]bool{math.NaN(): true, 1: false, -1: true}},
		{"mapany", map[string]any{"k": []int{1}, "n": nil}}, {"mapbool", map[bool]int{true: 1, false: 0}},
		{"imp", im}, {"pimp", pim}, {"nilimp", nilImp}, {"pint", pi}, {"holder", h}, {"pholder", ph},
		{"stringer", stringer{7}}, {"pstringer", ps}, {"err", errA}, {"errw", errB},
		{"pstrs", pstrs}, {"pmap", pmap},
	}
	return values, pointers
}

// fmtFormats are applied to every catalog value.
var fmtFormats = []string{
	"%v", "%+v", "%#v", "%s", "%d", "%q", "%x", "%X", "%t", "%f", "%.2f", "%g", "%e", "%T",
	"%5v|", "%-6s|", "%05d", "%.3s", "%.1q", "%c", "%U", "%#U", "%#x", "% x", "%# X", "%+d",
	"% d", "%o", "%O", "%b", "%8.3f", "%-8.2f|", "%+.3e", "%#q", "%+q", "%#g", "%G", "%010.4f",
	"%-05d|", "%.0d", "%6.2v", "%w", "%F", "%#o", "%#b", "%E", "%+v|%-4d", "%x %[1]X",
}

// fmtCalls are curated calls with several (or no) operands.
var fmtCalls = []struct{ kind, format, args string }{
	{"sprintf", "%d %d", "int"},
	{"sprintf", "%d", "int,s,nil"},
	{"sprintf", "%[2]d %[1]d", "int,uint"},
	{"sprintf", "%[3]d", "int,uint"},
	{"sprintf", "%[0]d", "int"},
	{"sprintf", "%[x]d", "int"},
	{"sprintf", "%[1]", "int"},
	{"sprintf", "%[", "int"},
	{"sprintf", "%[1", "int"},
	{"sprintf", "%[-1]d", "int"},
	{"sprintf", "%[99999999999]d", "int"},
	{"sprintf", "%[2]*[1]d|", "int,uint"},
	{"sprintf", "%*d|", "uint,int"},
	{"sprintf", "%-*d|", "uint,int"},
	{"sprintf", "%*d|", "int,uint"},
	{"sprintf", "%.*f", "uint,fpi"},
	{"sprintf", "%*d", "s,int"},
	{"sprintf", "%.*d", "int,uint"},
	{"sprintf", "%.*d", "s,uint"},
	{"sprintf", "%*d", "intbig,int"},
	{"sprintf", "%*d", "uint64,int"},
	{"sprintf", "%[3]*.[2]*[1]f", "fpi,i2,i6"},
	{"sprintf", "%[2]2d", "int,uint"},
	{"sprintf", "%[2].2d", "int,uint"},
	{"sprintf", "%[1]d %[1]q %v", "int32"},
	{"sprintf", "%[1]d %d", "int,uint"},
	{"sprintf", "%", ""},
	{"sprintf", "%-", ""},
	{"sprintf", "%5", "int"},
	{"sprintf", "%.", "int"},
	{"sprintf", "%.5", "int"},
	{"sprintf", "%3.", "int"},
	{"sprintf", "100%%", ""},
	{"sprintf", "%%d", "int"},
	{"sprintf", "%5%|%-5%|", ""},
	{"sprintf", "%!", "int"},
	{"sprintf", "%z", "s"},
	{"sprintf", "%\u00e9", "int"},
	{"sprintf", "%v %v %v", "s,int"},
	{"sprintf", "%s", ""},
	{"sprintf", "%v", "nil"},
	{"sprintf", "%d", "nil"},
	{"sprintf", "%s %d", "nil,nil"},
	{"sprintf", "%T %T", "nil,anys"},
	{"sprintf", "%-10T|", "pimp"},
	{"sprintf", "%.3T", "pimp"},
	{"sprintf", "%5.1s|", "scjk"},
	{"sprintf", "%08.3f|%-08.3f|%+08.3f", "fneg0,fpi,fpi"},
	{"sprintf", "%p", "pint"},
	{"sprintf", "%p", "nilimp"},
	{"sprintf", "%#p", "pimp"},
	{"sprintf", "%p", "err"},
	{"sprintf", "%p", "int"},
	{"sprintf", "%p", "nil"},
	{"sprintf", "%v %d %x %#v", "pint,pint,pint,pint"},
	{"sprintf", "%s", "pint"},
	{"sprintf", "no verbs", "int"},
	{"sprintf", "%v|%s|%d", "pstringer,pstringer,pstringer"},
	{"sprintf", "%v|%d|%s", "errs,errs,errs"},
	{"errorf", "%w", "err"},
	{"errorf", "x: %w", "errw"},
	{"errorf", "%w %w", "err,errw"},
	{"errorf", "%w", "s"},
	{"errorf", "%w", "nil"},
	{"errorf", "%v %w", "int,err"},
	{"errorf", "%#w", "err"},
	{"errorf", "%+w|%10w|%-10w|", "err,err,err"},
	{"errorf", "%[1]w %[1]v", "err"},
	{"errorf", "no verbs", ""},
	{"errorf", "%d%%", "int"},
	{"errorf", "%s", "pstringer"},
	{"errorf", "%x", "err"},
	{"errorf", "%w", "stringer"},
	{"errorf", "%w", "anys"},
	{"sprint", "", "int,uint"},
	{"sprint", "", "s,int,s"},
	{"sprint", "", "int,s,int"},
	{"sprint", "", "nil,nil"},
	{"sprint", "", "name,int"},
	{"sprint", "", "stringer,int"},
	{"sprint", "", "err,err"},
	{"sprint", "", ""},
	{"sprint", "", "bytes,int"},
	{"sprint", "", "s,s"},
	{"sprintln", "", "int,uint"},
	{"sprintln", "", "s,s"},
	{"sprintln", "", ""},
	{"sprintln", "", "nil,map"},
}

var fmtFlagChars = "#0+- "

var fmtVerbs = []string{
	"v", "d", "s", "q", "x", "X", "t", "f", "F", "e", "E", "g", "G", "c", "U", "b", "o", "O",
	"T", "%", "w", "z", "\u00e9",
}

// randomFormat builds a format of one to three directives from the shared generator.
func randomFormat(rng *splitmix64) string {
	var b strings.Builder
	n := 1 + int(rng.next()%3)
	for k := 0; k < n; k++ {
		if rng.next()%4 == 0 {
			b.WriteString("a")
		}
		b.WriteByte('%')
		for f := rng.next() % 3; f > 0; f-- {
			b.WriteByte(fmtFlagChars[rng.next()%uint64(len(fmtFlagChars))])
		}
		if rng.next()%6 == 0 {
			b.WriteString("[" + strconv.Itoa(int(rng.next()%4)) + "]")
		}
		switch rng.next() % 4 {
		case 1:
			b.WriteString(strconv.Itoa(int(rng.next() % 12)))
		case 2:
			b.WriteByte('*')
		}
		switch rng.next() % 4 {
		case 1:
			b.WriteString("." + strconv.Itoa(int(rng.next()%8)))
		case 2:
			b.WriteString(".")
		case 3:
			b.WriteString(".*")
		}
		b.WriteString(fmtVerbs[rng.next()%uint64(len(fmtVerbs))])
	}
	return b.String()
}

func fmtArgs(byName map[string]any, names string) []any {
	if names == "" {
		return nil
	}
	var args []any
	for _, name := range strings.Split(names, ",") {
		v, ok := byName[name]
		if !ok {
			check(errors.New("unknown fmt operand " + name))
		}
		args = append(args, v)
	}
	return args
}

func genFmt() {
	values, pointers := fmtCatalog()
	byName := map[string]any{}
	var catalog table
	for _, p := range pointers {
		catalog.row("addr", p.name, strconv.FormatUint(uint64(reflect.ValueOf(p.v).Pointer()), 10))
	}
	for _, v := range values {
		catalog.row("value", v.name)
		byName[v.name] = v.v
	}
	catalog.write("fmt_catalog.txt")

	var verbs table
	for _, f := range fmtFormats {
		for _, v := range values {
			verbs.row(f, v.name, fmt.Sprintf(f, v.v))
		}
	}
	verbs.write("fmt_verbs.txt")

	var calls table
	for _, c := range fmtCalls {
		args := fmtArgs(byName, c.args)
		var out string
		switch c.kind {
		case "sprintf":
			out = fmt.Sprintf(c.format, args...)
		case "errorf":
			out = fmt.Errorf(c.format, args...).Error()
		case "sprint":
			out = fmt.Sprint(args...)
		case "sprintln":
			out = fmt.Sprintln(args...)
		}
		calls.row(c.kind, c.format, c.args, out)
	}
	calls.write("fmt_calls.txt")

	sw := newSweep()
	rng := splitmix64{state: 4}
	for i := 0; i < 50000; i++ {
		f := randomFormat(&rng)
		args := make([]any, rng.next()%4)
		for j := range args {
			args[j] = values[rng.next()%uint64(len(values))].v
		}
		sw.add(f + "\t" + fmt.Sprintf(f, args...))
	}
	var sums table
	sums.row("fmt_random", sw.sum())
	sums.write("fmt_sweeps.txt")
}
