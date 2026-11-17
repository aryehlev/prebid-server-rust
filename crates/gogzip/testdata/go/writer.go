package main

import (
	"bytes"
	"compress/flate"
	"compress/gzip"
	"encoding/hex"
	"fmt"
	"io"
	"strconv"
	"strings"
	"time"
)

// levels are the valid compression levels, in the order of flate_writer.txt's columns.
var levels = []int{-2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9}

// target is the underlying writer of a compressor. It records the bytes and the Write calls,
// and fails Write call number failAt (counting from 0) with io.ErrClosedPipe, and when sticky
// every later call too.
type target struct {
	buf    bytes.Buffer
	calls  int
	failAt int
	sticky bool
}

func (t *target) Write(b []byte) (int, error) {
	i := t.calls
	t.calls++
	if t.failAt >= 0 && (i == t.failAt || t.sticky && i > t.failAt) {
		return 0, io.ErrClosedPipe
	}
	t.buf.Write(b)
	return len(b), nil
}

func (t *target) result() string {
	return fmt.Sprintf("%d.%016x.%d", t.buf.Len(), fnv64(t.buf.Bytes()), t.calls)
}

// compressor is the API that flate.Writer and gzip.Writer share.
type compressor interface {
	Write([]byte) (int, error)
	Flush() error
	Close() error
	Reset(io.Writer)
}

// script runs a script, a comma-separated list of operations, on a compressor:
//
//	w:N      write the next N bytes of the input (w:* for the rest)
//	c:K      write the rest of the input in K-byte chunks (cf:K flushes after each); with
//	         c:K:N (cf:K:N), only the next N bytes
//	r:M:S    write the rest in chunks of 1 + next()%M bytes, splitmix64 seeded with S
//	         (rf:M:S flushes after each)
//	e        write an empty slice
//	f        flush
//	x        close
//	H        set the gzip header of the case
//	R        reset onto a new target and the next input (or the same one, from the start)
//	RF:N     reset onto a target whose Write call N fails, restarting the same input
//	RS:N     reset onto a target whose Write calls from N on fail, restarting the same input
//
// The result is TRACE|N|TARGETS|HEX: the trace of errors (i!error, *count when an operation
// returned several) and flushed lengths (i@len), the sum of the counts Write returned, each
// target's LEN.FNV.CALLS, and the bytes of a single target of at most 64 bytes.
type script struct {
	w       compressor
	inputs  [][]byte
	seg     int
	pos     int
	targets []*target
	trace   []string
	total   int
	header  func()
}

func (s *script) rest() []byte { return s.inputs[s.seg][s.pos:] }

func (s *script) write(p []byte) error {
	n, err := s.w.Write(p)
	s.total += n
	s.pos += len(p)
	return err
}

func (s *script) chunks(op []string) []error {
	var errs []error
	var g *splitmix64
	end := len(s.inputs[s.seg])
	if op[0] == "r" || op[0] == "rf" {
		g = &splitmix64{atou(op[2])}
	} else if len(op) > 2 {
		end = s.pos + atoi(op[2])
	}
	for s.pos < end {
		var n int
		if g != nil {
			n = 1 + int(g.next()%atou(op[1]))
		} else {
			n = atoi(op[1])
		}
		n = min(n, end-s.pos)
		errs = append(errs, s.write(s.rest()[:n]))
		if op[0] == "cf" || op[0] == "rf" {
			errs = append(errs, s.w.Flush())
		}
	}
	return errs
}

