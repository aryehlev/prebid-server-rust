package main

import (
	"crypto/tls"
	"net/http"
	"net/http/httptest"
)

// protocolCase is one https request from a Go client to a loopback server: the protocol the
// response came back on and the User-Agent the server read.
type protocolCase struct {
	// Client is "default" (http.DefaultTransport, which the currency fetch in main.go:72 and the
	// analytics client in analytics/clients/http.go use) or "general" (router.go's Transport).
	Client string `json:"client"`
	// ServerH2 says whether the server offered h2 in ALPN.
	ServerH2  bool   `json:"server_h2"`
	Proto     string `json:"proto"`
	UserAgent string `json:"user_agent"`
}

type defaultClientDoc struct {
	Header header         `json:"header"`
	Cases  []protocolCase `json:"cases"`
}

func defaultClientGolden() defaultClientDoc {
	doc := defaultClientDoc{Header: newHeader()}
	pki := newTestPKI()
	for _, serverH2 := range []bool{true, false} {
		agents := make(chan string, 1)
		srv := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			agents <- r.UserAgent()
			_, _ = w.Write([]byte("ok"))
		}))
		srv.EnableHTTP2 = serverH2
		srv.TLS = &tls.Config{Certificates: []tls.Certificate{pki.leaf}}
		srv.StartTLS()
		for _, client := range []string{"default", "general"} {
			var transport *http.Transport
			if client == "default" {
				transport = http.DefaultTransport.(*http.Transport).Clone()
				transport.TLSClientConfig = &tls.Config{RootCAs: pki.roots}
			} else {
				transport = generalTransport(pki.roots)
			}
			req, err := http.NewRequest(http.MethodGet, srv.URL+"/d", nil)
			if err != nil {
				panic(err)
			}
			resp, _, err := roundTrip(&http.Client{Transport: transport}, req)
			if err != nil {
				panic(err)
			}
			transport.CloseIdleConnections()
			doc.Cases = append(doc.Cases, protocolCase{Client: client, ServerH2: serverH2, Proto: resp.Proto, UserAgent: <-agents})
		}
		srv.Close()
	}
	return doc
}
