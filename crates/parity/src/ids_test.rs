use std::collections::BTreeMap;

use serde::{Serialize, de::DeserializeOwned};

use super::{
    DecisionDomain, DecisionId, KeyedBy, OutboundKind, StageId, StageStatus, TraceField,
    UNKEYED_STAGE,
};
use crate::{
    contracts::{Domain, IMPL_GO, IMPL_RUST},
    golden::{Json, TestResult, check, check_eq, contract_json},
    tracedoc::Trace,
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

/// `v` as `serde_json` writes it.
fn encoded<T: Serialize + ?Sized>(v: &T) -> Result<String, String> {
    serde_json::to_string(v).map_err(|e| e.to_string())
}

/// `input` read as a `T`, or `serde_json`'s message.
fn decoded<T: DeserializeOwned>(input: &str) -> Result<T, String> {
    serde_json::from_str(input).map_err(|e| e.to_string())
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

// The IDs are written as their names; a map keyed by them in registry order, except the trace's
// stage maps (the documents' only ones), which are written sorted by name as Go writes a map.
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
        &r#"{"http.in":2,"targeting":1,"analytics":3}"#.to_owned(),
        "registry order",
    )?;
    let trace = Trace {
        stage_status: Some(BTreeMap::from([
            (StageId::Targeting, StageStatus::Captured),
            (StageId::HttpIn, StageStatus::NotReached),
            (StageId::Analytics, StageStatus::Captured),
        ])),
        ..Trace::default()
    };
    let text = encoded(&trace)?;
    let sorted = concat!(
        r#""stage_status":{"analytics":"captured","#,
        r#""http.in":"not_reached","targeting":"captured"}"#,
    );
    check(text.contains(sorted), &text)
}

// Go's string types take any value; the enums take the registered IDs only, as values and as
// map keys, and refuse anything else naming the value (json.parity-documents: only the Go
// runner writes these documents, from the same registry).
#[test]
fn decodes_registered_values_only() -> TestResult {
    check_eq(
        &decoded::<StageId>(r#""http.in""#),
        &Ok(StageId::HttpIn),
        "stage",
    )?;
    check_eq(
        &decoded::<StageId>(r#""made.up""#),
        &Err(
            r#"parity: unregistered parity.StageID value "made.up" at line 1 column 9"#.to_owned(),
        ),
        "unregistered",
    )?;
    check_eq(
        &decoded::<StageId>("5"),
        &Err(
            "invalid type: integer `5`, expected a registered parity.StageID at line 1 column 1"
                .to_owned(),
        ),
        "type error",
    )?;
    check_eq(
        &decoded::<Option<StageId>>("null"),
        &Ok(None),
        "null is no stage",
    )?;
    check_eq(
        &decoded::<Option<Vec<DecisionId>>>(r#"["floors.skip","usersync.shuffle"]"#),
        &Ok(Some(vec![
            DecisionId::FloorsSkip,
            DecisionId::UsersyncShuffle,
        ])),
        "list",
    )?;
    check_eq(
        &decoded::<BTreeMap<StageId, StageStatus>>(r#"{"http.in":"captured"}"#),
        &Ok(BTreeMap::from([(StageId::HttpIn, StageStatus::Captured)])),
        "map keys",
    )?;
    check_eq(
        &decoded::<StageStatus>(r#""unimplemented""#),
        &Ok(StageStatus::Unimplemented),
        "status",
    )?;
    check_eq(
        &decoded::<DecisionId>(r#""""#),
        &Err(r#"parity: unregistered parity.DecisionID value "" at line 1 column 2"#.to_owned()),
        "the empty value",
    )?;
    check_eq(
        &decoded::<DecisionId>(r#""floors""#),
        &Err(
            r#"parity: unregistered parity.DecisionID value "floors" at line 1 column 8"#
                .to_owned(),
        ),
        "unregistered decision",
    )
}
