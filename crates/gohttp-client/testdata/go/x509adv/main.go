// Command x509adv writes testdata/golden/x509_adversarial.json, the adversarial server
// certificate verification goldens that pbs-gohttp-client's verifier replays
// (crates/gohttp-client/src/x509).
//
// x509.CreateCertificate refuses much of what a server can send, so these certificates are
// encoded by hand (der.go): name constraint values, certificate policies, keys and signature
// algorithms Go rejects or treats specially, encodings at the edges of Go's parser, cross-signed,
// looping and deep paths, and RSA-PSS parameters read by encoding/asn1. Each case records the
// error crypto/tls's client reports (verifyServerCertificate in crypto/tls/handshake_client.go)
// at the golden's time plus the case's nanos, or an empty error when the chain verifies. Keys and
// signatures are deterministic (keys.go), so every run writes the same bytes. Run it from the
// repository root with Go 1.26.2:
//
//	GOTOOLCHAIN=go1.26.2 go run ./x509adv
package main

import (
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/rsa"
	"crypto/x509"
	"encoding/json"
	"encoding/pem"
	"flag"
	"fmt"
	"log"
	"math/big"
	"os"
	"path/filepath"
	"time"
)

var now = time.Date(2026, 6, 1, 12, 0, 0, 0, time.UTC)

const day = 24 * time.Hour

type spec struct {
	name       string
	parent     string
	cn, org    string
	subject    []byte
	issuer     []byte
	version    int
	serial     int64
	from, to   int
	notBefore  []byte
	notAfter   []byte
	key        string
	keyOf      string
	spki       []byte
	alg        string
	ai         []byte
	exts       [][]byte
	corruptSig bool
	zeroSig    bool                // a serial whose signature starts with a zero byte
	sigEdit    func([]byte) []byte // re-encodes the signature
	noExtField bool
	rawDER     []byte
	exportKey  bool // writes the private key to the golden's keys
}

type built struct {
	der     []byte
	subject []byte
	key     *keyPair
}

var certs = map[string]*built{}
var order []string
var exported []string

func defaultAlg(k *keyPair) string {
	switch k.kind {
	case "ecdsa256":
		return "ecdsa-sha256"
	case "ecdsa384":
		return "ecdsa-sha384"
	case "ecdsa521":
		return "ecdsa-sha512"
	case "ed25519":
		return "ed25519"
	}
	return "rsa-sha256"
}

func make1(s spec) {
	if s.rawDER != nil {
		certs[s.name] = &built{der: s.rawDER}
		order = append(order, s.name)
		return
	}
	if s.version == 0 {
		s.version = 3
	}
	if s.serial == 0 {
		s.serial = int64(len(order) + 2)
	}
	if s.from == 0 && s.to == 0 {
		s.from, s.to = -1, 365
	}
	if s.cn == "" && s.subject == nil {
		s.cn = s.name
	}
	var key *keyPair
	switch {
	case s.keyOf != "":
		key = certs[s.keyOf].key
	default:
		kind := s.key
		if kind == "" {
			kind = "ecdsa256"
		}
		key = newKey(s.name, kind)
	}
	if s.exportKey {
		exported = append(exported, s.name)
	}
	subject := s.subject
	if subject == nil {
		subject = name(s.cn, s.org)
	}
	issuerName, issuerKey := subject, key
	if s.parent != "" {
		p, ok := certs[s.parent]
		if !ok {
			log.Fatalf("%s: no parent %s", s.name, s.parent)
		}
		issuerName, issuerKey = p.subject, p.key
	}
	if s.issuer != nil {
		issuerName = s.issuer
	}
	algName := s.alg
	if algName == "" {
		algName = defaultAlg(issuerKey)
	}
	alg := algs[algName]
	ai := alg.ai
	if s.ai != nil {
		ai = s.ai
	}
	nb, na := s.notBefore, s.notAfter
	if nb == nil {
		nb = timeDER(now.Add(time.Duration(s.from) * day))
	}
	if na == nil {
		na = timeDER(now.Add(time.Duration(s.to) * day))
	}
	spki := key.spki
	if s.spki != nil {
		spki = s.spki
	}
	tbsFor := func(serial int64) []byte {
		var tbsParts [][]byte
		if s.version > 1 {
			tbsParts = append(tbsParts, tlv(0xA0, integer(int64(s.version-1))))
		}
		tbsParts = append(tbsParts, bigInteger(big.NewInt(serial)), ai, issuerName, seq(nb, na), subject, spki)
		if s.version == 3 && !s.noExtField && len(s.exts) > 0 {
			tbsParts = append(tbsParts, tlv(0xA3, seq(s.exts...)))
		}
		return seq(tbsParts...)
	}
	serial := s.serial
	tbs := tbsFor(serial)
	sig := sign(issuerKey, alg, tbs)
	for s.zeroSig && sig[0] != 0 {
		serial++
		tbs = tbsFor(serial)
		sig = sign(issuerKey, alg, tbs)
	}
	if s.corruptSig {
		sig[len(sig)/2] ^= 0x01
	}
	if s.sigEdit != nil {
		sig = s.sigEdit(sig)
	}
	der := seq(tbs, ai, bitString(sig))
	certs[s.name] = &built{der: der, subject: subject, key: key}
	order = append(order, s.name)
}

