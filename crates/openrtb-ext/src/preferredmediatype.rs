//! Go: `openrtb_ext/preferredmediatype.go`.

use std::collections::BTreeMap;

use crate::{bid::BidType, bidders::BidderName, named_map::named_map};

/// Go type `openrtb_ext.PreferredMediaType`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreferredMediaType(pub Option<BTreeMap<BidderName, BidType>>);

named_map!(
    PreferredMediaType,
    "openrtb_ext.PreferredMediaType",
    BidderName,
    BidType
);
