//! Go: `openrtb_ext/request_test.go`.

use std::collections::{BTreeMap, BTreeSet};

use pbs_gojson::RawJson;
use pbs_openrtb::openrtb2::{SupplyChain, SupplyChainNode};
use pbs_openrtb_ext::openrtb_ext::{
    AdServerTarget, AdsCert, BidderConfig, Config, Experiment, ExtAdapterAlternateBidderCodes,
    ExtAlternateBidderCodes, ExtIncludeBrandCategory, ExtMultiBid, ExtRequestCurrency,
    ExtRequestPrebid, ExtRequestPrebidCache, ExtRequestPrebidCacheBids, ExtRequestPrebidCacheVAST,
    ExtRequestPrebidChannel, ExtRequestPrebidData, ExtRequestPrebidDataEidPermission,
    ExtRequestPrebidSChain, ExtRequestPrebidServer, ExtRequestTargeting, ExtStoredRequest,
    GranularityRange, ORTB2, PriceFloorData, PriceFloorEndpoint, PriceFloorEnforcement,
    PriceFloorModelGroup, PriceFloorRules, PriceFloorSchema, PriceGranularity,
};

use super::support::{TestResult, all_ok, ensure_eq, unmarshal_error};

fn range(min: f64, max: f64, increment: f64) -> GranularityRange {
    GranularityRange {
        min,
        max,
        increment,
    }
}

fn granularity(
    precision: Option<isize>,
    ranges: Option<Vec<GranularityRange>>,
) -> PriceGranularity {
    PriceGranularity { precision, ranges }
}

/// Go: `validGranularityTests`.
fn valid_granularity_tests() -> Vec<(&'static str, PriceGranularity)> {
    vec![
        (
            concat!(
                r#"{"precision": 4, "ranges": [{"min": 0, "max": 5, "increment": 0.1}, {"min": "#,
                r#"5, "max":10, "increment":0.5}, {"min":10, "max":20, "increment":1}]}"#,
            ),
            granularity(
                Some(4),
                Some(vec![
                    range(0.0, 5.0, 0.1),
                    range(5.0, 10.0, 0.5),
                    range(10.0, 20.0, 1.0),
                ]),
            ),
        ),
        (
            concat!(
                r#"{"ranges":[{ "max":5, "increment": 0.05}, {"max": 10, "increment": 0.25}, "#,
                r#"{"max": 20, "increment": 0.5}]}"#,
            ),
            granularity(
                None,
                Some(vec![
                    range(0.0, 5.0, 0.05),
                    range(0.0, 10.0, 0.25),
                    range(0.0, 20.0, 0.5),
                ]),
            ),
        ),
        (
            r#""medium""#,
            granularity(Some(2), Some(vec![range(0.0, 20.0, 0.1)])),
        ),
        (
            r#"{ "precision": 3, "ranges": [{"max":20, "increment":0.005}]}"#,
            granularity(Some(3), Some(vec![range(0.0, 20.0, 0.005)])),
        ),
        (
            concat!(
                r#"{"precision": 0, "ranges": [{"max":5, "increment": 1}, {"max": 10, "#,
                r#""increment": 2}, {"max": 20, "increment": 5}]}"#,
            ),
            granularity(
                Some(0),
                Some(vec![
                    range(0.0, 5.0, 1.0),
                    range(0.0, 10.0, 2.0),
                    range(0.0, 20.0, 5.0),
                ]),
            ),
        ),
        (
            concat!(
                r#"{"precision": 2, "ranges": [{"min": 0.5, "max":5, "increment": 0.1}, {"min": "#,
                r#"54, "max": 10, "increment": 1}, {"min": -42, "max": 20, "increment": 5}]}"#,
            ),
            granularity(
                Some(2),
                Some(vec![
                    range(0.5, 5.0, 0.1),
                    range(54.0, 10.0, 1.0),
                    range(-42.0, 20.0, 5.0),
                ]),
            ),
        ),
        ("{}", granularity(None, None)),
        (r#"{"precision": 2}"#, granularity(Some(2), None)),
        (
            r#"{"precision": 2, "ranges":[]}"#,
            granularity(Some(2), Some(Vec::new())),
        ),
    ]
}

/// Go's `fmt.Sprintf("%2.2f-%2.2f-%2.2f", r.Min, r.Max, r.Increment)` of each range.
fn range_keys(ranges: Option<&[GranularityRange]>) -> Vec<String> {
    ranges
        .unwrap_or_default()
        .iter()
        .map(|r| format!("{:2.2}-{:2.2}-{:2.2}", r.min, r.max, r.increment))
        .collect()
}

/// Go: `TestGranularityUnmarshal`: the precision, and the ranges (in any order).
#[test]
fn granularity_unmarshal() -> TestResult {
    let groups = [
        ("Unmarshal without error", valid_granularity_tests(), false),
        (
            "Malformed json. Expect unmarshall error",
            vec![("[]", PriceGranularity::default())],
            true,
        ),
    ];
    let mut results = Vec::new();
    for (desc, cases, expect_error) in groups {
        for (i, (json, target)) in cases.into_iter().enumerate() {
            let mut resolved = PriceGranularity::default();
            let err = unmarshal_error(json, &mut resolved);
            let result = if expect_error && err.is_none() {
                Err("no error".to_owned())
            } else {
                let want: BTreeSet<String> =
                    range_keys(target.ranges.as_deref()).into_iter().collect();
                let got = range_keys(resolved.ranges.as_deref());
                ensure_eq(&resolved.precision, &target.precision, "Precision")
                    .and_then(|()| ensure_eq(&got.len(), &want.len(), "len(Ranges)"))
                    .and_then(|()| {
                        got.iter().try_for_each(|key| {
                            ensure_eq(&want.contains(key), &true, &format!("target.range {key}"))
                        })
                    })
            };
            results.push((format!("{desc} test case {i}"), result));
        }
    }
    all_ok(results)
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|&s| s.to_owned()).collect()
}

