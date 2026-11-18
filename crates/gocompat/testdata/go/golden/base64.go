package main

import (
	"encoding/base64"
	"strconv"
	"strings"
)

var base64Inputs = []string{
	"", "Zg==", "Zm8=", "Zm9v", "Zg", "Zm8", "Zm9vYmFy", "Zm9vYmE=", "Zm9vYmE", "Zm9vYg==",
	"Zm9vYg", "Zg=", "Z===", "=", "==", "Zm9v\nYmFy", "Zm9v\r\nYmFy\r\n", "\n\n", "Zg=\n=",
	"Zg==Zg==", "Zm9vYmFy!", "Zm9v YmFy", "_-_-", "+/+/", "Zh==", "Zm9=", "eyJhIjoxfQ",
	"eyJhIjoxfQ==", "QUJD\n", "QQ\n=\n=", "QQ=\r\n", "A", "AA", "AAA", "AAAA", "AAAAA",
}

var base64Encodings = []struct {
	name string
	enc  *base64.Encoding
}{
	{"std", base64.StdEncoding},
	{"url", base64.URLEncoding},
	{"rawstd", base64.RawStdEncoding},
	{"rawurl", base64.RawURLEncoding},
}

var base64Alphabet = "ABZaz09+/-_=\n\r!. "

func genBase64() {
	var t table
	for _, e := range base64Encodings {
		for _, s := range base64Inputs {
			b, err := e.enc.DecodeString(s)
			t.row("decode", e.name, s, string(b), errText(err))
		}
		for _, s := range []string{"", "f", "fo", "foo", "foob", "fooba", "foobar", "\xff\xfe\xfd", "{\"uids\":{}}"} {
			t.row("encode", e.name, s, e.enc.EncodeToString([]byte(s)))
		}
	}
	t.write("base64.txt")

	sw := newSweep()
	rng := splitmix64{state: 3}
	for i := 0; i < 50000; i++ {
		n := int(rng.next() % 24)
		raw := make([]byte, n)
		var text strings.Builder
		for j := range raw {
			raw[j] = byte(rng.next())
			text.WriteByte(base64Alphabet[rng.next()%uint64(len(base64Alphabet))])
		}
		for _, e := range base64Encodings {
			b, err := e.enc.DecodeString(text.String())
			sw.add(e.enc.EncodeToString(raw) + "\t" + strconv.Quote(string(b)) + "\t" + errText(err))
		}
	}
	var sums table
	sums.row("base64_random", sw.sum())
	sums.write("base64_sweeps.txt")
}
