package main

import (
	"bytes"
	"compress/gzip"
	"encoding/hex"
	"net/http"
	"strconv"
	"strings"
)

// compressionCase is one response the Transport's gzip handling sees
// (net/http/transport.go: the Transport asks for gzip and decompresses only when the request
// has no Accept-Encoding and no Range).
type compressionCase struct {
	Name string `json:"name"`
	// Headers are the caller's request headers.
	Headers []headerOp `json:"headers"`
	// ResponseHead is the status line and header lines the server writes (Content-Length
	// included), ResponseBodyHex the body bytes after them.
	ResponseHead    []string `json:"response_head"`
	ResponseBodyHex string   `json:"response_body_hex"`
	// AcceptEncoding is the request's Accept-Encoding line as the server read it ("" for none).
	AcceptEncoding string `json:"accept_encoding"`
	// What the caller got: the status, the Content-Encoding and Content-Length values left in the
	// response's header map, the body and the error io.ReadAll returned.
	Status          int      `json:"status"`
	ContentEncoding []string `json:"content_encoding"`
	ContentLength   []string `json:"content_length"`
	BodyHex         string   `json:"body_hex"`
	ReadError       string   `json:"read_error"`
}

type compressionDoc struct {
	Header header            `json:"header"`
	Cases  []compressionCase `json:"cases"`
}

// gzipped is s compressed by compress/gzip at the default level.
func gzipped(s string) []byte {
	var buf bytes.Buffer
	w := gzip.NewWriter(&buf)
	if _, err := w.Write([]byte(s)); err != nil {
		panic(err)
	}
	if err := w.Close(); err != nil {
		panic(err)
	}
	return buf.Bytes()
}

func compressionCases() []compressionCase {
	gz := gzipped(`{"id":"r","seatbid":[]}`)
	ok := func(encoding string, body []byte) []string {
		return []string{"HTTP/1.1 200 OK", "Content-Type: application/json", "Content-Encoding: " + encoding,
			"Content-Length: " + strconv.Itoa(len(body))}
	}
	return []compressionCase{
		{Name: "auto_gzip", ResponseHead: ok("gzip", gz), ResponseBodyHex: hex.EncodeToString(gz)},
		// adapters/gamma/gamma.go:136 sets the header itself.
		{Name: "caller_accept_encoding", Headers: []headerOp{{"add", "Accept-Encoding", "gzip, deflate"}},
			ResponseHead: ok("gzip", gz), ResponseBodyHex: hex.EncodeToString(gz)},
		{Name: "caller_range", Headers: []headerOp{{"set", "Range", "bytes=0-"}},
			ResponseHead: ok("gzip", gz), ResponseBodyHex: hex.EncodeToString(gz)},
		{Name: "upper_case_gzip", ResponseHead: ok("GZIP", gz), ResponseBodyHex: hex.EncodeToString(gz)},
		// A bidder's no-bid answer with a Content-Encoding and no body.
		{Name: "no_content_gzip", ResponseHead: []string{"HTTP/1.1 204 No Content", "Content-Encoding: gzip"}},
		{Name: "empty_gzip_body", ResponseHead: ok("gzip", nil)},
		{Name: "corrupt_gzip", ResponseHead: ok("gzip", []byte("hello")), ResponseBodyHex: hex.EncodeToString([]byte("hello"))},
		{Name: "unrequested_deflate", ResponseHead: ok("deflate", []byte("plain")), ResponseBodyHex: hex.EncodeToString([]byte("plain"))},
	}
}

func compressionGolden() compressionDoc {
	doc := compressionDoc{Header: newHeader()}
	for _, c := range compressionCases() {
		body, err := hex.DecodeString(c.ResponseBodyHex)
		if err != nil {
			panic(err)
		}
		acceptEncoding := make(chan string, 1)
		srv := newRawServer(func(conn *rawConn) {
			req, ok := conn.readRequest()
			if !ok {
				panic("compression: no request")
			}
			line := ""
			for _, l := range req.head {
				if name, _, found := strings.Cut(l, ":"); found && strings.EqualFold(name, "Accept-Encoding") {
					line = l
				}
			}
			acceptEncoding <- line
			conn.write(strings.Join(c.ResponseHead, "\r\n") + "\r\nConnection: close\r\n\r\n" + string(body))
		})
		req, err := http.NewRequest(http.MethodGet, "http://"+srv.addr()+"/c", nil)
		if err != nil {
			panic(err)
		}
		for _, op := range c.Headers {
			op.apply(req.Header)
		}
		resp, got, err := roundTrip(&http.Client{Transport: generalTransport(nil)}, req)
		if resp == nil {
			panic(err)
		}
		c.AcceptEncoding = <-acceptEncoding
		srv.close()
		c.Status = resp.StatusCode
		c.ContentEncoding = nonNil(resp.Header.Values("Content-Encoding"))
		c.ContentLength = nonNil(resp.Header.Values("Content-Length"))
		c.BodyHex = hex.EncodeToString(got)
		c.ReadError = errText(err)
		doc.Cases = append(doc.Cases, c)
	}
	return doc
}
