package server

import (
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"

	"github.com/julienschmidt/httprouter"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/wiring"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// sizeLimit is the limit of the size check sizeApp makes.
const sizeLimit = 64

// sizeApp checks the body size as endpoints/openrtb2/auction.go does for
// max_request_size: it reads at most sizeLimit bytes through an
// io.LimitedReader and, when that used the limit up, reads one more byte from
// the body and rejects the request unless that read returns io.EOF. Then it
// reads once more after closing the body.
func sizeApp() http.Handler {
	r := httprouter.New()
	r.POST("/size", func(w http.ResponseWriter, req *http.Request, _ httprouter.Params) {
		lr := &io.LimitedReader{R: req.Body, N: sizeLimit}
		got, err := io.ReadAll(lr)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		verdict := "accepted"
		if lr.N <= 0 {
			if n, err := lr.R.Read(make([]byte, 1)); err != io.EOF {
				verdict = fmt.Sprintf("rejected: the next read gave %d, %v", n, err)
			}
		}
		req.Body.Close()
		_, after := req.Body.Read(make([]byte, 1))
		fmt.Fprintf(w, "%s; read %d bytes; after close: %v", verdict, len(got), after)
	})
	return r
}

// TestExecBodyEndsLikeTheWire: around the limit of a LimitedReader size
// check, exec hands the handler a body that ends where a real net/http
// server's does, so the check takes the same branch.
func TestExecBodyEndsLikeTheWire(t *testing.T) {
	app := sizeApp()
	h := newServer(wiring.Deps{}, app)
	for _, size := range []int{0, 1, sizeLimit - 1, sizeLimit, sizeLimit + 1, sizeLimit + 2, 4 * sizeLimit} {
		t.Run(fmt.Sprint(size), func(t *testing.T) {
			body := strings.Repeat("x", size)
			var envBody *parity.Body
			if size > 0 {
				envBody = parity.TextBody(body)
			}
			wire := serveOverWire(t, app, http.MethodPost, "/size", nil, []byte(body))
			tr := execTrace(t, h, testEnvelope(http.MethodPost, "/size", nil, envBody))
			require.NotNil(t, tr.HTTPResponse)
			got, err := tr.HTTPResponse.Body.WireBytes()
			require.NoError(t, err)
			assert.Equal(t, wire.status, tr.HTTPResponse.Status)
			assert.Equal(t, string(wire.body), string(got))
			if size == sizeLimit+1 {
				assert.Contains(t, string(got), "accepted; read 64 bytes", "net/http returns the last byte with io.EOF")
			}
		})
	}
}
