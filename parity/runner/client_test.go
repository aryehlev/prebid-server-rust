package runner

import (
	"context"
	"net"
	"net/http"
	"net/http/httptest"
	"sync"
	"sync/atomic"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// TestClientUsesOneConnection: back-to-back and concurrent calls to one server
// share a single connection, so the runner never leaves an extra idle
// connection for the server to close while a case runs.
func TestClientUsesOneConnection(t *testing.T) {
	tests := []struct {
		name       string
		concurrent int
	}{
		{"back to back", 1},
		{"concurrent", 4},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var opened atomic.Int32
			srv := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.Header().Set("Content-Type", "application/json")
				_, _ = w.Write([]byte(`{"schema":"` + parity.SchemaHealth + `"}`))
			}))
			srv.Config.ConnState = func(_ net.Conn, s http.ConnState) {
				if s == http.StateNew {
					opened.Add(1)
				}
			}
			srv.Start()
			t.Cleanup(srv.Close)
			c := NewClient(srv.URL, "token")
			t.Cleanup(c.CloseIdle)
			var wg sync.WaitGroup
			errs := make([]error, tt.concurrent)
			for i := 0; i < tt.concurrent; i++ {
				wg.Add(1)
				go func(i int) {
					defer wg.Done()
					for j := 0; j < 50 && errs[i] == nil; j++ {
						_, errs[i] = c.Health(context.Background())
					}
				}(i)
			}
			wg.Wait()
			for _, err := range errs {
				require.NoError(t, err)
			}
			assert.Equal(t, int32(1), opened.Load())
		})
	}
}
