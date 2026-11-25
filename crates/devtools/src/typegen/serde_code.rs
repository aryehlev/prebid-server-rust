//! The serde code of a generated module (see [`super::emit`]).
//!
//! Derives, field attributes, `MergeClone` impls, and the members a struct that embeds another
//! reads, each laid out as rustfmt lays it out with the workspace's `rustfmt.toml`.

use std::collections::BTreeSet;

use super::{
    docs::WIDTH,
    emit::{literal, meta_list},
    registry::{Omit, SerdeField},
};
use crate::error::{Result, ToolError};

/// rustfmt's `fn_call_width`: a call whose arguments are longer puts them one per line.
const FN_CALL_WIDTH: usize = 60;

/// The longest `#[derive(...)]` rustfmt keeps on one line.
const DERIVE_WIDTH: usize = 96;

/// rustfmt's `struct_lit_width`: a struct literal whose fields are longer puts them one per line.
const STRUCT_LIT_WIDTH: usize = 18;

/// The indent of a `merge_struct` match arm.
const ARM_INDENT: usize = 12;

/// The bridge module of a `json.RawMessage` (see `registry`'s built-in extern types), the only
/// type `keep_null` applies to.
pub const RAW_JSON: &str = "raw_json";

/// `#[derive(...)]` of `derives`: on one line when it fits rustfmt's width for it, else the list
/// on one indented line, else one derive per line.
#[must_use]
pub fn derive(derives: &[&str]) -> Vec<String> {
    let joined = derives.join(", ");
    let line = format!("#[derive({joined})]");
    if line.len() <= DERIVE_WIDTH {
        return vec![line];
    }
    let inner = format!("    {joined},");
    if inner.len() <= WIDTH {
        return vec!["#[derive(".to_owned(), inner, ")]".to_owned()];
    }
    let mut out = vec!["#[derive(".to_owned()];
    out.extend(derives.iter().map(|d| format!("    {d},")));
    out.push(")]".to_owned());
    out
}

/// One JSON member of a generated struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The JSON key, `None` for `json:"-"`.
    pub key: Option<String>,
    /// The Rust field as written (`r#pub`).
    pub field: String,
    /// The Rust type.
    pub ty: String,
    /// Go's name of the struct that declares the field (`adcom1.DistributionChannel`), which
    /// merge errors name.
    pub go_type: String,
    /// The field's Go name.
    pub go_field: String,
    /// The tag's `omitempty`.
    pub omitempty: bool,
    /// What the field's Go type needs.
    pub serde: SerdeField,
    /// Read null as the bytes `null`, as `encoding/json` does (`keep_null`).
    pub keep_null: bool,
}

/// What one field of a generated struct is to the serde code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    /// A field that is one JSON member.
    Own(Member),
    /// An embedded struct, whose members Go promotes into the struct's own.
    Embedded {
        /// The Rust field as written.
        field: String,
        /// The embedded struct's Rust type.
        ty: String,
        /// Its members.
        members: Vec<Member>,
    },
}

impl Slot {
    /// The JSON members of the field, each with its Rust path from `self`.
    #[must_use]
    pub fn members(&self) -> Vec<(String, &Member)> {
        match self {
            Self::Own(m) => vec![(m.field.clone(), m)],
            Self::Embedded { field, members, .. } => members
                .iter()
                .map(|m| (format!("{field}.{}", m.field), m))
                .collect(),
        }
    }
}

/// Which derive a field attribute is for.
///
/// A struct derives both, unless its `Deserialize` goes through the members its embedded structs
/// promote: then its own fields serialize, and the private struct of [`mirror`] deserializes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// `Serialize` and `Deserialize`.
    Both,
    /// `Serialize` only.
    Serialize,
    /// `Deserialize` only.
    Deserialize,
}

/// The bridge module `module` of the config's `serde_bridge` (its last segment, which the
/// generated module imports), recording the import.
fn bridge_name<'a>(
    bridge: Option<&'a str>,
    what: &str,
    uses: &mut BTreeSet<String>,
) -> Result<&'a str> {
    let path = bridge.ok_or_else(|| {
        ToolError::new(format!(
            "{what} needs the config's serde_bridge module, which is not set"
        ))
    })?;
    let _ = uses.insert(path.to_owned());
    Ok(path.rsplit("::").next().unwrap_or(path))
}

