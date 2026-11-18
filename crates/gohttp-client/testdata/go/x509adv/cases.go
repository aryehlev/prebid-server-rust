package main

import (
	"bytes"
	"encoding/asn1"
	"math/big"
	"net"
	"strconv"
	"time"
)

func ncInter(n string, nc []byte) spec {
	return spec{name: n, parent: "root", exts: caExts(nc), from: -10, to: 3650}
}

func lf(n, parent string, exts [][]byte) spec {
	return spec{name: n, parent: parent, exts: exts}
}

func buildSpecs() {
	all := []spec{
		{name: "root", exts: caExts(), from: -1000, to: 7300},
		{name: "root_rsa", key: "rsa2048", exts: caExts(), from: -1000, to: 7300},
		{name: "root_rsa1024", key: "rsa1024", exts: caExts(), from: -1000, to: 7300},
		{name: "root_p384", key: "ecdsa384", exts: caExts(), from: -1000, to: 7300},
		{name: "root_p521", key: "ecdsa521", exts: caExts(), from: -1000, to: 7300},
		{name: "root_ed", key: "ed25519", exts: caExts(), from: -1000, to: 7300},
		{name: "leaf_ok", parent: "root", exts: leafExts()},
	}
	// Name constraints.
	all = append(all,
		ncInter("nc_lead_dot", ncExt(false, [][]byte{dnsName(".example.com")}, nil)),
		lf("l_nc_lead_dot_apex", "nc_lead_dot", leafNames(dnsName("example.com"))),
		lf("l_nc_lead_dot_sub", "nc_lead_dot", leafNames(dnsName("a.example.com"))),
		ncInter("nc_apex", ncExt(false, [][]byte{dnsName("example.com")}, nil)),
		lf("l_nc_apex_sub", "nc_apex", leafNames(dnsName("a.example.com"))),
		lf("l_nc_apex_notlabel", "nc_apex", leafNames(dnsName("aexample.com"))),
		lf("l_nc_apex_wild", "nc_apex", leafNames(dnsName("*.example.com"))),
		lf("l_nc_apex_trailing", "nc_apex", leafNames(dnsName("www.example.com."))),
		lf("l_nc_apex_badname", "nc_apex", leafNames(dnsName("www.example.com"), dnsName("bad name.example.com"))),
		lf("l_nc_apex_iponly", "nc_apex", leafNames(ipName("127.0.0.1"))),
		lf("l_nc_apex_cn", "nc_apex", leafNames(dnsName("www.example.com"))),
		lf("l_nc_apex_upper", "nc_apex", leafNames(dnsName("WWW.EXAMPLE.COM"))),
		ncInter("nc_sub", ncExt(false, [][]byte{dnsName("a.example.com")}, nil)),
		lf("l_nc_sub_wild", "nc_sub", leafNames(dnsName("*.example.com"))),
		ncInter("nc_excl_child", ncExt(false, nil, [][]byte{dnsName("a.example.com")})),
		lf("l_nc_excl_child_wild", "nc_excl_child", leafNames(dnsName("*.example.com"))),
		ncInter("nc_excl_parent", ncExt(false, nil, [][]byte{dnsName("example.com")})),
		lf("l_nc_excl_parent_wild", "nc_excl_parent", leafNames(dnsName("*.example.com"))),
		ncInter("nc_excl_case", ncExt(false, nil, [][]byte{dnsName("EXAMPLE.com")})),
		lf("l_nc_excl_case", "nc_excl_case", leafNames(dnsName("www.example.COM"))),
		ncInter("nc_trailing_constraint", ncExt(false, [][]byte{dnsName("example.com.")}, nil)),
		lf("l_nc_trailing_constraint", "nc_trailing_constraint", leafNames(dnsName("www.example.com"))),
		ncInter("nc_space_constraint", ncExt(false, [][]byte{dnsName("exa mple.com")}, nil)),
		lf("l_nc_space_constraint", "nc_space_constraint", leafNames(ipName("127.0.0.1"))),
		ncInter("nc_empty_perm", ncExt(false, [][]byte{dnsName("")}, nil)),
		lf("l_nc_empty_perm", "nc_empty_perm", leafExts()),
		ncInter("nc_empty_excl", ncExt(false, nil, [][]byte{dnsName("")})),
		lf("l_nc_empty_excl", "nc_empty_excl", leafExts()),
		ncInter("nc_ip_ok", ncExt(false, [][]byte{ipNet("127.0.0.0/8")}, nil)),
		lf("l_nc_ip_ok", "nc_ip_ok", leafExts()),
		ncInter("nc_ip_bad", ncExt(false, [][]byte{ipNet("10.0.0.0/8")}, nil)),
		lf("l_nc_ip_bad", "nc_ip_bad", leafExts()),
		ncInter("nc_ip_badmask", ncExt(false, [][]byte{tlv(0x87, []byte{10, 0, 0, 0, 0xff, 0, 0xff, 0})}, nil)),
		lf("l_nc_ip_badmask", "nc_ip_badmask", leafNames(dnsName("localhost"))),
		ncInter("nc_ip_badlen", ncExt(false, [][]byte{tlv(0x87, []byte{10, 0, 0, 0, 0xff})}, nil)),
		lf("l_nc_ip_badlen", "nc_ip_badlen", leafNames(dnsName("localhost"))),
		ncInter("nc_ip6_only", ncExt(false, [][]byte{ipNet("::/0")}, nil)),
		lf("l_nc_ip6_only", "nc_ip6_only", leafExts()),
		lf("l_nc_ip_mapped", "nc_ip_ok", leafNames(dnsName("localhost"), ip16Name("127.0.0.1"))),
		ncInter("nc_ip_excl", ncExt(false, nil, [][]byte{ipNet("127.0.0.0/8")})),
		lf("l_nc_ip_excl", "nc_ip_excl", leafExts()),
		ncInter("nc_ip6_excl", ncExt(false, nil, [][]byte{ipNet("::1/128")})),
		lf("l_nc_ip6_excl", "nc_ip6_excl", leafNames(ipName("::1"))),
		ncInter("nc_uri_perm", ncExt(false, [][]byte{uriName(".example.com")}, nil)),
		lf("l_nc_uri_perm", "nc_uri_perm", leafNames(dnsName("localhost"), uriName("https://a.example.com/x"))),
		ncInter("nc_dns_localhost", ncExt(false, [][]byte{dnsName("localhost")}, nil)),
		lf("l_nc_uri_unconstrained", "nc_dns_localhost", leafNames(dnsName("localhost"), uriName("https://evil.test/"))),
		lf("l_nc_email_unconstrained", "nc_dns_localhost", leafNames(dnsName("localhost"), emailName("a@evil.test"))),
		ncInter("nc_email_perm", ncExt(false, [][]byte{emailName("example.com")}, nil)),
		lf("l_nc_email_perm", "nc_email_perm", leafNames(dnsName("localhost"), emailName("a@example.com"))),
		lf("l_nc_email_perm_bad", "nc_email_perm", leafNames(dnsName("localhost"), emailName("a@evil.test"))),
		ncInter("nc_email_excl", ncExt(false, nil, [][]byte{emailName("example.com")})),
		lf("l_nc_email_excl", "nc_email_excl", leafNames(dnsName("localhost"), emailName("a@example.com"))),
		ncInter("nc_email_bad", ncExt(false, [][]byte{emailName("@@")}, nil)),
		lf("l_nc_email_bad", "nc_email_bad", leafExts()),
		ncInter("nc_dirname", ncExt(false, [][]byte{tlv(0xA4, name("x", ""))}, nil)),
		lf("l_nc_dirname", "nc_dirname", leafExts()),
		ncInter("nc_dirname_crit", ncExt(true, [][]byte{tlv(0xA4, name("x", ""))}, nil)),
		lf("l_nc_dirname_crit", "nc_dirname_crit", leafExts()),
		ncInter("nc_uri_ip", ncExt(false, [][]byte{uriName("10.0.0.1")}, nil)),
		lf("l_nc_uri_ip", "nc_uri_ip", leafExts()),
		ncInter("nc_perm_both", ncExt(false, [][]byte{dnsName("localhost"), ipNet("127.0.0.0/8")}, nil)),
		lf("l_nc_perm_both", "nc_perm_both", leafExts()),
		spec{name: "root_nc", exts: caExts(ncExt(false, [][]byte{dnsName("example.com")}, nil)), from: -1000, to: 7300},
		lf("l_root_nc", "root_nc", leafExts()),
		lf("l_root_nc_ok", "root_nc", leafNames(dnsName("www.example.com"))),
		spec{name: "inter_san_evil", parent: "root_nc", exts: caExts(sanExt(false, dnsName("evil.test"))), from: -10, to: 3650},
		lf("l_inter_san_evil", "inter_san_evil", leafNames(dnsName("www.example.com"))),
		lf("l_leaf_own_nc", "root", append(leafExts(), ncExt(true, nil, [][]byte{dnsName("localhost")}))),
		spec{name: "nc2_outer", parent: "root", exts: caExts(ncExt(false, [][]byte{dnsName("example.com")}, nil)), from: -10, to: 3650},
		spec{name: "nc2_inner", parent: "nc2_outer", exts: caExts(ncExt(false, [][]byte{dnsName("a.example.com")}, nil)), from: -10, to: 3650},
		lf("l_nc2_bad", "nc2_inner", leafNames(dnsName("b.example.com"))),
		lf("l_nc2_ok", "nc2_inner", leafNames(dnsName("x.a.example.com"))),
	)
	// Policies.
	all = append(all,
		spec{name: "pol_req", parent: "root", exts: caExts(policyConstraintsExt(false, 0, -1)), from: -10, to: 3650},
		lf("l_pol_req", "pol_req", leafExts()),
		lf("l_pol_req_pol", "pol_req", append(leafExts(), policiesExt(false, "1.2.3.4"))),
		spec{name: "pol_req_any", parent: "root", exts: caExts(policiesExt(false, anyPolicy), policyConstraintsExt(false, 0, -1)), from: -10, to: 3650},
		lf("l_pol_req_any", "pol_req_any", append(leafExts(), policiesExt(false, "1.2.3.4"))),
		lf("l_pol_req_any_none", "pol_req_any", leafExts()),
		spec{name: "pol_map_any", parent: "root", exts: caExts(policiesExt(false, anyPolicy), policyMappingsExt(false, [2]string{anyPolicy, "1.2.3"})), from: -10, to: 3650},
		lf("l_pol_map_any", "pol_map_any", leafExts()),
		spec{name: "root_pol", exts: caExts(policyConstraintsExt(false, 0, -1)), from: -1000, to: 7300},
		lf("l_root_pol", "root_pol", leafExts()),
		lf("l_pol_leaf_req", "root", append(leafExts(), policyConstraintsExt(false, 0, -1))),
		spec{name: "pol_inhibit", parent: "root", exts: caExts(policiesExt(false, anyPolicy), inhibitAnyExt(false, 0), policyConstraintsExt(false, 0, -1)), from: -10, to: 3650},
		lf("l_pol_inhibit", "pol_inhibit", append(leafExts(), policiesExt(false, anyPolicy))),
		spec{name: "pol_crit", parent: "root", exts: caExts(policyConstraintsExt(true, 5, -1)), from: -10, to: 3650},
		lf("l_pol_crit", "pol_crit", leafExts()),
		spec{name: "pol_map_ok", parent: "root", exts: caExts(policiesExt(false, "1.2.3"), policyMappingsExt(false, [2]string{"1.2.3", "1.2.4"}), policyConstraintsExt(false, 0, -1)), from: -10, to: 3650},
		lf("l_pol_map_ok", "pol_map_ok", append(leafExts(), policiesExt(false, "1.2.4"))),
		lf("l_pol_map_bad", "pol_map_ok", append(leafExts(), policiesExt(false, "1.2.3"))),
	)
	// Extended key usages.
	all = append(all,
		spec{name: "eku_code", parent: "root", exts: caExts(ekuExt(ekuCode)), from: -10, to: 3650},
		lf("l_eku_code", "eku_code", leafExts()),
		spec{name: "eku_unknown", parent: "root", exts: caExts(ekuExt("1.2.3.4")), from: -10, to: 3650},
		lf("l_eku_unknown", "eku_unknown", leafExts()),
		spec{name: "eku_any", parent: "root", exts: caExts(ekuExt(ekuAny)), from: -10, to: 3650},
		lf("l_eku_any_client", "eku_any", [][]byte{kuExt(0), ekuExt(ekuClient), sanExt(false, dnsName("localhost"), ipName("127.0.0.1"))}),
		spec{name: "eku_mixed", parent: "root", exts: caExts(ekuExt(ekuClient, ekuServer)), from: -10, to: 3650},
		lf("l_eku_mixed", "eku_mixed", leafExts()),
		lf("l_eku_sgc", "root", [][]byte{kuExt(0), ekuExt(ekuNSSGC), sanExt(false, dnsName("localhost"), ipName("127.0.0.1"))}),
		spec{name: "eku_empty", parent: "root", exts: caExts(ext("2.5.29.37", false, seq())), from: -10, to: 3650},
		lf("l_eku_empty", "eku_empty", leafExts()),
		spec{name: "root_eku_code", exts: caExts(ekuExt(ekuCode)), from: -1000, to: 7300},
		lf("l_root_eku_code", "root_eku_code", leafExts()),
	)
	// Validity.
	exact := timeDER(now)
	all = append(all,
		spec{name: "val_inter_notyet", parent: "root", exts: caExts(), from: 10, to: 3650},
		lf("l_val_inter_notyet", "val_inter_notyet", leafExts()),
		spec{name: "l_val_boundary", parent: "root", exts: leafExts(), notAfter: exact},
		spec{name: "l_val_boundary_nb", parent: "root", exts: leafExts(), notBefore: exact},
		spec{name: "val_inter_boundary", parent: "root", exts: caExts(), notAfter: exact},
		lf("l_val_inter_boundary", "val_inter_boundary", leafExts()),
		spec{name: "root_boundary", exts: caExts(), notAfter: exact},
		lf("l_root_boundary", "root_boundary", leafExts()),
		spec{name: "l_val_offset", parent: "root", exts: leafExts(), notBefore: tlv(0x17, []byte("260101000000+0100")), notAfter: tlv(0x17, []byte("270101000000-0130"))},
		spec{name: "l_val_gentime", parent: "root", exts: leafExts(), notAfter: generalizedTime(now.Add(30 * day))},
		spec{name: "val_inter_offset", parent: "root", exts: caExts(), notBefore: tlv(0x17, []byte("260101000000+0100")), notAfter: tlv(0x17, []byte("300101000000+0100"))},
		lf("l_val_inter_offset", "val_inter_offset", leafExts()),
		spec{name: "l_val_minutes", parent: "root", exts: leafExts(), notBefore: tlv(0x17, []byte("2601010000Z"))},
		spec{name: "l_val_leap", parent: "root", exts: leafExts(), notBefore: tlv(0x17, []byte("260101235960Z"))},
		spec{name: "l_val_pre1970", parent: "root", exts: leafExts(), notBefore: tlv(0x17, []byte("600101000000Z"))},
		spec{name: "val_inter_pre1970", parent: "root", exts: caExts(), notBefore: tlv(0x17, []byte("600101000000Z"))},
		lf("l_val_inter_pre1970", "val_inter_pre1970", leafExts()),
	)
	// Signatures.
	all = append(all,
		spec{name: "l_sig_sha1_rsa", parent: "root_rsa", exts: leafExts(), alg: "rsa-sha1"},
		spec{name: "l_sig_sha1_ecdsa", parent: "root", exts: leafExts(), alg: "ecdsa-sha1"},
		spec{name: "l_sig_md5", parent: "root_rsa", exts: leafExts(), alg: "rsa-md5"},
		lf("l_sig_rsa1024", "root_rsa1024", leafExts()),
		lf("l_sig_rsa2048", "root_rsa", leafExts()),
		spec{name: "l_sig_rsa384", parent: "root_rsa", exts: leafExts(), alg: "rsa-sha384"},
		spec{name: "l_sig_rsa512", parent: "root_rsa", exts: leafExts(), alg: "rsa-sha512"},
		lf("l_sig_p384", "root_p384", leafExts()),
		lf("l_sig_p521", "root_p521", leafExts()),
		spec{name: "l_sig_p521_256", parent: "root_p521", exts: leafExts(), alg: "ecdsa-sha256"},
		spec{name: "l_sig_p384_256", parent: "root_p384", exts: leafExts(), alg: "ecdsa-sha256"},
		lf("l_sig_ed", "root_ed", leafExts()),
		spec{name: "l_sig_p256_512", parent: "root", exts: leafExts(), alg: "ecdsa-sha512"},
		spec{name: "l_sig_p256_384", parent: "root", exts: leafExts(), alg: "ecdsa-sha384"},
		spec{name: "l_sig_rsa_noparm", parent: "root_rsa", exts: leafExts(), alg: "rsa-sha256-noparm"},
		spec{name: "l_sig_wrongtype", parent: "root_rsa", exts: leafExts(), ai: algs["ecdsa-sha256"].ai},
		spec{name: "l_sig_corrupt", parent: "root", exts: leafExts(), corruptSig: true},
		spec{name: "l_sig_corrupt_rsa", parent: "root_rsa", exts: leafExts(), corruptSig: true},
		spec{name: "l_sig_corrupt_ed", parent: "root_ed", exts: leafExts(), corruptSig: true},
		spec{name: "l_rsa1024_key", parent: "root", key: "rsa1024", exts: leafExts()},
	)
	// Basic constraints and key usage.
	all = append(all,
		spec{name: "bc_none", parent: "root", exts: [][]byte{kuExt(0, 5, 6)}, from: -10, to: 3650},
		lf("l_bc_none", "bc_none", leafExts()),
		spec{name: "bc_v1", parent: "root", version: 1, from: -10, to: 3650},
		lf("l_bc_v1", "bc_v1", leafExts()),
		spec{name: "bc_pl0", parent: "root", exts: [][]byte{bcExt(true, true, 0), kuExt(0, 5, 6)}, from: -10, to: 3650},
		spec{name: "bc_pl0_sub", parent: "bc_pl0", exts: caExts(), from: -10, to: 3650},
		lf("l_bc_pl0", "bc_pl0_sub", leafExts()),
		spec{name: "bc_pl300", parent: "root", exts: [][]byte{bcExt(true, true, 300), kuExt(0, 5, 6)}, from: -10, to: 3650},
		lf("l_bc_pl300", "bc_pl300", leafExts()),
		spec{name: "root_v1", version: 1, from: -1000, to: 7300},
		lf("l_root_v1", "root_v1", leafExts()),
		spec{name: "root_ku_nosign", exts: [][]byte{bcExt(true, true, -1), kuExt(0)}, from: -1000, to: 7300},
		lf("l_root_ku_nosign", "root_ku_nosign", leafExts()),
		spec{name: "bc_noncrit", parent: "root", exts: [][]byte{bcExt(false, true, -1), kuExt(0, 5, 6)}, from: -10, to: 3650},
		lf("l_bc_noncrit", "bc_noncrit", leafExts()),
		spec{name: "l_leaf_ca", parent: "root", exts: append(leafExts(), bcExt(true, true, -1))},
		spec{name: "l_leaf_bc_false", parent: "root", exts: append(leafExts(), bcExt(true, false, -1))},
	)
	// Critical extensions.
	all = append(all,
		spec{name: "root_crit", exts: caExts(ext("1.2.3.4", true, []byte{5, 0})), from: -1000, to: 7300},
		lf("l_root_crit", "root_crit", leafExts()),
		lf("l_crit_san_other", "root", [][]byte{kuExt(0), ekuExt(ekuServer), sanExt(true, tlv(0xA0, oid("1.2.3"), tlv(0xA0, utf8String("x"))))}),
		lf("l_crit_nc_leaf", "root", append(leafExts(), ncExt(true, [][]byte{dnsName("example.com")}, nil))),
		spec{name: "crit_inhibit", parent: "root", exts: caExts(inhibitAnyExt(true, 5)), from: -10, to: 3650},
		lf("l_crit_inhibit", "crit_inhibit", leafExts()),
		lf("l_crit_eku", "root", [][]byte{kuExt(0), ext("2.5.29.37", true, seq(oid(ekuServer))), sanExt(false, dnsName("localhost"), ipName("127.0.0.1"))}),
		lf("l_crit_crldp", "root", append(leafExts(), ext("2.5.29.31", true, seq(seq(tlv(0xA0, tlv(0xA0, uriName("http://crl.test/x.crl")))))))),
	)
	// Parse-time rejections of presented and pooled certificates.
	rsaBigN := []byte{0x00}
	for i := 0; i < 1025; i++ {
		rsaBigN = append(rsaBigN, 0xC3)
	}
	all = append(all,
		spec{name: "u_rsa_zero", parent: "root", exts: leafExts(), spki: seq(seq(oid("1.2.840.113549.1.1.1"), null()), bitString(seq(integer(0), integer(65537))))},
		spec{name: "u_rsa_noparams", parent: "root", exts: leafExts(), spki: seq(seq(oid("1.2.840.113549.1.1.1")), bitString(seq(integer(12345), integer(65537))))},
		spec{name: "u_rsa_big", parent: "root", exts: leafExts(), spki: seq(seq(oid("1.2.840.113549.1.1.1"), null()), bitString(seq(tlv(0x02, rsaBigN), integer(65537))))},
		spec{name: "u_ec_offcurve", parent: "root", exts: leafExts(), spki: seq(seq(oid("1.2.840.10045.2.1"), oid("1.2.840.10045.3.1.7")), bitString(append([]byte{4}, make([]byte, 64)...)))},
		spec{name: "u_ec_curve", parent: "root", exts: leafExts(), spki: seq(seq(oid("1.2.840.10045.2.1"), oid("1.3.132.0.10")), bitString(append([]byte{4}, make([]byte, 64)...)))},
		spec{name: "u_ec_compressed", parent: "root", exts: leafExts(), spki: seq(seq(oid("1.2.840.10045.2.1"), oid("1.2.840.10045.3.1.7")), bitString(append([]byte{2}, make([]byte, 32)...)))},
		spec{name: "u_ed_params", parent: "root", exts: leafExts(), spki: seq(seq(oid("1.3.101.112"), null()), bitString(make([]byte, 32)))},
		spec{name: "u_ed_size", parent: "root", exts: leafExts(), spki: seq(seq(oid("1.3.101.112")), bitString(make([]byte, 31)))},
		spec{name: "u_x25519", parent: "root", exts: leafExts(), spki: seq(seq(oid("1.3.101.110")), bitString(make([]byte, 32)))},
		spec{name: "u_x25519_size", parent: "root", exts: leafExts(), spki: seq(seq(oid("1.3.101.110")), bitString(make([]byte, 31)))},
		spec{name: "u_unknown_key", parent: "root", exts: leafExts(), spki: seq(seq(oid("1.2.3.4.5")), bitString([]byte{1, 2, 3}))},
		spec{name: "u_dsa_bad", parent: "root", exts: leafExts(), spki: seq(seq(oid("1.2.840.10040.4.1"), seq(integer(0), integer(1), integer(1))), bitString(integer(5)))},
		lf("u_uri_bad", "root", leafNames(dnsName("localhost"), ipName("127.0.0.1"), uriName("http://[::1"))),
		lf("u_uri_baddomain", "root", leafNames(dnsName("localhost"), ipName("127.0.0.1"), uriName("http://exa mple.com/"))),
		spec{name: "u_nc_badmask", parent: "root", exts: caExts(ncExt(false, [][]byte{tlv(0x87, []byte{10, 0, 0, 0, 0xff, 0, 0xff, 0})}, nil)), from: -10, to: 3650},
		spec{name: "root_badnc", exts: caExts(ncExt(false, [][]byte{tlv(0x87, []byte{10, 0, 0, 0, 0xff, 0, 0xff, 0})}, nil)), from: -1000, to: 7300},
		lf("l_root_badnc", "root_badnc", leafExts()),
		spec{name: "root_baduri", exts: caExts(sanExt(false, uriName("http://[::1"))), from: -1000, to: 7300},
		lf("l_root_baduri", "root_baduri", leafExts()),
		spec{name: "root_rsa_noparams", keyOf: "root_rsa", cn: "root_rsa", exts: caExts(), from: -1000, to: 7300, spki: append([]byte{}, noNullSPKI(certs)...)},
	)
	// Hostnames.
	all = append(all,
		lf("l_host_trailing", "root", leafNames(dnsName("localhost."))),
		lf("l_host_star", "root", leafNames(dnsName("*.example.com"))),
		lf("l_host_partial", "root", leafNames(dnsName("a*.example.com"))),
		lf("l_host_empty", "root", leafNames(dnsName(""))),
		lf("l_host_ipdns", "root", leafNames(dnsName("127.0.0.1"))),
		lf("l_host_upper", "root", leafNames(dnsName("LOCALHOST"))),
		lf("l_host_v6", "root", leafNames(ipName("::1"))),
		lf("l_host_mapped", "root", leafNames(ip16Name("127.0.0.1"))),
	)
	var many, manyIP [][]byte
	for i := 0; i < 100; i++ {
		many = append(many, dnsName("n"+strconv.Itoa(i)+".test"))
		manyIP = append(manyIP, ipName("10.0.0."+strconv.Itoa(i)))
	}
	all = append(all,
		lf("l_host_many", "root", leafNames(many...)),
		lf("l_host_many_ip", "root", leafNames(manyIP...)),
	)
	// Cross-signing and candidate order.
	all = append(all,
		spec{name: "root_old", exts: caExts(), from: -4000, to: -100},
		spec{name: "x_inter_old", parent: "root_old", cn: "x_inter", exts: caExts(), from: -100, to: 3650},
		spec{name: "x_inter_new", parent: "root", cn: "x_inter", keyOf: "x_inter_old", exts: caExts(), from: -10, to: 3650},
		lf("l_x", "x_inter_old", leafExts()),
		spec{name: "loop_a", cn: "loop_a", exts: caExts(), from: -10, to: 3650},
		spec{name: "loop_b", parent: "loop_a", exts: caExts(), from: -10, to: 3650},
		spec{name: "loop_a2", parent: "loop_b", cn: "loop_a", keyOf: "loop_a", exts: caExts(), from: -10, to: 3650},
		lf("l_loop", "loop_a", leafExts()),
		spec{name: "root_twin_bad", cn: "twin", exts: caExts(skidExt([]byte{1, 2, 3})), from: -1000, to: 7300},
		spec{name: "root_twin_good", cn: "twin", exts: caExts(skidExt([]byte{9, 9, 9})), from: -1000, to: 7300},
		spec{name: "l_twin", parent: "root_twin_good", exts: append(leafExts(), akidExt([]byte{9, 9, 9}))},
		spec{name: "root_twin_expired", cn: "twin2", exts: caExts(skidExt([]byte{7})), from: -1000, to: -5},
		spec{name: "root_twin_other", cn: "twin2", exts: caExts(), from: -1000, to: 7300},
		spec{name: "l_twin2", parent: "root_twin_expired", exts: append(leafExts(), akidExt([]byte{7}))},
		spec{name: "deep1", parent: "root", exts: caExts(), from: -10, to: 3650},
		spec{name: "deep2", parent: "deep1", exts: caExts(), from: -10, to: 3650},
		spec{name: "deep3", parent: "deep2", exts: caExts(), from: -10, to: 3650},
		spec{name: "deep4", parent: "deep3", exts: caExts(), from: -10, to: 3650},
		spec{name: "deep5", parent: "deep4", exts: caExts(), from: -10, to: 3650},
		spec{name: "deep6", parent: "deep5", exts: caExts(), from: -10, to: 3650},
		spec{name: "deep7", parent: "deep6", exts: caExts(), from: -10, to: 3650},
		lf("l_deep", "deep7", leafExts()),
	)
	// RSA-PSS parameters, which getSignatureAlgorithmFromAI reads with encoding/asn1: an explicit
	// tag's length is not checked, the optional trailer field falls back to its default for an
	// element that does not match, and extra bytes at the end of a SEQUENCE are ignored.
	h256, m256, s32 := pssHash(oidSHA256), pssMGF(oidSHA256), pssSalt(32)
	relen := func(field []byte, n byte) []byte { return append([]byte{field[0], n}, field[2:]...) }
	pss := func(n string, fields ...[]byte) spec {
		return spec{name: n, parent: "root_rsa", exts: leafExts(), alg: "rsa-pss-sha256", ai: pssAI(fields...)}
	}
	all = append(all,
		spec{name: "l_pss_sha256", parent: "root_rsa", exts: leafExts(), alg: "rsa-pss-sha256"},
		spec{name: "l_pss_sha384", parent: "root_rsa", exts: leafExts(), alg: "rsa-pss-sha384"},
		spec{name: "l_pss_sha512", parent: "root_rsa", exts: leafExts(), alg: "rsa-pss-sha512"},
		spec{name: "l_pss_ec_parent", parent: "root", exts: leafExts(), alg: "rsa-pss-sha256"},
		pss("l_pss_wrapper_long", relen(h256, 0x7F), m256, s32),
		pss("l_pss_wrapper_short", relen(h256, 0x01), m256, s32),
		pss("l_pss_wrapper_spill", tlv(0xA0, seq(oid(oidSHA256), null()), m256), s32),
		pss("l_pss_wrapper_primitive", append([]byte{0x80}, h256[1:]...), m256, s32),
		pss("l_pss_trailer_one", h256, m256, s32, tlv(0xA3, integer(1))),
		pss("l_pss_trailer_two", h256, m256, s32, tlv(0xA3, integer(2))),
		pss("l_pss_trailer_null_last", h256, m256, s32, null()),
		pss("l_pss_trailer_nulls", h256, m256, s32, null(), null()),
		pss("l_pss_trailer_empty", h256, m256, s32, []byte{0xA3, 0x00}, null()),
		pss("l_pss_trailer_octets", h256, m256, s32, tlv(0xA3, octets([]byte{2}))),
		pss("l_pss_trailer_primitive", h256, m256, s32, []byte{0x83, 0x01, 0x02}),
		pss("l_pss_trailer_high_tag", h256, m256, s32, []byte{0xBF, 0x1F, 0x00}, null()),
		pss("l_pss_trailer_nonminimal_tag", h256, m256, s32, []byte{0xBF, 0x1E, 0x00}, null()),
		pss("l_pss_trailer_long_form", h256, m256, s32, []byte{0x04, 0x81, 0x01, 0x00}, null()),
		pss("l_pss_trailer_extra", h256, m256, s32, tlv(0xA3, integer(1)), octets(nil)),
		pss("l_pss_salt_20", h256, m256, pssSalt(20)),
		pss("l_pss_hash_params", tlv(0xA0, seq(oid(oidSHA256), integer(0))), m256, s32),
		pss("l_pss_hash_extra", tlv(0xA0, seq(oid(oidSHA256), null(), null())), m256, s32),
		pss("l_pss_mgf_sha384", h256, pssMGF(oidSHA384), s32),
		pss("l_pss_no_salt", h256, m256),
	)
	// IPv4 addresses in 16 bytes: Go keeps IPv4 and IPv6 constraints apart by their length and
	// looks a name up by its own length.
	mapped := func(n string, permitted, excluded [][]byte, ip []byte) []spec {
		return []spec{ncInter(n, ncExt(false, permitted, excluded)), lf("l_"+n, n, leafNames(dnsName("localhost"), ip))}
	}
	mappedNet := tlv(0x87, append(net.ParseIP("10.0.0.0").To16(), append(bytes.Repeat([]byte{0xFF}, 15), 0x00)...))
	all = append(all, mapped("nc_mapped_san_permitted", [][]byte{ipNet("10.0.0.0/8")}, nil, ip16Name("10.0.0.1"))...)
	all = append(all, mapped("nc_mapped_san_excluded", nil, [][]byte{ipNet("10.0.0.0/8")}, ip16Name("10.0.0.1"))...)
	all = append(all, mapped("nc_mapped_net_permitted", [][]byte{mappedNet}, nil, ipName("10.0.0.1"))...)
	all = append(all, mapped("nc_mapped_both", [][]byte{mappedNet}, nil, ip16Name("10.0.0.1"))...)
	all = append(all, mapped("nc_mapped_net_excluded", nil, [][]byte{mappedNet}, ip16Name("10.0.0.1"))...)
	// Signature encodings: crypto/rsa takes a signature exactly as long as the modulus, and
	// ecdsa.VerifyASN1 a DER SEQUENCE of two minimal INTEGERs and nothing after it.
	all = append(all,
		spec{name: "l_sig_rsa_short", parent: "root_rsa", exts: leafExts(), zeroSig: true, sigEdit: func(b []byte) []byte { return b[1:] }},
		spec{name: "l_sig_rsa_long", parent: "root_rsa", exts: leafExts(), sigEdit: func(b []byte) []byte { return append([]byte{0}, b...) }},
		spec{name: "l_sig_ec_nonminimal", parent: "root", exts: leafExts(), sigEdit: ecNonMinimal},
		spec{name: "l_sig_ec_trailing", parent: "root", exts: leafExts(), sigEdit: func(b []byte) []byte { return append(b, 0) }},
		spec{name: "l_sig_ec_extra_int", parent: "root", exts: leafExts(), sigEdit: ecExtraInteger},
		spec{name: "l_sig_ed_short", parent: "root_ed", exts: leafExts(), sigEdit: func(b []byte) []byte { return b[:len(b)-1] }},
		// Go handles critical certificate policies in a leaf, which rustls-webpki refuses to read;
		// its key signs the handshake in the Rust tests.
		spec{name: "l_hs_policies", parent: "root", exts: leafExts(policiesExt(true, anyPolicy)), exportKey: true},
	)
	// buildChains stops after 100 signature checks: issuers that share a name and fail count.
	for i := 0; i < 100; i++ {
		all = append(all, spec{name: "lim_bad_" + strconv.Itoa(i), cn: "lim", exts: caExts(), from: -1000, to: 7300})
	}
	all = append(all,
		spec{name: "lim_good", cn: "lim", exts: caExts(), from: -1000, to: 7300},
		lf("l_lim", "lim_good", leafExts()),
	)
	for _, s := range all {
		make1(s)
	}
}

