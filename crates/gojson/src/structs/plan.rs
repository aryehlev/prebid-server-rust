//! Field binding plans: jsoniter's `describeStruct` / `decoderOfStruct` / `encoderOfStruct` and
//! `encoding/json`'s `typeFields`, computed once per struct type.

use core::mem;
use std::collections::HashMap;

use super::{FieldDesc, StructDesc};
use crate::{
    gotype::GoKind,
    jsoniter::calc_hash,
    unicode::{fold_name, strings_to_lower},
};

/// jsoniter's `,string` handling for a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StringMode {
    /// No `,string` option.
    None,
    /// `stringModeNumberDecoder`/`Encoder` (any kind but `string`); for `encoding/json`
    /// fields, the quoted encoding.
    Number,
    /// `stringModeStringDecoder`/`Encoder` (a `string` kind).
    String,
}

/// One jsoniter binding: a field reachable from the struct (through embedded structs).
#[derive(Debug)]
struct Binding {
    path: Vec<usize>,
    name: Option<String>,
    tagged: bool,
    omitempty: bool,
    mode: StringMode,
}

/// A field as one of the plans uses it.
#[derive(Debug)]
pub struct PlanField {
    /// Field index path (embedded fields first).
    pub path: Vec<usize>,
    /// JSON object key.
    pub name: String,
    /// Skip when empty.
    pub omitempty: bool,
    /// `,string` handling.
    pub mode: StringMode,
    /// Go names of the embedded fields on the path (`encoding/json` error context).
    pub embedded_names: Vec<&'static str>,
    /// The leaf field.
    pub leaf: &'static FieldDesc,
}

/// How jsoniter decodes the struct (`createStructDecoder`).
#[derive(Debug)]
pub enum JsoniterDecoder {
    /// No fields: `skipObjectDecoder`.
    Skip,
    /// Up to ten fields without hash collisions: matched by case-folded FNV hash.
    Hashed(Vec<(i64, usize)>),
    /// `generalStructDecoder`: exact name, then `strings.ToLower` name.
    General(HashMap<Vec<u8>, usize>),
}

/// Everything the runtime needs to encode and decode one struct type.
#[derive(Debug)]
pub struct Plans {
    /// jsoniter decode bindings (indexed by [`JsoniterDecoder`]).
    pub jsoniter_bindings: Vec<PlanField>,
    /// jsoniter decoder shape.
    pub jsoniter_decoder: JsoniterDecoder,
    /// jsoniter encode fields, or `None` for `emptyStructEncoder`.
    pub jsoniter_encode: Option<Vec<PlanField>>,
    /// `encoding/json` fields in index order (encode, and decode by lookup).
    pub std_fields: Vec<PlanField>,
    /// `encoding/json` exact-name index.
    pub std_exact: HashMap<Vec<u8>, usize>,
    /// `encoding/json` case-folded-name index (first field wins).
    pub std_folded: HashMap<Vec<u8>, usize>,
}

/// Splits a tag into its name and options (`tagParts`).
fn tag_parts(tag: &str) -> (&str, Vec<&str>) {
    let mut parts = tag.split(',');
    let name = parts.next().unwrap_or_default();
    (name, parts.collect())
}

/// jsoniter's exported test (`calcFieldNames`): not lower-case and not `_`.
fn jsoniter_exported(name: &str) -> bool {
    match name.bytes().next() {
        Some(b'_') | None => false,
        Some(b) => !char::from(b).is_lowercase(),
    }
}

