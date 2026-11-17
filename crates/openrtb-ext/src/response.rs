//! Go: `openrtb_ext/response.go`: the named string types of the response extension (the
//! structs are generated).

use std::borrow::Cow;

use crate::{cow_str::cow_string_type, generated::ExtHttpCall};

/// The debug HTTP calls to one bidder: a value of Go's `ExtResponseDebug.HttpCalls`
/// (`map[BidderName][]*ExtHttpCall`).
pub type ExtHttpCalls = Option<Vec<Option<Box<ExtHttpCall>>>>;

cow_string_type! {
    /// `CookieStatus` describes the allowed values for bidresponse.ext.usersync.{bidder}.status
    pub struct CookieStatus = "openrtb_ext.CookieStatus";
}

/// Go constant `CookieNone`.
#[doc(alias = "CookieNone")]
pub const COOKIE_NONE: CookieStatus = CookieStatus::from_static("none");
/// Go constant `CookieExpired`.
#[doc(alias = "CookieExpired")]
pub const COOKIE_EXPIRED: CookieStatus = CookieStatus::from_static("expired");
/// Go constant `CookieAvailable`.
#[doc(alias = "CookieAvailable")]
pub const COOKIE_AVAILABLE: CookieStatus = CookieStatus::from_static("available");

cow_string_type! {
    /// `UserSyncType` describes the allowed values for
    /// `bidresponse.ext.usersync.{bidder}.syncs[i].type`
    pub struct UserSyncType = "openrtb_ext.UserSyncType";
}

/// Go constant `UserSyncIframe`.
#[doc(alias = "UserSyncIframe")]
pub const USER_SYNC_IFRAME: UserSyncType = UserSyncType::from_static("iframe");
/// Go constant `UserSyncPixel`.
#[doc(alias = "UserSyncPixel")]
pub const USER_SYNC_PIXEL: UserSyncType = UserSyncType::from_static("pixel");
