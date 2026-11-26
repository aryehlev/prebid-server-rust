//! Glue (Rule B (d)): the `type` keyword and the type names gojsonschema prints.
//!
//! Go: gojsonschema/types.go and gojsonschema/schemaType.go.

/// Go: gojsonschema `JSON_TYPES`, in Go's order.
pub(super) const JSON_TYPES: [&str; 7] = [
    "array", "boolean", "integer", "number", "null", "object", "string",
];

/// Go: gojsonschema `TYPE_*` constants.
pub(super) const TYPE_ARRAY: &str = "array";
pub(super) const TYPE_BOOLEAN: &str = "boolean";
pub(super) const TYPE_INTEGER: &str = "integer";
pub(super) const TYPE_NUMBER: &str = "number";
pub(super) const TYPE_NULL: &str = "null";
pub(super) const TYPE_OBJECT: &str = "object";
pub(super) const TYPE_STRING: &str = "string";

/// Go: gojsonschema `jsonSchemaType`: the `type` keyword of a schema.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct JsonSchemaType {
    types: Vec<&'static str>,
}

impl JsonSchemaType {
    /// Go: `IsTyped`.
    pub(super) const fn is_typed(&self) -> bool {
        !self.types.is_empty()
    }

    /// Go: `Add`.
    ///
    /// # Errors
    ///
    /// Go's messages for an unknown type name and for a duplicate.
    pub(super) fn add(&mut self, etype: &str) -> Result<(), String> {
        let Some(known) = JSON_TYPES.iter().find(|t| **t == etype) else {
            // Go prints JSON_TYPES with %v: "[array boolean ...]".
            return Err(format!(
                "has a primitive type that is NOT VALID -- given: /{etype}/ Expected valid values \
                 are:[{}]",
                JSON_TYPES.join(" ")
            ));
        };
        if self.contains(etype) {
            return Err(format!("{etype} type is duplicated"));
        }
        self.types.push(known);
        Ok(())
    }

    /// Go: `Contains`.
    pub(super) fn contains(&self, etype: &str) -> bool {
        self.types.contains(&etype)
    }

    /// Go: `String`: `[t1,t2]` for several types, the name for one.
    pub(super) fn to_go_string(&self) -> String {
        match self.types.as_slice() {
            [] => "undefined".to_owned(),
            [one] => (*one).to_owned(),
            many => format!("[{}]", many.join(",")),
        }
    }
}
