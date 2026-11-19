package main

import (
	"encoding/asn1"
	"math/big"
	"net"
	"strings"
	"time"
)

func tlv(tag byte, parts ...[]byte) []byte {
	var content []byte
	for _, p := range parts {
		content = append(content, p...)
	}
	n := len(content)
	var hdr []byte
	switch {
	case n < 0x80:
		hdr = []byte{tag, byte(n)}
	case n < 0x100:
		hdr = []byte{tag, 0x81, byte(n)}
	case n < 0x10000:
		hdr = []byte{tag, 0x82, byte(n >> 8), byte(n)}
	default:
		hdr = []byte{tag, 0x83, byte(n >> 16), byte(n >> 8), byte(n)}
	}
	return append(hdr, content...)
}

func seq(parts ...[]byte) []byte { return tlv(0x30, parts...) }
func set(parts ...[]byte) []byte { return tlv(0x31, parts...) }

func base128(n *big.Int) []byte {
	if n.Sign() == 0 {
		return []byte{0}
	}
	var groups []byte
	m := new(big.Int).Set(n)
	for m.Sign() > 0 {
		groups = append([]byte{byte(new(big.Int).And(m, big.NewInt(0x7f)).Int64())}, groups...)
		m.Rsh(m, 7)
	}
	for i := 0; i < len(groups)-1; i++ {
		groups[i] |= 0x80
	}
	return groups
}

func oidContent(s string) []byte {
	parts := strings.Split(s, ".")
	arcs := make([]*big.Int, len(parts))
	for i, p := range parts {
		arcs[i], _ = new(big.Int).SetString(p, 10)
	}
	first := new(big.Int).Mul(arcs[0], big.NewInt(40))
	first.Add(first, arcs[1])
	out := base128(first)
	for _, a := range arcs[2:] {
		out = append(out, base128(a)...)
	}
	return out
}

func oid(s string) []byte { return tlv(0x06, oidContent(s)) }

func integer(v int64) []byte {
	b, err := asn1.Marshal(v)
	if err != nil {
		panic(err)
	}
	return b
}

func bigInteger(v *big.Int) []byte {
	b, err := asn1.Marshal(v)
	if err != nil {
		panic(err)
	}
	return b
}

// intContent is the contents of a minimal INTEGER for v.
func intContent(v int64) []byte {
	b := integer(v)
	return b[2:]
}

func boolean(v bool) []byte {
	if v {
		return tlv(0x01, []byte{0xff})
	}
	return tlv(0x01, []byte{0x00})
}

func null() []byte { return []byte{0x05, 0x00} }

func bitString(b []byte) []byte { return tlv(0x03, append([]byte{0}, b...)) }

func octets(b []byte) []byte { return tlv(0x04, b) }

func utf8String(s string) []byte { return tlv(0x0c, []byte(s)) }

func utcTime(t time.Time) []byte {
	return tlv(0x17, []byte(t.UTC().Format("060102150405Z")))
}

func generalizedTime(t time.Time) []byte {
	return tlv(0x18, []byte(t.UTC().Format("20060102150405Z")))
}

func timeDER(t time.Time) []byte {
	if t.UTC().Year() >= 1950 && t.UTC().Year() < 2050 {
		return utcTime(t)
	}
	return generalizedTime(t)
}

// name is a Name with a common name (when non-empty) and an organization (when non-empty).
func name(cn, org string) []byte {
	var rdns [][]byte
	if org != "" {
		rdns = append(rdns, set(seq(oid("2.5.4.10"), utf8String(org))))
	}
	if cn != "" {
		rdns = append(rdns, set(seq(oid("2.5.4.3"), utf8String(cn))))
	}
	return seq(rdns...)
}

func ext(id string, critical bool, value []byte) []byte {
	parts := [][]byte{oid(id)}
	if critical {
		parts = append(parts, boolean(true))
	}
	parts = append(parts, octets(value))
	return seq(parts...)
}

func bcExt(critical, ca bool, pathLen int) []byte {
	var p [][]byte
	if ca {
		p = append(p, boolean(true))
	}
	if pathLen >= 0 {
		p = append(p, integer(int64(pathLen)))
	}
	return ext("2.5.29.19", critical, seq(p...))
}

