package main

import (
	"crypto/ed25519"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/hex"
	"encoding/pem"
	"math/big"
	"strings"
	"time"
)

// certFileCase is one certificates_file content and the certificates x509.CertPool's
// AppendCertsFromPEM takes from it (server/ssl/ssl.go:15-31, called by router/router.go:137 for
// the general client's roots).
type certFileCase struct {
	Name string `json:"name"`
	PEM  string `json:"pem"`
	// Accepted names the certificates of Certs the pool trusts afterwards, sorted, and Ok is
	// AppendCertsFromPEM's result.
	Accepted []string `json:"accepted"`
	Ok       bool     `json:"ok"`
}

type certFileDoc struct {
	Header header `json:"header"`
	// Certs are the certificates the cases use, by common name, as DER in hex.
	Certs map[string]string `json:"certs"`
	Cases []certFileCase    `json:"cases"`
}

// fixedCert is a self-signed CA certificate for cn whose Ed25519 key comes from seed, so the
// certificate's bytes are the same on every run.
func fixedCert(cn string, seed byte) []byte {
	key := ed25519.NewKeyFromSeed([]byte(strings.Repeat(string(rune(seed)), ed25519.SeedSize)))
	tmpl := &x509.Certificate{
		SerialNumber:          big.NewInt(int64(seed)),
		Subject:               pkix.Name{CommonName: cn},
		NotBefore:             time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
		NotAfter:              time.Date(2036, 1, 1, 0, 0, 0, 0, time.UTC),
		IsCA:                  true,
		BasicConstraintsValid: true,
		KeyUsage:              x509.KeyUsageCertSign,
	}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, key.Public(), key)
	if err != nil {
		panic(err)
	}
	return der
}

func certificatesFileGolden() certFileDoc {
	a := fixedCert("cert-a", 'a')
	b := fixedCert("cert-b", 'b')
	pemA := string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: a}))
	pemB := string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: b}))
	withHeaders := string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Headers: map[string]string{"Proc-Type": "4,ENCRYPTED"}, Bytes: a}))
	badDER := string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: []byte("not a certificate")}))
	privateKey := string(pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: []byte("not a key")}))
	badBase64 := "-----BEGIN CERTIFICATE-----\n" + "@@@@" + base64.StdEncoding.EncodeToString(a)[4:] + "\n-----END CERTIFICATE-----\n"
	noEnd := strings.TrimSuffix(pemA, "-----END CERTIFICATE-----\n")
	crlf := strings.ReplaceAll(pemA, "\n", "\r\n")
	indented := "  " + strings.ReplaceAll(strings.TrimSuffix(pemA, "\n"), "\n", "\n  ") + "\n"
	// Blanks inside the base64 lines, which Decode drops before it decodes.
	bodyLines := strings.Split(strings.TrimSuffix(pemA, "\n"), "\n")
	for i := 1; i < len(bodyLines)-1; i++ {
		bodyLines[i] = bodyLines[i][:8] + " \t " + bodyLines[i][8:]
	}
	blanksInBody := strings.Join(bodyLines, "\n") + "\n"
	// The last base64 digit before the padding with its unused low bits set, which
	// base64.StdEncoding (not Strict) ignores.
	trailingBits := ""
	for _, der := range [][]byte{a, b} {
		enc := base64.StdEncoding.EncodeToString(der)
		pad := len(enc) - len(strings.TrimRight(enc, "="))
		if pad == 0 {
			continue
		}
		const digits = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
		last := len(enc) - pad - 1
		value := strings.IndexByte(digits, enc[last]) | (1<<(2*pad) - 1)
		enc = enc[:last] + string(digits[value]) + enc[last+1:]
		var lines []string
		for len(enc) > 64 {
			lines, enc = append(lines, enc[:64]), enc[64:]
		}
		lines = append(lines, enc)
		trailingBits = "-----BEGIN CERTIFICATE-----\n" + strings.Join(lines, "\n") + "\n-----END CERTIFICATE-----\n"
		break
	}
	if trailingBits == "" {
		panic("no certificate needs base64 padding")
	}
	// The URL-safe alphabet, which base64.StdEncoding refuses.
	if !strings.ContainsAny(pemA, "+/") {
		panic("cert-a's base64 has no + or /")
	}
	urlSafe := strings.NewReplacer("+", "-", "/", "_").Replace(pemA)
	trusted := "-----BEGIN TRUSTED CERTIFICATE-----\n" + strings.Join(strings.Split(strings.TrimSpace(strings.Split(pemA, "\n-----END")[0]), "\n")[1:], "\n") + "\n-----END TRUSTED CERTIFICATE-----\n"
	cases := []certFileCase{
		{Name: "two_certs", PEM: pemA + pemB},
		{Name: "text_around", PEM: "issuer list\n" + pemA + "\nnext one:\n" + pemB + "trailing words"},
		{Name: "duplicate", PEM: pemA + pemA},
		{Name: "headers_block", PEM: withHeaders + pemB},
		{Name: "private_key_block", PEM: privateKey + pemB},
		{Name: "bad_der", PEM: badDER + pemB},
		{Name: "bad_base64", PEM: badBase64 + pemB},
		{Name: "missing_end", PEM: pemB + noEnd},
		{Name: "crlf", PEM: crlf},
		{Name: "indented", PEM: indented},
		{Name: "trusted_certificate", PEM: trusted + pemB},
		{Name: "empty", PEM: ""},
		// pem.Decode's edges: what may follow the END line, which BEGIN line a block takes, where
		// the lines may start and end, and how strictly the body's base64 is read.
		{Name: "end_line_junk", PEM: strings.Replace(pemA, "END CERTIFICATE-----", "END CERTIFICATE----- junk", 1) + pemB},
		{Name: "two_begins", PEM: "-----BEGIN CERTIFICATE-----\n" + pemA},
		{Name: "begin_mid_line", PEM: "x" + pemA},
		{Name: "trailing_blanks", PEM: strings.ReplaceAll(pemA, "-----\n", "----- \t\n")},
		{Name: "blanks_in_body", PEM: blanksInBody},
		{Name: "lower_case_type", PEM: strings.ReplaceAll(pemA, "CERTIFICATE", "certificate") + pemB},
		{Name: "trailing_bits", PEM: trailingBits},
		{Name: "no_final_newline", PEM: strings.TrimSuffix(pemA, "\n")},
		{Name: "end_not_on_own_line", PEM: strings.Replace(pemA, "\n-----END", "-----END", 1) + pemB},
		{Name: "url_safe_base64", PEM: urlSafe},
	}
	certs := map[string][]byte{"cert-a": a, "cert-b": b}
	names := []string{"cert-a", "cert-b"}
	for i, c := range cases {
		pool := x509.NewCertPool()
		cases[i].Ok = pool.AppendCertsFromPEM([]byte(c.PEM))
		// Each certificate is a self-signed CA, so it verifies against the pool exactly when the
		// pool holds it.
		accepted := []string{}
		for _, name := range names {
			cert, err := x509.ParseCertificate(certs[name])
			if err != nil {
				panic(err)
			}
			opts := x509.VerifyOptions{Roots: pool, CurrentTime: time.Date(2027, 1, 1, 0, 0, 0, 0, time.UTC)}
			if _, err := cert.Verify(opts); err == nil {
				accepted = append(accepted, name)
			}
		}
		cases[i].Accepted = accepted
	}
	return certFileDoc{Header: newHeader(), Certs: map[string]string{"cert-a": hex.EncodeToString(a), "cert-b": hex.EncodeToString(b)}, Cases: cases}
}
