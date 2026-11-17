package gen

import "encoding/json"

// Privacy steps write each signal in its OpenRTB 2.6 location (regs.gdpr,
// regs.us_privacy, regs.gpp, user.consent, user.eids, source.schain) or its 2.5
// extension location (regs.ext.*, user.ext.*, source.ext.schain), and some
// write both with conflicting values. They edit the auction body, the AMP
// stored request or the video body, which all carry regs, user and device.

// privacyStep builds a privacy step that edits d.req().
func privacyStep(name string, v Validity, eps endpointSet, doc string, edit func(d *draft, r *RNG, req obj)) *mutator {
	when := hasReq
	if eps == onORTB {
		when = hasORTB
	}
	return feat(name, v, eps, doc, when, func(d *draft, r *RNG) error {
		edit(d, r, d.req())
		return nil
	})
}

func privacyMutators() []*mutator {
	return []*mutator{
		privacyStep("privacy.gdpr", Preserves, onAll, "sets regs.gdpr=1 and user.consent (2.6)", func(d *draft, r *RNG, req obj) {
			path(req, "regs")["gdpr"] = num(1)
			path(req, "user")["consent"] = Pick(r, d.g.pools.Privacy.TCFValid)
			eeaGeo(d, r, req)
		}),
		privacyStep("privacy.gdpr_25", Preserves, onAll, "sets regs.ext.gdpr=1 and user.ext.consent (2.5)", func(d *draft, r *RNG, req obj) {
			path(req, "regs.ext")["gdpr"] = num(1)
			path(req, "user.ext")["consent"] = Pick(r, d.g.pools.Privacy.TCFValid)
			eeaGeo(d, r, req)
		}),
		privacyStep("privacy.gdpr_off", Preserves, onAll, "sets regs.gdpr=0", func(_ *draft, _ *RNG, req obj) { path(req, "regs")["gdpr"] = num(0) }),
		privacyStep("privacy.gdpr_conflict", Uncertain, onAll, "sets different gdpr and consent values in the 2.5 and 2.6 locations", func(d *draft, r *RNG, req obj) {
			path(req, "regs")["gdpr"] = num(1)
			path(req, "regs.ext")["gdpr"] = num(0)
			consents := Sample(r, d.g.pools.Privacy.TCFValid, 2)
			path(req, "user")["consent"] = consents[0]
			path(req, "user.ext")["consent"] = consents[len(consents)-1]
		}),
		privacyStep("privacy.gdpr_invalid_consent", Uncertain, onAll, "sets regs.gdpr=1 with an invalid consent string", func(d *draft, r *RNG, req obj) {
			path(req, "regs")["gdpr"] = num(1)
			path(req, "user")["consent"] = Pick(r, d.g.pools.Privacy.TCFInvalid)
		}),
		privacyStep("privacy.gdpr_tcf1", Uncertain, onAll, "sets regs.gdpr=1 with a TCF v1 consent string", func(d *draft, r *RNG, req obj) {
			path(req, "regs")["gdpr"] = num(1)
			path(req, "user")["consent"] = Pick(r, d.g.pools.Privacy.TCFv1)
		}),
		privacyStep("privacy.gdpr_bad_value", Invalidates, onORTB, "sets regs.ext.gdpr=2", func(_ *draft, _ *RNG, req obj) { path(req, "regs.ext")["gdpr"] = num(2) }).rare(),
		privacyStep("privacy.gdpr_default", Preserves, onAll, "leaves gdpr unset and lets the host default and EEA countries decide", func(d *draft, r *RNG, req obj) {
			enabled := true
			d.profile.GDPREnabled = &enabled
			d.profile.GDPRDefaultValue = Pick(r, []string{"0", "1"})
			if r.Chance(0.5) {
				d.profile.GDPREEACountries = Sample(r, d.g.pools.ORTB.EEACountries, 3)
			}
			eeaGeo(d, r, req)
		}),
		privacyStep("privacy.ccpa", Preserves, onAll, "sets regs.us_privacy (2.6)", func(d *draft, r *RNG, req obj) {
			path(req, "regs")["us_privacy"] = Pick(r, d.g.pools.Privacy.USPrivacyValid)
		}),
		privacyStep("privacy.ccpa_25", Preserves, onAll, "sets regs.ext.us_privacy (2.5)", func(d *draft, r *RNG, req obj) {
			path(req, "regs.ext")["us_privacy"] = Pick(r, d.g.pools.Privacy.USPrivacyValid)
		}),
		privacyStep("privacy.ccpa_invalid", Uncertain, onAll, "sets an invalid us_privacy string", func(d *draft, r *RNG, req obj) {
			path(req, "regs")["us_privacy"] = Pick(r, d.g.pools.Privacy.USPrivacyInvalid)
		}),
		privacyStep("privacy.ccpa_nosale", Preserves, onORTB, "opts out of sale, enforces CCPA and lists ext.prebid.nosale", ccpaNoSale),
		privacyStep("privacy.gpp", Preserves, onAll, "sets regs.gpp and a matching regs.gpp_sid", func(d *draft, r *RNG, req obj) {
			setGPP(req, Pick(r, gppEntries(d, true)), nil)
		}),
		privacyStep("privacy.gpp_invalid", Uncertain, onAll, "sets a GPP string with an invalid header", func(d *draft, r *RNG, req obj) {
			setGPP(req, Pick(r, gppEntries(d, false)), nil)
		}),
		privacyStep("privacy.gpp_sid_mismatch", Uncertain, onAll, "sets regs.gpp_sid to sections the string does not carry", func(d *draft, r *RNG, req obj) {
			setGPP(req, Pick(r, gppEntries(d, true)), []int{7, 8})
		}),
		privacyStep("privacy.gpp_conflict", Uncertain, onAll, "sets a TCF GPP section with a conflicting regs.gdpr and user.consent", func(d *draft, r *RNG, req obj) {
			var tcf []GPPEntry
			for _, e := range gppEntries(d, true) {
				if containsInt(e.SIDs, 2) {
					tcf = append(tcf, e)
				}
			}
			setGPP(req, Pick(r, tcf), nil)
			path(req, "regs")["gdpr"] = num(0)
			path(req, "user")["consent"] = Pick(r, d.g.pools.Privacy.TCFValid)
		}),
		privacyStep("privacy.coppa", Preserves, onAll, "sets regs.coppa=1", func(_ *draft, _ *RNG, req obj) { path(req, "regs")["coppa"] = num(1) }),
		privacyStep("privacy.lmt", Preserves, onAll, "sets device.lmt=1, sometimes enforcing LMT", func(d *draft, r *RNG, req obj) {
			path(req, "device")["lmt"] = num(1)
			if r.Chance(0.5) {
				enforce := true
				d.profile.LMTEnforce = &enforce
			}
		}),
		privacyStep("privacy.dnt", Preserves, onAll, "sets device.dnt=1", func(_ *draft, _ *RNG, req obj) { path(req, "device")["dnt"] = num(1) }),
		privacyStep("privacy.gpc", Preserves, onAll, "sets regs.ext.gpc and/or the Sec-GPC header", func(d *draft, r *RNG, req obj) {
			if r.Chance(0.6) {
				path(req, "regs.ext")["gpc"] = "1"
			}
			if r.Chance(0.6) {
				d.setHeader("Sec-GPC", "1")
			}
		}),
		privacyStep("privacy.eids", Preserves, onAll, "sets user.eids (2.6)", func(d *draft, r *RNG, req obj) { path(req, "user")["eids"] = eids(d, r) }),
		privacyStep("privacy.eids_25", Preserves, onAll, "sets user.ext.eids (2.5)", func(d *draft, r *RNG, req obj) { path(req, "user.ext")["eids"] = eids(d, r) }),
		privacyStep("privacy.eid_permissions", Preserves, onORTB, "sets user.eids and ext.prebid.data.eidpermissions", eidPermissions),
		privacyStep("privacy.eid_permissions_bad", Invalidates, onORTB, "sets an eidpermissions entry without source", func(_ *draft, _ *RNG, req obj) {
			path(req, "ext.prebid.data")["eidpermissions"] = []any{obj{"bidders": strList("*")}}
		}).rare(),
		privacyStep("privacy.schain", Preserves, onORTB, "sets source.schain (2.6)", func(d *draft, r *RNG, req obj) { path(req, "source")["schain"] = schain(d, r) }),
		privacyStep("privacy.schain_25", Preserves, onORTB, "sets source.ext.schain (2.5)", func(d *draft, r *RNG, req obj) { path(req, "source.ext")["schain"] = schain(d, r) }),
		privacyStep("privacy.schains_bidder", Preserves, onORTB, "sets bidder-specific ext.prebid.schains", func(d *draft, r *RNG, req obj) {
			bidders := strList("*")
			if names := requestBidders(req); len(names) > 0 && r.Chance(0.6) {
				bidders = strList(Pick(r, names))
			}
			prebid(req)["schains"] = []any{obj{"bidders": bidders, "schain": schain(d, r)}}
		}),
		privacyStep("privacy.host_schain", Preserves, onAll, "configures a host schain node", func(d *draft, _ *RNG, _ obj) {
			d.profile.HostSChainNode = json.RawMessage(`{"asi":"pbshost.parity.test","sid":"host-1","hp":1}`)
		}),
		privacyStep("privacy.geo_precise", Preserves, onAll, "sets precise device and user geo plus both IP versions", func(d *draft, r *RNG, req obj) {
			geo := obj{"lat": fnum(Pick(r, []float64{37.774929, 52.520008})), "lon": fnum(Pick(r, []float64{-122.419418, 13.404954})), "type": num(1), "country": Pick(r, d.g.pools.ORTB.Countries)}
			path(req, "device")["geo"] = geo
			path(req, "user")["geo"] = deepCopy(geo)
			dev := path(req, "device")
			dev["ip"], dev["ipv6"] = Pick(r, d.g.pools.ORTB.IPv4), Pick(r, d.g.pools.ORTB.IPv6)
		}),
		privacyStep("privacy.ip_private", Preserves, onAll, "sets a private device.ip", func(d *draft, r *RNG, req obj) {
			path(req, "device")["ip"] = Pick(r, d.g.pools.ORTB.PrivateIPv4)
		}),
		privacyStep("privacy.user_ids", Preserves, onAll, "sets user.id, user.buyeruid and device ids", func(_ *draft, r *RNG, req obj) {
			user := path(req, "user")
			user["id"], user["buyeruid"] = "user-"+r.Hex(8), "buyer-"+r.Hex(8)
			dev := path(req, "device")
			dev["ifa"], dev["didsha1"], dev["macsha1"] = r.UUID(), r.Hex(40), r.Hex(40)
		}),
		privacyStep("privacy.consented_providers", Preserves, onAll, "sets Google additional consent in user.ext (legacy string or parsed list)", func(_ *draft, r *RNG, req obj) {
			if r.Chance(0.5) {
				path(req, "user.ext.ConsentedProvidersSettings")["consented_providers"] = "1~7.12.35"
				return
			}
			path(req, "user.ext.consented_providers_settings")["consented_providers"] = intList(7, 12, 35)
		}),
	}
}

