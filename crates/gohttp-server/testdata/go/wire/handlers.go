package main

import (
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/julienschmidt/httprouter"

	"github.com/prebid/prebid-server/v4/endpoints"
	"github.com/prebid/prebid-server/v4/router"
)

// The files of the static directory the router serves under /static/, all with modification
// time staticModTime. tests/go_wire writes the same tree. Their extensions have the same type
// in Go's built-in table and in the system tables mime reads (which, for .js or .xml, depend on
// the host), so the golden does not depend on where it was generated.
var staticFiles = []struct{ name, body string }{
	{"index.html", "<html><body>Prebid Server</body></html>\n"},
	{"a.txt", "hello, static\n"},
	{"data.json", `{"a":1}`},
	{"big.txt", strings.Repeat("0123456789", 300)},
	{"noext", "\x00\x01\x02binary"},
	{"spa ce.txt", "space\n"},
	{"sub/index.html", "<p>sub</p>\n"},
	{"sub/b.css", "body{}\n"},
	{"nested/c.png", "var c = 1;\n"},
	{"nested/d.json", "[1,2]"},
}

// staticDirs are created before the files and get their modification times last.
var staticDirs = []string{"sub", "nested", "empty"}

var staticModTime = time.Date(2020, time.January, 2, 3, 4, 5, 0, time.UTC)

func writeStatic(root string) error {
	for _, d := range staticDirs {
		if err := os.MkdirAll(filepath.Join(root, d), 0o755); err != nil {
			return err
		}
	}
	for _, f := range staticFiles {
		p := filepath.Join(root, f.name)
		if err := os.WriteFile(p, []byte(f.body), 0o644); err != nil {
			return err
		}
		if err := os.Chtimes(p, staticModTime, staticModTime); err != nil {
			return err
		}
	}
	for _, d := range append(staticDirs, ".") {
		if err := os.Chtimes(filepath.Join(root, d), staticModTime, staticModTime); err != nil {
			return err
		}
	}
	return nil
}

// newMainHandler is Prebid's public handler: main.go wraps router.New's httprouter in
// router.SupportCORS and router.NoCache. The routes are router/router.go's, with deterministic
// stand-ins for the endpoints, plus /t/ routes that each reach one net/http behaviour.
func newMainHandler(static string) http.Handler {
	r := httprouter.New()
	r.POST("/openrtb2/auction", echoBody(0))
	r.POST("/openrtb2/video", echoBody(10))
	r.GET("/openrtb2/amp", echoQuery)
	r.GET("/info/bidders", jsonResponse(`["appnexus","rubicon"]`))
	r.GET("/info/bidders/:bidderName", bidderDetail)
	r.GET("/bidders/params", jsonResponse(`{"appnexus":{}}`))
	r.POST("/cookie_sync", echoBody(0))
	r.GET("/status", endpoints.NewStatusEndpoint(""))
	r.GET("/", serveIndex(filepath.Join(static, "index.html")))
	r.Handler("GET", "/version", endpoints.NewVersionEndpoint("", ""))
	r.ServeFiles("/static/*filepath", http.Dir(static))
	r.POST("/vtrack", echoBody(0))
	r.GET("/event", echoQuery)
	r.GET("/setuid", setUID)
	r.GET("/getuids", jsonResponse(`{"buyeruids":{}}`))
	r.POST("/optout", optOut)
	r.GET("/optout", optOut)

	r.GET("/t/write/:n", writeN)
	r.GET("/t/writes/:n/:size", writesN)
	r.GET("/t/flush/:mode", flushMode)
	r.GET("/t/cl/:declared/:actual", declaredLength)
	r.GET("/t/status/:code/:body", statusCode)
	r.GET("/t/twice", writeHeaderTwice)
	r.GET("/t/header/:mode", headerMode)
	r.GET("/t/panic/:mode", panicMode)
	r.GET("/t/error/:mode", errorMode)
	r.GET("/t/redirect/:code", redirectTo)
	r.POST("/t/redirect/:code", redirectTo)
	r.Handler("GET", "/t/strip/*rest", http.StripPrefix("/t/strip", http.HandlerFunc(echoURL)))
	r.POST("/t/body/:mode", bodyMode)
	r.GET("/t/form", form)
	r.POST("/t/form", form)
	r.GET("/t/cookie", cookie)
	for _, m := range []string{"GET", "POST", "PUT", "DELETE", "PATCH", "PURGE"} {
		r.Handle(m, "/t/req/*rest", echoRequest)
	}
	r.GET("/t/remote", remoteAddr)
	r.GET("/t/plan/:spec", func(w http.ResponseWriter, _ *http.Request, ps httprouter.Params) {
		runPlan(w, ps.ByName("spec"))
	})
	return router.NoCache{Handler: router.SupportCORS(r)}
}

