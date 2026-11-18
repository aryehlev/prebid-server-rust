// Command x509golden writes testdata/golden/x509.json, the server certificate verification
// goldens that pbs-gohttp-client's verifier replays (crates/gohttp-client/src/x509).
//
// Each case is a server certificate chain (leaf first), a root pool and a server name. The golden
// records the error crypto/tls's client reports for it (verifyServerCertificate in
// crypto/tls/handshake_client.go): the parse error of a certificate that does not parse, else the
// error of crypto/x509's Certificate.Verify with the options the client passes, at a fixed time.
// An empty error means the chain verifies. Keys are derived from the certificate names and ECDSA
// signs deterministically (RFC 6979), so every run writes the same bytes. Run it from the
// directory of its module (crates/gohttp-client/testdata/go) with Go 1.26.2:
//
//	GOTOOLCHAIN=go1.26.2 go run ./x509
package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/sha256"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/asn1"
	"encoding/json"
	"encoding/pem"
	"errors"
	"flag"
	"fmt"
	"log"
	"math/big"
	"net"
	"os"
	"path/filepath"
	"time"
)

const (
	rootCA    = "root"
	interCA   = "inter"
	localhost = "localhost"
	loopback  = "127.0.0.1"
	malformed = "malformed"
	expiredCA = "root_expired"
	day       = 24 * time.Hour
)

var errNoParent = errors.New("parent certificate not created yet")

// anyPolicyCritical is a critical certificatePolicies extension listing anyPolicy (2.5.29.32.0).
// Go handles the extension; rustls-webpki rejects it when critical, except in a trust anchor.
var anyPolicyCritical = pkix.Extension{
	Id:       asn1.ObjectIdentifier{2, 5, 29, 32},
	Critical: true,
	Value:    []byte{0x30, 0x08, 0x30, 0x06, 0x06, 0x04, 0x55, 0x1d, 0x20, 0x00},
}

// emptyEKU is an extKeyUsage extension that lists no usage, which Go reads as no restriction.
var emptyEKU = pkix.Extension{Id: asn1.ObjectIdentifier{2, 5, 29, 37}, Value: []byte{0x30, 0x00}}

// spec describes one certificate. Validity is in days relative to the golden's time.
type spec struct {
	name       string
	parent     string
	commonName string
	keyOf      string
	dns        []string
	ips        []string
	eku        []x509.ExtKeyUsage
	keyUsage   x509.KeyUsage
	extra      []pkix.Extension
	policies   []string
	permitted  []string
	from, to   int
	pathLen    int
	isCA       bool
	notCA      bool
	noBC       bool
	critical   bool
}

// verifyCase is one verification: a chain, a pool and the server name.
type verifyCase struct {
	Name  string   `json:"name"`
	Roots []string `json:"roots"`
	Chain []string `json:"chain"`
	Host  string   `json:"host"`
	Error string   `json:"error"`
}

// golden is the whole file.
type golden struct {
	Certs map[string]string `json:"certs"`
	Cases []verifyCase      `json:"cases"`
	Now   int64             `json:"now"`
}

// issued holds the certificates made so far and their keys.
type issued struct {
	certs map[string]*x509.Certificate
	keys  map[string]*ecdsa.PrivateKey
	pems  map[string]string
}

func leafSpec(name, parent string, dns, ips []string) spec {
	return spec{
		name: name, parent: parent, commonName: name, dns: dns, ips: ips, from: -1, to: 3650,
		eku: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth}, pathLen: -1,
	}
}

func localLeaf(name, parent string) spec {
	return leafSpec(name, parent, []string{localhost}, []string{loopback})
}

func caSpec(name, parent string, from, to int) spec {
	return spec{name: name, parent: parent, commonName: name, isCA: true, from: from, to: to, pathLen: -1}
}

