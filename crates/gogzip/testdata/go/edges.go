package main

import (
	"fmt"
	"strconv"
	"strings"
)

// The edge cases of the port's review, in four golden files:
//
//	flate_edges.txt         NAME LEVEL DICT INPUTS SCRIPT RESULT#SIZES
//	gzip_edges.txt          NAME LEVEL HEADER INPUTS SCRIPT RESULT#SIZES
//	flate_reader_edges.txt  lines of the flate_reader.txt format
//	gzip_reader_edges.txt   lines of the gzip_reader.txt format
//
// RESULT is a script's result (writer.go) and SIZES the sizesResult of each of its targets,
// separated by semicolons, so that these files also check how the compressed bytes are split
// into Write calls. The writer cases are the BestSpeed encoder's offsets wrapping (wrap), hash
// chains rebased while their data still matches (rebase), stored blocks around the 65535-byte
// boundaries (stored), flush and close patterns (flush), literal codes past the 15-bit limit
// (huff), and generated scripts over generated inputs (gen, gendict). The reader cases are
// every truncation and a bit flip at every byte of small streams (truncated, flip), trailing
// data (trailing), and generated corruptions of generated streams (gen).

// runSizes runs text and returns its result followed by #SIZES.
func (s *script) runSizes(text string) string {
	res := s.run(text)
	var sizes []string
	for _, t := range s.targets {
		sizes = append(sizes, t.sizesResult())
	}
	return res + "#" + strings.Join(sizes, ";")
}

// gen generates inputs, scripts, headers and mutations from a splitmix64 sequence.
type gen struct{ r *splitmix64 }

func (g *gen) pick(xs ...int) int { return xs[g.r.next()%uint64(len(xs))] }

// boundarySizes are input sizes around the compressors' buffer and block limits.
var boundarySizes = []int{0, 1, 2, 3, 4, 5, 15, 16, 17, 18, 127, 128, 129, 255, 256, 257, 258, 259,
	260, 261, 262, 263, 264, 4095, 4096, 4097, 16383, 16384, 16385, 32767, 32768, 32769, 65273,
	65274, 65275, 65276, 65534, 65535, 65536, 65537, 98303, 98304, 98305, 131069, 131070, 131071,
	131072, 131073, 196605, 196606, 196607, 262144}

// chunkSizes are the chunk sizes of generated chunked writes.
var chunkSizes = []int{1, 2, 3, 5, 7, 16, 100, 128, 255, 256, 257, 261, 262, 263, 1000, 1024, 4095,
	4096, 4097, 8192, 16384, 32767, 32768, 32769, 65535, 65536, 65537, 100000}

func (g *gen) size(limit int) int {
	var n int
	switch g.r.next() % 4 {
	case 0:
		n = g.pick(boundarySizes...)
	case 1:
		n = int(g.r.next() % 300)
	case 2:
		n = int(g.r.next() % 70000)
	default:
		n = int(g.r.next() % uint64(limit+1))
	}
	return min(n, limit)
}

// atom returns an input spec of about n bytes.
func (g *gen) atom(n int) string {
	seed := g.r.next() % 100000
	switch g.r.next() % 14 {
	case 0:
		return fmt.Sprintf("zeros:%d", n)
	case 1, 2:
		return fmt.Sprintf("rand:%d:%d", n, seed)
	case 3:
		p := 1 + g.r.next()%uint64(g.pick(4, 300, 5000, 40000))
		return fmt.Sprintf("repeat:%d:%d:%d", p, n, seed)
	case 4:
		k := 1 + g.r.next()%uint64(g.pick(2, 4, 16, 256))
		return fmt.Sprintf("low:%d:%d:%d", k, n, seed)
	case 5, 6:
		return fmt.Sprintf("text:%d:%d", n, seed)
	case 7, 8:
		return fmt.Sprintf("json:%d:%d", n, seed)
	case 9, 10:
		return fmt.Sprintf("mixed:%d:%d", n, seed)
	case 11:
		return fmt.Sprintf("file:fixtures.json:%d", min(n, 200000))
	case 12:
		return fmt.Sprintf("fib:%d:%d", 10+g.r.next()%15, seed)
	default:
		return fmt.Sprintf("%s:%d", []string{"abc", "seq", "square"}[g.r.next()%3], n)
	}
}

