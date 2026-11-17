package main

import (
	"time"

	"github.com/prebid/prebid-server/v4/config"
)

// Kitchen has a field of every kind yaml.v3 and mapstructure decode into. tests/golden.rs
// mirrors it field for field.
type Kitchen struct {
	Int      int                    `yaml:"int" mapstructure:"int"`
	Int8     int8                   `yaml:"int8" mapstructure:"int8"`
	Int16    int16                  `yaml:"int16" mapstructure:"int16"`
	Int32    int32                  `yaml:"int32" mapstructure:"int32"`
	Int64    int64                  `yaml:"int64" mapstructure:"int64"`
	Uint     uint                   `yaml:"uint" mapstructure:"uint"`
	Uint8    uint8                  `yaml:"uint8" mapstructure:"uint8"`
	Uint16   uint16                 `yaml:"uint16" mapstructure:"uint16"`
	Uint32   uint32                 `yaml:"uint32" mapstructure:"uint32"`
	Uint64   uint64                 `yaml:"uint64" mapstructure:"uint64"`
	Float32  float32                `yaml:"float32" mapstructure:"float32"`
	Float64  float64                `yaml:"float64" mapstructure:"float64"`
	Bool     bool                   `yaml:"bool" mapstructure:"bool"`
	String   string                 `yaml:"string" mapstructure:"string"`
	Duration time.Duration          `yaml:"duration" mapstructure:"duration"`
	Time     time.Time              `yaml:"time" mapstructure:"time"`
	PtrInt   *int                   `yaml:"ptrint" mapstructure:"ptrint"`
	PtrStr   *string                `yaml:"ptrstr" mapstructure:"ptrstr"`
	PtrBool  *bool                  `yaml:"ptrbool" mapstructure:"ptrbool"`
	PtrDur   *time.Duration         `yaml:"ptrdur" mapstructure:"ptrdur"`
	PtrPtr   **int                  `yaml:"ptrptr" mapstructure:"ptrptr"`
	Ints     []int                  `yaml:"ints" mapstructure:"ints"`
	Strings  []string               `yaml:"strings" mapstructure:"strings"`
	Bytes    []byte                 `yaml:"bytes" mapstructure:"bytes"`
	Any      interface{}            `yaml:"any" mapstructure:"any"`
	Anys     []interface{}          `yaml:"anys" mapstructure:"anys"`
	StrMap   map[string]string      `yaml:"strmap" mapstructure:"strmap"`
	IntMap   map[string]int         `yaml:"intmap" mapstructure:"intmap"`
	AnyMap   map[string]interface{} `yaml:"anymap" mapstructure:"anymap"`
	KeyMap   map[int]string         `yaml:"keymap" mapstructure:"keymap"`
	Nested   *Nested                `yaml:"nested" mapstructure:"nested"`
	Value    Nested                 `yaml:"value" mapstructure:"value"`
	Nesteds  []Nested               `yaml:"nesteds" mapstructure:"nesteds"`
	BidType  BidType                `yaml:"bidtype" mapstructure:"bidtype"`
	BidTypes []BidType              `yaml:"bidtypes" mapstructure:"bidtypes"`
	Plain    string
	Inline   `yaml:",inline" mapstructure:",squash"`
	Extra    map[string]interface{} `yaml:",inline" mapstructure:",remain"`
	Skipped  string                 `yaml:"-" mapstructure:"-"`
	hidden   string
}

// Nested is a struct field of Kitchen.
type Nested struct {
	A   int      `yaml:"a" mapstructure:"a"`
	B   []string `yaml:"b" mapstructure:"b"`
	Ptr *Nested  `yaml:"ptr" mapstructure:"ptr"`
}

// Inline is inlined into Kitchen (squashed by mapstructure).
type Inline struct {
	InA string `yaml:"ina" mapstructure:"ina"`
	InB *int   `yaml:"inb" mapstructure:"inb"`
}

// BidType stands for openrtb_ext.BidType: a named string type.
type BidType string

// aliasNillableFields is config.aliasNillableFields, which processBidderInfos decodes alias
// bidder files into a second time.
type aliasNillableFields struct {
	Disabled                *bool                        `yaml:"disabled" mapstructure:"disabled"`
	ModifyingVastXmlAllowed *bool                        `yaml:"modifyingVastXmlAllowed" mapstructure:"modifyingVastXmlAllowed"`
	Experiment              *config.BidderInfoExperiment `yaml:"experiment" mapstructure:"experiment"`
	XAPI                    *config.AdapterXAPI          `yaml:"xapi" mapstructure:"xapi"`
}

// The mapstructure v1.5.0 test types (mapstructure_test.go) whose inputs the goldens replay;
// arrays, json.Number, functions and io.Writer fields are left out.

type Basic struct {
	Vstring     string
	Vint        int
	Vint8       int8
	Vint16      int16
	Vint32      int32
	Vint64      int64
	Vuint       uint
	Vbool       bool
	Vfloat      float64
	Vextra      string
	vsilent     bool
	Vdata       interface{}
	VjsonInt    int
	VjsonUint   uint
	VjsonUint64 uint64
	VjsonFloat  float64
}

type BasicPointer struct {
	Vstring    *string
	Vint       *int
	Vuint      *uint
	Vbool      *bool
	Vfloat     *float64
	Vextra     *string
	vsilent    *bool
	Vdata      *interface{}
	VjsonInt   *int
	VjsonFloat *float64
}

type BasicSquash struct {
	Test Basic `mapstructure:",squash"`
}

