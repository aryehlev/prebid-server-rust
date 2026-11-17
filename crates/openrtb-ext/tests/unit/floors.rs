//! Go: `openrtb_ext/floors_test.go`.

use std::collections::BTreeMap;

use pbs_openrtb_ext::openrtb_ext::{
    PriceFloorData, PriceFloorEndpoint, PriceFloorEnforcement, PriceFloorModelGroup,
    PriceFloorRules, PriceFloorSchema,
};

use super::support::{TestResult, all_ok, ensure_eq};

/// Go: `TestPriceFloorRulesGetEnforcePBS`.
#[test]
fn get_enforce_pbs() -> TestResult {
    let cases = [
        (
            "EnforcePBS_Enabled",
            PriceFloorRules {
                enabled: Some(true),
                enforcement: Some(Box::new(PriceFloorEnforcement {
                    enforce_pbs: Some(true),
                    ..PriceFloorEnforcement::default()
                })),
                ..PriceFloorRules::default()
            },
            true,
        ),
        (
            "EnforcePBS_NotProvided",
            PriceFloorRules {
                enabled: Some(true),
                enforcement: Some(Box::default()),
                ..PriceFloorRules::default()
            },
            true,
        ),
        (
            "EnforcePBS_Disabled",
            PriceFloorRules {
                enabled: Some(true),
                enforcement: Some(Box::new(PriceFloorEnforcement {
                    enforce_pbs: Some(false),
                    ..PriceFloorEnforcement::default()
                })),
                ..PriceFloorRules::default()
            },
            false,
        ),
    ];
    all_ok(cases.into_iter().map(|(name, floors, want)| {
        let got = PriceFloorRules::get_enforce_pbs(Some(&floors));
        (name.to_owned(), ensure_eq(&got, &want, name))
    }))
}

/// Go: `TestPriceFloorRulesGetFloorsSkippedFlag`.
#[test]
fn get_floors_skipped_flag() -> TestResult {
    let cases = [
        ("Skipped_true", Some(true), true),
        ("Skipped_false", Some(false), false),
        ("Skipped_NotProvided", None, false),
    ];
    all_ok(cases.into_iter().map(|(name, skipped, want)| {
        let floors = PriceFloorRules {
            enabled: Some(true),
            skipped,
            ..PriceFloorRules::default()
        };
        let got = PriceFloorRules::get_floors_skipped_flag(Some(&floors));
        (name.to_owned(), ensure_eq(&got, &want, name))
    }))
}

/// Go: `TestPriceFloorRulesGetEnforceRate`.
#[test]
fn get_enforce_rate() -> TestResult {
    let with_rate = |enforce_rate| {
        Some(Box::new(PriceFloorEnforcement {
            enforce_pbs: Some(true),
            enforce_rate,
            ..PriceFloorEnforcement::default()
        }))
    };
    let cases = [
        ("EnforceRate_100", with_rate(100), 100),
        ("EnforceRate_0", with_rate(0), 0),
        ("EnforceRate_NotProvided", None, 0),
    ];
    all_ok(cases.into_iter().map(|(name, enforcement, want)| {
        let floors = PriceFloorRules {
            enabled: Some(true),
            enforcement,
            ..PriceFloorRules::default()
        };
        let got = PriceFloorRules::get_enforce_rate(Some(&floors));
        (name.to_owned(), ensure_eq(&got, &want, name))
    }))
}

/// Go: `TestPriceFloorRulesGetEnforceDealsFlag`.
#[test]
fn get_enforce_deals_flag() -> TestResult {
    let cases = [
        (
            "FloorDeals_true",
            PriceFloorRules {
                enabled: Some(true),
                enforcement: Some(Box::new(PriceFloorEnforcement {
                    enforce_pbs: Some(true),
                    enforce_rate: 0,
                    floor_deals: Some(true),
                    ..PriceFloorEnforcement::default()
                })),
                ..PriceFloorRules::default()
            },
            true,
        ),
        (
            "FloorDeals_false",
            PriceFloorRules {
                enabled: Some(true),
                enforcement: Some(Box::new(PriceFloorEnforcement {
                    enforce_pbs: Some(true),
                    floor_deals: Some(false),
                    ..PriceFloorEnforcement::default()
                })),
                skipped: Some(false),
                ..PriceFloorRules::default()
            },
            false,
        ),
        (
            "FloorDeals_NotProvided",
            PriceFloorRules {
                enabled: Some(true),
                ..PriceFloorRules::default()
            },
            false,
        ),
    ];
    all_ok(cases.into_iter().map(|(name, floors, want)| {
        let got = PriceFloorRules::get_enforce_deals_flag(Some(&floors));
        (name.to_owned(), ensure_eq(&got, &want, name))
    }))
}

