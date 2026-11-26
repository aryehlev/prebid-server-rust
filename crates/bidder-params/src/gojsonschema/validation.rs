//! Glue (Rule B (d)): gojsonschema's walk of a document, which decides the errors Prebid joins
//! into its 400 response and their order.
//!
//! Go: gojsonschema/validation.go, for the keywords the port reads. The order in which keywords
//! add errors and the match score that selects the closest `anyOf`/`oneOf` branch are Go's;
//! where Go walks a map (the document's keys) the port walks `serde_json`'s key order.

use std::{error::Error, fmt};

use serde_json::{Map, Number, Value};

use super::{
    errors::{Details, ErrorType, ValidationResult},
    json_context::JsonContext,
    number::{compare, is_integer, limit_text},
    schema::Schema,
    schema_type::{
        TYPE_ARRAY, TYPE_BOOLEAN, TYPE_INTEGER, TYPE_NULL, TYPE_NUMBER, TYPE_OBJECT, TYPE_STRING,
    },
    sub_schema::{Additional, Dependency, SubId, SubSchema},
};

/// A condition under which Go panics instead of returning a result (a nil pointer
/// dereference). A Go server handling the request would abort it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoPanic(String);

impl GoPanic {
    /// Go: `Validate` called on a nil `*gojsonschema.Schema` (a bidder without a schema).
    #[must_use]
    pub fn nil_schema() -> Self {
        Self(
            "runtime error: invalid memory address or nil pointer dereference (no schema for \
             this bidder)"
                .to_owned(),
        )
    }
}

impl fmt::Display for GoPanic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for GoPanic {}

/// Why [`Schema::validate`] produced no [`ValidationResult`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidateError {
    /// Go: the error `JSONLoader.LoadJSON` returned (here `serde_json`'s text,
    /// jsonschema.decode-errors).
    Load(String),
    /// Go panics while validating this document. The port reads no number Go panics on
    /// (jsonschema.number-precision), so validation does not produce this.
    Panic(GoPanic),
}

impl fmt::Display for ValidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(text) => f.write_str(text),
            Self::Panic(panic) => write!(f, "panic: {panic}"),
        }
    }
}

impl Error for ValidateError {}

/// Go: `(*Schema).validateDocument(root)`.
pub(super) fn validate_document(schema: &Schema, document: &Value) -> ValidationResult {
    let mut result = ValidationResult::default();
    Validator { schema }.validate_recursive(
        schema.root(),
        document,
        &mut result,
        &JsonContext::root(),
    );
    result
}

struct Validator<'s> {
    schema: &'s Schema,
}