/// `encoding/json`'s exported test (`token.IsExported`).
fn std_exported(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

const fn is_embedded_struct(field: &FieldDesc) -> bool {
    field.embedded.is_some()
}

/// jsoniter `describeStruct`: the bindings of `desc` prefixed by `prefix`, sorted by levels.
fn describe(
    desc: &'static StructDesc,
    prefix: &[usize],
    out: &mut Vec<(Binding, &'static FieldDesc)>,
) {
    // Go: json-iterator/go reflect_extension.go describeStruct / processTags / calcFieldNames
    let mut own: Vec<(Binding, &'static FieldDesc)> = Vec::new();
    let mut embedded: Vec<(Binding, &'static FieldDesc)> = Vec::new();
    for (i, field) in desc.fields.iter().enumerate() {
        if field.tag == "-" || field.go_name == "_" {
            continue;
        }
        let (tag_name, options) = tag_parts(field.tag);
        let mut path = prefix.to_vec();
        path.push(i);
        if let Some(emb) = field.embedded
            && tag_name.is_empty()
        {
            describe((emb.desc)(), &path, &mut embedded);
            continue;
        }
        let name = jsoniter_exported(field.go_name).then(|| {
            if tag_name.is_empty() {
                field.go_name.to_owned()
            } else {
                tag_name.to_owned()
            }
        });
        let mut omitempty = false;
        let mut mode = StringMode::None;
        for option in options {
            if option == "omitempty" {
                omitempty = true;
            } else if option == "string" {
                mode = if field.kind == GoKind::String {
                    StringMode::String
                } else {
                    StringMode::Number
                };
            }
        }
        own.push((
            Binding {
                path,
                name,
                tagged: !field.tag.is_empty(),
                omitempty,
                mode,
            },
            field,
        ));
    }
    embedded.append(&mut own);
    embedded.sort_by(|a, b| a.0.path.cmp(&b.0.path));
    out.append(&mut embedded);
}

/// jsoniter `resolveConflictBinding`: (ignore old, ignore new).
const fn resolve_conflict(old: &Binding, new: &Binding) -> (bool, bool) {
    let deeper_old = old.path.len() > new.path.len();
    let deeper_new = new.path.len() > old.path.len();
    if new.tagged && !old.tagged {
        return (true, false);
    }
    if !new.tagged && old.tagged {
        return (true, false);
    }
    if deeper_old {
        (true, false)
    } else if deeper_new {
        (false, true)
    } else {
        (true, true)
    }
}

/// The Go names of the embedded fields along `path` (excluding the leaf), and the leaf.
fn walk(
    root: &'static StructDesc,
    path: &[usize],
) -> (Vec<&'static str>, Option<&'static FieldDesc>) {
    let mut names = Vec::new();
    let mut desc = root;
    let mut leaf = None;
    for (depth, &i) in path.iter().enumerate() {
        let Some(field) = desc.fields.get(i) else {
            return (names, None);
        };
        if depth + 1 == path.len() {
            leaf = Some(field);
        } else if let Some(emb) = field.embedded {
            names.push(field.go_name);
            desc = (emb.desc)();
        }
    }
    (names, leaf)
}

fn plan_field(root: &'static StructDesc, binding: &Binding, name: String) -> Option<PlanField> {
    let (embedded_names, leaf) = walk(root, &binding.path);
    Some(PlanField {
        path: binding.path.clone(),
        name,
        omitempty: binding.omitempty,
        mode: binding.mode,
        embedded_names,
        leaf: leaf?,
    })
}

/// jsoniter `decoderOfStruct` + `createStructDecoder`.
fn jsoniter_decoder(
    root: &'static StructDesc,
    bindings: &[(Binding, &'static FieldDesc)],
) -> (Vec<PlanField>, JsoniterDecoder) {
    // Go: json-iterator/go reflect_struct_decoder.go decoderOfStruct / createStructDecoder
    let mut by_name: Vec<(String, usize)> = Vec::new();
    for (index, (binding, _)) in bindings.iter().enumerate() {
        let Some(name) = &binding.name else {
            continue;
        };
        let Some(pos) = by_name.iter().position(|(n, _)| n == name) else {
            by_name.push((name.clone(), index));
            continue;
        };
        let old = by_name
            .get(pos)
            .and_then(|(_, i)| bindings.get(*i))
            .map(|(b, _)| b);
        let (ignore_old, ignore_new) =
            old.map_or((false, false), |old| resolve_conflict(old, binding));
        if ignore_old {
            let _removed = by_name.remove(pos);
        }
        if !ignore_new {
            by_name.push((name.clone(), index));
        }
    }
    let mut plan_fields = Vec::new();
    let mut fields: Vec<(Vec<u8>, usize)> = Vec::new();
    for (name, index) in &by_name {
        let Some(field) = bindings
            .get(*index)
            .and_then(|(b, _)| plan_field(root, b, name.clone()))
        else {
            continue;
        };
        fields.push((name.as_bytes().to_vec(), plan_fields.len()));
        plan_fields.push(field);
    }
    // Case-insensitive aliases (Go adds them in map order; the first binding wins here).
    let exact: Vec<(Vec<u8>, usize)> = fields.clone();
    for (name, index) in exact {
        let lower = strings_to_lower(&name);
        if !fields.iter().any(|(n, _)| *n == lower) {
            fields.push((lower, index));
        }
    }
    if fields.is_empty() {
        return (plan_fields, JsoniterDecoder::Skip);
    }
    if fields.len() <= 10 {
        let mut known: Vec<i64> = vec![0];
        let mut hashed = Vec::new();
        let mut collision = false;
        for (name, index) in &fields {
            let hash = calc_hash(&strings_to_lower(name));
            if known.contains(&hash) {
                collision = true;
                break;
            }
            known.push(hash);
            hashed.push((hash, *index));
        }
        if !collision {
            return (plan_fields, JsoniterDecoder::Hashed(hashed));
        }
    }
    (
        plan_fields,
        JsoniterDecoder::General(fields.into_iter().collect()),
    )
}

/// jsoniter `encoderOfStruct`: the fields to write, or `None` for `emptyStructEncoder`.
fn jsoniter_encoder(
    root: &'static StructDesc,
    bindings: &[(Binding, &'static FieldDesc)],
) -> Option<Vec<PlanField>> {
    // Go: json-iterator/go reflect_struct_encoder.go encoderOfStruct
    let mut ordered: Vec<(usize, String, bool)> = Vec::new();
    for (index, (binding, _)) in bindings.iter().enumerate() {
        let Some(name) = &binding.name else {
            continue;
        };
        let mut new_ignored = false;
        for old in &mut ordered {
            if old.1 != *name {
                continue;
            }
            let Some((old_binding, _)) = bindings.get(old.0) else {
                continue;
            };
            let (ignore_old, ignore_new) = resolve_conflict(old_binding, binding);
            old.2 = ignore_old;
            new_ignored = ignore_new;
        }
        ordered.push((index, name.clone(), new_ignored));
    }
    if ordered.is_empty() {
        return None;
    }
    Some(
        ordered
            .into_iter()
            .filter(|(_, _, ignored)| !ignored)
            .filter_map(|(index, name, _)| {
                bindings
                    .get(index)
                    .and_then(|(b, _)| plan_field(root, b, name))
            })
            .collect(),
    )
}

/// `encoding/json` `isValidTag`.
fn is_valid_tag(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            "!#$%&()*+-./:;<=>?@[]^_{|}~ ".contains(c) || c.is_alphabetic() || c.is_numeric()
        })
}

/// One `encoding/json` field candidate (`field` in `typeFields`).
#[derive(Debug, Clone)]
struct StdCandidate {
    name: String,
    tagged: bool,
    path: Vec<usize>,
    omitempty: bool,
    quoted: bool,
}

/// A struct type queued for a `typeFields` level, with the index path that reaches it.
type Queued = (&'static StructDesc, Vec<usize>);

/// How many times a struct type was queued at one `typeFields` level (Go's `count`).
type Count = (*const StructDesc, usize);

/// One breadth-first level of `typeFields`: records fields and queues embedded structs.
fn std_scan_level(
    current: Vec<Queued>,
    count: &[Count],
    visited: &mut Vec<*const StructDesc>,
    fields: &mut Vec<StdCandidate>,
) -> (Vec<Queued>, Vec<Count>) {
    let mut next = Vec::new();
    let mut next_count: Vec<Count> = Vec::new();
    for (desc, path) in current {
        let key: *const StructDesc = desc;
        if visited.contains(&key) {
            continue;
        }
        visited.push(key);
        let repeats = count.iter().find(|(k, _)| *k == key).map_or(0, |(_, n)| *n);
        for (i, field) in desc.fields.iter().enumerate() {
            if !is_embedded_struct(field) && !std_exported(field.go_name) {
                continue;
            }
            if field.tag == "-" {
                continue;
            }
            let (tag_name, options) = tag_parts(field.tag);
            let name = if is_valid_tag(tag_name) { tag_name } else { "" };
            let mut index = path.clone();
            index.push(i);
            let quoted = options.contains(&"string") && field.deref_kind.std_quotable();
            match field.embedded {
                Some(emb) if name.is_empty() => {
                    let sub = (emb.desc)();
                    let sub_key: *const StructDesc = sub;
                    if let Some(entry) = next_count.iter_mut().find(|(k, _)| *k == sub_key) {
                        entry.1 += 1;
                    } else {
                        next_count.push((sub_key, 1));
                        next.push((sub, index));
                    }
                }
                Some(_) | None => {
                    let candidate = StdCandidate {
                        name: if name.is_empty() {
                            field.go_name.to_owned()
                        } else {
                            name.to_owned()
                        },
                        tagged: !name.is_empty(),
                        path: index,
                        omitempty: options.contains(&"omitempty"),
                        quoted,
                    };
                    if repeats > 1 {
                        fields.push(candidate.clone());
                    }
                    fields.push(candidate);
                }
            }
        }
    }
    (next, next_count)
}

/// `encoding/json` `typeFields`: breadth-first over embedded structs, then Go's dominance rules.
fn std_fields(root: &'static StructDesc) -> Vec<PlanField> {
    // Go: encoding/json/encode.go typeFields / dominantField
    let mut fields: Vec<StdCandidate> = Vec::new();
    let mut next = vec![(root, Vec::new())];
    let mut count: Vec<Count> = Vec::new();
    let mut visited: Vec<*const StructDesc> = Vec::new();
    while !next.is_empty() {
        let current = mem::take(&mut next);
        let (queued, queued_count) = std_scan_level(current, &count, &mut visited, &mut fields);
        next = queued;
        count = queued_count;
    }
    fields.sort_by(|a, b| {
        a.name
            .as_bytes()
            .cmp(b.name.as_bytes())
            .then(a.path.len().cmp(&b.path.len()))
            .then(b.tagged.cmp(&a.tagged))
            .then(a.path.cmp(&b.path))
    });
    let mut out: Vec<StdCandidate> = Vec::new();
    let mut i = 0;
    while let Some(first) = fields.get(i) {
        let run = fields
            .get(i..)
            .unwrap_or_default()
            .iter()
            .take_while(|f| f.name == first.name)
            .count();
        let second = fields.get(i + 1).filter(|_| run > 1);
        let dominant =
            second.is_none_or(|s| s.path.len() != first.path.len() || s.tagged != first.tagged);
        if dominant {
            out.push(first.clone());
        }
        i += run.max(1);
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.into_iter()
        .filter_map(|c| {
            let (embedded_names, leaf) = walk(root, &c.path);
            Some(PlanField {
                path: c.path,
                name: c.name,
                omitempty: c.omitempty,
                mode: if c.quoted {
                    StringMode::Number
                } else {
                    StringMode::None
                },
                embedded_names,
                leaf: leaf?,
            })
        })
        .collect()
}

impl Plans {
    /// Builds the plans for a struct type.
    #[must_use]
    pub fn build(desc: &'static StructDesc) -> Self {
        let mut bindings = Vec::new();
        describe(desc, &[], &mut bindings);
        let (jsoniter_bindings, jsoniter_decoder) = jsoniter_decoder(desc, &bindings);
        let jsoniter_encode = jsoniter_encoder(desc, &bindings);
        let std_fields = std_fields(desc);
        let mut std_exact = HashMap::new();
        let mut std_folded = HashMap::new();
        for (i, field) in std_fields.iter().enumerate() {
            let _replaced = std_exact.insert(field.name.as_bytes().to_vec(), i);
            let _existing = std_folded
                .entry(fold_name(field.name.as_bytes()))
                .or_insert(i);
        }
        Self {
            jsoniter_bindings,
            jsoniter_decoder,
            jsoniter_encode,
            std_fields,
            std_exact,
            std_folded,
        }
    }
}
