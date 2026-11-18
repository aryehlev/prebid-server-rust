package main

import (
	"fmt"
	"math"
	"strconv"
	"strings"
)

// floatValues are the explicit FormatFloat inputs: specials, ties, prices and extremes.
var floatValues = []float64{
	0, math.Copysign(0, -1), 1, -1, 0.1, 0.2, 0.3, 0.5, 1.5, 2.5, -2.5, 0.125, 0.375, 1.005,
	2.675, 8.675, 0.045, 4.35, 0.01, 1.2, 3.14159, 99.99, 100, 123.456, 1e-5, 1e-6, 1e-7,
	0.000123, 123456789, 1e20, 1e21, 1e23, 1 / 3.0, 2 / 3.0, 9007199254740993, 12345678901234567890,
	5e-324, math.SmallestNonzeroFloat32, math.MaxFloat32, math.MaxFloat64, 1.7976931348623157e308,
	2.2250738585072014e-308, 1e100, 123e-20, 0.999999, 9.5, 99.5, 0.05, 0.15, 0.25, 0.35, 1.25,
	1e15, 1e16, 123456.7, 7.0e-10, 4.940656458412465e-324, 3.4028235e38, 1.401298464324817e-45,
	math.Inf(1), math.Inf(-1), math.NaN(),
}

var floatFmts = []byte{'e', 'E', 'f', 'g', 'G', 'b', 'x', 'X'}
var floatPrecs = []int{-1, 0, 1, 2, 3, 4, 5, 6, 10, 17, 20, 25}

