package main

import (
	"encoding/json"
	"errors"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// Small has few fields, so jsoniter decodes it with a hashed field decoder.
type Small struct {
	ID    string          `json:"id"`
	W     *int64          `json:"w,omitempty"`
	H     int64           `json:"h,omitempty"`
	Price float64         `json:"price"`
	Tags  []string        `json:"tags,omitempty"`
	Ext   json.RawMessage `json:"ext,omitempty"`
}

// Wide has more than ten fields, so jsoniter uses its general struct decoder.
type Wide struct {
	A string           `json:"a"`
	B int              `json:"b"`
	C uint8            `json:"c"`
	D int8             `json:"d"`
	E float32          `json:"e"`
	F bool             `json:"f"`
	G *string          `json:"g"`
	H []int            `json:"h"`
	I map[string]int   `json:"i"`
	J interface{}      `json:"j"`
	K []Small          `json:"k"`
	L *Small           `json:"l"`
	M json.RawMessage  `json:"m"`
	N []byte           `json:"n"`
	O map[int64]string `json:"o"`
	P *json.RawMessage `json:"p"`
}

// Upper has an upper-case JSON name (which forces jsoniter's general decoder), untagged and
// unexported fields, and the "-" tags.
type Upper struct {
	ID       string `json:"ID"`
	Name     string
	Count    int `json:"count,omitempty"`
	internal int
	Skipped  string `json:"-"`
	Dash     string `json:"-,"`
}

// Base is embedded by value in Embeds.
type Base struct {
	ID   string `json:"id"`
	Name string `json:"name"`
	Dup  int    `json:"dup"`
}

// Deep is embedded two levels down in Embeds.
type Deep struct {
	Level int    `json:"level"`
	Name  string `json:"name"`
}

// Extra is embedded by pointer in Embeds.
type Extra struct {
	Dup  int    `json:"dup"`
	Note string `json:"note,omitempty"`
	Deep
}

// Embeds exercises embedded struct promotion, shadowing and ambiguous names.
type Embeds struct {
	Base
	*Extra
	Name string `json:"name"`
	Own  int    `json:"own"`
}

// Quoted exercises the ",string" option.
type Quoted struct {
	N int     `json:",string"`
	S string  `json:"s,string"`
	P *int    `json:"p,string"`
	B bool    `json:"b,string"`
	F float64 `json:"f,string,omitempty"`
	U uint16  `json:"u,string"`
}

// Validated has an UnmarshalJSON method that rejects values, like the request validation of
// openrtb_ext.ExtDevicePrebid.
type Validated struct {
	N int
}

// UnmarshalJSON accepts integers up to 100.
func (v *Validated) UnmarshalJSON(b []byte) error {
	n, err := strconv.Atoi(string(b))
	if err != nil {
		return errors.New("validated value must be an integer")
	}
	if n > 100 {
		return errors.New("request.v must be at most 100")
	}
	v.N = n
	return nil
}

// Custom holds types with custom UnmarshalJSON methods.
type Custom struct {
	SI jsonutil.StringInt `json:"si"`
	IS jsonutil.IntString `json:"is"`
	V  Validated          `json:"v"`
	PV *Validated         `json:"pv"`
}

// Shout has a MarshalJSON method whose output needs compaction, and fails for "bad".
type Shout string

// MarshalJSON writes the upper-cased string with a trailing newline.
func (s Shout) MarshalJSON() ([]byte, error) {
	switch s {
	case "":
		return nil, errors.New("empty shout")
	case "bad":
		return []byte(`{bad`), nil
	}
	return []byte(" \"" + strings.ToUpper(string(s)) + "\"\n"), nil
}

// Loud holds Shout values.
type Loud struct {
	S Shout  `json:"s"`
	T Shout  `json:"t,omitempty"`
	P *Shout `json:"p,omitempty"`
}

// Maps holds maps with struct, pointer and integer-keyed elements.
type Maps struct {
	Smalls map[string]Small           `json:"smalls"`
	Ptrs   map[string]*int            `json:"ptrs"`
	Bools  map[uint8]bool             `json:"bools"`
	Raw    map[string]json.RawMessage `json:"raw"`
}

// Empty has no fields (jsoniter's skipObjectDecoder).
type Empty struct{}

// OnlyPrivate has no exported fields.
type OnlyPrivate struct {
	a int
	b string
}

// Key is a named string map key type, like openrtb_ext.BidderName.
type Key string

// Slot is a named integer map key type.
type Slot int8

// Numbers holds json.Number fields: jsoniter's jsonNumberCodec and encoding/json's numberType
// special cases.
type Numbers struct {
	N json.Number   `json:"n"`
	O json.Number   `json:"o,omitempty"`
	S json.Number   `json:"s,string,omitempty"`
	P *json.Number  `json:"p,omitempty"`
	L []json.Number `json:"l,omitempty"`
}

// AnonMeta, AnonPrebid and AnonTop are aliases, not named types: they spell the unnamed struct
// types that adapters declare inline as decode targets (var ext struct { ... }).
type AnonMeta = struct {
	Count int     `json:"count,omitempty"`
	Price float64 `json:"price,omitempty"`
}

// AnonPrebid is an unnamed struct type holding a pointer to another one.
type AnonPrebid = struct {
	Type string    `json:"type"`
	Meta *AnonMeta `json:"meta,omitempty"`
}

// AnonTop is an unnamed struct type, also used as a decode target itself.
type AnonTop = struct {
	A int      `json:"a"`
	B []string `json:"b,omitempty"`
}

// Anon holds fields of unnamed struct types.
type Anon struct {
	Prebid AnonPrebid `json:"prebid"`
	List   []AnonTop  `json:"list,omitempty"`
}
