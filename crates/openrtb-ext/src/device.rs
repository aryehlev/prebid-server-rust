//! Go: `openrtb_ext/device.go`: the interstitial extension with its `UnmarshalJSON`, and the
//! iOS app tracking status helpers (the other structs are generated).

use pbs_errortypes::GoError;
use pbs_gojson::{
    GoJson, GoUnmarshaler, JsonError,
    jsonparser::{self, ValueType},
};
use pbs_jsonutil::{MergeClone, MergeCloneError};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::SerializeStruct as _,
};
use serde_json::value::RawValue;

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
    // `str::parse` reads what `strconv.Atoi` reads: an optional sign and decimal digits.
    match text.parse::<i64>() {
        Ok(perc) if (0..=100).contains(&perc) => Ok(perc),
        Ok(_) | Err(_) => Err(invalid()),
    }
}

/// The members `jsonparser.Get` finds for `(*ExtDeviceInt).UnmarshalJSON`, read as a typed
/// partial struct: an object's `minwidthperc` and `minheightperc`, raw. A repeated member fails
/// the read, where jsonparser takes the first (`json.duplicate-keys`).
#[derive(Deserialize)]
struct PercentMembers<'a> {
    /// The raw value of `minwidthperc`, `None` when absent or null.
    #[serde(borrow)]
    minwidthperc: Option<&'a RawValue>,
    /// The raw value of `minheightperc`, `None` when absent or null.
    #[serde(borrow)]
    minheightperc: Option<&'a RawValue>,
}

impl ExtDeviceInt {
    /// Go: `(*ExtDeviceInt).UnmarshalJSON(b)` for the JSON value `raw`: each percentage must be
    /// a JSON number `strconv.Atoi` reads as 0 to 100. The width is set before the height is
    /// read, so a bad height leaves a good width set, as in Go.
    ///
    /// Go's `len(b) == 0` test has no counterpart: serde hands over one JSON value. jsonparser
    /// finds no member in a value that is not an object, as here, and its `Null` type fails the
    /// number test as `None` does.
    fn read(&mut self, raw: &RawValue) -> Result<(), String> {
        // Go: openrtb_ext/device.go:80
        let members = if raw.get().starts_with('{') {
            serde_json::from_str(raw.get()).map_err(|e| e.to_string())?
        } else {
            PercentMembers {
                minwidthperc: None,
                minheightperc: None,
            }
        };
        self.min_width_perc = raw_percentage(members.minwidthperc, "minwidthperc")?;
        self.min_height_perc = raw_percentage(members.minheightperc, "minheightperc")?;
        Ok(())
    }
}

/// Go: one percentage of `(*ExtDeviceInt).UnmarshalJSON`: a JSON number (jsonparser's `Number`
/// type: a value that starts with `-` or a digit) that `strconv.Atoi` reads as 0 to 100. On the
/// text of a JSON number `str::parse` reads what `Atoi` reads: an optional `-` and decimal
/// digits, failing on a fraction, an exponent or a value beyond `int`'s 64 bits.
fn raw_percentage(value: Option<&RawValue>, key: &str) -> Result<i64, String> {
    let text = value.map_or("", RawValue::get);
    if matches!(text.as_bytes().first(), Some(b'-' | b'0'..=b'9'))
        && let Ok(perc) = text.parse::<i64>()
        && (0..=100).contains(&perc)
    {
        return Ok(perc);
    }
    Err(format!(
        "request.device.ext.prebid.interstitial.{key} must be a number between 0 and 100"
    ))
}

impl Serialize for ExtDeviceInt {
    /// Go has no `MarshalJSON`: the struct, both members always written.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("ExtDeviceInt", 2)?;
        out.serialize_field("minwidthperc", &self.min_width_perc)?;
        out.serialize_field("minheightperc", &self.min_height_perc)?;
        out.end()
    }
}

impl<'de> Deserialize<'de> for ExtDeviceInt {
    /// Reads through Go's `UnmarshalJSON` (see `ExtDeviceInt::read`); null fails as Go's does.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        let mut interstitial = Self::default();
        interstitial.read(&raw).map_err(D::Error::custom)?;
        Ok(interstitial)
    }
}

impl MergeClone for ExtDeviceInt {
    /// Go: jsoniter hands the patch, null included, to `UnmarshalJSON`, which reads it over the
    /// value as `ExtDeviceInt::read` does.
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        self.read(patch).map_err(MergeCloneError::new)
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

/// The member `jsonparser.GetInt` finds for `ParseDeviceExtATTS`, read as a typed partial
/// struct: the top-level `atts` of an object, raw, null included (an `atts` inside another
/// value is ignored, as jsonparser does not look there). A repeated `atts` fails the read, where
/// jsonparser takes the first (`json.jsonparser-scan`).
#[derive(Deserialize)]
struct AttsMember {
    /// The raw value of `atts`, `None` when absent.
    #[serde(default, deserialize_with = "present")]
    atts: Option<Box<RawValue>>,
}

/// A member's raw value, null included, as present.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(deserializer).map(Some)
}

