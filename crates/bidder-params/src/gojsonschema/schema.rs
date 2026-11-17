//! Go: gojsonschema/schema.go (`parseSchema` and its helpers) and the parts of
//! gojsonschema/schemaLoader.go `Compile` that apply to a single self-contained document.
//!
//! Keywords are checked in Go's order and fail with Go's messages, so a broken schema is
//! reported the same way. Go iterates `properties`, `patternProperties`, `definitions` and
//! `dependencies` in random map order; here they keep document order (one of Go's orders, and
//! the most likely one for small maps).
//!
//! `$ref` is not supported (no bidder schema uses it); a schema containing one is rejected.

use super::{
    draft::{Draft, parse_schema_url},
    errors::ValidationResult,
    format_checkers::Format,
    go_url::parse_reference,
    json_loader::{Document, Node, NodeId, decode_json_using_number},
    marshal::marshal_without_number,
    rat::Rat,
    regexp::GoRegexp,
    schema_type::{TYPE_BOOLEAN, TYPE_INTEGER, TYPE_NUMBER, TYPE_OBJECT, TYPE_STRING},
    sub_schema::{Additional, Dependency, SubId, SubSchema},
    utils::{Kind, checked_kind, is_kind, must_be_integer, must_be_number},
    validation::{GoPanic, ValidateError, validate_document},
};

/// Go: gojsonschema `STRING_ROOT_SCHEMA_PROPERTY`.
const STRING_ROOT_SCHEMA_PROPERTY: &str = "(root)";
/// Go: gojsonschema `STRING_SCHEMA`.
const STRING_SCHEMA: &str = "valid schema";
/// Go: gojsonschema `STRING_ARRAY_OF_SCHEMAS`.
const STRING_ARRAY_OF_SCHEMAS: &str = "array of schemas";
/// Go: gojsonschema `STRING_ARRAY_OF_STRINGS`.
const STRING_ARRAY_OF_STRINGS: &str = "array of strings";
/// Go: gojsonschema `STRING_SCHEMA_OR_ARRAY_OF_STRINGS`.
const STRING_SCHEMA_OR_ARRAY_OF_STRINGS: &str = "schema or array of strings";

const MAP_OR_BOOL: &[Kind] = &[Kind::Map, Kind::Bool];

/// Go: `Locale.InvalidType()`.
fn invalid_type(expected: &str, given: &str) -> String {
    format!("Invalid type. Expected: {expected}, given: {given}")
}

/// Go: `Locale.MustBeOfType()`.
fn must_be_of_type(key: &str, type_name: &str) -> String {
    format!("{key} must be of type {type_name}")
}

/// Go: `Locale.MustBeOfA()`.
fn must_be_of_a(x: &str, y: &str) -> String {
    format!("{x} must be of a {y}")
}

/// Go: `Locale.MustBeOfAn()`.
fn must_be_of_an(x: &str, y: &str) -> String {
    format!("{x} must be of an {y}")
}

/// A compiled schema (Go: gojsonschema `Schema`).
#[derive(Debug, Clone)]
pub struct Schema {
    subs: Vec<SubSchema>,
    root: SubId,
}

impl Schema {
    /// Go: `gojsonschema.NewSchema(loader)` where the loader yields `document`: decodes it,
    /// detects the draft from `$schema` (defaulting to `Hybrid`) and parses it.
    ///
    /// # Errors
    ///
    /// Returns Go's error text: a JSON decoding error, a `$schema` problem, or the first
    /// `parseSchema` failure. Schemas using features this port does not implement (`$ref`, the
    /// formats whose Go checkers are not ported) are rejected too.
    pub fn new(document: &[u8]) -> Result<Self, String> {
        let doc = decode_json_using_number(document)?;
        Self::from_document(&doc)
    }