func leafSpecs() []spec {
	expired := localLeaf("expired", rootCA)
	expired.from, expired.to = -2000, -10
	notYet := localLeaf("notyet", rootCA)
	notYet.from, notYet.to = 30, 400
	cnOnly := leafSpec("cnonly", rootCA, nil, nil)
	cnOnly.commonName = localhost
	cnOnlyIP := leafSpec("cnonly_ip", rootCA, nil, nil)
	cnOnlyIP.commonName = loopback
	cnWildcard := leafSpec("cn_wildcard", rootCA, nil, nil)
	cnWildcard.commonName = "*.example.com"
	clientAuth := localLeaf("clientauth", rootCA)
	clientAuth.eku = []x509.ExtKeyUsage{x509.ExtKeyUsageClientAuth}
	anyEKU := localLeaf("anyeku", rootCA)
	anyEKU.eku = []x509.ExtKeyUsage{x509.ExtKeyUsageAny}
	noEKU := localLeaf("noeku", rootCA)
	noEKU.eku = nil
	critical := localLeaf("critical", rootCA)
	critical.critical = true
	leafCA := localLeaf("leaf_ca", rootCA)
	leafCA.isCA = true
	selfCA := localLeaf("self_ca", "")
	selfCA.isCA = true
	policiesCritical := localLeaf("policies_critical", rootCA)
	policiesCritical.extra = []pkix.Extension{anyPolicyCritical}
	// A UUID policy OID (RFC 4122 section 4.4 arc 2.25): Go parses arcs of any size.
	uuidPolicy := localLeaf("policy_uuid", rootCA)
	uuidPolicy.policies = []string{"2.25.329800735698586629295641978511506172918"}
	noUsages := localLeaf("empty_eku", rootCA)
	noUsages.extra = []pkix.Extension{emptyEKU}

	return []spec{
		localLeaf("ok", rootCA),
		leafSpec("other", rootCA, []string{"other.test", "*.other.test"}, []string{"10.0.0.1"}),
		leafSpec("dnsonly", rootCA, []string{"other.test"}, nil),
		leafSpec("wildcard", rootCA, []string{"*.example.com"}, nil),
		leafSpec("ipv6", rootCA, nil, []string{"::1", "2001:db8::1"}),
		leafSpec("nonames", rootCA, nil, nil),
		expired, notYet, cnOnly, cnOnlyIP, cnWildcard, clientAuth, anyEKU, noEKU, critical, leafCA,
		selfCA, localLeaf("self_leaf", ""), policiesCritical, uuidPolicy, noUsages,
	}
}

func chainSpecs() []spec {
	clientInter := caSpec("inter_clientauth", rootCA, -10, 3650)
	clientInter.eku = []x509.ExtKeyUsage{x509.ExtKeyUsageClientAuth}
	notCAInter := caSpec("inter_notca", rootCA, -10, 3650)
	notCAInter.isCA, notCAInter.notCA = false, true
	noCertSign := caSpec("inter_nocertsign", rootCA, -10, 3650)
	noCertSign.keyUsage = x509.KeyUsageDigitalSignature
	criticalInter := caSpec("inter_critical", rootCA, -10, 3650)
	criticalInter.critical = true
	renewed := caSpec("root_renewed", "", -100, 4000)
	renewed.commonName, renewed.keyOf = expiredCA, expiredCA
	pathLen0 := caSpec("root_pathlen0", "", -10, 3650)
	pathLen0.pathLen = 0
	clientRoot := caSpec("root_clientauth", "", -10, 3650)
	clientRoot.eku = []x509.ExtKeyUsage{x509.ExtKeyUsageClientAuth}
	noBCRoot := caSpec("root_nobc", "", -10, 3650)
	noBCRoot.noBC = true
	policiesInter := caSpec("inter_policies_critical", rootCA, -10, 3650)
	policiesInter.extra = []pkix.Extension{anyPolicyCritical}
	policiesRoot := caSpec("root_policies_critical", "", -10, 3650)
	policiesRoot.extra = []pkix.Extension{anyPolicyCritical}
	constrained := caSpec("inter_constrained", rootCA, -10, 3650)
	constrained.permitted = []string{"example.com"}

	return []spec{
		caSpec(interCA, rootCA, -10, 3650), localLeaf("leaf_inter", interCA),
		clientInter, localLeaf("leaf_inter_clientauth", "inter_clientauth"),
		caSpec("inter_expired", rootCA, -2000, -5), localLeaf("leaf_inter_expired", "inter_expired"),
		notCAInter, localLeaf("leaf_inter_notca", "inter_notca"),
		noCertSign, localLeaf("leaf_inter_nocertsign", "inter_nocertsign"),
		criticalInter, localLeaf("leaf_inter_critical", "inter_critical"),
		caSpec(expiredCA, "", -4000, -100), renewed, localLeaf("leaf_root_expired", expiredCA),
		pathLen0, caSpec("inter_pathlen0", "root_pathlen0", -10, 3650),
		localLeaf("leaf_pathlen0", "inter_pathlen0"),
		clientRoot, localLeaf("leaf_root_clientauth", "root_clientauth"),
		noBCRoot, localLeaf("leaf_root_nobc", "root_nobc"),
		policiesInter, localLeaf("leaf_inter_policies_critical", "inter_policies_critical"),
		policiesRoot, localLeaf("leaf_root_policies_critical", "root_policies_critical"),
		constrained, localLeaf("leaf_inter_constrained", "inter_constrained"),
	}
}

