package main

import (
	"bytes"
	"compress/gzip"
	"encoding/hex"
	"strings"
)

type gzipDoc struct {
	Header     header `json:"header"`
	Payload    string `json:"payload"`
	Compressed string `json:"compressed_hex"`
}

// gzipGolden compresses a bid request as the exchange compresses request bodies for a bidder
// with endpointCompression gzip (exchange/bidder.go:848-874, a gzip.NewWriter from the pool at
// :876-880, reset onto the buffer).
func gzipGolden() gzipDoc {
	doc := gzipDoc{
		Header: newHeader(),
		Payload: strings.Repeat(`{"id":"req-1","imp":[{"id":"1","banner":{"w":300,"h":250}}],`+
			`"site":{"page":"https://publisher.example/article"}}`, 8),
	}
	var buf bytes.Buffer
	w := gzip.NewWriter(nil)
	w.Reset(&buf)
	if _, err := w.Write([]byte(doc.Payload)); err != nil {
		panic(err)
	}
	if err := w.Close(); err != nil {
		panic(err)
	}
	doc.Compressed = hex.EncodeToString(buf.Bytes())
	return doc
}
