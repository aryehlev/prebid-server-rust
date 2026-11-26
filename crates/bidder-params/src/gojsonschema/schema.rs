//! Glue (Rule B (d)): schema loading, with gojsonschema's messages for a broken schema and a
//! loud refusal of every keyword the port does not read.
//!
//! Go: gojsonschema/schema.go (`parseSchema` and its helpers) and the parts of
//! gojsonschema/schemaLoader.go `Compile` that apply to a self-contained document. Keywords are
//! read in Go's order and fail with Go's messages; Go walks `properties` and `dependencies` in
//! random order, and the port in key order, one of Go's.

use std::iter;

use pbs_util::goregex::{self, CompileError};
use serde_json::{Map, Value};

use super::{
    decode::decode_json_using_number,
    draft::{Draft, parse_schema_url},
    errors::ValidationResult,
    format_checkers::Format,
    number::{must_be_integer, must_be_number},
    schema_type::{TYPE_BOOLEAN, TYPE_INTEGER, TYPE_NUMBER, TYPE_OBJECT, TYPE_STRING},
    sub_schema::{Additional, Dependency, Pattern, SubId, SubSchema},
    validation::{ValidateError, validate_document},
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

/// Keywords gojsonschema reads in every draft that no schema Prebid ships uses.
const UNREAD: [&str; 10] = [
    "$ref",
    "definitions",
    "patternProperties",
    "additionalItems",
    "multipleOf",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minProperties",
    "maxProperties",
    "uniqueItems",
];
/// Keywords gojsonschema reads from draft 6 on that no schema Prebid ships uses.
const UNREAD_FROM_DRAFT6: [&str; 3] = ["propertyNames", "contains", "const"];

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

/// Go: `Locale.ParseError()` for a value that is not a schema.
fn not_a_schema() -> String {
    format!("Expected: {STRING_SCHEMA}, given: Invalid JSON")
}

/// The refusal of a schema that uses `what`, which the port does not read.
fn unsupported(what: &str) -> String {
    format!(
        "{what} is not supported by the Rust port of gojsonschema, which reads only the \
         keywords Prebid's schemas use"
    )
}

/// Go: `json.Marshal` of a string: `serde_json`'s escapes, plus Go's `\u003c`, `\u003e` and
/// `\u0026` for `<`, `>` and `&`, and `\u2028` and `\u2029` for the line and paragraph
/// separators.
fn go_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in Value::from(s).to_string().chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            other => out.push(other),
        }
    }
    out
}

/// A compiled schema (Go: gojsonschema `Schema`).
#[derive(Debug, Clone)]
pub struct Schema {
    subs: Vec<SubSchema>,
    root: SubId,
}

impl Schema {
    /// Go: `gojsonschema.NewSchema(loader)` where the loader yields `document`: decodes it,
    /// takes the draft from `$schema` (`Hybrid` by default) and parses it.
    ///
    /// # Errors
    ///
    /// The decoding error, a `$schema` problem or the first `parseSchema` failure with Go's
    /// text, or the refusal of a keyword, format or schema form the port does not read.
    pub fn new(document: &[u8]) -> Result<Self, String> {
        Self::from_value(&decode_json_using_number(document)?)
    }

    /// Like [`Schema::new`] for an already decoded document.
    ///
    /// # Errors
    ///
    /// As for [`Schema::new`], less decoding errors.
    pub fn from_value(document: &Value) -> Result<Self, String> {
        let draft = parse_schema_url(document)?.unwrap_or(Draft::Hybrid);
        let mut parser = Parser {
            draft,
            subs: Vec::new(),
        };
        let root = parser.parse_schema(document, STRING_ROOT_SCHEMA_PROPERTY)?;
        Ok(Self {
            subs: parser.subs,
            root,
        })
    }

    /// Go: `(*Schema).Validate(gojsonschema.NewBytesLoader(document))`.
    ///
    /// # Errors
    ///
    /// [`ValidateError::Load`] when the document is not JSON.
    pub fn validate(&self, document: &[u8]) -> Result<ValidationResult, ValidateError> {
        let value = decode_json_using_number(document).map_err(ValidateError::Load)?;
        Ok(self.validate_value(&value))
    }

