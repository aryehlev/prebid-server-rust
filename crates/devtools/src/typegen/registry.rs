//! Every Go type the configured packages declare: its Rust path, and whether the Rust type is
//! `Copy` and `Eq` (which decides the derives of the structs holding it).

use std::collections::{BTreeMap, BTreeSet};

use super::{
    config::{Config, ExternType, PackageConfig, StringType},
    model::{Decl, DeclKind, Package, TypeKind, TypeRef},
    names,
};
use crate::error::{Result, ToolError};

/// Go types with a fixed Rust counterpart in pbs-gojson: the Go type, the Rust type, its `use`
/// path, and the serde bridge module that serializes and merges it (pbs-gojson's types have no
/// serde or `MergeClone` impls).
const BUILTIN_EXTERNS: &[(&str, &str, &str, &str)] = &[
    (
        "encoding/json.RawMessage",
        "RawJson",
        "pbs_gojson::RawJson",
        "raw_json",
    ),
    (
        "encoding/json.Number",
        "JsonNumber",
        "pbs_gojson::JsonNumber",
        "json_number",
    ),
];

/// What a Go type is on the Rust side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A struct with named fields (or a unit struct for `struct{}`).
    Struct,
    /// A Go named non-struct type: a single-field tuple struct.
    Named,
}

/// One generated (or hand-written) Rust type for a Go type.
#[derive(Debug, Clone)]
pub struct TypeInfo {
    /// The Rust crate (`pbs_openrtb`).
    pub crate_name: String,
    /// The module path inside the crate (`openrtb2`, `native1::request`).
    pub module: String,
    /// The Rust type name.
    pub rust_name: String,
    /// Go's `reflect.Type.String()` (`openrtb2.BidRequest`).
    pub go_name: String,
    /// Struct or named type.
    pub shape: Shape,
    /// Written by hand (a `skip` override), so not in the generated file.
    pub hand_written: bool,
    /// Whether the Rust type is `Copy`.
    pub copy: bool,
    /// Whether the Rust type is `Eq`.
    pub eq: bool,
}

/// The module a reference is rendered from, and the `use` paths the rendering needs.
#[derive(Debug)]
pub struct Ctx<'a> {
    /// The crate being generated.
    pub crate_name: &'a str,
    /// The module being generated.
    pub module: &'a str,
    /// The Rust type for Go strings.
    pub strings: StringType,
    /// `use` paths collected so far.
    pub uses: BTreeSet<String>,
    /// Types this module re-exports through a Go alias: `crate::module::Name` → local name.
    pub aliases: BTreeMap<String, String>,
}

impl Ctx<'_> {
    /// The `use` path and qualifier for a type in `module` of `crate_name`, from this module.
    fn qualify(&mut self, info: &TypeInfo) -> String {
        let full = format!("{}::{}::{}", info.crate_name, info.module, info.rust_name);
        if let Some(local) = self.aliases.get(&full) {
            return local.clone();
        }
        if info.crate_name == self.crate_name && info.module == self.module {
            if info.hand_written {
                let _ = self.uses.insert(format!("super::{}", info.rust_name));
            }
            return info.rust_name.clone();
        }
        let root = if info.crate_name == self.crate_name {
            "crate"
        } else {
            info.crate_name.as_str()
        };
        let _ = self.uses.insert(format!("{root}::{}", info.module));
        let qualifier = info.module.rsplit("::").next().unwrap_or(&info.module);
        format!("{qualifier}::{}", info.rust_name)
    }
}

/// The registry of every configured Go type.
#[derive(Debug, Default)]
pub struct Registry {
    types: BTreeMap<String, TypeInfo>,
    externs: BTreeMap<String, ExternType>,
}

/// A Go type's Rust properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Props {
    copy: bool,
    eq: bool,
}

impl Registry {
    /// Builds the registry for every package of every config from the described packages.
    ///
    /// # Errors
    ///
    /// Fails when a configured package was not described, or a type cannot be represented.
    pub fn build(configs: &[Config], packages: &BTreeMap<String, Package>) -> Result<Self> {
        let mut reg = Self::default();
        for (go, rust, use_path, bridge) in BUILTIN_EXTERNS {
            drop(reg.externs.insert(
                (*go).to_owned(),
                ExternType {
                    rust: (*rust).to_owned(),
                    use_path: (*use_path).to_owned(),
                    copy: false,
                    eq: true,
                    is_struct: false,
                    serde_bridge: Some((*bridge).to_owned()),
                },
            ));
        }
        let mut structs = Vec::new();
        for config in configs {
            for (key, ext) in &config.extern_types {
                drop(reg.externs.insert(key.clone(), ext.clone()));
            }
            for pc in &config.packages {
                let pkg = packages.get(&pc.go).ok_or_else(|| {
                    ToolError::new(format!(
                        "{}: package {} was not described",
                        config.source, pc.go
                    ))
                })?;
                structs.extend(reg.register(config, pc, pkg));
            }
        }
        // Named types first (their properties come from the underlying type alone).
        for config in configs {
            for pc in &config.packages {
                if let Some(pkg) = packages.get(&pc.go) {
                    reg.named_props(pc, pkg)?;
                }
            }
        }
        reg.struct_props(&structs)?;
        Ok(reg)
    }

