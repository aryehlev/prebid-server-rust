// Package shadow takes trace snapshots of live, mutable request state without
// changing it, which is the parity observer-effect rule.
//
// An openrtb_ext.RequestWrapper caches parsed ext objects. Changes made through
// them stay invisible in the embedded BidRequest until RebuildRequest runs, and
// RebuildRequest itself mutates the wrapper: it rewrites the BidRequest, the
// imps and every ext cache, and clears their dirty flags. The endpoints do not
// rebuild before HoldAuction, so a snapshot of wrapper.BidRequest would miss
// those changes, and a rebuild for tracing would change when and how Go itself
// rebuilds.
//
// RequestWrapper.Clone cannot help: the clone shares the BidRequest and the imps
// with the original, so rebuilding it writes into the live request, and
// ExtRequestPrebid.Clone drops fields such as targeting.mediatypepricegranularity.
// Copy makes a complete, independent deep copy instead, unexported fields
// included, and RebuiltRequest rebuilds only that copy.
package shadow

import (
	"errors"
	"fmt"
	"reflect"
	"unsafe"

	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
)

// ErrNoRequest is returned by RebuiltRequest when there is no BidRequest to
// snapshot.
var ErrNoRequest = errors.New("shadow: request wrapper has no BidRequest")

// RebuiltRequest returns the BidRequest that rw.RebuildRequest would produce at
// this moment. It rebuilds an independent deep copy, so rw, its BidRequest, its
// imp wrappers and its ext caches are left exactly as they were. A panic inside
// the copy or the rebuild is returned as an error instead of crossing into the
// request handler.
func RebuiltRequest(rw *openrtb_ext.RequestWrapper) (req *openrtb2.BidRequest, err error) {
	if rw == nil || rw.BidRequest == nil {
		return nil, ErrNoRequest
	}
	cp, err := Copy(rw)
	if err != nil {
		return nil, fmt.Errorf("shadow: copying the request wrapper: %w", err)
	}
	defer func() {
		if r := recover(); r != nil {
			req, err = nil, fmt.Errorf("shadow: rebuilding the request copy panicked: %v", r)
		}
	}()
	if err := cp.RebuildRequest(); err != nil {
		return nil, fmt.Errorf("shadow: rebuilding the request copy: %w", err)
	}
	return cp.BidRequest, nil
}

// Copy returns a deep copy of *src, unexported fields included. Pointer
// aliasing inside the value is preserved (each distinct pointer is copied once),
// map keys are shared because they are immutable in practice, and functions,
// channels and the immutable standard types listed in sharedPackages are shared
// rather than copied. src itself is only read.
func Copy[T any](src *T) (dst *T, err error) {
	if src == nil {
		return nil, nil
	}
	defer func() {
		if r := recover(); r != nil {
			dst, err = nil, fmt.Errorf("shadow: deep copy panicked: %v", r)
		}
	}()
	c := copier{seen: make(map[seenKey]reflect.Value)}
	in := reflect.ValueOf(src)
	out := reflect.New(in.Type().Elem())
	// Register the root so pointers back to it resolve to the copy.
	c.seen[seenKey{addr: in.Pointer(), typ: in.Type()}] = out
	if err := c.copyValue(out.Elem(), in.Elem()); err != nil {
		return nil, err
	}
	copied, ok := out.Interface().(*T)
	if !ok {
		return nil, fmt.Errorf("shadow: deep copy produced %s", out.Type())
	}
	return copied, nil
}

// sharedPackages hold struct types that are immutable or must not be copied by
// value (locks, compiled regexps, time locations). They are shared, not copied.
var sharedPackages = map[string]bool{
	"time":        true,
	"sync":        true,
	"sync/atomic": true,
	"regexp":      true,
}

type seenKey struct {
	addr uintptr
	typ  reflect.Type
}

type copier struct {
	seen map[seenKey]reflect.Value
}

