// Command httpgolden writes the differential goldens replayed by the pbs-gohttp-client Rust
// tests (rust/crates/gohttp-client/src/golden_tests.rs).
//
// Each case starts raw TCP capture servers that run a fixed script per accepted connection
// (read a request, write bytes, sleep, close, reset, ...), then drives a net/http Client whose
// Transport is configured the way Prebid Server's router.go configures its clients. The golden
// records, per case, the exact bytes every server connection consumed and, per request, the
// client's result: the response head, the body and its read error, the trailer, the final URL,
// the error texts and the httptrace events. Run it from the repository root:
//
//	go run ./rust/crates/gohttp-client/testdata/go
//
// Server addresses are written as {{A}}, {{B}}, ...; other loopback addresses (the client's
// side of a connection) as {{LOCAL}}; and the current time in certificate errors as {{NOW}}.
// Byte strings are JSON strings when they are valid UTF-8 and {"hex": "..."} objects otherwise.
package main

import (
	"encoding/hex"
	"encoding/json"
	"flag"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"time"
	"unicode/utf8"
)

// Bytes is a byte string in the golden file.
type Bytes []byte

// MarshalJSON writes valid UTF-8 as a JSON string and anything else as {"hex": "..."}.
func (b Bytes) MarshalJSON() ([]byte, error) {
	if utf8.Valid(b) {
		return json.Marshal(string(b))
	}
	return json.Marshal(map[string]string{"hex": hex.EncodeToString(b)})
}

// Op is one step of a server connection script.
type Op struct {
	// Op is req (a request head and its body), head, body, write, sleep, close, reset (close
	// with an RST), drain (read until EOF, recorded), skip (read for Ms milliseconds or until
	// EOF, not recorded) or tls (a server handshake with the certificate named by Data).
	Op   string `json:"op"`
	Data Bytes  `json:"data,omitempty"`
	Ms   int    `json:"ms,omitempty"`
}

// Server is a capture server: one script per accepted connection, in accept order. A, B, C
// and P are listeners that live for the whole run (P is also the proxy the environment names);
// a Closed server is an address nothing listens on.
type Server struct {
	Name   string `json:"name"`
	Closed bool   `json:"closed,omitempty"`
	Conns  [][]Op `json:"conns"`
}

// Body is a request body.
type Body struct {
	// Kind is none (nil), nobody (http.NoBody), bytes (*bytes.Reader) or reader (an
	// io.Reader that returns Pieces one read at a time).
	Kind   string  `json:"kind"`
	Data   Bytes   `json:"data,omitempty"`
	Pieces []Bytes `json:"pieces,omitempty"`
}

// Req is a request built with http.NewRequestWithContext.
type Req struct {
	Method string `json:"method"`
	URL    string `json:"url"`
	// Header entries are added with Header.Add; RawHeader entries are appended to the map
	// under the key as written.
	Header           [][2]string `json:"header,omitempty"`
	RawHeader        [][2]string `json:"raw_header,omitempty"`
	Body             Body        `json:"body"`
	ContentLength    *int64      `json:"content_length,omitempty"`
	Host             string      `json:"host,omitempty"`
	Close            bool        `json:"close,omitempty"`
	TransferEncoding []string    `json:"transfer_encoding,omitempty"`
	Trailer          [][2]string `json:"trailer,omitempty"`
	CtxTimeoutMs     int         `json:"ctx_timeout_ms,omitempty"`
}

// Step is one Client.Do call.
type Step struct {
	SleepMs int `json:"sleep_ms,omitempty"`
	// Async steps run concurrently with the steps after them.
	Async bool `json:"async,omitempty"`
	Req   Req  `json:"req"`
	// Read is all (io.ReadAll then Close) or none (Close without reading).
	Read string `json:"read"`
	// SettleMs waits before the trace is read, for events Go reports from another goroutine
	// after Client.Do has returned.
	SettleMs int `json:"settle_ms,omitempty"`
}