fn raw(json: &str) -> RawJson {
    RawJson::from_slice(json.as_bytes())
}

fn general() -> ExtRequestPrebid {
    ExtRequestPrebid {
        aliases: Some(BTreeMap::from([(
            "alias1".to_owned(),
            "bidder1".to_owned(),
        )])),
        bid_adjustment_factors: Some(BTreeMap::from([("bidder5".to_owned(), 1.2)])),
        bidder_params: raw("{}"),
        channel: Some(Box::new(ExtRequestPrebidChannel {
            name: "ABC".to_owned(),
            version: "1.0".to_owned(),
        })),
        debug: true,
        experiment: Some(Box::new(Experiment {
            ads_cert: Some(Box::new(AdsCert { enabled: false })),
        })),
        server: Some(Box::new(ExtRequestPrebidServer {
            external_url: "http://www.example.com".to_owned(),
            gvl_id: 5,
            data_center: "Universe Central".to_owned(),
        })),
        support_deals: true,
        ..ExtRequestPrebid::default()
    }
}

fn mutate_general(prebid: &mut ExtRequestPrebid) {
    let aliases = prebid.aliases.get_or_insert_with(BTreeMap::new);
    aliases.extend([
        ("alias2".to_owned(), "bidder52".to_owned()),
        ("alias1".to_owned(), "some other".to_owned()),
    ]);
    prebid.alias_gvlids = Some(BTreeMap::from([("alias2".to_owned(), 42)]));
    let factors = prebid
        .bid_adjustment_factors
        .get_or_insert_with(BTreeMap::new);
    factors.extend([("alias2".to_owned(), 1.1)]);
    let _removed = factors.remove("bidder5");
    prebid.bidder_params = raw(r#"{"someJSON": true}"#);
    prebid.channel = None;
    prebid.data = Some(Box::new(ExtRequestPrebidData {
        eid_permissions: Some(vec![ExtRequestPrebidDataEidPermission {
            source: "mySource".to_owned(),
            bidders: Some(strings(&["sauceBidder"])),
        }]),
        ..ExtRequestPrebidData::default()
    }));
    prebid.events = raw("{}");
    if let Some(server) = prebid.server.as_deref_mut() {
        server.gvl_id = 7;
    }
    prebid.support_deals = false;
}

fn bidder_config(bidders: &[&str], ortb2: ORTB2) -> BidderConfig {
    BidderConfig {
        bidders: Some(strings(bidders)),
        config: Some(Box::new(Config {
            ortb2: Some(Box::new(ortb2)),
        })),
    }
}

fn bidder_configs() -> ExtRequestPrebid {
    ExtRequestPrebid {
        bidder_configs: Some(vec![
            bidder_config(
                &["Bidder1", "bidder2"],
                ORTB2 {
                    site: raw(r#"{"value":"config1"}"#),
                    ..ORTB2::default()
                },
            ),
            bidder_config(
                &["Bidder5", "bidder17"],
                ORTB2 {
                    app: raw(r#"{"value":"config2"}"#),
                    ..ORTB2::default()
                },
            ),
            bidder_config(
                &["foo"],
                ORTB2 {
                    user: raw(r#"{"value":"config3"}"#),
                    ..ORTB2::default()
                },
            ),
            bidder_config(
                &["Bidder9"],
                ORTB2 {
                    device: raw(r#"{"value":"config4"}"#),
                    ..ORTB2::default()
                },
            ),
        ]),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_bidder_configs(prebid: &mut ExtRequestPrebid) {
    let configs = prebid.bidder_configs.get_or_insert_with(Vec::new);
    if let Some(config) = configs.get_mut(0) {
        config
            .bidders
            .get_or_insert_with(Vec::new)
            .push("bidder4".to_owned());
    }
    if let Some(config) = configs.get_mut(1) {
        *config = BidderConfig {
            bidders: Some(strings(&["george"])),
            config: Some(Box::new(Config { ortb2: None })),
        };
    }
    if let Some(ortb2) = configs
        .get_mut(2)
        .and_then(|c| c.config.as_deref_mut())
        .and_then(|c| c.ortb2.as_deref_mut())
    {
        ortb2.user = raw(r#"{"id": 345}"#);
    }
    if let Some(ortb2) = configs
        .get_mut(3)
        .and_then(|c| c.config.as_deref_mut())
        .and_then(|c| c.ortb2.as_deref_mut())
    {
        ortb2.device = raw(r#"{"id": 999}"#);
    }
    configs.push(bidder_config(&["bidder2"], ORTB2::default()));
}

fn cache() -> ExtRequestPrebid {
    ExtRequestPrebid {
        cache: Some(Box::new(ExtRequestPrebidCache {
            bids: Some(Box::new(ExtRequestPrebidCacheBids {
                return_creative: Some(true),
            })),
            vast_xml: Some(Box::new(ExtRequestPrebidCacheVAST {
                return_creative: Some(false),
            })),
        })),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_cache(prebid: &mut ExtRequestPrebid) {
    if let Some(cache) = prebid.cache.as_deref_mut() {
        if let Some(bids) = cache.bids.as_deref_mut() {
            bids.return_creative = Some(false);
        }
        cache.bids = None;
        cache.vast_xml = Some(Box::new(ExtRequestPrebidCacheVAST {
            return_creative: Some(true),
        }));
    }
}

fn currency() -> ExtRequestPrebid {
    ExtRequestPrebid {
        currency_conversions: Some(Box::new(ExtRequestCurrency {
            conversion_rates: Some(BTreeMap::from([(
                "A".to_owned(),
                Some(BTreeMap::from([("X".to_owned(), 5.4)])),
            )])),
            use_pbs_rates: Some(false),
        })),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_currency(prebid: &mut ExtRequestPrebid) {
    if let Some(conversions) = prebid.currency_conversions.as_deref_mut() {
        let rates = conversions
            .conversion_rates
            .get_or_insert_with(BTreeMap::new);
        if let Some(Some(a)) = rates.get_mut("A") {
            a.extend([("X".to_owned(), 3.4)]);
        }
        rates.extend([(
            "B".to_owned(),
            Some(BTreeMap::from([("Y".to_owned(), 0.76)])),
        )]);
        conversions.use_pbs_rates = Some(true);
    }
}

fn data() -> ExtRequestPrebid {
    ExtRequestPrebid {
        data: Some(Box::new(ExtRequestPrebidData {
            eid_permissions: Some(vec![
                ExtRequestPrebidDataEidPermission {
                    source: "Sauce".to_owned(),
                    bidders: Some(strings(&["G", "H"])),
                },
                ExtRequestPrebidDataEidPermission {
                    source: "Black Hole".to_owned(),
                    bidders: Some(strings(&["Q", "P"])),
                },
            ]),
            bidders: Some(strings(&["A", "B", "C"])),
        })),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_data(prebid: &mut ExtRequestPrebid) {
    let Some(data) = prebid.data.as_deref_mut() else {
        return;
    };
    let permissions = data.eid_permissions.get_or_insert_with(Vec::new);
    if let Some(p) = permissions.get_mut(0) {
        p.source = "Fuzzy Bunnies".to_owned();
    }
    if let Some(b) = permissions
        .get_mut(1)
        .and_then(|p| p.bidders.as_mut())
        .and_then(|b| b.get_mut(0))
    {
        "X".clone_into(b);
    }
    if let Some(p) = permissions.get_mut(0) {
        p.bidders.get_or_insert_with(Vec::new).push("R".to_owned());
    }
    permissions.push(ExtRequestPrebidDataEidPermission {
        source: "Harry".to_owned(),
        bidders: None,
    });
    let bidders = data.bidders.get_or_insert_with(Vec::new);
    if let Some(b) = bidders.get_mut(1) {
        "D".clone_into(b);
    }
    bidders.push("E".to_owned());
}

fn multi_bid(
    bidder: &str,
    bidders: Option<Vec<String>>,
    max_bids: Option<isize>,
    prefix: &str,
) -> ExtMultiBid {
    ExtMultiBid {
        bidder: bidder.to_owned(),
        bidders,
        max_bids,
        target_bidder_code_prefix: prefix.to_owned(),
    }
}

fn multibid() -> ExtRequestPrebid {
    ExtRequestPrebid {
        multi_bid: Some(vec![
            Some(Box::new(multi_bid("somebidder", None, Some(3), "SB"))),
            Some(Box::new(multi_bid(
                "",
                Some(strings(&["A", "B", "C"])),
                Some(4),
                "",
            ))),
        ]),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_multibid(prebid: &mut ExtRequestPrebid) {
    let entries = prebid.multi_bid.get_or_insert_with(Vec::new);
    if let Some(Some(entry)) = entries.get_mut(0) {
        entry.max_bids = Some(2);
    }
    if let Some(Some(entry)) = entries.get_mut(1) {
        entry.bidders = Some(strings(&["C", "D", "E", "F"]));
    }
    prebid.multi_bid = Some(vec![Some(Box::new(multi_bid("otherbid", None, None, "")))]);
}

fn schains() -> ExtRequestPrebid {
    ExtRequestPrebid {
        s_chains: Some(vec![Some(Box::new(ExtRequestPrebidSChain {
            bidders: Some(strings(&["A", "B", "C"])),
            s_chain: SupplyChain {
                complete: 1,
                ver: "2.2".to_owned(),
                ext: raw(r#"{"foo": "bar"}"#),
                nodes: Some(vec![SupplyChainNode {
                    asi: "something".to_owned(),
                    domain: "example.com".to_owned(),
                    hp: Some(1),
                    ..SupplyChainNode::default()
                }]),
            },
        }))]),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_schains(prebid: &mut ExtRequestPrebid) {
    prebid.passthrough = raw(r#"{"bar": "foo"}"#);
    let chains = prebid.s_chains.get_or_insert_with(Vec::new);
    if let Some(Some(chain)) = chains.get_mut(0) {
        chain
            .bidders
            .get_or_insert_with(Vec::new)
            .push("D".to_owned());
        chain.s_chain.ver = "2.3".to_owned();
        chain.s_chain.complete = 0;
        let nodes = chain.s_chain.nodes.get_or_insert_with(Vec::new);
        if let Some(node) = nodes.get_mut(0) {
            node.name = "Alice".to_owned();
            node.asi = "New ASI".to_owned();
            node.hp = Some(0);
        }
        if let Some(node) = nodes.first().cloned() {
            nodes.push(node);
        }
    }
    if let Some(first) = chains.first().cloned() {
        chains.push(first);
    }
}

fn stored_request() -> ExtRequestPrebid {
    ExtRequestPrebid {
        stored_request: Some(Box::new(ExtStoredRequest {
            id: "abc123".to_owned(),
        })),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_stored_request(prebid: &mut ExtRequestPrebid) {
    if let Some(stored) = prebid.stored_request.as_deref_mut() {
        "nada".clone_into(&mut stored.id);
    }
    prebid.stored_request = Some(Box::new(ExtStoredRequest {
        id: "ID".to_owned(),
    }));
}

fn targeting() -> ExtRequestPrebid {
    ExtRequestPrebid {
        targeting: Some(Box::new(ExtRequestTargeting {
            price_granularity: Some(Box::new(granularity(
                Some(2),
                Some(vec![
                    range(0.0, 2.0, 0.1),
                    range(0.0, 10.0, 0.5),
                    range(0.0, 20.0, 1.0),
                ]),
            ))),
            include_winners: Some(true),
            include_brand_category: Some(Box::new(ExtIncludeBrandCategory {
                primary_ad_server: 1,
                publisher: "Bob".to_owned(),
                translate_categories: Some(true),
                ..ExtIncludeBrandCategory::default()
            })),
            duration_range_sec: Some(vec![1, 2, 3]),
            ..ExtRequestTargeting::default()
        })),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_targeting(prebid: &mut ExtRequestPrebid) {
    let Some(targeting) = prebid.targeting.as_deref_mut() else {
        return;
    };
    if let Some(pg) = targeting.price_granularity.as_deref_mut() {
        let ranges = pg.ranges.get_or_insert_with(Vec::new);
        if let Some(r) = ranges.get_mut(1) {
            r.max = 12.0;
            r.min = 2.0;
        }
        ranges.push(range(0.0, 50.0, 2.0));
    }
    targeting.include_winners = None;
    targeting.include_bidder_keys = Some(true);
    if let Some(category) = targeting.include_brand_category.as_deref_mut() {
        category.translate_categories = Some(false);
    }
    targeting.include_brand_category = None;
    let durations = targeting.duration_range_sec.get_or_insert_with(Vec::new);
    if let Some(d) = durations.get_mut(1) {
        *d = 5;
    }
    durations.push(1);
    targeting.append_bidder_names = true;
}

fn no_sale() -> ExtRequestPrebid {
    ExtRequestPrebid {
        no_sale: Some(strings(&["A", "B", "C"])),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_no_sale(prebid: &mut ExtRequestPrebid) {
    let no_sale = prebid.no_sale.get_or_insert_with(Vec::new);
    if let Some(s) = no_sale.get_mut(1) {
        "G".clone_into(s);
    }
    no_sale.push("D".to_owned());
}

fn alternate_bidder_codes() -> ExtRequestPrebid {
    ExtRequestPrebid {
        alternate_bidder_codes: Some(Box::new(ExtAlternateBidderCodes {
            enabled: true,
            bidders: Some(BTreeMap::from([
                (
                    "X".to_owned(),
                    ExtAdapterAlternateBidderCodes {
                        enabled: true,
                        allowed_bidder_codes: Some(strings(&["A", "B", "C"])),
                    },
                ),
                (
                    "Y".to_owned(),
                    ExtAdapterAlternateBidderCodes {
                        enabled: false,
                        allowed_bidder_codes: Some(strings(&["C", "B", "G"])),
                    },
                ),
            ])),
        })),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_alternate_bidder_codes(prebid: &mut ExtRequestPrebid) {
    if let Some(codes) = prebid.alternate_bidder_codes.as_deref_mut() {
        let bidders = codes.bidders.get_or_insert_with(BTreeMap::new);
        let mut new_aabc = bidders.get("X").cloned().unwrap_or_default();
        new_aabc.enabled = false;
        let allowed = new_aabc.allowed_bidder_codes.get_or_insert_with(Vec::new);
        if let Some(code) = allowed.get_mut(1) {
            "F".clone_into(code);
        }
        allowed.push("Z".to_owned());
        bidders.extend([
            ("X".to_owned(), new_aabc),
            (
                "Z".to_owned(),
                ExtAdapterAlternateBidderCodes {
                    enabled: true,
                    allowed_bidder_codes: Some(strings(&["G", "Z"])),
                },
            ),
        ]);
        codes.enabled = false;
    }
    prebid.alternate_bidder_codes = None;
}

fn floors() -> ExtRequestPrebid {
    ExtRequestPrebid {
        floors: Some(Box::new(PriceFloorRules {
            floor_min: 0.25,
            floor_min_cur: "EUR".to_owned(),
            location: Some(Box::new(PriceFloorEndpoint {
                url: "http://www.example.com".to_owned(),
            })),
            data: Some(Box::new(PriceFloorData {
                currency: "USD".to_owned(),
                skip_rate: 3,
                model_groups: Some(vec![PriceFloorModelGroup {
                    currency: "USD".to_owned(),
                    model_weight: Some(0),
                    skip_rate: 2,
                    schema: PriceFloorSchema {
                        fields: Some(strings(&["A", "B"])),
                        delimiter: "^".to_owned(),
                    },
                    values: Some(BTreeMap::from([
                        ("A".to_owned(), 2.0),
                        ("B".to_owned(), 1.0),
                    ])),
                    ..PriceFloorModelGroup::default()
                }]),
                ..PriceFloorData::default()
            })),
            enforcement: Some(Box::new(PriceFloorEnforcement {
                enforce_js: Some(true),
                floor_deals: Some(false),
                enforce_rate: 5,
                ..PriceFloorEnforcement::default()
            })),
            enabled: Some(true),
            floor_provider: "Someone".to_owned(),
            ..PriceFloorRules::default()
        })),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_floors(prebid: &mut ExtRequestPrebid) {
    let Some(floors) = prebid.floors.as_deref_mut() else {
        return;
    };
    if let Some(data) = floors.data.as_deref_mut() {
        let groups = data.model_groups.get_or_insert_with(Vec::new);
        if let Some(group) = groups.get_mut(0) {
            let fields = group.schema.fields.get_or_insert_with(Vec::new);
            if let Some(field) = fields.get_mut(1) {
                "C".clone_into(field);
            }
            fields.push("D".to_owned());
            ",".clone_into(&mut group.schema.delimiter);
            "CRO".clone_into(&mut group.currency);
            group.model_weight = Some(8);
            let values = group.values.get_or_insert_with(BTreeMap::new);
            values.extend([("A".to_owned(), 0.0), ("C".to_owned(), 7.0)]);
        }
        groups.push(PriceFloorModelGroup::default());
    }
    floors.floor_min = 0.3;
    "arf".clone_into(&mut floors.fetch_status);
    if let Some(location) = floors.location.as_deref_mut() {
        "www.nothere.com".clone_into(&mut location.url);
    }
    floors.location = None;
    floors.enabled = None;
    floors.skipped = Some(true);
    if let Some(enforcement) = floors.enforcement.as_deref_mut() {
        enforcement.bid_adjustment = Some(true);
        enforcement.enforce_js = Some(false);
        enforcement.floor_deals = None;
    }
    floors.floor_provider.clear();
}

fn multi_bid_map() -> ExtRequestPrebid {
    ExtRequestPrebid {
        multi_bid_map: Some(BTreeMap::from([
            (
                "A".to_owned(),
                multi_bid("J", Some(strings(&["X", "Y", "Z"])), Some(5), ">>"),
            ),
            (
                "B".to_owned(),
                multi_bid("J", Some(strings(&["One", "Two", "Three"])), None, ""),
            ),
        ])),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_multi_bid_map(prebid: &mut ExtRequestPrebid) {
    let map = prebid.multi_bid_map.get_or_insert_with(BTreeMap::new);
    let mut mulbid_b = map.get("B").cloned().unwrap_or_default();
    "|".clone_into(&mut mulbid_b.target_bidder_code_prefix);
    let bidders = mulbid_b.bidders.get_or_insert_with(Vec::new);
    if let Some(b) = bidders.get_mut(1) {
        "Five".clone_into(b);
    }
    bidders.push("Six".to_owned());
    mulbid_b.max_bids = Some(2);
    map.extend([
        ("B".to_owned(), mulbid_b),
        ("C".to_owned(), multi_bid("alpha", None, Some(3), "")),
    ]);
}

fn ad_server_targeting() -> ExtRequestPrebid {
    ExtRequestPrebid {
        ad_server_targeting: Some(vec![
            AdServerTarget {
                key: "A".to_owned(),
                source: "Sauce".to_owned(),
                value: "Gold".to_owned(),
            },
            AdServerTarget {
                key: "B".to_owned(),
                source: "Omega".to_owned(),
                value: "Dirt".to_owned(),
            },
        ]),
        ..ExtRequestPrebid::default()
    }
}

fn mutate_ad_server_targeting(prebid: &mut ExtRequestPrebid) {
    let targets = prebid.ad_server_targeting.get_or_insert_with(Vec::new);
    if let Some(t) = targets.get_mut(0) {
        "Five".clone_into(&mut t.key);
    }
    if let Some(t) = targets.get_mut(1) {
        "Dust".clone_into(&mut t.value);
    }
    targets.push(AdServerTarget {
        key: "Val".to_owned(),
        ..AdServerTarget::default()
    });
    prebid.ad_server_targeting = None;
}

fn no_mutation(_prebid: &mut ExtRequestPrebid) {}

/// A case of `TestCloneExtRequestPrebid`: the value (built twice, as Go writes `prebid` and its
/// manual copy `prebidCopy`) and the change made to the original after cloning it.
type CloneCase = (
    &'static str,
    fn() -> ExtRequestPrebid,
    fn(&mut ExtRequestPrebid),
);

/// Go: `TestCloneExtRequestPrebid`: changing the original after cloning leaves the clone equal
/// to the original's copy. (Go's nil value clones to nil, the caller's `None`; Go also checks
/// that the clone is a new pointer, which a Rust clone always is.)
#[test]
fn clone_ext_request_prebid() -> TestResult {
    let none: Option<ExtRequestPrebid> = None;
    ensure_eq(&none.as_ref().map(ExtRequestPrebid::clone_go), &None, "Nil")?;
    let cases: [CloneCase; 15] = [
        ("NoMutateTest", general, no_mutation),
        ("GeneralTest", general, mutate_general),
        ("BidderConfig", bidder_configs, mutate_bidder_configs),
        ("Cache", cache, mutate_cache),
        ("Currency", currency, mutate_currency),
        ("Data", data, mutate_data),
        ("Multibid", multibid, mutate_multibid),
        ("PassthroughSChains", schains, mutate_schains),
        ("StoredRequest", stored_request, mutate_stored_request),
        ("Targeting", targeting, mutate_targeting),
        ("NoSale", no_sale, mutate_no_sale),
        (
            "AlternateBidderCodes",
            alternate_bidder_codes,
            mutate_alternate_bidder_codes,
        ),
        ("Floors", floors, mutate_floors),
        ("MultiBidMap", multi_bid_map, mutate_multi_bid_map),
        (
            "MultiBidMap",
            ad_server_targeting,
            mutate_ad_server_targeting,
        ),
    ];
    all_ok(cases.into_iter().map(|(name, build, mutator)| {
        let mut prebid = build();
        let prebid_copy = build();
        let clone = prebid.clone_go();
        mutator(&mut prebid);
        (name.to_owned(), ensure_eq(&clone, &prebid_copy, "clone"))
    }))
}
