//! The serde code end to end: a described Go package in, its generated module out, a case per
//! mapping rule and per refusal.

use core::slice;
use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{config::Config, emit, model::Package, registry::Registry};
use crate::{
    error::{Result, ToolError},
    testutil::check,
};

/// The described package's import path.
const PKG: &str = "example.com/p";

fn basic(name: &str) -> Value {
    json!({"kind": "basic", "name": name, "reflect": name})
}

fn named(pkg: &str, pkg_name: &str, name: &str) -> Value {
    json!({
        "kind": "named",
        "name": name,
        "pkg": pkg,
        "pkg_name": pkg_name,
        "reflect": format!("{pkg_name}.{name}"),
    })
}

fn local(name: &str) -> Value {
    named(PKG, "p", name)
}

fn raw_message() -> Value {
    named("encoding/json", "json", "RawMessage")
}

fn pointer(elem: &Value) -> Value {
    json!({"kind": "pointer", "elem": elem, "reflect": "*T"})
}

fn slice_of(elem: &Value) -> Value {
    json!({"kind": "slice", "elem": elem, "reflect": "[]T"})
}

fn map_of(key: &Value, elem: &Value) -> Value {
    json!({"kind": "map", "key": key, "elem": elem, "reflect": "map[K]V"})
}

fn field(name: &str, ty: &Value, tag: Option<&str>) -> Value {
    json!({"name": name, "type": ty, "json": tag, "exported": true})
}

fn embedded(name: &str) -> Value {
    json!({"name": name, "type": local(name), "embedded": true, "exported": true})
}

fn struct_decl(name: &str, fields: &[Value]) -> Value {
    json!({"kind": "struct", "name": name, "file": "p.go", "exported": true, "fields": fields})
}

fn named_decl(name: &str, under: &Value) -> Value {
    json!({"kind": "named", "name": name, "file": "p.go", "exported": true, "type": under})
}

/// The module typegen generates for a package of `decls`, its package config extended with
/// `extra` (`overrides`, `strings`), serde on and the bridge `crate::gojson_bridge` when `bridge`
/// is set.
fn generate(decls: &[Value], extra: &Value, bridge: bool) -> Result<String> {
    let mut package = json!({
        "go": PKG,
        "rust": "p",
        "out": "src/p/generated.rs",
        "field_names": "json",
    });
    if let (Some(package), Some(extra)) = (package.as_object_mut(), extra.as_object()) {
        package.extend(extra.clone());
    }
    let mut config: Config = serde_json::from_value(json!({
        "crate": "pbs_example",
        "serde": true,
        "serde_bridge": bridge.then_some("crate::gojson_bridge"),
        "packages": [package],
    }))?;
    "crates/example/typegen.json".clone_into(&mut config.source);
    let pkg: Package = serde_json::from_value(json!({
        "path": PKG,
        "name": "p",
        "module": PKG,
        "version": "v1.0.0",
        "decls": decls,
    }))?;
    let packages = BTreeMap::from([(PKG.to_owned(), pkg.clone())]);
    let reg = Registry::build(slice::from_ref(&config), &packages)?;
    let pc = config
        .packages
        .first()
        .ok_or_else(|| ToolError::new("no package"))?;
    emit::package(&reg, &config, pc, &pkg).map(|g| g.content)
}

/// Fails unless `content` has `snippet`, lines joined.
fn check_has(content: &str, what: &str, snippet: &[&str]) -> Result<()> {
    let text = snippet.join("\n");
    if content.contains(&text) {
        Ok(())
    } else {
        Err(ToolError::new(format!(
            "{what}: the generated module has no\n{text}\n---\n{content}"
        )))
    }
}

/// Fails unless generating `decls` fails with a message containing `want`.
fn check_refused(decls: &[Value], extra: &Value, bridge: bool, want: &str) -> Result<()> {
    match generate(decls, extra, bridge) {
        Ok(content) => Err(ToolError::new(format!(
            "generated where it should refuse ({want}):\n{content}"
        ))),
        Err(e) if e.message().contains(want) => Ok(()),
        Err(e) => Err(ToolError::new(format!("refused with {e}, want {want}"))),
    }
}

