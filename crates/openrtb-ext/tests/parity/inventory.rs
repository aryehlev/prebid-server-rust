//! Every Go type, struct field, JSON tag, constant and function of `openrtb_ext`, against Go.
//!
//! The Go side is the reflect and `go/types` metadata in `testdata/go_struct_meta.json`,
//! the Go type index that typegen writes (`testdata/go/ported_types_gen.go`), the untyped
//! constants of `constants.tsv`, and the function ports that `typegen.json` registers.

use std::collections::{BTreeMap, BTreeSet};

use pbs_gojson::{
    GoEncode, GoKind, GoType,
    gotype::go_type_name,
    stdjson,
    structs::{FieldDesc, GoStruct, StructDesc},
};
use pbs_openrtb_ext::openrtb_ext;
use serde_json::Value;

use super::{
    index::{
        CONSTANTS, FUNCTIONS, TYPES, for_each_const, for_each_struct, for_each_type,
        for_each_untyped_const,
    },
    support::{TestResult, crate_dir, ensure_eq, golden_rows, read, report, workspace_root},
};

/// The Go types of `request_wrapper.go`, which a separate unit ports (they hold unexported
/// state, not JSON).
const WRAPPER_TYPES: [&str; 11] = [
    "openrtb_ext.AppExt",
    "openrtb_ext.DOOHExt",
    "openrtb_ext.DeviceExt",
    "openrtb_ext.ImpExt",
    "openrtb_ext.ImpWrapper",
    "openrtb_ext.RegExt",
    "openrtb_ext.RequestExt",
    "openrtb_ext.RequestWrapper",
    "openrtb_ext.SiteExt",
    "openrtb_ext.SourceExt",
    "openrtb_ext.UserExt",
];

/// The Go types that no JSON document decodes into, and how each is ported.
const NOT_DECODED: [(&str, &str); 6] = [
    (
        "openrtb_ext.BidderNameNormalizer",
        "a function type: the `BidderNameNormalizer` fn pointer type",
    ),
    (
        "openrtb_ext.BidderParamValidator",
        "an interface: the `pbs_bidder_params::BidderParamValidator` trait, re-exported",
    ),
    (
        "openrtb_ext.bidderParamValidator",
        "its implementation: `pbs_bidder_params::StandardBidderParamValidator`, re-exported",
    ),
    (
        "openrtb_ext.bidderParamsFileSystem",
        "the file system seam of Go's tests: the port reads the file system",
    ),
    (
        "openrtb_ext.standardBidderParamsFileSystem",
        "the file system seam of Go's tests: the port reads the file system",
    ),
    (
        "openrtb_ext.extImpAppnexusKeyVal",
        "a private helper of `ExtImpAppnexusKeywords`'s decoding (tested through it)",
    ),
];

/// A port of each Go type of [`NOT_DECODED`] that exists in Rust.
const _NOT_DECODED_PORTS: (
    openrtb_ext::BidderNameNormalizer,
    Option<&dyn openrtb_ext::BidderParamValidator>,
    Option<&openrtb_ext::StandardBidderParamValidator>,
) = (openrtb_ext::normalize_bidder_name, None, None);

/// Untyped Go constants that the port gives another type than Go's default for the constant
/// (`typegen.json` says why).
const UNTYPED_AS: [(&str, &str); 1] = [("openrtb_ext.NativeExchangeSpecificLowerBound", "int64")];

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