type Embedded struct {
	Basic
	Vunique string
}

type EmbeddedPointer struct {
	*Basic
	Vunique string
}

type EmbeddedSquash struct {
	Basic   `mapstructure:",squash"`
	Vunique string
}

type EmbeddedAndNamed struct {
	Basic
	Named   Basic
	Vunique string
}

type SliceAlias []string

type EmbeddedSlice struct {
	SliceAlias `mapstructure:"slice_alias"`
	Vunique    string
}

type Map struct {
	Vfoo   string
	Vother map[string]string
}

type MapOfStruct struct {
	Value map[string]Basic
}

type NestedBasic struct {
	Vfoo string
	Vbar Basic
}

type NestedPointer struct {
	Vfoo string
	Vbar *Basic
}

type Slice struct {
	Vfoo string
	Vbar []string
}

type SliceOfAlias struct {
	Vfoo string
	Vbar SliceAlias
}

type SliceOfStruct struct {
	Value []Basic
}

type SlicePointer struct {
	Vbar *[]string
}

type Tagged struct {
	Extra string `mapstructure:"bar,what,what"`
	Value string `mapstructure:"foo"`
}

type Remainder struct {
	A     string
	Extra map[string]interface{} `mapstructure:",remain"`
}

type TypeConversionResult struct {
	IntToFloat         float32
	IntToUint          uint
	IntToBool          bool
	IntToString        string
	UintToInt          int
	UintToFloat        float32
	UintToBool         bool
	UintToString       string
	BoolToInt          int
	BoolToUint         uint
	BoolToFloat        float32
	BoolToString       string
	FloatToInt         int
	FloatToUint        uint
	FloatToBool        bool
	FloatToString      string
	SliceUint8ToString string
	StringToSliceUint8 []byte
	StringToInt        int
	StringToUint       uint
	StringToBool       bool
	StringToFloat      float32
	StringToStrSlice   []string
	StringToIntSlice   []int
	SliceToMap         map[string]interface{}
	MapToSlice         []interface{}
}

// modulesHolder holds a config.AccountModules, the type config.AccountModulesHookFunc converts.
type modulesHolder struct {
	Modules config.AccountModules `mapstructure:"modules"`
}

// msTargets makes a new value of each mapstructure target type, by the name the goldens use.
var msTargets = map[string]func() interface{}{
	"Kitchen":              func() interface{} { return new(Kitchen) },
	"BidderInfo":           func() interface{} { return new(config.BidderInfo) },
	"Basic":                func() interface{} { return new(Basic) },
	"BasicPointer":         func() interface{} { return new(BasicPointer) },
	"BasicSquash":          func() interface{} { return new(BasicSquash) },
	"Embedded":             func() interface{} { return new(Embedded) },
	"EmbeddedPointer":      func() interface{} { return new(EmbeddedPointer) },
	"EmbeddedSquash":       func() interface{} { return new(EmbeddedSquash) },
	"EmbeddedAndNamed":     func() interface{} { return new(EmbeddedAndNamed) },
	"EmbeddedSlice":        func() interface{} { return new(EmbeddedSlice) },
	"Map":                  func() interface{} { return new(Map) },
	"MapOfStruct":          func() interface{} { return new(MapOfStruct) },
	"NestedBasic":          func() interface{} { return new(NestedBasic) },
	"NestedPointer":        func() interface{} { return new(NestedPointer) },
	"Slice":                func() interface{} { return new(Slice) },
	"SliceOfAlias":         func() interface{} { return new(SliceOfAlias) },
	"SliceOfStruct":        func() interface{} { return new(SliceOfStruct) },
	"SlicePointer":         func() interface{} { return new(SlicePointer) },
	"Tagged":               func() interface{} { return new(Tagged) },
	"Remainder":            func() interface{} { return new(Remainder) },
	"TypeConversionResult": func() interface{} { return new(TypeConversionResult) },
	"modulesHolder":        func() interface{} { return new(modulesHolder) },
	"map[string]interface {}": func() interface{} {
		return new(map[string]interface{})
	},
	"map[string]string": func() interface{} { return new(map[string]string) },
	"[]string":          func() interface{} { return new([]string) },
	"[]interface {}":    func() interface{} { return new([]interface{}) },
	"interface {}":      func() interface{} { return new(interface{}) },
	"string":            func() interface{} { return new(string) },
	"int":               func() interface{} { return new(int) },
	"*int":              func() interface{} { return new(*int) },
	"bool":              func() interface{} { return new(bool) },
	"float64":           func() interface{} { return new(float64) },
	"time.Duration":     func() interface{} { return new(time.Duration) },
	"int8":              func() interface{} { return new(int8) },
	"uint8":             func() interface{} { return new(uint8) },
	"uint":              func() interface{} { return new(uint) },
	"float32":           func() interface{} { return new(float32) },
	"[]int":             func() interface{} { return new([]int) },
	"[]uint8":           func() interface{} { return new([]uint8) },
	"map[string]int":    func() interface{} { return new(map[string]int) },
	"map[int]string":    func() interface{} { return new(map[int]string) },
	"map[string][]string": func() interface{} {
		return new(map[string][]string)
	},
	"[]map[string]string": func() interface{} {
		return new([]map[string]string)
	},
	"time.Time":  func() interface{} { return new(time.Time) },
	"*time.Time": func() interface{} { return new(*time.Time) },
	"Nested":     func() interface{} { return new(Nested) },
	"*Nested":    func() interface{} { return new(*Nested) },
	"**int":      func() interface{} { return new(**int) },
}
