//! Derive macros for pbs-gojson: `#[derive(GoJson)]` declares a Rust type's Go identity and
//! generates Go-faithful JSON encoding and decoding for it. See the `pbs-gojson` crate
//! documentation for the attribute reference.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{
    Data, DeriveInput, Error, Fields, GenericArgument, Ident, LitStr, PathArguments, Result, Type,
    parse_macro_input,
};

/// Derives `GoType`, `GoEncode`, `JsoniterDecode` and `StdDecode` (and, for structs with named
/// fields, `GoStruct`).
///
/// Container attributes (`#[gojson(...)]` on the type):
///
/// * `go_type = "pkg.Name"` (required): Go's `reflect.Type.String()`.
/// * `anonymous`: the Go type is an unnamed struct type (no `pkg.Type.` error prefix in jsoniter
///   decoding, empty struct name in `encoding/json` errors).
/// * `unmarshaler`: the type implements `GoUnmarshaler` (Go `UnmarshalJSON`); both decoders call
///   it.
/// * `marshaler`: the type implements `GoMarshaler` (Go `MarshalJSON`, value receiver).
/// * `map_key` (single-field tuple structs only): the named type is also a map key type
///   (`map[openrtb_ext.BidderName]T`), keyed like its underlying type.
///
/// Field attributes (`#[gojson(...)]` on a named field):
///
/// * `go = "ID"`: the Go field name (default: the Rust name in `UpperCamelCase`).
/// * `json = "id,omitempty"`: the `json` struct tag (default: no tag).
/// * `embedded`: the Go field is an embedded struct (`T`, `*T` as `Option<T>` or `Option<Box<T>>`).
/// * `skip`: the field does not exist in Go.
///
/// A tuple struct with one field is a Go named type over the field's type (`type BidType
/// string`): it encodes and decodes as that type, but under its own Go name.
#[proc_macro_derive(GoJson, attributes(gojson))]
pub fn derive_go_json(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// The JSON methods the Go type declares.
#[derive(Default)]
struct GoMethods {
    unmarshaler: bool,
    marshaler: bool,
}

#[derive(Default)]
struct ContainerAttrs {
    go_type: Option<String>,
    anonymous: bool,
    map_key: bool,
    methods: GoMethods,
}

#[derive(Default)]
struct FieldAttrs {
    go: Option<String>,
    json: Option<String>,
    embedded: bool,
    skip: bool,
}

fn container_attrs(input: &DeriveInput) -> Result<ContainerAttrs> {
    let mut attrs = ContainerAttrs::default();
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("gojson")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("go_type") {
                attrs.go_type = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("anonymous") {
                attrs.anonymous = true;
            } else if meta.path.is_ident("unmarshaler") {
                attrs.methods.unmarshaler = true;
            } else if meta.path.is_ident("marshaler") {
                attrs.methods.marshaler = true;
            } else if meta.path.is_ident("map_key") {
                attrs.map_key = true;
            } else {
                return Err(meta.error("unknown gojson container attribute"));
            }
            Ok(())
        })?;
    }
    Ok(attrs)
}

fn field_attrs(field: &syn::Field) -> Result<FieldAttrs> {
    let mut attrs = FieldAttrs::default();
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("gojson")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("go") {
                attrs.go = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("json") {
                attrs.json = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("embedded") {
                attrs.embedded = true;
            } else if meta.path.is_ident("skip") {
                attrs.skip = true;
            } else {
                return Err(meta.error("unknown gojson field attribute"));
            }
            Ok(())
        })?;
    }
    Ok(attrs)
}

/// `snake_case` → `UpperCamelCase` (the default Go field name).
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

/// The single type argument of `Name<T>`, if `ty` is that.
fn generic_arg<'a>(ty: &'a Type, name: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else {
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

/// How an embedded field holds its struct.
struct EmbedShape<'a> {
    pointer: bool,
    boxed: bool,
    target: &'a Type,
}