func (s *script) exec(i int, text string) {
	op := strings.Split(text, ":")
	var errs []error
	switch op[0] {
	case "w":
		n := len(s.rest())
		if op[1] != "*" {
			n = atoi(op[1])
		}
		errs = append(errs, s.write(s.rest()[:n]))
	case "c", "cf", "r", "rf":
		errs = s.chunks(op)
	case "e":
		errs = append(errs, s.write(nil))
	case "f":
		errs = append(errs, s.w.Flush())
	case "x":
		errs = append(errs, s.w.Close())
	case "H":
		s.header()
	case "R", "RF", "RS":
		t := &target{failAt: -1}
		if op[0] != "R" {
			t.failAt = atoi(op[1])
			t.sticky = op[0] == "RS"
		}
		s.targets = append(s.targets, t)
		s.w.Reset(t)
		if op[0] == "R" && s.seg+1 < len(s.inputs) {
			s.seg++
		}
		s.pos = 0
	default:
		panic("unknown script operation " + text)
	}
	var first error
	count := 0
	for _, err := range errs {
		if err != nil {
			if first == nil {
				first = err
			}
			count++
		}
	}
	if first != nil {
		entry := fmt.Sprintf("%d!%s", i, first)
		if count > 1 {
			entry += fmt.Sprintf("*%d", count)
		}
		s.trace = append(s.trace, entry)
	}
	if op[0] == "f" {
		s.trace = append(s.trace, fmt.Sprintf("%d@%d", i, s.targets[len(s.targets)-1].buf.Len()))
	}
}

func (s *script) run(text string) string {
	for i, op := range strings.Split(text, ",") {
		s.exec(i, op)
	}
	trace := "-"
	if len(s.trace) > 0 {
		trace = strings.Join(s.trace, ",")
	}
	var targets []string
	for _, t := range s.targets {
		targets = append(targets, t.result())
	}
	bytesHex := "-"
	if len(s.targets) == 1 && s.targets[0].buf.Len() <= 64 {
		bytesHex = hex.EncodeToString(s.targets[0].buf.Bytes())
	}
	return fmt.Sprintf("%s|%d|%s|%s", trace, s.total, strings.Join(targets, ";"), bytesHex)
}

// genInputs returns the inputs of a |-separated list of input specs.
func genInputs(specs string) [][]byte {
	var inputs [][]byte
	for _, spec := range strings.Split(specs, "|") {
		inputs = append(inputs, genInput(spec))
	}
	return inputs
}

func runFlate(level int, dict, inputs, text string) string {
	t := &target{failAt: -1}
	var w *flate.Writer
	var err error
	if dict == "-" {
		w, err = flate.NewWriter(t, level)
	} else {
		w, err = flate.NewWriterDict(t, level, genInput(dict))
	}
	if err != nil {
		panic(err)
	}
	s := &script{w: w, inputs: genInputs(inputs), targets: []*target{t}}
	return s.run(text)
}

// header is a gzip header spec: "-" or comma-separated name=HEX, comment=HEX, extra=HEX
// (extra=@SPEC for the bytes of an input spec), mtime=SEC.NSEC and os=N.
func parseHeader(spec string) gzip.Header {
	h := gzip.Header{OS: 255}
	if spec == "-" {
		return h
	}
	unhex := func(s string) []byte {
		b, err := hex.DecodeString(s)
		if err != nil {
			panic(err)
		}
		return b
	}
	for _, kv := range strings.Split(spec, ",") {
		k, v, _ := strings.Cut(kv, "=")
		switch k {
		case "name":
			h.Name = string(unhex(v))
		case "comment":
			h.Comment = string(unhex(v))
		case "extra":
			if strings.HasPrefix(v, "@") {
				h.Extra = genInput(v[1:])
			} else {
				h.Extra = append([]byte{}, unhex(v)...)
			}
		case "mtime":
			sec, nsec, _ := strings.Cut(v, ".")
			s, err := strconv.ParseInt(sec, 10, 64)
			if err != nil {
				panic(err)
			}
			h.ModTime = time.Unix(s, int64(atoi(nsec)))
		case "os":
			h.OS = byte(atoi(v))
		default:
			panic("unknown header field " + k)
		}
	}
	return h
}

