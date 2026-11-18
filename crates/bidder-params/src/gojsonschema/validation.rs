//! Go: gojsonschema/validation.go.
//!
//! The walk, the order in which keywords add errors, and the match score that selects the
//! closest `anyOf`/`oneOf` branch are ported statement by statement. Where Go iterates a map (the
//! instance's keys, `patternProperties`) document order is used.
//!
//! A few inputs make Go dereference a nil pointer and panic; they surface as [`GoPanic`].

use std::{collections::BTreeMap, error::Error, fmt};

use super::{
    errors::{Details, ErrorType, ValidationResult},
    json_context::JsonContext,
    json_loader::{Document, Node, NodeId, rune_count},
    marshal::marshal_without_number,
    rat::Rat,
    schema::Schema,
    schema_type::{
        TYPE_ARRAY, TYPE_BOOLEAN, TYPE_INTEGER, TYPE_NULL, TYPE_NUMBER, TYPE_OBJECT, TYPE_STRING,
    },
    sub_schema::{Additional, Dependency, SubId, SubSchema},
    utils::check_json_integer,
};

/// A condition under which Go's gojsonschema panics instead of returning a result (a nil
/// pointer dereference). A Go server handling the request would abort it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoPanic(String);

impl GoPanic {
    fn new(what: &str) -> Self {
        Self(format!(
            "runtime error: invalid memory address or nil pointer dereference ({what})"
        ))
    }

    /// Go: `Validate` called on a nil `*gojsonschema.Schema` (a bidder without a schema).
    #[must_use]
    pub fn nil_schema() -> Self {
        Self::new("no schema for this bidder")
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
    /// Go: the error `JSONLoader.LoadJSON` returned (the `encoding/json` decoder's text).
    Load(String),
    /// Go panics while validating this document.
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

type Step = Result<(), GoPanic>;

/// Go: `(*Schema).validateDocument(root)`.
pub(super) fn validate_document(
    schema: &Schema,
    doc: &Document,
) -> Result<ValidationResult, GoPanic> {
    let mut result = ValidationResult::default();
    let validator = Validator { schema, doc };
    validator.validate_recursive(schema.root(), doc.root(), &mut result, &JsonContext::root())?;
    Ok(result)
}

struct Validator<'s, 'd> {
    schema: &'s Schema,
    doc: &'d Document,
}

impl<'s> Validator<'s, '_> {
    fn sub(&self, id: SubId) -> Option<&'s SubSchema> {
        self.schema.sub(id)
    }

    /// Go: `subValidateWithContext`.
    fn sub_validate(
        &self,
        sub: SubId,
        node: NodeId,
        ctx: &JsonContext<'_>,
    ) -> Result<ValidationResult, GoPanic> {
        let mut result = ValidationResult::default();
        self.validate_recursive(sub, node, &mut result, ctx)?;
        Ok(result)
    }