/// The arguments of a field's `#[serde(...)]` attribute, recording the imports they need.
///
/// # Errors
///
/// When the field needs the bridge and the config has none.
pub fn field_args(
    m: &Member,
    side: Side,
    bridge: Option<&str>,
    uses: &mut BTreeSet<String>,
) -> Result<Vec<String>> {
    let Some(key) = &m.key else {
        return Ok(vec!["skip".to_owned()]);
    };
    let mut args = vec![format!("rename = {}", literal(key))];
    let module = match &m.serde.with {
        Some(module) => {
            let name = bridge_name(bridge, &format!("field {}", m.go_field), uses)?;
            let suffix = if m.keep_null { "_keep_null" } else { "" };
            Some(format!("{name}::{module}{suffix}"))
        }
        None => None,
    };
    if side != Side::Deserialize && m.omitempty {
        let test = match (m.serde.omit, &module) {
            (Omit::Never, _) => None,
            (Omit::Zero, _) => {
                let _ = uses.insert("pbs_jsonutil::is_zero".to_owned());
                Some("is_zero".to_owned())
            }
            (Omit::Nil, _) => Some("Option::is_none".to_owned()),
            (Omit::NilOrEmpty, _) => {
                let _ = uses.insert("pbs_jsonutil::is_nil_or_empty".to_owned());
                Some("is_nil_or_empty".to_owned())
            }
            (Omit::Bridge, Some(module)) => Some(format!("{module}::is_empty")),
            (Omit::Bridge, None) => {
                return Err(ToolError::new(format!(
                    "field {}: a bridged type without a bridge module",
                    m.go_field
                )));
            }
        };
        if let Some(test) = test {
            args.push(format!("skip_serializing_if = {}", literal(&test)));
        }
    }
    match (side, module) {
        (Side::Both, Some(module)) => args.push(format!("with = {}", literal(&module))),
        (Side::Serialize, Some(module)) => args.push(format!(
            "serialize_with = {}",
            literal(&format!("{module}::serialize"))
        )),
        (Side::Deserialize, Some(module)) => args.push(format!(
            "deserialize_with = {}",
            literal(&format!("{module}::deserialize"))
        )),
        (Side::Both | Side::Deserialize, None) if m.serde.value => {
            let _ = uses.insert("pbs_jsonutil::null_default".to_owned());
            args.push("deserialize_with = \"null_default\"".to_owned());
        }
        (Side::Both | Side::Deserialize | Side::Serialize, None) => {}
    }
    Ok(args)
}

/// A `merge_struct` match arm calling `callee(args)`, broken as rustfmt breaks it.
///
/// It is on one line when the arguments fit rustfmt's `fn_call_width` and the line its width, in
/// a block when only the line is too long, else with one argument per line.
#[must_use]
pub fn call_arm(pattern: &str, callee: &str, args: &[String]) -> Vec<String> {
    let pad = " ".repeat(ARM_INDENT);
    let joined = args.join(", ");
    if joined.len() <= FN_CALL_WIDTH {
        let call = format!("{callee}({joined})");
        let line = format!("{pad}{pattern} => {call},");
        if line.len() <= WIDTH {
            return vec![line];
        }
        let body = format!("{pad}    {call}");
        if body.len() <= WIDTH {
            return vec![format!("{pad}{pattern} => {{"), body, format!("{pad}}}")];
        }
    }
    let mut out = vec![format!("{pad}{pattern} => {callee}(")];
    out.extend(args.iter().map(|a| format!("{pad}    {a},")));
    out.push(format!("{pad}),"));
    out
}

/// The first lines of a `MergeClone` impl, recording the imports it needs.
fn merge_head(rust: &str, uses: &mut BTreeSet<String>) -> Vec<String> {
    for path in [
        "pbs_jsonutil::MergeClone",
        "pbs_jsonutil::MergeCloneError",
        "serde_json::value::RawValue",
    ] {
        let _ = uses.insert(path.to_owned());
    }
    vec![
        format!("impl MergeClone for {rust} {{"),
        "    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {"
            .to_owned(),
    ]
}

