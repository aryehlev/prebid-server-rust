//! Go: `openrtb_ext/convert_down.go`: [`convert_down_to25`], which moves the fields ORTB 2.6
//! defines back to their ORTB 2.5 extension locations, and the `clear*_fields` functions, which
//! drop the fields of each ORTB 2.6 release.

use core::mem;

use pbs_errortypes::GoError;
use pbs_openrtb::{adcom1, openrtb2};

use crate::request_wrapper::{ImpWrapperMut, RequestWrapper};

#[cfg(test)]
#[path = "convert_down_test.rs"]
mod tests;

/// Go: `ConvertDownTo25(r)`: moves the fields ORTB 2.6 defines back to their ORTB 2.5 locations.
///
/// They are the supply chain (`source.schain`), the GDPR signal and consent (`regs.gdpr`,
/// `user.consent`), the US Privacy string (`regs.us_privacy`), the external identifiers
/// (`user.eids`) and the rewarded signal (`imp.rwdd`, moved to Prebid's
/// `imp.ext.prebid.is_rewarded_inventory`). What is in the 2.5 location is overwritten.
///
/// A nil request has nothing to move (Go panics).
///
/// # Errors
///
/// The decoding error of the ext a value moves into. The value has then already been removed
/// from its ORTB 2.6 location, and the values after it are not moved, as in Go.
pub fn convert_down_to25(r: &mut RequestWrapper) -> Result<(), GoError> {
    // Go: openrtb_ext/convert_down.go:5
    if r.bid_request().is_none() {
        return Ok(());
    }

    // schain
    move_supply_chain_from26_to25(r)?;

    // gdpr
    move_gdpr_from26_to25(r)?;
    move_consent_from26_to25(r)?;

    // ccpa
    move_us_privacy_from26_to25(r)?;

    // eid
    move_eid_from26_to25(r)?;

    // imp
    for imp in &mut r.get_imp() {
        move_rewarded_from26_to_prebid_ext(imp)?;
    }

    Ok(())
}

/// Go: `moveSupplyChainFrom26To25(r)`: moves `source.schain` (ORTB 2.6) to
/// `source.ext.schain` (ORTB 2.5).
fn move_supply_chain_from26_to25(r: &mut RequestWrapper) -> Result<(), GoError> {
    // Go: openrtb_ext/convert_down.go:43
    // read and clear 2.6 location
    let Some(schain26) = r
        .bid_request_mut()
        .and_then(|req| req.source.as_deref_mut())
        .and_then(|source| source.schain.take())
    else {
        return Ok(());
    };

    // move to 2.5 location
    let source_ext = r.get_source_ext()?;
    source_ext.set_s_chain(Some(*schain26));
    Ok(())
}

/// Go: `moveGDPRFrom26To25(r)`: moves `regs.gdpr` (ORTB 2.6) to `regs.ext.gdpr`
/// (ORTB 2.5).
fn move_gdpr_from26_to25(r: &mut RequestWrapper) -> Result<(), GoError> {
    // Go: openrtb_ext/convert_down.go:66
    // read and clear 2.6 location
    let Some(gdpr26) = r
        .bid_request_mut()
        .and_then(|req| req.regs.as_deref_mut())
        .and_then(|regs| regs.gdpr.take())
    else {
        return Ok(());
    };

    // move to 2.5 location
    let reg_ext = r.get_reg_ext()?;
    reg_ext.set_gdpr(Some(gdpr26));
    Ok(())
}

/// Go: `moveConsentFrom26To25(r)`: moves `user.consent` (ORTB 2.6) to `user.ext.consent`
/// (ORTB 2.5).
fn move_consent_from26_to25(r: &mut RequestWrapper) -> Result<(), GoError> {
    // Go: openrtb_ext/convert_down.go:89
    // read and clear 2.6 location
    let Some(consent26) = r
        .bid_request_mut()
        .and_then(|req| req.user.as_deref_mut())
        .filter(|user| !user.consent.is_empty())
        .map(|user| mem::take(&mut user.consent))
    else {
        return Ok(());
    };

    // move to 2.5 location
    let user_ext = r.get_user_ext()?;
    user_ext.set_consent(Some(consent26));
    Ok(())
}