// spec returns an input spec of at most about limit bytes, sometimes a concatenation.
func (g *gen) spec(limit int) string {
	if g.r.next()%5 == 0 {
		return g.atom(g.size(limit/2)) + "+" + g.atom(g.size(limit/2))
	}
	return g.atom(g.size(limit))
}

// script returns a script that writes all of an input of n bytes with writes, chunked writes
// (at most 20000 writes and 200 flushes each), flushes and empty writes, then closes,
// sometimes followed by more operations.
func (g *gen) script(n int) string {
	var ops []string
	pos := 0
	for pos < n && len(ops) < 30 {
		rem := n - pos
		switch g.r.next() % 12 {
		case 0, 1, 2:
			k := 1 + int(g.r.next()%uint64(min(rem, g.pick(10, 300, 70000, 200000))))
			ops = append(ops, fmt.Sprintf("w:%d", k))
			pos += k
		case 3, 4, 5:
			k := g.pick(chunkSizes...)
			m := 1 + int(g.r.next()%uint64(rem))
			op, limit := "c", 20000
			if g.r.next()%3 == 0 {
				op, limit = "cf", 200
			}
			m = min(m, k*limit)
			ops = append(ops, fmt.Sprintf("%s:%d:%d", op, k, m))
			pos += m
		case 6, 7:
			ops = append(ops, "f")
		case 8:
			ops = append(ops, "e")
		case 9:
			m := g.pick(300, 5000, 70000)
			if rem < 20000 {
				m = g.pick(3, 300, 5000, 70000)
			}
			op := "r"
			if rem/m < 100 && g.r.next()%3 == 0 {
				op = "rf"
			}
			ops = append(ops, fmt.Sprintf("%s:%d:%d", op, m, g.r.next()%1000))
			pos = n
		default:
			ops = append(ops, "w:*")
			pos = n
		}
	}
	if pos < n {
		ops = append(ops, "w:*")
	}
	ops = append(ops, "x")
	switch g.r.next() % 8 {
	case 0:
		ops = append(ops, "x")
	case 1:
		ops = append(ops, "f", "x")
	case 2:
		ops = append(ops, "e", "f")
	}
	return strings.Join(ops, ",")
}

// header returns a gzip header spec. With latin, its strings are short and Latin-1 and its
// extra field is short, so that gzip.Writer accepts it and the reader's header result stays
// short (gzip_reader.txt has the long names).
func (g *gen) header(latin bool) string {
	var kv []string
	if g.r.next()%2 == 0 {
		names := []string{"name", "a", "Äußerung.txt", "ÿ\u0080!", "xÿ", "file.json"}
		if !latin {
			names = append(names, strings.Repeat("n", 600), "Ā", "日本")
		}
		kv = append(kv, "name="+hexs(names[g.r.next()%uint64(len(names))]))
	}
	if g.r.next()%3 == 0 {
		comments := []string{"comment", "", "ÿ", "cé"}
		if !latin {
			comments = append(comments, strings.Repeat("c", 520), "a\x00b")
		}
		kv = append(kv, "comment="+hexs(comments[g.r.next()%uint64(len(comments))]))
	}
	if g.r.next()%3 == 0 {
		switch g.r.next() % 4 {
		case 0:
			kv = append(kv, "extra=")
		case 1:
			kv = append(kv, "extra="+hexs("extra"))
		case 2:
			kv = append(kv, fmt.Sprintf("extra=@rand:%d:%d", g.r.next()%300, g.r.next()%100))
		default:
			if latin {
				kv = append(kv, "extra=@zeros:2")
			} else {
				kv = append(kv, fmt.Sprintf("extra=@zeros:%d", g.pick(65534, 65535, 65536)))
			}
		}
	}
	if g.r.next()%3 == 0 {
		times := []string{"0.0", "0.1", "1.0", "-1.0", "100000000.0", "4294967295.0", "4294967296.0",
			"4294967297.9", "-4294967296.0", "1700000000.5"}
		kv = append(kv, "mtime="+times[g.r.next()%uint64(len(times))])
	}
	if g.r.next()%3 == 0 {
		kv = append(kv, fmt.Sprintf("os=%d", g.r.next()%256))
	}
	if len(kv) == 0 {
		return "-"
	}
	return strings.Join(kv, ",")
}