fn embed_shape(ty: &Type) -> EmbedShape<'_> {
    let (pointer, held) = generic_arg(ty, "Option").map_or((false, ty), |inner| (true, inner));
    let (boxed, target) = generic_arg(held, "Box").map_or((false, held), |inner| (true, inner));
    EmbedShape {
        pointer,
        boxed,
        target,
    }
}

struct FieldInfo<'a> {
    ident: &'a Ident,
    ty: &'a Type,
    go_name: String,
    tag: String,
    embed: Option<EmbedShape<'a>>,
}

impl FieldInfo<'_> {
    /// `&mut T` of the embedded struct, allocating a nil pointer (`dereferenceDecoder`).
    fn embed_mut(&self) -> TokenStream2 {
        let f = self.ident;
        match &self.embed {
            Some(EmbedShape {
                pointer: true,
                boxed: true,
                ..
            }) => quote!(&mut **self.#f.get_or_insert_with(__g::Default::default)),
            Some(EmbedShape {
                pointer: true,
                boxed: false,
                ..
            }) => quote!(self.#f.get_or_insert_with(__g::Default::default)),
            Some(EmbedShape {
                pointer: false,
                boxed: true,
                ..
            }) => quote!(&mut *self.#f),
            Some(EmbedShape {
                pointer: false,
                boxed: false,
                ..
            })
            | None => quote!(&mut self.#f),
        }
    }

    /// `Option<&T>` of the embedded struct (`None` for a nil pointer).
    fn embed_ref(&self) -> TokenStream2 {
        let f = self.ident;
        match &self.embed {
            Some(EmbedShape {
                pointer: true,
                boxed: true,
                ..
            }) => quote!(self.#f.as_deref()),
            Some(EmbedShape {
                pointer: true,
                boxed: false,
                ..
            }) => quote!(self.#f.as_ref()),
            Some(EmbedShape {
                pointer: false,
                boxed: true,
                ..
            }) => quote!(__g::Some(&*self.#f)),
            Some(EmbedShape {
                pointer: false,
                boxed: false,
                ..
            })
            | None => quote!(__g::Some(&self.#f)),
        }
    }
}

fn expand(input: &DeriveInput) -> Result<TokenStream2> {
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "GoJson does not support generic types",
        ));
    }
    let attrs = container_attrs(input)?;
    let Some(go_type) = attrs.go_type.clone() else {
        return Err(Error::new(
            Span::call_site(),
            "GoJson needs the Go type name: #[gojson(go_type = \"pkg.Name\")]",
        ));
    };
    let Data::Struct(data) = &input.data else {
        return Err(Error::new_spanned(
            &input.ident,
            "GoJson supports structs only",
        ));
    };
    let newtype = match &data.fields {
        Fields::Unnamed(unnamed) => unnamed.unnamed.len() == 1,
        Fields::Named(_) | Fields::Unit => false,
    };
    if attrs.map_key && !newtype {
        return Err(Error::new_spanned(
            &input.ident,
            "gojson(map_key) needs a single-field tuple struct (a Go named type)",
        ));
    }
    match &data.fields {
        Fields::Named(named) => {
            let mut fields = Vec::new();
            for field in &named.named {
                let fa = field_attrs(field)?;
                if fa.skip {
                    continue;
                }
                let Some(ident) = field.ident.as_ref() else {
                    continue;
                };
                fields.push(FieldInfo {
                    ident,
                    ty: &field.ty,
                    go_name: fa.go.unwrap_or_else(|| upper_camel(ident)),
                    tag: fa.json.unwrap_or_default(),
                    embed: fa.embedded.then(|| embed_shape(&field.ty)),
                });
            }
            Ok(expand_struct(input, &attrs, &go_type, &fields))
        }
        Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => {
            let inner = unnamed.unnamed.first().map(|f| &f.ty);
            inner.map_or_else(
                || {
                    Err(Error::new_spanned(
                        &input.ident,
                        "GoJson newtype needs one field",
                    ))
                },
                |inner| Ok(expand_newtype(input, &attrs, &go_type, inner)),
            )
        }
        // A unit struct is a Go struct without fields (`struct{}`).
        Fields::Unit => Ok(expand_struct(input, &attrs, &go_type, &[])),
        Fields::Unnamed(_) => Err(Error::new_spanned(
            &input.ident,
            "GoJson supports structs with named fields, unit structs and single-field tuple \
             structs",
        )),
    }
}

/// The bodies of the generated decoding methods.
struct DecodeBodies {
    jsoniter: TokenStream2,
    jsoniter_extra: TokenStream2,
    std_value: TokenStream2,
    std_literal: TokenStream2,
}

fn decode_impls(name: &Ident, attrs: &ContainerAttrs, bodies: DecodeBodies) -> TokenStream2 {
    let (jsoniter, jsoniter_extra, std_value, std_literal, from_quoted) =
        if attrs.methods.unmarshaler {
            (
                quote!(__g::jsoniter_unmarshal(self, it);),
                quote!(),
                quote!(__g::std_unmarshal(self, d)),
                quote!(__g::std_unmarshal_literal::<N, Self>(self, item, d)),
                quote!(_from_quoted),
            )
        } else {
            (
                bodies.jsoniter,
                bodies.jsoniter_extra,
                bodies.std_value,
                bodies.std_literal,
                quote!(from_quoted),
            )
        };
    quote! {
        impl __g::JsoniterDecode for #name {
            fn jsoniter_decode(&mut self, it: &mut __g::Iter<'_>) {
                #jsoniter
            }

            #jsoniter_extra
        }

        impl __g::StdDecode for #name {
            fn std_decode_as<N: __g::GoType + ?Sized>(
                &mut self,
                d: &mut __g::StdDecoder<'_>,
            ) -> __g::Result<(), __g::JsonError> {
                #std_value
            }

            fn std_literal_as<N: __g::GoType + ?Sized>(
                &mut self,
                item: &[u8],
                #from_quoted: bool,
                d: &mut __g::StdDecoder<'_>,
            ) -> __g::Result<(), __g::JsonError> {
                #std_literal
            }
        }
    }
}

fn expand_newtype(
    input: &DeriveInput,
    attrs: &ContainerAttrs,
    go_type: &str,
    inner: &Type,
) -> TokenStream2 {
    let name = &input.ident;
    let encode = if attrs.methods.marshaler {
        quote!(__g::encode_marshaler(self, enc);)
    } else {
        quote!(__g::GoEncode::go_encode(&self.0, enc);)
    };
    let option_encode = option_encode(attrs);
    let map_key = if attrs.map_key {
        quote! {
            impl __g::GoMapKey for #name {
                fn key_text(&self) -> __g::Cow<'_, [u8]> {
                    __g::GoMapKey::key_text(&self.0)
                }

                fn write_jsoniter_key(&self, enc: &mut __g::Encoder) {
                    __g::GoMapKey::write_jsoniter_key(&self.0, enc);
                }

                fn std_parse_key(key: &[u8]) -> __g::Option<Self> {
                    <#inner as __g::GoMapKey>::std_parse_key(key).map(Self)
                }

                fn jsoniter_decode_key(it: &mut __g::Iter<'_>) -> Self {
                    Self(<#inner as __g::GoMapKey>::jsoniter_decode_key(it))
                }
            }
        }
    } else {
        quote!()
    };
    let decode = decode_impls(
        name,
        attrs,
        DecodeBodies {
            jsoniter: quote!(__g::JsoniterDecode::jsoniter_decode(&mut self.0, it);),
            jsoniter_extra: quote! {
                fn jsoniter_decode_string_mode(&mut self, it: &mut __g::Iter<'_>) {
                    __g::JsoniterDecode::jsoniter_decode_string_mode(&mut self.0, it);
                }
            },
            std_value: quote!(__g::StdDecode::std_decode_as::<N>(&mut self.0, d)),
            std_literal: quote!(__g::StdDecode::std_literal_as::<N>(
                &mut self.0,
                item,
                from_quoted,
                d
            )),
        },
    );
    quote! {
        const _: () = {
            use ::pbs_gojson::__private as __g;

            impl __g::GoType for #name {
                const KIND: __g::GoKind = <#inner as __g::GoType>::KIND;

                fn go_type_name(out: &mut __g::String) {
                    out.push_str(#go_type);
                }
            }

            impl __g::GoEncode for #name {
                fn go_encode(&self, enc: &mut __g::Encoder) {
                    #encode
                }

                fn go_is_empty(&self) -> bool {
                    __g::GoEncode::go_is_empty(&self.0)
                }

                fn go_encode_quoted(&self, enc: &mut __g::Encoder) {
                    __g::GoEncode::go_encode_quoted(&self.0, enc);
                }

                #option_encode
            }

            #map_key

            #decode
        };
    }
}

/// A `MarshalJSON` type encodes its pointer through the method too (`*T` implements
/// `json.Marshaler`), so `encoding/json` errors name the pointer type.
fn option_encode(attrs: &ContainerAttrs) -> TokenStream2 {
    if !attrs.methods.marshaler {
        return quote!();
    }
    quote! {
        fn option_go_encode(value: &__g::Option<Self>, enc: &mut __g::Encoder) {
            __g::encode_marshaler_ptr(value.as_ref(), enc);
        }
    }
}

fn field_desc(field: &FieldInfo<'_>) -> TokenStream2 {
    let ty = field.ty;
    let go_name = &field.go_name;
    let tag = &field.tag;
    let embedded = field.embed.as_ref().map_or_else(
        || quote!(__g::None),
        |shape| {
            let pointer = shape.pointer;
            let target = shape.target;
            quote!(__g::Some(__g::Embedded {
                pointer: #pointer,
                desc: <#target as __g::GoStruct>::go_struct_desc,
            }))
        },
    );
    quote! {
        __g::FieldDesc {
            go_name: #go_name,
            tag: #tag,
            kind: <#ty as __g::GoType>::KIND,
            deref_kind: <#ty as __g::GoType>::DEREF_KIND,
            type_name: <#ty as __g::GoType>::go_type_name,
            embedded: #embedded,
        }
    }
}

/// The per-field `match` arms of the `GoStruct` methods.
struct Arms {
    jsoniter: Vec<TokenStream2>,
    std: Vec<TokenStream2>,
    encode: Vec<TokenStream2>,
    empty: Vec<TokenStream2>,
    reachable: Vec<TokenStream2>,
}

fn field_arms(fields: &[FieldInfo<'_>]) -> Arms {
    let mut arms = Arms {
        jsoniter: Vec::new(),
        std: Vec::new(),
        encode: Vec::new(),
        empty: Vec::new(),
        reachable: Vec::new(),
    };
    for (index, field) in fields.iter().enumerate() {
        let f = field.ident;
        let go_name = &field.go_name;
        arms.jsoniter.push(quote! {
            [#index] => {
                __g::jsoniter_leaf(&mut self.#f, mode, it);
                it.wrap_error(#go_name, ": ");
            }
        });
        arms.std
            .push(quote!([#index] => __g::std_leaf(&mut self.#f, op, d),));
        arms.encode.push(quote! {
            [#index] => {
                __g::encode_leaf(&self.#f, mode, enc);
                enc.wrap_error(#go_name, ": ");
            }
        });
        arms.empty
            .push(quote!([#index] => __g::GoEncode::go_is_empty(&self.#f),));
        if field.embed.is_some() {
            let embed_mut = field.embed_mut();
            let embed_ref = field.embed_ref();
            arms.jsoniter.push(quote! {
                [#index, rest @ ..] => {
                    __g::GoStruct::go_jsoniter_field(#embed_mut, rest, mode, it);
                    it.wrap_error(#go_name, ": ");
                }
            });
            arms.std.push(quote! {
                [#index, rest @ ..] => __g::GoStruct::go_std_field(#embed_mut, rest, op, d),
            });
            arms.encode.push(quote! {
                [#index, rest @ ..] => {
                    if let __g::Some(inner) = #embed_ref {
                        __g::GoStruct::go_encode_field(inner, rest, mode, enc);
                    }
                    enc.wrap_error(#go_name, ": ");
                }
            });
            arms.empty.push(quote! {
                [#index, rest @ ..] => match #embed_ref {
                    __g::Some(inner) => __g::GoStruct::go_field_is_empty(inner, rest),
                    __g::None => true,
                },
            });
            arms.reachable.push(quote! {
                [#index, rest @ ..] => match #embed_ref {
                    __g::Some(inner) => __g::GoStruct::go_field_reachable(inner, rest),
                    __g::None => false,
                },
            });
        }
    }
    arms
}

fn expand_struct(
    input: &DeriveInput,
    attrs: &ContainerAttrs,
    go_type: &str,
    fields: &[FieldInfo<'_>],
) -> TokenStream2 {
    let name = &input.ident;
    let go_struct = go_struct_impl(name, attrs, go_type, fields);
    let go_encode = if attrs.methods.marshaler {
        quote!(__g::encode_marshaler(self, enc);)
    } else {
        quote!(__g::encode_struct(self, enc);)
    };
    let option_encode = option_encode(attrs);
    let decode = decode_impls(
        name,
        attrs,
        DecodeBodies {
            jsoniter: quote!(__g::jsoniter_decode_struct(self, it);),
            jsoniter_extra: quote!(),
            std_value: quote!(__g::std_decode_struct_as::<N, Self>(self, d)),
            std_literal: quote!(__g::std_struct_literal::<N>(item, from_quoted, d)),
        },
    );
    quote! {
        const _: () = {
            use ::pbs_gojson::__private as __g;

            impl __g::GoType for #name {
                const KIND: __g::GoKind = __g::GoKind::Struct;

                fn go_type_name(out: &mut __g::String) {
                    out.push_str(#go_type);
                }
            }

            #go_struct

            impl __g::GoEncode for #name {
                fn go_encode(&self, enc: &mut __g::Encoder) {
                    #go_encode
                }

                fn go_is_empty(&self) -> bool {
                    false
                }

                #option_encode
            }

            #decode
        };
    }
}

/// The `GoStruct` impl: the field descriptions and per-field dispatch.
fn go_struct_impl(
    name: &Ident,
    attrs: &ContainerAttrs,
    go_type: &str,
    fields: &[FieldInfo<'_>],
) -> TokenStream2 {
    let named = !attrs.anonymous;
    let descs: Vec<TokenStream2> = fields.iter().map(field_desc).collect();
    let Arms {
        jsoniter,
        std,
        encode,
        empty,
        reachable,
    } = field_arms(fields);
    quote! {
        impl __g::GoStruct for #name {
            fn go_struct_desc() -> &'static __g::StructDesc {
                static DESC: __g::StructDesc = __g::StructDesc {
                    go_type: #go_type,
                    named: #named,
                    fields: &[#(#descs),*],
                };
                &DESC
            }

            fn go_struct_plans() -> &'static __g::Plans {
                static PLANS: __g::OnceLock<__g::Plans> = __g::OnceLock::new();
                PLANS.get_or_init(|| __g::Plans::build(Self::go_struct_desc()))
            }

            fn go_jsoniter_field(
                &mut self,
                path: &[usize],
                mode: __g::StringMode,
                it: &mut __g::Iter<'_>,
            ) {
                match path {
                    #(#jsoniter)*
                    _ => {}
                }
            }

            fn go_std_field(
                &mut self,
                path: &[usize],
                op: __g::StdOp<'_>,
                d: &mut __g::StdDecoder<'_>,
            ) -> __g::Result<(), __g::JsonError> {
                match path {
                    #(#std)*
                    _ => __g::Ok(()),
                }
            }

            fn go_encode_field(
                &self,
                path: &[usize],
                mode: __g::StringMode,
                enc: &mut __g::Encoder,
            ) {
                match path {
                    #(#encode)*
                    _ => {}
                }
            }

            fn go_field_is_empty(&self, path: &[usize]) -> bool {
                match path {
                    #(#empty)*
                    _ => true,
                }
            }

            fn go_field_reachable(&self, path: &[usize]) -> bool {
                match path {
                    #(#reachable)*
                    _ => true,
                }
            }
        }
    }
}
