package gen

import (
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"regexp"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// Structural steps edit one node of the JSON tree the endpoint parses (the
// body, or the AMP stored request); raw steps edit body bytes and leave the
// tree behind; HTTP steps edit method, headers and query; encoding steps
// compress the body last.

// tree returns the JSON tree structural steps edit, or nil.
func (d *draft) tree() any {
	if d.endpoint == corpus.EndpointAMP {
		if d.amp == nil {
			return nil
		}
		return d.amp
	}
	if d.rawSet || d.body == nil {
		return nil
	}
	return d.body
}

// structStep builds a structural step that edits one node keep selects.
func structStep(name, doc string, keep func(n nodeRef) bool, edit func(d *draft, r *RNG, n nodeRef)) *mutator {
	candidates := func(d *draft) []nodeRef {
		t := d.tree()
		if t == nil {
			return nil
		}
		return filterNodes(nodes(t), keep)
	}
	return newStep(PhaseStructure, name, Uncertain, onAll, doc, func(d *draft) bool { return len(candidates(d)) > 0 }, func(d *draft, r *RNG) error {
		n := Pick(r, candidates(d))
		edit(d, r, n)
		d.note("%s at %s", name, n.path)
		return nil
	})
}

func valueIs(pred func(v any) bool) func(nodeRef) bool {
	return func(n nodeRef) bool { return pred(n.get()) }
}

func isNumber(v any) bool { _, ok := v.(json.Number); return ok }
func isString(v any) bool { _, ok := v.(string); return ok }
func isObject(v any) bool { _, ok := v.(obj); return ok }

func isContainer(v any) bool {
	switch t := v.(type) {
	case obj:
		return len(t) > 0
	case []any:
		return len(t) > 0
	}
	return false
}

func structureMutators() []*mutator {
	return []*mutator{
		structStep("struct.drop", "deletes one object member", func(n nodeRef) bool { _, ok := n.parent.(obj); return ok }, func(_ *draft, _ *RNG, n nodeRef) {
			if o, ok := n.parent.(obj); ok {
				delete(o, n.key)
			}
		}),
		structStep("struct.dup", "duplicates one array element", valueIs(func(v any) bool { a, ok := v.([]any); return ok && len(a) > 0 }), func(_ *draft, r *RNG, n nodeRef) {
			arr, _ := n.get().([]any)
			n.set(append(append([]any(nil), arr...), deepCopy(Pick(r, arr))))
		}),
		structStep("struct.retype", "replaces one value with a value of another JSON type", func(nodeRef) bool { return true }, func(_ *draft, r *RNG, n nodeRef) {
			n.set(retype(r, n.get()))
		}),
		structStep("struct.boundary", "replaces one number with a boundary literal", valueIs(isNumber), func(d *draft, r *RNG, n nodeRef) {
			n.set(json.Number(Pick(r, d.g.pools.Values.BoundaryNumbers)))
		}),
		structStep("struct.unicode", "replaces or extends one string with unusual unicode", valueIs(isString), func(d *draft, r *RNG, n nodeRef) {
			s := Pick(r, d.g.pools.Values.UnicodeStrings)
			if old, _ := n.get().(string); r.Chance(0.5) {
				s = old + s
			}
			n.set(s)
		}),
		structStep("struct.null", "sets one value to null", valueIs(func(v any) bool { return v != nil }), func(_ *draft, _ *RNG, n nodeRef) { n.set(nil) }),
		structStep("struct.empty", "empties one object or array", valueIs(isContainer), func(_ *draft, _ *RNG, n nodeRef) {
			if isObject(n.get()) {
				n.set(obj{})
				return
			}
			n.set([]any{})
		}),
		structStep("struct.unknown_field", "adds an unknown member to one object", valueIs(isObject), func(_ *draft, r *RNG, n nodeRef) {
			if o, ok := n.get().(obj); ok {
				o["parity_unknown_"+r.Hex(4)] = Pick(r, []any{"x", num(1), true, obj{}})
			}
		}),
		newStep(PhaseStructure, "struct.deep_nest", Uncertain, onAll, "adds a deeply nested array member to the root", func(d *draft) bool {
			return isObject(d.tree())
		}, func(d *draft, r *RNG) error {
			depth := Pick(r, d.g.pools.Values.NestingDepths)
			var v any = []any{}
			for i := 1; i < depth; i++ {
				v = []any{v}
			}
			root, _ := d.tree().(obj)
			root["parity_nest"] = v
			return nil
		}).rare(),
	}
}

// retype returns a value of a different JSON type derived from v.
func retype(r *RNG, v any) any {
	switch t := v.(type) {
	case string:
		return Pick(r, []any{num(int64(len(t))), true, []any{t}})
	case json.Number:
		return Pick(r, []any{t.String(), false, obj{"value": t}})
	case bool:
		return Pick(r, []any{strconv.FormatBool(t), num(1)})
	case obj:
		return Pick(r, []any{[]any{t}, "object"})
	case []any:
		return Pick(r, []any{obj{"items": t}, num(int64(len(t)))})
	}
	return Pick(r, []any{"", num(0), obj{}})
}

// rawStep builds a byte-level body step.
func rawStep(name string, v Validity, doc string, when func(b []byte) bool, edit func(d *draft, r *RNG, b []byte) []byte) *mutator {
	return newStep(PhaseRaw, name, v, onBody, doc, func(d *draft) bool {
		if !d.hasBody() || d.contentEncoding != "" {
			return false
		}
		b, err := d.bodyBytes()
		return err == nil && (when == nil || when(b))
	}, func(d *draft, r *RNG) error {
		b, err := d.bodyBytes()
		if err != nil {
			return err
		}
		d.setRaw(edit(d, r, append([]byte(nil), b...)))
		return nil
	})
}

func nonEmpty(b []byte) bool { return len(b) > 0 }

// objectStart returns the index just after the first '{' and whether the
// object has members after it.
func objectStart(b []byte) (int, bool) {
	i := bytes.IndexByte(b, '{')
	if i < 0 {
		return -1, false
	}
	rest := bytes.TrimLeft(b[i+1:], " \t\r\n")
	return i + 1, len(rest) > 0 && rest[0] != '}'
}

func hasObjectStart(b []byte) bool { i, _ := objectStart(b); return i >= 0 }

// insertMember inserts `"key":value` as the first member of the first object.
func insertMember(b []byte, key, value string) []byte {
	i, members := objectStart(b)
	if i < 0 {
		return b
	}
	member := strconv.Quote(key) + ":" + value
	if members {
		member += ","
	}
	return splice(b, i, member)
}

func splice(b []byte, at int, s string) []byte {
	out := make([]byte, 0, len(b)+len(s))
	out = append(out, b[:at]...)
	out = append(out, s...)
	return append(out, b[at:]...)
}

// rootKeys returns the root member names of a JSON object body.
func rootKeys(b []byte) []string {
	o, err := decodeObject(b)
	if err != nil {
		return nil
	}
	return sortedKeys(o)
}

var (
	stringValueRE = regexp.MustCompile(`":"`)
	numberValueRE = regexp.MustCompile(`":[0-9]`)
)

func rawMutators() []*mutator {
	return []*mutator{
		rawStep("raw.truncate", Malformed, "cuts the body short", func(b []byte) bool { return len(b) > 1 }, func(_ *draft, r *RNG, b []byte) []byte {
			return b[:r.Range(1, len(b)-1)]
		}),
		rawStep("raw.trailing", Malformed, "appends bytes after the JSON value", nonEmpty, func(d *draft, r *RNG, b []byte) []byte {
			return append(b, Pick(r, d.g.pools.Values.TrailingGarbage)...)
		}),
		rawStep("raw.trailing_comma", Malformed, "adds a trailing comma before the last closing bracket", func(b []byte) bool { return bytes.LastIndexAny(b, "}]") > 0 }, func(_ *draft, _ *RNG, b []byte) []byte {
			return splice(b, bytes.LastIndexAny(b, "}]"), ",")
		}),
		rawStep("raw.dupkey", Uncertain, "repeats a root member (null first, the real value last)", func(b []byte) bool { return len(rootKeys(b)) > 0 }, func(_ *draft, r *RNG, b []byte) []byte {
			return insertMember(b, Pick(r, rootKeys(b)), "null")
		}),
		rawStep("raw.escape_key", Uncertain, "writes one member name with a \\u escape", func(b []byte) bool { return len(escapableKeys(b)) > 0 }, func(_ *draft, r *RNG, b []byte) []byte {
			k := Pick(r, escapableKeys(b))
			escaped := fmt.Sprintf(`"\u%04x%s":`, k[0], k[1:])
			return bytes.Replace(b, []byte(strconv.Quote(k)+":"), []byte(escaped), 1)
		}),
		rawStep("raw.bom", Malformed, "prefixes a UTF-8 byte order mark", nil, func(_ *draft, _ *RNG, b []byte) []byte {
			return append([]byte("\xef\xbb\xbf"), b...)
		}),
		rawStep("raw.whitespace", Preserves, "re-indents the body with unusual whitespace", nonEmpty, func(_ *draft, _ *RNG, b []byte) []byte {
			var buf bytes.Buffer
			buf.WriteString("\r\n \t")
			if err := json.Indent(&buf, b, "\t", "   "); err != nil {
				buf.Write(b)
			}
			buf.WriteString(" \n")
			return buf.Bytes()
		}),
		rawStep("raw.invalid_utf8", Uncertain, "puts an invalid UTF-8 byte into a string value", stringValueRE.Match, func(_ *draft, _ *RNG, b []byte) []byte {
			return splice(b, stringValueRE.FindIndex(b)[1], "\xff")
		}),
		rawStep("raw.lone_surrogate", Uncertain, "puts an unpaired surrogate escape into a string value", stringValueRE.Match, func(_ *draft, _ *RNG, b []byte) []byte {
			return splice(b, stringValueRE.FindIndex(b)[1], `\ud800`)
		}),
		rawStep("raw.leading_zero", Malformed, "gives a number a leading zero", numberValueRE.Match, func(_ *draft, _ *RNG, b []byte) []byte {
			return splice(b, numberValueRE.FindIndex(b)[1]-1, "0")
		}),
		rawStep("raw.comment", Malformed, "inserts a comment", hasObjectStart, func(_ *draft, _ *RNG, b []byte) []byte {
			i, _ := objectStart(b)
			return splice(b, i, "/*parity*/")
		}),
		rawStep("raw.nesting", Uncertain, "inserts a deeply nested array as the first member", hasObjectStart, func(d *draft, r *RNG, b []byte) []byte {
			n := Pick(r, d.g.pools.Values.NestingDepths)
			return insertMember(b, "parity_nest", strings.Repeat("[", n)+strings.Repeat("]", n))
		}).rare(),
		rawStep("raw.alt_body", Malformed, "replaces the body with a non-request JSON value or nothing", nil, func(d *draft, r *RNG, _ []byte) []byte {
			return []byte(Pick(r, d.g.pools.Values.AltBodies))
		}),
		rawStep("raw.big_body", Invalidates, "pads the body past the default max_request_size", hasObjectStart, func(_ *draft, _ *RNG, b []byte) []byte {
			return insertMember(b, "parity_pad", strconv.Quote(strings.Repeat("x", defaultMaxRequest+oversizeMargin)))
		}).rare(),
	}
}

// escapableKeys are root member names that start with an ASCII letter.
func escapableKeys(b []byte) []string {
	var out []string
	for _, k := range rootKeys(b) {
		if k != "" && k[0] < 0x80 && bytes.Contains(b, []byte(strconv.Quote(k)+":")) {
			out = append(out, k)
		}
	}
	return out
}

func httpStep(name string, v Validity, eps endpointSet, doc string, when func(*draft) bool, run func(*draft, *RNG) error) *mutator {
	return newStep(PhaseHTTP, name, v, eps, doc, when, run)
}

func hasBodyHeader(d *draft) bool { return d.hasBody() }

func httpMutators() []*mutator {
	return []*mutator{
		httpStep("http.method", Malformed, onAll, "uses another HTTP method", nil, func(d *draft, r *RNG) error {
			var methods []string
			for _, m := range append([]string{http.MethodPost}, d.g.pools.Values.Methods...) {
				if m != d.method {
					methods = append(methods, m)
				}
			}
			d.method = Pick(r, methods)
			return nil
		}).rare(),
		httpStep("http.content_type", Uncertain, onBody, "sends another Content-Type", hasBodyHeader, func(d *draft, r *RNG) error {
			d.setHeader("Content-Type", Pick(r, d.g.pools.Values.ContentTypes))
			return nil
		}),
		httpStep("http.no_content_type", Uncertain, onBody, "drops Content-Type", hasBodyHeader, func(d *draft, _ *RNG) error { d.dropHeader("Content-Type"); return nil }),
		httpStep("http.dup_header", Uncertain, onBody, "repeats Content-Type with another value", hasBodyHeader, func(d *draft, r *RNG) error {
			d.headers = append(d.headers, [2]string{"Content-Type", Pick(r, d.g.pools.Values.ContentTypes)})
			return nil
		}),
		httpStep("http.encoding_header", Uncertain, onBody, "declares a Content-Encoding the body does not use", func(d *draft) bool { return d.hasBody() && d.contentEncoding == "" }, func(d *draft, r *RNG) error {
			d.setHeader("Content-Encoding", Pick(r, d.g.pools.Values.ContentEncodings))
			return nil
		}),
		httpStep("http.user_agent", Preserves, onAll, "sends a User-Agent header, sometimes instead of device.ua", nil, func(d *draft, r *RNG) error {
			d.setHeader("User-Agent", Pick(r, d.g.pools.ORTB.UserAgents))
			if dev, ok := lookupObj(d.req(), "device"); ok && r.Chance(0.5) {
				delete(dev, "ua")
			}
			return nil
		}),
		httpStep("http.xff", Preserves, onAll, "sends X-Forwarded-For / X-Real-IP, sometimes instead of device.ip", nil, func(d *draft, r *RNG) error {
			d.setHeader("X-Forwarded-For", Pick(r, d.g.pools.ORTB.IPv4)+", "+Pick(r, d.g.pools.ORTB.PrivateIPv4))
			if r.Chance(0.4) {
				d.setHeader("X-Real-IP", Pick(r, d.g.pools.ORTB.IPv4))
			}
			if dev, ok := lookupObj(d.req(), "device"); ok && r.Chance(0.5) {
				delete(dev, "ip")
			}
			return nil
		}),
		httpStep("http.remote_addr", Preserves, onAll, "uses another client address", nil, func(d *draft, r *RNG) error {
			d.remoteAddr = Pick(r, []string{"[2001:db8::7]:443", "198.51.100.23:5555", "10.1.2.3:80"})
			return nil
		}),
		httpStep("http.referer", Preserves, onAll, "sends a Referer header", nil, func(d *draft, r *RNG) error {
			d.setHeader("Referer", Pick(r, d.g.pools.ORTB.Sites).Page)
			return nil
		}),
		httpStep("http.origin", Preserves, onAll, "sends an Origin header", nil, func(d *draft, r *RNG) error {
			d.setHeader("Origin", "https://"+Pick(r, d.g.pools.ORTB.Sites).Domain)
			return nil
		}),
		httpStep("http.cookie", Preserves, onAll, "sends a uids cookie with a bidder uid", nil, uidsCookie),
		httpStep("http.accept_gzip", Preserves, onAll, "accepts a gzip response", nil, func(d *draft, _ *RNG) error {
			d.setHeader("Accept-Encoding", "gzip")
			return nil
		}),
		httpStep("http.dnt", Preserves, onAll, "sends DNT: 1", nil, func(d *draft, _ *RNG) error { d.setHeader("DNT", "1"); return nil }),
		httpStep("http.cookie_deprecation", Preserves, onAll, "sends Sec-Cookie-Deprecation", nil, func(d *draft, r *RNG) error {
			d.setHeader("Sec-Cookie-Deprecation", Pick(r, []string{"label_only_1", strings.Repeat("d", 101)}))
			return nil
		}),
		httpStep("http.query_junk", Preserves, onBody, "adds unknown query parameters", nil, func(d *draft, r *RNG) error {
			d.setQuery("parity", "1")
			d.setQuery("pg_"+r.Hex(3), Pick(r, d.g.pools.Values.UnicodeStrings))
			return nil
		}),
		newStep(PhaseEncoding, "http.gzip", Uncertain, onBody, "gzips the body and sets Content-Encoding: gzip", canEncode, func(d *draft, _ *RNG) error {
			return gzipBody(d, false)
		}),
		newStep(PhaseEncoding, "http.gzip_corrupt", Malformed, onBody, "sends a corrupt gzip body with Content-Encoding: gzip", canEncode, func(d *draft, _ *RNG) error {
			return gzipBody(d, true)
		}).rare(),
	}
}

func uidsCookie(d *draft, r *RNG) error {
	bidder := Pick(r, d.bidders())
	if bidder == "" {
		bidder = "appnexus"
	}
	uids := obj{"tempUIDs": obj{bidder: obj{"uid": "uid-" + r.Hex(8), "expires": "2099-01-01T00:00:00Z"}}}
	raw, err := encodeTree(uids)
	if err != nil {
		return err
	}
	d.setHeader("Cookie", "uids="+base64.URLEncoding.EncodeToString(raw))
	return nil
}

func canEncode(d *draft) bool {
	if !d.hasBody() || d.contentEncoding != "" {
		return false
	}
	b, err := d.bodyBytes()
	return err == nil && len(b) > 0
}

// gzipBody compresses the body at a fixed level; corrupt flips bytes in the
// deflate stream and drops the trailer.
func gzipBody(d *draft, corrupt bool) error {
	b, err := d.bodyBytes()
	if err != nil {
		return err
	}
	var buf bytes.Buffer
	zw, err := gzip.NewWriterLevel(&buf, gzip.BestCompression)
	if err != nil {
		return fmt.Errorf("gen: gzip: %w", err)
	}
	if _, err := zw.Write(b); err != nil {
		return fmt.Errorf("gen: gzip: %w", err)
	}
	if err := zw.Close(); err != nil {
		return fmt.Errorf("gen: gzip: %w", err)
	}
	out := buf.Bytes()
	if corrupt && len(out) > 18 {
		for i := 10; i < len(out)-8; i += 7 {
			out[i] ^= 0x5a
		}
		out = out[:len(out)-4]
	}
	d.setRaw(out)
	d.contentEncoding = "gzip"
	d.setHeader("Content-Encoding", "gzip")
	return nil
}
