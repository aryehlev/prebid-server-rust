//! Go: `openrtb_ext/convert_up.go`: [`convert_up_to26`], which moves the fields ORTB 2.6
//! defines from their ORTB 2.4 and 2.5 extension locations to their 2.6 locations.

use pbs_errortypes::GoError;
use pbs_openrtb::openrtb2;

use crate::request_wrapper::{ImpWrapperMut, RequestWrapper};

#[cfg(test)]
#[path = "convert_up_test.rs"]
mod tests;

/// Go: `ConvertUpTo26(r)`: moves the fields ORTB 2.6 defines to their 2.6 locations.
///
/// They are the supply chain (`ext.schain` and `source.ext.schain`), the GDPR signal and consent
/// (`regs.ext.gdpr`, `user.ext.consent`), the US Privacy string (`regs.ext.us_privacy`), the
/// external identifiers (`user.ext.eids`) and Prebid's rewarded signal
/// (`imp.ext.prebid.is_rewarded_inventory`). A value that is already in its 2.6 location is kept,
/// and the older one is dropped.
///
/// Where Go panics (a value to move into a nil request, `regs` or `user`), the value is dropped
/// from its old location and not moved; a nil request without impression wrappers (where Go's
/// `GetImp` panics) has no impressions.
///
/// # Errors
///
/// `req.ext is invalid: ...`, `req.source.ext is invalid: ...`, `req.regs.ext is invalid: ...`,
/// `req.user.ext is invalid: ...` or `imp[i].imp.ext is invalid: ...` when that ext cannot be
/// decoded; nothing is moved then.
pub fn convert_up_to26(r: &mut RequestWrapper) -> Result<(), GoError> {
    // Go: openrtb_ext/convert_up.go:9
    convert_up_ensure_ext(r)?;

    // schain
    move_supply_chain_from24_to25(r);
    move_supply_chain_from25_to26(r);

    // gdpr
    move_gdpr_from25_to26(r);
    move_consent_from25_to26(r);

    // ccpa
    move_us_privacy_from25_to26(r);

    // eid
    move_eid_from25_to26(r);

    // imp
    for imp in &mut r.get_imp() {
        move_rewarded_from_prebid_ext_to26(imp);
    }

    Ok(())
}

/// Go: `convertUpEnsureExt(r)`: decodes every ext the conversion reads, so that a decoding error
/// is returned before anything is moved.
fn convert_up_ensure_ext(r: &mut RequestWrapper) -> Result<(), GoError> {
    // Go: openrtb_ext/convert_up.go:38
    if let Err(err) = r.get_request_ext() {
        return Err(GoError::generic(format!(
            "req.ext is invalid: {}",
            err.message()
        )));
    }
    if let Err(err) = r.get_source_ext() {
        return Err(GoError::generic(format!(
            "req.source.ext is invalid: {}",
            err.message()
        )));
    }
    if let Err(err) = r.get_reg_ext() {
        return Err(GoError::generic(format!(
            "req.regs.ext is invalid: {}",
            err.message()
        )));
    }
    if let Err(err) = r.get_user_ext() {
        return Err(GoError::generic(format!(
            "req.user.ext is invalid: {}",
            err.message()
        )));
    }
    for (i, imp) in r.get_imp().iter_mut().enumerate() {
        if let Err(err) = imp.get_imp_ext() {
            return Err(GoError::generic(format!(
                "imp[{i}].imp.ext is invalid: {}",
                err.message()
            )));
        }
    }
    Ok(())
}

/// Go: `moveSupplyChainFrom24To25(r)`: moves `ext.schain` (ORTB 2.4) to `source.ext.schain`
/// (ORTB 2.5) unless that is present, in which case the 2.4 supply chain is dropped.
fn move_supply_chain_from24_to25(r: &mut RequestWrapper) {
    // Go: openrtb_ext/convert_up.go:67
    // read and clear 2.4 location
    let (req_ext, _) = r.get_request_ext_pair();
    let schain24 = req_ext.get_s_chain().cloned();
    req_ext.set_s_chain(None);

    // move to 2.5 location if not already present
    let (source_ext, _) = r.get_source_ext_pair();
    if source_ext.get_s_chain().is_none() {
        source_ext.set_s_chain(schain24);
    }
}

/// Go: `moveSupplyChainFrom25To26(r)`: moves `source.ext.schain` (ORTB 2.5) to
/// `source.schain` (ORTB 2.6) unless that is present, in which case the 2.5 supply chain is
/// dropped. The `source` is created when absent.
fn move_supply_chain_from25_to26(r: &mut RequestWrapper) {
    // Go: openrtb_ext/convert_up.go:83
    // read and clear 2.5 location
    let (source_ext, _) = r.get_source_ext_pair();
    let schain25 = source_ext.get_s_chain().cloned();
    source_ext.set_s_chain(None);

    // move to 2.6 location if not already present
    let Some(schain25) = schain25 else {
        return;
    };
    // Go panics on a nil request; the supply chain is dropped.
    let Some(req) = r.bid_request_mut() else {
        return;
    };
    // source may be nil if moved indirectly from an OpenRTB 2.4 location, since the ext is not
    // defined on the source object.
    let source = req
        .source
        .get_or_insert_with(|| Box::new(openrtb2::Source::default()));
    if source.schain.is_none() {
        source.schain = Some(Box::new(schain25));
    }
}

