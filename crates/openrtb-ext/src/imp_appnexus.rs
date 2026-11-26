//! Go: `openrtb_ext/imp_appnexus.go`: the keywords of the Appnexus params with their
//! `UnmarshalJSON` (`ExtImpAppnexus` is generated).

use std::collections::BTreeMap;

use pbs_gojson::{GoJson, GoUnmarshaler, JsonError, jsonutil};
use pbs_jsonutil::{MergeClone, MergeCloneError, null_default, unmarshal_valid};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::value::RawValue;

/// `ExtImpAppnexusKeywords` (`type ExtImpAppnexusKeywords string`): the keywords of the
/// Appnexus params, from a string, an object or an array of key/value objects.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, GoJson)]
#[gojson(go_type = "openrtb_ext.ExtImpAppnexusKeywords", unmarshaler)]
pub struct ExtImpAppnexusKeywords(pub String);

impl ExtImpAppnexusKeywords {
    /// Go: `(*ExtImpAppnexusKeywords).String()`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Go: `(*ExtImpAppnexusKeywords).UnmarshalJSON(b)` for the JSON value `raw`, its decodes
    /// through `pbs_jsonutil::unmarshal_valid`: an object of keys to values and an array of
    /// `{"key", "value"}` objects become `key=value,` pieces without the last comma, a string is
    /// taken as it is, and any other value leaves the keywords as they are. The keys of an
    /// object come in byte order, where Go ranges over its map in a random order.
    fn read(&mut self, raw: &RawValue) -> Result<(), String> {
        // Go: openrtb_ext/imp_appnexus.go:40
        let data = raw.get().as_bytes();
        match data.first() {
            Some(b'{') => {
                let results: Option<BTreeMap<String, Option<Vec<String>>>> =
                    unmarshal_valid(data).map_err(|e| e.to_string())?;
                let mut keywords = String::new();
                for (key, values) in results.iter().flatten() {
                    push_key_values(&mut keywords, key, values.as_deref().unwrap_or_default());
                }
                if let Some(keywords) = without_trailing_comma(keywords) {
                    self.0 = keywords;
                }
            }
            Some(b'[') => {
                let results: Option<Vec<ExtImpAppnexusKeyVal>> =
                    unmarshal_valid(data).map_err(|e| e.to_string())?;
                let mut kvs = String::new();
                for kv in results.iter().flatten() {
                    push_key_values(&mut kvs, &kv.key, kv.values.as_deref().unwrap_or_default());
                }
                if let Some(kvs) = without_trailing_comma(kvs) {
                    self.0 = kvs;
                }
            }
            Some(b'"') => self.0 = unmarshal_valid(data).map_err(|e| e.to_string())?,
            _ => {}
        }
        Ok(())
    }
}

/// Go: `extImpAppnexusKeyVal` defines the contract for
/// bidrequest.imp[i].ext.prebid.bidder.appnexus.keywords[i]
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, GoJson)]
#[serde(default)]
#[gojson(go_type = "openrtb_ext.extImpAppnexusKeyVal")]
struct ExtImpAppnexusKeyVal {
    #[serde(deserialize_with = "null_default")]
    #[gojson(go = "Key", json = "key,omitempty")]
    key: String,
    #[serde(rename = "value")]
    #[gojson(go = "Values", json = "value,omitempty")]
    values: Option<Vec<String>>,
}

/// Appends `key,`, or `key=value,` per value (Go: `fmt.Sprintf("%s=%s,", key, val)`).
fn push_key_values(out: &mut String, key: &str, values: &[String]) {
    if values.is_empty() {
        out.push_str(key);
        out.push(',');
        return;
    }
    for value in values {
        out.push_str(key);
        out.push('=');
        out.push_str(value);
        out.push(',');
    }
}

/// The keywords without their trailing comma; `None` when there are none (Go then leaves the
/// value as it was).
fn without_trailing_comma(mut keywords: String) -> Option<String> {
    keywords.pop().map(|_| keywords)
}

fn decode_error(err: jsonutil::FailedToUnmarshal) -> JsonError {
    JsonError::msg(err.message.into_bytes())
}

impl GoUnmarshaler for ExtImpAppnexusKeywords {
    /// Go: `(*ExtImpAppnexusKeywords).UnmarshalJSON`, by the value's first byte:
    ///
    /// - `{`: `{"k":["v1","v2"],"e":[]}` becomes `e,k=v1,k=v2`. Go walks the decoded map in random
    ///   order, so the order of its keys varies from call to call; here the keys are in byte order.
    /// - `[`: `[{"key":"k","value":["v"]}]` becomes `k=v`, in array order.
    /// - `"`: the string itself.
    ///
    /// Nothing changes for other values (`null`, numbers, ...) or for an object or array that
    /// yields no keywords. A decode failure is `jsonutil.UnmarshalValid`'s error.
    fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), JsonError> {
        // Go: openrtb_ext/imp_appnexus.go:40
        match data.first() {
            Some(b'{') => {
                let mut results: Option<BTreeMap<String, Option<Vec<String>>>> = None;
                jsonutil::unmarshal_valid(data, &mut results).map_err(decode_error)?;
                let mut keywords = String::new();
                for (key, values) in results.iter().flatten() {
                    push_key_values(&mut keywords, key, values.as_deref().unwrap_or_default());
                }
                if let Some(keywords) = without_trailing_comma(keywords) {
                    self.0 = keywords;
                }
            }
            Some(b'[') => {
                let mut results: Option<Vec<ExtImpAppnexusKeyVal>> = None;
                jsonutil::unmarshal_valid(data, &mut results).map_err(decode_error)?;
                let mut kvs = String::new();
                for kv in results.iter().flatten() {
                    push_key_values(&mut kvs, &kv.key, kv.values.as_deref().unwrap_or_default());
                }
                if let Some(kvs) = without_trailing_comma(kvs) {
                    self.0 = kvs;
                }
            }
            Some(b'"') => {
                let mut keywords = String::new();
                jsonutil::unmarshal_valid(data, &mut keywords).map_err(decode_error)?;
                self.0 = keywords;
            }
            _ => {}
        }
        Ok(())
    }
}

impl Serialize for ExtImpAppnexusKeywords {
    /// Go has no `MarshalJSON`: the string.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ExtImpAppnexusKeywords {
    /// Reads through Go's `UnmarshalJSON` (see `ExtImpAppnexusKeywords::read`).
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        let mut keywords = Self::default();
        keywords.read(&raw).map_err(D::Error::custom)?;
        Ok(keywords)
    }
}

impl MergeClone for ExtImpAppnexusKeywords {
    /// Go: jsoniter hands the patch, null included, to `UnmarshalJSON`, which reads it over the
    /// keywords as `ExtImpAppnexusKeywords::read` does.
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        self.read(patch).map_err(MergeCloneError::new)
    }
}