/// Go: `moveUSPrivacyFrom26To25(r)`: moves `regs.us_privacy` (ORTB 2.6) to
/// `regs.ext.us_privacy` (ORTB 2.5).
fn move_us_privacy_from26_to25(r: &mut RequestWrapper) -> Result<(), GoError> {
    // Go: openrtb_ext/convert_down.go:112
    // read and clear 2.6 location
    let Some(us_privacy26) = r
        .bid_request_mut()
        .and_then(|req| req.regs.as_deref_mut())
        .filter(|regs| !regs.us_privacy.is_empty())
        .map(|regs| mem::take(&mut regs.us_privacy))
    else {
        return Ok(());
    };

    // move to 2.5 location
    let reg_ext = r.get_reg_ext()?;
    reg_ext.set_us_privacy(us_privacy26);
    Ok(())
}

/// Go: `moveEIDFrom26To25(r)`: moves `user.eids` (ORTB 2.6) to `user.ext.eids`
/// (ORTB 2.5).
fn move_eid_from26_to25(r: &mut RequestWrapper) -> Result<(), GoError> {
    // Go: openrtb_ext/convert_down.go:135
    // read and clear 2.6 location
    let Some(eid26) = r
        .bid_request_mut()
        .and_then(|req| req.user.as_deref_mut())
        .and_then(|user| user.eids.take())
    else {
        return Ok(());
    };

    // move to 2.5 location
    let user_ext = r.get_user_ext()?;
    user_ext.set_eid(Some(Some(eid26)));
    Ok(())
}

/// Go: `moveRewardedFrom26ToPrebidExt(i)`: moves `imp.rwdd` (ORTB 2.6) to Prebid's
/// `imp.ext.prebid.is_rewarded_inventory`.
fn move_rewarded_from26_to_prebid_ext(i: &mut ImpWrapperMut<'_>) -> Result<(), GoError> {
    // Go: openrtb_ext/convert_down.go:158
    // read and clear 2.6 location (Go panics on a nil impression, which has nothing to move)
    let Some(imp) = i.imp_mut().filter(|imp| imp.rwdd != 0) else {
        return Ok(());
    };
    let rwdd26 = imp.rwdd;
    imp.rwdd = 0;

    // move to Prebid specific location
    let imp_ext = i.get_imp_ext()?;
    let mut imp_ext_prebid = imp_ext.get_or_create_prebid().clone();
    imp_ext_prebid.is_rewarded_inventory = Some(rwdd26);
    imp_ext.set_prebid(Some(imp_ext_prebid));
    Ok(())
}

/// Go: `Clear26Fields(r)`: resets every field ORTB 2.6 introduced to its zero value, which
/// leaves it out of the encoded request. Nothing happens on a nil request (Go panics).
pub fn clear26_fields(r: &mut RequestWrapper) {
    // Go: openrtb_ext/convert_down.go:181
    let Some(req) = r.bid_request_mut() else {
        return;
    };
    req.wlangb = None;
    req.cattax = adcom1::CategoryTaxonomy::default();

    if let Some(app) = req.app.as_deref_mut() {
        app.cattax = adcom1::CategoryTaxonomy::default();
        app.kwarray = None;
        if let Some(content) = app.content.as_deref_mut() {
            clear26_content_fields(content);
        }
        if let Some(publisher) = app.publisher.as_deref_mut() {
            publisher.cattax = adcom1::CategoryTaxonomy::default();
        }
    }

    if let Some(site) = req.site.as_deref_mut() {
        site.cattax = adcom1::CategoryTaxonomy::default();
        site.kwarray = None;
        if let Some(content) = site.content.as_deref_mut() {
            clear26_content_fields(content);
        }
        if let Some(publisher) = site.publisher.as_deref_mut() {
            publisher.cattax = adcom1::CategoryTaxonomy::default();
        }
    }

    if let Some(device) = req.device.as_deref_mut() {
        device.sua = None;
        device.langb = String::new();
    }

    if let Some(regs) = req.regs.as_deref_mut() {
        regs.gdpr = None;
        regs.us_privacy = String::new();
    }

    if let Some(source) = req.source.as_deref_mut() {
        source.schain = None;
    }

    if let Some(user) = req.user.as_deref_mut() {
        user.kwarray = None;
        user.consent = String::new();
        user.eids = None;
    }

    for imp in &mut r.get_imp() {
        // Go panics on a wrapper without an impression; there is nothing to clear.
        let Some(imp) = imp.imp_mut() else {
            continue;
        };
        imp.rwdd = 0;
        imp.ssai = openrtb2::AdInsertion::default();

        if let Some(audio) = imp.audio.as_deref_mut() {
            audio.poddur = 0;
            audio.rqddurs = None;
            audio.podid = String::new();
            audio.podseq = adcom1::PodSequence::default();
            audio.slotinpod = adcom1::SlotPositionInPod::default();
            audio.mincpmpersec = 0.0;
        }

        if let Some(video) = imp.video.as_deref_mut() {
            video.maxseq = 0;
            video.poddur = 0;
            video.podid = String::new();
            video.podseq = adcom1::PodSequence::default();
            video.rqddurs = None;
            video.slotinpod = adcom1::SlotPositionInPod::default();
            video.mincpmpersec = 0.0;
        }
    }
}