    /// Like [`Schema::new`] for an already decoded document.
    ///
    /// # Errors
    ///
    /// As for [`Schema::new`], minus decoding errors.
    pub fn from_document(doc: &Document) -> Result<Self, String> {
        let draft = parse_schema_url(doc, doc.root())?.unwrap_or(Draft::Hybrid);
        check_references(doc)?;
        let mut parser = Parser {
            doc,
            subs: Vec::new(),
        };
        let root = parser.parse_schema(doc.root(), STRING_ROOT_SCHEMA_PROPERTY, draft)?;
        Ok(Self {
            subs: parser.subs,
            root,
        })
    }

    /// Go: `(*Schema).Validate(gojsonschema.NewBytesLoader(document))`.
    ///
    /// # Errors
    ///
    /// [`ValidateError::Load`] when the document is not JSON (Go's decoder text), and
    /// [`ValidateError::Panic`] where Go would panic.
    pub fn validate(&self, document: &[u8]) -> Result<ValidationResult, ValidateError> {
        let doc = decode_json_using_number(document).map_err(ValidateError::Load)?;
        validate_document(self, &doc).map_err(ValidateError::Panic)
    }

    /// Go: `(*Schema).validateDocument(root)` for an already decoded document.
    ///
    /// # Errors
    ///
    /// Where Go would panic.
    pub fn validate_document(&self, doc: &Document) -> Result<ValidationResult, GoPanic> {
        validate_document(self, doc)
    }

    /// The root sub-schema.
    pub(super) const fn root(&self) -> SubId {
        self.root
    }

    /// The sub-schema behind `id` (ids always come from this schema).
    pub(super) fn sub(&self, id: SubId) -> Option<&SubSchema> {
        self.subs.get(id.0)
    }
}

/// Go: `schemaPool.parseReferences`, which walks the whole schema document before it is parsed
/// (gojsonschema/schemaPool.go). Its errors are swallowed, so for a self-contained schema the
/// only observable effect is a panic: `isKind` on an `id`/`$id` or `$ref` value that is a
/// number `big.Rat` rejects. The walk skips `const` and `enum` values and, under `properties`,
/// `dependencies` and `patternProperties`, descends only into an object's values.
///
/// Not ported: Go stops walking an object whose `id` resolves to an id it already saw
/// ("Reference already exists"), which needs Go's URL resolution; such a subtree is walked here.
fn check_references(doc: &Document) -> Result<(), String> {
    let mut stack = vec![doc.root()];
    while let Some(id) = stack.pop() {
        match doc.node(id) {
            Node::Array(items) => stack.extend(items.iter().copied()),
            Node::Object(entries) => {
                let key_id = if doc.get(id, "id").is_some() {
                    "id"
                } else {
                    "$id"
                };
                for key in [key_id, "$ref"] {
                    if let Some(value) = doc.get(id, key) {
                        let _kind = checked_kind(doc, value)?;
                    }
                }
                for (key, value) in entries {
                    match key.as_str() {
                        "const" | "enum" => {}
                        "properties" | "dependencies" | "patternProperties" => {
                            if let Node::Object(children) = doc.node(*value) {
                                stack.extend(children.iter().map(|&(_, child)| child));
                            }
                        }
                        _ => stack.push(*value),
                    }
                }
            }
            Node::Null | Node::Bool(_) | Node::Number(_) | Node::String(_) => {}
        }
    }
    Ok(())
}

struct Parser<'d> {
    doc: &'d Document,
    subs: Vec<SubSchema>,
}

impl<'d> Parser<'d> {
    fn push(&mut self, sub: SubSchema) -> SubId {
        self.subs.push(sub);
        SubId(self.subs.len() - 1)
    }

