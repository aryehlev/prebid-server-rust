package main

import (
	"encoding/hex"
	"net"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/util/iputil"
)

// ipSweepCount is the number of seeded addresses of the iputil sweep.
const ipSweepCount = 50000

// ipMutationAlphabet are the bytes a mutation inserts or writes.
const ipMutationAlphabet = "0123456789abcdefABCDEF.:%[]/ \tx"

// ipZones are the zones of the zoned addresses.
var ipZones = []string{"eth0", "1", "en0", "", "25"}

// ipMaskBits are the bit lengths of the seeded masks.
var ipMaskBits = []int{32, 128, 64, 0}

func sweepOctet(r *splitmix64) string {
	switch r.intn(6) {
	case 0:
		return "0"
	case 1:
		return "255"
	default:
		return strconv.Itoa(r.intn(256))
	}
}

func sweepGroup(r *splitmix64) string {
	v := r.intn(0x10000)
	if r.intn(4) == 0 {
		v = 0
	}
	s := strconv.FormatInt(int64(v), 16)
	switch r.intn(4) {
	case 0:
		s = strings.ToUpper(s)
	case 1:
		s = strings.Repeat("0", 4-len(s)) + s
	}
	return s
}

func sweepDotted(r *splitmix64) string {
	return sweepOctet(r) + "." + sweepOctet(r) + "." + sweepOctet(r) + "." + sweepOctet(r)
}

func sweepGroups(r *splitmix64, n int) string {
	groups := make([]string, n)
	for i := range groups {
		groups[i] = sweepGroup(r)
	}
	return strings.Join(groups, ":")
}

// seededAddressBase draws an address text of one of eleven forms: dotted quads, one with a
// leading zero, IPv4-mapped forms in three spellings, the deprecated IPv4-compatible form, eight
// groups, groups around "::", zoned link-local addresses and six groups before a dotted quad.
func seededAddressBase(r *splitmix64) string {
	switch r.intn(11) {
	case 0, 1:
		return sweepDotted(r)
	case 2:
		o := []string{sweepOctet(r), sweepOctet(r), sweepOctet(r), sweepOctet(r)}
		i := r.intn(4)
		o[i] = "0" + o[i]
		return strings.Join(o, ".")
	case 3:
		return "::ffff:" + sweepDotted(r)
	case 4:
		return "::FFFF:" + sweepDotted(r)
	case 5:
		return "0:0:0:0:0:ffff:" + sweepDotted(r)
	case 6:
		return "::" + sweepDotted(r)
	case 7:
		return sweepGroups(r, 8)
	case 8:
		h := r.intn(8)
		t := r.intn(8 - h)
		head := sweepGroups(r, h)
		return head + "::" + sweepGroups(r, t)
	case 9:
		return "fe80::" + sweepGroup(r) + "%" + pick(r, ipZones)
	default:
		return sweepGroups(r, 6) + ":" + sweepDotted(r)
	}
}

// seededAddress is a base form, or one mutated by inserting, deleting or replacing a byte, or
// adding one before or after.
func seededAddress(r *splitmix64) string {
	s := seededAddressBase(r)
	if r.intn(4) != 0 {
		return s
	}
	b := string(ipMutationAlphabet[r.intn(len(ipMutationAlphabet))])
	i := r.intn(len(s))
	switch r.intn(5) {
	case 0:
		return s[:i] + b + s[i:]
	case 1:
		return s[:i] + s[i+1:]
	case 2:
		return s[:i] + b + s[i+1:]
	case 3:
		return b + s
	default:
		return s + b
	}
}

// scrubIP is privacy.scrubIP (privacy/scrubber.go:149-156).
func scrubIP(ip string, ones, bits int) string {
	if ip == "" {
		return ""
	}
	return net.ParseIP(ip).Mask(net.CIDRMask(ones, bits)).String()
}

// cidrVerdict is ParseCIDR's network as config.parseNetworks keeps it: "err", or the
// network's IP bytes, its prefix length and the IP's length, which parseNetworks compares with
// 4 for the IPv4 and 16 for the IPv6 private networks.
func cidrVerdict(s string) string {
	_, n, err := net.ParseCIDR(s)
	if err != nil {
		return "err"
	}
	ones, _ := n.Mask.Size()
	return hex.EncodeToString(n.IP) + " " + strconv.Itoa(ones) + " " + strconv.Itoa(len(n.IP))
}

// ipOutputs are the outputs of one address: net.ParseIP's bytes and String, iputil.ParseIP's
// version, the default validator's verdict ("-" when it does not parse), and scrubIP with the
// default masks.
func ipOutputs(s string) []string {
	ip := net.ParseIP(s)
	_, ver := iputil.ParseIP(s)
	valid := "-"
	if ip != nil {
		valid = strconv.FormatBool(defaultValidator().IsValid(ip, ver))
	}
	return []string{ipHex(ip), ip.String(), strconv.Itoa(int(ver)), valid,
		scrubIP(s, iputil.IPv4DefaultMaskingBitSize, iputil.IPv4BitSize),
		scrubIP(s, iputil.IPv6DefaultMaskingBitSize, iputil.IPv6BitSize)}
}

type ipEdgeJSON struct {
	Input   string   `json:"input"`
	Outputs []string `json:"outputs"`
}

type cidrEdgeJSON struct {
	Input   string `json:"input"`
	Verdict string `json:"verdict"`
}

type ipSweepJSON struct {
	Seed          uint64         `json:"seed"`
	Count         int            `json:"count"`
	InputsSHA256  string         `json:"inputs_sha256"`
	OutputsSHA256 string         `json:"outputs_sha256"`
	Edges         []ipEdgeJSON   `json:"edges"`
	CIDREdges     []cidrEdgeJSON `json:"cidr_edges"`
}

