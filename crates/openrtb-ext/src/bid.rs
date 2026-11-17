//! Go: `openrtb_ext/bid.go`: bid types and targeting keys (the structs are generated).

use std::borrow::Cow;

use pbs_errortypes::GoError;

use crate::{bidders::BidderName, cow_str::cow_string_type};

cow_string_type! {
    /// `BidType` describes the allowed values for bidresponse.seatbid.bid[i].ext.prebid.type
    pub struct BidType = "openrtb_ext.BidType";
}

/// Go constant `BidTypeBanner`.
#[doc(alias = "BidTypeBanner")]
pub const BID_TYPE_BANNER: BidType = BidType::from_static("banner");
/// Go constant `BidTypeVideo`.
#[doc(alias = "BidTypeVideo")]
pub const BID_TYPE_VIDEO: BidType = BidType::from_static("video");
/// Go constant `BidTypeAudio`.
#[doc(alias = "BidTypeAudio")]
pub const BID_TYPE_AUDIO: BidType = BidType::from_static("audio");
/// Go constant `BidTypeNative`.
#[doc(alias = "BidTypeNative")]
pub const BID_TYPE_NATIVE: BidType = BidType::from_static("native");

/// Go: `BidTypes()`: every bid type, banner, video, audio, native.
#[must_use]
pub fn bid_types() -> Vec<BidType> {
    vec![
        BID_TYPE_BANNER,
        BID_TYPE_VIDEO,
        BID_TYPE_AUDIO,
        BID_TYPE_NATIVE,
    ]
}

/// Go: `ParseBidType(bidType)`.
///
/// # Errors
///
/// `invalid BidType: %s` for anything but the four exact names.
pub fn parse_bid_type(bid_type: &str) -> Result<BidType, GoError> {
    match bid_type {
        "banner" => Ok(BID_TYPE_BANNER),
        "video" => Ok(BID_TYPE_VIDEO),
        "audio" => Ok(BID_TYPE_AUDIO),
        "native" => Ok(BID_TYPE_NATIVE),
        _ => Err(GoError::generic(format!("invalid BidType: {bid_type}"))),
    }
}

cow_string_type! {
    /// `TargetingKey`s are the keys Prebid sets for an ad server like DFP.
    ///
    /// Clients set the values we assign on the request to the ad server, where they can be
    /// substituted like macros into Creatives.
    ///
    /// Removing one of these, or changing the semantics of what we store there, will probably
    /// break the line item setups for many publishers.
    ///
    /// These are especially important to Prebid Mobile. It's much more cumbersome for a Mobile
    /// App to update code than it is for a website. As a result, they rely heavily on these
    /// targeting keys so that any changes can be made on Prebid Server and the Ad Server's line
    /// items.
    pub struct TargetingKey = "openrtb_ext.TargetingKey";

    /// Go: `TargetingKey.BidderKey(prefix, bidder, maxLength)`: `prefix + key + "_" + bidder`,
    /// cut to `maxLength` bytes unless it is 0.
    ///
    /// Go slices the string's bytes: a cut inside a UTF-8 sequence leaves bytes a Rust
    /// `String` cannot hold, which become U+FFFD. Go panics on a negative `maxLength` (the
    /// exchange only passes positive ones); the key is then returned whole.
    #[must_use]
    pub fn bidder_key(&self, prefix: &str, bidder: &BidderName, max_length: isize) -> String {
        // Go: openrtb_ext/bid.go:177
        let s = format!("{prefix}{}_{bidder}", self.as_str());
        if max_length != 0 {
            return truncate(s, max_length);
        }
        s
    }

    /// Go: `TargetingKey.TruncateKey(prefix, maxLength)`: `prefix + key`, cut to `maxLength`
    /// bytes when it is positive (a cut inside a UTF-8 sequence as in
    /// [`TargetingKey::bidder_key`]).
    #[must_use]
    pub fn truncate_key(&self, prefix: &str, max_length: isize) -> String {
        // Go: openrtb_ext/bid.go:192
        let result = format!("{prefix}{}", self.as_str());
        if max_length > 0 {
            return truncate(result, max_length);
        }
        result
    }
}

/// Go's `s[:min(len(s), maxLength)]` for a non-negative `maxLength`.
fn truncate(mut s: String, max_length: isize) -> String {
    let Ok(max) = usize::try_from(max_length) else {
        return s;
    };
    if s.len() <= max {
        return s;
    }
    if s.is_char_boundary(max) {
        s.truncate(max);
        return s;
    }
    String::from_utf8_lossy(s.as_bytes().get(..max).unwrap_or_default()).into_owned()
}

/// Go constant `PbKey`.
#[doc(alias = "PbKey")]
pub const PB_KEY: TargetingKey = TargetingKey::from_static("_pb");
/// `EnvKey` exists to support the Prebid Universal Creative.
///
/// If it exists, the only legal values are "mobile-app" and "amp". It will exist only if the
/// incoming bidRequest defined request.app instead of request.site.
#[doc(alias = "EnvKey")]
pub const ENV_KEY: TargetingKey = TargetingKey::from_static("_env");
/// `CacheHostKey` and `CachePathKey` exist to supply cache host and path as targeting
/// parameters
#[doc(alias = "CacheHostKey")]
pub const CACHE_HOST_KEY: TargetingKey = TargetingKey::from_static("_cache_host");
/// Go constant `CachePathKey`.
#[doc(alias = "CachePathKey")]
pub const CACHE_PATH_KEY: TargetingKey = TargetingKey::from_static("_cache_path");
/// `BidderKey` is the name of the Bidder. For example, "appnexus" or "rubicon".
#[doc(alias = "BidderKey")]
pub const BIDDER_KEY: TargetingKey = TargetingKey::from_static("_bidder");
/// Go constant `SizeKey`.
#[doc(alias = "SizeKey")]
pub const SIZE_KEY: TargetingKey = TargetingKey::from_static("_size");
/// Go constant `DealKey`.
#[doc(alias = "DealKey")]
pub const DEAL_KEY: TargetingKey = TargetingKey::from_static("_deal");
/// `FormatKey` is the format of the bid. For example, "video", "banner"
#[doc(alias = "FormatKey")]
pub const FORMAT_KEY: TargetingKey = TargetingKey::from_static("_format");
/// `CacheKey` and `VastCacheKey` store UUIDs which can be used to fetch things from prebid
/// cache. Callers should *never* assume that either of these exist, since the call to the
/// cache may always fail.
///
/// `VastCacheKey`'s UUID will fetch the entire bid JSON, while `VastCacheKey` will fetch just
/// the VAST XML. `VastCacheKey` will only ever exist for Video bids.
#[doc(alias = "CacheKey")]
pub const CACHE_KEY: TargetingKey = TargetingKey::from_static("_cache_id");
/// Go constant `VastCacheKey`.
#[doc(alias = "VastCacheKey")]
pub const VAST_CACHE_KEY: TargetingKey = TargetingKey::from_static("_uuid");
/// Go constant `CategoryDurationKey`.
#[doc(alias = "CategoryDurationKey")]
pub const CATEGORY_DURATION_KEY: TargetingKey = TargetingKey::from_static("_pb_cat_dur");
