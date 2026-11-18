package main

import (
	"encoding/hex"
	"math"
	"strconv"

	"github.com/gofrs/uuid"
	"github.com/prebid/prebid-server/v4/util/iosutil"
	"github.com/prebid/prebid-server/v4/util/mathutil"
	"github.com/prebid/prebid-server/v4/util/stringutil"
	"github.com/prebid/prebid-server/v4/util/urlutil"
)

// genSmall writes the goldens of the small util packages: iosutil, stringutil, urlutil,
// mathutil and the gofrs/uuid V4 layout.
func genSmall() {
	// iosutil.txt: input, ParseVersion major, minor, error, DetectVersionClassification.
	var ios table
	iosInputs := []string{
		"", "14", "14.0", "14.0.1", "14.1", "14.1.2", "14.2", "14.2.3", "14.3", "15.0", "13.7",
		"14.2.1.3", "xxx.2", "14.xxx", "+14.2", "-14.2", "14.-1", "014.02", "14..2", "14.2.",
		".14.2", "1_4.2", "0x14.2", " 14.2", "14.2 ", "9223372036854775807.1",
		"9223372036854775808.1", "14.9223372036854775808", "１４.2", "14.2.x", "a.b.c",
		"14.1.0.0", "0.0", "100.100",
	}
	for _, s := range iosInputs {
		v, err := iosutil.ParseVersion(s)
		ios.row(s, strconv.Itoa(v.Major), strconv.Itoa(v.Minor), errText(err),
			strconv.Itoa(int(iosutil.DetectVersionClassification(s))))
	}
	ios.write("iosutil.txt")

	// stringutil.txt: input, nil, the values joined by spaces, error.
	var str table
	strInputs := []string{
		"", "1", "1,2", "1,2,3", "-1,127,-128", "128", "-129", "1,,2", ",", "1,", ",1", " 1",
		"1 ", "a", "0x1", "+5", "007", "1_0", "1,2,x", "99999999999999999999", "١",
	}
	for _, s := range strInputs {
		v, err := stringutil.StrToInt8Slice(s)
		joined := ""
		for i, x := range v {
			if i > 0 {
				joined += " "
			}
			joined += strconv.Itoa(int(x))
		}
		str.row(s, strconv.FormatBool(v == nil), joined, errText(err))
	}
	str.write("stringutil.txt")

	// urlutil.txt: input, IsSafeHost.
	var u table
	urlInputs := []string{
		"example.com", "api-us", "example.com:8080", "", "127.0.0.1:6060/debug/pprof",
		"127.0.0.1:6060#", "example.com?x=1", "example.com@127.0.0.1", "http://127.0.0.1",
		"example.com:", ":8080", "a:1:2", "a:b", "example.com\n", "example.com:80\n", "\nexample.com",
		"ex ample.com", "exam_ple.com", "[::1]:80", "::1", "a.b.c.d-e", "-", ".", "a:0123456789",
		"éxample.com", "A-Z.a-z.0-9:65535", "host:99999999999999999999",
	}
	r := &splitmix64{state: 0x5EED_0006}
	for range 2000 {
		urlInputs = append(urlInputs, sweepString(r, "aZ9.-:/?#@_ \n\xc3\xa9", 8))
	}
	for _, s := range urlInputs {
		u.row(s, strconv.FormatBool(urlutil.IsSafeHost(s)))
	}
	u.write("urlutil.txt")

	// mathutil.txt: input bits, RoundTo4Decimals bits.
	var m table
	floats := []float64{
		123.456789, -123.456789, 123.4567, 123.45675, 123.45674, 0.00005, -0.00005, 0, 1.00005,
		2.00005, 0.12345, 1.23455, 1e300, -1e300, 5e-324, math.Inf(1), math.Inf(-1), math.NaN(),
		math.Copysign(0, -1), 922337203685477.5807, 0.1 + 0.2, 1.0 / 3, 2.5e-5, 4.99995,
	}
	r = &splitmix64{state: 0x5EED_0007}
	for range 2000 {
		switch r.intn(3) {
		case 0:
			floats = append(floats, math.Float64frombits(r.next()))
		case 1:
			floats = append(floats, float64(r.intn(100000000))/100000+0.00005)
		default:
			floats = append(floats, (float64(r.intn(2000000))-1000000)/float64(1+r.intn(1000)))
		}
	}
	for _, f := range floats {
		m.row(strconv.FormatUint(math.Float64bits(f), 16),
			strconv.FormatUint(math.Float64bits(mathutil.RoundTo4Decimals(f)), 16))
	}
	m.write("mathutil.txt")

	// uuid.txt: 16 random bytes, the V4 UUID gofrs/uuid makes from them.
	var id table
	r = &splitmix64{state: 0x5EED_0008}
	for range 300 {
		var b uuid.UUID
		for i := range b {
			b[i] = byte(r.next())
		}
		raw := hex.EncodeToString(b[:])
		b.SetVersion(uuid.V4)
		b.SetVariant(uuid.VariantRFC4122)
		id.row(raw, b.String())
	}
	id.write("uuid.txt")
}
