//! The Go JSON traits of the package's named map types (`type T map[K]V`), written out instead of
//! derived: `#[derive(GoJson)]` decodes a newtype through its field, and `pbs_gojson` decodes an
//! `Option<map>` field under the map type's own name, so `encoding/json`'s errors would name
//! `map[K]V` where Go's name the named type (`json: cannot unmarshal array into Go value of type
//! openrtb_ext.DealTierBidderMap`), and a non-`null` literal behind a pointer would leave an empty
//! map where Go leaves the map nil. jsoniter and the encoders behave as derived.
//!
//! The serde impls are those a generated named type has: the map reads and writes as the
//! `Option<BTreeMap>` it holds (`#[serde(transparent)]`), and merges as that map does.

/// Implements `GoType`, `GoEncode`, `JsoniterDecode`, `StdDecode`, `Serialize`, `Deserialize`
/// and `MergeClone` for `$name`, a newtype of `Option<BTreeMap<$key, $value>>` (nil is `None`)
/// that Go names `$go`.
macro_rules! named_map {
    ($name:ident, $go:literal, $key:ty, $value:ty) => {
        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                ::serde::Serialize::serialize(&self.0, serializer)
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Self, D::Error> {
                <Option<BTreeMap<$key, $value>> as ::serde::Deserialize<'de>>::deserialize(
                    deserializer,
                )
                .map(Self)
            }
        }

        impl ::pbs_jsonutil::MergeClone for $name {
            fn merge_clone(
                &mut self,
                patch: &::serde_json::value::RawValue,
            ) -> Result<(), ::pbs_jsonutil::MergeCloneError> {
                ::pbs_jsonutil::MergeClone::merge_clone(&mut self.0, patch)
            }
        }

        impl ::pbs_gojson::GoType for $name {
            const KIND: ::pbs_gojson::GoKind =
                <Option<BTreeMap<$key, $value>> as ::pbs_gojson::GoType>::KIND;

            fn go_type_name(out: &mut String) {
                out.push_str($go);
            }
        }

        impl ::pbs_gojson::GoEncode for $name {
            fn go_encode(&self, enc: &mut ::pbs_gojson::Encoder) {
                ::pbs_gojson::GoEncode::go_encode(&self.0, enc);
            }

            fn go_is_empty(&self) -> bool {
                ::pbs_gojson::GoEncode::go_is_empty(&self.0)
            }

            fn go_encode_quoted(&self, enc: &mut ::pbs_gojson::Encoder) {
                ::pbs_gojson::GoEncode::go_encode_quoted(&self.0, enc);
            }
        }

        impl ::pbs_gojson::JsoniterDecode for $name {
            fn jsoniter_decode(&mut self, it: &mut ::pbs_gojson::jsoniter::Iter<'_>) {
                ::pbs_gojson::JsoniterDecode::jsoniter_decode(&mut self.0, it);
            }

            fn jsoniter_decode_string_mode(&mut self, it: &mut ::pbs_gojson::jsoniter::Iter<'_>) {
                ::pbs_gojson::JsoniterDecode::jsoniter_decode_string_mode(&mut self.0, it);
            }
        }

        impl ::pbs_gojson::StdDecode for $name {
            // Go: encoding/json/decode.go (`object` and `literalStore` name the value's type).
            fn std_decode_as<N: ::pbs_gojson::GoType + ?Sized>(
                &mut self,
                d: &mut ::pbs_gojson::stdjson::StdDecoder<'_>,
            ) -> Result<(), ::pbs_gojson::JsonError> {
                ::pbs_gojson::stdjson::std_decode_map::<N, BTreeMap<$key, $value>>(&mut self.0, d)
            }

            // Go: encoding/json/decode.go (`literalStore`: `null` sets a map to nil; any other
            // literal is a type error that leaves the map as it is).
            fn std_literal_as<N: ::pbs_gojson::GoType + ?Sized>(
                &mut self,
                item: &[u8],
                from_quoted: bool,
                d: &mut ::pbs_gojson::stdjson::StdDecoder<'_>,
            ) -> Result<(), ::pbs_gojson::JsonError> {
                use ::pbs_gojson::stdjson::{Class, Store, literal_store};

                if let Store::Null = literal_store::<N>(item, from_quoted, d, Class::Nilable)? {
                    self.0 = None;
                }
                Ok(())
            }
        }
    };
}

pub(crate) use named_map;