// newAdminHandler is router.Admin's ServeMux with stand-ins for the pprof and currency
// handlers.
func newAdminHandler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("/debug/pprof/", adminEcho("index"))
	mux.HandleFunc("/debug/pprof/cmdline", adminEcho("cmdline"))
	mux.HandleFunc("/debug/pprof/profile", adminEcho("profile"))
	mux.HandleFunc("/debug/pprof/symbol", adminEcho("symbol"))
	mux.HandleFunc("/debug/pprof/trace", adminEcho("trace"))
	mux.HandleFunc("/currency/rates", adminEcho("rates"))
	mux.HandleFunc("/version", endpoints.NewVersionEndpoint("", ""))
	// Not one of Prebid's: the admin server is where HEAD requests reach a handler.
	mux.HandleFunc("/t/plan/", func(w http.ResponseWriter, r *http.Request) {
		runPlan(w, strings.TrimPrefix(r.URL.Path, "/t/plan/"))
	})
	return mux
}

// newDrainHandler serves the shutdown scenarios (drain.go): Prebid's wrapping of the public
// router (NoCache over SupportCORS) around /status and /hold/:ms.
func newDrainHandler() http.Handler {
	r := httprouter.New()
	r.GET("/status", endpoints.NewStatusEndpoint(""))
	r.GET("/hold/:ms", hold)
	return router.NoCache{Handler: router.SupportCORS(r)}
}

// newShortHandler serves the server with short timeouts.
func newShortHandler() http.Handler {
	r := httprouter.New()
	r.GET("/t/sleep/:ms", sleepMS)
	r.POST("/t/slowbody", slowBody)
	r.GET("/status", endpoints.NewStatusEndpoint(""))
	return r
}

// formatHeader lists a header map: sorted keys, one " key=quoted-value" per value, and
// " key=<none>" for a key without values.
func formatHeader(h http.Header) string {
	keys := make([]string, 0, len(h))
	for k := range h {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	var b strings.Builder
	for _, k := range keys {
		if len(h[k]) == 0 {
			fmt.Fprintf(&b, " %s=<none>", k)
		}
		for _, v := range h[k] {
			fmt.Fprintf(&b, " %s=%s", k, strconv.Quote(v))
		}
	}
	return b.String()
}

// pattern is n bytes cycling through the alphabet.
func pattern(n int) []byte {
	b := make([]byte, n)
	for i := range b {
		b[i] = byte('a' + i%26)
	}
	return b
}

func atoi(s string) int {
	n, _ := strconv.Atoi(s)
	return n
}

func echoBody(limit int64) httprouter.Handle {
	return func(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
		var src io.Reader = r.Body
		if limit > 0 {
			src = io.LimitReader(r.Body, limit)
		}
		body, err := io.ReadAll(src)
		w.Header().Set("Content-Type", "application/json")
		if err != nil {
			w.WriteHeader(http.StatusBadRequest)
			fmt.Fprintf(w, "read error: %s\n", err.Error())
			return
		}
		fmt.Fprintf(w, "len=%d body=%s trailer:%s\n", len(body), strconv.Quote(string(body)), formatHeader(r.Trailer))
	}
}

func echoQuery(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
	fmt.Fprintf(w, "raw=%s encoded=%s", r.URL.RawQuery, r.URL.Query().Encode())
}

func jsonResponse(body string) httprouter.Handle {
	return func(w http.ResponseWriter, _ *http.Request, _ httprouter.Params) {
		w.Header().Add("Content-Type", "application/json")
		w.Write([]byte(body))
	}
}

// bidderDetail follows endpoints/info/bidders_detail.go, including its second WriteHeader.
func bidderDetail(w http.ResponseWriter, _ *http.Request, ps httprouter.Params) {
	responses := map[string]string{"appnexus": `{"status":"ACTIVE"}`, "all": `{"appnexus":{}}`}
	name := strings.ToLower(ps.ByName("bidderName"))
	response, found := responses[name]
	if !found {
		w.WriteHeader(http.StatusNotFound)
	}
	if found {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(response))
	} else {
		w.WriteHeader(http.StatusNotFound)
	}
}