    /// Go: `validateRecursive`.
    fn validate_recursive(
        &self,
        sub_id: SubId,
        node: NodeId,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Step {
        let Some(sub) = self.sub(sub_id) else {
            return Ok(());
        };
        // Handle true/false schema as early as possible as all other fields will be nil.
        if let Some(pass) = sub.pass {
            if !pass {
                result.add_internal_error(ErrorType::False, ctx, Details::None);
            }
            return Ok(());
        }
        let given = match self.doc.node(node) {
            Node::Null => TYPE_NULL,
            Node::Number(literal) => {
                if check_json_integer(literal) {
                    TYPE_INTEGER
                } else {
                    TYPE_NUMBER
                }
            }
            Node::Array(_) => TYPE_ARRAY,
            Node::Object(_) => TYPE_OBJECT,
            Node::Bool(_) => TYPE_BOOLEAN,
            Node::String(_) => TYPE_STRING,
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
            return Ok(());
        }
        self.validate_schema(sub, node, result, ctx)?;
        match self.doc.node(node) {
            Node::Null | Node::Bool(_) => self.validate_common(sub, node, result, ctx)?,
            Node::Number(literal) => {
                Self::validate_number(sub, literal, result, ctx)?;
                self.validate_common(sub, node, result, ctx)?;
            }
            Node::String(s) => {
                self.validate_common(sub, node, result, ctx)?;
                Self::validate_string(sub, s, result, ctx);
            }
            Node::Array(items) => {
                self.validate_array(sub, items, result, ctx)?;
                self.validate_common(sub, node, result, ctx)?;
            }
            Node::Object(entries) => {
                self.validate_object(sub, entries, result, ctx)?;
                self.validate_common(sub, node, result, ctx)?;
                for &child in &sub.properties_children {
                    let Some(child_schema) = self.sub(child) else {
                        continue;
                    };
                    if let Some(next) = self.doc.get(node, &child_schema.property) {
                        let sub_ctx = ctx.child(&child_schema.property);
                        self.validate_recursive(child, next, result, &sub_ctx)?;
                    }
                }
            }
        }
        result.increment_score();
        Ok(())
    }

    /// Go: `validateSchema`: `anyOf`, `oneOf`, `allOf`, `not`, `dependencies`, `if`.
    fn validate_schema(
        &self,
        sub: &SubSchema,
        node: NodeId,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Step {
        if !sub.any_of.is_empty() {
            self.validate_any_of(sub, node, result, ctx)?;
        }
        if !sub.one_of.is_empty() {
            self.validate_one_of(sub, node, result, ctx)?;
        }
        if !sub.all_of.is_empty() {
            let mut validated = 0usize;
            for &all_of in &sub.all_of {
                let r = self.sub_validate(all_of, node, ctx)?;
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
            && self.sub_validate(not, node, ctx)?.valid()
        {
            result.add_internal_error(ErrorType::NumberNot, ctx, Details::None);
        }
        if !sub.dependencies.is_empty() {
            self.validate_dependencies(sub, node, result, ctx)?;
        }
        if let Some(if_schema) = sub.if_schema {
            self.validate_condition(sub, if_schema, node, result, ctx)?;
        }
        result.increment_score();
        Ok(())
    }

    fn validate_any_of(
        &self,
        sub: &SubSchema,
        node: NodeId,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Step {
        let mut best: Option<ValidationResult> = None;
        for &any_of in &sub.any_of {
            let r = self.sub_validate(any_of, node, ctx)?;
            if r.valid() {
                return Ok(());
            }
            if best.as_ref().is_none_or(|b| r.score() > b.score()) {
                best = Some(r);
            }
        }
        result.add_internal_error(ErrorType::NumberAnyOf, ctx, Details::None);
        if let Some(best) = best {
            // Add the errors of the closest matching sub-schema, probably the one the user was
            // trying to match.
            result.merge_errors(best);
        }
        Ok(())
    }

    fn validate_one_of(
        &self,
        sub: &SubSchema,
        node: NodeId,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Step {
        let mut validated = 0usize;
        let mut best: Option<ValidationResult> = None;
        for &one_of in &sub.one_of {
            let r = self.sub_validate(one_of, node, ctx)?;
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
        Ok(())
    }

    fn validate_dependencies(
        &self,
        sub: &SubSchema,
        node: NodeId,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Step {
        let entries = match self.doc.node(node) {
            Node::Object(entries) => entries,
            // Go: isKind converts a json.Number to a big.Rat and dereferences the result.
            Node::Number(literal) if Rat::parse(literal).is_none() => {
                return Err(GoPanic::new("isKind on a number big.Rat cannot parse"));
            }
            Node::Null | Node::Bool(_) | Node::Number(_) | Node::String(_) | Node::Array(_) => {
                return Ok(());
            }
        };
        for (key, _) in entries {
            let Some((_, dependency)) = sub.dependencies.iter().find(|(k, _)| k == key) else {
                continue;
            };
            match dependency {
                Dependency::Properties(names) => {
                    for name in names {
                        if self.doc.get(node, name).is_none() {
                            result.add_internal_error(
                                ErrorType::MissingDependency,
                                ctx,
                                Details::Dependency(name),
                            );
                        }
                    }
                }
                Dependency::Schema(schema) => {
                    self.validate_recursive(*schema, node, result, ctx)?;
                }
            }
        }
        Ok(())
    }

    fn validate_condition(
        &self,
        sub: &SubSchema,
        if_schema: SubId,
        node: NodeId,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Step {
        let if_valid = self.sub_validate(if_schema, node, ctx)?.valid();
        if let Some(then_schema) = sub.then_schema
            && if_valid
        {
            let r = self.sub_validate(then_schema, node, ctx)?;
            if !r.valid() {
                result.add_internal_error(ErrorType::ConditionThen, ctx, Details::None);
                result.merge_errors(r);
            }
        }
        if let Some(else_schema) = sub.else_schema
            && !if_valid
        {
            let r = self.sub_validate(else_schema, node, ctx)?;
            if !r.valid() {
                result.add_internal_error(ErrorType::ConditionElse, ctx, Details::None);
                result.merge_errors(r);
            }
        }
        Ok(())
    }

    /// Go: `validateCommon`: `const` and `enum`.
    fn validate_common(
        &self,
        sub: &SubSchema,
        node: NodeId,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Step {
        if let Some(const_value) = &sub.const_value {
            let text = self.canonical(node)?;
            if text != *const_value {
                result.add_internal_error(ErrorType::Const, ctx, Details::Allowed(const_value));
            }
        }
        if !sub.enum_values.is_empty() {
            let text = self.canonical(node)?;
            if !sub.enum_values.contains(&text) {
                let allowed = sub.enum_values.join(", ");
                result.add_internal_error(ErrorType::Enum, ctx, Details::Allowed(&allowed));
            }
        }
        result.increment_score();
        Ok(())
    }

    /// Go: `marshalWithoutNumber(value)`; on failure Go adds an internal error and then
    /// dereferences the nil result.
    fn canonical(&self, node: NodeId) -> Result<String, GoPanic> {
        marshal_without_number(self.doc, node)
            .map_err(|e| GoPanic::new(&format!("marshalWithoutNumber failed: {e}")))
    }

    /// Go: `validateArray`.
    fn validate_array(
        &self,
        sub: &SubSchema,
        items: &[NodeId],
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Step {
        if sub.items_children_is_single_schema {
            if let Some(&item_schema) = sub.items_children.first() {
                for (i, &item) in items.iter().enumerate() {
                    let index = i.to_string();
                    let r = self.sub_validate(item_schema, item, &ctx.child(&index))?;
                    result.merge_errors(r);
                }
            }
        } else if !sub.items_children.is_empty() {
            // While we have both schemas and values, check them against each other.
            for (i, (&item_schema, &item)) in sub.items_children.iter().zip(items).enumerate() {
                let index = i.to_string();
                let r = self.sub_validate(item_schema, item, &ctx.child(&index))?;
                result.merge_errors(r);
            }
            if sub.items_children.len() < items.len() {
                self.validate_additional_items(sub, items, result, ctx)?;
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
        if sub.unique_items {
            let mut seen: BTreeMap<String, usize> = BTreeMap::new();
            for (j, &item) in items.iter().enumerate() {
                let text = self.canonical(item)?;
                if let Some(&i) = seen.get(&text) {
                    result.add_internal_error(
                        ErrorType::Unique,
                        ctx,
                        Details::Unique(TYPE_ARRAY, i, j),
                    );
                }
                let _previous = seen.insert(text, j);
            }
        }
        if let Some(contains) = sub.contains {
            self.validate_contains(contains, items, result, ctx)?;
        }
        result.increment_score();
        Ok(())
    }

    /// The `additionalItems` part of `validateArray`.
    fn validate_additional_items(
        &self,
        sub: &SubSchema,
        items: &[NodeId],
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Step {
        match sub.additional_items {
            Additional::Forbidden => {
                result.add_internal_error(ErrorType::ArrayNoAdditionalItems, ctx, Details::None);
            }
            Additional::Schema(schema) => {
                for (i, &item) in items.iter().enumerate().skip(sub.items_children.len()) {
                    let index = i.to_string();
                    let r = self.sub_validate(schema, item, &ctx.child(&index))?;
                    result.merge_errors(r);
                }
            }
            Additional::Allowed | Additional::Unset => {}
        }
        Ok(())
    }

    /// The `contains` part of `validateArray`.
    fn validate_contains(
        &self,
        contains: SubId,
        items: &[NodeId],
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Step {
        let mut best: Option<ValidationResult> = None;
        for (i, &item) in items.iter().enumerate() {
            let index = i.to_string();
            let r = self.sub_validate(contains, item, &ctx.child(&index))?;
            if r.valid() {
                return Ok(());
            }
            if best.as_ref().is_none_or(|b| r.score() > b.score()) {
                best = Some(r);
            }
        }
        result.add_internal_error(ErrorType::ArrayContains, ctx, Details::None);
        if let Some(best) = best {
            result.merge_errors(best);
        }
        Ok(())
    }

    /// Go: `validateObject`.
    fn validate_object(
        &self,
        sub: &SubSchema,
        entries: &[(String, NodeId)],
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Step {
        let count = i64::try_from(entries.len()).unwrap_or(i64::MAX);
        if let Some(min) = sub.min_properties
            && count < min
        {
            let limit = min.to_string();
            result.add_internal_error(ErrorType::ArrayMinProperties, ctx, Details::Limit(&limit));
        }
        if let Some(max) = sub.max_properties
            && count > max
        {
            let limit = max.to_string();
            result.add_internal_error(ErrorType::ArrayMaxProperties, ctx, Details::Limit(&limit));
        }
        for required in &sub.required {
            if entries.iter().any(|(k, _)| k == required) {
                result.increment_score();
            } else {
                result.add_internal_error(ErrorType::Required, ctx, Details::Property(required));
            }
        }
        for (key, value) in entries {
            let found = sub
                .properties_children
                .iter()
                .any(|&c| self.sub(c).is_some_and(|s| s.property == *key));
            let pattern_match = self.validate_pattern_property(sub, key, *value, result, ctx)?;
            if found || pattern_match {
                continue;
            }
            match sub.additional_properties {
                Additional::Forbidden => {
                    result.add_internal_error(
                        ErrorType::AdditionalPropertyNotAllowed,
                        ctx,
                        Details::Property(key),
                    );
                }
                Additional::Schema(schema) => {
                    let r = self.sub_validate(schema, *value, &ctx.child(key))?;
                    result.merge_errors(r);
                }
                Additional::Allowed | Additional::Unset => {}
            }
        }
        if let Some(property_names) = sub.property_names {
            for (key, _) in entries {
                let name_doc = Document::from_string(key);
                let name_validator = Validator {
                    schema: self.schema,
                    doc: &name_doc,
                };
                let r = name_validator.sub_validate(property_names, name_doc.root(), ctx)?;
                if !r.valid() {
                    result.add_internal_error(
                        ErrorType::InvalidPropertyName,
                        ctx,
                        Details::Property(key),
                    );
                    result.merge_errors(r);
                }
            }
        }
        result.increment_score();
        Ok(())
    }

    /// Go: `validatePatternProperty`.
    fn validate_pattern_property(
        &self,
        sub: &SubSchema,
        key: &str,
        value: NodeId,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Result<bool, GoPanic> {
        let mut validated = false;
        for (pattern, schema) in &sub.pattern_properties {
            if pattern.match_string(key) {
                validated = true;
                let r = self.sub_validate(*schema, value, &ctx.child(key))?;
                result.merge_errors(r);
            }
        }
        if !validated {
            return Ok(false);
        }
        result.increment_score();
        Ok(true)
    }

    /// Go: `validateString` (for string values; Go returns early for everything else).
    fn validate_string(
        sub: &SubSchema,
        value: &str,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) {
        let length = i64::try_from(rune_count(value)).unwrap_or(i64::MAX);
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
            && !pattern.match_string(value)
        {
            result.add_internal_error(ErrorType::Pattern, ctx, Details::Pattern(pattern.as_str()));
        }
        if sub.format.is_set() && !sub.format.is_format_string(value) {
            result.add_internal_error(ErrorType::Format, ctx, Details::Format(sub.format.name()));
        }
        result.increment_score();
    }

    /// Go: `validateNumber` (for number values; Go returns early for everything else).
    fn validate_number(
        sub: &SubSchema,
        literal: &str,
        result: &mut ValidationResult,
        ctx: &JsonContext<'_>,
    ) -> Step {
        let value = Rat::parse(literal);
        // Go: a failed big.Rat SetString yields nil, and the comparisons dereference it.
        let checked = || {
            value
                .as_ref()
                .ok_or_else(|| GoPanic::new("big.Rat comparison on nil"))
        };
        if let Some(multiple) = &sub.multiple_of
            && !checked()?.quo_is_int(multiple)
        {
            let limit = multiple.go_float_string();
            result.add_internal_error(ErrorType::MultipleOf, ctx, Details::Limit(&limit));
        }
        if let Some(max) = &sub.maximum
            && checked()?.cmp_rat(max).is_gt()
        {
            let limit = max.go_float_string();
            result.add_internal_error(ErrorType::NumberLte, ctx, Details::Limit(&limit));
        }
        if let Some(max) = &sub.exclusive_maximum
            && checked()?.cmp_rat(max).is_ge()
        {
            let limit = max.go_float_string();
            result.add_internal_error(ErrorType::NumberLt, ctx, Details::Limit(&limit));
        }
        if let Some(min) = &sub.minimum
            && checked()?.cmp_rat(min).is_lt()
        {
            let limit = min.go_float_string();
            result.add_internal_error(ErrorType::NumberGte, ctx, Details::Limit(&limit));
        }
        if let Some(min) = &sub.exclusive_minimum
            && checked()?.cmp_rat(min).is_le()
        {
            let limit = min.go_float_string();
            result.add_internal_error(ErrorType::NumberGt, ctx, Details::Limit(&limit));
        }
        if sub.format.is_set() && !sub.format.is_format_number() {
            result.add_internal_error(ErrorType::Format, ctx, Details::Format(sub.format.name()));
        }
        result.increment_score();
        Ok(())
    }
}
