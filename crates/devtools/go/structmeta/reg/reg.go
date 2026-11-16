// Package reg is the registry the structmeta overlay files write into.
//
// Go cannot enumerate a package's types or constants at run time, so `structmeta -plan` parses
// each target package and writes one overlay file per package whose init registers every named
// type (exported or not) and every exported typed constant here. `structmeta -out` is then run
// with `go run -overlay`, so the registrations are compiled into the real packages and reflect
// sees the real types.
package reg

import "reflect"

// Type is one registered named type.
type Type struct {
	PkgPath string
	Name    string
	Type    reflect.Type
	// Alias is true for `type Name = Other`.
	Alias bool
}

// Const is one registered constant.
type Const struct {
	PkgPath string
	Name    string
	Value   interface{}
	// Order is the constant's position in its package (files sorted by name, then source order).
	Order int
}

// PackageInfo identifies a package whose overlay ran.
type PackageInfo struct {
	Path string
	Name string
}

// Skipped is a declaration the overlay could not register, with the reason.
type Skipped struct {
	PkgPath string
	Name    string
	Reason  string
}

var (
	types    []Type
	consts   []Const
	skipped  []Skipped
	packages []PackageInfo
)

// Package records that the overlay for pkgPath (package name) ran.
func Package(pkgPath, name string) {
	packages = append(packages, PackageInfo{Path: pkgPath, Name: name})
}

// AddType registers a named type.
func AddType(pkgPath, name string, t reflect.Type, alias bool) {
	types = append(types, Type{PkgPath: pkgPath, Name: name, Type: t, Alias: alias})
}

// AddConst registers a typed constant.
func AddConst(pkgPath, name string, order int, value interface{}) {
	consts = append(consts, Const{PkgPath: pkgPath, Name: name, Value: value, Order: order})
}

// Skip records a declaration that could not be registered.
func Skip(pkgPath, name, reason string) {
	skipped = append(skipped, Skipped{PkgPath: pkgPath, Name: name, Reason: reason})
}

// Packages returns the packages whose overlay ran.
func Packages() []PackageInfo { return packages }

// Types returns every registered type.
func Types() []Type { return types }

// Consts returns every registered constant.
func Consts() []Const { return consts }

// Skips returns every skipped declaration.
func Skips() []Skipped { return skipped }