func serveIndex(name string) httprouter.Handle {
	return func(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
		http.ServeFile(w, r, name)
	}
}

func setUID(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
	q := r.URL.Query()
	if q.Get("bidder") == "" {
		w.WriteHeader(http.StatusBadRequest)
		w.Write([]byte(`"bidder" query param is required`))
		return
	}
	w.Header().Add("Set-Cookie", "uids="+q.Get("uid")+"; Path=/; Max-Age=60")
	w.Header().Set("Content-Type", "image/gif")
	w.Write([]byte("GIF89a"))
}

func optOut(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
	http.Redirect(w, r, "https://example.com/optout?from="+r.Method, http.StatusMovedPermanently)
}

func writeN(w http.ResponseWriter, _ *http.Request, ps httprouter.Params) {
	w.Write(pattern(atoi(ps.ByName("n"))))
}

func writesN(w http.ResponseWriter, _ *http.Request, ps httprouter.Params) {
	chunk := pattern(atoi(ps.ByName("size")))
	for i := 0; i < atoi(ps.ByName("n")); i++ {
		w.Write(chunk)
	}
}

func flushMode(w http.ResponseWriter, _ *http.Request, ps httprouter.Params) {
	f := w.(http.Flusher)
	switch ps.ByName("mode") {
	case "early":
		f.Flush()
		w.Write([]byte("x"))
	case "mid":
		w.Write([]byte("a"))
		f.Flush()
		w.Write([]byte("b"))
	case "end":
		w.Write([]byte("abc"))
		f.Flush()
	case "header":
		w.WriteHeader(http.StatusAccepted)
		f.Flush()
	case "html":
		w.Write([]byte("<html><body>"))
		f.Flush()
		w.Write([]byte("</body></html>"))
	}
}

func declaredLength(w http.ResponseWriter, _ *http.Request, ps httprouter.Params) {
	w.Header().Set("Content-Length", ps.ByName("declared"))
	actual := pattern(atoi(ps.ByName("actual")))
	if _, err := w.Write(actual); err != nil {
		w.Write([]byte(err.Error()))
	}
}

func statusCode(w http.ResponseWriter, _ *http.Request, ps httprouter.Params) {
	w.Header().Set("X-Before", "1")
	w.WriteHeader(atoi(ps.ByName("code")))
	if ps.ByName("body") == "y" {
		w.Write([]byte("body"))
	}
}

func writeHeaderTwice(w http.ResponseWriter, _ *http.Request, _ httprouter.Params) {
	w.WriteHeader(http.StatusCreated)
	w.WriteHeader(http.StatusAccepted)
	w.Write([]byte("x"))
}

