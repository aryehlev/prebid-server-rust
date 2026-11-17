//! Go: `openrtb_ext/floors.go`: the price floor getters and copies (the structs are generated).
//!
//! Go's getters accept a nil receiver; here they take `Option<&PriceFloorRules>` and return
//! Go's nil-receiver defaults for `None`.

use crate::generated::{
    PriceFloorData, PriceFloorEnforcement, PriceFloorModelGroup, PriceFloorRules, PriceFloorSchema,
};

impl PriceFloorRules {
    /// Go: `GetEnforcePBS()`: `enforcement.enforcepbs`, `true` when unset.
    #[must_use]
    pub fn get_enforce_pbs(floors: Option<&Self>) -> bool {
        floors
            .and_then(|f| f.enforcement.as_deref())
            .and_then(|e| e.enforce_pbs)
            .unwrap_or(true)
    }

    /// Go: `GetFloorsSkippedFlag()`: `skipped`, `false` when unset.
    #[must_use]
    pub fn get_floors_skipped_flag(floors: Option<&Self>) -> bool {
        floors.and_then(|f| f.skipped).unwrap_or(false)
    }

    /// Go: `GetEnforceRate()`: `enforcement.enforcerate`, 0 without an enforcement object.
    #[must_use]
    pub fn get_enforce_rate(floors: Option<&Self>) -> isize {
        floors
            .and_then(|f| f.enforcement.as_deref())
            .map_or(0, |e| e.enforce_rate)
    }

    /// Go: `GetEnforceDealsFlag()`: `enforcement.floordeals`, `false` when unset.
    #[must_use]
    pub fn get_enforce_deals_flag(floors: Option<&Self>) -> bool {
        floors
            .and_then(|f| f.enforcement.as_deref())
            .and_then(|e| e.floor_deals)
            .unwrap_or(false)
    }

    /// Go: `GetEnabled()`: `enabled`, `true` when unset.
    #[must_use]
    pub fn get_enabled(floors: Option<&Self>) -> bool {
        floors.and_then(|f| f.enabled).unwrap_or(true)
    }

    /// Go: `(*PriceFloorRules).DeepCopy()` (a nil receiver gives nil: map the `Option`). `data`
    /// and `enforcement` are copied with their own `DeepCopy`.
    #[must_use]
    pub fn deep_copy(&self) -> Self {
        // Go: openrtb_ext/floors.go:157
        Self {
            data: self.data.as_deref().map(|data| Box::new(data.deep_copy())),
            enforcement: self
                .enforcement
                .as_deref()
                .map(|enforcement| Box::new(enforcement.deep_copy())),
            ..self.clone()
        }
    }
}

impl PriceFloorModelGroup {
    /// Go: `PriceFloorModelGroup.Copy()`: every field, with `schema.fields` and `values`
    /// always non-nil in the copy (empty when they were nil).
    #[must_use]
    pub fn copy(&self) -> Self {
        // Go: openrtb_ext/floors.go:88
        Self {
            currency: self.currency.clone(),
            model_weight: self.model_weight,
            model_version: self.model_version.clone(),
            skip_rate: self.skip_rate,
            schema: PriceFloorSchema {
                fields: Some(self.schema.fields.clone().unwrap_or_default()),
                delimiter: self.schema.delimiter.clone(),
            },
            values: Some(self.values.clone().unwrap_or_default()),
            default: self.default,
        }
    }
}

impl PriceFloorData {
    /// Go: `(*PriceFloorData).DeepCopy()`: every field, with `modelgroups` always non-nil in
    /// the copy (empty when it was nil).
    #[must_use]
    pub fn deep_copy(&self) -> Self {
        // Go: openrtb_ext/floors.go:172
        Self {
            model_groups: Some(self.model_groups.clone().unwrap_or_default()),
            ..self.clone()
        }
    }
}

impl PriceFloorEnforcement {
    /// Go: `(*PriceFloorEnforcement).DeepCopy()`: every field.
    #[must_use]
    pub const fn deep_copy(&self) -> Self {
        *self
    }
}
