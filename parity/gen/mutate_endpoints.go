package gen

import (
	"strconv"
	"strings"
)

// hasVideoBody reports whether d is a video case with a JSON body.
func hasVideoBody(d *draft) bool { return d.isVideoEndpoint() && d.req() != nil }

// videoPodList returns podconfig.pods as objects.
func videoPodList(req obj) []obj {
	pods, _ := lookup(req, "podconfig.pods")
	list, _ := pods.([]any)
	var out []obj
	for _, p := range list {
		if o, ok := p.(obj); ok {
			out = append(out, o)
		}
	}
	return out
}

func hasPods(d *draft) bool { return hasVideoBody(d) && len(videoPodList(d.req())) > 0 }

func videoStep(name string, v Validity, doc string, when func(*draft) bool, edit func(d *draft, r *RNG, body obj)) *mutator {
	return feat(name, v, onVideo, doc, when, func(d *draft, r *RNG) error {
		edit(d, r, d.req())
		return nil
	})
}

func videoMutators() []*mutator {
	return []*mutator{
		videoStep("video.exact_duration", Preserves, "sets podconfig.requireexactduration", hasVideoBody, func(_ *draft, _ *RNG, b obj) {
			path(b, "podconfig")["requireexactduration"] = true
		}),
		videoStep("video.add_pod", Preserves, "adds a pod reusing an existing configid", hasPods, addPod),
		videoStep("video.brand_category", Preserves, "sets includebrandcategory (freewheel mapping)", hasVideoBody, func(_ *draft, r *RNG, b obj) {
			b["includebrandcategory"] = obj{"primaryadserver": num(1), "publisher": "", "translatecategories": r.Chance(0.5)}
		}),
		videoStep("video.price_granularity", Preserves, "sets a custom video price granularity", hasVideoBody, func(_ *draft, _ *RNG, b obj) {
			b["pricegranularity"] = customGranularity(2)
		}),
		videoStep("video.cache_ttl", Preserves, "sets cacheconfig.ttl", hasVideoBody, func(_ *draft, r *RNG, b obj) {
			b["cacheconfig"] = obj{"ttl": num(int64(Pick(r, []int{60, 300, 3600})))}
		}),
		videoStep("video.inline_site", Preserves, "inlines the stored video request and drops storedrequestid", hasStoredVideoRequest, inlineVideoStored),
		videoStep("video.deals", Preserves, "sets supportdeals and appendbiddernames", hasVideoBody, func(_ *draft, r *RNG, b obj) {
			b["supportdeals"] = true
			b["appendbiddernames"] = r.Chance(0.5)
		}),
		videoStep("video.content", Preserves, "sets content", hasVideoBody, func(_ *draft, r *RNG, b obj) {
			b["content"] = obj{"episode": num(int64(r.Range(1, 12))), "title": "Parity Episode", "series": "Parity", "len": num(900), "livestream": num(0)}
		}),
		videoStep("video.no_pods", Invalidates, "empties podconfig.pods", hasVideoBody, func(_ *draft, _ *RNG, b obj) {
			path(b, "podconfig")["pods"] = []any{}
		}).rare(),
		videoStep("video.zero_duration", Invalidates, "puts 0 into durationrangesec", hasVideoBody, func(_ *draft, _ *RNG, b obj) {
			path(b, "podconfig")["durationrangesec"] = intList(0, 15)
		}).rare(),
		videoStep("video.no_durations", Invalidates, "removes durationrangesec", hasVideoBody, func(_ *draft, _ *RNG, b obj) {
			delete(path(b, "podconfig"), "durationrangesec")
		}).rare(),
		videoStep("video.dup_pod_id", Uncertain, "gives two pods the same podid", hasPods, func(_ *draft, r *RNG, b obj) {
			appendPod(b, deepCopy(Pick(r, videoPodList(b))))
		}),
		videoStep("video.missing_config", Uncertain, "points one pod at a stored imp that does not exist", hasPods, func(_ *draft, r *RNG, b obj) {
			Pick(r, videoPodList(b))["configid"] = missingStoredID
		}),
		videoStep("video.negative_pod_duration", Uncertain, "sets a negative adpoddurationsec", hasPods, func(_ *draft, r *RNG, b obj) {
			Pick(r, videoPodList(b))["adpoddurationsec"] = num(-30)
		}),
		videoStep("video.unknown_stored", Invalidates, "references a stored video request that does not exist", hasVideoBody, func(_ *draft, _ *RNG, b obj) {
			b["storedrequestid"] = missingStoredID
		}).rare(),
		videoStep("video.no_stored", Invalidates, "drops storedrequestid while site and app live only in the stored request", func(d *draft) bool {
			return hasVideoBody(d) && d.req()[channelSite] == nil && d.req()[channelApp] == nil
		}, func(_ *draft, _ *RNG, b obj) { delete(b, "storedrequestid") }).rare(),
	}
}

