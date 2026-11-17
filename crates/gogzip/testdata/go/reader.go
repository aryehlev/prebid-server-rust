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
)

// readTrace reads r with a bufsize buffer until three reads have returned an error (or, for
// an empty buffer, for three reads) and returns TRACE|LEN.FNV: the reads as N or N!error,
// with runs of the same entry as entry*count, and the bytes read.
func readTrace(r io.Reader, bufsize int) string {
	buf := make([]byte, bufsize)
	var out []byte
	var entries []string
	errs := 0
	for calls := 0; calls < 200000; calls++ {
		n, err := r.Read(buf)
		out = append(out, buf[:n]...)
		entry := strconv.Itoa(n)
		if err != nil {
			entry += "!" + err.Error()
			errs++
		}
		entries = append(entries, entry)
		if errs == 3 || bufsize == 0 && calls == 2 {
			break
		}
	}
	return fmt.Sprintf("%s|%d.%016x", rle(entries), len(out), fnv64(out))
}

// rle joins entries with commas, writing a run of equal entries as entry*count.
func rle(entries []string) string {
	var parts []string
	for i := 0; i < len(entries); {
		j := i
		for j < len(entries) && entries[j] == entries[i] {
			j++
		}
		if j-i > 1 {
			parts = append(parts, fmt.Sprintf("%s*%d", entries[i], j-i))
		} else {
			parts = append(parts, entries[i])
		}
		i = j
	}
	return strings.Join(parts, ",")
}

func errResult(err error) string {
	if err == nil {
		return "ok"
	}
	return "!" + err.Error()
}

// mutate applies a mutation to a stream: "-", t:N (keep the first N bytes), x:POS:VAL (xor
// the byte at POS with VAL), a:HEX (append the bytes) or m:SEED:K (K edits at positions
// splitmix64 seeded with SEED picks: flip a bit, set a byte, truncate, insert a byte or
// delete one).
func mutate(stream []byte, m string) []byte {
	f := strings.Split(m, ":")
	b := append([]byte{}, stream...)
	switch f[0] {
	case "-":
	case "t":
		b = b[:atoi(f[1])]
	case "x":
		b[atoi(f[1])] ^= byte(atoi(f[2]))
	case "m":
		g := &splitmix64{atou(f[1])}
		for i := 0; i < atoi(f[2]) && len(b) > 0; i++ {
			pos := int(g.next() % uint64(len(b)))
			switch g.next() % 5 {
			case 0:
				b[pos] ^= 1 << (g.next() % 8)
			case 1:
				b[pos] = byte(g.next())
			case 2:
				b = b[:pos]
			case 3:
				b = append(b[:pos], append([]byte{byte(g.next())}, b[pos:]...)...)
			default:
				b = append(b[:pos], b[pos+1:]...)
			}
		}
	case "a":
		tail, err := hex.DecodeString(f[1])
		if err != nil {
			panic(err)
		}
		b = append(b, tail...)
	default:
		panic("unknown mutation " + m)
	}
	return b
}

// flateStream compresses inputs (an input spec) with a script at level, with dict unless it
// is "-". Level "raw" means the input spec is the stream itself.
func flateStream(level, dict, input, text string) []byte {
	if level == "raw" {
		return genInput(input)
	}
	t := &target{failAt: -1}
	var w *flate.Writer
	var err error
	if dict == "-" {
		w, err = flate.NewWriter(t, atoi(level))
	} else {
		w, err = flate.NewWriterDict(t, atoi(level), genInput(dict))
	}
	if err != nil {
		panic(err)
	}
	s := &script{w: w, inputs: [][]byte{genInput(input)}, targets: []*target{t}}
	s.run(text)
	return t.buf.Bytes()
}

// flateReaderCase is one line of flate_reader.txt.
type flateReaderCase struct {
	name, level, dict, input, script, mutation string
	bufsize                                    int
	reset                                      bool
}