func headerMode(w http.ResponseWriter, _ *http.Request, ps httprouter.Params) {
	h := w.Header()
	body := []byte("x")
	switch ps.ByName("mode") {
	case "noncanon":
		h["x-lower"] = []string{"v"}
		h["X-Upper"] = []string{"V"}
		h["x-Mixed_Case"] = []string{"m"}
	case "newline":
		h.Set("X-Nl", "a\nb\r\nc")
	case "badkey":
		h["Bad Key"] = []string{"v"}
		h["Bad:Key"] = []string{"v"}
		h["Good-Key"] = []string{"v"}
	case "multi":
		h.Add("X-M", "1")
		h.Add("X-M", "2")
		h.Add("x-m", "3")
	case "empty":
		h.Set("X-Empty", "")
		h["X-None"] = []string{}
	case "trailer":
		h.Set("Trailer", "X-T1, X-T2")
		w.Write([]byte("body"))
		h.Set("X-T1", "v1")
		h.Set("X-T2", "v2")
		h.Set("X-Undeclared", "u")
		return
	case "trailerbig":
		h.Set("Trailer", "X-T1")
		w.Write(pattern(3000))
		h.Set("X-T1", "v1")
		return
	case "trailerprefix":
		w.Write([]byte("body"))
		h.Set(http.TrailerPrefix+"X-Late", "late")
		return
	case "trailerhead":
		h.Set("Trailer", "X-T1")
		h.Set("X-T1", "early")
	case "date":
		h.Set("Date", "yesterday")
	case "emptyctype":
		h["Content-Type"] = nil
		body = []byte("<html>")
	case "ce":
		h.Set("Content-Encoding", "gzip")
		body = []byte("<html>")
	case "space":
		h.Set("X-Sp", "  padded \t ")
	case "afterwrite":
		w.Write(body)
		h.Set("X-After", "1")
		return
	case "connclose":
		h.Set("Connection", "close")
	case "connka":
		h.Set("Connection", "keep-alive")
	case "connupgrade":
		h.Set("Connection", "Upgrade")
		h.Set("Upgrade", "websocket")
		w.WriteHeader(http.StatusSwitchingProtocols)
		return
	case "teidentity":
		h.Set("Transfer-Encoding", "identity")
	case "techunked":
		h.Set("Transfer-Encoding", "chunked")
	case "tegzip":
		h.Set("Transfer-Encoding", "gzip")
	case "clte":
		h.Set("Content-Length", "1")
		h.Set("Transfer-Encoding", "chunked")
	case "clbad":
		h.Set("Content-Length", "abc")
	case "clneg":
		h.Set("Content-Length", "-1")
	case "clzero":
		h.Set("Content-Length", "0")
		body = nil
	case "location":
		h.Set("Location", "/elsewhere")
		w.WriteHeader(http.StatusFound)
		return
	case "ctype":
		h.Set("Content-Type", "text/csv")
		body = []byte("<html>")
	case "sniffhtml":
		body = []byte("  <!DOCTYPE html><p>")
	case "sniffbin":
		body = []byte("\x00\x01\x02")
	case "sniffpng":
		body = []byte("\x89PNG\x0D\x0A\x1A\x0A....")
	case "sniffjson":
		body = []byte(`{"a":1}`)
	case "sniffutf16":
		body = []byte("\xFE\xFFab")
	case "none":
		body = nil
	}
	w.Write(body)
}

func panicMode(w http.ResponseWriter, _ *http.Request, ps httprouter.Params) {
	switch ps.ByName("mode") {
	case "small":
		w.Write([]byte("partial"))
	case "big":
		w.Write(pattern(3000))
	case "flush":
		w.Write([]byte("flushed"))
		w.(http.Flusher).Flush()
	case "header":
		w.WriteHeader(http.StatusAccepted)
	}
	panic("boom")
}

func errorMode(w http.ResponseWriter, r *http.Request, ps httprouter.Params) {
	switch ps.ByName("mode") {
	case "plain":
		http.Error(w, "boom", http.StatusInternalServerError)
	case "headers":
		w.Header().Set("Content-Length", "99")
		w.Header().Set("Content-Encoding", "gzip")
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("X-Keep", "k")
		http.Error(w, "bad <request>", http.StatusBadRequest)
	case "notfound":
		http.NotFound(w, r)
	case "multiline":
		http.Error(w, "line1\nline2", http.StatusTeapot)
	}
}

func redirectTo(w http.ResponseWriter, r *http.Request, ps httprouter.Params) {
	if r.URL.Query().Get("ct") != "" {
		w.Header().Set("Content-Type", r.URL.Query().Get("ct"))
	}
	http.Redirect(w, r, r.URL.Query().Get("to"), atoi(ps.ByName("code")))
}

func echoURL(w http.ResponseWriter, r *http.Request) {
	fmt.Fprintf(w, "path=%s rawpath=%s uri=%s", r.URL.Path, r.URL.RawPath, r.RequestURI)
}

func bodyMode(w http.ResponseWriter, r *http.Request, ps httprouter.Params) {
	switch ps.ByName("mode") {
	case "none":
		w.Write([]byte("ok"))
	case "some":
		buf := make([]byte, 10)
		n, err := io.ReadFull(r.Body, buf)
		fmt.Fprintf(w, "n=%d err=%v", n, err)
	case "all":
		body, err := io.ReadAll(r.Body)
		fmt.Fprintf(w, "n=%d err=%v", len(body), err)
	case "max":
		r.Body = http.MaxBytesReader(w, r.Body, 16)
		body, err := io.ReadAll(r.Body)
		fmt.Fprintf(w, "n=%d err=%v", len(body), err)
	case "maxok":
		r.Body = http.MaxBytesReader(w, r.Body, 64)
		body, err := io.ReadAll(r.Body)
		fmt.Fprintf(w, "n=%d err=%v", len(body), err)
	case "close":
		cerr := r.Body.Close()
		body, err := io.ReadAll(r.Body)
		fmt.Fprintf(w, "close=%v n=%d err=%v", cerr, len(body), err)
	case "big":
		w.Write(pattern(3000))
	}
}