func specs() []spec {
	sameName := caSpec("root_samename", "", -10, 3650)
	sameName.commonName = rootCA
	all := []spec{caSpec(rootCA, "", -1000, 7300), caSpec("other_root", "", -1000, 7300), sameName}
	all = append(all, leafSpecs()...)

	return append(all, chainSpecs()...)
}

// key returns the certificate's P-256 key: keyOf's, or one derived from the certificate's name.
func (is *issued) key(s spec) (*ecdsa.PrivateKey, error) {
	if s.keyOf != "" {
		return is.keys[s.keyOf], nil
	}

	scalar := sha256.Sum256([]byte("pbs-gohttp-client x509 golden key " + s.name))

	key, err := ecdsa.ParseRawPrivateKey(elliptic.P256(), scalar[:])
	if err != nil {
		return nil, fmt.Errorf("derive key: %w", err)
	}

	return key, nil
}

func template(s spec, serial int64, now time.Time) (*x509.Certificate, error) {
	tmpl := &x509.Certificate{
		SerialNumber: big.NewInt(serial),
		Subject:      pkix.Name{CommonName: s.commonName},
		NotBefore:    now.Add(time.Duration(s.from) * day),
		NotAfter:     now.Add(time.Duration(s.to) * day),
		DNSNames:     s.dns,
		ExtKeyUsage:  s.eku,
		KeyUsage:     x509.KeyUsageDigitalSignature,
		MaxPathLen:   -1,
	}

	if s.isCA || s.notCA {
		tmpl.KeyUsage |= x509.KeyUsageCertSign
		tmpl.BasicConstraintsValid = !s.noBC
		tmpl.IsCA = s.isCA
	}

	if s.keyUsage != 0 {
		tmpl.KeyUsage = s.keyUsage
	}

	if s.pathLen >= 0 {
		tmpl.MaxPathLen, tmpl.MaxPathLenZero = s.pathLen, s.pathLen == 0
	}

	for _, ip := range s.ips {
		tmpl.IPAddresses = append(tmpl.IPAddresses, net.ParseIP(ip))
	}

	if s.critical {
		tmpl.ExtraExtensions = []pkix.Extension{{Id: asn1.ObjectIdentifier{1, 2, 3, 4}, Critical: true, Value: []byte{5, 0}}}
	}

	tmpl.ExtraExtensions = append(tmpl.ExtraExtensions, s.extra...)
	tmpl.PermittedDNSDomains = s.permitted

	for _, text := range s.policies {
		oid, err := x509.ParseOID(text)
		if err != nil {
			return nil, fmt.Errorf("policy %s: %w", text, err)
		}

		tmpl.Policies = append(tmpl.Policies, oid)
	}

	return tmpl, nil
}