    fn node(&self, id: NodeId) -> &'d Node {
        self.doc.node(id)
    }

    fn get(&self, obj: NodeId, key: &str) -> Option<NodeId> {
        self.doc.get(obj, key)
    }

    /// Go: `isKind(value, kinds...)`, which panics on a number `big.Rat` rejects.
    fn is(&self, id: NodeId, kinds: &[Kind]) -> Result<bool, String> {
        is_kind(self.doc, id, kinds)
    }

    /// The panic of a Go `isKind` call on `id` whose result the caller works out itself.
    fn check(&self, id: NodeId) -> Result<(), String> {
        checked_kind(self.doc, id).map(|_| ())
    }

    /// Go: `parseSchema(documentNode, currentSchema)` for a new sub-schema found under
    /// `property`, inheriting `draft` from its parent.
    fn parse_schema(
        &mut self,
        node: NodeId,
        property: &str,
        draft: Draft,
    ) -> Result<SubId, String> {
        let mut cur = SubSchema {
            property: property.to_owned(),
            ..SubSchema::default()
        };
        // Go: the first isKind on the node (Bool for draft 6 and later, Map otherwise) panics on
        // a number big.Rat rejects.
        self.check(node)?;
        // As of draft 6 "true" is equivalent to an empty schema "{}" and false equals
        // {"not":{}}.
        if draft >= Draft::Draft6
            && let Node::Bool(b) = self.node(node)
        {
            cur.pass = Some(*b);
            return Ok(self.push(cur));
        }
        if !self.is(node, &[Kind::Map])? {
            return Err(format!("Expected: {STRING_SCHEMA}, given: Invalid JSON"));
        }
        self.parse_meta(node, draft)?;
        self.parse_type(node, &mut cur)?;
        self.parse_object_children(node, draft, &mut cur)?;
        self.parse_items(node, draft, &mut cur)?;
        self.parse_numeric(node, draft, &mut cur)?;
        self.parse_string_keywords(node, &mut cur)?;
        self.parse_object_keywords(node, &mut cur)?;
        self.parse_array_keywords(node, draft, &mut cur)?;
        self.parse_values(node, draft, &mut cur)?;
        self.parse_combinators(node, draft, &mut cur)?;
        Ok(self.push(cur))
    }

    /// `id`/`$id`, `definitions`, `title`, `description` and `$ref`.
    fn parse_meta(&mut self, node: NodeId, draft: Draft) -> Result<(), String> {
        // In draft 6 the id keyword was renamed to $id; Hybrid uses the old id when present.
        let key_id = match draft {
            Draft::Draft4 => "id",
            Draft::Hybrid if self.get(node, "id").is_some() => "id",
            Draft::Hybrid | Draft::Draft6 | Draft::Draft7 => "$id",
        };
        if let Some(id) = self.get(node, key_id) {
            self.check(id)?;
            let Node::String(reference) = self.node(id) else {
                return Err(invalid_type(TYPE_STRING, key_id));
            };
            // Go: `gojsonreference.NewJsonReference(id)`.
            let _reference = parse_reference(reference)?;
        }
        if let Some(defs) = self.get(node, "definitions") {
            self.check(defs)?;
            let entries = match self.node(defs) {
                Node::Object(entries) => entries,
                // Go: a boolean passes the kind check and then fails the map assertion.
                Node::Bool(_) => {
                    return Err("panic: interface conversion: interface {} is bool, not \
                                map[string]interface {}"
                        .to_owned());
                }
                Node::Null | Node::Number(_) | Node::String(_) | Node::Array(_) => {
                    return Err(invalid_type(STRING_ARRAY_OF_SCHEMAS, "definitions"));
                }
            };
            for &(_, dv) in entries {
                if !self.is(dv, MAP_OR_BOOL)? {
                    return Err(invalid_type(STRING_ARRAY_OF_SCHEMAS, "definitions"));
                }
                let _definition = self.parse_schema(dv, "definitions", draft)?;
            }
        }
        for key in ["title", "description", "$ref"] {
            if let Some(v) = self.get(node, key)
                && !self.is(v, &[Kind::String])?
            {
                return Err(invalid_type(TYPE_STRING, key));
            }
        }
        if let Some(Node::String(reference)) = self.get(node, "$ref").map(|r| self.node(r)) {
            let _reference = parse_reference(reference)?;
            return Err("$ref is not supported by the Rust port of gojsonschema".to_owned());
        }
        Ok(())
    }

    /// `type`.
    fn parse_type(&self, node: NodeId, cur: &mut SubSchema) -> Result<(), String> {
        let Some(t) = self.get(node, "type") else {
            return Ok(());
        };
        let bad = || invalid_type(&format!("{TYPE_STRING}/{STRING_ARRAY_OF_STRINGS}"), "type");
        self.check(t)?;
        match self.node(t) {
            Node::String(s) => cur.types.add(s),
            Node::Array(items) => {
                for &item in items {
                    match self.node(item) {
                        Node::String(s) => cur.types.add(s)?,
                        // Go: a json.Number passes the reflect.String check, then the
                        // `.(string)` assertion panics while loading.
                        Node::Number(_) => {
                            return Err("panic: interface conversion: interface {} is \
                                        json.Number, not string"
                                .to_owned());
                        }
                        Node::Null | Node::Bool(_) | Node::Array(_) | Node::Object(_) => {
                            return Err(bad());
                        }
                    }
                }
                Ok(())
            }
            Node::Null | Node::Bool(_) | Node::Number(_) | Node::Object(_) => Err(bad()),
        }
    }

    /// `properties`, `additionalProperties`, `patternProperties`, `propertyNames` and
    /// `dependencies`.
    fn parse_object_children(
        &mut self,
        node: NodeId,
        draft: Draft,
        cur: &mut SubSchema,
    ) -> Result<(), String> {
        if let Some(props) = self.get(node, "properties") {
            self.check(props)?;
            let Node::Object(entries) = self.node(props) else {
                return Err(must_be_of_type("properties", TYPE_OBJECT));
            };
            for (k, v) in entries {
                let child = self.parse_schema(*v, k, draft)?;
                cur.properties_children.push(child);
            }
        }
        if let Some(ap) = self.get(node, "additionalProperties") {
            self.check(ap)?;
            cur.additional_properties = match self.node(ap) {
                Node::Bool(b) => Additional::from_bool(*b),
                Node::Object(_) => {
                    Additional::Schema(self.parse_schema(ap, "additionalProperties", draft)?)
                }
                Node::Null | Node::Number(_) | Node::String(_) | Node::Array(_) => {
                    return Err(invalid_type(
                        &format!("{TYPE_BOOLEAN}/{STRING_SCHEMA}"),
                        "additionalProperties",
                    ));
                }
            };
        }
        if let Some(pp) = self.get(node, "patternProperties") {
            self.check(pp)?;
            let Node::Object(entries) = self.node(pp) else {
                return Err(invalid_type(STRING_SCHEMA, "patternProperties"));
            };
            for (k, v) in entries {
                // Go: `regexp.MatchString(k, "")` only to detect a compile error.
                let Ok(rx) = GoRegexp::compile(k) else {
                    return Err(format!("Invalid regex pattern '{k}'"));
                };
                let child = self.parse_schema(*v, k, draft)?;
                cur.pattern_properties.push((rx, child));
            }
        }
        if draft >= Draft::Draft6
            && let Some(pn) = self.get(node, "propertyNames")
        {
            if !self.is(pn, MAP_OR_BOOL)? {
                return Err(invalid_type(STRING_SCHEMA, "patternProperties"));
            }
            cur.property_names = Some(self.parse_schema(pn, "propertyNames", draft)?);
        }
        if let Some(deps) = self.get(node, "dependencies") {
            self.parse_dependencies(deps, draft, cur)?;
        }
        Ok(())
    }

    /// Go: `parseDependencies`.
    fn parse_dependencies(
        &mut self,
        deps: NodeId,
        draft: Draft,
        cur: &mut SubSchema,
    ) -> Result<(), String> {
        self.check(deps)?;
        let Node::Object(entries) = self.node(deps) else {
            return Err(must_be_of_type("dependencies", TYPE_OBJECT));
        };
        let bad = || must_be_of_type("dependency", STRING_SCHEMA_OR_ARRAY_OF_STRINGS);
        for (k, v) in entries {
            // Go switches on reflect.ValueOf(v).Kind() here, not isKind: a number never panics.
            match self.node(*v) {
                Node::Array(values) => {
                    let mut names = Vec::new();
                    for &value in values {
                        self.check(value)?;
                        let Node::String(name) = self.node(value) else {
                            return Err(bad());
                        };
                        names.push(name.clone());
                    }
                    // Go registers the key inside the loop: an empty list is never stored.
                    if !names.is_empty() {
                        cur.dependencies
                            .push((k.clone(), Dependency::Properties(names)));
                    }
                }
                Node::Object(_) | Node::Bool(_) => {
                    let schema = self.parse_schema(*v, k, draft)?;
                    cur.dependencies
                        .push((k.clone(), Dependency::Schema(schema)));
                }
                Node::Null | Node::Number(_) | Node::String(_) => return Err(bad()),
            }
        }
        Ok(())
    }

    /// `items` and `additionalItems`.
    fn parse_items(
        &mut self,
        node: NodeId,
        draft: Draft,
        cur: &mut SubSchema,
    ) -> Result<(), String> {
        let bad_items = || {
            invalid_type(
                &format!("{STRING_SCHEMA}/{STRING_ARRAY_OF_SCHEMAS}"),
                "items",
            )
        };
        if let Some(items) = self.get(node, "items") {
            self.check(items)?;
            match self.node(items) {
                Node::Array(elements) => {
                    for &element in elements {
                        if !self.is(element, MAP_OR_BOOL)? {
                            return Err(bad_items());
                        }
                        cur.items_children
                            .push(self.parse_schema(element, "items", draft)?);
                        cur.items_children_is_single_schema = false;
                    }
                }
                Node::Object(_) | Node::Bool(_) => {
                    cur.items_children
                        .push(self.parse_schema(items, "items", draft)?);
                    cur.items_children_is_single_schema = true;
                }
                Node::Null | Node::Number(_) | Node::String(_) => return Err(bad_items()),
            }
        }
        if let Some(ai) = self.get(node, "additionalItems") {
            self.check(ai)?;
            cur.additional_items = match self.node(ai) {
                Node::Bool(b) => Additional::from_bool(*b),
                Node::Object(_) => {
                    Additional::Schema(self.parse_schema(ai, "additionalItems", draft)?)
                }
                Node::Null | Node::Number(_) | Node::String(_) | Node::Array(_) => {
                    return Err(invalid_type(
                        &format!("{TYPE_BOOLEAN}/{STRING_SCHEMA}"),
                        "additionalItems",
                    ));
                }
            };
        }
        Ok(())
    }

    /// `multipleOf`, `minimum`, `exclusiveMinimum`, `maximum` and `exclusiveMaximum`.
    fn parse_numeric(&self, node: NodeId, draft: Draft, cur: &mut SubSchema) -> Result<(), String> {
        if let Some(m) = self.get(node, "multipleOf") {
            let value = must_be_number(self.node(m))
                .ok_or_else(|| invalid_type(TYPE_NUMBER, "multipleOf"))?;
            if value.is_zero() || value.is_negative() {
                return Err("multipleOf must be strictly greater than 0".to_owned());
            }
            cur.multiple_of = Some(value);
        }
        if let Some(m) = self.get(node, "minimum") {
            let value =
                must_be_number(self.node(m)).ok_or_else(|| must_be_of_a("minimum", TYPE_NUMBER))?;
            cur.minimum = Some(value);
        }
        if let Some(e) = self.get(node, "exclusiveMinimum") {
            let limits = self.exclusive_limit(e, draft, cur.minimum.take(), "exclusiveMinimum")?;
            (cur.minimum, cur.exclusive_minimum) = limits;
        }
        if let Some(m) = self.get(node, "maximum") {
            let value =
                must_be_number(self.node(m)).ok_or_else(|| must_be_of_a("maximum", TYPE_NUMBER))?;
            cur.maximum = Some(value);
        }
        if let Some(e) = self.get(node, "exclusiveMaximum") {
            let limits = self.exclusive_limit(e, draft, cur.maximum.take(), "exclusiveMaximum")?;
            (cur.maximum, cur.exclusive_maximum) = limits;
        }
        Ok(())
    }

    /// Go: the `exclusiveMinimum`/`exclusiveMaximum` switch on the draft. Returns the new
    /// (inclusive, exclusive) limits.
    fn exclusive_limit(
        &self,
        e: NodeId,
        draft: Draft,
        inclusive: Option<Rat>,
        key: &str,
    ) -> Result<(Option<Rat>, Option<Rat>), String> {
        let limit_key = if key == "exclusiveMinimum" {
            "minimum"
        } else {
            "maximum"
        };
        let boolean = |exclusive: bool, limit: Option<Rat>| {
            let value = limit.ok_or_else(|| format!("{key} cannot be used without {limit_key}"))?;
            Ok(if exclusive {
                (None, Some(value))
            } else {
                (Some(value), None)
            })
        };
        if matches!(draft, Draft::Draft4 | Draft::Hybrid) {
            // Go: isKind(value, reflect.Bool); drafts 6 and 7 only call isJSONNumber.
            self.check(e)?;
        }
        match (draft, self.node(e)) {
            (Draft::Draft4 | Draft::Hybrid, Node::Bool(b)) => boolean(*b, inclusive),
            (Draft::Draft4, _) => Err(invalid_type(TYPE_BOOLEAN, key)),
            (Draft::Hybrid | Draft::Draft6 | Draft::Draft7, n @ Node::Number(_)) => {
                Ok((inclusive, must_be_number(n)))
            }
            (Draft::Hybrid, _) => Err(invalid_type(&format!("{TYPE_BOOLEAN}/{TYPE_NUMBER}"), key)),
            (Draft::Draft6 | Draft::Draft7, _) => Err(invalid_type(TYPE_NUMBER, key)),
        }
    }

    /// A non-negative integer keyword (`minLength`, `maxItems`, ...).
    fn non_negative(&self, node: NodeId, key: &str) -> Result<Option<i64>, String> {
        let Some(v) = self.get(node, key) else {
            return Ok(None);
        };
        let value =
            must_be_integer(self.node(v)).ok_or_else(|| must_be_of_an(key, TYPE_INTEGER))?;
        if value < 0 {
            return Err(format!("{key} must be greater than or equal to 0"));
        }
        Ok(Some(value))
    }

    /// `minLength`, `maxLength`, `pattern` and `format`.
    fn parse_string_keywords(&self, node: NodeId, cur: &mut SubSchema) -> Result<(), String> {
        cur.min_length = self.non_negative(node, "minLength")?;
        cur.max_length = self.non_negative(node, "maxLength")?;
        if let (Some(min), Some(max)) = (cur.min_length, cur.max_length)
            && min > max
        {
            return Err("minLength cannot be greater than maxLength".to_owned());
        }
        if let Some(p) = self.get(node, "pattern") {
            self.check(p)?;
            let Node::String(source) = self.node(p) else {
                return Err(must_be_of_a("pattern", TYPE_STRING));
            };
            let Ok(rx) = GoRegexp::compile(source) else {
                return Err("pattern must be a valid regex".to_owned());
            };
            cur.pattern = Some(rx);
        }
        if let Some(f) = self.get(node, "format") {
            let Node::String(name) = self.node(f) else {
                return Err(must_be_of_type("format", TYPE_STRING));
            };
            cur.format = Format::parse(name)?;
        }
        Ok(())
    }

    /// `minProperties`, `maxProperties` and `required`.
    fn parse_object_keywords(&self, node: NodeId, cur: &mut SubSchema) -> Result<(), String> {
        cur.min_properties = self.non_negative(node, "minProperties")?;
        cur.max_properties = self.non_negative(node, "maxProperties")?;
        if let (Some(min), Some(max)) = (cur.min_properties, cur.max_properties)
            && min > max
        {
            return Err("minProperties cannot be greater than maxProperties".to_owned());
        }
        if let Some(r) = self.get(node, "required") {
            self.check(r)?;
            let Node::Array(values) = self.node(r) else {
                return Err(must_be_of_an("required", "array"));
            };
            for &value in values {
                self.check(value)?;
                let Node::String(name) = self.node(value) else {
                    return Err("required items must be string".to_owned());
                };
                if cur.required.contains(name) {
                    return Err("required items must be unique".to_owned());
                }
                cur.required.push(name.clone());
            }
        }
        Ok(())
    }

    /// `minItems`, `maxItems`, `uniqueItems` and `contains`.
    fn parse_array_keywords(
        &mut self,
        node: NodeId,
        draft: Draft,
        cur: &mut SubSchema,
    ) -> Result<(), String> {
        cur.min_items = self.non_negative(node, "minItems")?;
        cur.max_items = self.non_negative(node, "maxItems")?;
        if let Some(u) = self.get(node, "uniqueItems") {
            self.check(u)?;
            let Node::Bool(b) = self.node(u) else {
                return Err(must_be_of_a("uniqueItems", TYPE_BOOLEAN));
            };
            cur.unique_items = *b;
        }
        if draft >= Draft::Draft6
            && let Some(c) = self.get(node, "contains")
        {
            cur.contains = Some(self.parse_schema(c, "contains", draft)?);
        }
        Ok(())
    }

    /// `const` and `enum`.
    fn parse_values(&self, node: NodeId, draft: Draft, cur: &mut SubSchema) -> Result<(), String> {
        if draft >= Draft::Draft6
            && let Some(c) = self.get(node, "const")
        {
            cur.const_value = Some(marshal_without_number(self.doc, c)?);
        }
        if let Some(e) = self.get(node, "enum") {
            self.check(e)?;
            let Node::Array(values) = self.node(e) else {
                return Err(must_be_of_an("enum", "array"));
            };
            for &value in values {
                let text = marshal_without_number(self.doc, value)?;
                if cur.enum_values.contains(&text) {
                    return Err("enum items must be unique".to_owned());
                }
                cur.enum_values.push(text);
            }
        }
        Ok(())
    }

    /// `oneOf`, `anyOf`, `allOf`, `not`, and (draft 7 and hybrid) `if`, `then`, `else`.
    fn parse_combinators(
        &mut self,
        node: NodeId,
        draft: Draft,
        cur: &mut SubSchema,
    ) -> Result<(), String> {
        cur.one_of = self.schema_list(node, "oneOf", "oneOf", draft)?;
        cur.any_of = self.schema_list(node, "anyOf", "anyOf", draft)?;
        // Go reports a non-array allOf with the anyOf key.
        cur.all_of = self.schema_list(node, "allOf", "anyOf", draft)?;
        cur.not = self.optional_schema(node, "not", draft)?;
        if draft >= Draft::Draft7 {
            cur.if_schema = self.optional_schema(node, "if", draft)?;
            cur.then_schema = self.optional_schema(node, "then", draft)?;
            cur.else_schema = self.optional_schema(node, "else", draft)?;
        }
        Ok(())
    }

    /// A keyword holding an array of schemas.
    fn schema_list(
        &mut self,
        node: NodeId,
        key: &str,
        message_key: &str,
        draft: Draft,
    ) -> Result<Vec<SubId>, String> {
        let Some(list) = self.get(node, key) else {
            return Ok(Vec::new());
        };
        self.check(list)?;
        let Node::Array(items) = self.node(list) else {
            return Err(must_be_of_an(message_key, "array"));
        };
        let mut children = Vec::with_capacity(items.len());
        for &item in items {
            children.push(self.parse_schema(item, key, draft)?);
        }
        Ok(children)
    }

    /// A keyword holding one schema (object or boolean).
    fn optional_schema(
        &mut self,
        node: NodeId,
        key: &str,
        draft: Draft,
    ) -> Result<Option<SubId>, String> {
        let Some(v) = self.get(node, key) else {
            return Ok(None);
        };
        if !self.is(v, MAP_OR_BOOL)? {
            return Err(must_be_of_an(key, TYPE_OBJECT));
        }
        Ok(Some(self.parse_schema(v, key, draft)?))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::Schema;

    /// Features this port does not implement fail loudly instead of validating differently.
    #[test]
    fn unported_features_are_rejected() -> Result<(), String> {
        let schemas: [&[u8]; 4] = [
            br##"{"$ref":"#/definitions/a","definitions":{"a":{}}}"##,
            br#"{"properties":{"a":{"$ref":"http://example.com/s.json"}}}"#,
            br#"{"format":"date-time"}"#,
            br#"{"properties":{"a":{"format":"email"}}}"#,
        ];
        for schema in schemas {
            match Schema::new(schema) {
                Err(e) if e.contains("not supported by the Rust port") => {}
                other => {
                    return Err(format!("{}: {other:?}", String::from_utf8_lossy(schema)));
                }
            }
        }
        Ok(())
    }

    /// Go's `isKind` dereferences nil on a number `big.Rat` rejects, both while parsing and in
    /// the `parseReferences` walk over every `id`/`$id`/`$ref` (outside `const` and `enum`).
    /// Expected outcomes are gojsonschema v1.2.0's under Go 1.26.2.
    #[test]
    fn unparsable_numbers_panic_where_go_panics() -> Result<(), String> {
        let panic = "panic: runtime error: invalid memory address or nil pointer dereference";
        let float = "json: cannot unmarshal number 1e1000001 into Go value of type float64";
        let cases: [(&str, Option<&str>); 18] = [
            (r#"{"title":1e1000001}"#, Some(panic)),
            (r#"{"type":1e1000001}"#, Some(panic)),
            (r#"{"x":{"$ref":1e1000001}}"#, Some(panic)),
            (r#"{"x":[{"id":1e1000001}]}"#, Some(panic)),
            (r#"{"x":{"$id":1e1000001}}"#, Some(panic)),
            (r#"{"x":{"id":"a","$id":1e1000001}}"#, None),
            (r#"{"enum":[{"$ref":1e1000001}]}"#, Some(float)),
            (r#"{"const":{"id":1e1000001}}"#, Some(float)),
            (
                r#"{"properties":{"$ref":{"title":"x"}},"x":{"properties":{"$ref":1e1000001}}}"#,
                None,
            ),
            (
                r#"{"dependencies":{"a":1e1000001}}"#,
                Some("dependency must be of type schema or array of strings"),
            ),
            (r#"{"exclusiveMinimum":1e1000001}"#, Some(panic)),
            (
                concat!(
                    r#"{"$schema":"http://json-schema.org/draft-07/schema#","#,
                    r#""exclusiveMinimum":1e1000001}"#
                ),
                None,
            ),
            (
                concat!(
                    r#"{"$schema":"http://json-schema.org/draft-04/schema#","#,
                    r#""exclusiveMinimum":1e1000001}"#
                ),
                Some(panic),
            ),
            (r#"{"required":[1e1000001]}"#, Some(panic)),
            (r#"{"items":[1e1000001]}"#, Some(panic)),
            ("1e1000001", Some(panic)),
            (r#"{"$schema":1e1000001}"#, Some(panic)),
            (
                r#"{"minimum":1e1000001}"#,
                Some("minimum must be of a number"),
            ),
        ];
        for (schema, want) in cases {
            let got = Schema::new(schema.as_bytes()).err();
            if got.as_deref() != want {
                return Err(format!("{schema}: got {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }

    /// Every bidder schema in the repository compiles (Go: `TestBidderParamSchemas`).
    #[test]
    fn every_bidder_schema_compiles() -> Result<(), String> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../static/bidder-params");
        let mut count = 0usize;
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let bytes = fs::read(&path).map_err(|e| e.to_string())?;
            let _schema = Schema::new(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            count += 1;
        }
        if count < 250 {
            return Err(format!("only {count} schemas in {dir}"));
        }
        Ok(())
    }
}