/// The `MergeClone` impl of a struct whose fields are `slots` (Go's name `go_type`).
///
/// Each JSON member merges into its field with `merge_field` (the bridge's for a field that holds
/// a bridged type or a Go `int` or `uint`), naming the Go struct that declares it; other keys are
/// ignored, as Go ignores them.
///
/// # Errors
///
/// When a member needs the bridge and the config has none.
pub fn merge_impl(
    rust: &str,
    go_type: &str,
    slots: &[Slot],
    bridge: Option<&str>,
    uses: &mut BTreeSet<String>,
) -> Result<Vec<String>> {
    let mut out = merge_head(rust, uses);
    let _ = uses.insert("pbs_jsonutil::merge_struct".to_owned());
    let mut arms = Vec::new();
    let mut own_type = false;
    for slot in slots {
        for (path, m) in slot.members() {
            let Some(key) = &m.key else {
                continue;
            };
            let callee = if m.serde.merge_bridge {
                let name = bridge_name(bridge, &format!("merging field {}", m.go_field), uses)?;
                format!("{name}::merge_field")
            } else {
                let _ = uses.insert("pbs_jsonutil::merge_field".to_owned());
                "merge_field".to_owned()
            };
            let ty = if m.go_type == go_type {
                own_type = true;
                "GO_TYPE".to_owned()
            } else {
                literal(&m.go_type)
            };
            let args = [
                format!("&mut self.{path}"),
                "value".to_owned(),
                ty,
                literal(&m.go_field),
            ];
            arms.extend(call_arm(&literal(key), &callee, &args));
        }
    }
    if arms.is_empty() {
        out.push("        merge_struct(patch, |_, _| Ok(()))".to_owned());
    } else {
        if own_type {
            out.push(format!(
                "        const GO_TYPE: &str = {};",
                literal(go_type)
            ));
        }
        out.push("        merge_struct(patch, |key, value| match key {".to_owned());
        out.extend(arms);
        out.push("            _ => Ok(()),".to_owned());
        out.push("        })".to_owned());
    }
    out.extend(["    }".to_owned(), "}".to_owned()]);
    Ok(out)
}

/// The `MergeClone` impl of a named type: it merges as its underlying type (through the bridge
/// for a Go `int` or `uint`).
///
/// # Errors
///
/// When the underlying type needs the bridge and the config has none.
pub fn named_merge_impl(
    rust: &str,
    bridged: bool,
    bridge: Option<&str>,
    uses: &mut BTreeSet<String>,
) -> Result<Vec<String>> {
    let mut out = merge_head(rust, uses);
    if bridged {
        let name = bridge_name(bridge, &format!("merging {rust}"), uses)?;
        out.push(format!("        {name}::merge(&mut self.0, patch)"));
    } else {
        out.push("        self.0.merge_clone(patch)".to_owned());
    }
    out.extend(["    }".to_owned(), "}".to_owned()]);
    Ok(out)
}

/// The Rust name of the private struct [`mirror`] writes for `rust`.
#[must_use]
pub fn mirror_name(rust: &str) -> String {
    format!("{rust}Fields")
}