// ipEdgeList are the edge inputs: leading zeros, IPv4-mapped and IPv4-compatible forms, zones,
// white space, ports and brackets, and the forms around the limits of each field.
var ipEdgeList = []string{
	"", "1.2.3.4", "01.2.3.4", "1.02.3.4", "1.2.3.04", "1.2.3.00", "00.0.0.0", "0.0.0.0",
	"001.2.3.4", "1.2.3.4.", "1.2.3", "1.2.3.4.5", "256.1.1.1", "1.2.3.256", "0x1.2.3.4",
	"::1.2.3.04", "::ffff:01.2.3.4", "0001::", "00001::", "1:02:003:0004::", "0:0:0:0:0:0:0:01",
	"::ffff:1.2.3.4", "::FFFF:1.2.3.4", "::ffff:102:304", "0:0:0:0:0:ffff:1.2.3.4",
	"0000:0000:0000:0000:0000:ffff:0102:0304", "::ffff:0.0.0.0", "::ffff:0:0", "::ffff:10.0.0.1",
	"::ffff:192.168.1.1", "::ffff:127.0.0.1", "::ffff:8.8.8.8", "::1.2.3.4", "::0.0.0.1",
	"::ffff:0:1.2.3.4", "64:ff9b::1.2.3.4", "1:2:3:4:5:6:1.2.3.4", "1:2:3:4:5:6:7:1.2.3.4",
	"1:2:3:4:5::1.2.3.4", "::ffff:1.2.3.4:5", "fe80::1%eth0", "fe80::1%1", "fe80::1%", "%eth0",
	"::ffff:1.2.3.4%eth0", "1.2.3.4%eth0", "fe80::1%eth0%1", "fe80::1%25eth0", "::1%lo0",
	" 1.2.3.4", "1.2.3.4 ", "\t::1", "::1\n", "[::1]", "[::1]:80", "1.2.3.4:80", "1.2.3.4/24",
	"::", "::1", ":::", "1::", "::1:", "1:::2", "1::2::3", "1:2:3:4:5:6:7:8", "1:2:3:4:5:6:7::",
	"::2:3:4:5:6:7:8", "1:2:3:4:5:6:7:8:9", "1:2:3:4:5:6::7:8", "fc00::1", "fe80::", "ff02::1",
	"2001:db8::1", "2001:0db8:0000:0000:0000:0000:0000:0001", "ABCD:EF01:2345:6789:ABCD:EF01:2345:6789",
	"g::", "::g", "\uff11.2.3.4", "1.2.3.4\u00a0",
}

// cidrEdgeList are the edge networks: prefix spellings dtoi accepts and refuses, leading
// zeros, mapped networks of both lengths, zones and the limits of the prefix.
var cidrEdgeList = []string{
	"10.0.0.0/8", "10.0.0.1/8", "10.1.2.3/32", "0.0.0.0/0", "1.2.3.4/33", "1.2.3.4/-1",
	"1.2.3.4/", "1.2.3.4", "/8", "1.2.3.4/08", "1.2.3.4/000000008", "1.2.3.4/+8", "1.2.3.4/ 8",
	"1.2.3.4/8 ", " 1.2.3.4/8", "1.2.3.4/0x8", "1.2.3.4/8a", "1.2.3.4/16777215",
	"1.2.3.4/16777216", "1.2.3.4/99999999", "1.2.3.4/8/8", "01.2.3.4/8", "1.2.3.04/8",
	"::1/128", "::1/129", "fc00::/7", "fe80::/10", "2001:db8::/32", "::/0", "::ffff:10.0.0.0/104",
	"::ffff:10.0.0.0/8", "::ffff:0:0/96", "::ffff:0:0/95", "::ffff:1.2.3.4/128", "::1.2.3.4/96",
	"fe80::1%eth0/64", "1.2.3.4%eth0/8", "fe80::/0010", "fe80::/128", "fe80::/1000",
	"2001:db8::1/64", "[::1]/128",
}

// genIPSweep writes iputil_sweep.json: ipOutputs, scrubIP with a seeded mask and the
// ParseCIDR verdict of the address with a seeded prefix, over 50,000 seeded addresses (an input
// is the address; an output the ipOutputs, the mask's ones and bits, its scrubIP and the
// CIDR's verdict, joined by tabs), then the edge list and the CIDR edge list.
func genIPSweep() {
	out := ipSweepJSON{Seed: 0x5EED_0401, Count: ipSweepCount}
	r := &splitmix64{state: out.Seed}
	m := newManifest()
	for i := 0; i < out.Count; i++ {
		s := seededAddress(r)
		ones := r.intn(134) - 2
		bits := pick(r, ipMaskBits)
		prefix := strconv.Itoa(r.intn(134) - 2)
		if r.intn(5) == 0 {
			prefix = "0" + prefix
		}
		fields := append(ipOutputs(s), strconv.Itoa(ones), strconv.Itoa(bits),
			scrubIP(s, ones, bits), cidrVerdict(s+"/"+prefix))
		m.add(s, strings.Join(fields, "\t"))
	}
	out.InputsSHA256, out.OutputsSHA256 = m.inputs(), m.outputs()
	for _, s := range ipEdgeList {
		out.Edges = append(out.Edges, ipEdgeJSON{s, ipOutputs(s)})
	}
	for _, s := range cidrEdgeList {
		out.CIDREdges = append(out.CIDREdges, cidrEdgeJSON{s, cidrVerdict(s)})
	}
	writeJSON("iputil_sweep.json", out)
}
