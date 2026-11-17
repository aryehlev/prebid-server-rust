//! Derive macros for `pbs-goyaml`: `#[derive(GoYaml)]` (yaml.v3 decoding) and
//! `#[derive(GoMapstructure)]` (mapstructure decoding). Both read the `#[goyaml(...)]`
//! attribute, falling back to `#[gojson(...)]` for the Go type and field names; see the
//! `pbs-goyaml` crate documentation for the attribute reference.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{
    Data, DeriveInput, Error, Fields, GenericArgument, Ident, LitStr, PathArguments, Result, Token,
    Type, meta::ParseNestedMeta, parse_macro_input,
};

/// Derives `YamlDecode` (and, for structs with named fields, `YamlStruct` and `YamlInline`).
#[proc_macro_derive(GoYaml, attributes(goyaml))]
pub fn derive_go_yaml(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match Item::parse(&input) {
        Ok(item) => item.yaml().into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Derives `MsDecode` (and, for structs with named fields, `MsStruct`).
#[proc_macro_derive(GoMapstructure, attributes(goyaml))]
pub fn derive_go_mapstructure(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match Item::parse(&input) {
        Ok(item) => item.mapstructure().into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// The string value of `key = "..."`.
fn lit(meta: &ParseNestedMeta<'_>) -> Result<String> {
    Ok(meta.value()?.parse::<LitStr>()?.value())
}

/// Consumes the value of a `#[gojson(...)]` setting this crate does not use.
fn skip_value(meta: &ParseNestedMeta<'_>) -> Result<()> {
    if meta.input.peek(Token![=]) {
        let _ignored: LitStr = meta.value()?.parse()?;
    }
    Ok(())
}

/// The Go type name: `#[goyaml(go_type = ...)]`, else `#[gojson(go_type = ...)]`.
fn go_type(input: &DeriveInput) -> Result<String> {
    let mut own = None;
    let mut json = None;
    for attr in &input.attrs {
        if attr.path().is_ident("goyaml") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("go_type") {
                    own = Some(lit(&meta)?);
                    Ok(())
                } else {
                    Err(meta.error("unknown goyaml container attribute"))
                }
            })?;
        } else if attr.path().is_ident("gojson") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("go_type") {
                    json = Some(lit(&meta)?);
                    Ok(())
                } else {
                    skip_value(&meta)
                }
            })?;
        }
    }
    own.or(json).ok_or_else(|| {
        Error::new(
            Span::call_site(),
            "GoYaml and GoMapstructure need the Go type name: #[goyaml(go_type = \"pkg.Name\")] \
             or #[gojson(go_type = \"pkg.Name\")]",
        )
    })
}

/// The field settings of `#[goyaml(...)]`, with `#[gojson(...)]` fallbacks.
#[derive(Default)]
struct FieldAttrs {
    go: Option<String>,
    yaml: Option<String>,
    mapstructure: Option<String>,
    embedded: bool,
    skip: bool,
}

fn field_attrs(field: &syn::Field) -> Result<FieldAttrs> {
    let mut attrs = FieldAttrs::default();
    let mut json = FieldAttrs::default();
    for attr in &field.attrs {
        if attr.path().is_ident("goyaml") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("go") {
                    attrs.go = Some(lit(&meta)?);
                } else if meta.path.is_ident("yaml") {
                    attrs.yaml = Some(lit(&meta)?);
                } else if meta.path.is_ident("mapstructure") {
                    attrs.mapstructure = Some(lit(&meta)?);
                } else if meta.path.is_ident("embedded") {
                    attrs.embedded = true;
                } else if meta.path.is_ident("skip") {
                    attrs.skip = true;
                } else {
                    return Err(meta.error("unknown goyaml field attribute"));
                }
                Ok(())
            })?;
        } else if attr.path().is_ident("gojson") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("go") {
                    json.go = Some(lit(&meta)?);
                } else if meta.path.is_ident("embedded") {
                    json.embedded = true;
                } else if meta.path.is_ident("skip") {
                    json.skip = true;
                } else {
                    skip_value(&meta)?;
                }
                Ok(())
            })?;
        }
    }
    attrs.go = attrs.go.or(json.go);
    attrs.embedded |= json.embedded;
    attrs.skip |= json.skip;
    Ok(attrs)
}

