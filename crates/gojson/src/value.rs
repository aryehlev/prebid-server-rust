//! Deeply nested `interface{}` values ([`serde_json::Value`]) without deep native stacks.
//!
//! Go's goroutine stacks grow, so Go code decodes, keeps and drops documents nested as deep as
//! the decoders accept (10000 levels) on any goroutine. This crate's decoders, encoders,
//! [`mergepatch`](crate::mergepatch) and [`jsonutil`](crate::jsonutil) helpers need no native
//! stack per level, but `serde_json::Value`'s own `Drop` (and `Clone`) recurse once per level,
//! which overflows a 2 MiB thread (Tokio's default worker stack) in a debug build at a few
//! thousand nested objects. [`drop_value`] drops a value with a heap work list instead; code
//! that may hold a deep value on a small stack should drop it with it.

use core::mem;

use serde_json::Value;

/// Whether dropping `value` can recurse.
const fn is_container(value: &Value) -> bool {
    matches!(value, Value::Array(_) | Value::Object(_))
}

/// Moves the containers directly inside `value` to `pending`; everything else in it, and
/// `value` itself, is dropped here without recursion.
fn detach_children(value: Value, pending: &mut Vec<Value>) {
    match value {
        Value::Array(items) => pending.extend(items.into_iter().filter(is_container)),
        Value::Object(members) => pending.extend(
            members
                .into_iter()
                .map(|(_key, member)| member)
                .filter(is_container),
        ),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Drops `value` using a heap work list rather than the native stack, so that it is safe for a
/// value of any depth on any thread.
///
/// ```
/// use pbs_gojson::{jsonutil, value::drop_value};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let deep = format!("{}1{}", r#"{"a":"#.repeat(10_000), "}".repeat(10_000));
/// let mut v = serde_json::Value::Null;
/// jsonutil::unmarshal(deep.as_bytes(), &mut v)?;
/// drop_value(v);
/// # Ok(())
/// # }
/// ```
pub fn drop_value(value: Value) {
    let mut pending = Vec::new();
    detach_children(value, &mut pending);
    while let Some(next) = pending.pop() {
        detach_children(next, &mut pending);
    }
}

/// Replaces `*slot` with `value`, dropping the old value with [`drop_value`].
pub(crate) fn replace_value(slot: &mut Value, value: Value) {
    drop_value(mem::replace(slot, value));
}

#[cfg(test)]
mod tests {
    use std::thread;

    use serde_json::{Map, Value};

    use super::drop_value;

    /// `levels` nested objects (`{"a":{"a":...}}`) around an array of `levels` nested arrays,
    /// built without recursion.
    fn deep(levels: usize) -> Value {
        let mut value = Value::Null;
        for _ in 0..levels {
            value = Value::Array(vec![value, Value::Bool(true)]);
        }
        for _ in 0..levels {
            let members = Map::from_iter([
                ("a".to_owned(), value),
                ("b".to_owned(), Value::Array(Vec::new())),
            ]);
            value = Value::Object(members);
        }
        value
    }

    #[test]
    fn drops_deep_values_on_a_small_stack() -> Result<(), String> {
        thread::Builder::new()
            .stack_size(64 << 10)
            .spawn(|| {
                drop_value(deep(100_000));
                drop_value(Value::Null);
                drop_value(Value::String("s".to_owned()));
            })
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_panic| "the drop thread panicked".to_owned())
    }
}
