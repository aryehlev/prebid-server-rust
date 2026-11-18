package main

import (
	"encoding/hex"
	"fmt"
	"net"
	"net/netip"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/util/iputil"
)

// curatedIPs are hand-picked inputs covering every branch of netip.ParseAddr and net.IP.String.
var curatedIPs = []string{
	"", "1.1.1.1", "0.0.0.0", "255.255.255.255", "256.1.1.1", "1.256.1.1", "01.1.1.1", "1.01.1.1",
	"1.1.1.00", "1.1.1.0", "1.1.1", "1.1.1.1.1", "1..1.1", ".1.1.1", "1.1.1.", "1.1.1.1 ",
	" 1.1.1.1", "1.1.1.1\n", "-1.-1.-1.-1", "1.1.1.1%eth0", "1.1.1.1:80", "0x1.1.1.1",
	"１.1.1.1", "1.1.1.1\x00", "1.2.3.4\xff", "10.0.0.1", "172.16.5.4", "192.168.0.1",
	"127.0.0.1", "100.64.0.1", "169.254.1.1", "999.1.1.1", "1.1.1.1111", "4294967296",
	"::", "::1", "1::", "1::1", ":::", ":1::", "1:", ":1", "::1:", "1:::2", "12345::",
	"::ffff:1.1.1.1", "::FFFF:1.2.3.4", "::ffff:0:1.2.3.4", "::1.2.3.4", "::ffff:256.1.1.1",
	"::ffff:1.2.3", "::ffff:1.2.3.4.5", "::1.2.3.04", "1.2.3.4::", "::ffff:1.2.3.4:5",
	"0:0:0:0:0:ffff:102:304", "0:0:0:0:0:ffff:1.2.3.4", "0:0:0:0:0:0:1.2.3.4",
	"1:2:3:4:5:6:1.2.3.4", "1:2:3:4:5:6:7:1.2.3.4", "1:2:3:4:5:1.2.3.4", "1:2:3:4:5::1.2.3.4",
	"1:2:3:4:5:6:7:8", "1:2:3:4:5:6:7:8:9", "1:2:3:4:5:6:7", "1:2:3:4:5:6::7:8",
	"1:2:3:4:5:6:7::", "::1:2:3:4:5:6:7", "1::2::3", "1:2:3:4:5:6:7:8::", "::1:2:3:4:5:6:7:8",
	"fe80::1%eth0", "fe80::1%", "%eth0", "%", "fe80::1%%", "fe80::1%eth0%1", "::ffff:1.2.3.4%z",
	"2001:db8::68", "2001:0db8:0000:0000:0000:0000:0000:0068", "2001:db8:0:0:1:0:0:1",
	"2001:db8::1:0:0:1", "0101::", "zzzz::", "[::1]", "::ffff:10.0.0.1", "::ffff:192.168.1.1",
	"::ffff:127.0.0.1", "fc00::1", "fd12:3456::1", "fe80::", "ff02::1", "::ffff:ffff:ffff",
	"0:0:0:0:0:0:0:0", "1:0:0:2:0:0:0:3", "1:0:0:0:2:0:0:3", "0:0:1:0:0:1:0:0",
	"1:0:1:0:1:0:1:0", "a:b:c:d:e:f:0:0", "0:0:a:b:c:d:e:f", "ABCD:EF01:2345:6789:ABCD:EF01:2345:6789",
	"abcd:ef01:2345:6789:abcd:ef01:2345:6789", "00001::", "0001::", "1:02:003:0004::",
	"10000::", "fffff::", "ffff::", "g::", "::g", "1:2:3:4:5:6:7:8%eth0", "::%eth0",
	"::ffff:0.0.0.0", "::ffff:0:0", "::0.0.0.0", "64:ff9b::1.2.3.4", "2002:102:304::",
	" ::1", "::1 ", "::1\t", "\t::1", "::1\n", ":: 1", "1.2.3.4/24", "::/0",
	"2001:db8::1:2:3:4:5", "2001:db8:1:2:3:4:5:6", "1:2:3:4:5:6:0:0", "0:1:2:3:4:5:6:7",
	"1::2:3:4:5:6:7", "1:2::3:4:5:6:7", "1:2:3:4:5:6:7:", ":1:2:3:4:5:6:7",
}

// ipLikeAlphabet is the byte alphabet of the random IP-shaped strings.
const ipLikeAlphabet = "0123456789abcdefABCDEF.:%  \t01:1.ffg[]/-+xz\xff\xa0"