    /// Registers the exported types of one package, all `Copy` and `Eq` until shown otherwise;
    /// returns its structs with their keys.
    fn register<'a>(
        &mut self,
        config: &Config,
        pc: &'a PackageConfig,
        pkg: &'a Package,
    ) -> Vec<(&'a PackageConfig, &'a Decl, String)> {
        let mut structs = Vec::new();
        for decl in pkg.decls.iter().filter(|d| is_type(d) && d.exported) {
            let o = pc.overrides.get(&decl.name);
            let shape = if decl.kind == DeclKind::Struct {
                structs.push((pc, decl, key(&pkg.path, &decl.name)));
                Shape::Struct
            } else {
                Shape::Named
            };
            let info = TypeInfo {
                crate_name: config.crate_name.clone(),
                module: pc.rust.clone(),
                rust_name: o
                    .and_then(|o| o.rust_name.clone())
                    .unwrap_or_else(|| names::type_name(&decl.name)),
                go_name: format!("{}.{}", pkg.name, decl.name),
                shape,
                hand_written: o.is_some_and(|o| o.skip),
                copy: o.is_none_or(|o| !o.skip || o.is_copy()),
                eq: o.is_none_or(|o| !o.skip || o.is_eq()),
            };
            drop(self.types.insert(key(&pkg.path, &decl.name), info));
        }
        structs
    }

    /// Sets the properties of the generated named types of one package from their underlying
    /// types.
    fn named_props(&mut self, pc: &PackageConfig, pkg: &Package) -> Result<()> {
        for decl in pkg
            .decls
            .iter()
            .filter(|d| d.kind == DeclKind::Named && d.exported)
        {
            if pc.overrides.get(&decl.name).is_some_and(|o| o.skip) {
                continue;
            }
            let under = decl.ty.as_ref().ok_or_else(|| {
                ToolError::new(format!("{}.{} has no underlying type", pkg.path, decl.name))
            })?;
            let props = self.props(under, false, pc.strings)?;
            if let Some(info) = self.types.get_mut(&key(&pkg.path, &decl.name)) {
                info.copy = props.copy;
                info.eq = props.eq;
            }
        }
        Ok(())
    }

    /// Sets the properties of the generated structs: the greatest fixed point (a struct is
    /// `Copy` and `Eq` unless a field is not).
    fn struct_props(&mut self, structs: &[(&PackageConfig, &Decl, String)]) -> Result<()> {
        loop {
            let mut changed = false;
            for (pc, decl, k) in structs {
                if pc.overrides.get(&decl.name).is_some_and(|o| o.skip) {
                    continue;
                }
                let mut props = Props {
                    copy: true,
                    eq: true,
                };
                for f in &decl.fields {
                    let o = pc.overrides.get(&format!("{}.{}", decl.name, f.name));
                    if o.is_some_and(|o| o.skip) {
                        continue;
                    }
                    let p = match o {
                        Some(o) if o.rust_type.is_some() => Props {
                            copy: o.is_copy(),
                            eq: o.is_eq(),
                        },
                        _ => self.props(&f.ty, o.is_some_and(|o| o.inline), pc.strings)?,
                    };
                    props.copy &= p.copy;
                    props.eq &= p.eq;
                }
                if let Some(info) = self.types.get_mut(k)
                    && (info.copy, info.eq) != (props.copy, props.eq)
                {
                    info.copy = props.copy;
                    info.eq = props.eq;
                    changed = true;
                }
            }
            if !changed {
                return Ok(());
            }
        }
    }

    /// The registered type for a Go package path and type name.
    #[must_use]
    pub fn get(&self, pkg: &str, name: &str) -> Option<&TypeInfo> {
        self.types.get(&key(pkg, name))
    }

    fn named(&self, t: &TypeRef) -> Result<Named<'_>> {
        let k = key(&t.pkg, &t.name);
        if let Some(info) = self.types.get(&k) {
            return Ok(Named::Generated(info));
        }
        self.externs.get(&k).map(Named::Extern).ok_or_else(|| {
            ToolError::new(format!(
                "Go type {k} is not generated by any typegen.json package and has no \
                 extern_types entry"
            ))
        })
    }

    fn props(&self, t: &TypeRef, inline: bool, strings: StringType) -> Result<Props> {
        Ok(match t.kind {
            TypeKind::Basic => {
                let b = basic(&t.name, strings)?;
                Props {
                    copy: b.copy,
                    eq: b.eq,
                }
            }
            TypeKind::Named => match self.named(t)? {
                Named::Generated(info) => Props {
                    copy: info.copy,
                    eq: info.eq,
                },
                Named::Extern(ext) => Props {
                    copy: ext.copy,
                    eq: ext.eq,
                },
            },
            TypeKind::Pointer => {
                let elem = elem(t)?;
                let inner = self.props(elem, false, strings)?;
                if self.is_struct(elem)? && !inline {
                    Props {
                        copy: false,
                        eq: inner.eq,
                    }
                } else {
                    inner
                }
            }
            TypeKind::Slice => Props {
                copy: false,
                eq: self.props(elem(t)?, false, strings)?.eq,
            },
            TypeKind::Map => {
                let k = t
                    .key
                    .as_deref()
                    .ok_or_else(|| ToolError::new(format!("map type {} has no key", t.reflect)))?;
                Props {
                    copy: false,
                    eq: self.props(k, false, strings)?.eq
                        && self.props(elem(t)?, false, strings)?.eq,
                }
            }
            TypeKind::Interface if t.methods.is_empty() => Props {
                copy: false,
                eq: true,
            },
            TypeKind::Interface
            | TypeKind::Array
            | TypeKind::Struct
            | TypeKind::Func
            | TypeKind::Chan
            | TypeKind::Typeparam => return Err(unsupported(t)),
        })
    }

    fn is_struct(&self, t: &TypeRef) -> Result<bool> {
        Ok(t.kind == TypeKind::Named
            && match self.named(t)? {
                Named::Generated(info) => info.shape == Shape::Struct,
                Named::Extern(ext) => ext.is_struct,
            })
    }

    /// The Rust type for a Go field type, as written in `ctx`'s module.
    ///
    /// # Errors
    ///
    /// Fails for Go types with no Rust representation (channels, functions, arrays, non-empty
    /// interfaces, unnamed structs, pointers to pointers, slices or maps).
    pub fn render(&self, t: &TypeRef, inline: bool, ctx: &mut Ctx<'_>) -> Result<String> {
        Ok(match t.kind {
            TypeKind::Basic => {
                let b = basic(&t.name, ctx.strings)?;
                if let Some(u) = b.use_path {
                    let _ = ctx.uses.insert(u.to_owned());
                }
                b.rust.to_owned()
            }
            TypeKind::Named => match self.named(t)? {
                Named::Generated(info) => ctx.qualify(info),
                Named::Extern(ext) => {
                    let _ = ctx.uses.insert(ext.use_path.clone());
                    ext.rust.clone()
                }
            },
            TypeKind::Pointer => {
                let elem = elem(t)?;
                if matches!(
                    elem.kind,
                    TypeKind::Pointer | TypeKind::Slice | TypeKind::Map | TypeKind::Interface
                ) {
                    return Err(unsupported(t));
                }
                let inner = self.render(elem, false, ctx)?;
                if self.is_struct(elem)? && !inline {
                    format!("Option<Box<{inner}>>")
                } else {
                    format!("Option<{inner}>")
                }
            }
            TypeKind::Slice => {
                let elem = elem(t)?;
                if elem.kind == TypeKind::Basic && elem.name == "uint8" {
                    return Err(ToolError::new(format!(
                        "{}: []byte (base64 in JSON) has no generated mapping; override the field",
                        t.reflect
                    )));
                }
                format!("Option<Vec<{}>>", self.render(elem, false, ctx)?)
            }
            TypeKind::Map => {
                let k = t
                    .key
                    .as_deref()
                    .ok_or_else(|| ToolError::new(format!("map type {} has no key", t.reflect)))?;
                let key = self.render(k, false, ctx)?;
                let value = self.render(elem(t)?, false, ctx)?;
                let _ = ctx.uses.insert("std::collections::BTreeMap".to_owned());
                format!("Option<BTreeMap<{key}, {value}>>")
            }
            TypeKind::Interface if t.methods.is_empty() => {
                let _ = ctx.uses.insert("serde_json::Value".to_owned());
                "Value".to_owned()
            }
            TypeKind::Interface
            | TypeKind::Array
            | TypeKind::Struct
            | TypeKind::Func
            | TypeKind::Chan
            | TypeKind::Typeparam => return Err(unsupported(t)),
        })
    }

    /// The Rust type of a constant of Go type `t` (a named type of a generated package or a
    /// basic type), with the constructor its value is wrapped in (`None` for a basic type).
    ///
    /// # Errors
    ///
    /// Fails for other types.
    pub fn const_type(
        &self,
        t: &TypeRef,
        ctx: &mut Ctx<'_>,
    ) -> Result<(String, Option<String>, String)> {
        match t.kind {
            TypeKind::Basic => {
                let b = basic(&t.name, ctx.strings)?;
                let rust = if t.name == "string" { "&str" } else { b.rust };
                Ok((rust.to_owned(), None, t.name.clone()))
            }
            TypeKind::Named => match self.named(t)? {
                Named::Generated(info) if info.shape == Shape::Named => {
                    let path = ctx.qualify(info);
                    Ok((path.clone(), Some(path), info.go_name.clone()))
                }
                Named::Generated(_) | Named::Extern(_) => Err(unsupported(t)),
            },
            TypeKind::Pointer
            | TypeKind::Slice
            | TypeKind::Array
            | TypeKind::Map
            | TypeKind::Interface
            | TypeKind::Struct
            | TypeKind::Func
            | TypeKind::Chan
            | TypeKind::Typeparam => Err(unsupported(t)),
        }
    }

    /// How the serde code treats a field of Go type `t`.
    ///
    /// # Errors
    ///
    /// Fails for types the serde code cannot represent: those [`Registry::render`] refuses but an
    /// unnamed struct or a pointer to a slice or map (which a `rust_type` override spells), a
    /// bridged type inside a pointer, slice or map, and `GoString` strings.
    pub fn serde_field(&self, t: &TypeRef, strings: StringType) -> Result<SerdeField> {
        let value = |omit| SerdeField {
            omit,
            value: true,
            with: None,
            merge_bridge: false,
        };
        match t.kind {
            TypeKind::Basic => {
                let _ = basic(&t.name, strings)?;
                if t.name == "string" && strings == StringType::GoString {
                    return Err(ToolError::new(
                        "GoString strings have no serde mapping; use String strings with serde",
                    ));
                }
                Ok(SerdeField {
                    merge_bridge: matches!(t.name.as_str(), "int" | "uint"),
                    ..value(Omit::Zero)
                })
            }
            TypeKind::Named => Ok(match self.named(t)? {
                Named::Generated(info) if info.shape == Shape::Struct => value(Omit::Never),
                Named::Generated(_) => value(Omit::Zero),
                Named::Extern(ext) => match &ext.serde_bridge {
                    Some(module) => SerdeField {
                        omit: Omit::Bridge,
                        value: false,
                        with: Some(module.clone()),
                        merge_bridge: true,
                    },
                    None if ext.is_struct => value(Omit::Never),
                    None => value(Omit::Zero),
                },
            }),
            TypeKind::Struct => Ok(value(Omit::Never)),
            TypeKind::Pointer => {
                let inner = self.serde_holder(t, elem(t)?, strings)?;
                Ok(SerdeField {
                    omit: Omit::Nil,
                    value: false,
                    with: None,
                    merge_bridge: inner,
                })
            }
            TypeKind::Slice => {
                self.serde_element(t, elem(t)?, strings)?;
                Ok(SerdeField {
                    omit: Omit::NilOrEmpty,
                    value: false,
                    with: None,
                    merge_bridge: false,
                })
            }
            TypeKind::Map => {
                let k = t
                    .key
                    .as_deref()
                    .ok_or_else(|| ToolError::new(format!("map type {} has no key", t.reflect)))?;
                let _ = self.serde_holder(t, k, strings)?;
                self.serde_element(t, elem(t)?, strings)?;
                Ok(SerdeField {
                    omit: Omit::NilOrEmpty,
                    value: false,
                    with: None,
                    merge_bridge: false,
                })
            }
            TypeKind::Interface if t.methods.is_empty() => Ok(SerdeField {
                omit: Omit::Zero,
                value: false,
                with: None,
                merge_bridge: false,
            }),
            TypeKind::Interface
            | TypeKind::Array
            | TypeKind::Func
            | TypeKind::Chan
            | TypeKind::Typeparam => Err(unsupported(t)),
        }
    }

    /// Whether `MergeClone` of the pointer, slice or map `outer` goes through the bridge for its
    /// element or key `inner`.
    fn serde_holder(&self, outer: &TypeRef, inner: &TypeRef, strings: StringType) -> Result<bool> {
        let field = self.serde_field(inner, strings)?;
        if field.with.is_some() {
            return Err(ToolError::new(format!(
                "{}: a bridged type inside a pointer, slice or map has no serde mapping; \
                 override the field",
                outer.reflect
            )));
        }
        Ok(field.merge_bridge)
    }

    /// Checks the element `inner` of the slice or map `outer`: pbs-jsonutil merges a `Vec` or
    /// `BTreeMap` only of elements with a `MergeClone` of their own, and the bridge merges no
    /// container, so an element that merges through the bridge (a Go `int` or `uint`, or a
    /// pointer to one) has no serde mapping.
    fn serde_element(&self, outer: &TypeRef, inner: &TypeRef, strings: StringType) -> Result<()> {
        if self.serde_holder(outer, inner, strings)? {
            return Err(ToolError::new(format!(
                "{}: a Go int or uint inside a slice or map has no serde mapping (pbs-jsonutil \
                 merges no isize or usize); override the field",
                outer.reflect
            )));
        }
        Ok(())
    }
}