/// The private struct a struct that embeds another (`rust`, its fields `slots`) deserializes
/// from, and the `From` impl that builds the struct from it.
///
/// The private struct has every JSON member the struct reads, the promoted ones in their place,
/// and the struct's `#[serde(from = ...)]` names it: `#[serde(flatten)]` would read the promoted
/// members through serde's buffer, which cannot hold a `RawValue`.
///
/// # Errors
///
/// When two members have the same Rust name, or a member needs the bridge and the config has
/// none.
pub fn mirror(
    rust: &str,
    slots: &[Slot],
    bridge: Option<&str>,
    uses: &mut BTreeSet<String>,
) -> Result<[Vec<String>; 2]> {
    let name = mirror_name(rust);
    let mut item = wrap_doc(&format!(
        "The JSON members of [`{rust}`], those it promotes from its embedded structs included, for \
         its `Deserialize`: `#[serde(flatten)]` would read them through serde's buffer, which \
         cannot hold a `RawValue`."
    ));
    item.extend([
        "#[derive(Default, Deserialize)]".to_owned(),
        "#[serde(default)]".to_owned(),
        format!("struct {name} {{"),
    ]);
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    for slot in slots {
        for (_, m) in slot.members() {
            if !seen.insert(m.field.clone()) {
                return Err(ToolError::new(format!(
                    "{rust}: two members become field {} of {name}; rename one",
                    m.field
                )));
            }
            item.extend(meta_list(
                4,
                "serde",
                &field_args(m, Side::Deserialize, bridge, uses)?,
            ));
            let line = format!("    {}: {},", m.field, m.ty);
            if line.len() <= WIDTH {
                item.push(line);
            } else {
                item.extend([format!("    {}:", m.field), format!("        {},", m.ty)]);
            }
        }
        entries.push(match slot {
            Slot::Own(m) => Entry::Plain(format!("{}: fields.{}", m.field, m.field)),
            Slot::Embedded { field, ty, members } => Entry::Nested {
                field: field.clone(),
                ty: ty.clone(),
                fields: members
                    .iter()
                    .map(|m| format!("{}: fields.{}", m.field, m.field))
                    .collect(),
            },
        });
    }
    item.push("}".to_owned());
    let mut from = vec![
        format!("impl From<{name}> for {rust} {{"),
        format!("    fn from(fields: {name}) -> Self {{"),
    ];
    from.extend(struct_literal(8, "Self", &entries));
    from.extend(["    }".to_owned(), "}".to_owned()]);
    Ok([item, from])
}

/// A field of a struct literal.
enum Entry {
    /// `field: value`.
    Plain(String),
    /// `field: Type { ... }`.
    Nested {
        /// The field.
        field: String,
        /// The struct type.
        ty: String,
        /// Its `field: value` entries.
        fields: Vec<String>,
    },
}

/// A struct literal `name { entries }` at `indent` (the expression of a body): on one line when
/// its fields fit rustfmt's `struct_lit_width`, else one field per line.
fn struct_literal(indent: usize, name: &str, entries: &[Entry]) -> Vec<String> {
    let pad = " ".repeat(indent);
    let plain: Option<Vec<&str>> = entries
        .iter()
        .map(|e| match e {
            Entry::Plain(text) => Some(text.as_str()),
            Entry::Nested { .. } => None,
        })
        .collect();
    if let Some(plain) = plain {
        let joined = plain.join(", ");
        let line = format!("{pad}{name} {{ {joined} }}");
        if joined.len() <= STRUCT_LIT_WIDTH && line.len() <= WIDTH {
            return vec![line];
        }
    }
    let mut out = vec![format!("{pad}{name} {{")];
    for entry in entries {
        match entry {
            Entry::Plain(text) => out.push(format!("{pad}    {text},")),
            Entry::Nested { field, ty, fields } => {
                let inner: Vec<Entry> = fields.iter().cloned().map(Entry::Plain).collect();
                let mut lines = struct_literal(indent + 4, ty, &inner);
                if let Some(first) = lines.first_mut() {
                    *first = format!("{pad}    {field}: {}", first.trim_start());
                }
                if let Some(last) = lines.last_mut() {
                    last.push(',');
                }
                out.extend(lines);
            }
        }
    }
    out.push(format!("{pad}}}"));
    out
}

