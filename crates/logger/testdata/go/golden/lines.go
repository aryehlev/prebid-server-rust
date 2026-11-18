package main

import (
	"bytes"
	"flag"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/golang/glog"
)

// lineCase is one glog call: a severity, the function family (depthf for XDepthf(1, ...), as
// Prebid's logger package calls glog; print for XDepth(1, ...); ln for Xln) and its arguments.
type lineCase struct {
	name, sev, fn, format string
	args                  []any
}

// sentinel separates the output of consecutive calls in the captured stderr.
const sentinel = "\x00END\x00"

func callGlog(c lineCase) {
	switch c.fn + "/" + c.sev {
	case "depthf/INFO":
		glog.InfoDepthf(1, c.format, c.args...)
	case "depthf/WARNING":
		glog.WarningDepthf(1, c.format, c.args...)
	case "depthf/ERROR":
		glog.ErrorDepthf(1, c.format, c.args...)
	case "print/INFO":
		glog.InfoDepth(1, c.args...)
	case "print/WARNING":
		glog.WarningDepth(1, c.args...)
	case "ln/INFO":
		glog.Infoln(c.args...)
	case "ln/ERROR":
		glog.Errorln(c.args...)
	default:
		check(fmt.Errorf("unknown call %s/%s", c.fn, c.sev))
	}
}

// encodeArg renders an argument for the Rust test: s:<string>, d:<int>, f:<float>,
// b:<bool> or nil.
func encodeArg(a any) string {
	switch v := a.(type) {
	case string:
		return "s:" + v
	case int:
		return fmt.Sprintf("d:%d", v)
	case float64:
		return fmt.Sprintf("f:%v", v)
	case bool:
		return fmt.Sprintf("b:%v", v)
	case nil:
		return "nil"
	}
	check(fmt.Errorf("unsupported argument %T", a))
	return ""
}

// genLines writes lines.txt: glog's exact stderr output for each call under -logtostderr.
func genLines() {
	check(flag.Set("logtostderr", "true"))
	defer resetFlags()

	// The header of an empty message from callGlog sizes the boundary cases below.
	headerLen := len(captureGlog([]lineCase{{"probe", "INFO", "depthf", "", nil}})[0]) - 1

	exact := strings.Repeat("a", 14999-headerLen-1) + "\n"
	cut := strings.Repeat("b", 14999-headerLen-1) + "\nbc"
	cases := []lineCase{
		{"plain", "INFO", "depthf", "hello", nil},
		{"verbs", "INFO", "depthf", "%s: %d bids at %.2f", []any{"imp-1", 3, 0.5}},
		{"missing", "WARNING", "depthf", "missing %s %d", []any{"x"}},
		{"extra", "ERROR", "depthf", "extra", []any{1}},
		{"empty", "INFO", "depthf", "", nil},
		{"trailing newline", "INFO", "depthf", "ends with newline\n", nil},
		{"two lines", "WARNING", "depthf", "two\nlines", nil},
		{"two newlines", "ERROR", "depthf", "two newlines\n\n", nil},
		{"long", "INFO", "depthf", strings.Repeat("a", 20000), nil},
		{"exact limit", "INFO", "depthf", exact, nil},
		{"cut at newline", "INFO", "depthf", cut, nil},
		{"cut after format", "INFO", "depthf", "%s", []any{strings.Repeat("c", 15000)}},
		{"nil", "ERROR", "depthf", "%v", []any{nil}},
		{"quoted", "INFO", "depthf", "%q", []any{"quo\"te"}},
		{"unicode", "INFO", "depthf", "tab\tand unicode é %d%%", []any{50}},
		{"bool", "WARNING", "depthf", "%v %t", []any{true, false}},
		{"error args", "ERROR", "depthf", "Error: %s, %d", []any{"test", 999}},
		{"println", "INFO", "ln", "", []any{"a", 1, 2, "b"}},
		{"println empty", "INFO", "ln", "", nil},
		{"println error", "ERROR", "ln", "", []any{"rule", "failed", 2.5}},
		{"print mixed", "INFO", "print", "", []any{"a", 1, 2, "b"}},
		{"print none", "INFO", "print", "", nil},
		{"print one", "WARNING", "print", "", []any{7}},
		{"print numbers", "WARNING", "print", "", []any{3, 4}},
		{"print strings", "INFO", "print", "", []any{"x", "y"}},
	}
	outs := captureGlog(cases)
	var t table
	pid := fmt.Sprint(os.Getpid())
	for i, c := range cases {
		fields := []string{c.name, c.sev, c.fn, c.format, pid, outs[i]}
		for _, a := range c.args {
			fields = append(fields, encodeArg(a))
		}
		t.row(fields...)
	}
	t.write("lines.txt")
}

// captureGlog runs the calls with os.Stderr redirected and returns each call's output.
func captureGlog(cases []lineCase) []string {
	r, w, err := os.Pipe()
	check(err)
	saved := os.Stderr
	os.Stderr = w
	done := make(chan []byte)
	go func() {
		b, err := io.ReadAll(r)
		check(err)
		done <- b
	}()
	for _, c := range cases {
		callGlog(c)
		_, err := w.WriteString(sentinel)
		check(err)
	}
	os.Stderr = saved
	check(w.Close())
	all := <-done
	parts := bytes.Split(all, []byte(sentinel))
	if len(parts) != len(cases)+1 {
		check(fmt.Errorf("captured %d outputs for %d calls", len(parts)-1, len(cases)))
	}
	outs := make([]string, len(cases))
	for i := range cases {
		outs[i] = string(parts[i])
	}
	return outs
}