/// Go: `TestPriceFloorRulesGetEnabled`.
#[test]
fn get_enabled() -> TestResult {
    let cases = [
        ("Enabled_true", Some(true), true),
        ("Enabled_false", Some(false), false),
        ("Enabled_NotProvided", None, true),
    ];
    all_ok(cases.into_iter().map(|(name, enabled, want)| {
        let floors = PriceFloorRules {
            enabled,
            ..PriceFloorRules::default()
        };
        let got = PriceFloorRules::get_enabled(Some(&floors));
        (name.to_owned(), ensure_eq(&got, &want, name))
    }))
}

fn values<const N: usize>(entries: [(&str, f64); N]) -> BTreeMap<String, f64> {
    entries
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect()
}

fn fields(names: &[&str]) -> Vec<String> {
    names.iter().map(|&n| n.to_owned()).collect()
}

/// Go: `TestPriceFloorRulesDeepCopy`: the copy is a new value with new data (Go compares the
/// pointers; a Rust copy owns its own, so the copy is checked to equal the original).
#[test]
fn price_floor_rules_deep_copy() -> TestResult {
    let pf = PriceFloorRules {
        floor_min: 10.0,
        floor_min_cur: "INR".to_owned(),
        skip_rate: 0,
        location: Some(Box::new(PriceFloorEndpoint {
            url: "https://test/floors".to_owned(),
        })),
        data: Some(Box::new(PriceFloorData {
            currency: "INR".to_owned(),
            skip_rate: 0,
            model_groups: Some(vec![PriceFloorModelGroup {
                currency: "INR".to_owned(),
                model_weight: Some(1),
                skip_rate: 0,
                values: Some(values([
                    ("banner|300x600|www.website5.com", 20.0),
                    ("*|*|*", 50.0),
                ])),
                schema: PriceFloorSchema {
                    fields: Some(fields(&["mediaType", "size", "domain"])),
                    delimiter: "|".to_owned(),
                },
                ..PriceFloorModelGroup::default()
            }]),
            ..PriceFloorData::default()
        })),
        ..PriceFloorRules::default()
    };
    ensure_eq(
        &pf.deep_copy(),
        &pf,
        "DeepCopy does not share same reference",
    )
}

/// The rules of `TestFloorRulesDeepCopy`, with or without data.
fn floor_rules(with_data: bool) -> PriceFloorRules {
    let data = PriceFloorData {
        currency: "INR".to_owned(),
        skip_rate: 0,
        floors_schema_version: 2,
        model_timestamp: 123,
        model_groups: Some(vec![PriceFloorModelGroup {
            currency: "INR".to_owned(),
            model_weight: Some(50),
            model_version: "version 1".to_owned(),
            skip_rate: 0,
            schema: PriceFloorSchema {
                fields: Some(fields(&["a", "b", "c"])),
                delimiter: "|".to_owned(),
            },
            values: Some(values([("*|*|*", 20.0)])),
            default: 1.0,
        }]),
        floor_provider: "prebid".to_owned(),
        ..PriceFloorData::default()
    };
    PriceFloorRules {
        floor_min: 10.0,
        floor_min_cur: "INR".to_owned(),
        skip_rate: 0,
        location: Some(Box::new(PriceFloorEndpoint {
            url: "http://prebid.com/floor".to_owned(),
        })),
        data: with_data.then(|| Box::new(data)),
        enforcement: Some(Box::new(PriceFloorEnforcement {
            enforce_js: Some(true),
            enforce_pbs: Some(true),
            floor_deals: Some(true),
            bid_adjustment: Some(true),
            enforce_rate: 100,
        })),
        enabled: Some(true),
        skipped: Some(false),
        floor_provider: "Prebid".to_owned(),
        fetch_status: "success".to_owned(),
        price_floor_location: "fetch".to_owned(),
    }
}

/// Go: `TestFloorRulesDeepCopy`.
#[test]
fn floor_rules_deep_copy() -> TestResult {
    for with_data in [true, false] {
        let pf = floor_rules(with_data);
        ensure_eq(
            &pf.deep_copy(),
            &floor_rules(with_data),
            "Copy entire floors object",
        )?;
    }
    Ok(())
}

/// Go: `TestFloorRuleDeepCopyNil`: Go's nil rules copy to nil (the caller's `None`).
#[test]
fn floor_rule_deep_copy_nil() -> TestResult {
    let rules: Option<PriceFloorRules> = None;
    ensure_eq(
        &rules.as_ref().map(PriceFloorRules::deep_copy),
        &None,
        "nil",
    )
}