// runFlateReader returns STREAM|TRACE|LEN.FNV|CLOSE|REMAINING, with ||TRACE|LEN.FNV|CLOSE|REMAINING
// for a second read after Reset: the stream read (LEN.FNV), the reads, the bytes read, the
// result of Close and the count of stream bytes the reader left unread.
func runFlateReader(c flateReaderCase) string {
	stream := mutate(flateStream(c.level, c.dict, c.input, c.script), c.mutation)
	var dict []byte
	if c.dict != "-" {
		dict = genInput(c.dict)
	}
	br := bytes.NewReader(stream)
	r := flate.NewReaderDict(br, dict)
	res := fmt.Sprintf("%d.%016x|", len(stream), fnv64(stream))
	res += readTrace(r, c.bufsize) + "|" + errResult(r.Close()) + "|" + strconv.Itoa(br.Len())
	if c.reset {
		br = bytes.NewReader(stream)
		if err := r.(flate.Resetter).Reset(br, dict); err != nil {
			panic(err)
		}
		res += "||" + readTrace(r, c.bufsize) + "|" + errResult(r.Close()) + "|" + strconv.Itoa(br.Len())
	}
	return res
}

func flateReaderCases(goroot string) []flateReaderCase {
	var cases []flateReaderCase
	add := func(c flateReaderCase) { cases = append(cases, c) }
	for _, input := range []string{"lit:", "lit:68656c6c6f2c20776f726c640a", "zeros:100000", "rand:70000:1",
		"text:100000:2", "json:70000:3", "file:fixtures.json", "mixed:150000:4"} {
		for _, level := range []string{"-2", "0", "1", "6", "9"} {
			for _, bufsize := range []int{1, 100, 4096, 32768, 70000} {
				if bufsize == 1 && len(genInput(input)) > 20000 {
					continue
				}
				add(flateReaderCase{"roundtrip", level, "-", input, "w:*,x", "-", bufsize, bufsize == 4096})
			}
		}
	}
	for _, s := range []string{"cf:1000,x", "f,w:*,f,f,x", "r:20000:3,x", "w:*,f"} {
		add(flateReaderCase{"flushes", "6", "-", "json:50000:5", s, "-", 512, false})
		add(flateReaderCase{"flushes", "0", "-", "json:50000:5", s, "-", 512, false})
		add(flateReaderCase{"flushes", "1", "-", "json:50000:5", s, "-", 512, false})
	}
	add(flateReaderCase{"buffer0", "6", "-", "text:1000:1", "w:*,x", "-", 0, false})
	// Go's TestReaderEarlyEOF: whether the last bytes come with io.EOF.
	for _, n := range []int{1, 2, 3, 4, 5, 6, 7, 8, 100, 1000, 10000, 100000, 128, 1024, 16384, 131072,
		32768, 65536, 98304} {
		for _, s := range []string{"w:*,f,x", "w:*,x"} {
			add(flateReaderCase{"earlyeof", "5", "-", fmt.Sprintf("seq:%d", n), s, "-", 40, false})
		}
	}
	for _, level := range []string{"2", "6", "9"} {
		add(flateReaderCase{"dict", level, "text:30000:1", "text:20000:1", "w:*,x", "-", 4096, true})
	}
	// Truncations and single byte corruptions of small and large streams.
	for _, src := range []struct{ level, input string }{
		{"6", "lit:68656c6c6f2c20776f726c640a"}, {"0", "text:300:3"}, {"1", "text:3000:4"},
		{"6", "text:3000:4"}, {"9", "json:2000:6"}, {"-2", "text:500:7"},
	} {
		stream := flateStream(src.level, "-", src.input, "w:*,x")
		for n := 0; n < len(stream); n += 1 + len(stream)/40 {
			add(flateReaderCase{"truncated", src.level, "-", src.input, "w:*,x", fmt.Sprintf("t:%d", n), 4096, false})
		}
		g := &splitmix64{uint64(len(stream))}
		for i := 0; i < 40; i++ {
			pos := int(g.next() % uint64(len(stream)))
			val := 1 + int(g.next()%255)
			add(flateReaderCase{"corrupt", src.level, "-", src.input, "w:*,x", fmt.Sprintf("x:%d:%d", pos, val), 4096, false})
		}
		add(flateReaderCase{"trailing", src.level, "-", src.input, "w:*,x", "a:00ff00ff", 4096, false})
	}
	for _, v := range goFlateVectors(goroot) {
		for _, bufsize := range []int{1, 4096} {
			add(flateReaderCase{v.name, "raw", "-", "lit:" + hex.EncodeToString(v.stream), "-", "-", bufsize, false})
		}
	}
	return cases
}