/// The package of the mapping cases.
fn mapping_package() -> Vec<Value> {
    let string = || basic("string");
    vec![
        named_decl("Named", &basic("int")),
        named_decl("Small", &basic("int8")),
        struct_decl(
            "Inner",
            &[
                field("ID", &string(), Some("id,omitempty")),
                field("Name", &string(), Some("name")),
            ],
        ),
        struct_decl(
            "Plain",
            &[
                field("ID", &string(), Some("id,omitempty")),
                field("Count", &pointer(&basic("int")), Some("count,omitempty")),
                field("W", &pointer(&basic("int64")), Some("w,omitempty")),
                field("Tags", &slice_of(&string()), Some("tags,omitempty")),
                field(
                    "Labels",
                    &map_of(&string(), &string()),
                    Some("labels,omitempty"),
                ),
                field("N", &local("Named"), Some("n,omitempty")),
                field("S", &local("Small"), Some("s")),
                field(
                    "Any",
                    &json!({"kind": "interface", "reflect": "interface {}"}),
                    None,
                ),
                field("Skipped", &string(), Some("-")),
                field("NoTag", &basic("int8"), None),
                field("Child", &local("Inner"), Some("child,omitempty")),
                field("Ptr", &pointer(&local("Inner")), Some("ptr,omitempty")),
                field("Ext", &raw_message(), Some("ext,omitempty")),
                field("Raw", &raw_message(), Some("raw")),
            ],
        ),
        struct_decl(
            "Outer",
            &[
                embedded("Inner"),
                field("Domain", &string(), Some("domain,omitempty")),
                field("Ext", &raw_message(), Some("ext,omitempty")),
            ],
        ),
    ]
}

/// The mapping package's module.
fn mapping() -> Result<String> {
    let extra = json!({"overrides": {
        "Plain.Raw": {"reason": "decoded with encoding/json", "keep_null": true},
    }});
    generate(&mapping_package(), &extra, true)
}

#[test]
fn imports_what_the_serde_code_names() -> Result<()> {
    check_has(
        &mapping()?,
        "imports",
        &[
            "use pbs_gojson::{GoJson, RawJson};",
            "use pbs_jsonutil::{",
            "    MergeClone, MergeCloneError, is_nil_or_empty, is_zero, merge_field, merge_struct, \
             null_default,",
            "};",
            "use serde::{Deserialize, Serialize};",
            "use serde_json::{Value, value::RawValue};",
            "",
            "use crate::gojson_bridge;",
        ],
    )
}

#[test]
fn maps_named_types_to_transparent_newtypes() -> Result<()> {
    let content = mapping()?;
    // Go `int` merges through the bridge (pbs-jsonutil merges no isize), `int8` as itself.
    check_has(
        &content,
        "named int",
        &[
            "    Serialize,",
            "    Deserialize,",
            "    GoJson,",
            ")]",
            "#[serde(transparent)]",
            "#[gojson(go_type = \"p.Named\", map_key)]",
            "pub struct Named(pub isize);",
            "",
            "impl MergeClone for Named {",
            "    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {",
            "        gojson_bridge::merge(&mut self.0, patch)",
            "    }",
            "}",
        ],
    )?;
    check_has(
        &content,
        "named int8",
        &[
            "pub struct Small(pub i8);",
            "",
            "impl MergeClone for Small {",
            "    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {",
            "        self.0.merge_clone(patch)",
        ],
    )
}

/// Fails unless the mapping package's module has each case's snippet.
fn check_mapped(cases: &[(&str, &[&str])]) -> Result<()> {
    let content = mapping()?;
    for (what, snippet) in cases {
        check_has(&content, what, snippet)?;
    }
    Ok(())
}