// eeaGeo sometimes places the device in an EEA country.
func eeaGeo(d *draft, r *RNG, req obj) {
	if r.Chance(0.5) {
		path(req, "device.geo")["country"] = Pick(r, d.g.pools.ORTB.EEACountries)
	}
}

func ccpaNoSale(d *draft, r *RNG, req obj) {
	path(req, "regs")["us_privacy"] = "1YYY"
	d.profile.CCPAEnforce = true
	nosale := strList("*")
	if names := requestBidders(req); len(names) > 0 && r.Chance(0.6) {
		nosale = strList(Pick(r, names))
	}
	prebid(req)["nosale"] = nosale
}

// gppEntries returns the pool entries whose header validity is headerValid.
func gppEntries(d *draft, headerValid bool) []GPPEntry {
	var out []GPPEntry
	for _, e := range d.g.pools.Privacy.GPP {
		if e.HeaderValid == headerValid {
			out = append(out, e)
		}
	}
	return out
}

// setGPP sets regs.gpp and regs.gpp_sid (the entry's sections unless sids is
// given).
func setGPP(req obj, e GPPEntry, sids []int) {
	regs := path(req, "regs")
	regs["gpp"] = e.GPP
	if sids == nil {
		sids = e.SIDs
	}
	if len(sids) > 0 {
		regs["gpp_sid"] = intList(sids...)
	}
}