func form(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
	err := r.ParseForm()
	fmt.Fprintf(w, "err=%v post=%s form=%s", err, r.PostForm.Encode(), r.Form.Encode())
}

func cookie(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
	c, err := r.Cookie("uids")
	if err != nil {
		fmt.Fprintf(w, "err=%s count=%d", err.Error(), len(r.Cookies()))
		return
	}
	fmt.Fprintf(w, "value=%s quoted=%v count=%d", c.Value, c.Quoted, len(r.Cookies()))
}

func echoRequest(w http.ResponseWriter, r *http.Request, ps httprouter.Params) {
	fmt.Fprintf(w, "method=%s\nuri=%s\nurl=%s\npath=%s\nrawpath=%s\nrawquery=%s\nhost=%s\n",
		r.Method, r.RequestURI, r.URL.String(), r.URL.Path, r.URL.RawPath, r.URL.RawQuery, r.Host)
	fmt.Fprintf(w, "proto=%s %d.%d\nclose=%v\ncl=%d\nte=%s\nrest=%s\nheader:%s\n",
		r.Proto, r.ProtoMajor, r.ProtoMinor, r.Close, r.ContentLength,
		strings.Join(r.TransferEncoding, ","), ps.ByName("rest"), formatHeader(r.Header))
}

// planBodies are the bodies of a plan's b steps.
var planBodies = map[string]string{
	"json": `{"a":1}`, "html": "<html>", "htmlws": " \t\r\n<!DOCTYPE HTML>", "htmlopen": "<html",
	"bin": "\x00\x01\x02", "xml": `<?xml version="1.0"?>`, "text": "hello", "pdf": "%PDF-1.4",
	"bom": "\xEF\xBB\xBFx", "gif": "GIF89a", "lt": "<", "ws": " \t\r\n", "empty": "",
	"ht": "<ht", "ml": "ml>",
}

// runPlan runs the dot-separated steps of spec: wN writes N pattern bytes, xN does the same and
// then writes that write's result, bNAME writes planBodies[NAME], f flushes, sN writes the
// status code N, tX sets the Content-Type (to nil for "nil", to "" for "empty", else to
// "application/X"), lX sets the Content-Length and cX the Connection header.
func runPlan(w http.ResponseWriter, spec string) {
	h := w.Header()
	for _, op := range strings.Split(spec, ".") {
		if op == "" {
			continue
		}
		arg := op[1:]
		switch op[0] {
		case 'w':
			w.Write(pattern(atoi(arg)))
		case 'x':
			n, err := w.Write(pattern(atoi(arg)))
			fmt.Fprintf(w, "[n=%d err=%v]", n, err)
		case 'b':
			w.Write([]byte(planBodies[arg]))
		case 'f':
			w.(http.Flusher).Flush()
		case 's':
			w.WriteHeader(atoi(arg))
		case 't':
			switch arg {
			case "nil":
				h["Content-Type"] = nil
			case "empty":
				h.Set("Content-Type", "")
			default:
				h.Set("Content-Type", "application/"+arg)
			}
		case 'l':
			h.Set("Content-Length", arg)
		case 'c':
			h.Set("Connection", arg)
		}
	}
}

func remoteAddr(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
	fmt.Fprintf(w, "remote=%q", r.RemoteAddr)
}

func adminEcho(name string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprintf(w, "%s %s", name, r.URL.Path)
	}
}

func sleepMS(w http.ResponseWriter, _ *http.Request, ps httprouter.Params) {
	time.Sleep(time.Duration(atoi(ps.ByName("ms"))) * time.Millisecond)
	w.Write([]byte("late"))
}

// hold replies "held" after ms milliseconds.
func hold(w http.ResponseWriter, _ *http.Request, ps httprouter.Params) {
	time.Sleep(time.Duration(atoi(ps.ByName("ms"))) * time.Millisecond)
	w.Write([]byte("held"))
}

func slowBody(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
	body, err := io.ReadAll(r.Body)
	timeout := err != nil && strings.Contains(err.Error(), "i/o timeout")
	fmt.Fprintf(w, "n=%d timeout=%v", len(body), timeout)
}