// genIPInputs returns the curated inputs, then random valid addresses in their textual
// forms, then mutations of those, then random strings.
func genIPInputs() []string {
	in := append([]string(nil), curatedIPs...)
	r := &splitmix64{state: 0x5EED_0001}
	for range 1500 {
		in = append(in, randomIPText(r))
	}
	for range 1500 {
		in = append(in, mutate(r, randomIPText(r)))
	}
	for range 1000 {
		in = append(in, sweepString(r, ipLikeAlphabet, 24))
	}
	return in
}

// randomIPText formats a random address in one of several textual forms.
func randomIPText(r *splitmix64) string {
	switch r.intn(8) {
	case 0, 1:
		return fmt.Sprintf("%d.%d.%d.%d", octet(r), octet(r), octet(r), octet(r))
	case 2:
		return fmt.Sprintf("::ffff:%d.%d.%d.%d", octet(r), octet(r), octet(r), octet(r))
	case 3:
		// Uncompressed with optional leading zeros and mixed case.
		groups := make([]string, 8)
		for i := range groups {
			groups[i] = group(r)
		}
		return strings.Join(groups, ":")
	case 4:
		var a [16]byte
		for i := range a {
			if r.intn(3) != 0 {
				a[i] = byte(r.intn(256))
			}
		}
		return netip.AddrFrom16(a).String()
	case 5:
		// Zero runs of different lengths, compressed by Go.
		var a [16]byte
		start := r.intn(8)
		end := start + r.intn(9-start)
		for i := 0; i < 8; i++ {
			if i >= start && i < end {
				continue
			}
			v := r.intn(0x10000)
			a[2*i] = byte(v >> 8)
			a[2*i+1] = byte(v)
		}
		return netip.AddrFrom16(a).String()
	case 6:
		groups := make([]string, 6)
		for i := range groups {
			groups[i] = group(r)
		}
		return strings.Join(groups, ":") + fmt.Sprintf(":%d.%d.%d.%d", octet(r), octet(r), octet(r), octet(r))
	default:
		return fmt.Sprintf("fe80::%x%%eth%d", r.intn(0x10000), r.intn(3))
	}
}

func octet(r *splitmix64) int {
	switch r.intn(6) {
	case 0:
		return 0
	case 1:
		return 255
	default:
		return r.intn(256)
	}
}

func group(r *splitmix64) string {
	v := r.intn(0x10000)
	if r.intn(4) == 0 {
		v = 0
	}
	s := strconv.FormatInt(int64(v), 16)
	switch r.intn(4) {
	case 0:
		s = strings.ToUpper(s)
	case 1:
		for len(s) < 4 {
			s = "0" + s
		}
	}
	return s
}

// mutate inserts, deletes or replaces one byte, or adds a prefix or suffix.
func mutate(r *splitmix64, s string) string {
	if s == "" {
		return pickByte(r)
	}
	i := r.intn(len(s))
	switch r.intn(5) {
	case 0:
		return s[:i] + pickByte(r) + s[i:]
	case 1:
		return s[:i] + s[i+1:]
	case 2:
		return s[:i] + pickByte(r) + s[i+1:]
	case 3:
		return pickByte(r) + s
	default:
		return s + pickByte(r)
	}
}

func pickByte(r *splitmix64) string {
	return string(ipLikeAlphabet[r.intn(len(ipLikeAlphabet))])
}

func ipHex(ip net.IP) string {
	if ip == nil {
		return "nil"
	}
	return hex.EncodeToString(ip)
}

func maskHex(m net.IPMask) string {
	if m == nil {
		return "nil"
	}
	return hex.EncodeToString(m)
}

