//! Go: `openrtb_ext/imp.go`: the impression IDs of a request (the types are generated).

use pbs_openrtb::openrtb2;

/// Go: `GetImpIDs(imps)` returns the ID of every impression, in order.
#[must_use]
pub fn get_imp_ids(imps: &[openrtb2::Imp]) -> Vec<String> {
    imps.iter().map(|imp| imp.id.clone()).collect()
}
