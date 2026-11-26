//! Go: `openrtb_ext/site.go`: the site extension with its `UnmarshalJSON`.
//!
//! The serde impls keep the unmarshaler: [`ExtSite`] reads (and merges, as jsoniter hands a
//! merged value to `UnmarshalJSON`) through it, and writes as Go's encoders write the struct.

use pbs_gojson::{
    GoJson, GoUnmarshaler, JsonError,
    jsonparser::{self, ValueType},
};
use pbs_jsonutil::{MergeClone, MergeCloneError};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::SerializeStruct as _,
};
use serde_json::value::RawValue;

/// `ExtSite` defines the contract for bidrequest.site.ext
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "openrtb_ext.ExtSite", unmarshaler)]
pub struct ExtSite {
    /// AMP should be 1 if the request comes from an AMP page, and 0 if not.
    #[gojson(go = "AMP", json = "amp")]
    pub amp: i8,
}

const INVALID_AMP: &str = "request.site.ext.amp must be either 1, 0, or undefined";

impl GoUnmarshaler for ExtSite {
    /// Go: `(*ExtSite).UnmarshalJSON`: `amp` must be the one-character JSON number `0` or `1`.
    /// Despite the message, an absent `amp` is rejected too: `jsonparser.Get` then returns no
    /// value, which fails the one-character test.
    fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), JsonError> {
        // Go: openrtb_ext/site.go:15
        if data.is_empty() {
            return Err(JsonError::msg("request.site.ext must have some data in it"));
        }
        let got = jsonparser::get(data, &["amp"]);
        if (got.data_type != ValueType::NotExist && got.data_type != ValueType::Number)
            || got.value.len() != 1
        {
            return Err(JsonError::msg(INVALID_AMP));
        }
        match got.value.first() {
            Some(b'0') => self.amp = 0,
            Some(b'1') => self.amp = 1,
            _ => return Err(JsonError::msg(INVALID_AMP)),
        }
        Ok(())
    }
}

/// The member `jsonparser.Get(b, "amp")` finds, read as a typed partial struct: an object's
/// `amp`, raw. A repeated `amp` fails the read, where jsonparser takes the first
/// (`json.duplicate-keys`).
#[derive(Deserialize)]
struct AmpMember<'a> {
    /// The raw value of `amp`, `None` when absent or null.
    #[serde(borrow)]
    amp: Option<&'a RawValue>,
}

impl ExtSite {
    /// Go: `(*ExtSite).UnmarshalJSON(b)` for the JSON value `raw`: `amp` must be the
    /// one-character JSON number `0` or `1`, and is set only then.
    ///
    /// Go's `len(b) == 0` test has no counterpart: serde hands over one JSON value. jsonparser
    /// finds no member in a value that is not an object, as here, and its `Null` type fails the
    /// number test as `None` does.
    fn read(&mut self, raw: &RawValue) -> Result<(), String> {
        // Go: openrtb_ext/site.go:18
        if !raw.get().starts_with('{') {
            return Err(INVALID_AMP.to_owned());
        }
        let member: AmpMember<'_> = serde_json::from_str(raw.get()).map_err(|e| e.to_string())?;
        match member.amp.map(RawValue::get) {
            Some("0") => self.amp = 0,
            Some("1") => self.amp = 1,
            Some(_) | None => return Err(INVALID_AMP.to_owned()),
        }
        Ok(())
    }
}

impl Serialize for ExtSite {
    /// Go has no `MarshalJSON`: the struct, `amp` always written.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("ExtSite", 1)?;
        out.serialize_field("amp", &self.amp)?;
        out.end()
    }
}

impl<'de> Deserialize<'de> for ExtSite {
    /// Reads through Go's `UnmarshalJSON` (see `ExtSite::read`); null fails as Go's does.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        let mut site = Self::default();
        site.read(&raw).map_err(D::Error::custom)?;
        Ok(site)
    }
}

impl MergeClone for ExtSite {
    /// Go: jsoniter hands the patch, null included, to `UnmarshalJSON`, which sets `amp` only
    /// when it succeeds.
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        self.read(patch).map_err(MergeCloneError::new)
    }
}
