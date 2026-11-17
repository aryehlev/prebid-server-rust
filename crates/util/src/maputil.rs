//! Go `util/maputil`: typed reads from decoded JSON objects (Go: `map[string]interface{}`).
//!
//! The `_mut` variants serve callers that, like the Go adapters, change the nested value in
//! place (Go maps and slices are references).

use serde_json::{Map, Value};

/// A decoded JSON object (Go: `map[string]interface{}`).
pub type JsonObject = Map<String, Value>;

/// Go: `maputil.ReadEmbeddedMap`: the object under `k`, or `None` when `k` is missing or holds
/// something else.
#[must_use]
pub fn read_embedded_map<'a>(m: &'a JsonObject, k: &str) -> Option<&'a JsonObject> {
    m.get(k).and_then(Value::as_object)
}

/// [`read_embedded_map`] for changing the object in place.
#[must_use]
pub fn read_embedded_map_mut<'a>(m: &'a mut JsonObject, k: &str) -> Option<&'a mut JsonObject> {
    m.get_mut(k).and_then(Value::as_object_mut)
}

/// Go: `maputil.ReadEmbeddedSlice`: the array under `k`, or `None` when `k` is missing or holds
/// something else.
#[must_use]
pub fn read_embedded_slice<'a>(m: &'a JsonObject, k: &str) -> Option<&'a Vec<Value>> {
    m.get(k).and_then(Value::as_array)
}

/// [`read_embedded_slice`] for changing the array in place.
#[must_use]
pub fn read_embedded_slice_mut<'a>(m: &'a mut JsonObject, k: &str) -> Option<&'a mut Vec<Value>> {
    m.get_mut(k).and_then(Value::as_array_mut)
}

/// Go: `maputil.ReadEmbeddedString`: the string under `k`, or `None` (Go: `"", false`) when
/// `k` is missing or holds something else.
#[must_use]
pub fn read_embedded_string<'a>(m: &'a JsonObject, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str)
}

/// Go: `maputil.HasElement`: whether the path of keys exists, every key but the last naming
/// an object. An empty path is never found.
#[must_use]
pub fn has_element(m: &JsonObject, keys: &[&str]) -> bool {
    let Some((last, path)) = keys.split_last() else {
        return false;
    };
    let mut current = m;
    for k in path {
        match read_embedded_map(current, k) {
            Some(next) => current = next,
            None => return false,
        }
    }
    current.contains_key(*last)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        JsonObject, has_element, read_embedded_map, read_embedded_map_mut, read_embedded_slice,
        read_embedded_slice_mut, read_embedded_string,
    };
    use crate::golden::{TestResult, check_eq};

    fn object(v: &Value) -> Result<JsonObject, String> {
        v.as_object()
            .cloned()
            .ok_or_else(|| format!("not an object: {v}"))
    }

    #[test]
    fn read_embedded_map_go_cases() -> TestResult {
        // Go: TestReadEmbeddedMap (a nil map reads like an empty one).
        let cases = [
            ("Nil", json!({}), "", None),
            ("Empty", json!({}), "foo", None),
            (
                "Success",
                json!({"foo": {"bar": 42}}),
                "foo",
                Some(json!({"bar": 42})),
            ),
            ("Not Found", json!({"foo": {"bar": 42}}), "notFound", None),
            ("Wrong Type", json!({"foo": 42}), "foo", None),
        ];
        for (description, value, key, want) in cases {
            let m = object(&value)?;
            let want = want.as_ref().map(object).transpose()?;
            check_eq(&read_embedded_map(&m, key), &want.as_ref(), description)?;
        }
        Ok(())
    }

    #[test]
    fn read_embedded_slice_go_cases() -> TestResult {
        // Go: TestReadEmbeddedSlice.
        let cases = [
            ("Nil", json!({}), "", None),
            ("Empty", json!({}), "foo", None),
            (
                "Success",
                json!({"foo": [42]}),
                "foo",
                Some(vec![json!(42)]),
            ),
            ("Not Found", json!({"foo": [42]}), "notFound", None),
            ("Wrong Type", json!({"foo": 42}), "foo", None),
        ];
        for (description, value, key, want) in cases {
            let m = object(&value)?;
            check_eq(&read_embedded_slice(&m, key), &want.as_ref(), description)?;
        }
        Ok(())
    }

    #[test]
    fn read_embedded_string_go_cases() -> TestResult {
        // Go: TestReadEmbeddedString.
        let cases = [
            ("Nil", json!({}), "", None),
            ("Empty", json!({}), "foo", None),
            (
                "Success",
                json!({"foo": "stringValue"}),
                "foo",
                Some("stringValue"),
            ),
            ("Not Found", json!({"foo": "stringValue"}), "notFound", None),
            ("Wrong Type", json!({"foo": [42]}), "foo", None),
            ("Null", json!({"foo": null}), "foo", None),
        ];
        for (description, value, key, want) in cases {
            let m = object(&value)?;
            check_eq(&read_embedded_string(&m, key), &want, description)?;
        }
        Ok(())
    }

    #[test]
    fn has_element_go_cases() -> TestResult {
        // Go: TestHasElement.
        let level1 = json!({"foo": "exists"});
        let level2 = json!({"foo": {"bar": "exists"}});
        let cases: [(&str, &Value, &[&str], bool); 11] = [
            ("Level 1 - Exists", &level1, &["foo"], true),
            (
                "Level 1 - Does Not Exist",
                &level1,
                &["doesnotexist"],
                false,
            ),
            ("Level 2 - Exists", &level2, &["foo", "bar"], true),
            (
                "Level 2 - Top Level Does Not Exist",
                &level2,
                &["doesnotexist", "bar"],
                false,
            ),
            (
                "Level 2 - Lower Level Does Not Exist",
                &level2,
                &["foo", "doesnotexist"],
                false,
            ),
            (
                "Level 2 - Does Not Exist At All",
                &level2,
                &["doesnotexist", "doesnotexist"],
                false,
            ),
            ("Keys Empty", &level1, &[], false),
            ("Map Empty", &json!({}), &["foo"], false),
            ("Nil", &json!({}), &[], false),
            ("Through A String", &level1, &["foo", "bar"], false),
            ("Null Leaf", &json!({"foo": null}), &["foo"], true),
        ];
        for (description, value, keys, want) in cases {
            let m = object(value)?;
            check_eq(&has_element(&m, keys), &want, description)?;
        }
        Ok(())
    }

    #[test]
    fn mut_variants_change_in_place() -> TestResult {
        // The adapters edit the maps they read (Go maps are references).
        let mut m = object(&json!({"video": {"w": 1}, "list": [1]}))?;
        if let Some(video) = read_embedded_map_mut(&mut m, "video") {
            let _previous = video.insert("h".to_owned(), json!(2));
        }
        if let Some(list) = read_embedded_slice_mut(&mut m, "list") {
            list.push(json!(2));
        }
        check_eq(
            &Value::Object(m),
            &json!({"video": {"w": 1, "h": 2}, "list": [1, 2]}),
            "edited",
        )
    }
}