// genNet writes the net package goldens: ParseIP, netip.ParseAddr, IP.String, CIDRMask,
// IP.Mask, IPMask.Size, ParseCIDR, IPNet.Contains, IPNet.String and SplitHostPort.
func genNet() {
	inputs := genIPInputs()

	// net_parse.txt: input, ParseIP bytes, ParseIP(...).String(), To4 != nil,
	// netip.ParseAddr result (String or error), iputil.ParseIP bytes and version.
	var parse table
	for _, s := range inputs {
		ip := net.ParseIP(s)
		addr, err := netip.ParseAddr(s)
		addrText := errText(err)
		if err == nil {
			addrText = addr.String()
		}
		uip, ver := iputil.ParseIP(s)
		parse.row(s, ipHex(ip), ip.String(), strconv.FormatBool(ip.To4() != nil), addrText,
			ipHex(uip), strconv.Itoa(int(ver)))
	}
	parse.write("net_parse.txt")

	// Sweep: ParseIP over 200000 random strings.
	sw := newSweep()
	r := &splitmix64{state: 0x5EED_0002}
	for range 200000 {
		s := sweepString(r, ipLikeAlphabet, 20)
		ip := net.ParseIP(s)
		sw.add(ipHex(ip) + " " + ip.String())
	}
	var sweeps table
	sweeps.row("parse_ip_200000", sw.sum())

	// net_mask.txt: ip text, ones, bits, CIDRMask bytes, Size, String, ParseIP(ip).Mask,
	// To4().Mask, and the masked String.
	var mask table
	maskIPs := []string{
		"1.2.3.4", "255.255.255.255", "0.0.0.0", "192.168.123.231", "::ffff:10.20.30.40",
		"2001:db8:85a3:8d3:1319:8a2e:370:7348", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
		"::1", "::", "fe80::1", "::1.2.3.4", "not-an-ip", "",
	}
	ones := []int{-1, 0, 1, 7, 8, 9, 15, 16, 17, 23, 24, 25, 31, 32, 33, 48, 55, 56, 57, 64, 95, 96, 97, 104, 120, 127, 128, 129}
	bits := []int{32, 128, 0, 64, 16}
	for _, s := range maskIPs {
		for _, b := range bits {
			for _, o := range ones {
				m := net.CIDRMask(o, b)
				mo, mb := m.Size()
				ip := net.ParseIP(s)
				masked := ip.Mask(m)
				masked4 := ip.To4().Mask(m)
				mask.row(s, strconv.Itoa(o), strconv.Itoa(b), maskHex(m),
					strconv.Itoa(mo), strconv.Itoa(mb), m.String(),
					ipHex(masked), masked.String(), ipHex(masked4), masked4.String())
			}
		}
	}
	mask.write("net_mask.txt")

	// net_cidr.txt: input, ParseCIDR error or IP bytes, IPNet.IP bytes, IPNet.Mask bytes,
	// IPNet.String().
	var cidr table
	cidrInputs := []string{
		"10.0.0.0/8", "10.0.0.1/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16",
		"127.0.0.0/8", "::1/128", "fc00::/7", "fe80::/10", "ff00::/8", "2001:db8::/32",
		"::ffff:10.0.0.0/104", "::ffff:10.0.0.0/8", "::ffff:0:0/96", "0.0.0.0/0", "::/0",
		"1.2.3.4/32", "1.2.3.4/33", "1.2.3.4/-1", "1.2.3.4/", "1.2.3.4", "/8", "1.2.3.4/08",
		"1.2.3.4/8 ", " 1.2.3.4/8", "1.2.3.4/+8", "fe80::1%eth0/64", "1.2.3.4/16777215",
		"1.2.3.4/16777216", "1.2.3.4/99999999", "1.2.3.4/8/8", "2001:db8::1/129",
		"2001:db8::1/128", "2001:db8::1/64", "2001:db8::1/0", "01.2.3.4/8", "1.2.3.4/a",
		"1.2.3.4/8a", "::1/", "/", "", "1.2.3.4//8", "1.2.3.4/0x8", "1.2.3.4/000000008",
	}
	r = &splitmix64{state: 0x5EED_0003}
	for range 800 {
		ip := randomIPText(r)
		if r.intn(4) == 0 {
			ip = mutate(r, ip)
		}
		cidrInputs = append(cidrInputs, ip+"/"+strconv.Itoa(r.intn(140)-4))
	}
	for _, s := range cidrInputs {
		ip, n, err := net.ParseCIDR(s)
		if err != nil {
			cidr.row(s, errText(err), "", "", "")
			continue
		}
		cidr.row(s, ipHex(ip), ipHex(n.IP), maskHex(n.Mask), n.String())
	}
	cidr.write("net_cidr.txt")

	// net_contains.txt: network IP bytes, network mask bytes, IPNet.String(), then for each
	// candidate, candidate bytes and Contains.
	var contains table
	var nets []net.IPNet
	for _, s := range []string{
		"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "127.0.0.0/8",
		"::1/128", "fc00::/7", "fe80::/10", "ff00::/8", "2001:db8::/32", "::ffff:10.0.0.0/104",
		"::ffff:0:0/96", "0.0.0.0/0", "::/0", "1.2.3.4/32",
	} {
		_, n, err := net.ParseCIDR(s)
		check(err)
		nets = append(nets, *n)
	}
	// Struct literals as Go tests write them: a 16-byte IPv4 with a 4-byte mask, and others.
	nets = append(nets,
		net.IPNet{IP: net.ParseIP("1.0.0.0"), Mask: net.CIDRMask(8, 32)},
		net.IPNet{IP: net.ParseIP("3300::"), Mask: net.CIDRMask(8, 128)},
		net.IPNet{IP: net.ParseIP("1.0.0.0").To4(), Mask: net.CIDRMask(104, 128)},
		net.IPNet{IP: net.ParseIP("1.0.0.0").To4(), Mask: net.CIDRMask(8, 128)},
		net.IPNet{IP: net.ParseIP("3300::"), Mask: net.CIDRMask(8, 32)},
		net.IPNet{IP: net.ParseIP("10.1.2.3"), Mask: net.IPv4Mask(255, 0, 255, 0)},
		net.IPNet{IP: net.ParseIP("2001:db8::"), Mask: net.IPMask{0xff, 0xff, 0, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0}},
	)
	var candidates []net.IP
	for _, s := range []string{
		"10.1.2.3", "11.1.2.3", "172.31.255.255", "172.32.0.0", "192.168.1.1", "169.254.0.1",
		"127.0.0.1", "1.2.3.4", "1.2.3.5", "1.0.0.1", "8.8.8.8", "::1", "::2", "fc00::1",
		"fdff::1", "fe00::1", "fe80::1", "febf::1", "ff02::1", "2001:db8::1", "2001:db9::1",
		"::ffff:10.0.0.1", "::ffff:11.0.0.1", "3300::1", "3400::1", "10.0.2.0", "10.1.2.0",
		"2001:db8::ff00:0:0:0", "::10.0.0.1",
	} {
		ip := net.ParseIP(s)
		candidates = append(candidates, ip, ip.To4())
	}
	for _, n := range nets {
		fields := []string{ipHex(n.IP), maskHex(n.Mask), n.String()}
		for _, c := range candidates {
			if c == nil {
				continue
			}
			fields = append(fields, ipHex(c), strconv.FormatBool(n.Contains(c)))
		}
		contains.row(fields...)
	}
	contains.write("net_contains.txt")

	// net_splithostport.txt: input, host, port, error.
	var hp table
	hpInputs := []string{
		"1.2.3.4:80", "[::1]:80", "::1:80", "[::1]", "[::1]:", "1.2.3.4", "1.2.3.4:", ":80", ":",
		"", "[fe80::1%eth0]:443", "[1.2.3.4]:80", "[]:80", "[::1]]:80", "[[::1]:80", "a[b:80",
		"a]b:80", "[::1]x:80", "[::1]:80:90", "host:port:extra", "example.com:8080",
		"[::1:80", "::1]:80", "malformed", "5.5.5.5:5", "[5555::]:5", "[5555::]:5]",
		" 1.2.3.4:80", "1.2.3.4:80 ", "[::ffff:1.2.3.4]:80", "@", "[a:b]:c:d",
	}
	r = &splitmix64{state: 0x5EED_0004}
	for range 1500 {
		hpInputs = append(hpInputs, sweepString(r, "[]:%:1a.[]::", 10))
	}
	for _, s := range hpInputs {
		host, port, err := net.SplitHostPort(s)
		hp.row(s, host, port, errText(err))
	}
	hp.write("net_splithostport.txt")

	sweeps.write("net_sweeps.txt")
}

