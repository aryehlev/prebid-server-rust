//! Port of go-gdpr `consentconstants/tcf2`: the TCF 2.0 purposes and special features.

use super::{Purpose, SpecialFeature};

/// Special feature 1: use precise geolocation data.
pub const GEOLOCATION: SpecialFeature = SpecialFeature(1);

/// Special feature 2: actively scan device characteristics for identification.
pub const DEVICE_SCAN: SpecialFeature = SpecialFeature(2);

/// Purpose 1: store and/or access information on a device.
pub const INFO_STORAGE_ACCESS: Purpose = Purpose(1);

/// Purpose 2: select basic ads.
pub const BASIC_ADSERVING: Purpose = Purpose(2);

/// Purpose 3: create a personalised ads profile.
pub const PERSONALIZATION_PROFILE: Purpose = Purpose(3);

/// Purpose 4: select personalised ads.
pub const PERSONALIZATION_SELECTION: Purpose = Purpose(4);

/// Purpose 5: create a personalised content profile.
pub const CONTENT_PROFILE: Purpose = Purpose(5);

/// Purpose 6: select personalised content.
pub const CONTENT_SELECTION: Purpose = Purpose(6);

/// Purpose 7: measure ad performance.
pub const AD_PERFORMANCE: Purpose = Purpose(7);

/// Purpose 8: measure content performance.
pub const CONTENT_PERFORMANCE: Purpose = Purpose(8);

/// Purpose 9: apply market research to generate audience insights.
pub const MARKET_RESEARCH: Purpose = Purpose(9);

/// Purpose 10: develop and improve products.
pub const DEVELOP_IMPROVE: Purpose = Purpose(10);
