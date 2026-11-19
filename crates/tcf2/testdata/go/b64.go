package main

import (
	"encoding/base64"
	"encoding/hex"
	"math/rand"
	"unicode/utf8"
)

// base64Case records the two ways go-gdpr and prebid-server decode a consent string:
//   - RawURL: base64.RawURLEncoding.DecodeString (adapters/flipp)
//   - RawURLInPlace: base64.RawURLEncoding.Decode(buf, buf) as tcf1/tcf2 ParseString do
type base64Case struct {
	InputHex      string      `json:"input_hex,omitempty"`
	Input         *string     `json:"input,omitempty"`
	RawURL        bytesResult `json:"raw_url"`
	RawURLInPlace bytesResult `json:"raw_url_in_place"`
}

func buildBase64Cases() []base64Case {
	r := rand.New(rand.NewSource(99))
	alphabets := []string{
		"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_",
		"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
		"AQgw-_+/=\n\r .",
		"AB=\n",
		"A_\r=",
	}
	inputs := []string{
		"", "=", "==", "===", "====", "A", "AA", "AAA", "AAAA", "AA==", "AAA=", "A===", "AA=A",
		"AA==\n", "AA=\n=", "AA=\r\n=", "AA==A", "AAAA\n", "\nAAAA", "A\nA\nA\nA", "A\n", "AA\n\n",
		"QQ", "QR", "QR==", "QUI", "QUJ", "QUJ=", "/+/+", "-_-_", "\x00", "\xff", "AAAAAAAA", "AAAAAAA",
		"AAAAAAAAA", "AAAAAAAAAAAAAAAA\n", "AAAAAAA\nAAAAAAAA", "AAAAAAAAAAA=AAAA", "Zm9vYmFy", "Zm9vYmE=",
		"Zm9vYg==", "Zm9vYg", "Zm9vYmFyZm9vYmFy", "Zm9vYmFyZm9vYmF", "Zm9vYmFyZm9vYm", "Zm9vYmFyZm9vY",
	}
	for i := 0; i < 1400; i++ {
		alpha := alphabets[r.Intn(len(alphabets))]
		n := r.Intn(26)
		buf := make([]byte, n)
		for j := range buf {
			buf[j] = alpha[r.Intn(len(alpha))]
		}
		inputs = append(inputs, string(buf))
	}
	var out []base64Case
	seen := map[string]bool{}
	for _, in := range inputs {
		if seen[in] {
			continue
		}
		seen[in] = true
		c := base64Case{}
		c.Input, c.InputHex = encodeInput([]byte(in))
		dec, err := base64.RawURLEncoding.DecodeString(in)
		c.RawURL = bytesOrErr(dec, err)

		buf := []byte(in)
		n, err := base64.RawURLEncoding.Decode(buf, buf)
		c.RawURLInPlace = bytesOrErr(buf[:n], err)
		out = append(out, c)
	}
	return out
}

// encodeInput stores valid UTF-8 inputs as text and anything else as hex.
func encodeInput(b []byte) (*string, string) {
	if utf8.Valid(b) {
		s := string(b)
		return &s, ""
	}
	return nil, hex.EncodeToString(b)
}

// bytesOrErr drops the partial output Go returns alongside an error: no caller in go-gdpr or
// prebid-server looks at it.
func bytesOrErr(b []byte, err error) bytesResult {
	if err != nil {
		return bytesResult{Err: err.Error()}
	}
	return bytesResult{Hex: hex.EncodeToString(b), Len: len(b)}
}
