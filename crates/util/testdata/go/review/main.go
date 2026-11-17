// Command review writes the adversarial goldens replayed by the pbs-util review tests.
//
// The inputs are built systematically rather than at random: every octet spelling in every
// IPv4 position, every embedded-IPv4 prefix, zones and brackets, the first and last address
// of each private network and their neighbours in every textual form, privacy.scrubIP masking
// for every prefix length, X-Forwarded-For lists with empty, ported, bracketed and padded
// entries, strings.TrimSpace around every Unicode space and broken UTF-8, and the text Go's
// HTTP server stores in Request.RemoteAddr. Every output line is a tab-separated list of
// strconv.Quote'd fields, the format the other pbs-util goldens use.
//
// Run from the repository root:
//
//	go run ./rust/crates/util/testdata/go/review rust/crates/util/testdata/golden
package main

import (
	"encoding/hex"
	"fmt"
	"hash/fnv"
	"math/big"
	"net"
	"net/http"
	"net/netip"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"unicode"

	"github.com/prebid/prebid-server/v4/util/httputil"
	"github.com/prebid/prebid-server/v4/util/iputil"
)

var outDir string

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: review <output dir>")
		os.Exit(2)
	}
	outDir = os.Args[1]
	genParse()
	genMask()
	genBoundaries()
	genTrim()
	genFindIP()
	genRemoteAddr()
}

type table struct{ b strings.Builder }

func (t *table) row(fields ...string) {
	for i, f := range fields {
		if i > 0 {
			t.b.WriteByte('\t')
		}
		t.b.WriteString(strconv.Quote(f))
	}
	t.b.WriteByte('\n')
}

func (t *table) write(name string) {
	check(os.WriteFile(filepath.Join(outDir, name), []byte(t.b.String()), 0o644))
}