/// `text` as `///` lines of a top-level item, wrapped at the width.
fn wrap_doc(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && "/// ".len() + line.len() + 1 + word.len() > WIDTH {
            out.push(format!("/// {line}"));
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    out.push(format!("/// {line}"));
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        Member, Side, Slot, call_arm, derive, field_args, merge_impl, mirror, named_merge_impl,
    };
    use crate::{
        error::Result,
        testutil::{check, check_eq},
        typegen::registry::{Omit, SerdeField},
    };

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|&s| s.to_owned()).collect()
    }

    fn member(key: &str, field: &str, ty: &str, serde: SerdeField) -> Member {
        Member {
            key: Some(key.to_owned()),
            field: field.to_owned(),
            ty: ty.to_owned(),
            go_type: "pkg.T".to_owned(),
            go_field: "Go".to_owned(),
            omitempty: true,
            serde,
            keep_null: false,
        }
    }

    const fn value(omit: Omit) -> SerdeField {
        SerdeField {
            omit,
            value: true,
            with: None,
            merge_bridge: false,
        }
    }

    const fn holder(omit: Omit, merge_bridge: bool) -> SerdeField {
        SerdeField {
            omit,
            value: false,
            with: None,
            merge_bridge,
        }
    }

    fn raw() -> SerdeField {
        SerdeField {
            omit: Omit::Bridge,
            value: false,
            with: Some("raw_json".to_owned()),
            merge_bridge: true,
        }
    }

    #[test]
    fn wraps_derives_like_rustfmt() -> Result<()> {
        check_eq(
            &derive(&["Clone", "Debug", "Default", "PartialEq", "Eq", "GoJson"]),
            &lines(&["#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]"]),
            "one line",
        )?;
        // 102 wide on one line, 96 as the indented list.
        check_eq(
            &derive(&[
                "Clone",
                "Debug",
                "Default",
                "PartialEq",
                "Eq",
                "PartialOrd",
                "Ord",
                "Hash",
                "Serialize",
                "Deserialize",
                "GoJson",
            ]),
            &lines(&[
                "#[derive(",
                "    Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, \
                 Deserialize, GoJson,",
                ")]",
            ]),
            "indented list",
        )?;
        // 102 wide as the indented list.
        let all = [
            "Clone",
            "Copy",
            "Debug",
            "Default",
            "PartialEq",
            "Eq",
            "PartialOrd",
            "Ord",
            "Hash",
            "Serialize",
            "Deserialize",
            "GoJson",
        ];
        let mut want = lines(&["#[derive("]);
        want.extend(all.iter().map(|d| format!("    {d},")));
        want.push(")]".to_owned());
        check_eq(&derive(&all), &want, "one per line")
    }

    /// Checks each case's `#[serde(...)]` arguments.
    fn check_args(
        cases: Vec<(Member, Side, Vec<&str>)>,
        bridge: Option<&str>,
        uses: &mut BTreeSet<String>,
    ) -> Result<()> {
        for (m, side, want) in cases {
            check_eq(
                &field_args(&m, side, bridge, uses)?,
                &want.iter().map(|&s| s.to_owned()).collect::<Vec<_>>(),
                &format!("{m:?} {side:?}"),
            )?;
        }
        Ok(())
    }

    #[test]
    fn maps_values_and_holders_to_serde_attributes() -> Result<()> {
        let mut uses = BTreeSet::new();
        let cases = vec![
            (
                member("id", "id", "String", value(Omit::Zero)),
                Side::Both,
                vec![
                    "rename = \"id\"",
                    "skip_serializing_if = \"is_zero\"",
                    "deserialize_with = \"null_default\"",
                ],
            ),
            (
                member("w", "w", "Option<i64>", holder(Omit::Nil, false)),
                Side::Both,
                vec![
                    "rename = \"w\"",
                    "skip_serializing_if = \"Option::is_none\"",
                ],
            ),
            (
                member(
                    "c",
                    "c",
                    "Option<Vec<String>>",
                    holder(Omit::NilOrEmpty, false),
                ),
                Side::Both,
                vec![
                    "rename = \"c\"",
                    "skip_serializing_if = \"is_nil_or_empty\"",
                ],
            ),
            (
                member("ver", "ver", "OpenRTB", value(Omit::Never)),
                Side::Both,
                vec!["rename = \"ver\"", "deserialize_with = \"null_default\""],
            ),
            (
                Member {
                    omitempty: false,
                    ..member("id", "id", "String", value(Omit::Zero))
                },
                Side::Both,
                vec!["rename = \"id\"", "deserialize_with = \"null_default\""],
            ),
            (
                member("id", "id", "String", value(Omit::Zero)),
                Side::Serialize,
                vec!["rename = \"id\"", "skip_serializing_if = \"is_zero\""],
            ),
            (
                Member {
                    key: None,
                    ..member("x", "x", "String", value(Omit::Zero))
                },
                Side::Both,
                vec!["skip"],
            ),
        ];
        check_args(cases, None, &mut uses)?;
        check_eq(
            &uses.into_iter().collect::<Vec<_>>(),
            &lines(&[
                "pbs_jsonutil::is_nil_or_empty",
                "pbs_jsonutil::is_zero",
                "pbs_jsonutil::null_default",
            ]),
            "imports",
        )
    }

    #[test]
    fn maps_bridged_fields_to_the_bridge() -> Result<()> {
        let mut uses = BTreeSet::new();
        let ext = || member("ext", "ext", "RawJson", raw());
        let cases = vec![
            (
                ext(),
                Side::Both,
                vec![
                    "rename = \"ext\"",
                    "skip_serializing_if = \"gojson_bridge::raw_json::is_empty\"",
                    "with = \"gojson_bridge::raw_json\"",
                ],
            ),
            (
                ext(),
                Side::Serialize,
                vec![
                    "rename = \"ext\"",
                    "skip_serializing_if = \"gojson_bridge::raw_json::is_empty\"",
                    "serialize_with = \"gojson_bridge::raw_json::serialize\"",
                ],
            ),
            (
                ext(),
                Side::Deserialize,
                vec![
                    "rename = \"ext\"",
                    "deserialize_with = \"gojson_bridge::raw_json::deserialize\"",
                ],
            ),
            (
                Member {
                    keep_null: true,
                    omitempty: false,
                    ..ext()
                },
                Side::Both,
                vec![
                    "rename = \"ext\"",
                    "with = \"gojson_bridge::raw_json_keep_null\"",
                ],
            ),
        ];
        check_args(cases, Some("crate::gojson_bridge"), &mut uses)?;
        check(
            field_args(&ext(), Side::Both, None, &mut uses).is_err(),
            "a bridged field without a bridge",
        )?;
        check_eq(
            &uses.into_iter().collect::<Vec<_>>(),
            &lines(&["crate::gojson_bridge"]),
            "imports",
        )
    }

    #[test]
    fn breaks_merge_arms_like_rustfmt() -> Result<()> {
        let args = |field: &str, go: &str| {
            vec![
                format!("&mut self.{field}"),
                "value".to_owned(),
                "GO_TYPE".to_owned(),
                format!("\"{go}\""),
            ]
        };
        // Arguments 60 wide stay on one line; 61 go one per line.
        check_eq(
            &call_arm(
                "\"a\"",
                "merge_field",
                &args(&"a".repeat(15), &"B".repeat(15)),
            ),
            &vec![format!(
                "            \"a\" => merge_field(&mut self.{}, value, GO_TYPE, \"{}\"),",
                "a".repeat(15),
                "B".repeat(15)
            )],
            "one line",
        )?;
        check_eq(
            &call_arm(
                "\"a\"",
                "merge_field",
                &args(&"a".repeat(16), &"B".repeat(15)),
            ),
            &vec![
                "            \"a\" => merge_field(".to_owned(),
                format!("                &mut self.{},", "a".repeat(16)),
                "                value,".to_owned(),
                "                GO_TYPE,".to_owned(),
                format!("                \"{}\",", "B".repeat(15)),
                "            ),".to_owned(),
            ],
            "arguments too wide",
        )?;
        // A line too long (102) for the pattern goes into a block while the call fits there.
        let key = format!("\"{}\"", "k".repeat(23));
        check_eq(
            &call_arm(&key, "gojson_bridge::merge_field", &args("a", "B")),
            &vec![
                format!("            {key} => {{"),
                "                gojson_bridge::merge_field(&mut self.a, value, GO_TYPE, \"B\")"
                    .to_owned(),
                "            }".to_owned(),
            ],
            "block",
        )
    }

    /// The fields of a struct `T` that embeds `Inner` (member `id`), with a bridged member and
    /// one Go skips.
    fn slots() -> [Slot; 3] {
        let promoted = Member {
            go_type: "pkg.Inner".to_owned(),
            go_field: "ID".to_owned(),
            ..member("id", "id", "String", value(Omit::Zero))
        };
        [
            Slot::Embedded {
                field: "inner".to_owned(),
                ty: "Inner".to_owned(),
                members: vec![promoted],
            },
            Slot::Own(Member {
                go_field: "Ext".to_owned(),
                ..member("ext", "ext", "RawJson", raw())
            }),
            Slot::Own(Member {
                key: None,
                ..member("x", "x", "String", value(Omit::Zero))
            }),
        ]
    }

    #[test]
    fn merges_structs() -> Result<()> {
        let mut uses = BTreeSet::new();
        let bridge = Some("crate::gojson_bridge");
        check_eq(
            &merge_impl("T", "pkg.T", &slots(), bridge, &mut uses)?,
            &lines(&[
                "impl MergeClone for T {",
                "    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {",
                "        const GO_TYPE: &str = \"pkg.T\";",
                "        merge_struct(patch, |key, value| match key {",
                "            \"id\" => merge_field(&mut self.inner.id, value, \"pkg.Inner\", \
                 \"ID\"),",
                "            \"ext\" => gojson_bridge::merge_field(&mut self.ext, value, GO_TYPE, \
                 \"Ext\"),",
                "            _ => Ok(()),",
                "        })",
                "    }",
                "}",
            ]),
            "struct",
        )?;
        check_eq(
            &merge_impl("E", "pkg.E", &[], bridge, &mut uses)?
                .get(2)
                .cloned(),
            &Some("        merge_struct(patch, |_, _| Ok(()))".to_owned()),
            "no members",
        )?;
        check(
            merge_impl("T", "pkg.T", &slots(), None, &mut uses).is_err(),
            "a bridged member without a bridge",
        )
    }

    #[test]
    fn merges_named_types() -> Result<()> {
        let mut uses = BTreeSet::new();
        let bridge = Some("crate::gojson_bridge");
        check_eq(
            &named_merge_impl("N", true, bridge, &mut uses)?
                .get(2)
                .cloned(),
            &Some("        gojson_bridge::merge(&mut self.0, patch)".to_owned()),
            "bridged named type",
        )?;
        check_eq(
            &named_merge_impl("N", false, bridge, &mut uses)?
                .get(2)
                .cloned(),
            &Some("        self.0.merge_clone(patch)".to_owned()),
            "named type",
        )?;
        check(
            named_merge_impl("N", true, None, &mut uses).is_err(),
            "a bridged named type without a bridge",
        )
    }

    #[test]
    fn reads_structs_that_embed_through_their_members() -> Result<()> {
        let mut uses = BTreeSet::new();
        let bridge = Some("crate::gojson_bridge");
        let [item, from] = mirror("T", &slots(), bridge, &mut uses)?;
        check(
            item.first().is_some_and(|l| l.starts_with("/// ")),
            "the mirror's doc",
        )?;
        check_eq(
            &item
                .iter()
                .skip_while(|l| l.starts_with("///"))
                .cloned()
                .collect::<Vec<_>>(),
            &lines(&[
                "#[derive(Default, Deserialize)]",
                "#[serde(default)]",
                "struct TFields {",
                "    #[serde(rename = \"id\", deserialize_with = \"null_default\")]",
                "    id: String,",
                "    #[serde(",
                "        rename = \"ext\",",
                "        deserialize_with = \"gojson_bridge::raw_json::deserialize\"",
                "    )]",
                "    ext: RawJson,",
                "    #[serde(skip)]",
                "    x: String,",
                "}",
            ]),
            "mirror",
        )?;
        check_eq(
            &from,
            &lines(&[
                "impl From<TFields> for T {",
                "    fn from(fields: TFields) -> Self {",
                "        Self {",
                "            inner: Inner { id: fields.id },",
                "            ext: fields.ext,",
                "            x: fields.x,",
                "        }",
                "    }",
                "}",
            ]),
            "from",
        )?;
        let twice = [
            Slot::Own(member("a", "a", "String", value(Omit::Zero))),
            Slot::Own(member("b", "a", "String", value(Omit::Zero))),
        ];
        check(
            mirror("T", &twice, bridge, &mut uses).is_err(),
            "two members with one Rust name",
        )
    }
}
