//! Every Go type, struct field, JSON tag and constant of the six packages against the reflect
//! and `go/types` metadata of the Go build in `testdata/go_struct_meta.json`.

use std::collections::BTreeMap;

use pbs_gojson::{
    GoEncode, GoKind, GoType,
    gotype::go_type_name,
    stdjson,
    structs::{FieldDesc, GoStruct, StructDesc},
};
use pbs_openrtb::{adcom1, native1, openrtb2, openrtb3};
use serde_json::Value;

use super::{
    index::{CONSTANTS, for_each_const, for_each_struct, for_each_type},
    support::{TestResult, ensure_eq, read, report, workspace_root},
};

/// The import paths of the ported packages start with this.
const MODULE: &str = "github.com/prebid/openrtb/v20/";

/// The untyped Go constants, which the metadata leaves out (it lists typed constants): each
/// is ported with the named type it is used as (an override in `typegen.json`), with its Go
/// value (`openrtb3/auction_type.go`, `native1/placement_type.go`).
const UNTYPED: [(&str, &str, &str); 6] = [
    (
        "native1.PlacementTypeAtomicContentUnit",
        "native1.PlacementType",
        "2",
    ),
    ("native1.PlacementTypeFeed", "native1.PlacementType", "1"),
    (
        "native1.PlacementTypeOutsideCoreContent",
        "native1.PlacementType",
        "3",
    ),
    (
        "native1.PlacementTypeRecommendationWidget",
        "native1.PlacementType",
        "4",
    ),
    ("openrtb3.DealPrice", "openrtb3.AuctionType", "3"),
    ("openrtb3.SecondPricePlus", "openrtb3.AuctionType", "2"),
];

/// Go's `reflect.Kind.String()`.
const fn kind_name(kind: GoKind) -> &'static str {
    match kind {
        GoKind::Bool => "bool",
        GoKind::Int => "int",
        GoKind::Int8 => "int8",
        GoKind::Int16 => "int16",
        GoKind::Int32 => "int32",
        GoKind::Int64 => "int64",
        GoKind::Uint => "uint",
        GoKind::Uint8 => "uint8",
        GoKind::Uint16 => "uint16",
        GoKind::Uint32 => "uint32",
        GoKind::Uint64 => "uint64",
        GoKind::Float32 => "float32",
        GoKind::Float64 => "float64",
        GoKind::String => "string",
        GoKind::Interface => "interface",
        GoKind::Map => "map",
        GoKind::Ptr => "ptr",
        GoKind::Slice => "slice",
        GoKind::Struct => "struct",
    }
}

fn text<'a>(value: &'a Value, key: &str) -> TestResult<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("metadata without {key}: {value}"))
}

fn list<'a>(value: &'a Value, key: &str) -> TestResult<&'a [Value]> {
    match value.get(key) {
        Some(Value::Array(items)) => Ok(items),
        Some(Value::Null) | None => Ok(&[]),
        Some(other) => Err(format!("metadata {key} is not a list: {other}")),
    }
}

/// The metadata of the ported packages.
fn packages() -> TestResult<Vec<Value>> {
    let path = workspace_root()?.join("testdata/go_struct_meta.json");
    let meta: Value = serde_json::from_slice(&read(&path)?).map_err(|e| e.to_string())?;
    let packages: Vec<Value> = list(&meta, "packages")?
        .iter()
        .filter(|p| text(p, "path").is_ok_and(|import| import.starts_with(MODULE)))
        .cloned()
        .collect();
    ensure_eq(&packages.len(), &6, "the ported packages in the metadata")?;
    Ok(packages)
}

/// A ported type as the Go JSON libraries see it.
struct Ported {
    kind: GoKind,
    desc: Option<&'static StructDesc>,
}

/// Every ported type by Go name.
fn ported() -> TestResult<BTreeMap<String, Ported>> {
    let mut types = BTreeMap::new();
    macro_rules! kind {
        ($ty:ty) => {{
            let ported = Ported {
                kind: <$ty as GoType>::KIND,
                desc: None,
            };
            if types.insert(go_type_name::<$ty>(), ported).is_some() {
                return Err(format!(
                    "two Rust types for Go type {}",
                    go_type_name::<$ty>()
                ));
            }
        }};
    }
    for_each_type!(kind);
    macro_rules! desc {
        ($ty:ty) => {{
            let name = go_type_name::<$ty>();
            let ported = types
                .get_mut(&name)
                .ok_or_else(|| format!("{name} is in for_each_struct only"))?;
            ported.desc = Some(<$ty as GoStruct>::go_struct_desc());
        }};
    }
    for_each_struct!(desc);
    Ok(types)
}

