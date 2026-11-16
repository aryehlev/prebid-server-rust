//! Go: gojsonschema/errors.go, gojsonschema/locales.go (`DefaultLocale`) and
//! gojsonschema/result.go (`ResultErrorFields`).

use std::fmt;

use super::json_context::{JsonContext, STRING_CONTEXT_ROOT};

/// Go: the concrete `ResultError` types, identified by `ResultError.Type()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorType {
    /// `FalseError` (`false`).
    False,
    /// `RequiredError` (`required`).
    Required,
    /// `InvalidTypeError` (`invalid_type`).
    InvalidType,
    /// `NumberAnyOfError` (`number_any_of`).
    NumberAnyOf,
    /// `NumberOneOfError` (`number_one_of`).
    NumberOneOf,
    /// `NumberAllOfError` (`number_all_of`).
    NumberAllOf,
    /// `NumberNotError` (`number_not`).
    NumberNot,
    /// `MissingDependencyError` (`missing_dependency`).
    MissingDependency,
    /// `ConstError` (`const`).
    Const,
    /// `EnumError` (`enum`).
    Enum,
    /// `ArrayNoAdditionalItemsError` (`array_no_additional_items`).
    ArrayNoAdditionalItems,
    /// `ArrayMinItemsError` (`array_min_items`).
    ArrayMinItems,
    /// `ArrayMaxItemsError` (`array_max_items`).
    ArrayMaxItems,
    /// `ItemsMustBeUniqueError` (`unique`).
    Unique,
    /// `ArrayContainsError` (`contains`).
    ArrayContains,
    /// `ArrayMinPropertiesError` (`array_min_properties`).
    ArrayMinProperties,
    /// `ArrayMaxPropertiesError` (`array_max_properties`).
    ArrayMaxProperties,
    /// `AdditionalPropertyNotAllowedError` (`additional_property_not_allowed`).
    AdditionalPropertyNotAllowed,
    /// `InvalidPropertyNameError` (`invalid_property_name`).
    InvalidPropertyName,
    /// `StringLengthGTEError` (`string_gte`).
    StringGte,
    /// `StringLengthLTEError` (`string_lte`).
    StringLte,
    /// `DoesNotMatchPatternError` (`pattern`).
    Pattern,
    /// `DoesNotMatchFormatError` (`format`).
    Format,
    /// `MultipleOfError` (`multiple_of`).
    MultipleOf,
    /// `NumberGTEError` (`number_gte`).
    NumberGte,
    /// `NumberGTError` (`number_gt`).
    NumberGt,
    /// `NumberLTEError` (`number_lte`).
    NumberLte,
    /// `NumberLTError` (`number_lt`).
    NumberLt,
    /// `ConditionThenError` (`condition_then`).
    ConditionThen,
    /// `ConditionElseError` (`condition_else`).
    ConditionElse,
}

impl ErrorType {
    /// Go: `ResultError.Type()`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::False => "false",
            Self::Required => "required",
            Self::InvalidType => "invalid_type",
            Self::NumberAnyOf => "number_any_of",
            Self::NumberOneOf => "number_one_of",
            Self::NumberAllOf => "number_all_of",
            Self::NumberNot => "number_not",
            Self::MissingDependency => "missing_dependency",
            Self::Const => "const",
            Self::Enum => "enum",
            Self::ArrayNoAdditionalItems => "array_no_additional_items",
            Self::ArrayMinItems => "array_min_items",
            Self::ArrayMaxItems => "array_max_items",
            Self::Unique => "unique",
            Self::ArrayContains => "contains",
            Self::ArrayMinProperties => "array_min_properties",
            Self::ArrayMaxProperties => "array_max_properties",
            Self::AdditionalPropertyNotAllowed => "additional_property_not_allowed",
            Self::InvalidPropertyName => "invalid_property_name",
            Self::StringGte => "string_gte",
            Self::StringLte => "string_lte",
            Self::Pattern => "pattern",
            Self::Format => "format",
            Self::MultipleOf => "multiple_of",
            Self::NumberGte => "number_gte",
            Self::NumberGt => "number_gt",
            Self::NumberLte => "number_lte",
            Self::NumberLt => "number_lt",
            Self::ConditionThen => "condition_then",
            Self::ConditionElse => "condition_else",
        }
    }
}