func caExts(extra ...[]byte) [][]byte {
	return append([][]byte{bcExt(true, true, -1), kuExt(0, 5, 6)}, extra...)
}

func leafExts(extra ...[]byte) [][]byte {
	return append([][]byte{kuExt(0), ekuExt(ekuServer), sanExt(false, dnsName("localhost"), ipName("127.0.0.1"))}, extra...)
}

func leafNames(names ...[]byte) [][]byte {
	return [][]byte{kuExt(0), ekuExt(ekuServer), sanExt(false, names...)}
}

type verifyCase struct {
	Name  string   `json:"name"`
	Roots []string `json:"roots"`
	Chain []string `json:"chain"`
	Host  string   `json:"host"`
	Error string   `json:"error"`
	Nanos int64    `json:"nanos,omitempty"`
}

func one(name string, roots []string, host string, chain ...string) verifyCase {
	return verifyCase{Name: name, Roots: roots, Chain: chain, Host: host}
}

func pemOf(n string) []byte {
	return pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: certs[n].der})
}

func goVerify(c verifyCase) string {
	var parsed []*x509.Certificate
	for _, n := range c.Chain {
		cert, err := x509.ParseCertificate(certs[n].der)
		if err != nil {
			return "tls: failed to parse certificate from server: " + err.Error()
		}
		if cert.PublicKeyAlgorithm == x509.RSA {
			if cert.PublicKey.(*rsa.PublicKey).N.BitLen() > 8192 {
				return "tls: server sent certificate containing RSA key larger than 8192 bits"
			}
		}
		parsed = append(parsed, cert)
	}
	roots := x509.NewCertPool()
	for _, n := range c.Roots {
		roots.AppendCertsFromPEM(pemOf(n))
	}
	inter := x509.NewCertPool()
	for _, cert := range parsed[1:] {
		inter.AddCert(cert)
	}
	opts := x509.VerifyOptions{Roots: roots, CurrentTime: now.Add(time.Duration(c.Nanos)), DNSName: c.Host, Intermediates: inter}
	if _, err := parsed[0].Verify(opts); err != nil {
		return "tls: failed to verify certificate: " + err.Error()
	}
	switch parsed[0].PublicKey.(type) {
	case *rsa.PublicKey, *ecdsa.PublicKey, ed25519.PublicKey:
	default:
		return fmt.Sprintf("tls: server's certificate contains an unsupported type of public key: %T", parsed[0].PublicKey)
	}
	return ""
}

func main() {
	out := flag.String("out", "../golden/x509_adversarial.json", "output file")
	flag.Parse()
	buildSpecs()
	cases := buildCases()
	for i := range cases {
		cases[i].Error = goVerify(cases[i])
	}
	pems := map[string]string{}
	for _, n := range order {
		pems[n] = string(pemOf(n))
	}
	keys := map[string]string{}
	for _, n := range exported {
		der, err := x509.MarshalPKCS8PrivateKey(certs[n].key.priv)
		if err != nil {
			log.Fatal(err)
		}
		keys[n] = string(pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: der}))
	}
	g := struct {
		Certs map[string]string `json:"certs"`
		Keys  map[string]string `json:"keys"`
		Cases []verifyCase      `json:"cases"`
		Now   int64             `json:"now"`
	}{pems, keys, cases, now.Unix()}
	data, err := json.MarshalIndent(g, "", " ")
	if err != nil {
		log.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(*out), 0o750); err != nil {
		log.Fatal(err)
	}
	if err := os.WriteFile(*out, append(data, '\n'), 0o600); err != nil {
		log.Fatal(err)
	}
	fmt.Println(len(cases), "cases")
}