// copyValue deep-copies src into dst. Both must be settable views (see
// writable), so unexported fields can be read and written alike.
func (c *copier) copyValue(dst, src reflect.Value) error {
	switch src.Kind() {
	case reflect.Pointer:
		return c.copyPointer(dst, src)
	case reflect.Struct:
		return c.copyStruct(dst, src)
	case reflect.Slice:
		return c.copySlice(dst, src)
	case reflect.Array:
		for i := 0; i < src.Len(); i++ {
			if err := c.copyValue(dst.Index(i), src.Index(i)); err != nil {
				return fmt.Errorf("[%d]: %w", i, err)
			}
		}
		return nil
	case reflect.Map:
		return c.copyMap(dst, src)
	case reflect.Interface:
		return c.copyInterface(dst, src)
	default:
		// Scalars and strings are values; functions, channels and unsafe
		// pointers are shared handles.
		dst.Set(src)
		return nil
	}
}

func (c *copier) copyPointer(dst, src reflect.Value) error {
	if src.IsNil() {
		return nil
	}
	if isShared(src.Type().Elem()) {
		dst.Set(src)
		return nil
	}
	key := seenKey{addr: src.Pointer(), typ: src.Type()}
	if p, ok := c.seen[key]; ok {
		dst.Set(p)
		return nil
	}
	p := reflect.New(src.Type().Elem())
	c.seen[key] = p
	dst.Set(p)
	return c.copyValue(p.Elem(), src.Elem())
}

func (c *copier) copyStruct(dst, src reflect.Value) error {
	if isShared(src.Type()) {
		dst.Set(src)
		return nil
	}
	for i := 0; i < src.NumField(); i++ {
		if err := c.copyValue(writable(dst.Field(i)), writable(src.Field(i))); err != nil {
			return fmt.Errorf("%s.%s: %w", src.Type(), src.Type().Field(i).Name, err)
		}
	}
	return nil
}

func (c *copier) copySlice(dst, src reflect.Value) error {
	if src.IsNil() {
		return nil
	}
	n := src.Len()
	out := reflect.MakeSlice(src.Type(), n, n)
	if src.Type().Elem().Kind() == reflect.Uint8 {
		reflect.Copy(out, src)
	} else {
		for i := 0; i < n; i++ {
			if err := c.copyValue(out.Index(i), src.Index(i)); err != nil {
				return fmt.Errorf("[%d]: %w", i, err)
			}
		}
	}
	dst.Set(out)
	return nil
}

func (c *copier) copyMap(dst, src reflect.Value) error {
	if src.IsNil() {
		return nil
	}
	out := reflect.MakeMapWithSize(src.Type(), src.Len())
	iter := src.MapRange()
	for iter.Next() {
		v := reflect.New(src.Type().Elem()).Elem()
		if err := c.copyValue(v, addressable(iter.Value())); err != nil {
			return fmt.Errorf("[%v]: %w", iter.Key(), err)
		}
		out.SetMapIndex(iter.Key(), v)
	}
	dst.Set(out)
	return nil
}

func (c *copier) copyInterface(dst, src reflect.Value) error {
	if src.IsNil() {
		return nil
	}
	elem := src.Elem()
	v := reflect.New(elem.Type()).Elem()
	if err := c.copyValue(v, addressable(elem)); err != nil {
		return err
	}
	dst.Set(v)
	return nil
}

// writable returns a settable view of an addressable value, including values
// reached through unexported struct fields, which reflect otherwise marks
// read-only.
func writable(v reflect.Value) reflect.Value {
	if v.CanSet() {
		return v
	}
	return reflect.NewAt(v.Type(), unsafe.Pointer(v.UnsafeAddr())).Elem()
}

// addressable returns an addressable copy of a value that reflect does not let
// us address (map entries, interface contents), so its fields can be read
// through writable.
func addressable(v reflect.Value) reflect.Value {
	tmp := reflect.New(v.Type()).Elem()
	tmp.Set(v)
	return tmp
}

func isShared(t reflect.Type) bool {
	return t.Kind() == reflect.Struct && sharedPackages[t.PkgPath()]
}