// genIPUtil writes the PublicNetworkIPValidator golden (iputil.ParseIP is in net_parse.txt).
func genIPUtil() {
	// iputil_validator.txt: validator name, ip text, then IsValid for IPvUnknown, IPv4 and
	// IPv6.
	var val table
	validators := map[string]iputil.PublicNetworkIPValidator{
		"default": defaultValidator(),
		"empty":   {},
		"mapped": {
			IPv4PrivateNetworks: parseNets("::ffff:10.0.0.0/104"),
			IPv6PrivateNetworks: parseNets("::ffff:10.0.0.0/104", "::ffff:0:0/96"),
		},
	}
	for _, name := range []string{"default", "empty", "mapped"} {
		v := validators[name]
		for _, s := range genIPInputs() {
			ip := net.ParseIP(s)
			if ip == nil {
				continue
			}
			val.row(name, s,
				strconv.FormatBool(v.IsValid(ip, iputil.IPvUnknown)),
				strconv.FormatBool(v.IsValid(ip, iputil.IPv4)),
				strconv.FormatBool(v.IsValid(ip, iputil.IPv6)))
		}
	}
	val.write("iputil_validator.txt")
}

func parseNets(cidrs ...string) []net.IPNet {
	var out []net.IPNet
	for _, s := range cidrs {
		_, n, err := net.ParseCIDR(s)
		check(err)
		out = append(out, *n)
	}
	return out
}

// defaultValidator is the validator built from the config defaults of
// request_validation.ipv4_private_networks and ipv6_private_networks.
func defaultValidator() iputil.PublicNetworkIPValidator {
	return iputil.PublicNetworkIPValidator{
		IPv4PrivateNetworks: parseNets("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "127.0.0.0/8"),
		IPv6PrivateNetworks: parseNets("::1/128", "fc00::/7", "fe80::/10", "ff00::/8", "2001:db8::/32"),
	}
}