// mutation returns a mutation of a stream of n bytes (see mutate).
func (g *gen) mutation(n int) string {
	if n == 0 {
		return "-"
	}
	switch g.r.next() % 6 {
	case 0:
		return "-"
	case 1:
		return fmt.Sprintf("t:%d", g.r.next()%uint64(n))
	case 2:
		return fmt.Sprintf("x:%d:%d", g.r.next()%uint64(n), 1+g.r.next()%255)
	case 3:
		return fmt.Sprintf("a:%02x%02x", g.r.next()%256, g.r.next()%256)
	default:
		return fmt.Sprintf("m:%d:%d", g.r.next()%1000000, 1+g.r.next()%8)
	}
}

// flateEdge is one or more lines of flate_edges.txt: a script at each of levels.
type flateEdge struct {
	name                 string
	levels               []int
	dict, inputs, script string
}

func flateEdgeCases() []flateEdge {
	var cases []flateEdge
	add := func(name string, levels []int, dict, inputs string, scripts ...string) {
		for _, s := range scripts {
			cases = append(cases, flateEdge{name, levels, dict, inputs, s})
		}
	}
	// BestSpeed: a flush of up to 16 bytes writes a stored block and resets the encoder, which
	// adds maxMatchOffset to its offsets, from maxStoreBlockSize. 65530 of them bring the
	// offset to 2 below bufferReset, so that the next encode shifts the table while the encoder
	// has history (Go's TestDeflateFast_Reset, through the public API); with fewer, the shift
	// comes while encoding the text, and with more, at a reset, without history.
	for _, k := range []int{65520, 65529, 65530, 65531, 65540} {
		add("wrap", []int{1}, "-", fmt.Sprintf("rand:%d:1+text:300000:2", k),
			fmt.Sprintf("cf:1:%d,w:200,f,w:300,f,w:*,x", k),
			fmt.Sprintf("cf:1:%d,w:128,f,w:127,f,w:20,f,w:65535,w:70000,f,w:*,x", k),
			fmt.Sprintf("cf:1:%d,c:1000,x", k))
	}
	add("wrap", []int{1}, "-", "rand:65500:1+json:200000:3", "cf:1:65500,r:70000:9,x", "cf:1:65500,rf:70000:9,x")
	// Levels 2 to 9 past maxHashOffset (1<<24): zeros, which compress fast, then data whose
	// hash chains are rebased while they still match (flate_large.txt repeats one pattern).
	deflate := []int{2, 3, 4, 5, 6, 7, 8, 9}
	add("rebase", deflate, "-", "zeros:16760000+json:200000:12", "w:*,x")
	add("rebase", deflate, "-", "zeros:16700000+text:400000:11", "c:65536,x")
	// Stored blocks at the 65535-byte boundaries of level 0, and the other levels' choice of
	// a stored block there.
	for _, n := range []int{65534, 65535, 65536, 131070, 131071, 131072} {
		add("stored", levels, "-", fmt.Sprintf("rand:%d:%d", n, n), "w:*,x", "c:65535,x", "cf:65535,x",
			fmt.Sprintf("w:%d,f,w:*,x", min(n, 65535)), fmt.Sprintf("w:1,w:%d,w:*,x", min(n-1, 65534)),
			"c:32768,x")
	}
	// Flush and close: on an empty writer, repeated, after Close, around the lookahead the
	// compressor keeps (65275 = 2*windowSize - minMatchLength - maxMatchLength), and with a
	// dictionary and Reset.
	add("flush", levels, "-", "lit:", "f", "f,f", "f,x", "x,f", "f,e,f,x", "e", "x,x,x", "f,x,f,x")
	add("flush", levels, "-", "text:100000:5", "w:*,f,f,f,x", "c:65275,f,x", "c:65276,f,x", "c:65274,f,x",
		"w:65275,f,w:261,f,w:*,x", "w:32768,f,w:32768,f,w:*,x", "cf:16384,x", "cf:258,x", "cf:261,x")
	add("flush", levels, "-", "rand:100000:6", "c:65275,f,x", "w:65276,f,w:*,x", "cf:32767,x",
		"w:16,f,w:17,f,w:127,f,w:128,f,w:*,x")
	add("flush", levels, "text:40000:1", "text:100000:5", "w:*,x", "f,x", "x", "R,x", "R,f,x", "c:30000,R,w:*,x")
	// Literal codes past the 15-bit limit, in one block (fib:22 is 46367 bytes) and more.
	for _, k := range []int{16, 17, 20, 22, 23, 24} {
		add("huff", levels, "-", fmt.Sprintf("fib:%d:%d", k, k), "w:*,x", "c:4096,x", "cf:30000,x")
	}
	g := &gen{&splitmix64{20261117}}
	for i := 0; i < 80; i++ {
		inputs := g.spec(300000)
		text := g.script(len(genInput(inputs)))
		if g.r.next()%6 == 0 {
			second := g.spec(100000)
			inputs += "|" + second
			text += ",R," + g.script(len(genInput(second)))
		}
		add("gen", levels, "-", inputs, text)
	}
	for i := 0; i < 10; i++ {
		dict := g.spec(70000)
		if g.r.next()%5 == 0 {
			dict = "lit:"
		}
		inputs := g.spec(150000)
		text := g.script(len(genInput(inputs)))
		if g.r.next()%3 == 0 {
			second := g.spec(50000)
			inputs += "|" + second
			text += ",R," + g.script(len(genInput(second)))
		}
		add("gendict", levels, dict, inputs, text)
	}
	return cases
}

