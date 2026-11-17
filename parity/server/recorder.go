package server

import (
	"bytes"
	"net/http"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/transport"
)

// sniffLen is how many body bytes http.DetectContentType considers.
const sniffLen = 512

// recorder captures the response the handler chain writes (the http.response
// stage). It follows the http.ResponseWriter contract of net/http's server, so
// the recorded response is what a client of the main listener receives:
//
//   - the header map is snapshotted by the first WriteHeader (explicit, or
//     implicit 200 on the first Write or Flush); later changes are ignored;
//   - 1xx interim codes other than 101 are not the final status;
//   - when the final header has no Content-Type, Content-Encoding or
//     Transfer-Encoding and the status allows a body, Content-Type is sniffed
//     from the body written before the header was sent (first Flush or the end
//     of the handler);
//   - statuses without a body (1xx, 204, 304) reject writes with
//     http.ErrBodyNotAllowed, and 304 drops Content-Type;
//   - a HEAD response keeps its headers and drops the body.
//
// The framing headers the listener itself adds on the wire (Date,
// Content-Length, Transfer-Encoding, Connection) are not part of the stage.
type recorder struct {
	header      http.Header
	snapshot    http.Header
	sent        http.Header
	status      int
	wroteHeader bool
	head        bool
	body        bytes.Buffer
	// invalidCode is a WriteHeader code net/http would panic on.
	invalidCode int
}

func newRecorder(head bool) *recorder {
	return &recorder{header: make(http.Header), head: head}
}

// Header implements http.ResponseWriter.
func (r *recorder) Header() http.Header { return r.header }

// WriteHeader implements http.ResponseWriter.
func (r *recorder) WriteHeader(code int) {
	if code < 100 || code > 999 {
		if r.invalidCode == 0 {
			r.invalidCode = code
		}
		return
	}
	if r.wroteHeader || (code >= 100 && code <= 199 && code != http.StatusSwitchingProtocols) {
		return
	}
	r.status = code
	r.wroteHeader = true
	r.snapshot = r.header.Clone()
}

// Write implements http.ResponseWriter.
func (r *recorder) Write(p []byte) (int, error) {
	if !r.wroteHeader {
		r.WriteHeader(http.StatusOK)
	}
	if !bodyAllowedForStatus(r.status) {
		return 0, http.ErrBodyNotAllowed
	}
	return r.body.Write(p)
}

// Flush implements http.Flusher: it sends the header, as net/http does.
func (r *recorder) Flush() {
	if !r.wroteHeader {
		r.WriteHeader(http.StatusOK)
	}
	r.sendHeader()
}

// sendHeader fixes the header as the listener would send it.
func (r *recorder) sendHeader() {
	if r.sent != nil {
		return
	}
	h := r.snapshot.Clone()
	if h == nil {
		h = http.Header{}
	}
	if !bodyAllowedForStatus(r.status) {
		h.Del("Content-Length")
		h.Del("Transfer-Encoding")
		if r.status == http.StatusNotModified {
			h.Del("Content-Type")
		}
		r.sent = h
		return
	}
	_, haveType := h["Content-Type"]
	if !haveType && h.Get("Content-Encoding") == "" && h.Get("Transfer-Encoding") == "" && r.body.Len() > 0 {
		sniff := r.body.Bytes()
		if len(sniff) > sniffLen {
			sniff = sniff[:sniffLen]
		}
		h.Set("Content-Type", http.DetectContentType(sniff))
	}
	r.sent = h
}

// result finishes the response like net/http does when the handler returns
// (implicit 200, header sent) and projects it to the trace form.
func (r *recorder) result() *parity.HTTPResponse {
	if !r.wroteHeader {
		r.WriteHeader(http.StatusOK)
	}
	r.sendHeader()
	resp := &parity.HTTPResponse{Status: r.status, Headers: transport.HeaderListOf(r.sent)}
	if !r.head {
		resp.Body = parity.NewBody(r.body.Bytes(), r.sent.Get("Content-Encoding"))
	}
	return resp
}

// bodyAllowedForStatus mirrors net/http: 1xx, 204 and 304 have no body.
func bodyAllowedForStatus(status int) bool {
	switch {
	case status >= 100 && status <= 199:
		return false
	case status == http.StatusNoContent, status == http.StatusNotModified:
		return false
	}
	return true
}
