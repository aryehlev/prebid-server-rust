//! Go package `native1`: `OpenRTB` Native 1.2 enums (section "7 Reference Lists/Enumerations"),
//! with the request and response types in [`request`] and [`response`].
//!
//! The specification is linked from <https://iabtechlab.com/standards/openrtb-native/>.

mod generated;
pub mod request;
pub mod response;

pub use generated::*;