func writeFlateEdges(dir string) {
	g := newGolden(dir, "flate_edges.txt")
	defer g.close()
	for _, c := range flateEdgeCases() {
		for _, level := range c.levels {
			g.line(c.name, strconv.Itoa(level), c.dict, c.inputs, c.script,
				flateScript(level, c.dict, c.inputs).runSizes(c.script))
		}
	}
}

func writeGzipEdges(dir string) {
	g := newGolden(dir, "gzip_edges.txt")
	defer g.close()
	r := &gen{&splitmix64{4242}}
	for i := 0; i < 150; i++ {
		header := r.header(false)
		inputs := r.spec(100000)
		text := r.script(len(genInput(inputs)))
		if r.r.next()%4 == 0 {
			second := r.spec(30000)
			inputs += "|" + second
			if r.r.next()%2 == 0 {
				text += ",R,H," + r.script(len(genInput(second)))
			} else {
				text += ",R," + r.script(len(genInput(second)))
			}
		}
		switch r.r.next() % 5 {
		case 0:
			text = fmt.Sprintf("RS:%d,H,", r.r.next()%12) + text
		case 1:
			text = fmt.Sprintf("RF:%d,H,", r.r.next()%12) + text
		}
		level := levels[r.r.next()%uint64(len(levels))]
		g.line("gen", strconv.Itoa(level), header, inputs, text, gzipScript(level, header, inputs).runSizes(text))
	}
}