func containsInt(xs []int, x int) bool {
	for _, v := range xs {
		if v == x {
			return true
		}
	}
	return false
}

// eids builds one or two extended IDs from the pool sources.
func eids(d *draft, r *RNG) []any {
	var out []any
	for i, src := range Sample(r, d.g.pools.ORTB.EIDSources, r.Range(1, 2)) {
		uid := obj{"id": "eid-" + r.Hex(10), "atype": num(int64(r.Range(1, 3)))}
		out = append(out, obj{"source": src, "uids": []any{uid}})
		if i == 0 && r.Chance(0.3) {
			uid["ext"] = obj{"stype": "ppuid"}
		}
	}
	return out
}

func eidPermissions(d *draft, r *RNG, req obj) {
	list := eids(d, r)
	path(req, "user")["eids"] = list
	src := ""
	if len(list) > 0 {
		first, _ := list[0].(obj)
		src, _ = first["source"].(string)
	}
	bidders := strList("*")
	if names := requestBidders(req); len(names) > 0 && r.Chance(0.7) {
		bidders = strList(Pick(r, names))
	}
	path(req, "ext.prebid.data")["eidpermissions"] = []any{obj{"source": src, "bidders": bidders}}
}

// schain builds a supply chain from the pool nodes.
func schain(d *draft, r *RNG) obj {
	var nodes []any
	for _, raw := range Sample(r, d.g.pools.ORTB.SChainNodes, r.Range(1, len(d.g.pools.ORTB.SChainNodes))) {
		if n, err := decodeTree(raw); err == nil {
			nodes = append(nodes, n)
		}
	}
	return obj{"complete": num(int64(r.Range(0, 1))), "ver": "1.0", "nodes": nodes}
}
