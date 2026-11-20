package main

// Generated differential cases: scripted sessions drawn from a fixed seed
// (sessions_fuzz.json, and the portable forms of portable.go) and the fake
// values, UUID streams and map orders the installed fakes serve across a
// sequence of cases (cases.json). The Rust crate replays them against its own
// Session and fakes.

import (
	"cmp"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"math"
	"math/rand"
	"strconv"
	"strings"
	"time"
	"unicode/utf16"

	"github.com/gofrs/uuid"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/util/uuidutil"
)

// fuzzSeed fixes the generated inputs; the goldens record the cases, so a new
// seed only needs a regeneration.
const fuzzSeed = 20261117

// fuzzScripts is the number of generated sessions.
const fuzzScripts = 400

// fuzzer draws the generated inputs.
type fuzzer struct{ r *rand.Rand }

func newFuzzer(seed int64) *fuzzer { return &fuzzer{r: rand.New(rand.NewSource(seed))} }

func (f *fuzzer) intn(n int) int { return f.r.Intn(n) }

func (f *fuzzer) chance(percent int) bool { return f.r.Intn(100) < percent }

func (f *fuzzer) pick(xs ...string) string { return xs[f.r.Intn(len(xs))] }

// cp is the UTF-8 encoding of the code point c, so the source stays ASCII
// (some tools rewrite escapes and invisible characters in sources).
func cp(c rune) string { return string(c) }