func writeFlateReaderEdges(dir string) {
	g := newGolden(dir, "flate_reader_edges.txt")
	defer g.close()
	line := func(c flateReaderCase) {
		reset := "once"
		if c.reset {
			reset = "reset"
		}
		g.line(c.name, c.level, c.dict, c.input, c.script, c.mutation, strconv.Itoa(c.bufsize), reset, runFlateReader(c))
	}
	// Every truncation (and with a 1-byte buffer for the first), and a flip of bit pos%8 at
	// every byte pos, of a dynamic block, of BestSpeed blocks around a flush and of a
	// Huffman-only block.
	for i, src := range []struct{ level, input, script string }{
		{"6", "text:300:3", "w:*,x"},
		{"1", "text:300:3", "w:100,f,w:*,x"},
		{"-2", "text:200:7", "w:*,x"},
	} {
		stream := flateStream(src.level, "-", src.input, src.script)
		for n := 0; n <= len(stream); n++ {
			line(flateReaderCase{"truncated", src.level, "-", src.input, src.script, fmt.Sprintf("t:%d", n), 4096, false})
			if i == 0 {
				line(flateReaderCase{"truncated", src.level, "-", src.input, src.script, fmt.Sprintf("t:%d", n), 1, false})
			}
		}
		for pos := 0; pos < len(stream); pos++ {
			flip := fmt.Sprintf("x:%d:%d", pos, 1<<(pos%8))
			line(flateReaderCase{"flip", src.level, "-", src.input, src.script, flip, 4096, false})
		}
	}
	r := &gen{&splitmix64{99}}
	for i := 0; i < 400; i++ {
		c := flateReaderCase{name: "gen", dict: "-", script: "-"}
		if r.r.next()%10 == 0 {
			c.level = "raw"
			c.input = fmt.Sprintf("rand:%d:%d", r.r.next()%200, r.r.next()%100000)
		} else {
			c.level = strconv.Itoa(levels[r.r.next()%uint64(len(levels))])
			c.input = r.spec(20000)
			c.script = r.script(len(genInput(c.input)))
			if r.r.next()%8 == 0 {
				c.dict = r.spec(40000)
			}
		}
		stream := flateStream(c.level, c.dict, c.input, c.script)
		c.mutation = r.mutation(len(stream))
		c.bufsize = r.pick(1, 7, 100, 512, 4096, 32768, 70000)
		if c.bufsize == 1 && len(stream) > 3000 {
			c.bufsize = 512
		}
		c.reset = r.r.next()%4 == 0
		line(c)
	}
}

func writeGzipReaderEdges(dir string) {
	g := newGolden(dir, "gzip_reader_edges.txt")
	defer g.close()
	line := func(c gzipReaderCase) {
		g.line(c.name, c.members, c.mutation, c.mode, strconv.Itoa(c.bufsize), runGzipReader(c))
	}
	// Every truncation and a flip of bit pos%8 at every byte pos of a member with every header
	// field, and of two members, the trailers included; then trailing data after them.
	full := "name=" + hexs("name") + ",comment=" + hexs("comment") + ",extra=" + hexs("extra") + ",mtime=100000000.0,os=3"
	for _, m := range []string{"6~" + full + "~text:200:1", "1~-~lit:61&6~name=" + hexs("b") + "~lit:62"} {
		stream := gzipStream(m)
		for n := 0; n <= len(stream); n++ {
			for _, mode := range []string{"ms", "ss"} {
				line(gzipReaderCase{"truncated", m, fmt.Sprintf("t:%d", n), mode, 512})
			}
		}
		for pos := 0; pos < len(stream); pos++ {
			line(gzipReaderCase{"flip", m, fmt.Sprintf("x:%d:%d", pos, 1<<(pos%8)), "ms", 512})
		}
		for _, tail := range []string{"00", "1f", "1f8b", "1f8b08", "1f8b0800000000000000",
			"1f8b0800000000000000030000000000000000", "1f8b080000000000000003000000000000000000", "ffffffff"} {
			for _, mode := range []string{"ms", "ss", "rs"} {
				line(gzipReaderCase{"trailing", m, "a:" + tail, mode, 512})
			}
		}
	}
	r := &gen{&splitmix64{31337}}
	for i := 0; i < 200; i++ {
		var members []string
		count := 1 + int(r.r.next()%3)
		for j := 0; j < count; j++ {
			level := levels[r.r.next()%uint64(len(levels))]
			members = append(members, fmt.Sprintf("%d~%s~%s", level, r.header(true), r.spec(5000)))
		}
		m := strings.Join(members, "&")
		if r.r.next()%12 == 0 {
			m = fmt.Sprintf("raw:rand:%d:%d", r.r.next()%100, r.r.next()%1000)
		}
		mutation := r.mutation(len(gzipStream(m)))
		mode := []string{"ms", "ss", "rs"}[r.r.next()%3]
		bufsize := r.pick(1, 7, 100, 512, 4096, 70000)
		if bufsize == 1 && len(gzipStream(m)) > 3000 {
			bufsize = 512
		}
		line(gzipReaderCase{"gen", m, mutation, mode, bufsize})
	}
}
