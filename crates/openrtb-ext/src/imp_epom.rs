//! Go: `openrtb_ext/imp_epom.go`: the Epom params, a struct without fields.
//!
//! Written by hand for its serde code: the derive reads a unit struct from null only, where Go
//! reads any object into the struct (and null, as every struct); here an object is read with its
//! members ignored, as the generated structs read an object without the members they know.

use core::fmt;

use pbs_gojson::GoJson;
use pbs_jsonutil::{MergeClone, MergeCloneError, merge_struct};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{IgnoredAny, MapAccess, SeqAccess, Visitor},
    ser::SerializeStruct as _,
};
use serde_json::value::RawValue;

/// Go type `openrtb_ext.ImpExtEpom`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "openrtb_ext.ImpExtEpom")]
pub struct ImpExtEpom;

/// Reads [`ImpExtEpom`] as the serde derive reads a struct whose every member is unknown: an
/// object with its members ignored, or an empty array (`json.struct-from-array`).
struct NoMembers;

impl<'de> Visitor<'de> for NoMembers {
    type Value = ImpExtEpom;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("struct ImpExtEpom")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(ImpExtEpom)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, _seq: A) -> Result<Self::Value, A::Error> {
        Ok(ImpExtEpom)
    }
}

impl Serialize for ImpExtEpom {
    /// Go writes the struct as `{}`.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_struct("ImpExtEpom", 0)?.end()
    }
}

impl<'de> Deserialize<'de> for ImpExtEpom {
    /// Reads an object, its members ignored, as the generated structs read the members they do
    /// not know.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct("ImpExtEpom", &[], NoMembers)
    }
}

impl MergeClone for ImpExtEpom {
    /// Go: an object merges nothing into the struct and null leaves it; any other value fails.
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        merge_struct(patch, |_, _| Ok(()))
    }
}