#[test]
fn maps_values_and_holders_to_serde_attributes() -> Result<()> {
    check_mapped(&[
        (
            "container",
            &[
                "#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, GoJson)]",
                "#[serde(default)]",
                "#[gojson(go_type = \"p.Plain\")]",
            ],
        ),
        (
            "value with omitempty",
            &[
                "    #[serde(",
                "        rename = \"id\",",
                "        skip_serializing_if = \"is_zero\",",
                "        deserialize_with = \"null_default\"",
                "    )]",
                "    #[gojson(go = \"ID\", json = \"id,omitempty\")]",
            ],
        ),
        (
            "Go int pointer",
            &[
                "    #[serde(rename = \"count\", skip_serializing_if = \"Option::is_none\")]",
                "    #[gojson(go = \"Count\", json = \"count,omitempty\")]",
                "    pub count: Option<isize>,",
            ],
        ),
        (
            "slice",
            &["    #[serde(rename = \"tags\", skip_serializing_if = \"is_nil_or_empty\")]"],
        ),
        (
            "map",
            &[
                "    #[serde(rename = \"labels\", skip_serializing_if = \"is_nil_or_empty\")]",
                "    #[gojson(go = \"Labels\", json = \"labels,omitempty\")]",
                "    pub labels: Option<BTreeMap<String, String>>,",
            ],
        ),
        (
            "named value without omitempty",
            &[
                "    #[serde(rename = \"s\", deserialize_with = \"null_default\")]",
                "    #[gojson(go = \"S\", json = \"s\")]",
                "    pub s: Small,",
            ],
        ),
        (
            "interface{} without a tag",
            &[
                "    #[serde(rename = \"Any\")]",
                "    #[gojson(go = \"Any\")]",
                "    pub any: Value,",
            ],
        ),
    ])
}

#[test]
fn maps_skips_structs_and_raw_messages_to_serde_attributes() -> Result<()> {
    check_mapped(&[
        (
            "json:\"-\"",
            &[
                "    #[serde(skip)]",
                "    #[gojson(go = \"Skipped\", json = \"-\")]",
            ],
        ),
        (
            "value without a tag",
            &["    #[serde(rename = \"NoTag\", deserialize_with = \"null_default\")]"],
        ),
        (
            "struct, which omitempty never omits",
            &[
                "    #[serde(rename = \"child\", deserialize_with = \"null_default\")]",
                "    #[gojson(go = \"Child\", json = \"child,omitempty\")]",
                "    pub child: Inner,",
            ],
        ),
        (
            "pointer to a struct",
            &[
                "    #[serde(rename = \"ptr\", skip_serializing_if = \"Option::is_none\")]",
                "    #[gojson(go = \"Ptr\", json = \"ptr,omitempty\")]",
                "    pub ptr: Option<Box<Inner>>,",
            ],
        ),
        (
            "json.RawMessage",
            &[
                "    #[serde(",
                "        rename = \"ext\",",
                "        skip_serializing_if = \"gojson_bridge::raw_json::is_empty\",",
                "        with = \"gojson_bridge::raw_json\"",
                "    )]",
            ],
        ),
        (
            "json.RawMessage with keep_null",
            &[
                "    #[serde(rename = \"raw\", with = \"gojson_bridge::raw_json_keep_null\")]",
                "    #[gojson(go = \"Raw\", json = \"raw\")]",
                "    pub raw: RawJson,",
            ],
        ),
    ])
}

#[test]
fn merges_struct_members_into_their_fields() -> Result<()> {
    check_has(
        &mapping()?,
        "merge",
        &[
            "impl MergeClone for Plain {",
            "    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {",
            "        const GO_TYPE: &str = \"p.Plain\";",
            "        merge_struct(patch, |key, value| match key {",
            "            \"id\" => merge_field(&mut self.id, value, GO_TYPE, \"ID\"),",
            "            \"count\" => gojson_bridge::merge_field(&mut self.count, value, GO_TYPE, \
             \"Count\"),",
            "            \"w\" => merge_field(&mut self.w, value, GO_TYPE, \"W\"),",
            "            \"tags\" => merge_field(&mut self.tags, value, GO_TYPE, \"Tags\"),",
            "            \"labels\" => merge_field(&mut self.labels, value, GO_TYPE, \"Labels\"),",
            "            \"n\" => merge_field(&mut self.n, value, GO_TYPE, \"N\"),",
            "            \"s\" => merge_field(&mut self.s, value, GO_TYPE, \"S\"),",
            "            \"Any\" => merge_field(&mut self.any, value, GO_TYPE, \"Any\"),",
            "            \"NoTag\" => merge_field(&mut self.no_tag, value, GO_TYPE, \"NoTag\"),",
            "            \"child\" => merge_field(&mut self.child, value, GO_TYPE, \"Child\"),",
            "            \"ptr\" => merge_field(&mut self.ptr, value, GO_TYPE, \"Ptr\"),",
            "            \"ext\" => gojson_bridge::merge_field(&mut self.ext, value, GO_TYPE, \
             \"Ext\"),",
            "            \"raw\" => gojson_bridge::merge_field(&mut self.raw, value, GO_TYPE, \
             \"Raw\"),",
            "            _ => Ok(()),",
            "        })",
            "    }",
            "}",
        ],
    )
}

