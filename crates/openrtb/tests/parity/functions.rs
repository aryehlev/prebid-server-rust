//! The hand-written ports of the Go functions and methods (`openrtb2/ptr.go`,
//! `native1/response/ptr.go`, and the `Ptr`/`Val` methods of `openrtb3` and `adcom1`).
//!
//! In Go, `Ptr` returns a pointer to a copy of the value and `Val` the value a pointer points
//! to, or a default for a nil pointer.

use pbs_openrtb::{adcom1, native1, openrtb2, openrtb3};

use super::support::{TestResult, ensure_eq};

#[test]
fn int_ptr() -> TestResult {
    ensure_eq(&openrtb2::int8_ptr(-3), &Some(-3), "openrtb2.Int8Ptr")?;
    ensure_eq(
        &openrtb2::int8_ptr(i8::MAX),
        &Some(i8::MAX),
        "openrtb2.Int8Ptr max",
    )?;
    ensure_eq(&openrtb2::int64_ptr(0), &Some(0), "openrtb2.Int64Ptr")?;
    ensure_eq(
        &openrtb2::int64_ptr(i64::MIN),
        &Some(i64::MIN),
        "openrtb2.Int64Ptr min",
    )?;
    ensure_eq(
        &native1::response::int64_ptr(i64::MAX),
        &Some(i64::MAX),
        "response.Int64Ptr",
    )
}

#[test]
fn no_bid_reason() -> TestResult {
    let reason = openrtb3::NoBidReason(5);
    ensure_eq(&reason.ptr(), &Some(reason), "NoBidReason.Ptr")?;
    ensure_eq(
        &openrtb3::NoBidReason::val(Some(reason)),
        &reason,
        "NoBidReason.Val",
    )?;
    // Go: openrtb3/no_bid_reason.go Val (nil: NoBidUnknownError)
    ensure_eq(
        &openrtb3::NoBidReason::val(None),
        &openrtb3::NO_BID_UNKNOWN_ERROR,
        "NoBidReason.Val of nil",
    )
}

#[test]
fn adcom1_ptr_val() -> TestResult {
    let connection = adcom1::CONNECTION_WIFI;
    ensure_eq(&connection.ptr(), &Some(connection), "ConnectionType.Ptr")?;
    ensure_eq(
        &adcom1::ConnectionType::val(Some(connection)),
        &connection,
        "ConnectionType.Val",
    )?;
    // Go: adcom1/connection_type.go Val (nil: ConnectionUnknown)
    ensure_eq(
        &adcom1::ConnectionType::val(None),
        &adcom1::CONNECTION_UNKNOWN,
        "ConnectionType.Val of nil",
    )?;

    let taxonomy = adcom1::VENUE_TAXONOMY_DPAA;
    ensure_eq(&taxonomy.ptr(), &Some(taxonomy), "DOOHVenueTaxonomy.Ptr")?;
    ensure_eq(
        &adcom1::DOOHVenueTaxonomy::val(Some(taxonomy)),
        &taxonomy,
        "DOOHVenueTaxonomy.Val",
    )?;
    // Go: adcom1/dooh_venue_taxonomies.go Val (nil: VenueTaxonomyOpenOOH10)
    ensure_eq(
        &adcom1::DOOHVenueTaxonomy::val(None),
        &adcom1::VENUE_TAXONOMY_OPEN_OOH10,
        "DOOHVenueTaxonomy.Val of nil",
    )?;

    let position = adcom1::POSITION_ABOVE_FOLD;
    ensure_eq(&position.ptr(), &Some(position), "PlacementPosition.Ptr")?;
    ensure_eq(
        &adcom1::PlacementPosition::val(Some(position)),
        &position,
        "PlacementPosition.Val",
    )?;
    // Go: adcom1/placement_position.go Val (nil: PositionUnknown)
    ensure_eq(
        &adcom1::PlacementPosition::val(None),
        &adcom1::POSITION_UNKNOWN,
        "PlacementPosition.Val of nil",
    )?;

    let delay = adcom1::START_MID_ROLL;
    ensure_eq(&delay.ptr(), &Some(delay), "StartDelay.Ptr")?;
    ensure_eq(
        &adcom1::StartDelay::val(Some(delay)),
        &delay,
        "StartDelay.Val",
    )?;
    // Go: adcom1/start_delay.go Val (nil: StartPreRoll)
    ensure_eq(
        &adcom1::StartDelay::val(None),
        &adcom1::START_PRE_ROLL,
        "StartDelay.Val of nil",
    )?;

    let volume = adcom1::VOLUME_NORM_PEAK;
    ensure_eq(&volume.ptr(), &Some(volume), "VolumeNormalizationMode.Ptr")?;
    ensure_eq(
        &adcom1::VolumeNormalizationMode::val(Some(volume)),
        &volume,
        "VolumeNormalizationMode.Val",
    )?;
    // Go: adcom1/volume_normalization_mode.go Val (nil: VolumeNormNone)
    ensure_eq(
        &adcom1::VolumeNormalizationMode::val(None),
        &adcom1::VOLUME_NORM_NONE,
        "VolumeNormalizationMode.Val of nil",
    )
}
