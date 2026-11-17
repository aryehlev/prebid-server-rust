//! Go: `openrtb_ext/device.go`: the interstitial extension with its `UnmarshalJSON`, and the
//! iOS app tracking status helpers (the other structs are generated).

use pbs_errortypes::GoError;
use pbs_gocompat::strconv;
use pbs_gojson::{
    GoJson, GoUnmarshaler, JsonError,
    jsonparser::{self, ParserError, ValueType},
};

use crate::generated::{
    IOS_APP_TRACKING_STATUS_AUTHORIZED, IOS_APP_TRACKING_STATUS_DENIED,
    IOS_APP_TRACKING_STATUS_NOT_DETERMINED, IOS_APP_TRACKING_STATUS_RESTRICTED,
    IOSAppTrackingStatus,
};

/// `ExtDeviceInt` defines the contract for bidrequest.device.ext.prebid.interstitial
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "openrtb_ext.ExtDeviceInt", unmarshaler)]
pub struct ExtDeviceInt {
    /// Go field `MinWidthPerc`.
    #[gojson(go = "MinWidthPerc", json = "minwidthperc")]
    pub min_width_perc: i64,
    /// Go field `MinHeightPerc`.
    #[gojson(go = "MinHeightPerc", json = "minheightperc")]
    pub min_height_perc: i64,
}

impl GoUnmarshaler for ExtDeviceInt {
    /// Go: `(*ExtDeviceInt).UnmarshalJSON`: both percentages must be JSON numbers that
    /// `strconv.Atoi` reads as 0 to 100, found by `jsonparser.Get`. The width is stored before
    /// the height is checked, as in Go.
    ///
    /// Go returns `*errortypes.BadInput` errors; only their text survives here (and through
    /// `jsonutil.Unmarshal`, which is how Prebid Server decodes this type).
    fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), JsonError> {
        // Go: openrtb_ext/device.go:77
        if data.is_empty() {
            return Err(JsonError::msg(
                "request.device.ext.prebid.interstitial must have some data in it",
            ));
        }
        self.min_width_perc = percentage(data, "minwidthperc")?;
        self.min_height_perc = percentage(data, "minheightperc")?;
        Ok(())
    }
}

/// One percentage of [`ExtDeviceInt::unmarshal_json`].
fn percentage(data: &[u8], key: &str) -> Result<i64, JsonError> {
    let invalid = || {
        JsonError::msg(format!(
            "request.device.ext.prebid.interstitial.{key} must be a number between 0 and 100"
        ))
    };
    let got = jsonparser::get(data, &[key]);
    if got.data_type != ValueType::Number {
        return Err(invalid());
    }
    let Ok(text) = str::from_utf8(got.value) else {
        return Err(invalid());
    };
    match strconv::atoi(text) {
        Ok(perc) if (0..=100).contains(&perc) => Ok(perc),
        Ok(_) | Err(_) => Err(invalid()),
    }
}

/// Go: `IsKnownIOSAppTrackingStatus(v)`: whether `v` is a known iOS app tracking authorization
/// status (0 to 3).
#[must_use]
pub fn is_known_ios_app_tracking_status(v: i64) -> bool {
    let Ok(v) = isize::try_from(v) else {
        return false;
    };
    matches!(
        IOSAppTrackingStatus(v),
        IOS_APP_TRACKING_STATUS_NOT_DETERMINED
            | IOS_APP_TRACKING_STATUS_RESTRICTED
            | IOS_APP_TRACKING_STATUS_DENIED
            | IOS_APP_TRACKING_STATUS_AUTHORIZED
    )
}

/// Go: `ParseDeviceExtATTS(deviceExt)`: the `atts` value of `request.device.ext`, read with
/// `jsonparser.GetInt` (`None` when it is absent, which is also what malformed JSON gives).
///
/// # Errors
///
/// jsonparser's error (`Value is not a number: "1"`, ...), or `invalid status` for a number
/// that is not a known status.
pub fn parse_device_ext_atts(device_ext: &[u8]) -> Result<Option<IOSAppTrackingStatus>, GoError> {
    // Go: openrtb_ext/device.go:103
    let v = match jsonparser::get_int(device_ext, &["atts"]) {
        Ok(v) => v,
        Err(ParserError::KeyPathNotFound) => return Ok(None),
        Err(err) => {
            return Err(GoError::generic(
                String::from_utf8_lossy(&err.go_text()).into_owned(),
            ));
        }
    };
    if !is_known_ios_app_tracking_status(v) {
        return Err(GoError::generic("invalid status"));
    }
    Ok(isize::try_from(v).ok().map(IOSAppTrackingStatus))
}