/// The `ErrorDetails` values a description template refers to.
#[derive(Debug, Clone, Copy)]
pub(super) enum Details<'a> {
    /// No template variables.
    None,
    /// `{{.property}}`.
    Property(&'a str),
    /// `{{.expected}}` and `{{.given}}`.
    Types(&'a str, &'a str),
    /// `{{.dependency}}`.
    Dependency(&'a str),
    /// `{{.allowed}}` (with `{{.field}}`).
    Allowed(&'a str),
    /// `{{.min}}`, `{{.max}}` or `{{.multiple}}`, already printed.
    Limit(&'a str),
    /// `{{.type}}`, `{{.i}}` and `{{.j}}`.
    Unique(&'a str, usize, usize),
    /// `{{.pattern}}`.
    Pattern(&'a str),
    /// `{{.format}}`.
    Format(&'a str),
}

/// Go: the `DefaultLocale` template for `error_type`, executed with `details`.
fn describe(error_type: ErrorType, field: &str, details: Details<'_>) -> String {
    let (property, expected, given, allowed, limit, pattern) = match details {
        Details::Property(p) | Details::Dependency(p) => (p, "", "", "", "", ""),
        Details::Types(e, g) => ("", e, g, "", "", ""),
        Details::Allowed(a) => ("", "", "", a, "", ""),
        Details::Limit(l) => ("", "", "", "", l, ""),
        Details::Pattern(p) | Details::Format(p) => ("", "", "", "", "", p),
        Details::None | Details::Unique(..) => ("", "", "", "", "", ""),
    };
    match error_type {
        ErrorType::False => "False always fails validation".to_owned(),
        ErrorType::Required => format!("{property} is required"),
        ErrorType::InvalidType => format!("Invalid type. Expected: {expected}, given: {given}"),
        ErrorType::NumberAnyOf => "Must validate at least one schema (anyOf)".to_owned(),
        ErrorType::NumberOneOf => "Must validate one and only one schema (oneOf)".to_owned(),
        ErrorType::NumberAllOf => "Must validate all the schemas (allOf)".to_owned(),
        ErrorType::NumberNot => "Must not validate the schema (not)".to_owned(),
        ErrorType::MissingDependency => format!("Has a dependency on {property}"),
        ErrorType::Const => format!("{field} does not match: {allowed}"),
        ErrorType::Enum => format!("{field} must be one of the following: {allowed}"),
        ErrorType::ArrayNoAdditionalItems => "No additional items allowed on array".to_owned(),
        ErrorType::ArrayMinItems => format!("Array must have at least {limit} items"),
        ErrorType::ArrayMaxItems => format!("Array must have at most {limit} items"),
        ErrorType::Unique => match details {
            Details::Unique(t, i, j) => format!("{t} items[{i},{j}] must be unique"),
            Details::None
            | Details::Property(_)
            | Details::Types(..)
            | Details::Dependency(_)
            | Details::Allowed(_)
            | Details::Limit(_)
            | Details::Pattern(_)
            | Details::Format(_) => String::new(),
        },
        ErrorType::ArrayContains => "At least one of the items must match".to_owned(),
        ErrorType::ArrayMinProperties => format!("Must have at least {limit} properties"),
        ErrorType::ArrayMaxProperties => format!("Must have at most {limit} properties"),
        ErrorType::AdditionalPropertyNotAllowed => {
            format!("Additional property {property} is not allowed")
        }
        ErrorType::InvalidPropertyName => format!("Property name of \"{property}\" does not match"),
        ErrorType::StringGte => format!("String length must be greater than or equal to {limit}"),
        ErrorType::StringLte => format!("String length must be less than or equal to {limit}"),
        ErrorType::Pattern => format!("Does not match pattern '{pattern}'"),
        ErrorType::Format => format!("Does not match format '{pattern}'"),
        ErrorType::MultipleOf => format!("Must be a multiple of {limit}"),
        ErrorType::NumberGte => format!("Must be greater than or equal to {limit}"),
        ErrorType::NumberGt => format!("Must be greater than {limit}"),
        ErrorType::NumberLte => format!("Must be less than or equal to {limit}"),
        ErrorType::NumberLt => format!("Must be less than {limit}"),
        ErrorType::ConditionThen => "Must validate \"then\" as \"if\" was valid".to_owned(),
        ErrorType::ConditionElse => "Must validate \"else\" as \"if\" was not valid".to_owned(),
    }
}

/// Go: gojsonschema `ResultError` (`ResultErrorFields`) as the default locale renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultError {
    error_type: ErrorType,
    context: String,
    description: String,
}

impl ResultError {
    /// Go: `newError`: fills the type, the context and the rendered description.
    pub(super) fn new(
        error_type: ErrorType,
        context: &JsonContext<'_>,
        details: Details<'_>,
    ) -> Self {
        let context = context.to_go_string();
        let description = describe(error_type, field_of(&context), details);
        Self {
            error_type,
            context,
            description,
        }
    }

    /// Go: `ResultError.Type()`.
    #[must_use]
    pub const fn error_type(&self) -> ErrorType {
        self.error_type
    }

    /// Go: `ResultError.Context().String()`, e.g. `(root).keywords.0`.
    #[must_use]
    pub fn context(&self) -> &str {
        &self.context
    }

    /// Go: `ResultError.Field()`: the context without the `(root).` prefix.
    #[must_use]
    pub fn field(&self) -> &str {
        field_of(&self.context)
    }

    /// Go: `ResultError.Description()`.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }
}

/// Go: `ResultError.String()` with the default `ErrorFormat` (`{{.field}}: {{.description}}`).
impl fmt::Display for ResultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field(), self.description)
    }
}

/// Go: `strings.TrimPrefix(context, STRING_ROOT_SCHEMA_PROPERTY + ".")`.
fn field_of(context: &str) -> &str {
    context
        .strip_prefix(STRING_CONTEXT_ROOT)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or(context)
}

/// Go: gojsonschema `Result`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationResult {
    errors: Vec<ResultError>,
    score: i64,
}

impl ValidationResult {
    /// Go: `Result.Valid()`.
    #[must_use]
    pub const fn valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Go: `Result.Errors()`, in Go's order.
    #[must_use]
    pub fn errors(&self) -> &[ResultError] {
        &self.errors
    }

    /// The match score gojsonschema uses to pick the closest `anyOf`/`oneOf` branch.
    pub(super) const fn score(&self) -> i64 {
        self.score
    }

    /// Go: `addInternalError`.
    pub(super) fn add_internal_error(
        &mut self,
        error_type: ErrorType,
        context: &JsonContext<'_>,
        details: Details<'_>,
    ) {
        self.errors
            .push(ResultError::new(error_type, context, details));
        // Go: results in a net -1 when added to the +1 at the end of the validation function.
        self.score -= 2;
    }

    /// Go: `mergeErrors`.
    pub(super) fn merge_errors(&mut self, other: Self) {
        self.errors.extend(other.errors);
        self.score += other.score;
    }

    /// Go: `incrementScore`.
    pub(super) const fn increment_score(&mut self) {
        self.score += 1;
    }
}