#[test]
fn reads_promoted_members_directly() -> Result<()> {
    let content = mapping()?;
    check_has(
        &content,
        "embedding struct",
        &[
            "#[serde(from = \"OuterFields\")]",
            "#[gojson(go_type = \"p.Outer\")]",
            "pub struct Outer {",
            "    /// Go field `Inner`.",
            "    #[serde(flatten)]",
            "    #[gojson(go = \"Inner\", embedded)]",
            "    pub inner: Inner,",
            "    /// Go field `Domain`.",
            "    #[serde(rename = \"domain\", skip_serializing_if = \"is_zero\")]",
        ],
    )?;
    check_has(
        &content,
        "its serialize-only bridge",
        &["        serialize_with = \"gojson_bridge::raw_json::serialize\""],
    )?;
    check_has(
        &content,
        "promoted merges",
        &[
            "            \"id\" => merge_field(&mut self.inner.id, value, \"p.Inner\", \"ID\"),",
            "            \"name\" => merge_field(&mut self.inner.name, value, \"p.Inner\", \
             \"Name\"),",
            "            \"domain\" => merge_field(&mut self.domain, value, GO_TYPE, \"Domain\"),",
        ],
    )?;
    check_has(
        &content,
        "members",
        &[
            "#[derive(Default, Deserialize)]",
            "#[serde(default)]",
            "struct OuterFields {",
            "    #[serde(rename = \"id\", deserialize_with = \"null_default\")]",
            "    id: String,",
            "    #[serde(rename = \"name\", deserialize_with = \"null_default\")]",
            "    name: String,",
            "    #[serde(rename = \"domain\", deserialize_with = \"null_default\")]",
            "    domain: String,",
            "    #[serde(",
            "        rename = \"ext\",",
            "        deserialize_with = \"gojson_bridge::raw_json::deserialize\"",
            "    )]",
            "    ext: RawJson,",
            "}",
            "",
            "impl From<OuterFields> for Outer {",
            "    fn from(fields: OuterFields) -> Self {",
            "        Self {",
            "            inner: Inner {",
            "                id: fields.id,",
            "                name: fields.name,",
            "            },",
            "            domain: fields.domain,",
            "            ext: fields.ext,",
            "        }",
            "    }",
            "}",
        ],
    )
}

/// A refusal case: the declarations, the package config's extras, whether the config names a
/// bridge, and what the refusal says.
type Refusal<'a> = (Vec<Value>, Value, bool, &'a str);

/// Fails unless each case is refused with its message.
fn check_refusals(cases: Vec<Refusal<'_>>) -> Result<()> {
    for (decls, extra, bridge, want) in cases {
        check_refused(&decls, &extra, bridge, want)?;
    }
    Ok(())
}

/// The struct the embedding cases embed.
fn inner() -> Value {
    struct_decl("Inner", &[field("ID", &basic("string"), Some("id"))])
}

