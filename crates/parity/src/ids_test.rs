use std::collections::BTreeMap;

use pbs_gojson::{GoEncode, jsoniter, stdjson};

use super::{
    DecisionDomain, DecisionId, KeyedBy, OutboundKind, StageId, StageStatus, TraceField,
    UNKEYED_STAGE,
};
use crate::{
    contracts::{Domain, IMPL_GO, IMPL_RUST},
    golden::{Json, TestResult, check, check_eq, contract_json},
};

/// The `enum` of a `common.schema.json` definition.
fn schema_enum(definition: &str) -> Result<Vec<String>, String> {
    contract_json("common.schema.json")?
        .get("definitions")?
        .get(definition)?
        .array("enum")?
        .iter()
        .map(Json::text)
        .collect()
}

fn names<T: Copy>(all: &[T], name: fn(T) -> &'static str) -> Vec<String> {
    all.iter().map(|v| name(*v).to_owned()).collect()
}

/// `json.Marshal(v)` as text.
fn encoded<T: GoEncode + ?Sized>(v: &T) -> Result<String, String> {
    stdjson::marshal(v)
        .map(|raw| String::from_utf8_lossy(&raw).into_owned())
        .map_err(|e| e.to_string())
}

// Every stage of stages.json, in order, with its domain, keys and trace member.
#[test]
fn stages_match_the_registry() -> TestResult {
    let registry = contract_json("stages.json")?;
    let stages = registry.array("stages")?;
    check_eq(&stages.len(), &StageId::ALL.len(), "stage count")?;
    for (spec, stage) in stages.iter().zip(StageId::ALL) {
        let id = spec.string("id")?;
        check_eq(&stage.as_str(), &id.as_str(), "stage id")?;
        check_eq(
            &i64::try_from(stage.order()).ok(),
            &Some(spec.int("order")?),
            &id,
        )?;
        check_eq(
            &stage.domain().as_str(),
            &spec.string("domain")?.as_str(),
            &id,
        )?;
        check_eq(
            &stage.keyed_by().as_str(),
            &spec.string("keyed_by")?.as_str(),
            &id,
        )?;
        check_eq(
            &stage.trace_field().as_str(),
            &spec.string("trace_field")?.as_str(),
            &id,
        )?;
    }
    let domains: Vec<String> = registry
        .array("domains")?
        .iter()
        .map(|d| d.string("id").or_else(|_| d.text()))
        .collect::<Result<_, _>>()?;
    check_eq(&domains, &names(&Domain::ALL, Domain::as_str), "domains")
}

// The closed enums of common.schema.json.
#[test]
fn identifiers_match_the_common_schema() -> TestResult {
    check_eq(
        &schema_enum("StageID")?,
        &names(&StageId::ALL, StageId::as_str),
        "StageID",
    )?;
    check_eq(
        &schema_enum("DecisionID")?,
        &names(&DecisionId::ALL, DecisionId::as_str),
        "DecisionID",
    )?;
    check_eq(
        &schema_enum("OutboundKind")?,
        &names(&OutboundKind::ALL, OutboundKind::as_str),
        "OutboundKind",
    )?;
    check_eq(
        &schema_enum("Domain")?,
        &names(&Domain::ALL, Domain::as_str),
        "Domain",
    )?;
    check_eq(
        &schema_enum("Impl")?,
        &vec![IMPL_GO.to_owned(), IMPL_RUST.to_owned()],
        "Impl",
    )
}

// Every decision point of decisions.json, in order, with its domain and enum values.
#[test]
fn decisions_match_the_registry() -> TestResult {
    let decisions = contract_json("decisions.json")?.array("decisions")?;
    check_eq(&decisions.len(), &DecisionId::ALL.len(), "decision count")?;
    for (spec, point) in decisions.iter().zip(DecisionId::ALL) {
        let id = spec.string("id")?;
        check_eq(&point.as_str(), &id.as_str(), "decision id")?;
        check_eq(
            &point.domain().as_str(),
            &spec.string("domain")?.as_str(),
            &id,
        )?;
        let values: Vec<String> = spec
            .get("values")?
            .0
            .as_object()
            .map_or_default(|m| m.keys().cloned().collect());
        if point.domain() == DecisionDomain::Enum {
            let want: Vec<String> = point
                .enum_values()
                .iter()
                .map(|v| (*v).to_owned())
                .collect();
            check_eq(&values, &want, &id)?;
        } else {
            check(point.enum_values().is_empty(), "no enum values")?;
        }
        if point.domain() == DecisionDomain::Binary {
            check_eq(
                &values,
                &vec!["0".to_owned(), "1".to_owned()],
                "binary values",
            )?;
        }
        let outside_case = spec.opt("outside_case").map(|v| v.text()).transpose()?;
        check_eq(
            &point.outside_case().map(str::to_owned),
            &outside_case,
            &format!("{id} outside_case"),
        )?;
        if let Some(value) = point.outside_case() {
            check(
                values.iter().any(|v| v == value),
                &format!("{id} outside_case is one of its values"),
            )?;
        }
    }
    Ok(())
}