/// Go: `ParseDeviceExtATTS(deviceExt)`: the iOS app tracking status in `request.device.ext`.
///
/// Go reads `atts` with `jsonparser.GetInt`, which scans the bytes as far as the first top-level
/// `atts`. `None` when it is absent, and for bytes that do not start an object: JSON that is not an
/// object has no member, in Go as here. The ext is read with `serde_json`, which fails on a
/// repeated `atts`, where jsonparser takes the first, and on bytes it does not read, where
/// jsonparser reads `atts` as it scans to it (`{"atts":01}` gives 1, `{"atts":2` 2, and
/// `x{"atts":1}`, which gives no status here, 1) and stops at a top-level key with a lone
/// surrogate escape, finding no `atts` after it (`json.jsonparser-scan`). The request's decode
/// writes that escape as U+FFFD, a key `serde_json` reads past, so an `atts` after it gives a
/// status where Go has none. Go's one caller (privacy/lmt/ios.go:52) treats an error as no status.
///
/// # Errors
///
/// `serde_json`'s error for an ext that does not decode or an `atts` that is not an integer
/// (Go's are jsonparser's: `json.error-texts`), or `invalid status` for a number that is not a
/// known status.
pub fn parse_device_ext_atts(device_ext: &[u8]) -> Result<Option<IOSAppTrackingStatus>, GoError> {
    // Go: openrtb_ext/device.go:103
    let object = device_ext
        .iter()
        .find(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        == Some(&b'{');
    if !object {
        return Ok(None);
    }
    let member: AttsMember =
        serde_json::from_slice(device_ext).map_err(|e| GoError::generic(e.to_string()))?;
    let Some(atts) = member.atts else {
        return Ok(None);
    };
    // jsonparser reads a JSON number as an optional `-` and decimal digits, as `str::parse`
    // reads its text (`-0` is 0), failing on a fraction, an exponent and a value beyond 64 bits.
    let v = atts.get().parse::<i64>().map_err(|parse| {
        GoError::generic(
            serde_json::from_str::<i64>(atts.get())
                .err()
                .map_or_else(|| parse.to_string(), |e| e.to_string()),
        )
    })?;
    if !is_known_ios_app_tracking_status(v) {
        return Err(GoError::generic("invalid status"));
    }
    Ok(isize::try_from(v).ok().map(IOSAppTrackingStatus))
}

#[cfg(test)]
mod tests {
    use super::parse_device_ext_atts;
    use crate::generated::IOSAppTrackingStatus;

    /// A test result whose error is the failure message.
    type TestResult = Result<(), String>;

    /// Bytes that `serde_json` does not read fail, where jsonparser reads `atts` as it scans to it
    /// (`json.jsonparser-scan`): go1.26.2 gives 1 for `{"atts":01}` and 2 for `{"atts":2`
    /// (`functions.tsv`), 0 for `{"atts":-}` and `{"x":1,"atts":00}` (`review_functions.tsv`), and,
    /// run by hand, 1 for `{"atts":1,"\ud800":1}` and no status for `{"\ud800":1,"atts":1}`, whose
    /// key ends jsonparser's scan. Only those two pass the request's decode, in Go and in the port,
    /// and `pbs_jsonutil`'s decode writes their escape as U+FFFD (`json.lone-surrogates-raw`),
    /// after which `atts` reads 1 from both: Go's status for the first, and a status where Go has
    /// none for the second. A repeated `atts`, which also passes the request's decode, fails too,
    /// where jsonparser stops at the first: go1.26.2 gives 2 for `{"atts":2,"atts":3}`
    /// (`functions.tsv`). Bytes before an object give no status, where go1.26.2 scans past them
    /// and gives 1 for `x{"atts":1}` (run by hand). Other JSON reads as Go reads it, and other
    /// bytes that do not start an object give no status, as in Go: the values of the last cases
    /// are the goldens'.
    // Go: openrtb_ext/device.go:103-117
    #[test]
    fn atts_read_only_from_json() -> TestResult {
        const LONE_SURROGATE_KEYS: [&str; 2] =
            [r#"{"atts":1,"\ud800":1}"#, r#"{"\ud800":1,"atts":1}"#];
        for doc in [
            r#"{"atts":01}"#,
            r#"{"atts":2"#,
            r#"{"atts":-}"#,
            r#"{"x":1,"atts":00}"#,
            r#"{"atts":2,"atts":3}"#,
        ]
        .into_iter()
        .chain(LONE_SURROGATE_KEYS)
        {
            if let Ok(got) = parse_device_ext_atts(doc.as_bytes()) {
                return Err(format!("{doc}: got {got:?}, want an error"));
            }
        }
        let prefixed = r#"x{"atts":1}"#;
        let prefixed_status =
            parse_device_ext_atts(prefixed.as_bytes()).map_err(|e| format!("{prefixed}: {e}"))?;
        if prefixed_status.is_some() {
            return Err(format!("{prefixed}: got {prefixed_status:?}, want None"));
        }
        for doc in LONE_SURROGATE_KEYS {
            let decoded = pbs_jsonutil::replace_lone_surrogates(doc.as_bytes());
            let rewritten =
                parse_device_ext_atts(&decoded).map_err(|e| format!("{doc} rewritten: {e}"))?;
            if rewritten != Some(IOSAppTrackingStatus(1)) {
                return Err(format!("{doc} rewritten: got {rewritten:?}, want Some(1)"));
            }
        }
        for (doc, want) in [
            ("", None),
            ("malformed", None),
            (r#"[{"atts":1}]"#, None),
            (r#"{"a":{"atts":1}}"#, None),
            (r#"{"ATTS":1}"#, None),
            (r#"{"atts":3}"#, Some(3)),
            (r#"{"x":"y","atts":1}"#, Some(1)),
            (r#"{"a":{"atts":9},"atts":-0}"#, Some(0)),
        ] {
            let got = parse_device_ext_atts(doc.as_bytes()).map_err(|e| format!("{doc}: {e}"))?;
            if got != want.map(IOSAppTrackingStatus) {
                return Err(format!("{doc}: got {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }
}