func runGzip(level int, header, inputs, text string) string {
	t := &target{failAt: -1}
	w, err := gzip.NewWriterLevel(t, level)
	if err != nil {
		panic(err)
	}
	h := parseHeader(header)
	w.Header = h
	s := &script{w: w, inputs: genInputs(inputs), targets: []*target{t}, header: func() { w.Header = h }}
	return s.run(text)
}

func hexs(s string) string { return hex.EncodeToString([]byte(s)) }

// writeLevels records the level checks of flate.NewWriter and gzip.NewWriterLevel.
func writeLevels(dir string) {
	g := newGolden(dir, "levels.txt")
	defer g.close()
	for level := -5; level <= 12; level++ {
		_, ferr := flate.NewWriter(io.Discard, level)
		_, gerr := gzip.NewWriterLevel(io.Discard, level)
		res := func(err error) string {
			if err == nil {
				return "ok"
			}
			return "!" + err.Error()
		}
		g.line(strconv.Itoa(level), res(ferr), res(gerr))
	}
}

// flateCase is one line of flate_writer.txt: an input list and a script, run at every level.
type flateCase struct{ inputs, script string }

func flateWriterCases() []flateCase {
	var cases []flateCase
	add := func(inputs string, scripts ...string) {
		for _, s := range scripts {
			cases = append(cases, flateCase{inputs, s})
		}
	}
	// Go's TestDeflate vectors, TestWriterClose, and small edge cases.
	add("lit:", "w:*,x", "x", "e,x", "f,x", "f,f,x", "x,x", "e,f,e,x,x", "x,w:*,f,x")
	add("lit:11", "w:*,x", "f,w:*,x")
	add("lit:1112", "w:*,x")
	add("lit:1111111111111111", "w:*,x", "c:1,x")
	add("lit:5465737454657374", "w:4,x,w:4,f,x")
	add("lit:68656c6c6f2c20776f726c640a", "w:*,x", "c:1,x", "w:5,f,w:*,f,x")
	for _, n := range []int{1, 3, 4, 5, 15, 16, 17, 100, 127, 128, 129, 257, 258, 259, 260, 1000} {
		add(fmt.Sprintf("zeros:%d", n), "w:*,x", "c:1,x", "cf:7,x")
	}
	for _, n := range []int{32768, 65535, 65536, 65537, 140000} {
		add(fmt.Sprintf("zeros:%d", n), "w:*,x", "c:4096,x")
	}
	for _, n := range []int{1, 2, 3, 4, 5, 16, 17, 18, 64, 127, 128, 129, 300, 1000} {
		add(fmt.Sprintf("rand:%d:%d", n, n), "w:*,x", "c:1,x", "cf:5,x")
	}
	for _, n := range []int{4096, 32767, 32768, 32769, 65534, 65535, 65536, 65537, 131072} {
		add(fmt.Sprintf("rand:%d:%d", n, n), "w:*,x", "c:7919,x")
	}
	add("rand:70000:3", "cf:30000,x", "r:20000:5,x", "w:65535,f,w:*,x")
	for _, pn := range [][2]int{{1, 1000}, {2, 5000}, {3, 70000}, {4, 300}, {7, 70000}, {13, 66000},
		{64, 140000}, {255, 20000}, {258, 70000}, {259, 70000}, {1000, 140000}, {4096, 100000},
		{32767, 70000}, {32768, 70000}, {32769, 100000}} {
		add(fmt.Sprintf("repeat:%d:%d:%d", pn[0], pn[1], pn[0]), "w:*,x", "r:9000:1,x")
	}
	for _, kn := range [][2]int{{2, 1000}, {4, 70000}, {8, 100000}, {16, 70000}, {64, 70000}} {
		add(fmt.Sprintf("low:%d:%d:%d", kn[0], kn[1], kn[0]), "w:*,x", "c:787,x")
	}
	// Go's TestDeterministic, at a tenth of its length (testing.Short()).
	add("low:8:196655:1", "c:787,x", "c:81761,x")
	for _, n := range []int{100, 1000, 10000, 65536, 100000, 200000} {
		add(fmt.Sprintf("text:%d:%d", n, n), "w:*,x", "r:5000:2,x")
	}
	add("text:3000:7", "c:1,x", "cf:100,x", "rf:700:3,x")
	for _, n := range []int{500, 5000, 50000, 70000, 140000} {
		add(fmt.Sprintf("json:%d:%d", n, n), "w:*,x", "r:3000:4,x")
	}
	add("json:20000:9", "cf:1000,x", "w:10000,f,f,e,w:*,f,x,x")
	add("mixed:150000:1", "w:*,x", "r:40000:6,x")
	add("mixed:300000:2", "w:*,x")
	add("file:fixtures.json", "w:*,x", "c:4096,x", "r:30000:8,x")
	add("file:fixtures.json:65536", "w:*,x", "cf:16384,x")
	add("file:gettysburg.txt", "w:*,x", "c:1,x", "cf:100,x")
	add("square:100000", "w:*,x")
	// Go's TestBestSpeed: pieces of abcabc, with and without a flush after each.
	add("abc:65536+abc:1+abc:256", "w:65536,w:1,w:256,x", "w:65536,f,w:1,f,w:256,f,x")
	add("abc:1+abc:16+abc:65536", "w:1,w:16,w:65536,x", "w:1,f,w:16,f,w:65536,f,x")
	add("abc:65535+abc:128+abc:65536", "w:65535,w:128,w:65536,x", "w:65535,f,w:128,f,w:65536,f,x")
	add("abc:131072+abc:65536+abc:256", "w:131072,w:65536,w:256,x")
	add("abc:65537+abc:129", "w:65537,f,w:129,f,x")
	// Go's TestBestSpeedMaxMatchOffset: abcdefgh twice, about maxMatchOffset apart, with
	// stuvwxyz just before the second unless the match is before it.
	const abc, xyz = "lit:6162636465666768", "lit:737475767778797a"
	for _, extra := range []int{0, 14, 15, 16, 30} {
		for _, adj := range []int{-1, 0, 1} {
			offset := 32768 + adj
			add(fmt.Sprintf("%s+zeros:%d+%s+zeros:%d", abc, offset-8, abc, extra), "w:*,x")
			add(fmt.Sprintf("%s+zeros:%d+%s+%s+zeros:%d", abc, offset-16, xyz, abc, extra), "w:*,x")
		}
	}
	// Writer reuse: Reset, with and without Close first, onto other inputs.
	add("json:30000:1|text:30000:2|rand:500:3", "w:*,x,R,w:*,x,R,w:*,x")
	add("text:70000:4|text:70000:4", "w:40000,R,w:*,x")
	add("zeros:100000|json:1000:5", "w:*,f,R,w:*,x")
	// Failing underlying writers (TestWriteError, TestWriterPersistent*Error).
	for _, n := range []int{0, 1, 2, 3, 5, 8, 16, 64, 256} {
		add("text:20000:5+lit:010202030405|rand:6:6", fmt.Sprintf("RS:%d,c:128:20000,w:6,e,f,x,R,w:*,x", n))
	}
	for _, n := range []int{0, 1, 2, 4, 7, 50} {
		add("text:10000:6", fmt.Sprintf("RF:%d,w:*,x,f,w:*", n))
	}
	add("lit:54657374", "RF:0,f,x,w:*,R,x,x,f,w:*")
	add("lit:54657374", "RF:0,x,f,w:*")
	return cases
}

