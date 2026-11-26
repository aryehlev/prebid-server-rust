//! Glue (Rule B (d)): the parsed keywords of one (sub-)schema.
//!
//! Go: gojsonschema/subSchema.go, the fields of the keywords the port reads.

use regex::Regex;
use serde_json::Number;

use super::{format_checkers::Format, schema_type::JsonSchemaType};

/// Index of a sub-schema inside a compiled [`super::Schema`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SubId(pub(super) usize);

/// Go: `subSchema.additionalProperties` (`interface{}`: nil, a bool or a schema).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Additional {
    /// Not set (Go: nil), or `true`: every property is allowed.
    #[default]
    Allowed,
    /// `false`.
    Forbidden,
    /// A schema every other property must satisfy.
    Schema(SubId),
}

/// Go: a `subSchema.dependencies` value (`[]string` or `*subSchema`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Dependency {
    /// Property names that must be present.
    Properties(Vec<String>),
    /// A schema the whole object must satisfy.
    Schema(SubId),
}

/// Go: `subSchema.pattern`: the source, which errors print, and the compiled pattern.
#[derive(Debug, Clone)]
pub(super) struct Pattern {
    pub(super) source: String,
    pub(super) regex: Regex,
}

/// Go: gojsonschema `subSchema`, for the keywords the port reads.
#[derive(Debug, Clone, Default)]
pub(super) struct SubSchema {
    /// Go: `property`: the key this schema was found under.
    pub(super) property: String,
    pub(super) types: JsonSchemaType,
    pub(super) properties_children: Vec<SubId>,
    pub(super) additional_properties: Additional,
    /// Go: `dependencies` (a map, looked up by the keys of the object validated).
    pub(super) dependencies: Vec<(String, Dependency)>,
    /// Go: `itemsChildren[0]` when `itemsChildrenIsSingleSchema`.
    pub(super) items: Option<SubId>,

    pub(super) maximum: Option<Number>,
    pub(super) minimum: Option<Number>,

    pub(super) min_length: Option<i64>,
    pub(super) max_length: Option<i64>,
    pub(super) pattern: Option<Pattern>,
    pub(super) format: Option<Format>,

    pub(super) required: Vec<String>,

    pub(super) min_items: Option<i64>,
    pub(super) max_items: Option<i64>,

    /// Go: `enum`, the strings (Go compares their `json.Marshal` texts, which are equal exactly
    /// when the strings are).
    pub(super) enum_values: Vec<String>,
    /// The `{{.allowed}}` of an enum error: the values as `json.Marshal` writes them, joined by
    /// `, `.
    pub(super) enum_allowed: String,

    pub(super) one_of: Vec<SubId>,
    pub(super) any_of: Vec<SubId>,
    pub(super) all_of: Vec<SubId>,
    pub(super) not: Option<SubId>,
    pub(super) if_schema: Option<SubId>,
    pub(super) then_schema: Option<SubId>,
}