/// `snake_case` → `UpperCamelCase` (the default Go field name, as in `pbs-gojson`).
fn upper_camel(ident: &Ident) -> String {
    let name = ident.to_string();
    let name = name.strip_prefix("r#").unwrap_or(&name);
    let mut out = String::with_capacity(name.len());
    let mut upper = true;
    for c in name.chars() {
        if c == '_' {
            upper = true;
            continue;
        }
        if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// `ty` without invisible groups (a type that went through a `macro_rules!` `$t:ty`) and
/// parentheses.
fn ungroup(ty: &Type) -> &Type {
    if let Type::Group(group) = ty {
        ungroup(&group.elem)
    } else if let Type::Paren(paren) = ty {
        ungroup(&paren.elem)
    } else {
        ty
    }
}

/// The single type argument of `Name<T>`, if `ty` is that.
fn generic_arg<'a>(ty: &'a Type, name: &str) -> Option<&'a Type> {
    let Type::Path(path) = ungroup(ty) else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != name {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    if args.args.len() != 1 {
        return None;
    }
    let Some(GenericArgument::Type(inner)) = args.args.first() else {
        return None;
    };
    Some(inner)
}

/// Whether `ty` (or `Option<ty>`) is a `BTreeMap` or `HashMap`.
fn is_map_type(ty: &Type) -> bool {
    let ty = generic_arg(ty, "Option").unwrap_or(ty);
    let Type::Path(path) = ungroup(ty) else {
        return false;
    };
    path.path
        .segments
        .last()
        .is_some_and(|s| s.ident == "BTreeMap" || s.ident == "HashMap")
}

/// How yaml.v3 decodes a field (Go: `getStructInfo`).
enum YamlField {
    /// Not decoded: `yaml:"-"`, or unexported and not embedded.
    Skip,
    /// Decoded under the key.
    Key(String),
    /// `,inline` struct.
    Inline,
    /// `,inline` map.
    InlineMap,
}

fn yaml_field(field: &syn::Field, tag: &str, go_name: &str, visible: bool) -> Result<YamlField> {
    if !visible || tag == "-" {
        return Ok(YamlField::Skip);
    }
    let mut parts = tag.split(',');
    let key = parts.next().unwrap_or_default();
    let mut inline = false;
    for flag in parts {
        match flag {
            "omitempty" | "flow" => {}
            "inline" => inline = true,
            _ => {
                return Err(Error::new_spanned(
                    field,
                    format!(
                        "unsupported flag {flag:?} in yaml tag {tag:?} (yaml.v3 panics decoding \
                         this struct)"
                    ),
                ));
            }
        }
    }
    if inline {
        return Ok(if is_map_type(&field.ty) {
            YamlField::InlineMap
        } else {
            YamlField::Inline
        });
    }
    Ok(YamlField::Key(if key.is_empty() {
        go_name.to_lowercase()
    } else {
        key.to_owned()
    }))
}

/// A parsed `mapstructure` tag (Go: the parts `decodeStructFromMap` reads).
struct MsTag {
    name: String,
    squash: bool,
    remain: bool,
}

fn ms_tag(tag: &str) -> MsTag {
    let mut parts = tag.split(',');
    let name = parts.next().unwrap_or_default().to_owned();
    let mut squash = false;
    let mut remain = false;
    for flag in parts {
        if flag == "squash" {
            squash = true;
            break;
        }
        if flag == "remain" {
            remain = true;
            break;
        }
    }
    MsTag {
        name,
        squash,
        remain,
    }
}

/// A field of a struct with named fields.
struct FieldInfo<'a> {
    ident: &'a Ident,
    ty: &'a Type,
    go_name: String,
    exported: bool,
    yaml: YamlField,
    ms: MsTag,
    /// The struct a squashed field holds (the field type, or `T` of `Box<T>`), with whether it
    /// is boxed.
    squash_target: Option<(&'a Type, bool)>,
}

/// The shape of the deriving type.
enum Shape<'a> {
    Struct(Vec<FieldInfo<'a>>),
    Newtype(&'a Type),
}

/// The deriving type.
struct Item<'a> {
    name: &'a Ident,
    go_type: String,
    shape: Shape<'a>,
}

impl<'a> Item<'a> {
    fn parse(input: &'a DeriveInput) -> Result<Self> {
        if !input.generics.params.is_empty() {
            return Err(Error::new_spanned(
                &input.generics,
                "GoYaml and GoMapstructure do not support generic types",
            ));
        }
        let go_type = go_type(input)?;
        let Data::Struct(data) = &input.data else {
            return Err(Error::new_spanned(
                &input.ident,
                "GoYaml and GoMapstructure support structs only",
            ));
        };
        let shape = match &data.fields {
            Fields::Named(named) => {
                let mut fields = Vec::new();
                for field in &named.named {
                    if let Some(info) = Self::field(field)? {
                        fields.push(info);
                    }
                }
                Shape::Struct(fields)
            }
            Fields::Unit => Shape::Struct(Vec::new()),
            Fields::Unnamed(unnamed) => match unnamed.unnamed.first() {
                Some(field) if unnamed.unnamed.len() == 1 => Shape::Newtype(&field.ty),
                Some(_) | None => {
                    return Err(Error::new_spanned(
                        &input.ident,
                        "GoYaml and GoMapstructure support structs with named fields, unit \
                         structs and single-field tuple structs",
                    ));
                }
            },
        };
        Ok(Self {
            name: &input.ident,
            go_type,
            shape,
        })
    }

    fn field(field: &'a syn::Field) -> Result<Option<FieldInfo<'a>>> {
        let attrs = field_attrs(field)?;
        let Some(ident) = field.ident.as_ref() else {
            return Ok(None);
        };
        if attrs.skip {
            return Ok(None);
        }
        let go_name = attrs.go.unwrap_or_else(|| upper_camel(ident));
        let exported = go_name.chars().next().is_some_and(char::is_uppercase);
        let yaml = yaml_field(
            field,
            attrs.yaml.as_deref().unwrap_or_default(),
            &go_name,
            exported || attrs.embedded,
        )?;
        let ms = ms_tag(attrs.mapstructure.as_deref().unwrap_or_default());
        let pointer = generic_arg(&field.ty, "Option").is_some();
        if ms.squash && pointer {
            return Err(Error::new_spanned(
                field,
                "`,squash` on a pointer field is not supported",
            ));
        }
        let squash_target = ms.squash.then(|| {
            generic_arg(&field.ty, "Box").map_or((&field.ty, false), |inner| (inner, true))
        });
        Ok(Some(FieldInfo {
            ident,
            ty: &field.ty,
            go_name,
            exported,
            yaml,
            ms,
            squash_target,
        }))
    }

    /// The `#[derive(GoYaml)]` expansion.
    fn yaml(&self) -> TokenStream2 {
        let name = self.name;
        let go_type = &self.go_type;
        let body = match &self.shape {
            Shape::Newtype(inner) => yaml_newtype(name, go_type, inner),
            Shape::Struct(fields) => yaml_struct(name, go_type, fields),
        };
        quote! {
            const _: () = {
                use ::pbs_goyaml::__private as __y;

                #body
            };
        }
    }

    /// The `#[derive(GoMapstructure)]` expansion.
    fn mapstructure(&self) -> TokenStream2 {
        let name = self.name;
        let go_type = &self.go_type;
        let body = match &self.shape {
            Shape::Newtype(inner) => ms_newtype(name, go_type, inner),
            Shape::Struct(fields) => ms_struct(name, go_type, fields),
        };
        quote! {
            const _: () = {
                use ::pbs_goyaml::__private as __y;

                #body
            };
        }
    }
}

fn yaml_newtype(name: &Ident, go_type: &str, inner: &Type) -> TokenStream2 {
    quote! {
        impl __y::YamlDecode for #name {
            const YAML_KIND: __y::GoKind = <#inner as __y::YamlDecode>::YAML_KIND;
            const YAML_OPTION_IS_PTR: bool = <#inner as __y::YamlDecode>::YAML_OPTION_IS_PTR;

            fn yaml_type_name(out: &mut __y::String) {
                out.push_str(#go_type);
            }

            fn yaml_null(&mut self) -> bool {
                __y::YamlDecode::yaml_null(&mut self.0)
            }

            fn yaml_scalar(
                &mut self,
                s: &__y::Scalar<'_>,
            ) -> __y::Result<bool, __y::YamlError> {
                __y::YamlDecode::yaml_scalar(&mut self.0, s)
            }

            fn yaml_sequence(
                &mut self,
                d: &mut __y::YamlDecoder<'_>,
                n: __y::NodeId,
            ) -> __y::Result<__y::Option<bool>, __y::YamlError> {
                __y::YamlDecode::yaml_sequence(&mut self.0, d, n)
            }

            fn yaml_mapping(
                &mut self,
                d: &mut __y::YamlDecoder<'_>,
                n: __y::NodeId,
            ) -> __y::Result<__y::Option<bool>, __y::YamlError> {
                __y::YamlDecode::yaml_mapping(&mut self.0, d, n)
            }

            fn yaml_mapping_nil(
                &mut self,
                d: &mut __y::YamlDecoder<'_>,
                n: __y::NodeId,
                was_nil: bool,
            ) -> __y::Result<__y::Option<bool>, __y::YamlError> {
                __y::YamlDecode::yaml_mapping_nil(&mut self.0, d, n, was_nil)
            }

            fn yaml_key_interface(&self) -> __y::Option<(__y::String, __y::Value)> {
                __y::YamlDecode::yaml_key_interface(&self.0)
                    .map(|(_, value)| (__y::String::from(#go_type), value))
            }

            fn yaml_key_kind(&self) -> __y::Option<__y::GoKind> {
                __y::YamlDecode::yaml_key_kind(&self.0)
            }
        }
    }
}

/// The parts of a struct's `#[derive(GoYaml)]` expansion: the `FieldDesc`s, the arms of
/// `yaml_field`, and the `,inline` map field.
struct YamlParts<'a> {
    descs: Vec<TokenStream2>,
    arms: Vec<TokenStream2>,
    inline_map: Option<&'a Ident>,
}

fn yaml_parts<'a>(fields: &[FieldInfo<'a>]) -> YamlParts<'a> {
    let mut descs = Vec::new();
    let mut arms = Vec::new();
    let mut inline_map = None;
    for (num, field) in fields.iter().enumerate() {
        let f = field.ident;
        let ty = field.ty;
        match &field.yaml {
            YamlField::Skip => {}
            YamlField::Key(key) => {
                descs.push(quote!(__y::FieldDesc::Field { num: #num, key: #key }));
                arms.push(quote! {
                    [#num] => {
                        let _good = d.unmarshal(n, &mut self.#f)?;
                        __y::Ok(())
                    }
                });
            }
            YamlField::Inline => {
                descs.push(quote! {
                    __y::FieldDesc::Inline {
                        num: #num,
                        info: <#ty as __y::YamlInline>::yaml_inline_info,
                    }
                });
                arms.push(quote! {
                    [#num, rest @ ..] => {
                        __y::YamlInline::yaml_inline_field(&mut self.#f, d, rest, n)
                    }
                });
            }
            YamlField::InlineMap => {
                descs.push(quote!(__y::FieldDesc::InlineMap));
                inline_map = inline_map.or(Some(f));
            }
        }
    }
    YamlParts {
        descs,
        arms,
        inline_map,
    }
}

fn yaml_struct(name: &Ident, go_type: &str, fields: &[FieldInfo<'_>]) -> TokenStream2 {
    let YamlParts {
        descs,
        arms,
        inline_map,
    } = yaml_parts(fields);
    let field_body = if arms.is_empty() {
        quote! {
            let _ = (d, path, n);
            __y::Ok(())
        }
    } else {
        quote! {
            match path {
                #(#arms)*
                _ => __y::Ok(()),
            }
        }
    };
    let inline_map = inline_map.map(|f| {
        quote! {
            fn yaml_inline_map(
                &mut self,
                d: &mut __y::YamlDecoder<'_>,
                key: __y::String,
                n: __y::NodeId,
            ) -> __y::Result<(), __y::YamlError> {
                __y::YamlInlineMap::yaml_inline_insert(&mut self.#f, d, key, n)
            }
        }
    });
    quote! {
        impl __y::YamlDecode for #name {
            const YAML_KIND: __y::GoKind = __y::GoKind::Struct;

            fn yaml_type_name(out: &mut __y::String) {
                out.push_str(#go_type);
            }

            fn yaml_mapping(
                &mut self,
                d: &mut __y::YamlDecoder<'_>,
                n: __y::NodeId,
            ) -> __y::Result<__y::Option<bool>, __y::YamlError> {
                d.decode_struct(n, self).map(__y::Some)
            }
        }

        impl __y::YamlStruct for #name {
            fn yaml_struct_info() -> __y::Result<&'static __y::StructInfo, __y::YamlError> {
                static INFO: __y::OnceLock<__y::Result<__y::StructInfo, __y::YamlError>> =
                    __y::OnceLock::new();
                INFO.get_or_init(|| __y::StructInfo::build(#go_type, &[#(#descs),*]))
                    .as_ref()
                    .map_err(__y::Clone::clone)
            }

            fn yaml_field(
                &mut self,
                d: &mut __y::YamlDecoder<'_>,
                path: &[usize],
                n: __y::NodeId,
            ) -> __y::Result<(), __y::YamlError> {
                #field_body
            }

            #inline_map
        }

        impl __y::YamlInline for #name {
            fn yaml_inline_info() -> __y::Result<&'static __y::StructInfo, __y::YamlError> {
                <Self as __y::YamlStruct>::yaml_struct_info()
            }

            fn yaml_inline_field(
                &mut self,
                d: &mut __y::YamlDecoder<'_>,
                path: &[usize],
                n: __y::NodeId,
            ) -> __y::Result<(), __y::YamlError> {
                <Self as __y::YamlStruct>::yaml_field(self, d, path, n)
            }
        }
    }
}

fn ms_newtype(name: &Ident, go_type: &str, inner: &Type) -> TokenStream2 {
    quote! {
        impl __y::MsDecode for #name {
            const MS_KIND: __y::GoKind = <#inner as __y::MsDecode>::MS_KIND;
            const MS_OPTION_IS_PTR: bool = <#inner as __y::MsDecode>::MS_OPTION_IS_PTR;

            fn ms_type_name(out: &mut __y::String) {
                out.push_str(#go_type);
            }

            fn ms_decode(
                &mut self,
                d: &mut __y::MsDecoder,
                name: &str,
                input: &__y::Value,
            ) -> __y::Result<(), __y::MsError> {
                __y::MsDecode::ms_decode_as(
                    &mut self.0,
                    d,
                    name,
                    input,
                    <Self as __y::MsDecode>::ms_type_name,
                )
            }

            fn ms_decode_as(
                &mut self,
                d: &mut __y::MsDecoder,
                name: &str,
                input: &__y::Value,
                ty: fn(&mut __y::String),
            ) -> __y::Result<(), __y::MsError> {
                __y::MsDecode::ms_decode_as(&mut self.0, d, name, input, ty)
            }

            fn ms_zero(&mut self) {
                __y::MsDecode::ms_zero(&mut self.0);
            }

            fn ms_decode_option(
                slot: &mut __y::Option<Self>,
                d: &mut __y::MsDecoder,
                name: &str,
                input: &__y::Value,
            ) -> __y::Result<(), __y::MsError> {
                if <#inner as __y::MsDecode>::MS_OPTION_IS_PTR {
                    return d.decode_ptr(name, input, slot);
                }
                let mut inner = slot.take().map(|value| value.0);
                let result =
                    <#inner as __y::MsDecode>::ms_decode_option(&mut inner, d, name, input);
                *slot = inner.map(Self);
                result
            }
        }
    }
}

fn ms_struct(name: &Ident, go_type: &str, fields: &[FieldInfo<'_>]) -> TokenStream2 {
    let mut descs = Vec::new();
    let mut arms = Vec::new();
    let mut zero = Vec::new();
    for (num, field) in fields.iter().enumerate() {
        let f = field.ident;
        let ty = field.ty;
        let go_name = &field.go_name;
        let MsTag {
            name: tag,
            squash,
            remain,
        } = &field.ms;
        let exported = field.exported;
        let squash_fields = field.squash_target.map_or_else(
            || quote!(__y::None),
            |(target, _)| quote!(__y::Some(<#target as __y::MsStruct>::ms_fields)),
        );
        descs.push(quote! {
            __y::MsField {
                name: #go_name,
                tag: #tag,
                squash: #squash,
                remain: #remain,
                exported: #exported,
                kind: <#ty as __y::MsDecode>::MS_KIND,
                squash_fields: #squash_fields,
            }
        });
        if *remain {
            arms.push(quote!([#num] => d.decode_remain(name, input, &mut self.#f),));
        } else {
            arms.push(
                quote!([#num] => __y::MsDecode::ms_decode_field(&mut self.#f, d, name, input),),
            );
        }
        if let Some((target, boxed)) = field.squash_target {
            let place = if boxed {
                quote!(&mut *self.#f)
            } else {
                quote!(&mut self.#f)
            };
            arms.push(quote! {
                [#num, rest @ ..] => {
                    <#target as __y::MsStruct>::ms_field(#place, d, rest, name, input)
                }
            });
        }
        zero.push(quote!(__y::MsDecode::ms_zero(&mut self.#f);));
    }
    let field_body = if arms.is_empty() {
        quote! {
            let _ = (d, path, name, input);
            __y::Ok(())
        }
    } else {
        quote! {
            match path {
                #(#arms)*
                _ => __y::Ok(()),
            }
        }
    };
    quote! {
        impl __y::MsDecode for #name {
            const MS_KIND: __y::GoKind = __y::GoKind::Struct;

            fn ms_type_name(out: &mut __y::String) {
                out.push_str(#go_type);
            }

            fn ms_decode(
                &mut self,
                d: &mut __y::MsDecoder,
                name: &str,
                input: &__y::Value,
            ) -> __y::Result<(), __y::MsError> {
                d.decode_struct(name, input, self)
            }

            fn ms_zero(&mut self) {
                #(#zero)*
            }
        }

        impl __y::MsStruct for #name {
            fn ms_fields() -> &'static [__y::MsField] {
                const FIELDS: &[__y::MsField] = &[#(#descs),*];
                FIELDS
            }

            fn ms_field(
                &mut self,
                d: &mut __y::MsDecoder,
                path: &[usize],
                name: &str,
                input: &__y::Value,
            ) -> __y::Result<(), __y::MsError> {
                #field_body
            }
        }
    }
}