func (is *issued) make(s spec, serial int64, now time.Time) error {
	key, err := is.key(s)
	if err != nil {
		return err
	}

	tmpl, err := template(s, serial, now)
	if err != nil {
		return fmt.Errorf("%s: %w", s.name, err)
	}

	parent, parentKey := tmpl, key

	if s.parent != "" {
		var ok bool

		parent, ok = is.certs[s.parent]
		if !ok {
			return fmt.Errorf("%s: %w", s.name, errNoParent)
		}

		parentKey = is.keys[s.parent]
	}

	// A nil rand makes ECDSA sign deterministically (RFC 6979), so the golden is reproducible.
	der, err := x509.CreateCertificate(nil, tmpl, parent, &key.PublicKey, parentKey)
	if err != nil {
		return fmt.Errorf("%s: %w", s.name, err)
	}

	cert, err := x509.ParseCertificate(der)
	if err != nil {
		return fmt.Errorf("%s: %w", s.name, err)
	}

	is.certs[s.name], is.keys[s.name] = cert, key
	is.pems[s.name] = string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}))

	return nil
}

func one(name string, roots []string, host string, chain ...string) verifyCase {
	return verifyCase{Name: name, Roots: roots, Chain: chain, Host: host}
}

func leafCases() []verifyCase {
	root := []string{rootCA}

	return []verifyCase{
		one("ok_dns", root, localhost, "ok"),
		one("ok_ip", root, loopback, "ok"),
		one("ok_upper_case", root, "LOCALHOST", "ok"),
		one("ok_trailing_dot", root, "localhost.", "ok"),
		one("empty_pool", []string{}, loopback, "ok"),
		one("other_root", []string{"other_root"}, loopback, "ok"),
		one("same_name_other_key", []string{"root_samename"}, loopback, "ok"),
		one("expired", root, loopback, "expired"),
		one("not_yet_valid", root, loopback, "notyet"),
		one("dns_mismatch", root, localhost, "other"),
		one("ip_mismatch", root, loopback, "other"),
		one("no_ip_sans", root, loopback, "dnsonly"),
		one("no_names", root, localhost, "nonames"),
		one("wildcard_match", root, "a.example.com", "wildcard"),
		one("wildcard_two_labels", root, "a.b.example.com", "wildcard"),
		one("ipv6_match", root, "2001:db8::1", "ipv6"),
		one("ipv6_mismatch", root, "2001:db8::2", "ipv6"),
		one("legacy_cn_dns", root, localhost, "cnonly"),
		one("legacy_cn_ip", root, loopback, "cnonly_ip"),
		one("legacy_cn_wildcard", root, "a.example.com", "cn_wildcard"),
		one("cn_other_host", root, "example.org", "cnonly"),
		one("client_auth_eku", root, loopback, "clientauth"),
		one("any_eku", root, loopback, "anyeku"),
		one("no_eku", root, loopback, "noeku"),
		one("unhandled_critical", root, loopback, "critical"),
		one("leaf_is_ca", root, loopback, "leaf_ca"),
		one("expired_and_mismatch", root, "other.test", "expired"),
		one(malformed, root, loopback, malformed),
		one("leaf_policies_critical", root, loopback, "policies_critical"),
		one("leaf_policy_uuid", root, loopback, "policy_uuid"),
		one("leaf_empty_eku", root, loopback, "empty_eku"),
	}
}