#[test]
fn refuses_fields_serde_cannot_hold() -> Result<()> {
    let none = json!({});
    let string = || basic("string");
    check_refusals(vec![
        (
            vec![struct_decl("Empty", &[])],
            none.clone(),
            true,
            "has no fields",
        ),
        (
            vec![struct_decl(
                "Twice",
                &[
                    field("A", &string(), Some("k")),
                    field("B", &string(), Some("k,omitempty")),
                ],
            )],
            json!({"overrides": {"Twice.B": {"reason": "r", "rust_name": "b"}}}),
            true,
            "two fields with JSON key \"k\"",
        ),
        (
            vec![struct_decl(
                "Quoted",
                &[field("N", &basic("int64"), Some("n,string"))],
            )],
            none.clone(),
            true,
            "the json option \"string\" has no serde mapping",
        ),
        (
            vec![struct_decl("Kept", &[field("S", &string(), Some("s"))])],
            json!({"overrides": {"Kept.S": {"reason": "r", "keep_null": true}}}),
            true,
            "keep_null is for json.RawMessage fields",
        ),
        (
            vec![struct_decl(
                "Raws",
                &[field("R", &slice_of(&raw_message()), Some("r"))],
            )],
            none.clone(),
            true,
            "a bridged type inside a pointer, slice or map",
        ),
        (
            vec![struct_decl("Bytes", &[field("S", &string(), Some("s"))])],
            json!({"strings": "GoString"}),
            true,
            "GoString strings have no serde mapping",
        ),
        (
            vec![struct_decl(
                "Ints",
                &[field("N", &slice_of(&basic("int")), Some("n"))],
            )],
            none.clone(),
            true,
            "a Go int or uint inside a slice or map",
        ),
        (
            vec![struct_decl(
                "Counts",
                &[field("N", &map_of(&string(), &basic("uint")), Some("n"))],
            )],
            none.clone(),
            true,
            "a Go int or uint inside a slice or map",
        ),
        (
            vec![struct_decl(
                "Maybe",
                &[field("N", &slice_of(&pointer(&basic("int"))), Some("n"))],
            )],
            none.clone(),
            true,
            "a Go int or uint inside a slice or map",
        ),
    ])?;
    // A Go `int` key needs no merge of its own; `int64` elements merge as themselves.
    let content = generate(
        &[struct_decl(
            "Keyed",
            &[
                field("K", &map_of(&basic("int"), &string()), Some("k")),
                field("W", &slice_of(&basic("int64")), Some("w")),
            ],
        )],
        &none,
        true,
    )?;
    check_has(
        &content,
        "Go int keys and int64 elements",
        &["    pub k: Option<BTreeMap<isize, String>>,"],
    )?;
    check_has(
        &content,
        "Go int keys and int64 elements",
        &["            \"k\" => merge_field(&mut self.k, value, GO_TYPE, \"K\"),"],
    )
}

#[test]
fn refuses_types_serde_cannot_hold() -> Result<()> {
    let none = json!({});
    check_refusals(vec![
        (
            vec![
                inner(),
                struct_decl(
                    "Hides",
                    &[
                        embedded("Inner"),
                        field("Other", &basic("string"), Some("id")),
                    ],
                ),
            ],
            none.clone(),
            true,
            "two fields with JSON key \"id\"",
        ),
        (
            vec![named_decl("Raw", &raw_message())],
            none.clone(),
            true,
            "a named type of a bridged type",
        ),
        (
            vec![named_decl("Strings", &slice_of(&basic("string")))],
            none.clone(),
            true,
            "a named type of a slice, map or struct",
        ),
        (
            vec![inner(), named_decl("Again", &local("Inner"))],
            none.clone(),
            true,
            "a named type of a slice, map or struct",
        ),
        (
            vec![
                inner(),
                struct_decl(
                    "ByPointer",
                    &[json!({
                        "name": "Inner",
                        "type": pointer(&local("Inner")),
                        "embedded": true,
                        "exported": true,
                    })],
                ),
            ],
            none.clone(),
            true,
            "which is not a struct of its package",
        ),
        (
            vec![
                inner(),
                struct_decl("Middle", &[embedded("Inner")]),
                struct_decl("Deep", &[embedded("Middle")]),
            ],
            none.clone(),
            true,
            "serde code promotes one level",
        ),
        (
            vec![struct_decl("Ext", &[field("E", &raw_message(), Some("e"))])],
            none.clone(),
            false,
            "needs the config's serde_bridge module",
        ),
        (
            vec![struct_decl("Wide", &[field("N", &basic("int"), Some("n"))])],
            none.clone(),
            false,
            "needs the config's serde_bridge module",
        ),
    ])?;
    check(
        generate(&[inner()], &none, false).is_ok(),
        "a package without bridged types needs no bridge",
    )
}
