//! Go: `openrtb_ext/site.go`: the site extension with its `UnmarshalJSON`.

use pbs_gojson::{
    GoJson, GoUnmarshaler, JsonError,
    jsonparser::{self, ValueType},
};

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
