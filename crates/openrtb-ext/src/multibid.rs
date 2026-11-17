//! Go: `openrtb_ext/multibid.go`: `request.ext.prebid.multibid` validation.

use std::collections::HashSet;

use pbs_errortypes::GoError;

use crate::generated::{DEFAULT_BID_LIMIT, ExtMultiBid, ExtRequestPrebid, MAX_BID_LIMIT};

/// Go: `ValidateAndBuildExtMultiBid(prebid)`: the validated `multibid` entries and the
/// validation errors, in request order.
///
/// As in Go, the request's entries are corrected in place (`maxbids` clamped to 1..9, `bidders`
/// dropped from an entry with a `bidder`, `targetbiddercodeprefix` dropped from an entry with
/// `bidders`); the result holds copies of the entries Go returns pointers to. `None` is Go's nil
/// result (no `prebid`, no `multibid`, or nothing valid). Go panics on a nil element of
/// `multibid`; here it is validated as an empty entry, without changing the request.
pub fn validate_and_build_ext_multi_bid(
    prebid: Option<&mut ExtRequestPrebid>,
) -> (Option<Vec<ExtMultiBid>>, Vec<GoError>) {
    // Go: openrtb_ext/multibid.go:8
    let Some(multi_bids) = prebid.and_then(|prebid| prebid.multi_bid.as_mut()) else {
        return (None, Vec::new());
    };
    let mut validation_errs = Vec::new();
    let mut validated_multi_bids: Option<Vec<ExtMultiBid>> = None;
    let mut multi_bid_map = HashSet::new();
    for multi_bid in multi_bids.iter_mut() {
        let mut empty = ExtMultiBid::default();
        let multi_bid = multi_bid.as_deref_mut().unwrap_or(&mut empty);
        let (new_multi_bids, errs) = add_multi_bid(&mut multi_bid_map, multi_bid);
        validation_errs.extend(errs);
        if !new_multi_bids.is_empty() {
            validated_multi_bids
                .get_or_insert_with(Vec::new)
                .extend(new_multi_bids);
        }
    }
    (validated_multi_bids, validation_errs)
}

/// Go: `addMultiBid(multiBidMap, multiBid)`: validates and adds one entry. Every message prints
/// the entry as it was when the problem was found (`%v` calls [`ExtMultiBid::string`]).
fn add_multi_bid(
    multi_bid_map: &mut HashSet<String>,
    multi_bid: &mut ExtMultiBid,
) -> (Vec<ExtMultiBid>, Vec<GoError>) {
    // Go: openrtb_ext/multibid.go:30
    let mut errs = Vec::new();
    let Some(max_bids) = multi_bid.max_bids else {
        errs.push(GoError::generic(format!(
            "maxBids not defined for {}",
            multi_bid.string()
        )));
        return (Vec::new(), errs);
    };
    let mut max_bids = max_bids;
    if max_bids < DEFAULT_BID_LIMIT {
        errs.push(GoError::generic(format!(
            "invalid maxBids value, using minimum {DEFAULT_BID_LIMIT} limit for {}",
            multi_bid.string()
        )));
        max_bids = DEFAULT_BID_LIMIT;
        multi_bid.max_bids = Some(max_bids);
    }
    if max_bids > MAX_BID_LIMIT {
        errs.push(GoError::generic(format!(
            "invalid maxBids value, using maximum {MAX_BID_LIMIT} limit for {}",
            multi_bid.string()
        )));
        multi_bid.max_bids = Some(MAX_BID_LIMIT);
    }

    let mut validated_multi_bids = Vec::new();
    if !multi_bid.bidder.is_empty() {
        if multi_bid_map.contains(&multi_bid.bidder) {
            errs.push(GoError::generic(format!(
                "multiBid already defined for {}, ignoring this instance {}",
                multi_bid.bidder,
                multi_bid.string()
            )));
            return (Vec::new(), errs);
        }
        if multi_bid.bidders.is_some() {
            errs.push(GoError::generic(format!(
                "ignoring bidders from {}",
                multi_bid.string()
            )));
            multi_bid.bidders = None;
        }
        let _ = multi_bid_map.insert(multi_bid.bidder.clone());
        validated_multi_bids.push(multi_bid.clone());
    } else if multi_bid.bidders.as_ref().is_some_and(|b| !b.is_empty()) {
        let mut bidders: Option<Vec<String>> = None;
        for bidder in multi_bid.bidders.iter().flatten() {
            if multi_bid_map.contains(bidder) {
                errs.push(GoError::generic(format!(
                    "multiBid already defined for {bidder}, ignoring this instance {}",
                    multi_bid.string()
                )));
                continue;
            }
            let _ = multi_bid_map.insert(bidder.clone());
            bidders.get_or_insert_with(Vec::new).push(bidder.clone());
        }
        if !multi_bid.target_bidder_code_prefix.is_empty() {
            errs.push(GoError::generic(format!(
                "ignoring targetbiddercodeprefix for {}",
                multi_bid.string()
            )));
            multi_bid.target_bidder_code_prefix.clear();
        }
        if bidders.is_some() {
            validated_multi_bids.push(ExtMultiBid {
                max_bids: multi_bid.max_bids,
                bidders,
                ..ExtMultiBid::default()
            });
        }
    } else {
        errs.push(GoError::generic(format!(
            "bidder(s) not specified for {}",
            multi_bid.string()
        )));
    }
    (validated_multi_bids, errs)
}
