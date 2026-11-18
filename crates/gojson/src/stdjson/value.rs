//! `encoding/json` decoding into `interface{}` (`valueInterface`), iteratively.

use core::mem;

use serde_json::{Map, Number, Value};

use super::{StdDecoder, Token, unquote};
use crate::{error::JsonError, strconv::parse_float, unicode::go_lossy, value::drop_value};

/// Go's `convertNumber`: the `float64`, or `None` after saving an `UnmarshalTypeError`.
pub(crate) fn convert_number(d: &mut StdDecoder<'_>, item: &[u8]) -> Option<Value> {
    if let Ok(f) = parse_float(item, false) {
        return Some(Number::from_f64(f).map_or(Value::Null, Value::Number));
    }
    if d.saved.is_none() {
        d.save_error(JsonError::UnmarshalType {
            value: format!("number {}", go_lossy(item)),
            go_type: "float64".to_owned(),
            offset: i64::try_from(d.off).unwrap_or(i64::MAX),
            struct_name: String::new(),
            field: String::new(),
        });
    }
    None
}

/// Go's `literalInterface`.
pub(crate) fn literal_interface(d: &mut StdDecoder<'_>, item: &[u8]) -> Value {
    match item.first() {
        Some(b'n') => Value::Null,
        Some(b't') => Value::Bool(true),
        Some(b'f') => Value::Bool(false),
        Some(b'"') => Value::String(go_lossy(&unquote(item).unwrap_or_default())),
        _ => convert_number(d, item).unwrap_or(Value::Null),
    }
}

struct Open {
    value: Value,
    key: String,
}

/// Go's `valueInterface`: `[]interface{}`, `map[string]interface{}`, `float64`, `string`,
/// `bool` and nil.
pub(crate) fn value_interface(d: &mut StdDecoder<'_>) -> Value {
    // Go: encoding/json/decode.go valueInterface / arrayInterface / objectInterface
    let mut stack: Vec<Open> = Vec::new();
    loop {
        let mut done = match d.peek() {
            Token::Literal => {
                let item = d.read_literal();
                literal_interface(d, item)
            }
            Token::Array => {
                if d.array_begin() {
                    stack.push(Open {
                        value: Value::Array(Vec::new()),
                        key: String::new(),
                    });
                    continue;
                }
                Value::Array(Vec::new())
            }
            Token::Object => {
                if let Some(key) = d.object_begin() {
                    stack.push(Open {
                        value: Value::Object(Map::new()),
                        key: go_lossy(&unquote(key).unwrap_or_default()),
                    });
                    continue;
                }
                Value::Object(Map::new())
            }
        };
        loop {
            let Some(top) = stack.last_mut() else {
                return done;
            };
            let more = match &mut top.value {
                Value::Array(items) => {
                    items.push(done);
                    d.array_next()
                }
                Value::Object(map) => {
                    // A duplicate key replaces a value that can be as deep as the document.
                    if let Some(replaced) = map.insert(mem::take(&mut top.key), done) {
                        drop_value(replaced);
                    }
                    match d.object_next() {
                        Some(key) => {
                            top.key = go_lossy(&unquote(key).unwrap_or_default());
                            true
                        }
                        None => false,
                    }
                }
                Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
            };
            if more {
                break;
            }
            done = stack.pop().map_or(Value::Null, |open| open.value);
        }
    }
}