/// Go: `moveGDPRFrom25To26(r)`: moves `regs.ext.gdpr` (ORTB 2.5) to `regs.gdpr` (ORTB 2.6)
/// unless that is present, in which case the 2.5 signal is dropped.
fn move_gdpr_from25_to26(r: &mut RequestWrapper) {
    // Go: openrtb_ext/convert_up.go:106
    // read and clear 2.5 location
    let (regs_ext, _) = r.get_reg_ext_pair();
    let gdpr25 = regs_ext.get_gdpr();
    regs_ext.set_gdpr(None);

    // move to 2.6 location (Go panics on a nil request or regs; the signal is dropped)
    if let Some(gdpr25) = gdpr25
        && let Some(regs) = r.bid_request_mut().and_then(|req| req.regs.as_deref_mut())
        && regs.gdpr.is_none()
    {
        regs.gdpr = Some(gdpr25);
    }
}

/// Go: `moveConsentFrom25To26(r)`: moves `user.ext.consent` (ORTB 2.5) to `user.consent`
/// (ORTB 2.6) unless that is set, in which case the 2.5 consent is dropped.
fn move_consent_from25_to26(r: &mut RequestWrapper) {
    // Go: openrtb_ext/convert_up.go:121
    // read and clear 2.5 location
    let (user_ext, _) = r.get_user_ext_pair();
    let consent25 = user_ext.get_consent().map(str::to_owned);
    user_ext.set_consent(None);

    // move to 2.6 location (Go panics on a nil request or user; the consent is dropped)
    if let Some(consent25) = consent25
        && let Some(user) = r.bid_request_mut().and_then(|req| req.user.as_deref_mut())
        && user.consent.is_empty()
    {
        user.consent = consent25;
    }
}

/// Go: `moveUSPrivacyFrom25To26(r)`: moves `regs.ext.us_privacy` (ORTB 2.5) to
/// `regs.us_privacy` (ORTB 2.6) unless that is set, in which case the 2.5 string is dropped.
fn move_us_privacy_from25_to26(r: &mut RequestWrapper) {
    // Go: openrtb_ext/convert_up.go:136
    // read and clear 2.5 location
    let (regs_ext, _) = r.get_reg_ext_pair();
    let us_privacy25 = regs_ext.get_us_privacy().to_owned();
    regs_ext.set_us_privacy(String::new());

    // move to 2.6 location (Go panics on a nil request or regs; the string is dropped)
    if !us_privacy25.is_empty()
        && let Some(regs) = r.bid_request_mut().and_then(|req| req.regs.as_deref_mut())
        && regs.us_privacy.is_empty()
    {
        regs.us_privacy = us_privacy25;
    }
}

/// Go: `moveEIDFrom25To26(r)`: moves `user.ext.eids` (ORTB 2.5) to `user.eids` (ORTB 2.6)
/// unless that is present, in which case the 2.5 identifiers are dropped.
fn move_eid_from25_to26(r: &mut RequestWrapper) {
    // Go: openrtb_ext/convert_up.go:151
    // read and clear 2.5 location
    let (user_ext, _) = r.get_user_ext_pair();
    let eid25 = user_ext
        .get_eid()
        .map(|eids| eids.map(<[openrtb2::EID]>::to_vec));
    user_ext.set_eid(None);

    // move to 2.6 location (Go panics on a nil request or user; the identifiers are dropped)
    if let Some(eid25) = eid25
        && let Some(user) = r.bid_request_mut().and_then(|req| req.user.as_deref_mut())
        && user.eids.is_none()
    {
        user.eids = eid25;
    }
}

/// Go: `moveRewardedFromPrebidExtTo26(i)`: moves `imp.ext.prebid.is_rewarded_inventory` (Prebid's
/// signal) to `imp.rwdd` (ORTB 2.6) unless that is set, in which case Prebid's signal is
/// dropped.
fn move_rewarded_from_prebid_ext_to26(i: &mut ImpWrapperMut<'_>) {
    // Go: openrtb_ext/convert_up.go:167
    // read and clear prebid ext
    let (imp_ext, _) = i.get_imp_ext_pair();
    let mut rwdd_prebid_ext = None;
    if let Some(p) = imp_ext.get_prebid()
        && let Some(rewarded) = p.is_rewarded_inventory
    {
        rwdd_prebid_ext = Some(rewarded);
        let mut p = p.clone();
        p.is_rewarded_inventory = None;
        imp_ext.set_prebid(Some(p));
    }

    // move to 2.6 location (Go panics on a nil impression; the signal is dropped)
    if let Some(rewarded) = rwdd_prebid_ext
        && let Some(imp) = i.imp_mut()
        && imp.rwdd == 0
    {
        imp.rwdd = rewarded;
    }
}