func writeFlateWriter(dir string) {
	g := newGolden(dir, "flate_writer.txt")
	defer g.close()
	for _, c := range flateWriterCases() {
		fields := []string{c.inputs, c.script}
		for _, level := range levels {
			fields = append(fields, runFlate(level, "-", c.inputs, c.script))
		}
		g.line(fields...)
	}
}

// writeFlateLarge records levels 2 to 9 over an input longer than maxHashOffset (16 MiB), so
// that the compressor rebases its hash chains while matches still reach across the rebase.
func writeFlateLarge(dir string) {
	g := newGolden(dir, "flate_large.txt")
	defer g.close()
	const input, text = "repeat:4000:17000000:5", "w:*,x"
	for level := 2; level <= 9; level++ {
		g.line(strconv.Itoa(level), input, text, runFlate(level, "-", input, text))
	}
}

func writeFlateDict(dir string) {
	g := newGolden(dir, "flate_dict.txt")
	defer g.close()
	cases := []struct{ dict, inputs, script string }{
		{"lit:", "text:1000:1", "w:*,x"},
		{"text:100:3", "text:5000:3", "w:*,x,R,w:*,x"},
		{"text:40000:2", "text:40000:2", "w:*,x"},
		{"json:70000:7", "json:20000:8|json:20000:9", "w:*,x,R,c:1000,x"},
		{"rand:3:1", "rand:3:1+rand:3:1", "w:*,x"},
		{"zeros:5000", "zeros:300", "w:*,f,x"},
	}
	for _, c := range cases {
		for _, level := range levels {
			g.line(strconv.Itoa(level), c.dict, c.inputs, c.script, runFlate(level, c.dict, c.inputs, c.script))
		}
	}
}