func chainCases() []verifyCase {
	root := []string{rootCA}

	return []verifyCase{
		one("chain", root, loopback, "leaf_inter", interCA),
		one("chain_missing_intermediate", root, loopback, "leaf_inter"),
		one("intermediate_in_pool", []string{interCA}, loopback, "leaf_inter"),
		one("intermediate_client_auth", root, loopback, "leaf_inter_clientauth", "inter_clientauth"),
		one("intermediate_expired", root, loopback, "leaf_inter_expired", "inter_expired"),
		one("intermediate_not_ca", root, loopback, "leaf_inter_notca", "inter_notca"),
		one("intermediate_no_cert_sign", root, loopback, "leaf_inter_nocertsign", "inter_nocertsign"),
		one("intermediate_unhandled_critical", root, loopback, "leaf_inter_critical", "inter_critical"),
		one("intermediate_malformed", root, loopback, "leaf_inter", malformed),
		one(expiredCA, []string{expiredCA}, loopback, "leaf_root_expired"),
		one("root_expired_and_renewed", []string{expiredCA, "root_renewed"}, loopback, "leaf_root_expired"),
		one("root_path_len", []string{"root_pathlen0"}, loopback, "leaf_pathlen0", "inter_pathlen0"),
		one("root_client_auth", []string{"root_clientauth"}, loopback, "leaf_root_clientauth"),
		one("root_no_basic_constraints", []string{"root_nobc"}, loopback, "leaf_root_nobc"),
		one("intermediate_policies_critical", root, loopback, "leaf_inter_policies_critical", "inter_policies_critical"),
		one("root_policies_critical", []string{"root_policies_critical"}, loopback, "leaf_root_policies_critical"),
		one("intermediate_name_constraints", root, loopback, "leaf_inter_constrained", "inter_constrained"),
	}
}

func pinnedCases() []verifyCase {
	none := []string{}

	return []verifyCase{
		one("pinned_self_signed_ca", []string{"self_ca"}, localhost, "self_ca"),
		one("pinned_self_signed_leaf", []string{"self_leaf"}, localhost, "self_leaf"),
		one("pinned_leaf", []string{"ok"}, loopback, "ok"),
		one("pinned_expired", []string{"expired"}, loopback, "expired"),
		one("pinned_mismatch", []string{"other"}, localhost, "other"),
		one("pinned_client_auth", []string{"clientauth"}, loopback, "clientauth"),
		one("untrusted_self_signed_ca", none, localhost, "self_ca"),
		one("untrusted_self_signed_leaf", none, localhost, "self_leaf"),
		one("untrusted_dns_mismatch", none, localhost, "other"),
		one("untrusted_ip_mismatch", none, loopback, "other"),
		one("untrusted_expired", none, loopback, "expired"),
		one("untrusted_client_auth", none, loopback, "clientauth"),
		one("untrusted_legacy_cn", none, localhost, "cnonly"),
	}
}

// verify mirrors verifyServerCertificate: parse every certificate, then Verify the leaf.
func (is *issued) verify(c verifyCase, now time.Time) string {
	certs := make([]*x509.Certificate, 0, len(c.Chain))

	for _, name := range c.Chain {
		block, _ := pem.Decode([]byte(is.pems[name]))

		cert, err := x509.ParseCertificate(block.Bytes)
		if err != nil {
			return "tls: failed to parse certificate from server: " + err.Error()
		}

		certs = append(certs, cert)
	}

	roots := x509.NewCertPool()
	for _, name := range c.Roots {
		roots.AddCert(is.certs[name])
	}

	inter := x509.NewCertPool()
	for _, cert := range certs[1:] {
		inter.AddCert(cert)
	}

	opts := x509.VerifyOptions{Roots: roots, CurrentTime: now, DNSName: c.Host, Intermediates: inter}
	if _, err := certs[0].Verify(opts); err != nil {
		return "tls: failed to verify certificate: " + err.Error()
	}

	return ""
}

func build(now time.Time) (golden, error) {
	is := &issued{certs: map[string]*x509.Certificate{}, keys: map[string]*ecdsa.PrivateKey{}, pems: map[string]string{}}
	for i, s := range specs() {
		if err := is.make(s, int64(i+2), now); err != nil {
			return golden{}, err
		}
	}

	is.pems[malformed] = string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: []byte("not a certificate")}))

	cases := append(append(leafCases(), chainCases()...), pinnedCases()...)
	g := golden{Now: now.Unix(), Certs: is.pems, Cases: cases}

	for i := range g.Cases {
		g.Cases[i].Error = is.verify(g.Cases[i], now)
	}

	return g, nil
}

func main() {
	out := flag.String("out", "../golden/x509.json", "output file")

	flag.Parse()

	g, err := build(time.Date(2026, 6, 1, 12, 0, 0, 0, time.UTC))
	if err != nil {
		log.Fatal(err)
	}

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
}
