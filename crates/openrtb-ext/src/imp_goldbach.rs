//! Go: `openrtb_ext/imp_goldbach.go`: the custom targeting values with their `UnmarshalJSON`
//! (`ImpExtGoldbach` is generated).

use pbs_gojson::{GoJson, GoUnmarshaler, JsonError, jsonutil};
use pbs_jsonutil::{MergeClone, MergeCloneError, unmarshal_valid};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::value::RawValue;

/// Go: `stringOrStringArray` (`type stringOrStringArray []string`, unexported) is a custom type
/// that can hold either a single string or an array of strings.
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "openrtb_ext.stringOrStringArray", unmarshaler)]
pub struct StringOrStringArray(pub Option<Vec<String>>);

impl GoUnmarshaler for StringOrStringArray {
    /// Go: `(*stringOrStringArray).UnmarshalJSON`: a string `s` becomes `[s]` (`null` too,
    /// as `[""]`: it decodes as a string), otherwise an array of strings is taken as it is.
    fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), JsonError> {
        // Go: openrtb_ext/imp_goldbach.go:18
        let mut item_result = String::new();
        if jsonutil::unmarshal_valid(data, &mut item_result).is_ok() {
            self.0 = Some(vec![item_result]);
            return Ok(());
        }
        let mut array_result: Option<Vec<String>> = None;
        if jsonutil::unmarshal_valid(data, &mut array_result).is_ok() {
            self.0 = array_result;
            return Ok(());
        }
        // Go: fmt.Errorf("value should be of type %T or %T", itemResult, arrayResult)
        Err(JsonError::msg("value should be of type string or []string"))
    }
}

impl StringOrStringArray {
    /// Go: `(*stringOrStringArray).UnmarshalJSON(b)` for the JSON value `raw`, its decodes
    /// through `pbs_jsonutil::unmarshal_valid`: a string `s` becomes `[s]`, otherwise an array of
    /// strings is taken as it is. `pbs_jsonutil` reads no string from `null`, which jsoniter reads
    /// as `""`, so null is the nil array here where Go's is `[""]`
    /// (`json.string-or-array-null`).
    fn read(raw: &RawValue) -> Result<Self, String> {
        // Go: openrtb_ext/imp_goldbach.go:18
        let data = raw.get().as_bytes();
        if let Ok(item_result) = unmarshal_valid::<String>(data) {
            return Ok(Self(Some(vec![item_result])));
        }
        if let Ok(array_result) = unmarshal_valid::<Option<Vec<String>>>(data) {
            return Ok(Self(array_result));
        }
        // Go: fmt.Errorf("value should be of type %T or %T", itemResult, arrayResult)
        Err("value should be of type string or []string".to_owned())
    }
}

impl Serialize for StringOrStringArray {
    /// Go has no `MarshalJSON`: the array of strings, nil as null.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StringOrStringArray {
    /// Reads through Go's `UnmarshalJSON` (see `StringOrStringArray::read`).
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        Self::read(&raw).map_err(D::Error::custom)
    }
}

impl MergeClone for StringOrStringArray {
    /// Go: `MergeClone` decodes a slice into a new nil one, and jsoniter hands the patch, null
    /// included, to `UnmarshalJSON`, which sets the slice when it reads the patch: one that fails
    /// leaves it nil.
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        self.0 = None;
        *self = Self::read(patch).map_err(MergeCloneError::new)?;
        Ok(())
    }
}