// ecdsaSig splits an ECDSA signature into its INTEGERs' contents.
func ecdsaSig(sig []byte) (r, s []byte) {
	var parsed struct{ R, S *big.Int }
	if _, err := asn1.Unmarshal(sig, &parsed); err != nil {
		panic(err)
	}
	return bigInteger(parsed.R)[2:], bigInteger(parsed.S)[2:]
}

// ecNonMinimal re-encodes an ECDSA signature with a superfluous zero byte before r.
func ecNonMinimal(sig []byte) []byte {
	r, s := ecdsaSig(sig)
	return seq(tlv(0x02, append([]byte{0}, r...)), tlv(0x02, s))
}

// ecExtraInteger re-encodes an ECDSA signature with a third INTEGER.
func ecExtraInteger(sig []byte) []byte {
	r, s := ecdsaSig(sig)
	return seq(tlv(0x02, r), tlv(0x02, s), integer(0))
}

// limitRoots is lim_bad_0 to lim_bad_<bad-1>, then lim_good.
func limitRoots(bad int) []string {
	var roots []string
	for i := 0; i < bad; i++ {
		roots = append(roots, "lim_bad_"+strconv.Itoa(i))
	}
	return append(roots, "lim_good")
}

// noNullSPKI re-encodes root_rsa's RSA key without the NULL parameters.
func noNullSPKI(_ map[string]*built) []byte {
	k := newKey("root_rsa", "rsa2048")
	// SubjectPublicKeyInfo: SEQ { SEQ { OID, NULL }, BIT STRING }; drop the NULL.
	spki := k.spki
	// Skip outer SEQ header.
	inner := spki[4:]
	// AlgorithmIdentifier: 30 0d 06 09 ... 05 00
	ai := inner[:15]
	rest := inner[15:]
	return seq(seq(ai[2:13]), rest)
}