func writeFlateReader(dir, goroot string) {
	g := newGolden(dir, "flate_reader.txt")
	defer g.close()
	for _, c := range flateReaderCases(goroot) {
		reset := "once"
		if c.reset {
			reset = "reset"
		}
		g.line(c.name, c.level, c.dict, c.input, c.script, c.mutation, strconv.Itoa(c.bufsize), reset, runFlateReader(c))
	}
}

// gzipStream builds a stream from "raw:SPEC" (the bytes of an input spec) or &-separated
// members LEVEL~HEADER~INPUT, each written by gzip.Writer with w:*,x.
func gzipStream(members string) []byte {
	if spec, ok := strings.CutPrefix(members, "raw:"); ok {
		return genInput(spec)
	}
	var b []byte
	for _, m := range strings.Split(members, "&") {
		f := strings.Split(m, "~")
		t := &target{failAt: -1}
		w, err := gzip.NewWriterLevel(t, atoi(f[0]))
		if err != nil {
			panic(err)
		}
		w.Header = parseHeader(f[1])
		if _, err := w.Write(genInput(f[2])); err != nil {
			panic(err)
		}
		if err := w.Close(); err != nil {
			panic(err)
		}
		b = append(b, t.buf.Bytes()...)
	}
	return b
}

func headerResult(h gzip.Header) string {
	extra := "nil"
	if h.Extra != nil {
		extra = "x" + hex.EncodeToString(h.Extra)
	}
	mtime := "zero"
	if !h.ModTime.IsZero() {
		mtime = strconv.FormatInt(h.ModTime.Unix(), 10)
	}
	return fmt.Sprintf("%s.%s.%s.%s.%d", "x"+hexs(h.Name), "x"+hexs(h.Comment), extra, mtime, h.OS)
}

// gzipReaderCase is one line of gzip_reader.txt. Modes: ms (multistream), ss (one member per
// Reset, until Reset fails: Go's TestMultistreamFalse) and rs (read, then Reset onto the whole
// stream and read again).
type gzipReaderCase struct {
	name, members, mutation, mode string
	bufsize                       int
}

// runGzipReader returns LEN.FNV of the stream, then |!error|REMAINING when NewReader fails, or
// passes separated by ||: ok|HEADER|TRACE|LEN.FNV|CLOSE|REMAINING for a read, and
// !error|HEADER|REMAINING for a failed Reset.
func runGzipReader(c gzipReaderCase) string {
	stream := mutate(gzipStream(c.members), c.mutation)
	prefix := fmt.Sprintf("%d.%016x|", len(stream), fnv64(stream))
	br := bytes.NewReader(stream)
	z, err := gzip.NewReader(br)
	if err != nil {
		return prefix + errResult(err) + "|" + strconv.Itoa(br.Len())
	}
	var parts []string
	pass := func() string {
		return "ok|" + headerResult(z.Header) + "|" + readTrace(z, c.bufsize) + "|" + errResult(z.Close()) + "|" + strconv.Itoa(br.Len())
	}
	switch c.mode {
	case "ms":
		parts = append(parts, pass())
	case "ss":
		for round := 0; round < 4; round++ {
			z.Multistream(false)
			parts = append(parts, pass())
			if err := z.Reset(br); err != nil {
				parts = append(parts, errResult(err)+"|"+headerResult(z.Header)+"|"+strconv.Itoa(br.Len()))
				break
			}
		}
	case "rs":
		parts = append(parts, pass())
		br = bytes.NewReader(stream)
		if err := z.Reset(br); err != nil {
			parts = append(parts, errResult(err)+"|"+headerResult(z.Header)+"|"+strconv.Itoa(br.Len()))
		} else {
			parts = append(parts, pass())
		}
	default:
		panic("unknown mode " + c.mode)
	}
	return prefix + strings.Join(parts, "||")
}