/// How the serde code treats a field of one Go type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerdeField {
    /// Go's `omitempty` test for the type.
    pub omit: Omit,
    /// Whether the type is a Go value (a number, bool, string, named type or struct): JSON null
    /// leaves it at its zero value (`null_default`), where serde would fail.
    pub value: bool,
    /// The bridge module (`raw_json`) of a bridged type held directly.
    pub with: Option<String>,
    /// Whether `MergeClone` goes through the bridge: the type holds a bridged type, or a Go `int`
    /// or `uint` (pbs-jsonutil has no `MergeClone` for `isize` and `usize`).
    pub merge_bridge: bool,
}

/// Go's `omitempty` test, as the serde code spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Omit {
    /// Never omitted: Go ignores `omitempty` on a struct.
    Never,
    /// The zero value (`is_zero`): numbers, bools, strings, named types, `interface{}` (nil).
    Zero,
    /// Nil (`Option::is_none`): a pointer.
    Nil,
    /// Nil or empty (`is_nil_or_empty`): a slice or map.
    NilOrEmpty,
    /// The bridge module's `is_empty`.
    Bridge,
}

/// A named Go type's Rust side.
enum Named<'a> {
    Generated(&'a TypeInfo),
    Extern(&'a ExternType),
}

/// A Go basic type's Rust side.
struct Basic {
    rust: &'static str,
    use_path: Option<&'static str>,
    copy: bool,
    eq: bool,
}

fn basic(name: &str, strings: StringType) -> Result<Basic> {
    let (rust, copy, eq) = match name {
        "bool" => ("bool", true, true),
        "int" => ("isize", true, true),
        "int8" => ("i8", true, true),
        "int16" => ("i16", true, true),
        "int32" => ("i32", true, true),
        "int64" => ("i64", true, true),
        "uint" => ("usize", true, true),
        "uint8" => ("u8", true, true),
        "uint16" => ("u16", true, true),
        "uint32" => ("u32", true, true),
        "uint64" => ("u64", true, true),
        "float32" => ("f32", true, false),
        "float64" => ("f64", true, false),
        "string" => {
            return Ok(match strings {
                StringType::String => Basic {
                    rust: "String",
                    use_path: None,
                    copy: false,
                    eq: true,
                },
                StringType::GoString => Basic {
                    rust: "GoString",
                    use_path: Some("pbs_gojson::GoString"),
                    copy: false,
                    eq: true,
                },
            });
        }
        other => {
            return Err(ToolError::new(format!(
                "Go basic type {other} has no Rust mapping"
            )));
        }
    };
    Ok(Basic {
        rust,
        use_path: None,
        copy,
        eq,
    })
}

fn elem(t: &TypeRef) -> Result<&TypeRef> {
    t.elem
        .as_deref()
        .ok_or_else(|| ToolError::new(format!("type {} has no element type", t.reflect)))
}

fn unsupported(t: &TypeRef) -> ToolError {
    ToolError::new(format!(
        "Go type {} has no generated Rust mapping; override the field with rust_type",
        t.reflect
    ))
}

const fn is_type(d: &Decl) -> bool {
    matches!(d.kind, DeclKind::Struct | DeclKind::Named)
}

fn key(pkg: &str, name: &str) -> String {
    format!("{pkg}.{name}")
}