// fuzzFragments are the pieces generated strings are made of: HTML
// characters, the characters encoders escape differently, multi-byte and
// astral characters, fmt verbs and JSON punctuation.
var fuzzFragments = []string{
	"", "a", "Z", "0", " ", "<", ">", "&", `"`, `\`, "/", cp(0x2028), cp(0x2029), cp(0xe9), cp(0x65e5) + cp(0x672c), cp(0x1f600),
	cp(0xfffd), "\x00", "\x01", "\b", "\f", "\t", "\n", "\r", "\x1f", "\x7f", "\u0080", cp(0xa0),
	cp(0x200b), cp(0xfeff), "%", "%d", "%s", "%v", "%%", "%!", "{", "}", "[", "]", ":", ",", "null",
	"1e2", "-0", "<script>", "&amp;", "a#0", "_", "http://x/y?a=1&b=2",
}

func (f *fuzzer) text() string {
	var b strings.Builder
	for i, n := 0, f.intn(5); i < n; i++ {
		b.WriteString(fuzzFragments[f.intn(len(fuzzFragments))])
	}
	return b.String()
}

func (f *fuzzer) int64() int64 {
	switch f.intn(4) {
	case 0:
		return []int64{0, 1, -1, 2, 10, math.MaxInt64, math.MinInt64, math.MaxInt32, 1 << 53, -(1 << 53) - 1}[f.intn(10)]
	case 1:
		return int64(f.intn(2000)) - 1000
	default:
		v := f.r.Int63()
		if f.chance(50) {
			v = -v
		}
		return v
	}
}

func (f *fuzzer) float() float64 {
	special := []float64{
		0, math.Copysign(0, -1), 1, -1, 0.5, 1e21, 1e20, 999999999999999999999, 1e-6, 1e-7, 9.999999e-7,
		123456789.125, 5e-324, math.MaxFloat64, -math.MaxFloat64, 0.1, 0.30000000000000004, 1.5, 100,
		1e15, 1e16, 1e17, 9007199254740993, 4.35, 0.000001234, 12345678.9, math.Inf(1), math.Inf(-1),
		math.NaN(),
	}
	switch f.intn(4) {
	case 0:
		return special[f.intn(len(special))]
	case 1:
		return math.Float64frombits(f.r.Uint64())
	case 2:
		return float64(f.r.Int63n(1_000_000_000)-500_000_000) / 100
	default:
		return f.r.NormFloat64() * math.Pow(10, float64(f.intn(60)-30))
	}
}

// writeString writes s as a JSON string, using \u escapes (and surrogate
// pairs) for some characters.
func (f *fuzzer) writeString(b *strings.Builder, s string) {
	b.WriteByte('"')
	for _, r := range s {
		switch {
		case r == '"' || r == '\\':
			b.WriteByte('\\')
			b.WriteRune(r)
		case r == '/' && f.chance(30):
			b.WriteString(`\/`)
		case r < 0x20:
			fmt.Fprintf(b, `\u%04x`, r)
		case r >= 0x10000 && f.chance(30):
			hi, lo := utf16.EncodeRune(r)
			fmt.Fprintf(b, `\u%04x\u%04X`, hi, lo)
		case r < 0x10000 && f.chance(10):
			fmt.Fprintf(b, `\u%04x`, r)
		default:
			b.WriteRune(r)
		}
	}
	b.WriteByte('"')
}

func (f *fuzzer) space(b *strings.Builder, ws bool) {
	if ws && f.chance(30) {
		b.WriteString(f.pick(" ", "\n", "\t", "  ", "\r\n"))
	}
}

var fuzzLiterals = []string{
	"null", "true", "false", "0", "-0", "1", "-1", "1.5", "1.0", "1e2", "1E+2", "-1.5e-7",
	"12345678901234567890", "0.1", "1e400", "123456789.125", "5e-324", "0.0000001",
}

func (f *fuzzer) writeJSON(b *strings.Builder, depth int, ws bool) {
	f.space(b, ws)
	k := f.intn(6)
	if depth >= 3 {
		k = f.intn(2)
	}
	switch k {
	case 0:
		b.WriteString(fuzzLiterals[f.intn(len(fuzzLiterals))])
	case 1:
		f.writeString(b, f.text())
	case 2, 3:
		b.WriteByte('[')
		for i, n := 0, f.intn(4); i < n; i++ {
			if i > 0 {
				b.WriteByte(',')
			}
			f.writeJSON(b, depth+1, ws)
		}
		f.space(b, ws)
		b.WriteByte(']')
	default:
		b.WriteByte('{')
		for i, n := 0, f.intn(4); i < n; i++ {
			if i > 0 {
				b.WriteByte(',')
			}
			f.space(b, ws)
			f.writeString(b, f.pick("a", "b", "<", cp(0xe9), "", "a&b")+f.text())
			f.space(b, ws)
			b.WriteByte(':')
			f.writeJSON(b, depth+1, ws)
		}
		f.space(b, ws)
		b.WriteByte('}')
	}
	f.space(b, ws)
}

func (f *fuzzer) jsonText(ws bool) string {
	var b strings.Builder
	f.writeJSON(&b, 0, ws)
	return b.String()
}

// corrupt breaks a JSON text while keeping it valid UTF-8.
func (f *fuzzer) corrupt(s string) string {
	runes := []rune(s)
	switch f.intn(5) {
	case 0:
		if len(runes) > 0 {
			return string(runes[:f.intn(len(runes))])
		}
		return ""
	case 1:
		return s + f.pick(" x", "}", ",", "]", "\x01", "1")
	case 2:
		return f.pick("x", "{", ",", "'") + s
	case 3:
		if len(runes) > 0 {
			runes[f.intn(len(runes))] = []rune(f.pick("'", "}", "x", "\x02", ":"))[0]
		}
		return string(runes)
	default:
		return f.pick("", " ", "{", `{"a":}`, `{"a" 1}`, "[1,]", "nul", `"\x"`, `"\ud800"`, "01", "+1", ".5", "1.", "NaN")
	}
}

// rawText is a JSON text, valid or not, that is valid UTF-8.
func (f *fuzzer) rawText() string {
	if f.chance(70) {
		return f.jsonText(f.chance(50))
	}
	return f.corrupt(f.jsonText(f.chance(50)))
}

// rawBytes are arbitrary recorded bytes: JSON or not, valid UTF-8 or not.
func (f *fuzzer) rawBytes() []byte {
	switch f.intn(6) {
	case 0:
		return nil
	case 1:
		return []byte(f.text())
	case 2:
		return []byte(f.rawText())
	case 3:
		return []byte(`"` + f.pick("\xff", "\xc3", "\xed\xa0\x80", "\xf4\x90\x80\x80", "a\x80b", "\xc0\xaf") + `"`)
	case 4:
		b := make([]byte, f.intn(12))
		for i := range b {
			b[i] = byte(f.intn(256))
		}
		return b
	default:
		return []byte(`{"a":"` + f.pick("<", cp(0x2028), "\xe2\x80", "\xff") + `"}` + f.pick("", "\n", " "))
	}
}

func mustJSON(v any) json.RawMessage {
	raw, err := json.Marshal(v)
	if err != nil {
		panic(err)
	}
	return raw
}

// typed fills the type and value of a record or analytics op.
// byteText is a Go string of arbitrary bytes (invalid UTF-8 included), as
// base64 so it reaches the Rust side exactly.
func (f *fuzzer) byteText() string {
	if f.chance(50) {
		return base64.StdEncoding.EncodeToString(f.rawBytes())
	}
	return base64.StdEncoding.EncodeToString([]byte(f.text()))
}

func (f *fuzzer) typed(o *op) {
	switch f.intn(15) {
	case 11:
		o.Type, o.Text = "bytes_string", f.byteText()
		return
	case 12:
		pairs := make([][2]string, f.intn(4))
		for i := range pairs {
			pairs[i] = [2]string{f.byteText(), f.byteText()}
		}
		o.Type, o.Value = "bytes_map", mustJSON(pairs)
		return
	case 13:
		var list []string
		if f.chance(80) {
			list = make([]string, f.intn(4))
			for i := range list {
				list[i] = f.byteText()
			}
		}
		o.Type, o.Value = "bytes_strings", mustJSON(list)
		return
	}
	switch f.intn(12) {
	case 0:
		o.Type, o.Value = "int", mustJSON(f.int64())
	case 1:
		o.Type, o.Text = "float_bits", strconv.FormatUint(math.Float64bits(f.float()), 10)
	case 2:
		o.Type = "nan"
	case 3:
		o.Type, o.Value = "bool", mustJSON(f.chance(50))
	case 4:
		o.Type, o.Value = "string", mustJSON(f.text())
	case 5:
		m := map[string]string{}
		for i, n := 0, f.intn(4); i < n; i++ {
			m[f.text()] = f.text()
		}
		o.Type, o.Value = "map", mustJSON(m)
	case 6:
		var list []string
		if f.chance(80) {
			list = make([]string, f.intn(4))
			for i := range list {
				list[i] = f.text()
			}
		}
		o.Type, o.Value = "strings", mustJSON(list)
	case 7, 8:
		o.Type, o.Text = "raw", f.rawText()
	case 9, 10:
		v := map[string]any{"gdpr": f.intn(3)}
		if f.chance(70) {
			v["ext"] = f.rawText()
		}
		o.Type, o.Value = "regs_record", mustJSON(v)
	default:
		o.Type = "nil_regs"
	}
}

var fuzzErrorTypes = []string{
	"timeout", "tmax_timeout", "bad_input", "blocked_app", "account_disabled", "acct_required",
	"bad_server_response", "failed_to_request_bids", "bidder_temporarily_disabled", "bidder_throttled",
	"malformed_acct", "warning", "failed_to_unmarshal", "failed_to_marshal", "debug_warning",
	"invalid_imp_first_party_data", "generic",
}

func (f *fuzzer) errorList() json.RawMessage {
	if f.chance(15) {
		return rawJSON(`null`)
	}
	specs := make([]errSpec, f.intn(5))
	for i := range specs {
		specs[i] = errSpec{Type: fuzzErrorTypes[f.intn(len(fuzzErrorTypes))], Message: f.text()}
		if f.chance(60) {
			specs[i].Code = []int{0, 1, 999, 10001, 10003, 10999, 42, -7, 10017}[f.intn(9)]
		}
	}
	return mustJSON(specs)
}

func (f *fuzzer) key() string {
	if f.chance(70) {
		return f.pick("", "", "_", "appnexus", "appnexus#0", "appnexus#unindexed", "rubicon", "<&>", cp(0xe9), " ", "a"+cp(0x2028)+"b")
	}
	return f.text()
}

func (f *fuzzer) stage() string {
	all := parity.AllStages()
	return string(all[f.intn(len(all))])
}

var fuzzPoints = []string{
	"floors.skip", "floors.data_rate", "floors.modelgroup", "floors.enforce", "category.dedupe",
	"pricegranularity.timesplit", "usersync.shuffle",
}

func (f *fuzzer) body() *parity.Body {
	if f.chance(30) {
		return nil
	}
	b := &parity.Body{}
	if f.chance(50) {
		s := f.text()
		b.Text = &s
	}
	if f.chance(30) {
		s := base64.StdEncoding.EncodeToString(f.rawBytes())
		b.B64 = &s
	}
	if f.chance(50) {
		b.JSON = json.RawMessage(f.jsonText(true))
	}
	if f.chance(20) {
		b.ContentEncoding = f.pick("gzip", "br", "identity", "")
	}
	return b
}

func (f *fuzzer) headers() parity.HeaderList {
	switch f.intn(4) {
	case 0:
		return nil
	case 1:
		return parity.HeaderList{}
	default:
		h := make(parity.HeaderList, f.intn(4)+1)
		for i := range h {
			h[i] = [2]string{f.pick("Content-Type", "content-type", "X-Parity-Meta", "Accept-Encoding", "") + f.text(), f.text()}
		}
		return h
	}
}

func (f *fuzzer) optString() *string {
	if f.chance(50) {
		return nil
	}
	s := f.pick("timeout", "refused", "network_disabled", "") + f.text()
	return &s
}

func (f *fuzzer) notes() []string {
	switch f.intn(3) {
	case 0:
		return nil
	case 1:
		return []string{}
	default:
		n := make([]string, f.intn(3)+1)
		for i := range n {
			n[i] = f.text()
		}
		return n
	}
}

func (f *fuzzer) call() string {
	c := parity.OutboundCall{
		Seq:     int(f.int64() % 1000),
		Impl:    f.pick("", "go", "rust") + f.text(),
		Kind:    f.pick("bidder", "cache", "currency", "") + f.text(),
		Bidder:  f.text(),
		OrigURL: f.pick("https://ib.adnxs.com/openrtb2/prebid?a=<b>&c", "http://cache/x", "") + f.text(),
		Request: parity.OutboundRequest{
			Method: f.pick("POST", "GET", ""),
			URL:    f.text(),
			Proto:  f.pick("", "HTTP/1.1"),
			Header: f.headers(),
			Body:   f.body(),
		},
		Wire:  f.chance(50),
		Error: f.optString(),
		Notes: f.notes(),
	}
	if f.chance(50) {
		c.Mock = &parity.MockRef{Source: f.pick("inline", "rule", "") + f.text(), ID: f.text()}
	}
	if f.chance(60) {
		c.Response = &parity.OutboundResponse{
			Status:  []int{0, 200, 204, 500, -1}[f.intn(5)],
			Headers: f.headers(),
			Body:    f.body(),
			DelayMS: []int{0, 5, -3}[f.intn(3)],
		}
	}
	return string(mustJSON(c))
}

var fuzzDecisionValues = []string{
	`0`, `1`, `2`, `3`, `-1`, `null`, `"identity"`, `"rng"`, `""`, `"1"`, `1.0`, `2.5`, `1e2`, `-0`,
	`true`, `[1]`, `{}`, `9223372036854775807`, `9223372036854775808`, `-9223372036854775808`,
	`" identity"`, `"` + uEsc("0069") + `dentity"`, `"IDENTITY"`,
}

func (f *fuzzer) envelope(i int) string {
	var capture []string
	if f.chance(60) {
		stages := []string{`"*"`, `null`, `"all"`, `[]`, `[null]`, `5`, `{}`, `true`, `[1]`, `["http.in",1]`, `"` + uEsc("002a") + `"`}
		v := stages[f.intn(len(stages))]
		if f.chance(50) {
			ids := make([]string, f.intn(6))
			for j := range ids {
				if f.chance(85) {
					ids[j] = strconv.Quote(f.stage())
				} else {
					ids[j] = f.pick(`"made.up"`, `""`, `"HTTP.IN"`)
				}
			}
			v = "[" + strings.Join(ids, ",") + "]"
		}
		capture = append(capture, `"stages":`+v)
	}
	for _, flag := range []string{"metrics", "analytics"} {
		if f.chance(40) {
			capture = append(capture, `"`+flag+`":`+f.pick("true", "false", "null"))
		}
	}
	decisions := ""
	switch f.intn(5) {
	case 0:
		decisions = `,"decisions":null`
	case 1:
		decisions = `,"decisions":{}`
	case 2, 3:
		var pairs []string
		for _, p := range fuzzPoints {
			if f.chance(50) {
				pairs = append(pairs, strconv.Quote(p)+":"+fuzzDecisionValues[f.intn(len(fuzzDecisionValues))])
			}
		}
		if f.chance(20) {
			pairs = append(pairs, `"unknown.point":1`)
		}
		decisions = `,"decisions":{` + strings.Join(pairs, ",") + `}`
	}
	var caseID, path strings.Builder
	f.writeString(&caseID, fmt.Sprintf("fuzz-%d", i)+f.text())
	f.writeString(&path, f.pick("/openrtb2/auction", "/openrtb2/amp", "/openrtb2/video", "")+f.text())
	return fmt.Sprintf(`{"schema":"pbs-parity-envelope/1","schema_version":"1.0.0","case_id":%s,"request":{"method":"POST","path":%s,"query":"","headers":[]},"mock":{}%s,"capture":{%s}}`,
		caseID.String(), path.String(), decisions, strings.Join(capture, ","))
}

func (f *fuzzer) op() op {
	o := op{}
	switch k := f.intn(24); {
	case k < 5:
		o.Op, o.Stage, o.Key = "record", f.stage(), f.key()
		f.typed(&o)
	case k < 7:
		o.Op, o.Stage, o.Key, o.RawB64 = "record_raw", f.stage(), f.key(), base64.StdEncoding.EncodeToString(f.rawBytes())
	case k < 9:
		o.Op, o.Stage, o.Key, o.Errors = "errors", f.stage(), f.key(), f.errorList()
	case k < 10:
		o.Op, o.Stage, o.Status = "mark", f.stage(), f.pick("captured", "not_reached", "unimplemented")
	case k < 11:
		o.Op, o.Text = "note", f.text()
	case k < 12:
		o.Op, o.RawB64 = "note_b64", base64.StdEncoding.EncodeToString(f.rawBytes())
	case k < 13:
		o.Op, o.Call = "outbound", f.call()
	case k < 15:
		o.Op, o.Kind = "analytics", f.pick("auction", "amp", "video", "cookie_sync", "setuid", "notification", "", "<&>")
		f.typed(&o)
	case k < 16:
		o.Op = "next_seq"
	case k < 17:
		o.Op, o.Stage = "wants", f.stage()
	case k < 18:
		o.Op, o.Point, o.Def = "decide", fuzzPoints[f.intn(len(fuzzPoints))], mustJSON(f.int64())
	case k < 19:
		o.Op, o.Point, o.Draw = "decide_bool", fuzzPoints[f.intn(len(fuzzPoints))], f.chance(50)
	case k < 21:
		o.Op, o.Point, o.Uncertain, o.Draw = "decide_chance", fuzzPoints[f.intn(len(fuzzPoints))], f.chance(50), f.chance(50)
	case k < 22:
		o.Op, o.Point, o.Def = "decide_string", fuzzPoints[f.intn(len(fuzzPoints))], mustJSON(f.pick("rng", "identity", "")+f.text())
	case k < 23:
		o.Op, o.Point = "intn", fuzzPoints[f.intn(len(fuzzPoints))]
		o.N = []int{0, 1, 2, 3, 5, -1, -4, 100, math.MaxInt64, math.MinInt64}[f.intn(10)]
		if o.N > -100 && o.N < 1000 {
			o.Offset = f.intn(7) - 3
		}
	default:
		o.Op, o.Point = "last_decision", fuzzPoints[f.intn(len(fuzzPoints))]
	}
	return o
}

// fuzzSessions draws the generated scripts.
func fuzzSessions() []sessionScript {
	f := newFuzzer(fuzzSeed)
	scripts := make([]sessionScript, 0, fuzzScripts)
	for i := 0; i < fuzzScripts; i++ {
		envelope := f.envelope(i)
		ops := make([]op, f.intn(40))
		for j := range ops {
			ops[j] = f.op()
		}
		scripts = append(scripts, sessionScript{name: fmt.Sprintf("fuzz-%d", i), envelope: envelope, ops: ops})
	}
	return scripts
}

// fuzzSessionsGolden runs generated scripts through trace.Session, recording
// each op's result and the encoded trace as exact bytes.
func fuzzSessionsGolden() (any, error) {
	scripts := fuzzSessions()
	cases := make([]map[string]any, 0, len(scripts))
	for _, script := range scripts {
		results, doc, err := runScript(script)
		if err != nil {
			return nil, err
		}
		cases = append(cases, map[string]any{
			"name":      script.name,
			"envelope":  script.envelope,
			"ops":       script.ops,
			"results":   results,
			"trace_b64": base64.StdEncoding.EncodeToString([]byte(doc)),
		})
	}
	return cases, nil
}

// fakeOp is one draw from the installed fakes (cases.json).
type fakeOp struct {
	Op   string             `json:"op"`
	N    int                `json:"n,omitempty"`
	Keys []string           `json:"keys,omitempty"`
	Set  *parity.FakeValues `json:"set,omitempty"`
}

// fakePhase runs its ops inside a case of Envelope, or between cases when it
// is nil.
type fakePhase struct {
	Envelope *string  `json:"envelope"`
	Ops      []fakeOp `json:"ops"`
}

func runFakeOp(o fakeOp) (any, error) {
	src := fakes.Active()
	switch o.Op {
	case "new_v4":
		u, err := uuid.NewV4()
		return u.String(), err
	case "uuidutil":
		return uuidutil.UUIDRandomGenerator{}.Generate()
	case "generate":
		return src.Generate()
	case "now":
		t := fakes.Now()
		return map[string]any{
			"unix_milli": t.UnixMilli(),
			"unix":       t.Unix(),
			"format":     t.Format(time.RFC3339Nano),
			"huaweiads":  t.Format("2006-01-02 15:04:05.000"),
			"weekday":    int(t.Weekday()),
			"year_day":   t.YearDay(),
		}, nil
	case "int":
		return fakes.Int(), nil
	case "int63":
		return src.GenerateInt63(), nil
	case "intn":
		return src.Intn(o.N), nil
	case "map_keys":
		m := make(map[string]int, len(o.Keys))
		for i, k := range o.Keys {
			m[k] = i
		}
		return fakes.MapKeys(m, cmp.Less[string]), nil
	case "set":
		src.Set(o.Set)
		return nil, nil
	case "reset":
		src.Set(nil)
		return nil, nil
	case "values":
		return src.Values(), nil
	}
	return nil, fmt.Errorf("unknown fake op %q", o.Op)
}

func (f *fuzzer) fakeValues() *parity.FakeValues {
	v := &parity.FakeValues{}
	if f.chance(60) {
		v.NowUnixMS = []int64{0, 1, -1, 1629473330823, 1700000000000, 4102444800000, -62135596800000, 253402300799999, 86400000 * 3}[f.intn(9)]
		if f.chance(40) {
			v.NowUnixMS = f.r.Int63n(4102444800000)
		}
	}
	if f.chance(60) {
		v.Int63 = f.int64()
	}
	if f.chance(60) {
		v.UUID = f.pick(fakes.DefaultUUID, "case-seed", "x", cp(0xe9), "a:1") + f.text()
	}
	return v
}

func (f *fuzzer) fakeOps() []fakeOp {
	ops := make([]fakeOp, f.intn(12)+1)
	for i := range ops {
		switch k := f.intn(13); {
		case k < 4:
			ops[i] = fakeOp{Op: "new_v4"}
		case k < 5:
			ops[i] = fakeOp{Op: "uuidutil"}
		case k < 6:
			ops[i] = fakeOp{Op: "generate"}
		case k < 7:
			ops[i] = fakeOp{Op: "now"}
		case k < 8:
			ops[i] = fakeOp{Op: "int"}
		case k < 9:
			ops[i] = fakeOp{Op: "int63"}
		case k < 10:
			ops[i] = fakeOp{Op: "intn", N: []int{0, 1, 2, 3, 7, 10, 1000, -1, -5}[f.intn(9)]}
		case k < 11:
			keys := make([]string, f.intn(6))
			for j := range keys {
				keys[j] = f.pick("b", "a", "c", "appnexus", "Rubicon", "rubicon", "", cp(0xe9), "aa") + strconv.Itoa(j)
			}
			ops[i] = fakeOp{Op: "map_keys", Keys: keys}
		case k < 12:
			if f.chance(50) {
				ops[i] = fakeOp{Op: "set", Set: f.fakeValues()}
			} else {
				ops[i] = fakeOp{Op: "reset"}
			}
		default:
			ops[i] = fakeOp{Op: "values"}
		}
	}
	return ops
}

// casesGolden installs the fakes and runs phases inside and between cases:
// every case applies its envelope's fakes (restarting the UUID stream) and
// every end restores the defaults (restarting it again).
func casesGolden() (any, error) {
	envelope := func(caseID, fakesMember string) *string {
		s := fmt.Sprintf(`{"schema":"pbs-parity-envelope/1","schema_version":"1.0.0","case_id":%q,"request":{"method":"POST","path":"/openrtb2/auction","query":"","headers":[]},"mock":{},"capture":{}%s}`, caseID, fakesMember)
		return &s
	}
	draws := func(n int) []fakeOp {
		ops := make([]fakeOp, n)
		for i := range ops {
			ops[i] = fakeOp{Op: "new_v4"}
		}
		return ops
	}
	all := []fakeOp{
		{Op: "values"}, {Op: "new_v4"}, {Op: "uuidutil"}, {Op: "generate"}, {Op: "now"}, {Op: "int"},
		{Op: "int63"}, {Op: "intn", N: 3}, {Op: "intn", N: 0}, {Op: "intn", N: -1},
		{Op: "map_keys", Keys: []string{"b", "c", "a", "ab", ""}},
	}
	phases := []fakePhase{
		{Ops: append(draws(3), all...)},
		{Envelope: envelope("a", `,"fakes":{"now_unix_ms":1000,"int63":7,"uuid":"case-a"}`), Ops: append(draws(3), all...)},
		{Ops: draws(2)},
		{Envelope: envelope("b", ""), Ops: append(draws(2), all...)},
		{Envelope: envelope("c", `,"fakes":{"uuid":"case-c"}`), Ops: []fakeOp{
			{Op: "new_v4"}, {Op: "set", Set: &parity.FakeValues{UUID: "mid", Int63: 5}}, {Op: "new_v4"}, {Op: "new_v4"},
			{Op: "uuidutil"}, {Op: "int"}, {Op: "values"}, {Op: "reset"}, {Op: "new_v4"}, {Op: "values"},
			{Op: "set", Set: &parity.FakeValues{}}, {Op: "new_v4"},
		}},
		{Envelope: envelope("d", `,"fakes":{}`), Ops: []fakeOp{{Op: "values"}, {Op: "new_v4"}}},
		{Envelope: envelope("e", `,"fakes":{"now_unix_ms":-1,"int63":-9223372036854775808,"uuid":"`+cp(0xe9)+`"}`), Ops: []fakeOp{
			{Op: "now"}, {Op: "int"}, {Op: "int63"}, {Op: "intn", N: 7}, {Op: "intn", N: 1}, {Op: "new_v4"}, {Op: "generate"},
		}},
		{Envelope: envelope("f", `,"fakes":null`), Ops: draws(1)},
		{Ops: append(draws(1), fakeOp{Op: "values"})},
	}
	f := newFuzzer(fuzzSeed + 1)
	for i := 0; i < 40; i++ {
		if f.chance(30) {
			phases = append(phases, fakePhase{Ops: f.fakeOps()})
			continue
		}
		member := ""
		if f.chance(80) {
			member = `,"fakes":` + string(mustJSON(f.fakeValues()))
		}
		phases = append(phases, fakePhase{Envelope: envelope(fmt.Sprintf("fuzz-%d", i), member), Ops: f.fakeOps()})
	}

	fakes.Install()
	defer fakes.Uninstall()
	out := make([]map[string]any, 0, len(phases))
	for i, p := range phases {
		end := func() {}
		if p.Envelope != nil {
			var env parity.Envelope
			if err := json.Unmarshal([]byte(*p.Envelope), &env); err != nil {
				return nil, fmt.Errorf("phase %d: %w", i, err)
			}
			_, end = trace.Begin(&env)
		}
		results := make([]any, 0, len(p.Ops))
		for _, o := range p.Ops {
			r, err := runFakeOp(o)
			if err != nil {
				end()
				return nil, fmt.Errorf("phase %d: %s: %w", i, o.Op, err)
			}
			results = append(results, r)
		}
		end()
		out = append(out, map[string]any{"envelope": p.Envelope, "ops": p.Ops, "results": results})
	}
	return out, nil
}