func addPod(_ *draft, r *RNG, b obj) {
	pods := videoPodList(b)
	maxID := 0
	for _, p := range pods {
		if id, ok := intOf(p["podid"]); ok && id > maxID {
			maxID = id
		}
	}
	pod := obj{
		"podid":            num(int64(maxID + 1)),
		"adpoddurationsec": num(int64(Pick(r, []int{30, 60}))),
		"configid":         Pick(r, pods)["configid"],
	}
	appendPod(b, pod)
}

// appendPod appends a pod to podconfig.pods.
func appendPod(b obj, pod any) {
	cfg := path(b, "podconfig")
	list, _ := cfg["pods"].([]any)
	cfg["pods"] = append(list, pod)
}

func hasStoredVideoRequest(d *draft) bool {
	if !hasVideoBody(d) {
		return false
	}
	id, _ := d.req()["storedrequestid"].(string)
	_, ok := d.stored.Requests[id]
	return ok
}

func inlineVideoStored(d *draft, _ *RNG, b obj) {
	id, _ := b["storedrequestid"].(string)
	stored, err := decodeObject(d.stored.Requests[id])
	if err != nil {
		d.note("video.inline_site: stored request %s is not an object", id)
		return
	}
	for k, v := range stored {
		if _, ok := b[k]; !ok {
			b[k] = v
		}
	}
	delete(b, "storedrequestid")
}

func hasAMP(d *draft) bool { return d.amp != nil }

// ampQuery builds an AMP step that sets query parameters.
func ampQuery(name string, v Validity, doc string, params func(d *draft, r *RNG) [][2]string) *mutator {
	return feat(name, v, onAMP, doc, nil, func(d *draft, r *RNG) error {
		for _, kv := range params(d, r) {
			d.setQuery(kv[0], kv[1])
		}
		return nil
	})
}