impl<'s> Validator<'s> {
    fn sub(&self, id: SubId) -> Option<&'s SubSchema> {
        self.schema.sub(id)
    }

    /// Go: `subValidateWithContext`.
    fn sub_validate(&self, sub: SubId, node: &Value, ctx: &JsonContext<'_>) -> ValidationResult {
        let mut result = ValidationResult::default();
        self.validate_recursive(sub, node, &mut result, ctx);
        result
    }

    /// Go: `validateRecursive`.
    fn validate_recursive(
        &self,
        sub_id: SubId,
        node: &Value,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) {
        let Some(sub) = self.sub(sub_id) else {
            return;
        };
        let given = match node {
            Value::Null => TYPE_NULL,
            Value::Number(n) if is_integer(n) => TYPE_INTEGER,
            Value::Number(_) => TYPE_NUMBER,
            Value::Array(_) => TYPE_ARRAY,
            Value::Object(_) => TYPE_OBJECT,
            Value::Bool(_) => TYPE_BOOLEAN,
            Value::String(_) => TYPE_STRING,
        };
        let valid_type = match given {
            TYPE_INTEGER => sub.types.contains(TYPE_NUMBER) || sub.types.contains(TYPE_INTEGER),
            other => sub.types.contains(other),
        };
        if sub.types.is_typed() && !valid_type {
            let expected = sub.types.to_go_string();
            result.add_internal_error(
                ErrorType::InvalidType,
                ctx,
                Details::Types(&expected, given),
            );
            return;
        }
        self.validate_schema(sub, node, result, ctx);
        match node {
            Value::Null | Value::Bool(_) => Self::validate_common(sub, node, result, ctx),
            Value::Number(n) => {
                Self::validate_number(sub, n, result, ctx);
                Self::validate_common(sub, node, result, ctx);
            }
            Value::String(s) => {
                Self::validate_common(sub, node, result, ctx);
                Self::validate_string(sub, s, result, ctx);
            }
            Value::Array(items) => {
                self.validate_array(sub, items, result, ctx);
                Self::validate_common(sub, node, result, ctx);
            }
            Value::Object(members) => {
                self.validate_object(sub, members, result, ctx);
                Self::validate_common(sub, node, result, ctx);
                for &child in &sub.properties_children {
                    let Some(child_schema) = self.sub(child) else {
                        continue;
                    };
                    if let Some(next) = members.get(&child_schema.property) {
                        let sub_ctx = ctx.child(&child_schema.property);
                        self.validate_recursive(child, next, result, &sub_ctx);
                    }
                }
            }
        }
        result.increment_score();
    }

    /// Go: `validateSchema`: `anyOf`, `oneOf`, `allOf`, `not`, `dependencies` and `if`.
    fn validate_schema(
        &self,
        sub: &SubSchema,
        node: &Value,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) {
        if !sub.any_of.is_empty() {
            self.validate_any_of(sub, node, result, ctx);
        }
        if !sub.one_of.is_empty() {
            self.validate_one_of(sub, node, result, ctx);
        }
        if !sub.all_of.is_empty() {
            let mut validated = 0usize;
            for &all_of in &sub.all_of {
                let r = self.sub_validate(all_of, node, ctx);
                if r.valid() {
                    validated += 1;
                }
                result.merge_errors(r);
            }
            if validated != sub.all_of.len() {
                result.add_internal_error(ErrorType::NumberAllOf, ctx, Details::None);
            }
        }
        if let Some(not) = sub.not
            && self.sub_validate(not, node, ctx).valid()
        {
            result.add_internal_error(ErrorType::NumberNot, ctx, Details::None);
        }
        if let Value::Object(members) = node
            && !sub.dependencies.is_empty()
        {
            self.validate_dependencies(sub, node, members, result, ctx);
        }
        if let Some(if_schema) = sub.if_schema
            && let Some(then_schema) = sub.then_schema
            && self.sub_validate(if_schema, node, ctx).valid()
        {
            let r = self.sub_validate(then_schema, node, ctx);
            if !r.valid() {
                result.add_internal_error(ErrorType::ConditionThen, ctx, Details::None);
                result.merge_errors(r);
            }
        }
        result.increment_score();
    }

    fn validate_any_of(
        &self,
        sub: &SubSchema,
        node: &Value,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) {
        let mut best: Option<ValidationResult> = None;
        for &any_of in &sub.any_of {
            let r = self.sub_validate(any_of, node, ctx);
            if r.valid() {
                return;
            }
            if best.as_ref().is_none_or(|b| r.score() > b.score()) {
                best = Some(r);
            }
        }
        result.add_internal_error(ErrorType::NumberAnyOf, ctx, Details::None);
        if let Some(best) = best {
            // Go: add the errors of the closest matching sub-schema, probably the one the user
            // was trying to match.
            result.merge_errors(best);
        }
    }

    fn validate_one_of(
        &self,
        sub: &SubSchema,
        node: &Value,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) {
        let mut validated = 0usize;
        let mut best: Option<ValidationResult> = None;
        for &one_of in &sub.one_of {
            let r = self.sub_validate(one_of, node, ctx);
            if r.valid() {
                validated += 1;
            } else if validated == 0 && best.as_ref().is_none_or(|b| r.score() > b.score()) {
                best = Some(r);
            }
        }
        if validated != 1 {
            result.add_internal_error(ErrorType::NumberOneOf, ctx, Details::None);
            if validated == 0
                && let Some(best) = best
            {
                result.merge_errors(best);
            }
        }
    }

    /// The `dependencies` part of `validateSchema`, for the keys of the object.
    fn validate_dependencies(
        &self,
        sub: &SubSchema,
        node: &Value,
        members: &Map<String, Value>,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) {
        for key in members.keys() {
            let Some((_, dependency)) = sub.dependencies.iter().find(|(k, _)| k == key) else {
                continue;
            };
            match dependency {
                Dependency::Properties(names) => {
                    for name in names {
                        if !members.contains_key(name) {
                            result.add_internal_error(
                                ErrorType::MissingDependency,
                                ctx,
                                Details::Dependency(name),
                            );
                        }
                    }
                }
                Dependency::Schema(schema) => self.validate_recursive(*schema, node, result, ctx),
            }
        }
    }

    /// Go: `validateCommon`: `enum`, whose values are strings.
    fn validate_common(
        sub: &SubSchema,
        node: &Value,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) {
        if !sub.enum_values.is_empty()
            && !matches!(node, Value::String(s) if sub.enum_values.contains(s))
        {
            result.add_internal_error(ErrorType::Enum, ctx, Details::Allowed(&sub.enum_allowed));
        }
        result.increment_score();
    }

    /// Go: `validateArray`.
    fn validate_array(
        &self,
        sub: &SubSchema,
        items: &[Value],
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) {
        if let Some(item_schema) = sub.items {
            for (i, item) in items.iter().enumerate() {
                let index = i.to_string();
                let r = self.sub_validate(item_schema, item, &ctx.child(&index));
                result.merge_errors(r);
            }
        }
        let count = i64::try_from(items.len()).unwrap_or(i64::MAX);
        if let Some(min) = sub.min_items
            && count < min
        {
            let limit = min.to_string();
            result.add_internal_error(ErrorType::ArrayMinItems, ctx, Details::Limit(&limit));
        }
        if let Some(max) = sub.max_items
            && count > max
        {
            let limit = max.to_string();
            result.add_internal_error(ErrorType::ArrayMaxItems, ctx, Details::Limit(&limit));
        }
        result.increment_score();
    }

    /// Go: `validateObject`.
    fn validate_object(
        &self,
        sub: &SubSchema,
        members: &Map<String, Value>,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) {
        for required in &sub.required {
            if members.contains_key(required) {
                result.increment_score();
            } else {
                result.add_internal_error(ErrorType::Required, ctx, Details::Property(required));
            }
        }
        if sub.additional_properties != Additional::Allowed {
            for (key, value) in members {
                let found = sub
                    .properties_children
                    .iter()
                    .any(|&c| self.sub(c).is_some_and(|s| s.property == *key));
                if found {
                    continue;
                }
                match sub.additional_properties {
                    Additional::Forbidden => result.add_internal_error(
                        ErrorType::AdditionalPropertyNotAllowed,
                        ctx,
                        Details::Property(key),
                    ),
                    Additional::Schema(schema) => {
                        let r = self.sub_validate(schema, value, &ctx.child(key));
                        result.merge_errors(r);
                    }
                    Additional::Allowed => {}
                }
            }
        }
        result.increment_score();
    }

    /// Go: `validateString`, for a string value.
    fn validate_string(
        sub: &SubSchema,
        value: &str,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) {
        // Go: utf8.RuneCount; serde_json strings are UTF-8.
        let length = i64::try_from(value.chars().count()).unwrap_or(i64::MAX);
        if let Some(min) = sub.min_length
            && length < min
        {
            let limit = min.to_string();
            result.add_internal_error(ErrorType::StringGte, ctx, Details::Limit(&limit));
        }
        if let Some(max) = sub.max_length
            && length > max
        {
            let limit = max.to_string();
            result.add_internal_error(ErrorType::StringLte, ctx, Details::Limit(&limit));
        }
        if let Some(pattern) = &sub.pattern
            && !pattern.regex.is_match(value)
        {
            result.add_internal_error(ErrorType::Pattern, ctx, Details::Pattern(&pattern.source));
        }
        if let Some(format) = sub.format
            && !format.is_format(value)
        {
            result.add_internal_error(ErrorType::Format, ctx, Details::Format(format.name()));
        }
        result.increment_score();
    }

    /// Go: `validateNumber`, for a number value.
    fn validate_number(
        sub: &SubSchema,
        value: &Number,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) {
        if let Some(max) = &sub.maximum
            && compare(value, max).is_gt()
        {
            let limit = limit_text(max);
            result.add_internal_error(ErrorType::NumberLte, ctx, Details::Limit(&limit));
        }
        if let Some(min) = &sub.minimum
            && compare(value, min).is_lt()
        {
            let limit = limit_text(min);
            result.add_internal_error(ErrorType::NumberGte, ctx, Details::Limit(&limit));
        }
        // Go's checkers fail every value that is not a string, as the *big.Rat here.
        if let Some(format) = sub.format {
            result.add_internal_error(ErrorType::Format, ctx, Details::Format(format.name()));
        }
        result.increment_score();
    }
}