/// A field as the metadata describes it.
fn go_field(field: &Value) -> TestResult<String> {
    let tag = field
        .get("json")
        .and_then(|json| json.get("raw"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let embedded = field.get("embedded").and_then(Value::as_bool) == Some(true);
    Ok(format!(
        "{} {} {} json:{tag:?} embedded:{embedded}",
        text(field, "name")?,
        text(field, "type")?,
        text(field, "kind")?,
    ))
}

/// A field as the port describes it.
fn rust_field(field: &FieldDesc) -> String {
    let mut ty = String::new();
    (field.type_name)(&mut ty);
    format!(
        "{} {ty} {} json:{:?} embedded:{}",
        field.go_name,
        kind_name(field.kind),
        field.tag,
        field.embedded.is_some()
    )
}

/// Compares one Go type with its port.
fn compare(go_name: &str, ty: &Value, port: &Ported, failures: &mut Vec<String>) -> TestResult {
    let kind = text(ty, "kind")?;
    if kind_name(port.kind) != kind {
        failures.push(format!(
            "{go_name}: kind {}, Go's is {kind}",
            kind_name(port.kind)
        ));
    }
    let Some(desc) = port.desc else {
        if kind == "struct" {
            failures.push(format!("{go_name}: a Go struct, not a Rust struct"));
        }
        return Ok(());
    };
    if desc.go_type != go_name || !desc.named {
        failures.push(format!("{go_name}: described as {}", desc.go_type));
    }
    // Go's JSON libraries see exported fields and embedded structs.
    let mut want = Vec::new();
    for field in list(ty, "fields")? {
        let exported = field.get("exported").and_then(Value::as_bool) == Some(true);
        let embedded = field.get("embedded").and_then(Value::as_bool) == Some(true);
        if exported || embedded {
            want.push(go_field(field)?);
        }
    }
    let got: Vec<String> = desc.fields.iter().map(rust_field).collect();
    if got != want {
        failures.push(format!("{go_name}: fields {got:#?}, Go's are {want:#?}"));
    }
    Ok(())
}

/// Every Go type is ported with its kind, and every struct with its fields, in order, with
/// their Go names, types, kinds, JSON tags and embedding; every Go alias is a re-export of the
/// type it aliases; nothing else is ported.
#[test]
fn types_and_fields() -> TestResult {
    let mut ported = ported()?;
    let mut failures = Vec::new();
    let mut aliases = Vec::new();
    for package in packages()? {
        let package_name = text(&package, "name")?;
        for ty in list(&package, "types")? {
            if let Some(target) = ty.get("alias_of").and_then(Value::as_str) {
                aliases.push((
                    format!("{package_name}.{}", text(ty, "name")?),
                    target.to_owned(),
                ));
                continue;
            }
            let go_name = text(ty, "go_name")?;
            match ported.remove(go_name) {
                Some(port) => compare(go_name, ty, &port, &mut failures)?,
                None => failures.push(format!("{go_name} is not ported")),
            }
        }
    }
    for go_name in ported.keys() {
        failures.push(format!("{go_name} is ported but not a Go type"));
    }
    ensure_eq(
        &aliases,
        &vec![(
            "request.Video".to_owned(),
            go_type_name::<native1::request::Video>(),
        )],
        "the Go aliases and the Go types of their Rust re-exports",
    )?;
    report("types and fields", &failures, 20)
}

/// The Go type and `json.Marshal` encoding of a constant.
fn constant<T: GoType + GoEncode>(value: &T) -> TestResult<(String, String)> {
    let json =
        stdjson::marshal(value).map_err(|e| String::from_utf8_lossy(&e.go_text()).into_owned())?;
    let json = String::from_utf8(json).map_err(|e| e.to_string())?;
    Ok((go_type_name::<T>(), json))
}

/// Every Go constant is ported with its Go type and value; nothing else is ported.
#[test]
fn constants() -> TestResult {
    let mut values = Vec::new();
    macro_rules! value {
        ($c:expr) => {
            values.push(constant(&$c)?);
        };
    }
    for_each_const!(value);
    ensure_eq(&values.len(), &CONSTANTS.len(), "CONSTANTS")?;
    let mut ported: BTreeMap<&str, (String, String)> =
        CONSTANTS.iter().copied().zip(values).collect();
    ensure_eq(&ported.len(), &CONSTANTS.len(), "distinct CONSTANTS")?;
    let mut failures = Vec::new();
    for package in packages()? {
        let package_name = text(&package, "name")?;
        for c in list(&package, "consts")? {
            let go_name = format!("{package_name}.{}", text(c, "name")?);
            let want = (
                text(c, "type")?.to_owned(),
                c.get("value").map_or_else(String::new, Value::to_string),
            );
            match ported.remove(go_name.as_str()) {
                Some(got) if got == want => {}
                Some(got) => failures.push(format!("{go_name}: {got:?}, Go's is {want:?}")),
                None => failures.push(format!("{go_name} is not ported")),
            }
        }
    }
    let untyped: BTreeMap<&str, (String, String)> = UNTYPED
        .iter()
        .map(|&(name, ty, value)| (name, (ty.to_owned(), value.to_owned())))
        .collect();
    ensure_eq(
        &ported,
        &untyped,
        "the ported constants the metadata does not list",
    )?;
    report("constants", &failures, 20)
}