/// The content fields [`clear26_fields`] resets, for `app.content` and `site.content` alike.
fn clear26_content_fields(content: &mut openrtb2::Content) {
    // Go: openrtb_ext/convert_down.go:189
    content.cattax = adcom1::CategoryTaxonomy::default();
    content.kwarray = None;
    content.langb = String::new();
    content.network = None;
    content.channel = None;

    if let Some(producer) = content.producer.as_deref_mut() {
        producer.cattax = adcom1::CategoryTaxonomy::default();
    }
}

/// Go: `Clear202211Fields(r)`: resets every field ORTB 2.6-202211 introduced to its zero
/// value. Nothing happens on a nil request (Go panics).
pub fn clear202211_fields(r: &mut RequestWrapper) {
    // Go: openrtb_ext/convert_down.go:274
    let Some(req) = r.bid_request_mut() else {
        return;
    };
    req.dooh = None;

    if let Some(app) = req.app.as_deref_mut() {
        app.inventorypartnerdomain = String::new();
    }

    if let Some(site) = req.site.as_deref_mut() {
        site.inventorypartnerdomain = String::new();
    }

    if let Some(regs) = req.regs.as_deref_mut() {
        regs.gpp = String::new();
        regs.gpp_sid = None;
    }

    for imp in &mut r.get_imp() {
        let Some(imp) = imp.imp_mut() else {
            continue;
        };
        imp.qty = None;
        imp.dt = 0.0;
    }
}

/// Go: `Clear202303Fields(r)`: resets every field ORTB 2.6-202303 introduced to its zero
/// value, in the impressions of the wrappers (see [`RequestWrapper::get_imp`]).
pub fn clear202303_fields(r: &mut RequestWrapper) {
    // Go: openrtb_ext/convert_down.go:298
    for imp in &mut r.get_imp() {
        let Some(imp) = imp.imp_mut() else {
            continue;
        };
        imp.refresh = None;

        if let Some(video) = imp.video.as_deref_mut() {
            video.plcmt = adcom1::VideoPlcmtSubtype::default();
        }
    }
}

/// Go: `Clear202309Fields(r)`: resets every field ORTB 2.6-202309 introduced to its zero
/// value. Nothing happens on a nil request (Go panics).
pub fn clear202309_fields(r: &mut RequestWrapper) {
    // Go: openrtb_ext/convert_down.go:310
    let Some(req) = r.bid_request_mut() else {
        return;
    };
    req.acat = None;

    for imp in &mut r.get_imp() {
        let Some(imp) = imp.imp_mut() else {
            continue;
        };
        if let Some(audio) = imp.audio.as_deref_mut() {
            audio.durfloors = None;
        }

        if let Some(video) = imp.video.as_deref_mut() {
            video.durfloors = None;
        }

        if let Some(pmp) = imp.pmp.as_deref_mut() {
            for deal in pmp.deals.iter_mut().flatten() {
                deal.guar = 0;
                deal.mincpmpersec = 0.0;
                deal.durfloors = None;
            }
        }
    }
}

/// Go: `Clear202402Fields(r)`: resets every field ORTB 2.6-202402 introduced to its zero
/// value, in the impressions of the wrappers (see [`RequestWrapper::get_imp`]).
pub fn clear202402_fields(r: &mut RequestWrapper) {
    // Go: openrtb_ext/convert_down.go:334
    for imp in &mut r.get_imp() {
        if let Some(video) = imp.imp_mut().and_then(|imp| imp.video.as_deref_mut()) {
            video.poddedupe = None;
        }
    }
}

/// Go: `Clear202409Fields(r)`: resets every field ORTB 2.6-202409 introduced to its zero
/// value. Nothing happens on a nil request (Go panics).
pub fn clear202409_fields(r: &mut RequestWrapper) {
    // Go: openrtb_ext/convert_down.go:344
    let Some(user) = r.bid_request_mut().and_then(|req| req.user.as_deref_mut()) else {
        return;
    };
    for eid in user.eids.iter_mut().flatten() {
        eid.inserter = String::new();
        eid.matcher = String::new();
        eid.mm = adcom1::MATCH_METHOD_UNKNOWN;
    }
}