#[cfg(test)]
mod tests {
    use crate::gojsonschema::Schema;

    fn texts(schema: &str, document: &str) -> Result<Vec<String>, String> {
        let result = Schema::new(schema.as_bytes())?
            .validate(document.as_bytes())
            .map_err(|e| e.to_string())?;
        Ok(result.errors().iter().map(ToString::to_string).collect())
    }

    /// Go's texts and order for one value failing several keywords, and the closest branch
    /// of a failed `anyOf` and `oneOf`.
    #[test]
    fn errors_follow_go() -> Result<(), String> {
        let cases: [(&str, &str, &[&str]); 9] = [
            (
                concat!(
                    r#"{"type":"object","required":["a","b"],"additionalProperties":false,"#,
                    r#""properties":{"a":{"type":"integer","minimum":2}}}"#,
                ),
                r#"{"a":1,"c":true}"#,
                &[
                    "(root): b is required",
                    "(root): Additional property c is not allowed",
                    "a: Must be greater than or equal to 2",
                ],
            ),
            (
                r#"{"anyOf":[{"type":"string"},{"type":"object","required":["x","y"]}]}"#,
                "{}",
                &[
                    "(root): Must validate at least one schema (anyOf)",
                    "(root): x is required",
                    "(root): y is required",
                ],
            ),
            (
                r#"{"oneOf":[{"type":"string","minLength":1},{"type":"string","maxLength":5}]}"#,
                r#""abc""#,
                &["(root): Must validate one and only one schema (oneOf)"],
            ),
            (
                r#"{"properties":{"k":{"enum":["a","<b>"]}}}"#,
                r#"{"k":1}"#,
                &[r#"k: k must be one of the following: "a", "\u003cb\u003e""#],
            ),
            (
                r#"{"items":{"type":"string","pattern":"^[a-z]+$"},"maxItems":1}"#,
                r#"["ok","NO"]"#,
                &[
                    "1: Does not match pattern '^[a-z]+$'",
                    "(root): Array must have at most 1 items",
                ],
            ),
            (
                r#"{"format":"uuid"}"#,
                "1.5",
                &["(root): Does not match format 'uuid'"],
            ),
            (
                r#"{"dependencies":{"a":["b","c"]}}"#,
                r#"{"a":1,"c":2}"#,
                &["(root): Has a dependency on b"],
            ),
            (
                concat!(
                    r#"{"$schema":"http://json-schema.org/draft-07/schema#","#,
                    r#""if":{"required":["a"]},"then":{"required":["b"]}}"#,
                ),
                r#"{"a":1}"#,
                &[
                    "(root): Must validate \"then\" as \"if\" was valid",
                    "(root): b is required",
                ],
            ),
            (
                r#"{"type":["string","null"],"maximum":1e6}"#,
                "2",
                &["(root): Invalid type. Expected: [string,null], given: integer"],
            ),
        ];
        for (schema, document, want) in cases {
            let got = texts(schema, document)?;
            if got != want {
                return Err(format!(
                    "{schema} on {document}: got {got:?}, want {want:?}"
                ));
            }
        }
        // Under draft 4 and 6 gojsonschema does not read if and then.
        let draft4 =
            r#"{"$schema":"http://json-schema.org/draft-04/schema#","if":{},"then":false}"#;
        if !texts(draft4, "1")?.is_empty() {
            return Err("draft 4 reads if".to_owned());
        }
        Ok(())
    }
}