func gzipReaderCases(goroot string) []gzipReaderCase {
	var cases []gzipReaderCase
	add := func(c gzipReaderCase) { cases = append(cases, c) }
	full := "name=" + hexs("name") + ",comment=" + hexs("comment") + ",extra=" + hexs("extra") + ",mtime=100000000.0,os=3"
	latin := "name=" + hexs("Äußerung") + ",comment=" + hexs("ÿ")
	for _, m := range []string{
		"6~-~lit:",
		"6~-~lit:68656c6c6f20776f726c640a",
		"6~" + full + "~text:50000:1",
		"9~" + latin + "~json:70000:2",
		"1~extra=~zeros:100000",
		"0~-~rand:1000:3",
		"6~-~lit:61&6~-~lit:62&6~-~lit:",
		"6~" + full + "~text:300:4&1~-~json:300:5",
	} {
		for _, mode := range []string{"ms", "ss", "rs"} {
			for _, bufsize := range []int{1, 512, 70000} {
				if bufsize == 1 && len(gzipStream(m)) > 20000 {
					continue
				}
				add(gzipReaderCase{"members", m, "-", mode, bufsize})
			}
		}
	}
	for _, m := range []string{"6~" + full + "~text:2000:6", "6~-~lit:68656c6c6f&6~-~lit:21"} {
		stream := gzipStream(m)
		for n := 0; n <= len(stream); n += 1 + len(stream)/50 {
			add(gzipReaderCase{"truncated", m, fmt.Sprintf("t:%d", n), "ms", 512})
		}
		g := &splitmix64{uint64(len(stream))}
		for i := 0; i < 60; i++ {
			pos := int(g.next() % uint64(len(stream)))
			val := 1 + int(g.next()%255)
			add(gzipReaderCase{"corrupt", m, fmt.Sprintf("x:%d:%d", pos, val), "ms", 512})
		}
		add(gzipReaderCase{"trailing", m, "a:00", "ms", 512})
		add(gzipReaderCase{"trailing", m, "a:1f8b0800", "ms", 512})
		add(gzipReaderCase{"trailing", m, "a:1f8b08000000000000ff", "ss", 512})
	}
	// Header flags: FHCRC (good and bad), FTEXT, reserved bits, and long strings.
	for _, raw := range []string{
		"1f8b080200000000000099e00300",
		"1f8b080200000000000000000300",
		"1f8b08010000000000ff0300000000000000000000",
		"1f8b08e00000000000ff0300000000000000000000",
		"1f8b0800000000000000",
		"1f8b0900000000000000",
		"1f8a0800000000000000",
		"1f",
	} {
		add(gzipReaderCase{"header", "raw:lit:" + raw, "-", "ms", 512})
	}
	for _, n := range []int{510, 511, 512, 600} {
		name := strings.Repeat("6e", n)
		add(gzipReaderCase{"longname", "raw:lit:1f8b08080000000000ff" + name + "000300000000000000000000", "-", "ms", 512})
	}
	for _, v := range goGzipVectors(goroot) {
		for _, mode := range []string{"ms", "ss", "rs"} {
			add(gzipReaderCase{v.name, "raw:lit:" + hex.EncodeToString(v.stream), "-", mode, 4096})
		}
	}
	// Go's TestNilStream.
	add(gzipReaderCase{"TestNilStream", "raw:lit:", "-", "ms", 4096})
	add(gzipReaderCase{"issue6550", "raw:file:issue6550.gz", "-", "ms", 32768})
	return cases
}

func writeGzipReader(dir, goroot string) {
	g := newGolden(dir, "gzip_reader.txt")
	defer g.close()
	for _, c := range gzipReaderCases(goroot) {
		g.line(c.name, c.members, c.mutation, c.mode, strconv.Itoa(c.bufsize), runGzipReader(c))
	}
}
