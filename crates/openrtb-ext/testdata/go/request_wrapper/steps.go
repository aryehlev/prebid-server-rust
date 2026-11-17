package main

import (
	"encoding/json"
	"fmt"
	"log"
	"math"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/prebid/openrtb/v20/openrtb2"

	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/ortb"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// objects are the ext objects of a request, in the order of the "all" steps.
var objects = []string{"request", "user", "device", "app", "regs", "site", "dooh", "source", "imp"}

// each is a step for every object: prefix and each object name.
func each(prefix string) []string {
	out := make([]string, len(objects))
	for i, o := range objects {
		out[i] = prefix + o
	}
	return out
}

// scripts are the step sequences, by name. The steps are:
//
//	snap                    an empty error and the request
//	rebuild                 RebuildRequest; its error and the request
//	up, down                ConvertUpTo26, ConvertDownTo25; the error and the request
//	clear26 ... clear202409 the Clear* function; an empty error and the request
//	clone, cloneclear       the wrapper is replaced by its Clone or CloneAndClearImpWrappers;
//	                        LenImp
//	cloneclearkeep          CloneAndClearImpWrappers, the clone dropped; LenImp of the original
//	exchangecopy            the wrapper is replaced by exchange/utils.go's per-bidder copy
//	                        (CloneAndClearImpWrappers with a copy of the request); LenImp
//	setimprev, setimpdrop   SetImp with the GetImp wrappers reversed, or without the first one;
//	                        an empty error and the request
//	setimpnil               SetImp(nil); an empty error and the request
//	setimpraw=RAW           each GetImp wrapper's Ext set to RAW, then SetImp of them (the pattern
//	                        of privacy.ScrubTID); an empty error and the request
//	partialclone            rw.BidRequest = ortb.CloneBidRequestPartial(rw.BidRequest) (the
//	                        copy of exchange/utils.go's applyPrivacy); an empty error and the request
//	nilreq                  rw.BidRequest = nil; an empty error and the request
//	assignlast              rw.Imp = its last impression alone, appended to a nil slice (what
//	                        exchange/utils.go's removeImpsWithStoredResponses does with a stored
//	                        response for every other impression); an empty error and the request
//	lenimp                  LenImp
//	getimp                  GetImp; the number of wrappers and each wrapper's impression
//	setimp                  SetImp(GetImp()); an empty error and the request
//	assignrev, dropimp      rw.Imp = a reversed copy of rw.Imp (when not nil), rw.Imp = nil; an
//	                        empty error and the request
//	rebuildimp              each impression wrapper's RebuildImp; its error and impression
//	get:O                   GetOExt (each GetImpExt for imp); the error and every getter
//	set:O.M=JSON            the setter of member M with JSON decoded into its parameter (for
//	                        each impression wrapper for imp; "&nil" is a pointer to a nil slice);
//	                        Dirty
//	special:O.M.V           the setter of member M with the value V that JSON cannot give (see
//	                        special); Dirty
//	setext:O=JSON           SetExt with JSON decoded into a map; Dirty
//	setrawext:O=K=RAW       SetExt with {K: RAW}, RAW not decoded; Dirty
//	touchext:O              SetExt(GetExt()); Dirty
//	touch:O                 every member set to what its getter returns; Dirty
//	getorcreate:imp         each GetImpExt's GetOrCreatePrebid; the prebid and Dirty
//
// where O is an object (or "all" for touch and touchext: each object in turn).
var scripts = map[string][]string{
	"decode":   {"snap"},
	"get":      append(each("get:"), "rebuild"),
	"touchext": {"touchext:all", "rebuild"},
	"touch":    {"touch:all", "rebuild"},
	"up":       {"up", "rebuild", "down", "rebuild"},
	"down":     {"down", "rebuild", "up", "rebuild"},
	"clear": {"clear26", "clear202211", "clear202303", "clear202309", "clear202402",
		"clear202409", "rebuild"},
	"imps": {"getimp", `set:imp.tid="tid"`, "getorcreate:imp", "rebuild", "assignrev", "lenimp",
		"rebuild", "setimp", "cloneclear", "lenimp", "get:imp", "touch:imp", "rebuildimp",
		"rebuild"},
	"clone": {"get:user", `set:user.consent="c"`, "clone", "set:regs.gdpr=1", "touch:imp",
		"rebuild", "cloneclear", `set:imp.tid="t2"`, "dropimp", "rebuild"},
}

// suites are the scripts that the rows of a suite run, by name.
var suites = map[string][]string{
	corpusSuite: {"decode", "get", "touchext", "touch", "up", "down", "clear", "imps", "clone"},
}

// corpusSuite is the suite of every request of the corpora.
const corpusSuite = "corpus"

// obs is an observation: a sequence of parts, each its length, ":" and its bytes. lossy is the
// same observation with each invalid UTF-8 byte of an error text replaced by U+FFFD, as the
// Rust port's errors (whose messages are Rust strings) hold them.
type obs struct {
	buf   []byte
	lossy []byte
}

func appendPart(dst, b []byte) []byte {
	dst = strconv.AppendInt(dst, int64(len(b)), 10)
	dst = append(dst, ':')
	return append(dst, b...)
}

func (o *obs) part(b []byte) {
	o.buf = appendPart(o.buf, b)
	o.lossy = appendPart(o.lossy, b)
}

func (o *obs) text(s string) {
	o.part([]byte(s))
}

// errorText writes an error text, and its valid UTF-8 form into lossy.
func (o *obs) errorText(s string) {
	o.buf = appendPart(o.buf, []byte(s))
	o.lossy = appendPart(o.lossy, []byte(validUTF8(s)))
}

// validUTF8 is s with each invalid UTF-8 byte replaced by U+FFFD.
func validUTF8(s string) string {
	if utf8.ValidString(s) {
		return s
	}
	var b strings.Builder
	for _, r := range s {
		b.WriteRune(r)
	}
	return b.String()
}

func (o *obs) err(err error) {
	if err == nil {
		o.part(nil)
		return
	}
	o.errorText(err.Error())
}

func (o *obs) marshal(v interface{}) {
	b, err := jsonutil.Marshal(v)
	if err != nil {
		o.errorText("!" + err.Error())
		return
	}
	o.part(b)
}

// request writes a step's error (empty when it has none, so that equal requests are equal
// observations) and the request.
func (o *obs) request(err error, w *openrtb_ext.RequestWrapper) {
	o.err(err)
	o.marshal(w.BidRequest)
}

func (o *obs) bool(b bool) {
	if b {
		o.text("t")
	} else {
		o.text("f")
	}
}

func (o *obs) int(n int) {
	o.text(strconv.Itoa(n))
}

// rawMap writes the members of an ext map in key order: each key as jsonutil.Marshal writes it
// and its bytes verbatim ("<nil>" for a nil value).
func (o *obs) rawMap(m map[string]json.RawMessage) {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	var inner obs
	for _, k := range keys {
		inner.marshal(k)
		if m[k] == nil {
			inner.text("<nil>")
		} else {
			inner.part(m[k])
		}
	}
	o.part(inner.buf)
}

// runner runs the steps of a script on one wrapper.
type runner struct {
	w     *openrtb_ext.RequestWrapper
	label string
}

func newRunner(data []byte, label string) *runner {
	var req openrtb2.BidRequest
	if err := jsonutil.Unmarshal(data, &req); err != nil {
		log.Fatalf("%s: %v", label, err)
	}
	return &runner{w: &openrtb_ext.RequestWrapper{BidRequest: &req}, label: label}
}

// step runs a step and returns its observation, or the panic text (as the observation) and
// true when it panicked.
func (r *runner) step(step string) (out obs, panicked bool) {
	var o obs
	func() {
		defer func() {
			if p := recover(); p != nil {
				text := []byte(fmt.Sprint(p))
				out = obs{buf: text, lossy: text}
				panicked = true
			}
		}()
		r.run(step, &o)
	}()
	if panicked {
		return out, true
	}
	return o, false
}

// decode decodes a step's JSON argument; the arguments are the generator's own.
func (r *runner) decode(data string, v interface{}) {
	if err := jsonutil.Unmarshal([]byte(data), v); err != nil {
		log.Fatalf("%s: step argument %s: %v", r.label, data, err)
	}
}

func (r *runner) run(step string, o *obs) {
	w := r.w
	name, arg, _ := strings.Cut(step, "=")
	op, target, _ := strings.Cut(name, ":")
	switch op {
	case "snap":
		o.request(nil, w)
	case "rebuild":
		o.request(w.RebuildRequest(), w)
	case "up":
		o.request(openrtb_ext.ConvertUpTo26(w), w)
	case "down":
		o.request(openrtb_ext.ConvertDownTo25(w), w)
	case "clear26":
		openrtb_ext.Clear26Fields(w)
		o.request(nil, w)
	case "clear202211":
		openrtb_ext.Clear202211Fields(w)
		o.request(nil, w)
	case "clear202303":
		openrtb_ext.Clear202303Fields(w)
		o.request(nil, w)
	case "clear202309":
		openrtb_ext.Clear202309Fields(w)
		o.request(nil, w)
	case "clear202402":
		openrtb_ext.Clear202402Fields(w)
		o.request(nil, w)
	case "clear202409":
		openrtb_ext.Clear202409Fields(w)
		o.request(nil, w)
	case "clone":
		r.w = w.Clone()
		o.int(r.w.LenImp())
	case "cloneclear":
		r.w = w.CloneAndClearImpWrappers()
		o.int(r.w.LenImp())
	case "cloneclearkeep":
		// The clone is dropped: what CloneAndClearImpWrappers does to the original.
		_ = w.CloneAndClearImpWrappers()
		o.int(w.LenImp())
	case "exchangecopy":
		// exchange/utils.go's per-bidder copy; the original is dropped, so sharing the request's
		// objects with it cannot be observed.
		c := w.CloneAndClearImpWrappers()
		bidRequestCopy := *w.BidRequest
		c.BidRequest = &bidRequestCopy
		r.w = c
		o.int(c.LenImp())
	case "setimprev":
		imps := w.GetImp()
		rev := make([]*openrtb_ext.ImpWrapper, len(imps))
		for i := range imps {
			rev[len(imps)-1-i] = imps[i]
		}
		w.SetImp(rev)
		o.request(nil, w)
	case "setimpdrop":
		if imps := w.GetImp(); len(imps) > 0 {
			w.SetImp(imps[1:])
		}
		o.request(nil, w)
	case "setimpnil":
		w.SetImp(nil)
		o.request(nil, w)
	case "setimpraw":
		// privacy.ScrubTID's pattern: each wrapper's impression ext replaced, then SetImp.
		imps := w.GetImp()
		for _, iw := range imps {
			iw.Ext = json.RawMessage(arg)
		}
		w.SetImp(imps)
		o.request(nil, w)
	case "partialclone":
		// exchange/utils.go's applyPrivacy: the request replaced by its partial clone.
		w.BidRequest = ortb.CloneBidRequestPartial(w.BidRequest)
		o.request(nil, w)
	case "nilreq":
		w.BidRequest = nil
		o.request(nil, w)
	case "assignlast":
		// exchange/utils.go's removeImpsWithStoredResponses, with a stored response for every
		// impression but the last.
		imps := w.Imp
		w.Imp = nil
		for i, imp := range imps {
			if i == len(imps)-1 {
				w.Imp = append(w.Imp, imp)
			}
		}
		o.request(nil, w)
	case "lenimp":
		o.int(w.LenImp())
	case "getimp":
		imps := w.GetImp()
		o.int(len(imps))
		for _, iw := range imps {
			o.marshal(iw.Imp)
		}
	case "setimp":
		w.SetImp(w.GetImp())
		o.request(nil, w)
	case "assignrev":
		if w.Imp != nil {
			rev := make([]openrtb2.Imp, len(w.Imp))
			for i := range w.Imp {
				rev[len(w.Imp)-1-i] = w.Imp[i]
			}
			w.Imp = rev
		}
		o.request(nil, w)
	case "dropimp":
		w.Imp = nil
		o.request(nil, w)
	case "rebuildimp":
		for _, iw := range w.GetImp() {
			o.err(iw.RebuildImp())
			o.marshal(iw.Imp)
		}
	case "get":
		r.get(target, o)
	case "set":
		r.set(target, arg, o)
	case "special":
		r.special(target, o)
	case "setext":
		var m map[string]json.RawMessage
		r.decode(arg, &m)
		r.setExt(target, func() map[string]json.RawMessage { return m }, o)
	case "setrawext":
		key, raw, _ := strings.Cut(arg, "=")
		m := map[string]json.RawMessage{key: json.RawMessage(raw)}
		r.setExt(target, func() map[string]json.RawMessage { return m }, o)
	case "touchext":
		for _, obj := range targets(target) {
			r.setExt(obj, nil, o)
		}
	case "touch":
		for _, obj := range targets(target) {
			r.touch(obj, o)
		}
	case "getorcreate":
		for _, iw := range w.GetImp() {
			e, _ := iw.GetImpExt()
			o.marshal(e.GetOrCreatePrebid())
			o.bool(e.Dirty())
		}
	default:
		log.Fatalf("unknown step %s", step)
	}
}

// targets are the objects of a step's target: all of them for "all".
func targets(target string) []string {
	if target == "all" {
		return objects
	}
	return []string{target}
}

// get records an object's GetXExt error and every getter.
func (r *runner) get(obj string, o *obs) {
	w := r.w
	switch obj {
	case "request":
		e, err := w.GetRequestExt()
		o.err(err)
		o.rawMap(e.GetExt())
		o.marshal(e.GetPrebid())
		o.marshal(e.GetSChain())
		o.bool(e.Dirty())
	case "user":
		e, err := w.GetUserExt()
		o.err(err)
		o.rawMap(e.GetExt())
		o.marshal(e.GetConsent())
		o.marshal(e.GetPrebid())
		o.marshal(e.GetEid())
		o.marshal(e.GetConsentedProvidersSettingsIn())
		o.marshal(e.GetConsentedProvidersSettingsOut())
		o.bool(e.Dirty())
	case "device":
		e, err := w.GetDeviceExt()
		o.err(err)
		o.rawMap(e.GetExt())
		o.marshal(e.GetPrebid())
		o.marshal(e.GetCDep())
		o.bool(e.Dirty())
	case "app":
		e, err := w.GetAppExt()
		o.err(err)
		o.rawMap(e.GetExt())
		o.marshal(e.GetPrebid())
		o.bool(e.Dirty())
	case "regs":
		e, err := w.GetRegExt()
		o.err(err)
		o.rawMap(e.GetExt())
		o.marshal(e.GetDSA())
		o.marshal(e.GetGDPR())
		o.marshal(e.GetGPC())
		o.marshal(e.GetUSPrivacy())
		o.bool(e.Dirty())
	case "site":
		e, err := w.GetSiteExt()
		o.err(err)
		o.rawMap(e.GetExt())
		o.marshal(e.GetAmp())
		o.bool(e.Dirty())
	case "dooh":
		e, err := w.GetDOOHExt()
		o.err(err)
		o.rawMap(e.GetExt())
		o.bool(e.Dirty())
	case "source":
		e, err := w.GetSourceExt()
		o.err(err)
		o.rawMap(e.GetExt())
		o.marshal(e.GetSChain())
		o.bool(e.Dirty())
	case "imp":
		imps := w.GetImp()
		o.int(len(imps))
		for _, iw := range imps {
			e, err := iw.GetImpExt()
			o.err(err)
			o.rawMap(e.GetExt())
			o.marshal(e.GetPrebid())
			o.marshal(e.GetData())
			o.marshal(e.GetTid())
			o.marshal(e.GetGpId())
			o.bool(e.Dirty())
		}
	default:
		log.Fatalf("unknown object %s", obj)
	}
}

// set calls a member's setter with arg decoded into its parameter.
func (r *runner) set(target, arg string, o *obs) {
	w := r.w
	switch target {
	case "request.prebid":
		var v *openrtb_ext.ExtRequestPrebid
		r.decode(arg, &v)
		e, _ := w.GetRequestExt()
		e.SetPrebid(v)
		o.bool(e.Dirty())
	case "request.schain":
		var v *openrtb2.SupplyChain
		r.decode(arg, &v)
		e, _ := w.GetRequestExt()
		e.SetSChain(v)
		o.bool(e.Dirty())
	case "user.consent":
		var v *string
		r.decode(arg, &v)
		e, _ := w.GetUserExt()
		e.SetConsent(v)
		o.bool(e.Dirty())
	case "user.prebid":
		var v *openrtb_ext.ExtUserPrebid
		r.decode(arg, &v)
		e, _ := w.GetUserExt()
		e.SetPrebid(v)
		o.bool(e.Dirty())
	case "user.eids":
		var v *[]openrtb2.EID
		if arg == "&nil" {
			var nilSlice []openrtb2.EID
			v = &nilSlice
		} else {
			r.decode(arg, &v)
		}
		e, _ := w.GetUserExt()
		e.SetEid(v)
		o.bool(e.Dirty())
	case "user.cpsin":
		var v *openrtb_ext.ConsentedProvidersSettingsIn
		r.decode(arg, &v)
		e, _ := w.GetUserExt()
		e.SetConsentedProvidersSettingsIn(v)
		o.bool(e.Dirty())
	case "user.cpsout":
		var v *openrtb_ext.ConsentedProvidersSettingsOut
		r.decode(arg, &v)
		e, _ := w.GetUserExt()
		e.SetConsentedProvidersSettingsOut(v)
		o.bool(e.Dirty())
	case "device.prebid":
		var v *openrtb_ext.ExtDevicePrebid
		r.decode(arg, &v)
		e, _ := w.GetDeviceExt()
		e.SetPrebid(v)
		o.bool(e.Dirty())
	case "device.cdep":
		var v string
		r.decode(arg, &v)
		e, _ := w.GetDeviceExt()
		e.SetCDep(v)
		o.bool(e.Dirty())
	case "app.prebid":
		var v *openrtb_ext.ExtAppPrebid
		r.decode(arg, &v)
		e, _ := w.GetAppExt()
		e.SetPrebid(v)
		o.bool(e.Dirty())
	case "regs.dsa":
		var v *openrtb_ext.ExtRegsDSA
		r.decode(arg, &v)
		e, _ := w.GetRegExt()
		e.SetDSA(v)
		o.bool(e.Dirty())
	case "regs.gdpr":
		var v *int8
		r.decode(arg, &v)
		e, _ := w.GetRegExt()
		e.SetGDPR(v)
		o.bool(e.Dirty())
	case "regs.gpc":
		var v *string
		r.decode(arg, &v)
		e, _ := w.GetRegExt()
		e.SetGPC(v)
		o.bool(e.Dirty())
	case "regs.usprivacy":
		var v string
		r.decode(arg, &v)
		e, _ := w.GetRegExt()
		e.SetUSPrivacy(v)
		o.bool(e.Dirty())
	case "site.amp":
		var v *int8
		r.decode(arg, &v)
		e, _ := w.GetSiteExt()
		e.SetAmp(v)
		o.bool(e.Dirty())
	case "source.schain":
		var v *openrtb2.SupplyChain
		r.decode(arg, &v)
		e, _ := w.GetSourceExt()
		e.SetSChain(v)
		o.bool(e.Dirty())
	case "imp.prebid":
		for _, iw := range w.GetImp() {
			var v *openrtb_ext.ExtImpPrebid
			r.decode(arg, &v)
			e, _ := iw.GetImpExt()
			e.SetPrebid(v)
			o.bool(e.Dirty())
		}
	case "imp.tid":
		for _, iw := range w.GetImp() {
			var v string
			r.decode(arg, &v)
			e, _ := iw.GetImpExt()
			e.SetTid(v)
			o.bool(e.Dirty())
		}
	default:
		log.Fatalf("unknown member %s", target)
	}
}

// special calls a setter with a value that JSON cannot give: NaN and infinite floats, and raw
// messages that are invalid or not compact (which jsonutil.Marshal leaves out or compacts).
func (r *runner) special(target string, o *obs) {
	w := r.w
	switch target {
	case "request.prebid.nan":
		e, _ := w.GetRequestExt()
		e.SetPrebid(&openrtb_ext.ExtRequestPrebid{Debug: true, BidAdjustmentFactors: map[string]float64{"a": math.NaN()}})
		o.bool(e.Dirty())
	case "imp.prebid.inf":
		for _, iw := range w.GetImp() {
			e, _ := iw.GetImpExt()
			e.SetPrebid(&openrtb_ext.ExtImpPrebid{Floors: &openrtb_ext.ExtImpPrebidFloors{FloorRuleValue: math.Inf(1)}})
			o.bool(e.Dirty())
		}
	case "user.eids.badraw":
		e, _ := w.GetUserExt()
		e.SetEid(&[]openrtb2.EID{{Source: "s", Ext: json.RawMessage(`{bad`)}})
		o.bool(e.Dirty())
	case "source.schain.spacedraw":
		e, _ := w.GetSourceExt()
		e.SetSChain(&openrtb2.SupplyChain{Ver: "1", Ext: json.RawMessage(` { "a" : 1 } `)})
		o.bool(e.Dirty())
	default:
		log.Fatalf("unknown special member %s", target)
	}
}

// extObject is what every ext object has.
type extObject interface {
	GetExt() map[string]json.RawMessage
	SetExt(map[string]json.RawMessage)
	Dirty() bool
}

// object is the ext object obj (GetXExt, its error ignored); for imp, each wrapper's.
func (r *runner) object(obj string) []extObject {
	w := r.w
	switch obj {
	case "request":
		e, _ := w.GetRequestExt()
		return []extObject{e}
	case "user":
		e, _ := w.GetUserExt()
		return []extObject{e}
	case "device":
		e, _ := w.GetDeviceExt()
		return []extObject{e}
	case "app":
		e, _ := w.GetAppExt()
		return []extObject{e}
	case "regs":
		e, _ := w.GetRegExt()
		return []extObject{e}
	case "site":
		e, _ := w.GetSiteExt()
		return []extObject{e}
	case "dooh":
		e, _ := w.GetDOOHExt()
		return []extObject{e}
	case "source":
		e, _ := w.GetSourceExt()
		return []extObject{e}
	case "imp":
		var out []extObject
		for _, iw := range w.GetImp() {
			e, _ := iw.GetImpExt()
			out = append(out, e)
		}
		return out
	}
	log.Fatalf("unknown object %s", obj)
	return nil
}

// setExt calls SetExt with what ext returns, or with GetExt() when ext is nil.
func (r *runner) setExt(obj string, ext func() map[string]json.RawMessage, o *obs) {
	for _, e := range r.object(obj) {
		if ext == nil {
			e.SetExt(e.GetExt())
		} else {
			e.SetExt(ext())
		}
		o.bool(e.Dirty())
	}
}

// touch sets every member of an object to what its getter returns.
func (r *runner) touch(obj string, o *obs) {
	w := r.w
	switch obj {
	case "request":
		e, _ := w.GetRequestExt()
		e.SetExt(e.GetExt())
		e.SetPrebid(e.GetPrebid())
		e.SetSChain(e.GetSChain())
		o.bool(e.Dirty())
	case "user":
		e, _ := w.GetUserExt()
		e.SetExt(e.GetExt())
		e.SetConsent(e.GetConsent())
		e.SetPrebid(e.GetPrebid())
		e.SetEid(e.GetEid())
		e.SetConsentedProvidersSettingsIn(e.GetConsentedProvidersSettingsIn())
		e.SetConsentedProvidersSettingsOut(e.GetConsentedProvidersSettingsOut())
		o.bool(e.Dirty())
	case "device":
		e, _ := w.GetDeviceExt()
		e.SetExt(e.GetExt())
		e.SetPrebid(e.GetPrebid())
		e.SetCDep(e.GetCDep())
		o.bool(e.Dirty())
	case "app":
		e, _ := w.GetAppExt()
		e.SetExt(e.GetExt())
		e.SetPrebid(e.GetPrebid())
		o.bool(e.Dirty())
	case "regs":
		e, _ := w.GetRegExt()
		e.SetExt(e.GetExt())
		e.SetDSA(e.GetDSA())
		e.SetGDPR(e.GetGDPR())
		e.SetGPC(e.GetGPC())
		e.SetUSPrivacy(e.GetUSPrivacy())
		o.bool(e.Dirty())
	case "site":
		e, _ := w.GetSiteExt()
		e.SetExt(e.GetExt())
		e.SetAmp(e.GetAmp())
		o.bool(e.Dirty())
	case "dooh":
		e, _ := w.GetDOOHExt()
		e.SetExt(e.GetExt())
		o.bool(e.Dirty())
	case "source":
		e, _ := w.GetSourceExt()
		e.SetExt(e.GetExt())
		e.SetSChain(e.GetSChain())
		o.bool(e.Dirty())
	case "imp":
		for _, iw := range w.GetImp() {
			e, _ := iw.GetImpExt()
			e.SetExt(e.GetExt())
			e.SetPrebid(e.GetPrebid())
			e.SetTid(e.GetTid())
			o.bool(e.Dirty())
		}
	default:
		log.Fatalf("unknown object %s", obj)
	}
}