func writeGzipWriter(dir string) {
	g := newGolden(dir, "gzip_writer.txt")
	defer g.close()
	type gzCase struct{ header, inputs, script string }
	var cases []gzCase
	add := func(header, inputs string, scripts ...string) {
		for _, s := range scripts {
			cases = append(cases, gzCase{header, inputs, s})
		}
	}
	headers := []string{
		"name=" + hexs("name"),
		"comment=" + hexs("comment"),
		"extra=",
		"extra=" + hexs("extra"),
		"mtime=100000000.0",
		"mtime=0.0",
		"mtime=0.1",
		"mtime=-1.0",
		"mtime=4294967296.0",
		"mtime=4294967297.5",
		"mtime=1700000000.123456789",
		"os=0",
		"os=3",
		"name=" + hexs("Äußerung.txt") + ",comment=" + hexs("ÿ\u0080!"),
		"name=" + hexs("Ā"),
		"comment=" + hexs("a\x00b"),
		"name=" + hexs("ok") + ",comment=" + hexs("日本"),
		"extra=@zeros:65535",
		"extra=@zeros:65536",
		"name=" + hexs("name") + ",comment=" + hexs("comment") + ",extra=" + hexs("extra") + ",mtime=100000000.0,os=1",
	}
	for _, h := range headers {
		add(h, "lit:78", "w:*,x", "f,w:*,f,x")
	}
	add("-", "lit:", "x", "w:*,x", "e,x", "f,x", "x,x", "x,f,w:*,x")
	add("-", "lit:68656c6c6f20776f726c640a", "w:*,x", "w:*,x,w:*,f,x", "w:*,x,f,x", "c:1,f,x")
	add("name="+hexs("name")+",comment="+hexs("comment")+",extra="+hexs("extra")+",mtime=100000000.0",
		"lit:78", "f,w:*,f,x")
	add("-", "text:100000:1", "w:*,x", "r:5000:3,x", "cf:30000,x")
	add("-", "json:70000:2", "w:*,x")
	add("name="+hexs("a"), "text:1000:4|json:1000:5", "w:*,x,R,w:*,x", "w:*,x,R,H,w:*,x", "w:*,R,f,x")
	for _, n := range []int{0, 1, 2, 3, 4, 5, 6, 7} {
		add("name="+hexs("name")+",comment="+hexs("comment")+",extra="+hexs("extra"), "text:1000:6",
			fmt.Sprintf("RF:%d,H,w:*,x,f,w:*,x", n), fmt.Sprintf("RS:%d,H,w:*,x,f,x", n))
	}
	for _, c := range cases {
		for _, level := range levels {
			g.line(strconv.Itoa(level), c.header, c.inputs, c.script, runGzip(level, c.header, c.inputs, c.script))
		}
	}
}
