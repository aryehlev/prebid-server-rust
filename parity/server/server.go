// Package server implements the Go server's /__parity/* routes (PH-07): the
// contract every parity runner drives and the Rust server mirrors.
//
//	GET  /__parity/health                   pbs-parity/1
//	POST /__parity/exec                     envelope in, pbs-parity-trace/1 out
//	POST /__parity/auction|video            exec with the method and path fixed
//	GET|POST /__parity/amp                  exec with the method and path fixed
//	POST /__parity/adapter/{bidder}/requests  MakeRequests only
//	POST /__parity/adapter/{bidder}/bids      MakeBids only
//	GET  /__parity/config                   pbs-parity-config/1
//	GET  /__parity/bidders                  pbs-parity-bidders/1
//	POST /__parity/reset                    pbs-parity-reset/1
//
// The routes are linked into the binary by a blank import in main.go: init
// registers New with wiring.RegisterRoutes, and wiring.Mount serves them on the
// main port only when parity.enabled is true, after its loopback and
// X-Parity-Token checks. Every route answers application/json; failures use
// the error document {"error": "..."} (introspection.schema.json#/definitions/Error).
//
// The package is harness code linked into the server binary, so it does not
// import the harness-only packages (parity/schema, parity/corpus, ...). Input
// documents are decoded strictly instead: unknown members, a wrong "schema"
// member or an unknown schema major version are rejected with 400.
package server

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/wiring"
)

func init() { wiring.RegisterRoutes(New) }

// MaxDocumentBytes caps the size of an input document (envelope or adapter
// input). Envelopes carry whole request bodies, often base64 encoded.
const MaxDocumentBytes = 64 << 20

// Route paths. AdapterPrefix is followed by "{bidder}/requests" or "{bidder}/bids".
const (
	PathHealth    = parity.PathPrefix + "health"
	PathExec      = parity.PathPrefix + "exec"
	PathAuction   = parity.PathPrefix + "auction"
	PathAMP       = parity.PathPrefix + "amp"
	PathVideo     = parity.PathPrefix + "video"
	PathConfig    = parity.PathPrefix + "config"
	PathBidders   = parity.PathPrefix + "bidders"
	PathReset     = parity.PathPrefix + "reset"
	AdapterPrefix = parity.PathPrefix + "adapter/"
)

// Server serves the /__parity/* routes of one Go server process.
type Server struct {
	deps     wiring.Deps
	dispatch http.Handler
	routes   map[string]route
}

// route is one fixed path and the handler of each method it accepts.
type route map[string]http.HandlerFunc

// New builds the /__parity/* handler (a wiring.RoutesFactory). dispatch is the
// exact handler the main listener serves for normal traffic, compression
// included; exec sends every envelope through it. app is the same chain without
// compression and is not needed by any route.
func New(deps wiring.Deps, dispatch, _ http.Handler) http.Handler {
	s := &Server{deps: deps, dispatch: dispatch}
	s.routes = map[string]route{
		PathHealth:  {http.MethodGet: s.health},
		PathExec:    {http.MethodPost: s.exec},
		PathAuction: {http.MethodPost: s.alias(auctionAlias)},
		PathVideo:   {http.MethodPost: s.alias(videoAlias)},
		PathAMP:     {http.MethodGet: s.ampQueryAlias, http.MethodPost: s.alias(ampAlias)},
		PathConfig:  {http.MethodGet: s.dumpConfig},
		PathBidders: {http.MethodGet: s.dumpBidders},
		PathReset:   {http.MethodPost: s.reset},
	}
	return s
}

// ServeHTTP routes one /__parity/* request.
func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if rt, ok := s.routes[r.URL.Path]; ok {
		s.serveRoute(w, r, rt)
		return
	}
	if bidder, op, ok := adapterRoute(r.URL.Path); ok {
		s.serveRoute(w, r, s.adapterRoute(bidder, op))
		return
	}
	wiring.WriteError(w, http.StatusNotFound, fmt.Sprintf("parity: no route %s", r.URL.Path))
}

func (s *Server) serveRoute(w http.ResponseWriter, r *http.Request, rt route) {
	if h, ok := rt[r.Method]; ok {
		h(w, r)
		return
	}
	methods := make([]string, 0, len(rt))
	for m := range rt {
		methods = append(methods, m)
	}
	sort.Strings(methods)
	w.Header().Set("Allow", strings.Join(methods, ", "))
	wiring.WriteError(w, http.StatusMethodNotAllowed, fmt.Sprintf("parity: %s does not accept %s", r.URL.Path, r.Method))
}

// adapterRoute splits /__parity/adapter/{bidder}/{requests|bids}.
func adapterRoute(path string) (bidder, op string, ok bool) {
	rest, found := strings.CutPrefix(path, AdapterPrefix)
	if !found {
		return "", "", false
	}
	bidder, op, found = strings.Cut(rest, "/")
	if !found || bidder == "" || (op != adapterOpRequests && op != adapterOpBids) {
		return "", "", false
	}
	return bidder, op, true
}

// errDocument is an input-document error, answered with 400 (or 413 when the
// document is too large).
type errDocument struct {
	status int
	msg    string
}

func (e *errDocument) Error() string { return e.msg }

func badDocument(format string, args ...any) error {
	return &errDocument{status: http.StatusBadRequest, msg: "parity: " + fmt.Sprintf(format, args...)}
}

// writeFailure answers err with its document status, or 500 for anything else.
func writeFailure(w http.ResponseWriter, err error) {
	var doc *errDocument
	if errors.As(err, &doc) {
		wiring.WriteError(w, doc.status, doc.msg)
		return
	}
	wiring.WriteError(w, http.StatusInternalServerError, err.Error())
}

// readDocument reads a request body of at most MaxDocumentBytes.
func readDocument(r *http.Request) ([]byte, error) {
	if r.Body == nil {
		return nil, badDocument("empty request body")
	}
	raw, err := io.ReadAll(io.LimitReader(r.Body, MaxDocumentBytes+1))
	if err != nil {
		return nil, badDocument("reading request body: %v", err)
	}
	if len(raw) > MaxDocumentBytes {
		return nil, &errDocument{status: http.StatusRequestEntityTooLarge, msg: fmt.Sprintf("parity: document exceeds %d bytes", MaxDocumentBytes)}
	}
	return raw, nil
}

// decodeStrict decodes one JSON document into v, rejecting unknown members and
// trailing data.
func decodeStrict(raw []byte, v any) error {
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.DisallowUnknownFields()
	if err := dec.Decode(v); err != nil {
		return badDocument("decoding document: %v", err)
	}
	if dec.More() {
		return badDocument("decoding document: trailing data after the document")
	}
	return nil
}

// checkHeader validates the "schema" and "schema_version" members of an input
// document: the schema name must be want, and the version's major must be the
// one this server implements.
func checkHeader(schema, version, want string) error {
	if schema != want {
		return badDocument("schema must be %q, got %q", want, schema)
	}
	major, _, _ := strings.Cut(version, ".")
	wantMajor, _, _ := strings.Cut(parity.SchemaVersion, ".")
	if major != wantMajor || strings.Count(version, ".") != 2 {
		return badDocument("unsupported schema_version %q (this server implements %s)", version, parity.SchemaVersion)
	}
	return nil
}

// writeJSON writes v as a parity document. HTML escaping is off so embedded
// json.RawMessage members keep their bytes.
func writeJSON(w http.ResponseWriter, status int, v any) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		wiring.WriteError(w, http.StatusInternalServerError, fmt.Sprintf("parity: encoding response: %v", err))
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	w.Write(buf.Bytes())
}
