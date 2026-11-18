// Command review writes the golden files for the divergences found when reviewing pbs-gocompat
// against Go, replayed by the Rust tests named in each file's comment below.
//
// Every output line is a tab-separated list of fields, each quoted with strconv.Quote.
//
// Run from the directory of this module (crates/gocompat/testdata/go):
//
//	go run ./review golden
package main

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: review <output dir>")
		os.Exit(2)
	}
	write(os.Args[1], "review_parse_number.txt", parseNumberRows())
	write(os.Args[1], "review_fmt_bytes.txt", fmtBytesRows())
}

func write(dir, name string, rows [][]string) {
	var b strings.Builder
	for _, fields := range rows {
		for i, f := range fields {
			if i > 0 {
				b.WriteByte('\t')
			}
			b.WriteString(strconv.Quote(f))
		}
		b.WriteByte('\n')
	}
	if err := os.WriteFile(filepath.Join(dir, name), []byte(b.String()), 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func errText(err error) string {
	if err == nil {
		return ""
	}
	return "error: " + err.Error()
}

// numberPatterns place one byte (%c) inside hexadecimal and decimal numbers. Bytes 0x10..0x19
// matter most: lower(c) = c|0x20 maps them onto the ASCII digits, which Go's hex-digit test
// ('a' <= lower(c) && lower(c) <= 'f') rejects.
var numberPatterns = []string{
	"0x1%cp0", "0x%c1p0", "0x1.%cp0", "0x1p%c", "0x1p1%c", "0x1%c", "1%c", "1e%c", "%c1",
	"0x1_%cp0", "0x_%c1p0", "0x1%c_0p0", "0X%c1e308p12345678901234567890",
}

// parseNumberRows: strconv.ParseFloat (32 and 64 bits) and ParseInt/ParseUint (bases 0 and 16)
// of every pattern with every ASCII byte. Replayed by strconv::tests::review_parse_number_matches_go.
func parseNumberRows() [][]string {
	var rows [][]string
	for _, p := range numberPatterns {
		for c := 0; c < 0x80; c++ {
			s := strings.Replace(p, "%c", string(rune(c)), 1)
			for _, bits := range []int{32, 64} {
				f, err := strconv.ParseFloat(s, bits)
				rows = append(rows, []string{"ParseFloat", s, strconv.Itoa(bits), fmt.Sprintf("%016x", math.Float64bits(f)), errText(err)})
			}
			for _, base := range []int{0, 16} {
				i, ierr := strconv.ParseInt(s, base, 64)
				u, uerr := strconv.ParseUint(s, base, 64)
				rows = append(rows, []string{"ParseInt", s, strconv.Itoa(base), strconv.FormatInt(i, 10), errText(ierr), strconv.FormatUint(u, 10), errText(uerr)})
			}
		}
	}
	return rows
}

type b8 uint8

type withBytes struct {
	B [][]byte
	R json.RawMessage
}

// fmtBytesValues are slices and arrays whose element is, or is not, of kind uint8. Go prints
// the first group as bytes for %s %q %x %X and the second element by element.
func fmtBytesValues() []struct {
	name string
	v    any
} {
	return []struct {
		name string
		v    any
	}{
		{"bytes_slices", [][]byte{[]byte("ab"), nil, []byte("\x00\xff")}},
		{"byte_slice_array", [2][]uint8{{1, 2}, {}}},
		{"nil_byte_slice_ptrs", []*[]uint8{nil, nil}},
		{"raw_messages", []json.RawMessage{json.RawMessage(`{"a":1}`), nil}},
		{"byte_arrays", [][2]uint8{{65, 66}, {0, 255}}},
		{"named_uint8s", []b8{65, 66}},
		{"named_uint8_array", [3]b8{1, 2, 3}},
		{"any_uint8s", []any{uint8(65), uint8(66)}},
		{"uint8s", []uint8{65, 66}},
		{"uint8_array", [3]uint8{65, 66, 67}},
		{"bytes_map", map[string][]byte{"k": []byte("v")}},
		{"struct_bytes", withBytes{B: [][]byte{[]byte("x")}, R: json.RawMessage(`[]`)}},
	}
}

var fmtBytesFormats = []string{"%s", "%q", "%x", "%X", "% x", "%#x", "%v", "%d", "%+q", "%#q", "%5s|", "%.1x"}

// fmtBytesRows: fmt.Sprintf of every format with every value. Replayed by
// fmt::tests::review_fmt_bytes_match_go.
func fmtBytesRows() [][]string {
	var rows [][]string
	for _, f := range fmtBytesFormats {
		for _, v := range fmtBytesValues() {
			rows = append(rows, []string{f, v.name, fmt.Sprintf(f, v.v)})
		}
	}
	return rows
}