func buildCases() []verifyCase {
	r := []string{"root"}
	half := int64(500 * time.Millisecond)
	cases := []verifyCase{
		one("ok", r, "localhost", "leaf_ok"),
		one("nc_lead_dot_apex", r, "example.com", "l_nc_lead_dot_apex", "nc_lead_dot"),
		one("nc_lead_dot_sub", r, "a.example.com", "l_nc_lead_dot_sub", "nc_lead_dot"),
		one("nc_apex_sub", r, "a.example.com", "l_nc_apex_sub", "nc_apex"),
		one("nc_apex_notlabel", r, "aexample.com", "l_nc_apex_notlabel", "nc_apex"),
		one("nc_apex_wild", r, "a.example.com", "l_nc_apex_wild", "nc_apex"),
		one("nc_apex_trailing", r, "www.example.com.", "l_nc_apex_trailing", "nc_apex"),
		one("nc_apex_badname", r, "www.example.com", "l_nc_apex_badname", "nc_apex"),
		one("nc_apex_iponly", r, "127.0.0.1", "l_nc_apex_iponly", "nc_apex"),
		one("nc_apex_upper", r, "www.example.com", "l_nc_apex_upper", "nc_apex"),
		one("nc_sub_wild", r, "a.example.com", "l_nc_sub_wild", "nc_sub"),
		one("nc_excl_child_wild", r, "b.example.com", "l_nc_excl_child_wild", "nc_excl_child"),
		one("nc_excl_parent_wild", r, "a.example.com", "l_nc_excl_parent_wild", "nc_excl_parent"),
		one("nc_excl_case", r, "www.example.com", "l_nc_excl_case", "nc_excl_case"),
		one("nc_trailing_constraint", r, "www.example.com", "l_nc_trailing_constraint", "nc_trailing_constraint"),
		one("nc_space_constraint", r, "127.0.0.1", "l_nc_space_constraint", "nc_space_constraint"),
		one("nc_empty_perm", r, "localhost", "l_nc_empty_perm", "nc_empty_perm"),
		one("nc_empty_excl", r, "localhost", "l_nc_empty_excl", "nc_empty_excl"),
		one("nc_ip_ok", r, "127.0.0.1", "l_nc_ip_ok", "nc_ip_ok"),
		one("nc_ip_bad", r, "127.0.0.1", "l_nc_ip_bad", "nc_ip_bad"),
		one("nc_ip_badmask", r, "localhost", "l_nc_ip_badmask", "nc_ip_badmask"),
		one("nc_ip_badlen", r, "localhost", "l_nc_ip_badlen", "nc_ip_badlen"),
		one("nc_ip6_only", r, "127.0.0.1", "l_nc_ip6_only", "nc_ip6_only"),
		one("nc_ip_mapped", r, "127.0.0.1", "l_nc_ip_mapped", "nc_ip_ok"),
		one("nc_ip_excl", r, "127.0.0.1", "l_nc_ip_excl", "nc_ip_excl"),
		one("nc_ip6_excl", r, "::1", "l_nc_ip6_excl", "nc_ip6_excl"),
		one("nc_uri_perm", r, "localhost", "l_nc_uri_perm", "nc_uri_perm"),
		one("nc_uri_unconstrained", r, "localhost", "l_nc_uri_unconstrained", "nc_dns_localhost"),
		one("nc_email_unconstrained", r, "localhost", "l_nc_email_unconstrained", "nc_dns_localhost"),
		one("nc_email_perm", r, "localhost", "l_nc_email_perm", "nc_email_perm"),
		one("nc_email_perm_bad", r, "localhost", "l_nc_email_perm_bad", "nc_email_perm"),
		one("nc_email_excl", r, "localhost", "l_nc_email_excl", "nc_email_excl"),
		one("nc_email_bad", r, "localhost", "l_nc_email_bad", "nc_email_bad"),
		one("nc_dirname", r, "localhost", "l_nc_dirname", "nc_dirname"),
		one("nc_dirname_crit", r, "localhost", "l_nc_dirname_crit", "nc_dirname_crit"),
		one("nc_uri_ip", r, "localhost", "l_nc_uri_ip", "nc_uri_ip"),
		one("nc_perm_both", r, "localhost", "l_nc_perm_both", "nc_perm_both"),
		one("nc_root", []string{"root_nc"}, "localhost", "l_root_nc"),
		one("nc_root_ok", []string{"root_nc"}, "www.example.com", "l_root_nc_ok"),
		one("nc_inter_san", []string{"root_nc"}, "www.example.com", "l_inter_san_evil", "inter_san_evil"),
		one("nc_leaf_own", r, "localhost", "l_leaf_own_nc"),
		one("nc2_bad", r, "b.example.com", "l_nc2_bad", "nc2_inner", "nc2_outer"),
		one("nc2_ok", r, "x.a.example.com", "l_nc2_ok", "nc2_inner", "nc2_outer"),

		one("pol_req", r, "localhost", "l_pol_req", "pol_req"),
		one("pol_req_pol", r, "localhost", "l_pol_req_pol", "pol_req"),
		one("pol_req_any", r, "localhost", "l_pol_req_any", "pol_req_any"),
		one("pol_req_any_none", r, "localhost", "l_pol_req_any_none", "pol_req_any"),
		one("pol_map_any", r, "localhost", "l_pol_map_any", "pol_map_any"),
		one("pol_root", []string{"root_pol"}, "localhost", "l_root_pol"),
		one("pol_leaf_req", r, "localhost", "l_pol_leaf_req"),
		one("pol_inhibit", r, "localhost", "l_pol_inhibit", "pol_inhibit"),
		one("pol_crit", r, "localhost", "l_pol_crit", "pol_crit"),
		one("pol_map_ok", r, "localhost", "l_pol_map_ok", "pol_map_ok"),
		one("pol_map_bad", r, "localhost", "l_pol_map_bad", "pol_map_ok"),

		one("eku_code", r, "localhost", "l_eku_code", "eku_code"),
		one("eku_unknown", r, "localhost", "l_eku_unknown", "eku_unknown"),
		one("eku_any_client", r, "localhost", "l_eku_any_client", "eku_any"),
		one("eku_mixed", r, "localhost", "l_eku_mixed", "eku_mixed"),
		one("eku_sgc", r, "localhost", "l_eku_sgc"),
		one("eku_empty", r, "localhost", "l_eku_empty", "eku_empty"),
		one("eku_root_code", []string{"root_eku_code"}, "localhost", "l_root_eku_code"),

		one("val_inter_notyet", r, "localhost", "l_val_inter_notyet", "val_inter_notyet"),
		one("val_boundary", r, "localhost", "l_val_boundary"),
		{Name: "val_boundary_half", Roots: r, Host: "localhost", Chain: []string{"l_val_boundary"}, Nanos: half},
		{Name: "val_boundary_nb_minus_half", Roots: r, Host: "localhost", Chain: []string{"l_val_boundary_nb"}, Nanos: -half},
		one("val_inter_boundary", r, "localhost", "l_val_inter_boundary", "val_inter_boundary"),
		{Name: "val_inter_boundary_half", Roots: r, Host: "localhost", Chain: []string{"l_val_inter_boundary", "val_inter_boundary"}, Nanos: half},
		{Name: "val_root_boundary_half", Roots: []string{"root_boundary"}, Host: "localhost", Chain: []string{"l_root_boundary"}, Nanos: half},
		one("val_offset", r, "localhost", "l_val_offset"),
		one("val_gentime", r, "localhost", "l_val_gentime"),
		one("val_inter_offset", r, "localhost", "l_val_inter_offset", "val_inter_offset"),
		one("val_minutes", r, "localhost", "l_val_minutes"),
		one("val_leap", r, "localhost", "l_val_leap"),
		one("val_pre1970", r, "localhost", "l_val_pre1970"),
		one("val_inter_pre1970", r, "localhost", "l_val_inter_pre1970", "val_inter_pre1970"),

		one("sig_sha1_rsa", []string{"root_rsa"}, "localhost", "l_sig_sha1_rsa"),
		one("sig_sha1_ecdsa", r, "localhost", "l_sig_sha1_ecdsa"),
		one("sig_md5", []string{"root_rsa"}, "localhost", "l_sig_md5"),
		one("sig_rsa1024", []string{"root_rsa1024"}, "localhost", "l_sig_rsa1024"),
		one("sig_rsa2048", []string{"root_rsa"}, "localhost", "l_sig_rsa2048"),
		one("sig_rsa384", []string{"root_rsa"}, "localhost", "l_sig_rsa384"),
		one("sig_rsa512", []string{"root_rsa"}, "localhost", "l_sig_rsa512"),
		one("sig_p384", []string{"root_p384"}, "localhost", "l_sig_p384"),
		one("sig_p521", []string{"root_p521"}, "localhost", "l_sig_p521"),
		one("sig_p521_256", []string{"root_p521"}, "localhost", "l_sig_p521_256"),
		one("sig_p384_256", []string{"root_p384"}, "localhost", "l_sig_p384_256"),
		one("sig_ed", []string{"root_ed"}, "localhost", "l_sig_ed"),
		one("sig_p256_512", r, "localhost", "l_sig_p256_512"),
		one("sig_p256_384", r, "localhost", "l_sig_p256_384"),
		one("sig_rsa_noparm", []string{"root_rsa"}, "localhost", "l_sig_rsa_noparm"),
		one("sig_wrongtype", []string{"root_rsa"}, "localhost", "l_sig_wrongtype"),
		one("sig_corrupt", r, "localhost", "l_sig_corrupt"),
		one("sig_corrupt_rsa", []string{"root_rsa"}, "localhost", "l_sig_corrupt_rsa"),
		one("sig_corrupt_ed", []string{"root_ed"}, "localhost", "l_sig_corrupt_ed"),
		one("rsa1024_leaf_key", r, "localhost", "l_rsa1024_key"),

		one("bc_none", r, "localhost", "l_bc_none", "bc_none"),
		one("bc_v1", r, "localhost", "l_bc_v1", "bc_v1"),
		one("bc_pl0", r, "localhost", "l_bc_pl0", "bc_pl0_sub", "bc_pl0"),
		one("bc_pl300", r, "localhost", "l_bc_pl300", "bc_pl300"),
		one("root_v1", []string{"root_v1"}, "localhost", "l_root_v1"),
		one("root_ku_nosign", []string{"root_ku_nosign"}, "localhost", "l_root_ku_nosign"),
		one("bc_noncrit", r, "localhost", "l_bc_noncrit", "bc_noncrit"),
		one("leaf_ca", r, "localhost", "l_leaf_ca"),
		one("leaf_bc_false", r, "localhost", "l_leaf_bc_false"),

		one("crit_root", []string{"root_crit"}, "localhost", "l_root_crit"),
		one("crit_san_other", r, "localhost", "l_crit_san_other"),
		one("crit_nc_leaf", r, "localhost", "l_crit_nc_leaf"),
		one("crit_inhibit", r, "localhost", "l_crit_inhibit", "crit_inhibit"),
		one("crit_eku", r, "localhost", "l_crit_eku"),
		one("crit_crldp", r, "localhost", "l_crit_crldp"),

		one("unused_rsa_zero", r, "localhost", "leaf_ok", "u_rsa_zero"),
		one("unused_rsa_noparams", r, "localhost", "leaf_ok", "u_rsa_noparams"),
		one("unused_rsa_big", r, "localhost", "leaf_ok", "u_rsa_big"),
		one("unused_ec_offcurve", r, "localhost", "leaf_ok", "u_ec_offcurve"),
		one("unused_ec_curve", r, "localhost", "leaf_ok", "u_ec_curve"),
		one("unused_ec_compressed", r, "localhost", "leaf_ok", "u_ec_compressed"),
		one("unused_ed_params", r, "localhost", "leaf_ok", "u_ed_params"),
		one("unused_ed_size", r, "localhost", "leaf_ok", "u_ed_size"),
		one("unused_x25519_size", r, "localhost", "leaf_ok", "u_x25519_size"),
		one("unused_dsa_bad", r, "localhost", "leaf_ok", "u_dsa_bad"),
		one("unused_uri_bad", r, "localhost", "leaf_ok", "u_uri_bad"),
		one("unused_uri_baddomain", r, "localhost", "leaf_ok", "u_uri_baddomain"),
		one("unused_nc_badmask", r, "localhost", "leaf_ok", "u_nc_badmask"),
		one("leaf_x25519", r, "localhost", "u_x25519"),
		one("leaf_unknown_key", r, "localhost", "u_unknown_key"),
		one("leaf_rsa_big", r, "localhost", "u_rsa_big"),
		one("pool_badnc", []string{"root_badnc"}, "localhost", "l_root_badnc"),
		one("pool_baduri", []string{"root_baduri"}, "localhost", "l_root_baduri"),
		one("pool_rsa_noparams", []string{"root_rsa_noparams"}, "localhost", "l_sig_rsa2048"),

		one("host_trailing", r, "localhost.", "l_host_trailing"),
		one("host_trailing_nodot", r, "localhost", "l_host_trailing"),
		one("host_star", r, "*.example.com", "l_host_star"),
		one("host_partial", r, "ab.example.com", "l_host_partial"),
		one("host_empty", r, "localhost", "l_host_empty"),
		one("host_ipdns", r, "127.0.0.1", "l_host_ipdns"),
		one("host_upper", r, "localhost", "l_host_upper"),
		one("host_v6_brackets", r, "[::1]", "l_host_v6"),
		one("host_v6_brackets_bad", r, "[::2]", "l_host_v6"),
		one("host_mapped", r, "127.0.0.1", "l_host_mapped"),
		one("host_mapped_host", r, "::ffff:127.0.0.1", "leaf_ok"),
		one("host_many", r, "localhost", "l_host_many"),
		one("host_many_ip", r, "10.9.9.9", "l_host_many_ip"),
		one("host_dot_only", r, ".", "leaf_ok"),
		one("host_double_dot", r, "localhost..", "l_host_trailing"),

		one("cross_both", []string{"root_old", "root"}, "localhost", "l_x", "x_inter_old", "x_inter_new"),
		one("cross_old_only", []string{"root_old"}, "localhost", "l_x", "x_inter_old"),
		one("cross_new_pool_old_chain", []string{"root"}, "localhost", "l_x", "x_inter_old"),
		one("loop", []string{}, "localhost", "l_loop", "loop_b", "loop_a2"),
		one("twin_akid", []string{"root_twin_bad", "root_twin_good"}, "localhost", "l_twin"),
		one("twin_expired_first", []string{"root_twin_other", "root_twin_expired"}, "localhost", "l_twin2"),
		one("deep7", r, "localhost", "l_deep", "deep7", "deep6", "deep5", "deep4", "deep3", "deep2", "deep1"),
		one("pinned_with_chain", []string{"leaf_ok"}, "localhost", "leaf_ok", "deep1"),

		one("pss_sha256", []string{"root_rsa"}, "localhost", "l_pss_sha256"),
		one("pss_sha384", []string{"root_rsa"}, "localhost", "l_pss_sha384"),
		one("pss_sha512", []string{"root_rsa"}, "localhost", "l_pss_sha512"),
		one("pss_ec_parent", r, "localhost", "l_pss_ec_parent"),

		one("sig_rsa_short", []string{"root_rsa"}, "localhost", "l_sig_rsa_short"),
		one("sig_rsa_long", []string{"root_rsa"}, "localhost", "l_sig_rsa_long"),
		one("sig_ec_nonminimal", r, "localhost", "l_sig_ec_nonminimal"),
		one("sig_ec_trailing", r, "localhost", "l_sig_ec_trailing"),
		one("sig_ec_extra_int", r, "localhost", "l_sig_ec_extra_int"),
		one("sig_ed_short", []string{"root_ed"}, "localhost", "l_sig_ed_short"),
		one("hs_policies", r, "localhost", "l_hs_policies"),
	}
	for _, n := range []string{
		"wrapper_long", "wrapper_short", "wrapper_spill", "wrapper_primitive",
		"trailer_one", "trailer_two", "trailer_null_last", "trailer_nulls", "trailer_empty",
		"trailer_octets", "trailer_primitive", "trailer_high_tag", "trailer_nonminimal_tag",
		"trailer_long_form", "trailer_extra", "salt_20", "hash_params", "hash_extra", "mgf_sha384",
		"no_salt",
	} {
		cases = append(cases, one("pss_"+n, []string{"root_rsa"}, "localhost", "l_pss_"+n))
	}
	for _, n := range []string{"san_permitted", "san_excluded", "net_permitted", "both", "net_excluded"} {
		cases = append(cases, one("nc_mapped_"+n, r, "localhost", "l_nc_mapped_"+n, "nc_mapped_"+n))
	}
	return append(cases,
		one("limit_99_bad", limitRoots(99), "localhost", "l_lim"),
		one("limit_100_bad", limitRoots(100), "localhost", "l_lim"),
		one("limit_all_bad", limitRoots(100)[:100], "localhost", "l_lim"),
		one("limit_99_bad_without_good", limitRoots(99)[:99], "localhost", "l_lim"),
	)
}