func check(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func ipHex(ip net.IP) string {
	if ip == nil {
		return "nil"
	}
	return hex.EncodeToString(ip)
}

// octetSpellings are the texts tried in each IPv4 position: boundaries, leading zeros, signs,
// spaces, hex and non-ASCII digits.
var octetSpellings = []string{
	"0", "00", "000", "0000", "01", "001", "010", "0255", "1", "9", "10", "99", "100", "199",
	"200", "249", "250", "255", "256", "260", "299", "300", "999", "1000", "", "+1", "-1", " 1",
	"1 ", "0x1", "a", "\u0661", "\uff11", "1\x00",
}

// embedPrefixes are the IPv6 texts an IPv4 address may follow.
var embedPrefixes = []string{
	"::", "::ffff:", "::FFFF:", "::fFfF:", "::ffff:0:", "0:0:0:0:0:ffff:", "0::ffff:",
	"0000:0000:0000:0000:0000:ffff:", "64:ff9b::", "1:2:3:4:5:6:", "1:2:3:4:5::", "::1:2:3:4:",
	"1::", "::ffff:ffff:", "1:2:3:4:5:6:7:", "::0:", ":::", "::ffff::", "ffff::",
	"0:0:0:0:0:0:", "1:2:3:4:5:6:7:8:",
}

// zones are what may follow the '%' of an IPv6 zone.
var zones = []string{
	"", "eth0", "1", "%", "%25", "en0%", "\u00e9", "\xff", " ", "eth0 ", "::1", "[", "]", "/24",
	"0", "lo:1", "1.2.3.4",
}

// parseInputs builds the systematic ParseIP inputs.
func parseInputs() []string {
	var in []string
	base := [4]string{"1", "2", "3", "4"}
	for pos := 0; pos < 4; pos++ {
		for _, o := range octetSpellings {
			parts := base
			parts[pos] = o
			in = append(in, strings.Join(parts[:], "."))
		}
	}
	for _, o := range octetSpellings {
		in = append(in, strings.Repeat(o+".", 3)+o)
	}
	ipv4s := []string{"1.2.3.4", "10.0.0.1", "0.0.0.0", "255.255.255.255", "01.2.3.4", "1.2.3.04",
		"1.2.3.256", "1.2.3", "1.2.3.4.5", "1.2.3.4 ", "1.2.3.", "192.168.000.1"}
	for _, p := range embedPrefixes {
		for _, v4 := range ipv4s {
			in = append(in, p+v4)
		}
	}
	for _, z := range zones {
		for _, a := range []string{"fe80::1", "::", "::1", "::ffff:1.2.3.4", "1.2.3.4", "2001:db8::1:2:3:4:5", ""} {
			in = append(in, a+"%"+z)
		}
	}
	for _, a := range []string{"::1", "1.2.3.4", "fe80::1%eth0", "::ffff:10.0.0.1", "2600::"} {
		in = append(in, "["+a+"]", "["+a, a+"]", "["+a+"]:80", "[["+a+"]]", "<"+a+">", "\""+a+"\"")
	}
	// Groups of one to five digits in every position of an uncompressed address.
	for pos := 0; pos < 8; pos++ {
		for _, g := range []string{"0", "00", "000", "0000", "00000", "f", "ff", "fff", "ffff", "fffff", "FFFF", "1ffff", "g", ""} {
			groups := []string{"1", "2", "3", "4", "5", "6", "7", "8"}
			groups[pos] = g
			in = append(in, strings.Join(groups, ":"))
		}
	}
	// The ellipsis in every position, with seven, eight and nine groups around it.
	for total := 6; total <= 9; total++ {
		for pos := 0; pos <= total; pos++ {
			var left, right []string
			for i := 0; i < total; i++ {
				g := strconv.FormatInt(int64(i+1), 16)
				if i < pos {
					left = append(left, g)
				} else {
					right = append(right, g)
				}
			}
			in = append(in, strings.Join(left, ":")+"::"+strings.Join(right, ":"))
		}
	}
	return in
}

// genParse writes review_parse.txt: input, net.ParseIP bytes and String, iputil.ParseIP
// version, and netip.ParseAddr's String or error.
func genParse() {
	var t table
	for _, s := range parseInputs() {
		ip := net.ParseIP(s)
		_, ver := iputil.ParseIP(s)
		addr, err := netip.ParseAddr(s)
		addrText := "ok " + addr.String()
		if err != nil {
			addrText = "error: " + err.Error()
		}
		t.row(s, ipHex(ip), ip.String(), strconv.Itoa(int(ver)), addrText)
	}
	t.write("review_parse.txt")
}

// scrubIP is privacy.scrubIP.
func scrubIP(ip string, ones, bits int) string {
	if ip == "" {
		return ""
	}
	ipMask := net.CIDRMask(ones, bits)
	ipMasked := net.ParseIP(ip).Mask(ipMask)
	return ipMasked.String()
}

// genMask writes review_mask.txt: ip text, bits, then privacy.scrubIP for every ones from -1
// to 129, then the same masking of the 4-byte form, each list joined by ','.
func genMask() {
	var t table
	ips := []string{
		"1.2.3.4", "255.255.255.255", "0.0.0.0", "203.0.113.255", "::ffff:1.2.3.4", "::FFFF:255.255.255.255",
		"::1.2.3.4", "::ffff:0:1.2.3.4", "64:ff9b::1.2.3.4", "::", "::1", "::ffff:0:0", "ffff::",
		"ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", "2001:db8:85a3:8d3:1319:8a2e:370:7348",
		"0:0:0:0:0:ffff:ffff:ffff", "fe80::1%eth0", "01.2.3.4", "1.2.3", "", "x", "0:0:0:0:0:fffe:1:2",
		"::fffe:1.2.3.4", "1:0:0:0:0:ffff:1.2.3.4",
	}
	for _, s := range ips {
		for _, bits := range []int{32, 128} {
			var scrubbed, four []string
			for ones := -1; ones <= 129; ones++ {
				scrubbed = append(scrubbed, scrubIP(s, ones, bits))
				four = append(four, net.ParseIP(s).To4().Mask(net.CIDRMask(ones, bits)).String())
			}
			t.row(s, strconv.Itoa(bits), strings.Join(scrubbed, ","), strings.Join(four, ","))
		}
	}
	t.write("review_mask.txt")
}

// privateNetworks are the config defaults and other well-known ranges, IPv4 first.
var v4Networks = []string{"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "127.0.0.0/8", "100.64.0.0/10", "0.0.0.0/8", "224.0.0.0/4", "255.255.255.255/32"}
var v6Networks = []string{"::1/128", "fc00::/7", "fe80::/10", "ff00::/8", "2001:db8::/32", "::ffff:0:0/96", "64:ff9b::/96", "2002::/16", "::/128", "::ffff:10.0.0.0/104"}

func parseNets(cidrs []string) []net.IPNet {
	var out []net.IPNet
	for _, s := range cidrs {
		_, n, err := net.ParseCIDR(s)
		check(err)
		out = append(out, *n)
	}
	return out
}

// addBig adds d to a 16-byte address, wrapping.
func addBig(ip net.IP, d int64) net.IP {
	v := new(big.Int).SetBytes(ip.To16())
	v.Add(v, big.NewInt(d))
	mod := new(big.Int).Lsh(big.NewInt(1), 128)
	v.Mod(v, mod)
	b := v.FillBytes(make([]byte, 16))
	return net.IP(b)
}

// lastOf returns the last address of n, as 16 bytes.
func lastOf(n net.IPNet) net.IP {
	ip := n.IP.To16()
	mask := n.Mask
	if len(mask) == net.IPv4len {
		mask = append(net.CIDRMask(96, 128)[:12:12], mask...)
	}
	out := make(net.IP, 16)
	for i := range out {
		out[i] = ip[i] | ^mask[i]
	}
	return out
}

// spellings returns textual forms of a 16-byte address.
func spellings(ip net.IP) []string {
	var out []string
	if v4 := ip.To4(); v4 != nil {
		d := v4.String()
		out = append(out, d, "::ffff:"+d, "::FFFF:"+d, "0:0:0:0:0:ffff:"+d,
			fmt.Sprintf("::ffff:%x:%x", uint16(v4[0])<<8|uint16(v4[1]), uint16(v4[2])<<8|uint16(v4[3])),
			"::"+d, "64:ff9b::"+d)
		return out
	}
	addr := netip.AddrFrom16([16]byte(ip))
	full := addr.StringExpanded()
	out = append(out, addr.String(), strings.ToUpper(addr.String()), full, strings.ToUpper(full))
	return out
}

// genBoundaries writes review_boundaries.txt: address text, iputil.ParseIP version and
// String, the default validator's IsValid, the mapped-network validator's IsValid, then one
// Contains bit per network of v4Networks and v6Networks.
func genBoundaries() {
	all := append(parseNets(v4Networks), parseNets(v6Networks)...)
	def := iputil.PublicNetworkIPValidator{
		IPv4PrivateNetworks: parseNets([]string{"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "127.0.0.0/8"}),
		IPv6PrivateNetworks: parseNets([]string{"::1/128", "fc00::/7", "fe80::/10", "ff00::/8", "2001:db8::/32"}),
	}
	mapped := iputil.PublicNetworkIPValidator{
		IPv4PrivateNetworks: parseNets([]string{"::ffff:10.0.0.0/104", "::ffff:0:0/96"}),
		IPv6PrivateNetworks: parseNets([]string{"::ffff:10.0.0.0/104", "::ffff:0:0/96", "::ffff:192.168.0.0/112"}),
	}
	seen := map[string]bool{}
	var t table
	for _, n := range all {
		first := n.IP.To16()
		last := lastOf(n)
		for _, ip := range []net.IP{first, last, addBig(first, -1), addBig(last, 1), addBig(first, 1), addBig(last, -1)} {
			for _, s := range spellings(ip) {
				if seen[s] {
					continue
				}
				seen[s] = true
				parsed, ver := iputil.ParseIP(s)
				bitsOut := make([]byte, 0, len(all))
				for _, m := range all {
					if parsed != nil && m.Contains(parsed) {
						bitsOut = append(bitsOut, '1')
					} else {
						bitsOut = append(bitsOut, '0')
					}
				}
				t.row(s, strconv.Itoa(int(ver)), parsed.String(),
					strconv.FormatBool(parsed != nil && def.IsValid(parsed, ver)),
					strconv.FormatBool(parsed != nil && mapped.IsValid(parsed, ver)),
					string(bitsOut))
			}
		}
	}
	t.write("review_boundaries.txt")
}

// spaceRunes are every rune unicode.IsSpace accepts.
func spaceRunes() []rune {
	var out []rune
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if unicode.IsSpace(r) {
			out = append(out, r)
		}
	}
	return out
}

// trimPads are the non-space runes and byte sequences that sit next to spaces in the tests:
// look-alikes Go does not trim and broken UTF-8.
var trimPads = []string{
	"\u200b", "\ufeff", "\u180e", "\u0000", "\u001c", "\u001f", "\u00ad", "\ufffd", "\xff", "\xc2",
	"\xe3\x80", "\xe2\x80", "\xc0\xa0", "\xed\xa0\x80", "\xf4\x90\x80\x80", "\x80", "\x85", "\xa0",
	"\xf0\x9f\x98", "a", "",
}

// genTrim writes review_trim.txt: rows of input and strings.TrimSpace, then a "spaces" row
// listing every rune unicode.IsSpace accepts, a "frags" row of fragments, and a "sweep" row:
// the FNV-1a 64 checksum of TrimSpace over 100000 concatenations of 0 to 6 fragments, each
// drawn with splitmix64 (seed 0x5EED7001) as next() % len.
func genTrim() {
	var t table
	var pads []string
	for _, r := range spaceRunes() {
		pads = append(pads, string(r))
	}
	pads = append(pads, trimPads...)
	for _, core := range []string{"8.8.8.8", "", "\xff", "a b", "\u00a0x\u00a0"} {
		for _, p := range pads {
			for _, q := range []string{"", " ", "\u3000", "\xff", "\u0085"} {
				for _, s := range []string{p + core, core + p, p + core + p, q + p + core + p + q, p + q + core + q + p} {
					t.row(s, strings.TrimSpace(s))
				}
			}
		}
	}
	// A checksum sweep over random concatenations of the fragments.
	frags := append(append([]string{}, pads...), "1", ".", ":", "\xe3", "\x80", "\x80\x80", "\xc2\x85", "\xe2\x80\xa8")
	h := fnv.New64a()
	state := uint64(0x5EED_7001)
	next := func() uint64 {
		state += 0x9E3779B97F4A7C15
		z := state
		z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
		z = (z ^ (z >> 27)) * 0x94D049BB133111EB
		return z ^ (z >> 31)
	}
	for range 100000 {
		n := int(next() % 7)
		var b strings.Builder
		for range n {
			b.WriteString(frags[next()%uint64(len(frags))])
		}
		h.Write([]byte(strings.TrimSpace(b.String())))
		h.Write([]byte{'\n'})
	}
	var spaces []string
	for _, r := range spaceRunes() {
		spaces = append(spaces, strconv.FormatInt(int64(r), 16))
	}
	t.row(append([]string{"spaces"}, spaces...)...)
	t.row(append([]string{"frags"}, frags...)...)
	t.row("sweep", fmt.Sprintf("%016x", h.Sum64()))
	t.write("review_trim.txt")
}

// xffTokens are the entries the X-Forwarded-For lists are built from.
var xffTokens = []string{
	"8.8.8.8", "10.0.0.1", "192.168.1.1", "172.16.0.1", "172.32.0.1", "127.0.0.1", "169.254.0.1",
	"2600::1", "2001:db8::1", "fe80::1", "fc00::1", "::1", "::ffff:8.8.8.8", "::ffff:10.0.0.1",
	"", " ", "unknown", "8.8.8.8:80", "[2600::1]:443", "[2600::1]", "2600::1%eth0", "01.2.3.4",
	"1.2.3.04", "\t8.8.4.4", "8.8.4.4\u00a0", "\u00858.8.4.4", "8.8.4.4\u3000", "\xc2", "1.1.1.1\xc2",
	"1.1.1.1\xe3\x80", "\xe3\x80\x801.1.1.1", "\u200b1.1.1.1", "\ufeff1.1.1.1", "\u180e9.9.9.9",
	"\u20289.9.9.9", "9.9.9.9\u2029", "\u205f2.2.2.2", "\u16802.2.2.2", "2.2.2.2\u2000", "\u200a3.3.3.3",
	"3.3.3.3\u202f", "3.3.3.3\r\n", "\v4.4.4.4\f", "4.4.4.4;5.5.5.5", "5.5.5.5 6.6.6.6", "2600:0:0:0:0:0:0:1",
	"2600::1/64", "::FFFF:8.8.8.8", "1.1.1.1.", "0.0.0.0", "::",
}

var xffSeparators = []string{",", ", ", " ,", " , ", ",,", ",\t"}

// remoteAddrs includes the text Go's server stores for IPv4 clients of a dual-stack socket
// ("1.2.3.4:80") next to the bracketed IPv4-mapped form another server might produce.
var remoteAddrs = []string{
	"", "8.8.8.8:80", "10.0.0.1:80", "[::ffff:8.8.8.8]:80", "[::ffff:10.0.0.1]:80", "[2600::5]:80",
	"[::1]:80", "8.8.8.8:0", "8.8.8.8:", "[8.8.8.8]:80", "[2600::1%25eth0]:80", "[fe80::1%eth0]:80",
	"08.8.8.8:80", "8.8.8.8", "[2600::5]", "2600::5:80", "@", "[::ffff:8.8.8.8%1]:80",
}

type splitmix64 struct{ state uint64 }

func (s *splitmix64) next() uint64 {
	s.state += 0x9E3779B97F4A7C15
	z := s.state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

func (s *splitmix64) intn(n int) int { return int(s.next() % uint64(n)) }

func pick(s *splitmix64, xs []string) string { return xs[s.intn(len(xs))] }

// genFindIP writes review_findip.txt: True-Client-IP, X-Forwarded-For, X-Real-IP and
// RemoteAddr, then for each validator (default, mapped, v4only, v6only) FindIP's bytes,
// String and version. Empty header values are added to the request as often as not, which
// Header.Get cannot tell from absent ones.
func genFindIP() {
	validators := []struct {
		name string
		v    iputil.IPValidator
	}{
		{"default", iputil.PublicNetworkIPValidator{
			IPv4PrivateNetworks: parseNets([]string{"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "127.0.0.0/8"}),
			IPv6PrivateNetworks: parseNets([]string{"::1/128", "fc00::/7", "fe80::/10", "ff00::/8", "2001:db8::/32"}),
		}},
		{"mapped", iputil.PublicNetworkIPValidator{
			IPv4PrivateNetworks: parseNets([]string{"::ffff:10.0.0.0/104"}),
			IPv6PrivateNetworks: parseNets([]string{"::ffff:0:0/96", "fc00::/7"}),
		}},
		{"v4only", iputil.VersionIPValidator{Version: iputil.IPv4}},
		{"v6only", iputil.VersionIPValidator{Version: iputil.IPv6}},
	}
	type tc struct{ tci, xff, xri, remote string }
	var cases []tc
	// Every token alone in each source.
	for _, tok := range xffTokens {
		cases = append(cases, tc{tci: tok}, tc{xff: tok}, tc{xri: tok})
	}
	for _, ra := range remoteAddrs {
		cases = append(cases, tc{remote: ra})
	}
	r := &splitmix64{state: 0x5EED_7002}
	for range 1500 {
		n := 1 + r.intn(10)
		parts := make([]string, n)
		for i := range parts {
			parts[i] = pick(r, xffTokens)
		}
		var b strings.Builder
		for i, p := range parts {
			if i > 0 {
				b.WriteString(pick(r, xffSeparators))
			}
			b.WriteString(p)
		}
		c := tc{xff: b.String(), remote: pick(r, remoteAddrs)}
		if r.intn(3) == 0 {
			c.tci = pick(r, xffTokens)
		}
		if r.intn(3) == 0 {
			c.xri = pick(r, xffTokens)
		}
		cases = append(cases, c)
	}
	var t table
	for _, c := range cases {
		req, err := http.NewRequest("GET", "http://example.com", nil)
		check(err)
		if c.tci != "" || r.intn(2) == 0 {
			req.Header.Add("True-Client-IP", c.tci)
		}
		if c.xff != "" || r.intn(2) == 0 {
			req.Header.Add("X-Forwarded-For", c.xff)
		}
		if c.xri != "" || r.intn(2) == 0 {
			req.Header.Add("X-Real-IP", c.xri)
		}
		req.RemoteAddr = c.remote
		fields := []string{c.tci, c.xff, c.xri, c.remote}
		for _, v := range validators {
			ip, ver := httputil.FindIP(req, v.v)
			fields = append(fields, ipHex(ip), ip.String(), strconv.Itoa(int(ver)))
		}
		t.row(fields...)
	}
	t.write("review_findip.txt")
}

// genRemoteAddr writes review_remoteaddr.txt: the address bytes, port, zone index, then
// (&net.TCPAddr{...}).String(), the text Go's HTTP server stores in Request.RemoteAddr, and the
// address FindIP then derives from it with an accept-all validator.
func genRemoteAddr() {
	var t table
	addrs := []string{
		"1.2.3.4", "0.0.0.0", "255.255.255.255", "::ffff:1.2.3.4", "::ffff:10.0.0.1", "::1", "::",
		"2600::5", "fe80::1", "::1.2.3.4", "::ffff:0:1.2.3.4", "2001:db8:0:0:1:0:0:1", "1:0:0:1:0:0:0:1",
	}
	accept := iputil.VersionIPValidator{Version: iputil.IPv4}
	acceptV6 := iputil.VersionIPValidator{Version: iputil.IPv6}
	for _, s := range addrs {
		a := netip.MustParseAddr(s)
		for _, port := range []int{0, 80, 65535} {
			for _, zone := range []int{0, 3} {
				if a.Is4() && zone != 0 {
					continue
				}
				var ip net.IP
				if a.Is4() {
					b := a.As4()
					ip = net.IP(b[:])
				} else {
					b := a.As16()
					ip = net.IP(b[:])
				}
				z := ""
				if zone != 0 {
					z = strconv.Itoa(zone)
				}
				text := (&net.TCPAddr{IP: ip, Port: port, Zone: z}).String()
				req, err := http.NewRequest("GET", "http://example.com", nil)
				check(err)
				req.RemoteAddr = text
				found, ver := httputil.FindIP(req, accept)
				if found == nil {
					found, ver = httputil.FindIP(req, acceptV6)
				}
				t.row(hex.EncodeToString(ip), strconv.Itoa(port), strconv.Itoa(zone), text,
					found.String(), strconv.Itoa(int(ver)))
			}
		}
	}
	t.write("review_remoteaddr.txt")
}
