//! Go: `openrtb_ext/supplyChain.go`: copying a supply chain.

use pbs_openrtb::openrtb2;

/// Go: `cloneSupplyChain(schain)`: a copy whose `nodes` is never nil.
///
/// The copy's `nodes` is an empty slice (`[]`) when the original's was nil. Go's nil chain is the
/// caller's `None`. Go does not export it: the request wrapper's `RequestExt` and `SourceExt`
/// clones use it.
#[must_use]
pub fn clone_supply_chain(schain: &openrtb2::SupplyChain) -> openrtb2::SupplyChain {
    // Go: openrtb_ext/supplyChain.go:8
    openrtb2::SupplyChain {
        nodes: Some(schain.nodes.clone().unwrap_or_default()),
        ..schain.clone()
    }
}
