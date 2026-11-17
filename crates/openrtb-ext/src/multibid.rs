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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::add_multi_bid;
    use crate::generated::ExtMultiBid;

    /// An entry (Go: `ExtMultiBid{...}`).
    fn mb(
        bidder: &str,
        bidders: Option<&[&str]>,
        max_bids: Option<isize>,
        prefix: &str,
    ) -> ExtMultiBid {
        ExtMultiBid {
            bidder: bidder.to_owned(),
            bidders: bidders.map(|names| names.iter().map(|&n| n.to_owned()).collect()),
            max_bids,
            target_bidder_code_prefix: prefix.to_owned(),
        }
    }

    /// Go's `fmt.Errorf("...%v", ExtMultiBid{...})` text: `%v` calls `ExtMultiBid.String`.
    fn err(prefix: &str, entry: &ExtMultiBid) -> String {
        format!("{prefix}{}", entry.string())
    }

    /// A case of `Test_addMultiBid`: its name, the bidders already in `multiBidMap`, the entry,
    /// and the errors and entries wanted.
    type AddCase = (
        &'static str,
        &'static [&'static str],
        ExtMultiBid,
        Vec<String>,
        Vec<ExtMultiBid>,
    );

    /// The cases of `Test_addMultiBid` up to `Only Bidder defined`.
    fn bidder_cases() -> [AddCase; 5] {
        let pubmatic = mb("pubmatic", None, Some(2), "");
        let both = mb("appnexus", Some(&["rubicon"]), Some(2), "appN");
        let appnexus = mb("appnexus", None, Some(2), "appN");
        [
            (
                "MaxBids not defined",
                &[],
                ExtMultiBid::default(),
                vec![err("maxBids not defined for ", &ExtMultiBid::default())],
                vec![],
            ),
            (
                "Bidder or Bidders not defined",
                &[],
                mb("", None, Some(2), ""),
                vec![err(
                    "bidder(s) not specified for ",
                    &mb("", None, Some(2), ""),
                )],
                vec![],
            ),
            (
                "Input bidder is already present in multibid",
                &["pubmatic"],
                pubmatic.clone(),
                vec![err(
                    "multiBid already defined for pubmatic, ignoring this instance ",
                    &pubmatic,
                )],
                vec![],
            ),
            (
                "Bidder and Bidders both defined (only Bidder will be used)",
                &["pubmatic"],
                both.clone(),
                vec![err("ignoring bidders from ", &both)],
                vec![appnexus.clone()],
            ),
            (
                "Only Bidder defined",
                &[],
                appnexus.clone(),
                vec![],
                vec![appnexus],
            ),
        ]
    }

    /// The cases of `Test_addMultiBid` from `Only Bidders defined` on.
    fn bidders_cases() -> [AddCase; 5] {
        let some = mb("", Some(&["appnexus", "someBidder"]), Some(2), "");
        let both = mb("", Some(&["appnexus", "pubmatic"]), Some(2), "");
        let prefixed = mb("", Some(&["appnexus"]), Some(2), "appN");
        let below = mb("appnexus", None, Some(0), "appN");
        let over = mb("appnexus", None, Some(10), "appN");
        [
            (
                "Only Bidders defined",
                &[],
                some.clone(),
                vec![],
                vec![some],
            ),
            (
                "Bidders defined where a bidder is already present in multibid",
                &["pubmatic"],
                both.clone(),
                vec![err(
                    "multiBid already defined for pubmatic, ignoring this instance ",
                    &both,
                )],
                vec![mb("", Some(&["appnexus"]), Some(2), "")],
            ),
            (
                "Bidders defined along with TargetBidderCodePrefix",
                &[],
                prefixed.clone(),
                vec![err("ignoring targetbiddercodeprefix for ", &prefixed)],
                vec![mb("", Some(&["appnexus"]), Some(2), "")],
            ),
            (
                "MaxBids defined below minimum limit",
                &[],
                below.clone(),
                vec![err(
                    "invalid maxBids value, using minimum 1 limit for ",
                    &below,
                )],
                vec![mb("appnexus", None, Some(1), "appN")],
            ),
            (
                "MaxBids defined over max limit",
                &[],
                over.clone(),
                vec![err(
                    "invalid maxBids value, using maximum 9 limit for ",
                    &over,
                )],
                vec![mb("appnexus", None, Some(9), "appN")],
            ),
        ]
    }

    /// Go: `Test_addMultiBid`. Go's nil and empty result slices are both empty here.
    #[test]
    fn add_multi_bid_cases() -> Result<(), String> {
        let cases = bidder_cases().into_iter().chain(bidders_cases());
        for (name, present, mut multi_bid, want_errs, want_multi_bids) in cases {
            let mut multi_bid_map: HashSet<String> =
                present.iter().map(|&b| b.to_owned()).collect();
            let (multi_bids, errs) = add_multi_bid(&mut multi_bid_map, &mut multi_bid);
            let errs: Vec<String> = errs.iter().map(ToString::to_string).collect();
            if errs != want_errs || multi_bids != want_multi_bids {
                return Err(format!(
                    "{name}: got {multi_bids:?} and {errs:?}, want {want_multi_bids:?} and \
                     {want_errs:?}"
                ));
            }
        }
        Ok(())
    }
}
