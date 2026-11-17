package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"math/big"
	"net"
	"time"
)

// pki is a test CA and the server certificates it signed.
type pki struct {
	caPEM string
	pool  *x509.CertPool
	certs map[string]Cert
	tls   map[string]tls.Certificate
}

// leafSpec describes one server certificate.
type leafSpec struct {
	name      string
	dns       []string
	ips       []string
	notBefore time.Time
	notAfter  time.Time
}

func newPKI() (*pki, error) {
	caKey, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return nil, err
	}
	now := time.Now().UTC().Truncate(time.Second)
	caTmpl := &x509.Certificate{
		SerialNumber:          big.NewInt(1),
		Subject:               pkix.Name{CommonName: "pbs gohttp-client test CA"},
		NotBefore:             time.Date(2019, 1, 1, 0, 0, 0, 0, time.UTC),
		NotAfter:              now.AddDate(20, 0, 0),
		IsCA:                  true,
		BasicConstraintsValid: true,
		KeyUsage:              x509.KeyUsageCertSign | x509.KeyUsageDigitalSignature,
	}
	caDER, err := x509.CreateCertificate(rand.Reader, caTmpl, caTmpl, &caKey.PublicKey, caKey)
	if err != nil {
		return nil, err
	}
	ca, err := x509.ParseCertificate(caDER)
	if err != nil {
		return nil, err
	}
	p := &pki{
		caPEM: string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: caDER})),
		pool:  x509.NewCertPool(),
		certs: map[string]Cert{},
		tls:   map[string]tls.Certificate{},
	}
	p.pool.AddCert(ca)
	valid := func(name string, dns, ips []string) leafSpec {
		return leafSpec{name: name, dns: dns, ips: ips, notBefore: now.AddDate(0, 0, -1), notAfter: now.AddDate(10, 0, 0)}
	}
	specs := []leafSpec{
		valid("ip", nil, []string{"127.0.0.1"}),
		valid("origin", []string{"origin.test"}, nil),
		valid("dnsonly", []string{"other.test"}, nil),
		valid("otherip", nil, []string{"10.0.0.1"}),
		{
			name:      "expired",
			ips:       []string{"127.0.0.1"},
			notBefore: time.Date(2019, 1, 1, 0, 0, 0, 0, time.UTC),
			notAfter:  time.Date(2020, 1, 1, 0, 0, 0, 0, time.UTC),
		},
	}
	for i, s := range specs {
		key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
		if err != nil {
			return nil, err
		}
		tmpl := &x509.Certificate{
			SerialNumber: big.NewInt(int64(i + 2)),
			Subject:      pkix.Name{CommonName: s.name},
			NotBefore:    s.notBefore,
			NotAfter:     s.notAfter,
			KeyUsage:     x509.KeyUsageDigitalSignature,
			ExtKeyUsage:  []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
			DNSNames:     s.dns,
		}
		for _, ip := range s.ips {
			tmpl.IPAddresses = append(tmpl.IPAddresses, net.ParseIP(ip))
		}
		der, err := x509.CreateCertificate(rand.Reader, tmpl, ca, &key.PublicKey, caKey)
		if err != nil {
			return nil, err
		}
		keyDER, err := x509.MarshalPKCS8PrivateKey(key)
		if err != nil {
			return nil, err
		}
		chain := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})
		keyPEM := pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: keyDER})
		pair, err := tls.X509KeyPair(chain, keyPEM)
		if err != nil {
			return nil, err
		}
		p.certs[s.name] = Cert{Chain: string(chain), Key: string(keyPEM)}
		p.tls[s.name] = pair
	}
	return p, nil
}

func (p *pki) serverCert(name string) (tls.Certificate, error) {
	c, ok := p.tls[name]
	if !ok {
		return tls.Certificate{}, errUnknownCert
	}
	return c, nil
}