// kuExt is a key usage extension with the given bits (0 = digitalSignature, 5 = keyCertSign).
func kuExt(bits ...int) []byte {
	var b [2]byte
	maxBit := 0
	for _, i := range bits {
		b[i/8] |= 0x80 >> (i % 8)
		if i > maxBit {
			maxBit = i
		}
	}
	n := maxBit/8 + 1
	content := b[:n]
	last := content[n-1]
	pad := 0
	for pad < 7 && last&(1<<pad) == 0 {
		pad++
	}
	return ext("2.5.29.15", true, tlv(0x03, append([]byte{byte(pad)}, content...)))
}

const (
	ekuServer = "1.3.6.1.5.5.7.3.1"
	ekuClient = "1.3.6.1.5.5.7.3.2"
	ekuCode   = "1.3.6.1.5.5.7.3.3"
	ekuAny    = "2.5.29.37.0"
	ekuNSSGC  = "2.16.840.1.113730.4.1"
)

func ekuExt(oids ...string) []byte {
	var p [][]byte
	for _, o := range oids {
		p = append(p, oid(o))
	}
	return ext("2.5.29.37", false, seq(p...))
}

func dnsName(s string) []byte   { return tlv(0x82, []byte(s)) }
func uriName(s string) []byte   { return tlv(0x86, []byte(s)) }
func emailName(s string) []byte { return tlv(0x81, []byte(s)) }
func ipName(s string) []byte {
	ip := net.ParseIP(s)
	if v4 := ip.To4(); v4 != nil {
		return tlv(0x87, v4)
	}
	return tlv(0x87, ip)
}
func ip16Name(s string) []byte { return tlv(0x87, net.ParseIP(s).To16()) }

func sanExt(critical bool, names ...[]byte) []byte {
	return ext("2.5.29.17", critical, seq(names...))
}

func ipNet(cidr string) []byte {
	ip, n, err := net.ParseCIDR(cidr)
	if err != nil {
		panic(err)
	}
	if v4 := ip.To4(); v4 != nil {
		return tlv(0x87, append(append([]byte{}, n.IP.To4()...), n.Mask...))
	}
	return tlv(0x87, append(append([]byte{}, n.IP.To16()...), n.Mask...))
}

func ncExt(critical bool, permitted, excluded [][]byte) []byte {
	var p [][]byte
	if permitted != nil {
		var subs [][]byte
		for _, b := range permitted {
			subs = append(subs, seq(b))
		}
		p = append(p, tlv(0xA0, subs...))
	}
	if excluded != nil {
		var subs [][]byte
		for _, b := range excluded {
			subs = append(subs, seq(b))
		}
		p = append(p, tlv(0xA1, subs...))
	}
	return ext("2.5.29.30", critical, seq(p...))
}

func policiesExt(critical bool, oids ...string) []byte {
	var p [][]byte
	for _, o := range oids {
		p = append(p, seq(oid(o)))
	}
	return ext("2.5.29.32", critical, seq(p...))
}

func policyConstraintsExt(critical bool, requireExplicit, inhibitMapping int) []byte {
	var p [][]byte
	if requireExplicit >= 0 {
		p = append(p, tlv(0x80, intContent(int64(requireExplicit))))
	}
	if inhibitMapping >= 0 {
		p = append(p, tlv(0x81, intContent(int64(inhibitMapping))))
	}
	return ext("2.5.29.36", critical, seq(p...))
}

func policyMappingsExt(critical bool, pairs ...[2]string) []byte {
	var p [][]byte
	for _, pr := range pairs {
		p = append(p, seq(oid(pr[0]), oid(pr[1])))
	}
	return ext("2.5.29.33", critical, seq(p...))
}

func inhibitAnyExt(critical bool, n int) []byte {
	return ext("2.5.29.54", critical, integer(int64(n)))
}

func skidExt(id []byte) []byte { return ext("2.5.29.14", false, octets(id)) }
func akidExt(id []byte) []byte { return ext("2.5.29.35", false, seq(tlv(0x80, id))) }

const anyPolicy = "2.5.29.32.0"