func genStrconv() {
	// Explicit rows for the common formats; the full matrix is checksummed below.
	var ff table
	matrix := newSweep()
	for _, v := range floatValues {
		for _, bits := range []int{32, 64} {
			for _, f := range floatFmts {
				for _, p := range floatPrecs {
					out := strconv.FormatFloat(v, f, p, bits)
					matrix.add(out)
					if bits == 64 && (f == 'e' || f == 'f' || f == 'g') && (p == -1 || p == 0 || p == 2 || p == 6 || p == 17) {
						ff.row(fmt.Sprintf("%016x", math.Float64bits(v)), string(f), strconv.Itoa(p),
							strconv.Itoa(bits), out)
					}
				}
			}
		}
	}
	ff.write("strconv_format_float.txt")
	var fv table
	for _, v := range floatValues {
		fv.row(fmt.Sprintf("%016x", math.Float64bits(v)))
	}
	fv.write("strconv_float_values.txt")

	// Sweeps over random bit patterns and over price-like values.
	var sum table
	sum.row("format_float_matrix", matrix.sum())
	rng := splitmix64{state: 1}
	bitsSweep := newSweep()
	for i := 0; i < 100000; i++ {
		v := math.Float64frombits(rng.next())
		for _, f := range []byte{'e', 'f', 'g'} {
			for _, p := range []int{-1, 0, 3, 17} {
				if f == 'f' && math.Abs(v) > 1e30 {
					continue
				}
				bitsSweep.add(strconv.FormatFloat(v, f, p, 64))
			}
		}
		bitsSweep.add(strconv.FormatFloat(v, 'g', -1, 32))
	}
	sum.row("format_float_bits", bitsSweep.sum())
	priceSweep := newSweep()
	for i := 0; i < 100000; i++ {
		v := float64(rng.next()%10000000) / 1000
		for _, p := range []int{-1, 0, 1, 2, 3, 4} {
			priceSweep.add(strconv.FormatFloat(v, 'f', p, 64))
		}
		priceSweep.add(strconv.FormatFloat(v*1.1, 'f', -1, 64))
		priceSweep.add(strconv.FormatFloat(v*1.1, 'g', 3, 64))
	}
	sum.row("format_float_prices", priceSweep.sum())
	parseSweep := newSweep()
	for i := 0; i < 100000; i++ {
		v := math.Float64frombits(rng.next())
		for _, s := range []string{strconv.FormatFloat(v, 'g', -1, 64), strconv.FormatFloat(v, 'e', 25, 64), strconv.FormatFloat(v, 'g', 8, 64)} {
			f64, err64 := strconv.ParseFloat(s, 64)
			f32, err32 := strconv.ParseFloat(s, 32)
			parseSweep.add(fmt.Sprintf("%016x %s %016x %s", math.Float64bits(f64), errText(err64), math.Float64bits(f32), errText(err32)))
		}
	}
	sum.row("parse_float", parseSweep.sum())
	sum.write("strconv_sweeps.txt")

	var pf table
	for _, s := range []string{
		"", "1", "+1", "-1", "1.5", ".5", "5.", ".", "-.", "1e5", "1E5", "1e+5", "1e-5", "1e", "1e+",
		"1x", "x", "0x1p-2", "0x1.8p1", "0X1P+2", "0x1", "0x.8p0", "0x1p", "1_000", "0x_1p0", "0x1_0p0",
		"1__0", "_1", "inf", "Inf", "+Inf", "-inf", "infinity", "-Infinity", "infin", "infx", "nan",
		"NaN", "+nan", "-nan", "1e308", "1e309", "-1e309", "1e-400", "4.9e-324", "2e-324", "3e-324",
		"1.7976931348623157e308", "1.7976931348623158e308", "1.7976931348623159e308",
		"3.4028235e38", "3.4028236e38", "1e39", "1e-46", "123456789012345678901234567890",
		"0.1", "0.30000000000000004", "1e100000", "1e-100000", "00001", "1.2.3", "1e5.5", " 1",
		"1 ", "0x", "0x1.p0", "0x1p1_0", "9.5", "2.675",
	} {
		for _, bits := range []int{32, 64} {
			f, err := strconv.ParseFloat(s, bits)
			pf.row(s, strconv.Itoa(bits), fmt.Sprintf("%016x", math.Float64bits(f)), errText(err))
		}
	}
	pf.write("strconv_parse_float.txt")

	var pi table
	intInputs := []string{
		"", "0", "1", "-1", "+1", "-", "+", "12345", "-12345", "9223372036854775807",
		"9223372036854775808", "-9223372036854775808", "-9223372036854775809",
		"18446744073709551615", "18446744073709551616", "0x1f", "0X1F", "0b101", "0o17", "017",
		"0x", "0b", "08", "1_000", "0x_1f", "1__0", "_1", "1_", "abc", "z", "Z", "1e5", " 1",
		"127", "128", "-128", "-129", "255", "256", "00", "0_7", "9999999999999999999",
		"123456789012345678", "-123456789012345678", "1234567890123456789",
	}
	// Explicit rows for the common bases and sizes; the full matrix is checksummed.
	intMatrix := newSweep()
	for _, s := range intInputs {
		for _, base := range []int{0, 2, 8, 10, 16, 36, 1, 37, -1} {
			for _, bits := range []int{0, 8, 16, 32, 64, 65, -1} {
				iv, ierr := strconv.ParseInt(s, base, bits)
				uv, uerr := strconv.ParseUint(s, base, bits)
				fields := []string{s, strconv.Itoa(base), strconv.Itoa(bits),
					strconv.FormatInt(iv, 10), errText(ierr), strconv.FormatUint(uv, 10), errText(uerr)}
				intMatrix.add(strings.Join(fields, "|"))
				if (base == 0 || base == 10 || base == 16) && (bits == 0 || bits == 8 || bits == 64) {
					pi.row(fields...)
				}
			}
		}
		av, aerr := strconv.Atoi(s)
		pi.row(s, "atoi", "", strconv.Itoa(av), errText(aerr), "", "")
	}
	pi.write("strconv_parse_int.txt")
	var ii table
	for _, s := range intInputs {
		ii.row(s)
	}
	ii.write("strconv_int_inputs.txt")
	var pim table
	pim.row("parse_int_matrix", intMatrix.sum())
	pim.write("strconv_parse_int_matrix.txt")

	var fi table
	for _, v := range []int64{0, 1, -1, 35, 36, 255, -255, 1 << 62, math.MaxInt64, math.MinInt64} {
		for _, base := range []int{2, 8, 10, 16, 36} {
			fi.row(strconv.FormatInt(v, 10), strconv.Itoa(base), strconv.FormatInt(v, base), strconv.FormatUint(uint64(v), base))
		}
	}
	fi.write("strconv_format_int.txt")

	var pb table
	for _, s := range []string{"1", "t", "T", "true", "TRUE", "True", "0", "f", "F", "false", "FALSE", "False", "", "yes", "tRUE", " true"} {
		v, err := strconv.ParseBool(s)
		pb.row(s, strconv.FormatBool(v), errText(err))
	}
	pb.write("strconv_parse_bool.txt")

	var q table
	for _, s := range []string{
		"", "abc", "a\"b", "a\\b", "\a\b\f\n\r\t\v", "\x00\x01\x1f\x7f", "\u00a0\u00ad", "h\u00e9llo",
		"\u2028\u2029", "\ufeff", "\U0001f600", "\U0010ffff", "\U000e0001", "tab\there", "'single'",
		"\xff", "a\xffb", "\xe2\x82", "\xed\xa0\x80", "`", "\u65e5\u672c\u8a9e", "\u0378", "\u061c",
	} {
		q.row(s, strconv.Quote(s), strconv.QuoteToASCII(s), strconv.QuoteToGraphic(s), fmt.Sprint(strconv.CanBackquote(s)))
	}
	for _, r := range []rune{'a', '\'', '"', '\\', '\n', 0, 0x7f, 0xa0, 0xad, 0x2028, 0x1F600, 0x10FFFF, 0xFFFD} {
		q.row(string(r), strconv.QuoteRune(r), strconv.QuoteRuneToASCII(r), strconv.QuoteRuneToGraphic(r), "rune")
	}
	q.write("strconv_quote.txt")

	quoteSweep := newSweep()
	for r := rune(0); r <= 0x10FFFF; r++ {
		if r >= 0xD800 && r <= 0xDFFF {
			continue
		}
		quoteSweep.add(strconv.Quote(string(r)) + strconv.QuoteToASCII(string(r)) + strconv.QuoteToGraphic(string(r)))
	}
	var qs table
	qs.row("quote_all_runes", quoteSweep.sum())
	qs.write("strconv_quote_sweep.txt")

	var uq table
	for _, s := range []string{
		`""`, `"a"`, `"a\"b"`, `"\a\b\f\n\r\t\v\\"`, `"\x41\x7f\xff"`, `"\u0041\u00e9"`, `"\U0001F600"`,
		`"\101\377"`, `"\400"`, `"\8"`, `"\q"`, `"\'"`, `'\"'`, `'a'`, `'ab'`, `''`, `'\''`, `'\u00e9'`,
		"`raw\\n`", "`a\rb`", "`a`b`", `"a"b"`, `"unterminated`, `"`, ``, `x`, `"a` + "\n" + `b"`,
		`"\uD800"`, `"\U00110000"`, `"\x4"`, `"\u00e9\u00e9"`, `'\x41'`, `'\u00e9'`, `"\0"`, `"\00"`,
		`"\000"`, `"a\"`,
	} {
		out, err := strconv.Unquote(s)
		uq.row(s, out, errText(err))
	}
	uq.write("strconv_unquote.txt")
}
