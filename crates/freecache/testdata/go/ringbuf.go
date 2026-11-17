package main

import (
	"fmt"

	"github.com/coocood/freecache"
)

// writeRingBuf records the RingBuf state after every step of TestRingBuf (ringbuf_test.go),
// extended with the reads, writes and range errors around it. Dumps are printed with %q.
func writeRingBuf(g *golden) {
	defer g.close()
	rb := freecache.NewRingBuf(16, 0)
	for i := 0; i < 2; i++ {
		n, err := rb.Write([]byte("fghibbbbccccddde"))
		g.line("ringbuf", fmt.Sprintf("write %d %v %s", n, err, rb.String()))
		n, err = rb.Write([]byte("fghibbbbc"))
		g.line("ringbuf", fmt.Sprintf("write %d %v %s %q", n, err, rb.String(), rb.Dump()))
		rb.Resize(16)
		off := rb.Evacuate(9, 3)
		g.line("ringbuf", fmt.Sprintf("evacuate %d %s %q", off, rb.String(), rb.Dump()))
		off = rb.Evacuate(15, 5)
		g.line("ringbuf", fmt.Sprintf("evacuate %d %s %q", off, rb.String(), rb.Dump()))
		rb.Resize(64)
		g.line("ringbuf", fmt.Sprintf("resize %s %q", rb.String(), rb.Dump()))
		rb.Resize(32)
		g.line("ringbuf", fmt.Sprintf("resize %s %q", rb.String(), rb.Dump()))
		data := make([]byte, 5)
		n, err = rb.ReadAt(data, off)
		g.line("ringbuf", fmt.Sprintf("readat %d %v %q", n, err, data))
		long := make([]byte, 40)
		n, err = rb.ReadAt(long, off)
		g.line("ringbuf", fmt.Sprintf("readat %d %v", n, err))
		n, err = rb.ReadAt(long, rb.End()+1)
		g.line("ringbuf", fmt.Sprintf("readat %d %v", n, err))
		s, err := rb.Slice(off, 5)
		g.line("ringbuf", fmt.Sprintf("slice %q %v", s, err))
		g.line("ringbuf", fmt.Sprintf("equalat %v %v", rb.EqualAt([]byte("efghi"), off), rb.EqualAt([]byte("efghx"), off)))
		n, err = rb.WriteAt([]byte("XY"), off)
		g.line("ringbuf", fmt.Sprintf("writeat %d %v %q", n, err, rb.Dump()))
		n, err = rb.WriteAt([]byte("XY"), rb.End()-1)
		g.line("ringbuf", fmt.Sprintf("writeat %d %v", n, err))
		g.line("ringbuf", fmt.Sprintf("evacuate %d", rb.Evacuate(0, 10)))
		n, err = rb.Write(make([]byte, 33))
		g.line("ringbuf", fmt.Sprintf("write %d %v", n, err))
		rb.Skip(40)
		g.line("ringbuf", "skip "+rb.String())
		rb.Reset(0)
		g.line("ringbuf", "reset "+rb.String())
	}
}
