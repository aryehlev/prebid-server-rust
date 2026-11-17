//! The `Ptr` and `Val` methods of Go `adcom1.ConnectionType`, `DOOHVenueTaxonomy`,
//! `PlacementPosition`, `StartDelay` and `VolumeNormalizationMode`.

use super::{
    CONNECTION_UNKNOWN, ConnectionType, DOOHVenueTaxonomy, POSITION_UNKNOWN, PlacementPosition,
    START_PRE_ROLL, StartDelay, VENUE_TAXONOMY_OPEN_OOH10, VOLUME_NORM_NONE,
    VolumeNormalizationMode,
};

impl ConnectionType {
    /// `Ptr` returns pointer to own value.
    #[must_use]
    pub const fn ptr(self) -> Option<Self> {
        Some(self)
    }

    /// `Val` safely dereferences pointer, returning default value (`ConnectionUnknown`) for nil.
    #[must_use]
    pub const fn val(c: Option<Self>) -> Self {
        match c {
            Some(v) => v,
            None => CONNECTION_UNKNOWN,
        }
    }
}

impl DOOHVenueTaxonomy {
    /// `Ptr` returns pointer to own value.
    #[must_use]
    pub const fn ptr(self) -> Option<Self> {
        Some(self)
    }

    /// `Val` safely dereferences pointer, returning default value for nil: Go returns
    /// `VenueTaxonomyOpenOOH10` (its doc comment says `ConnectionUnknown`).
    #[must_use]
    pub const fn val(t: Option<Self>) -> Self {
        match t {
            Some(v) => v,
            None => VENUE_TAXONOMY_OPEN_OOH10,
        }
    }
}

impl PlacementPosition {
    /// `Ptr` returns pointer to own value.
    #[must_use]
    pub const fn ptr(self) -> Option<Self> {
        Some(self)
    }

    /// `Val` safely dereferences pointer, returning default value (`PositionUnknown`) for nil.
    #[must_use]
    pub const fn val(p: Option<Self>) -> Self {
        match p {
            Some(v) => v,
            None => POSITION_UNKNOWN,
        }
    }
}

impl StartDelay {
    /// `Ptr` returns pointer to own value.
    #[must_use]
    pub const fn ptr(self) -> Option<Self> {
        Some(self)
    }

    /// `Val` safely dereferences pointer, returning default value for nil: Go returns
    /// `StartPreRoll` (its doc comment says `StartDelayPreRoll`).
    #[must_use]
    pub const fn val(d: Option<Self>) -> Self {
        match d {
            Some(v) => v,
            None => START_PRE_ROLL,
        }
    }
}

impl VolumeNormalizationMode {
    /// `Ptr` returns pointer to own value.
    #[must_use]
    pub const fn ptr(self) -> Option<Self> {
        Some(self)
    }

    /// `Val` safely dereferences pointer, returning default value (`VolumeNormNone`) for nil.
    #[must_use]
    pub const fn val(v: Option<Self>) -> Self {
        match v {
            Some(v) => v,
            None => VOLUME_NORM_NONE,
        }
    }
}