func ampMutators() []*mutator {
	return []*mutator{
		ampQuery("amp.size", Preserves, "sets w/h, ow/oh or ms", ampSize),
		ampQuery("amp.consent", Preserves, "sends a TCF v2 consent string with gdpr_applies", func(d *draft, r *RNG) [][2]string {
			key := "consent_string"
			if r.Chance(0.3) {
				key = "gdpr_consent"
			}
			return [][2]string{{key, Pick(r, d.g.pools.Privacy.TCFValid)}, {"gdpr_applies", "true"}, {"consent_type", "2"}}
		}),
		ampQuery("amp.ccpa", Preserves, "sends a US privacy consent string", func(d *draft, r *RNG) [][2]string {
			return [][2]string{{"consent_string", Pick(r, d.g.pools.Privacy.USPrivacyValid)}, {"consent_type", "3"}}
		}),
		ampQuery("amp.addtl_consent", Preserves, "sends addtl_consent", func(*draft, *RNG) [][2]string {
			return [][2]string{{"addtl_consent", "1~7.12.35.62.66.70.89.93.108"}}
		}),
		ampQuery("amp.targeting", Preserves, "sends targeting JSON", func(_ *draft, r *RNG) [][2]string {
			return [][2]string{{"targeting", `{"pos":"` + Pick(r, []string{"top", "bottom"}) + `","kw":["a","b"]}`}}
		}),
		ampQuery("amp.targeting_bad", Uncertain, "sends targeting that is not JSON", func(*draft, *RNG) [][2]string {
			return [][2]string{{"targeting", "{not json"}}
		}).rare(),
		ampQuery("amp.curl", Preserves, "sets curl (canonical URL)", func(d *draft, r *RNG) [][2]string {
			return [][2]string{{"curl", Pick(r, d.g.pools.ORTB.Sites).Page}}
		}),
		ampQuery("amp.account", Preserves, "sets the account parameter", func(*draft, *RNG) [][2]string {
			return [][2]string{{"account", "parity-amp-account"}}
		}),
		ampQuery("amp.timeout", Preserves, "sets timeout", func(_ *draft, r *RNG) [][2]string {
			return [][2]string{{"timeout", strconv.Itoa(Pick(r, []int{500, 1000, 3000}))}}
		}),
		ampQuery("amp.debug", Preserves, "sets debug=1", func(*draft, *RNG) [][2]string { return [][2]string{{"debug", "1"}} }),
		ampQuery("amp.slot", Preserves, "sets slot", func(*draft, *RNG) [][2]string { return [][2]string{{"slot", "/1234/parity-slot"}} }),
		ampQuery("amp.trace", Preserves, "sets trace", func(_ *draft, r *RNG) [][2]string {
			return [][2]string{{"trace", Pick(r, []string{"verbose", "basic"})}}
		}),
		ampQuery("amp.source_origin", Preserves, "sets __amp_source_origin", func(*draft, *RNG) [][2]string {
			return [][2]string{{"__amp_source_origin", "https://amp.parity.test"}}
		}),
		ampQuery("amp.query_junk", Preserves, "adds unknown query parameters", func(_ *draft, r *RNG) [][2]string {
			return [][2]string{{"utm_source", "parity"}, {"pg_" + r.Hex(3), "1"}}
		}),
		ampQuery("amp.consent_type_bad", Uncertain, "sends an unknown consent_type", func(d *draft, r *RNG) [][2]string {
			return [][2]string{{"consent_string", Pick(r, d.g.pools.Privacy.TCFValid)}, {"consent_type", "99"}}
		}),
		ampQuery("amp.gdpr_applies_bad", Uncertain, "sends a non-boolean gdpr_applies", func(*draft, *RNG) [][2]string {
			return [][2]string{{"gdpr_applies", "maybe"}}
		}),
		feat("amp.no_tag", Invalidates, onAMP, "drops tag_id", nil, func(d *draft, _ *RNG) error { d.dropQuery("tag_id"); return nil }).rare(),
		feat("amp.empty_tag", Invalidates, onAMP, "sends an empty tag_id", nil, func(d *draft, _ *RNG) error {
			d.ampTagAuto = false
			d.setQuery("tag_id", "")
			return nil
		}).rare(),
		feat("amp.unknown_tag", Invalidates, onAMP, "references a stored request that does not exist", nil, func(d *draft, _ *RNG) error {
			d.ampTagAuto = false
			d.setQuery("tag_id", missingStoredID)
			return nil
		}).rare(),
		feat("amp.dup_tag", Uncertain, onAMP, "repeats tag_id with a second value", nil, func(d *draft, _ *RNG) error {
			d.query = append(d.query, queryParam{key: "tag_id", value: missingStoredID, hasValue: true})
			return nil
		}),
		feat("amp.multi_imp", Invalidates, onAMP, "stores an AMP request with two imps", func(d *draft) bool { return hasAMP(d) && len(impsOf(d.amp)) == 1 }, func(d *draft, _ *RNG) error {
			list, _ := d.amp["imp"].([]any)
			second, _ := deepCopy(impsOf(d.amp)[0]).(obj)
			second["id"] = "parity-gen-second"
			d.amp["imp"] = append(list, second)
			return nil
		}).rare(),
	}
}

func ampSize(d *draft, r *RNG) [][2]string {
	size := Pick(r, d.g.pools.ORTB.BannerSizes)
	w, h := strconv.Itoa(size[0]), strconv.Itoa(size[1])
	switch r.Intn(3) {
	case 0:
		return [][2]string{{"w", w}, {"h", h}}
	case 1:
		return [][2]string{{"ow", w}, {"oh", h}}
	}
	sizes := Sample(r, d.g.pools.ORTB.BannerSizes, 2)
	parts := make([]string, len(sizes))
	for i, s := range sizes {
		parts[i] = strconv.Itoa(s[0]) + "x" + strconv.Itoa(s[1])
	}
	return [][2]string{{"ms", strings.Join(parts, ",")}}
}