#[test]
fn names_round_trip() -> TestResult {
    for stage in StageId::ALL {
        check_eq(&StageId::from_name(stage.as_str()), &Some(stage), "stage")?;
    }
    for status in StageStatus::ALL {
        check_eq(
            &StageStatus::from_name(status.as_str()),
            &Some(status),
            "status",
        )?;
    }
    for point in DecisionId::ALL {
        check_eq(
            &DecisionId::from_name(point.as_str()),
            &Some(point),
            "point",
        )?;
    }
    for kind in OutboundKind::ALL {
        check_eq(&OutboundKind::from_name(kind.as_str()), &Some(kind), "kind")?;
    }
    check_eq(&StageId::from_name("made.up"), &None, "unregistered stage")?;
    check_eq(&StageId::from_name(""), &None, "empty stage")?;
    check_eq(&StageStatus::from_name("done"), &None, "unknown status")?;
    check_eq(&DecisionId::from_name("floors"), &None, "unknown point")?;
    check_eq(
        &OutboundKind::from_name("Bidder"),
        &None,
        "kinds are lower case",
    )?;
    check_eq(&UNKEYED_STAGE, &"_", "unkeyed stage")?;
    check_eq(
        &StageStatus::default(),
        &StageStatus::NotReached,
        "default status",
    )
}

#[test]
fn stage_metadata() -> TestResult {
    check_eq(&StageId::ConfigResolved.order(), &1, "first")?;
    check_eq(&StageId::HttpResponse.order(), &45, "last")?;
    check_eq(
        &StageId::AdapterBids.keyed_by(),
        &KeyedBy::BidderCall,
        "bidder#call",
    )?;
    check_eq(&StageId::BidsAdjusted.keyed_by(), &KeyedBy::Seat, "seat")?;
    check_eq(
        &StageId::HttpOut.trace_field(),
        &TraceField::HttpOut,
        "http_out",
    )?;
    check_eq(&StageId::HttpIn.domain(), &Domain::Endpoints, "domain")?;
    let mut set = 0_u64;
    for stage in StageId::ALL {
        check(set & stage.bit() == 0, "distinct bits")?;
        set |= stage.bit();
    }
    check_eq(&set.count_ones(), &45, "45 bits")
}

// Go encodes map keys sorted by their text, whatever the registry order.
#[test]
fn encodes_like_go_strings() -> TestResult {
    check_eq(
        &encoded(&StageId::HttpIn)?,
        &r#""http.in""#.to_owned(),
        "stage",
    )?;
    check_eq(
        &encoded(&StageStatus::NotReached)?,
        &r#""not_reached""#.to_owned(),
        "status",
    )?;
    check_eq(
        &encoded(&DecisionId::UsersyncShuffle)?,
        &r#""usersync.shuffle""#.to_owned(),
        "decision",
    )?;
    let map = BTreeMap::from([
        (StageId::Targeting, 1_isize),
        (StageId::HttpIn, 2),
        (StageId::Analytics, 3),
    ]);
    check_eq(
        &encoded(&map)?,
        &r#"{"analytics":3,"http.in":2,"targeting":1}"#.to_owned(),
        "keys sorted by text",
    )
}

#[test]
fn decodes_registered_values_only() -> TestResult {
    let mut stage = StageId::default();
    stdjson::unmarshal(b"\"http.in\"", &mut stage).map_err(|e| e.to_string())?;
    check_eq(&stage, &StageId::HttpIn, "std")?;
    stdjson::unmarshal(b"null", &mut stage).map_err(|e| e.to_string())?;
    check_eq(&stage, &StageId::HttpIn, "null is a no-op")?;
    check_eq(
        &stdjson::unmarshal(b"\"made.up\"", &mut stage)
            .err()
            .map(|e| e.to_string()),
        &Some(r#"parity: unregistered parity.StageID value "made.up""#.to_owned()),
        "unregistered",
    )?;
    check_eq(
        &stdjson::unmarshal(b"5", &mut stage)
            .err()
            .map(|e| e.to_string()),
        &Some("json: cannot unmarshal number into Go value of type parity.StageID".to_owned()),
        "type error",
    )?;
    let mut list: Option<Vec<DecisionId>> = None;
    stdjson::unmarshal(br#"["floors.skip","usersync.shuffle"]"#, &mut list)
        .map_err(|e| e.to_string())?;
    check_eq(
        &list,
        &Some(vec![DecisionId::FloorsSkip, DecisionId::UsersyncShuffle]),
        "list",
    )?;
    let mut statuses: Option<BTreeMap<StageId, StageStatus>> = None;
    stdjson::unmarshal(br#"{"http.in":"captured"}"#, &mut statuses).map_err(|e| e.to_string())?;
    check_eq(
        &statuses,
        &Some(BTreeMap::from([(StageId::HttpIn, StageStatus::Captured)])),
        "map keys",
    )?;

    let mut point = DecisionId::default();
    jsoniter::unmarshal(b"\"floors.enforce\"", &mut point).map_err(|e| e.to_string())?;
    check_eq(&point, &DecisionId::FloorsEnforce, "jsoniter")?;
    jsoniter::unmarshal(b"null", &mut point).map_err(|e| e.to_string())?;
    check_eq(&point, &DecisionId::FloorsEnforce, "jsoniter null")?;
    check(
        jsoniter::unmarshal(b"\"\"", &mut point).is_err(),
        "jsoniter rejects the empty value",
    )?;
    check_eq(
        &jsoniter::unmarshal(b"\"floors\"", &mut point)
            .err()
            .map(|e| e.to_string()),
        &Some(r#"parity: unregistered parity.DecisionID value "floors""#.to_owned()),
        "jsoniter unregistered",
    )
}