/// The metadata of `openrtb_ext`.
fn package() -> TestResult<Value> {
    let path = workspace_root()?.join("testdata/go_struct_meta.json");
    let meta: Value = serde_json::from_slice(&read(&path)?).map_err(|e| e.to_string())?;
    list(&meta, "packages")?
        .iter()
        .find(|p| {
            text(p, "path").is_ok_and(|import| import.ends_with("/prebid-server/v4/openrtb_ext"))
        })
        .cloned()
        .ok_or_else(|| "no openrtb_ext package in the metadata".to_owned())
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
/// their Go names, types, kinds, JSON tags and embedding; the types JSON never decodes into
/// and `request_wrapper.go`'s are the documented exceptions; nothing else is ported (the
/// unnamed struct types of fields are compared as their fields' types).
#[test]
fn types_and_fields() -> TestResult {
    let mut ported = ported()?;
    let mut failures = Vec::new();
    let exceptions: BTreeSet<&str> = WRAPPER_TYPES
        .iter()
        .copied()
        .chain(NOT_DECODED.iter().map(|&(name, _)| name))
        .collect();
    let package = package()?;
    for ty in list(&package, "types")? {
        if ty.get("alias_of").and_then(Value::as_str).is_some() {
            failures.push(format!("{}: an alias, not in the port", text(ty, "name")?));
            continue;
        }
        let go_name = text(ty, "go_name")?;
        match ported.remove(go_name) {
            Some(port) => compare(go_name, ty, &port, &mut failures)?,
            None if exceptions.contains(go_name) => {}
            None => failures.push(format!("{go_name} is not ported")),
        }
    }
    for go_name in ported.keys() {
        if !go_name.starts_with("struct {") {
            failures.push(format!("{go_name} is ported but not a Go type"));
        }
    }
    report("types and fields", &failures, 20)
}

/// The index lists the types of typegen's Go index (`testdata/go/ported_types_gen.go`) but
/// functions, interfaces and `request_wrapper.go`'s, with the same Rust paths, in its order.
#[test]
fn index_matches_typegen() -> TestResult {
    let path = crate_dir().join("testdata/go/ported_types_gen.go");
    let source = String::from_utf8(read(&path)?).map_err(|e| e.to_string())?;
    let mut go_index = Vec::new();
    for line in source.lines() {
        let Some(entry) = line.trim().strip_prefix("{reflect.TypeOf((*openrtb_ext.") else {
            continue;
        };
        let (go, tail) = entry
            .split_once(')')
            .ok_or("bad ported_types_gen.go line")?;
        let rust = tail
            .split('"')
            .nth(1)
            .ok_or("bad ported_types_gen.go line")?;
        go_index.push((format!("openrtb_ext.{go}"), rust.to_owned()));
    }
    let skipped: BTreeSet<&str> = WRAPPER_TYPES
        .iter()
        .copied()
        .chain(["openrtb_ext.BidderNameNormalizer"])
        .collect();
    let want: Vec<String> = go_index
        .iter()
        .filter(|(go, _)| !skipped.contains(go.as_str()))
        .map(|(go, _)| go.clone())
        .collect();
    let got: Vec<String> = TYPES
        .iter()
        .take(want.len())
        .map(|&name| name.to_owned())
        .collect();
    ensure_eq(&got, &want, "the ported Go types, in typegen's order")?;
    let mut paths = Vec::new();
    macro_rules! path {
        ($ty:ty) => {
            paths.push(
                stringify!($ty)
                    .replace(' ', "")
                    .replace("openrtb_ext::", ""),
            );
        };
    }
    for_each_type!(path);
    let want_paths: Vec<String> = go_index
        .iter()
        .filter(|(go, _)| !skipped.contains(go.as_str()))
        .map(|(_, rust)| rust.replace("openrtb_ext::", ""))
        .collect();
    let got_paths: Vec<String> = paths.into_iter().take(want_paths.len()).collect();
    ensure_eq(
        &got_paths,
        &want_paths,
        "the Rust paths of the ported Go types",
    )
}

/// The Go type and `json.Marshal` encoding of a constant.
fn constant<T: GoType + GoEncode>(value: &T) -> TestResult<(String, String)> {
    let json =
        stdjson::marshal(value).map_err(|e| String::from_utf8_lossy(&e.go_text()).into_owned())?;
    let json = String::from_utf8(json).map_err(|e| e.to_string())?;
    Ok((go_type_name::<T>(), json))
}

/// Every typed Go constant is ported with its Go type and value; nothing else is ported.
#[test]
fn typed_constants() -> TestResult {
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
    let package = package()?;
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
    for go_name in ported.keys() {
        failures.push(format!("{go_name} is ported but not a typed Go constant"));
    }
    report("typed constants", &failures, 20)
}

/// Every untyped Go constant (`constants.tsv`) is ported with its value, as Go's default type
/// for it or the type [`UNTYPED_AS`] documents; nothing else is ported.
#[test]
fn untyped_constants() -> TestResult {
    let mut ported: BTreeMap<String, (String, String)> = BTreeMap::new();
    macro_rules! value {
        ($go:expr, $c:expr) => {{
            let (ty, _) = constant(&$c)?;
            let _replaced = ported.insert($go.to_owned(), (ty, $c.to_string()));
        }};
    }
    for_each_untyped_const!(value);
    let retyped: BTreeMap<&str, &str> = UNTYPED_AS.iter().copied().collect();
    let mut failures = Vec::new();
    for row in golden_rows("constants.tsv")? {
        let [name, ty, value] = row.as_slice() else {
            continue;
        };
        let name = String::from_utf8_lossy(name).into_owned();
        let ty = String::from_utf8_lossy(ty).into_owned();
        let want_ty = retyped.get(name.as_str()).map_or(ty, |&t| t.to_owned());
        let want = (want_ty, String::from_utf8_lossy(value).into_owned());
        match ported.remove(&name) {
            Some(got) if got == want => {}
            Some(got) => failures.push(format!("{name}: {got:?}, Go's is {want:?}")),
            None => failures.push(format!("{name} is not ported")),
        }
    }
    for name in ported.keys() {
        failures.push(format!("{name} is ported but not an untyped Go constant"));
    }
    report("untyped constants", &failures, 20)
}

/// The index checks the signature of every function port that `typegen.json` registers (but
/// those of the `request_wrapper.go` and convert ports, which a separate unit writes).
#[test]
fn function_ports() -> TestResult {
    let config: Value = serde_json::from_slice(&read(&crate_dir().join("typegen.json"))?)
        .map_err(|e| e.to_string())?;
    let functions = config
        .get("packages")
        .and_then(Value::as_array)
        .and_then(|packages| packages.first())
        .and_then(|package| package.get("functions"))
        .and_then(Value::as_object)
        .ok_or("typegen.json has no functions")?;
    let registered: BTreeSet<&str> = functions
        .iter()
        .filter(|(_, port)| {
            port.get("signature")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.starts_with("TBD"))
        })
        .map(|(name, _)| name.as_str())
        .collect();
    let checked: BTreeSet<&str> = FUNCTIONS.iter().copied().collect();
    ensure_eq(&checked, &registered, "the function ports the index checks")?;
    ensure_eq(&checked.len(), &FUNCTIONS.len(), "distinct FUNCTIONS")
}
