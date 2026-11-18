//! Go: gojsonschema/subSchema.go.

use super::{format_checkers::Format, rat::Rat, regexp::GoRegexp, schema_type::JsonSchemaType};

/// Index of a sub-schema inside a compiled [`super::Schema`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SubId(pub(super) usize);

/// Go: `subSchema.additionalProperties` / `additionalItems` (`interface{}`: nil, bool or schema).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Additional {
    /// Not set (Go: nil).
    #[default]
    Unset,
    /// `true`.
    Allowed,
    /// `false`.
    Forbidden,
    /// A schema.
    Schema(SubId),
}

impl Additional {
    /// The boolean form.
    pub(super) const fn from_bool(allowed: bool) -> Self {
        if allowed {
            Self::Allowed
        } else {
            Self::Forbidden
        }
    }
}

/// Go: a `subSchema.dependencies` value (`[]string` or `*subSchema`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Dependency {
    /// Property names that must be present.
    Properties(Vec<String>),
    /// A schema the whole object must validate against.
    Schema(SubId),
}

/// Go: gojsonschema `subSchema` (only the fields validation reads).
#[derive(Debug, Clone, Default)]
pub(super) struct SubSchema {
    /// Go: `property`: the key this schema was found under.
    pub(super) property: String,
    /// Go: `pass`: set for the boolean schemas `true` and `false`.
    pub(super) pass: Option<bool>,
    pub(super) types: JsonSchemaType,
    pub(super) items_children: Vec<SubId>,
    pub(super) items_children_is_single_schema: bool,
    pub(super) properties_children: Vec<SubId>,

    pub(super) multiple_of: Option<Rat>,
    pub(super) maximum: Option<Rat>,
    pub(super) exclusive_maximum: Option<Rat>,
    pub(super) minimum: Option<Rat>,
    pub(super) exclusive_minimum: Option<Rat>,

    pub(super) min_length: Option<i64>,
    pub(super) max_length: Option<i64>,
    pub(super) pattern: Option<GoRegexp>,
    pub(super) format: Format,

    pub(super) min_properties: Option<i64>,
    pub(super) max_properties: Option<i64>,
    pub(super) required: Vec<String>,
    /// Go: `dependencies` (a map; looked up by key).
    pub(super) dependencies: Vec<(String, Dependency)>,
    pub(super) additional_properties: Additional,
    /// Go: `patternProperties` (a map; every entry is tried).
    pub(super) pattern_properties: Vec<(GoRegexp, SubId)>,
    pub(super) property_names: Option<SubId>,

    pub(super) min_items: Option<i64>,
    pub(super) max_items: Option<i64>,
    pub(super) unique_items: bool,
    pub(super) contains: Option<SubId>,
    pub(super) additional_items: Additional,

    pub(super) const_value: Option<String>,
    pub(super) enum_values: Vec<String>,

    pub(super) one_of: Vec<SubId>,
    pub(super) any_of: Vec<SubId>,
    pub(super) all_of: Vec<SubId>,
    pub(super) not: Option<SubId>,
    pub(super) if_schema: Option<SubId>,
    pub(super) then_schema: Option<SubId>,
    pub(super) else_schema: Option<SubId>,
}