// Client configures the http.Client and its http.Transport the way router.go does: Proxy is
// http.ProxyFromEnvironment and DialContext a net.Dialer's. Durations are in milliseconds.
type Client struct {
	Profile                 string `json:"profile"`
	TimeoutMs               int    `json:"timeout_ms"`
	DialTimeoutMs           int    `json:"dial_timeout_ms"`
	KeepAliveMs             int    `json:"keep_alive_ms"`
	MaxConnsPerHost         int    `json:"max_conns_per_host"`
	MaxIdleConns            int    `json:"max_idle_conns"`
	MaxIdleConnsPerHost     int    `json:"max_idle_conns_per_host"`
	IdleConnTimeoutMs       int    `json:"idle_conn_timeout_ms"`
	TLSHandshakeTimeoutMs   int    `json:"tls_handshake_timeout_ms"`
	ExpectContinueTimeoutMs int    `json:"expect_continue_timeout_ms"`
	ResponseHeaderTimeoutMs int    `json:"response_header_timeout_ms"`
	DisableCompression      bool   `json:"disable_compression"`
	DisableKeepAlives       bool   `json:"disable_keep_alives"`
	// TLSConfig sets TLSClientConfig with RootCAs: the test CA when TrustCA, else empty.
	TLSConfig bool `json:"tls_config"`
	TrustCA   bool `json:"trust_ca"`
	// Redirect is "" (the default policy), use_last (http.ErrUseLastResponse) or error.
	Redirect string `json:"redirect"`
}

// Result is what one Client.Do call produced.
type Result struct {
	Err              string   `json:"err"`
	Status           string   `json:"status"`
	Proto            string   `json:"proto"`
	Header           string   `json:"header"`
	ContentLength    int64    `json:"content_length"`
	TransferEncoding []string `json:"transfer_encoding"`
	Close            bool     `json:"close"`
	Uncompressed     bool     `json:"uncompressed"`
	Body             Bytes    `json:"body"`
	BodyErr          string   `json:"body_err"`
	Trailer          string   `json:"trailer"`
	URL              string   `json:"url"`
	TLS              string   `json:"tls"`
	Trace            []string `json:"trace"`
}

// Case is one scenario with its recorded outcome.
type Case struct {
	Name     string             `json:"name"`
	Client   Client             `json:"client"`
	Servers  []Server           `json:"servers"`
	Steps    []Step             `json:"steps"`
	Results  []Result           `json:"results"`
	Captured map[string][]Bytes `json:"captured"`
}

// Cert is a PEM certificate chain and key.
type Cert struct {
	Chain string `json:"chain"`
	Key   string `json:"key"`
}

// Golden is the whole file.
type Golden struct {
	ProxyEnv map[string]string `json:"proxy_env"`
	CA       string            `json:"ca"`
	Certs    map[string]Cert   `json:"certs"`
	Cases    []Case            `json:"cases"`
}

func main() {
	out := flag.String("out", "rust/crates/gohttp-client/testdata/golden/cases.json", "output file")
	only := flag.String("only", "", "run only the named case (for debugging; nothing is written)")
	verbose := flag.Bool("v", false, "log each case and its duration")
	flag.Parse()

	p, err := newPKI()
	if err != nil {
		log.Fatal(err)
	}
	env, err := newRunEnv(p)
	if err != nil {
		log.Fatal(err)
	}
	g := Golden{ProxyEnv: proxyEnv, CA: p.caPEM, Certs: p.certs}
	for _, c := range cases() {
		if *only != "" && c.Name != *only {
			continue
		}
		start := time.Now()
		watchdog := time.AfterFunc(30*time.Second, func() {
			buf := make([]byte, 1<<20)
			n := runtime.Stack(buf, true)
			log.Fatalf("%s: stuck\n%s", c.Name, buf[:n])
		})
		rc, err := run(c, env)
		watchdog.Stop()
		if err != nil {
			log.Fatalf("%s: %v", c.Name, err)
		}
		if *verbose {
			log.Printf("%s: %v", c.Name, time.Since(start).Round(time.Millisecond))
		}
		g.Cases = append(g.Cases, rc)
	}
	if *only != "" {
		enc := json.NewEncoder(os.Stdout)
		enc.SetIndent("", "  ")
		if err := enc.Encode(g.Cases); err != nil {
			log.Fatal(err)
		}
		return
	}
	data, err := json.MarshalIndent(g, "", " ")
	if err != nil {
		log.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(*out), 0o755); err != nil {
		log.Fatal(err)
	}
	if err := os.WriteFile(*out, append(data, '\n'), 0o644); err != nil {
		log.Fatal(err)
	}
}
