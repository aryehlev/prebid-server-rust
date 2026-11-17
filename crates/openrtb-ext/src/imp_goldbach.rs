//! Go: `openrtb_ext/imp_goldbach.go`: the custom targeting values with their `UnmarshalJSON`
//! (`ImpExtGoldbach` is generated).

use pbs_gojson::{GoJson, GoUnmarshaler, JsonError, jsonutil};

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
