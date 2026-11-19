package main

import (
	"crypto/tls"
	"errors"
	"net"
	"net/http"
)

// helloCase is the ClientHello one of router.go's Transports sends to an https server: the
// fields crypto/tls's ClientHelloInfo reads from it, as their code points.
type helloCase struct {
	// Client is "general" (generalHttpClient, whose TLSClientConfig carries the
	// certificates_file roots) or "cache" (cacheHttpClient and floorFechterHttpClient, with no
	// TLSClientConfig). Both set DialContext, so neither offers h2 (net/http/transport.go
	// Transport.protocols).
	Client           string   `json:"client"`
	ALPN             []string `json:"alpn"`
	Versions         []uint16 `json:"versions"`
	CipherSuites     []uint16 `json:"cipher_suites"`
	Groups           []uint16 `json:"groups"`
	SignatureSchemes []uint16 `json:"signature_schemes"`
}

type clientHelloDoc struct {
	Header header      `json:"header"`
	Cases  []helloCase `json:"cases"`
}

var errHelloRead = errors.New("the ClientHello is read")

func clientHelloGolden() clientHelloDoc {
	doc := clientHelloDoc{Header: newHeader()}
	pki := newTestPKI()
	for _, client := range []string{"general", "cache"} {
		ln, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			panic(err)
		}
		hellos := make(chan helloCase, 1)
		config := &tls.Config{
			Certificates: []tls.Certificate{pki.leaf},
			GetConfigForClient: func(hello *tls.ClientHelloInfo) (*tls.Config, error) {
				h := helloCase{
					Client:           client,
					ALPN:             nonNil(hello.SupportedProtos),
					Versions:         append([]uint16{}, hello.SupportedVersions...),
					CipherSuites:     append([]uint16{}, hello.CipherSuites...),
					Groups:           []uint16{},
					SignatureSchemes: []uint16{},
				}
				for _, group := range hello.SupportedCurves {
					h.Groups = append(h.Groups, uint16(group))
				}
				for _, scheme := range hello.SignatureSchemes {
					h.SignatureSchemes = append(h.SignatureSchemes, uint16(scheme))
				}
				hellos <- h
				return nil, errHelloRead
			},
		}
		go func() {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			_ = tls.Server(conn, config).Handshake()
			_ = conn.Close()
		}()
		transport := generalTransport(pki.roots)
		if client == "cache" {
			transport.TLSClientConfig = nil
		}
		transport.DialContext = dialTo(ln.Addr().String())
		req, err := http.NewRequest(http.MethodGet, "https://bidder.test/h", nil)
		if err != nil {
			panic(err)
		}
		if _, _, err := roundTrip(&http.Client{Transport: transport}, req); err == nil {
			panic("the handshake went through")
		}
		doc.Cases = append(doc.Cases, <-hellos)
		transport.CloseIdleConnections()
		_ = ln.Close()
	}
	return doc
}