    /// Go: `(*Schema).validateDocument(root)` for an already decoded document.
    #[must_use]
    pub fn validate_value(&self, document: &Value) -> ValidationResult {
        validate_document(self, document)
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

/// The parse of one schema document, whose draft every sub-schema inherits.
struct Parser {
    draft: Draft,
    subs: Vec<SubSchema>,
}

impl Parser {
    fn push(&mut self, sub: SubSchema) -> SubId {
        self.subs.push(sub);
        SubId(self.subs.len() - 1)
    }

    /// Go: `parseSchema(documentNode, currentSchema)` for a new sub-schema found under
    /// `property`.
    fn parse_schema(&mut self, node: &Value, property: &str) -> Result<SubId, String> {
        // As of draft 6, `true` and `false` are schemas.
        if self.draft >= Draft::Draft6 && node.is_boolean() {
            return Err(unsupported("A boolean schema"));
        }
        let Value::Object(m) = node else {
            return Err(not_a_schema());
        };
        self.refuse_unread(m)?;
        let mut cur = SubSchema {
            property: property.to_owned(),
            ..SubSchema::default()
        };
        for key in ["title", "description"] {
            if m.get(key).is_some_and(|v| !v.is_string()) {
                return Err(invalid_type(TYPE_STRING, key));
            }
        }
        parse_type(m, &mut cur)?;
        self.parse_object_children(m, &mut cur)?;
        self.parse_items(m, &mut cur)?;
        for (key, limit) in [("minimum", &mut cur.minimum), ("maximum", &mut cur.maximum)] {
            if let Some(v) = m.get(key) {
                let n = must_be_number(v).ok_or_else(|| must_be_of_a(key, TYPE_NUMBER))?;
                *limit = Some(n.clone());
            }
        }
        parse_string_keywords(m, &mut cur)?;
        parse_required(m, &mut cur)?;
        cur.min_items = non_negative(m, "minItems")?;
        cur.max_items = non_negative(m, "maxItems")?;
        parse_enum(m, &mut cur)?;
        self.parse_combinators(m, &mut cur)?;
        Ok(self.push(cur))
    }

    /// Fails on the first keyword gojsonschema reads under the draft that the port does not.
    fn refuse_unread(&self, m: &Map<String, Value>) -> Result<(), String> {
        // In draft 6 `id` became `$id`; Hybrid reads `id` when it is present.
        let id = match self.draft {
            Draft::Draft4 => "id",
            Draft::Hybrid if m.contains_key("id") => "id",
            Draft::Hybrid | Draft::Draft6 | Draft::Draft7 => "$id",
        };
        let from_draft6: &[&str] = if self.draft >= Draft::Draft6 {
            &UNREAD_FROM_DRAFT6
        } else {
            &[]
        };
        let from_draft7: &[&str] = if self.draft >= Draft::Draft7 {
            &["else"]
        } else {
            &[]
        };
        let mut keys = iter::once(id)
            .chain(UNREAD)
            .chain(from_draft6.iter().copied())
            .chain(from_draft7.iter().copied());
        keys.find(|key| m.contains_key(*key))
            .map_or(Ok(()), |key| Err(unsupported(key)))
    }

    /// `properties`, `additionalProperties` and `dependencies`.
    fn parse_object_children(
        &mut self,
        m: &Map<String, Value>,
        cur: &mut SubSchema,
    ) -> Result<(), String> {
        if let Some(props) = m.get("properties") {
            let Value::Object(entries) = props else {
                return Err(must_be_of_type("properties", TYPE_OBJECT));
            };
            for (k, v) in entries {
                let child = self.parse_schema(v, k)?;
                cur.properties_children.push(child);
            }
        }
        if let Some(ap) = m.get("additionalProperties") {
            cur.additional_properties = match ap {
                Value::Bool(true) => Additional::Allowed,
                Value::Bool(false) => Additional::Forbidden,
                Value::Object(_) => {
                    Additional::Schema(self.parse_schema(ap, "additionalProperties")?)
                }
                Value::Null | Value::Number(_) | Value::String(_) | Value::Array(_) => {
                    return Err(invalid_type(
                        &format!("{TYPE_BOOLEAN}/{STRING_SCHEMA}"),
                        "additionalProperties",
                    ));
                }
            };
        }
        if let Some(deps) = m.get("dependencies") {
            let Value::Object(entries) = deps else {
                return Err(must_be_of_type("dependencies", TYPE_OBJECT));
            };
            let bad = || must_be_of_type("dependency", STRING_SCHEMA_OR_ARRAY_OF_STRINGS);
            for (k, v) in entries {
                match v {
                    Value::Array(values) => {
                        let names = values
                            .iter()
                            .map(|value| value.as_str().map(str::to_owned).ok_or_else(bad))
                            .collect::<Result<Vec<_>, _>>()?;
                        // Go registers the key inside the loop: an empty list is never stored.
                        if !names.is_empty() {
                            cur.dependencies
                                .push((k.clone(), Dependency::Properties(names)));
                        }
                    }
                    Value::Object(_) | Value::Bool(_) => {
                        let schema = self.parse_schema(v, k)?;
                        cur.dependencies
                            .push((k.clone(), Dependency::Schema(schema)));
                    }
                    Value::Null | Value::Number(_) | Value::String(_) => return Err(bad()),
                }
            }
        }
        Ok(())
    }

    /// `items`, one schema for every item; the tuple form is refused.
    fn parse_items(&mut self, m: &Map<String, Value>, cur: &mut SubSchema) -> Result<(), String> {
        match m.get("items") {
            None => Ok(()),
            Some(Value::Array(_)) => Err(unsupported("The array form of items")),
            Some(items @ (Value::Object(_) | Value::Bool(_))) => {
                cur.items = Some(self.parse_schema(items, "items")?);
                Ok(())
            }
            Some(Value::Null | Value::Number(_) | Value::String(_)) => Err(invalid_type(
                &format!("{STRING_SCHEMA}/{STRING_ARRAY_OF_SCHEMAS}"),
                "items",
            )),
        }
    }

    /// `oneOf`, `anyOf`, `allOf`, `not`, and (from draft 7) `if` and `then`.
    fn parse_combinators(
        &mut self,
        m: &Map<String, Value>,
        cur: &mut SubSchema,
    ) -> Result<(), String> {
        cur.one_of = self.schema_list(m, "oneOf", "oneOf")?;
        cur.any_of = self.schema_list(m, "anyOf", "anyOf")?;
        // Go reports a non-array allOf with the anyOf key.
        cur.all_of = self.schema_list(m, "allOf", "anyOf")?;
        cur.not = self.optional_schema(m, "not")?;
        if self.draft >= Draft::Draft7 {
            cur.if_schema = self.optional_schema(m, "if")?;
            cur.then_schema = self.optional_schema(m, "then")?;
        }
        Ok(())
    }

    /// A keyword holding an array of schemas.
    fn schema_list(
        &mut self,
        m: &Map<String, Value>,
        key: &str,
        message_key: &str,
    ) -> Result<Vec<SubId>, String> {
        let Some(list) = m.get(key) else {
            return Ok(Vec::new());
        };
        let Value::Array(items) = list else {
            return Err(must_be_of_an(message_key, "array"));
        };
        items
            .iter()
            .map(|item| self.parse_schema(item, key))
            .collect()
    }

    /// A keyword holding one schema (an object, or a boolean from draft 6).
    fn optional_schema(
        &mut self,
        m: &Map<String, Value>,
        key: &str,
    ) -> Result<Option<SubId>, String> {
        match m.get(key) {
            None => Ok(None),
            Some(v @ (Value::Object(_) | Value::Bool(_))) => Ok(Some(self.parse_schema(v, key)?)),
            Some(Value::Null | Value::Number(_) | Value::String(_) | Value::Array(_)) => {
                Err(must_be_of_an(key, TYPE_OBJECT))
            }
        }
    }
}

/// `type`.
fn parse_type(m: &Map<String, Value>, cur: &mut SubSchema) -> Result<(), String> {
    let bad = || invalid_type(&format!("{TYPE_STRING}/{STRING_ARRAY_OF_STRINGS}"), "type");
    match m.get("type") {
        None => Ok(()),
        Some(Value::String(s)) => cur.types.add(s),
        Some(Value::Array(items)) => {
            for item in items {
                match item {
                    Value::String(s) => cur.types.add(s)?,
                    // Go: a json.Number passes the reflect.String check, then the `.(string)`
                    // assertion panics while loading.
                    Value::Number(_) => {
                        return Err("panic: interface conversion: interface {} is json.Number, \
                                    not string"
                            .to_owned());
                    }
                    Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => {
                        return Err(bad());
                    }
                }
            }
            Ok(())
        }
        Some(Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_)) => Err(bad()),
    }
}

/// A non-negative integer keyword (`minLength`, `maxItems`, ...).
fn non_negative(m: &Map<String, Value>, key: &str) -> Result<Option<i64>, String> {
    let Some(v) = m.get(key) else {
        return Ok(None);
    };
    let value = must_be_integer(v).ok_or_else(|| must_be_of_an(key, TYPE_INTEGER))?;
    if value < 0 {
        return Err(format!("{key} must be greater than or equal to 0"));
    }
    Ok(Some(value))
}

/// `minLength`, `maxLength`, `pattern` and `format`.
fn parse_string_keywords(m: &Map<String, Value>, cur: &mut SubSchema) -> Result<(), String> {
    cur.min_length = non_negative(m, "minLength")?;
    cur.max_length = non_negative(m, "maxLength")?;
    if let (Some(min), Some(max)) = (cur.min_length, cur.max_length)
        && min > max
    {
        return Err("minLength cannot be greater than maxLength".to_owned());
    }
    if let Some(p) = m.get("pattern") {
        let Value::String(source) = p else {
            return Err(must_be_of_a("pattern", TYPE_STRING));
        };
        let regex = match goregex::compile(source) {
            Ok(regex) => regex,
            Err(CompileError::Syntax { .. }) => {
                return Err("pattern must be a valid regex".to_owned());
            }
            Err(other) => return Err(unsupported(&format!("The pattern {source:?} ({other})"))),
        };
        cur.pattern = Some(Pattern {
            source: source.clone(),
            regex,
        });
    }
    if let Some(f) = m.get("format") {
        let Value::String(name) = f else {
            return Err(must_be_of_type("format", TYPE_STRING));
        };
        cur.format = Format::parse(name)?;
    }
    Ok(())
}

/// `required`.
fn parse_required(m: &Map<String, Value>, cur: &mut SubSchema) -> Result<(), String> {
    let Some(r) = m.get("required") else {
        return Ok(());
    };
    let Value::Array(values) = r else {
        return Err(must_be_of_an("required", "array"));
    };
    for value in values {
        let Value::String(name) = value else {
            return Err("required items must be string".to_owned());
        };
        if cur.required.contains(name) {
            return Err("required items must be unique".to_owned());
        }
        cur.required.push(name.clone());
    }
    Ok(())
}

/// `enum`, of strings: an enum holding any other value is refused.
fn parse_enum(m: &Map<String, Value>, cur: &mut SubSchema) -> Result<(), String> {
    let Some(e) = m.get("enum") else {
        return Ok(());
    };
    let Value::Array(values) = e else {
        return Err(must_be_of_an("enum", "array"));
    };
    let mut allowed = Vec::with_capacity(values.len());
    for value in values {
        let Value::String(s) = value else {
            return Err(unsupported(&format!(
                "The enum value {value}, not a string,"
            )));
        };
        if cur.enum_values.contains(s) {
            return Err("enum items must be unique".to_owned());
        }
        cur.enum_values.push(s.clone());
        allowed.push(go_json_string(s));
    }
    cur.enum_allowed = allowed.join(", ");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pbs_devtools::repo::go_root;

    use super::{Schema, go_json_string};

    /// Keywords, formats and forms the port does not read fail loudly instead of validating
    /// differently.
    #[test]
    fn unread_keywords_are_refused() -> Result<(), String> {
        let schemas: [&str; 16] = [
            r##"{"$ref":"#/definitions/a","definitions":{"a":{}}}"##,
            r#"{"properties":{"a":{"multipleOf":2}}}"#,
            r#"{"format":"date-time"}"#,
            r#"{"properties":{"a":{"format":"email"}}}"#,
            r#"{"items":[{}]}"#,
            r#"{"enum":[1]}"#,
            r#"{"patternProperties":{"a":{}}}"#,
            r#"{"const":1}"#,
            r#"{"if":{},"else":{}}"#,
            r#"{"anyOf":[true]}"#,
            "true",
            r#"{"id":"x"}"#,
            r#"{"$id":"x"}"#,
            r#"{"$schema":"http://json-schema.org/draft-06/schema#","contains":{}}"#,
            r#"{"exclusiveMinimum":true,"minimum":1}"#,
            r#"{"uniqueItems":true}"#,
        ];
        for schema in schemas {
            match Schema::new(schema.as_bytes()) {
                Err(e) if e.contains("not supported by the Rust port") => {}
                other => return Err(format!("{schema}: {other:?}")),
            }
        }
        // Keywords Go does not read under the schema's draft, and unknown keys, are ignored.
        let ignored = [
            concat!(
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","#,
                r#""const":1,"contains":{},"if":{},"else":{},"$id":"x"}"#,
            ),
            r#"{"$schema":"http://json-schema.org/draft-06/schema#","if":{},"else":{},"id":"x"}"#,
            r#"{"default":1,"examples":[],"$comment":"x","x-anything":{}}"#,
        ];
        for schema in ignored {
            let _schema = Schema::new(schema.as_bytes()).map_err(|e| format!("{schema}: {e}"))?;
        }
        Ok(())
    }

    /// Go's messages for broken schemas.
    #[test]
    fn broken_schemas_fail_like_go() -> Result<(), String> {
        let cases = [
            (
                r#"{"type":1}"#,
                "Invalid type. Expected: string/array of strings, given: type",
            ),
            (
                r#"{"type":"foo"}"#,
                concat!(
                    "has a primitive type that is NOT VALID -- given: /foo/ ",
                    "Expected valid values are:[array boolean integer number null object string]",
                ),
            ),
            (
                r#"{"type":["string","string"]}"#,
                "string type is duplicated",
            ),
            (
                r#"{"title":1}"#,
                "Invalid type. Expected: string, given: title",
            ),
            (r#"{"properties":[]}"#, "properties must be of type object"),
            (
                r#"{"properties":{"a":1}}"#,
                "Expected: valid schema, given: Invalid JSON",
            ),
            (
                r#"{"additionalProperties":1}"#,
                "Invalid type. Expected: boolean/valid schema, given: additionalProperties",
            ),
            (
                r#"{"dependencies":{"a":1}}"#,
                "dependency must be of type schema or array of strings",
            ),
            (
                r#"{"dependencies":{"a":[1]}}"#,
                "dependency must be of type schema or array of strings",
            ),
            (
                r#"{"items":1}"#,
                "Invalid type. Expected: valid schema/array of schemas, given: items",
            ),
            (r#"{"minimum":"1"}"#, "minimum must be of a number"),
            (r#"{"minLength":1.5}"#, "minLength must be of an integer"),
            (r#"{"minLength":1.0}"#, "minLength must be of an integer"),
            (
                r#"{"maxItems":-1}"#,
                "maxItems must be greater than or equal to 0",
            ),
            (
                r#"{"minLength":2,"maxLength":1}"#,
                "minLength cannot be greater than maxLength",
            ),
            (r#"{"pattern":1}"#, "pattern must be of a string"),
            (r#"{"pattern":"("}"#, "pattern must be a valid regex"),
            (r#"{"format":1}"#, "format must be of type string"),
            (r#"{"required":"a"}"#, "required must be of an array"),
            (r#"{"required":[1]}"#, "required items must be string"),
            (r#"{"required":["a","a"]}"#, "required items must be unique"),
            (r#"{"enum":{}}"#, "enum must be of an array"),
            (r#"{"enum":["a","a"]}"#, "enum items must be unique"),
            (r#"{"allOf":{}}"#, "anyOf must be of an array"),
            (
                r#"{"oneOf":[1]}"#,
                "Expected: valid schema, given: Invalid JSON",
            ),
            (r#"{"not":1}"#, "not must be of an object"),
            (r#"{"if":1}"#, "if must be of an object"),
            (
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","not":true}"#,
                "Expected: valid schema, given: Invalid JSON",
            ),
            (
                r#"{"type":[1]}"#,
                "panic: interface conversion: interface {} is json.Number, not string",
            ),
            ("[]", "schema is invalid"),
        ];
        for (schema, want) in cases {
            let got = Schema::new(schema.as_bytes()).err();
            if got.as_deref() != Some(want) {
                return Err(format!("{schema}: got {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }

    /// Go 1.26.2's `json.Marshal` of each string.
    #[test]
    fn enum_values_print_like_go() -> Result<(), String> {
        let cases = [
            ("a", r#""a""#),
            ("<a&b>", r#""\u003ca\u0026b\u003e""#),
            ("\u{2028}\u{2029}", r#""\u2028\u2029""#),
            ("\u{8}\u{c}\n\r\t\u{1}\u{1f}", r#""\b\f\n\r\t\u0001\u001f""#),
            ("\u{7f}", "\"\u{7f}\""),
            ("\"\\/", r#""\"\\/""#),
            ("\u{e9}", "\"\u{e9}\""),
        ];
        for (value, want) in cases {
            let got = go_json_string(value);
            if got != want {
                return Err(format!("{value:?}: got {got}, want {want}"));
            }
        }
        Ok(())
    }

    /// Every schema Prebid loads with gojsonschema compiles (Go: `TestBidderParamSchemas`).
    #[test]
    fn every_prebid_schema_compiles() -> Result<(), String> {
        let root = go_root().map_err(|e| e.to_string())?;
        let dir = root.join("static").join("bidder-params");
        let mut paths =
            vec![root.join("modules/prebid/rulesengine/config/rules-engine-schema.json")];
        for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            paths.push(entry.map_err(|e| e.to_string())?.path());
        }
        for path in &paths {
            let bytes = fs::read(path).map_err(|e| e.to_string())?;
            let _schema = Schema::new(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if paths.len() < 250 {
            return Err(format!("only {} schemas", paths.len()));
        }
        Ok(())
    }
}
