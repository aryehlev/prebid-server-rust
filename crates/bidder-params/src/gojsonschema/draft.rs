//! Glue (Rule B (d)): the draft a schema's `$schema` names, which decides the keywords
//! gojsonschema reads and so the errors it reports.
//!
//! Go: gojsonschema/draft.go.

use serde_json::Value;

/// Go: gojsonschema `Draft`. The order matters: Go compares drafts with `>=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Draft {
    /// `Draft4`.
    Draft4,
    /// `Draft6`.
    Draft6,
    /// `Draft7`.
    Draft7,
    /// `Hybrid` (`math.MaxInt32`): the default when `$schema` names no known draft. Every
    /// keyword of every draft is read.
    Hybrid,
}

/// Go: `drafts.GetDraftVersion(url)`.
fn draft_version(url: &str) -> Option<Draft> {
    match url {
        "http://json-schema.org/draft-04/schema" => Some(Draft::Draft4),
        "http://json-schema.org/draft-06/schema" => Some(Draft::Draft6),
        "http://json-schema.org/draft-07/schema" => Some(Draft::Draft7),
        _ => None,
    }
}

/// Go: `gojsonreference.NewJsonReference(url).String()` for a URL that parses unchanged: the
/// text without an empty fragment.
///
/// Go writes the reference back through `url.Parse` and `URL.String`, which lower-case a scheme,
/// escape and unescape bytes, and fail on some texts (a port that is not a number, a colon in a
/// first path segment). A URL made of ASCII letters, digits and `:/._~-` with at most one colon,
/// which ends a lower-case scheme, parses, and `URL.String` names a draft exactly when the text
/// less a trailing `#` does; every schema Prebid ships names its draft so. Any other text is
/// refused: what Go makes of it is not ported.
fn reference_text(url: &str) -> Result<&str, String> {
    let text = url.strip_suffix('#').unwrap_or(url);
    let plain = text
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'/' | b'.' | b'_' | b'~' | b'-'));
    let scheme_ok = text.split_once(':').is_none_or(|(scheme, rest)| {
        let mut bytes = scheme.bytes();
        !rest.contains(':')
            && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-'))
    });
    if plain && scheme_ok {
        Ok(text)
    } else {
        Err(format!(
            "$schema {url:?} is not supported by the Rust port of gojsonschema: only URLs of \
             ASCII letters, digits and :/._~- with a lower-case scheme are read"
        ))
    }
}

/// Go: gojsonschema `parseSchemaURL`: the draft named by the root `$schema`, if any.
///
/// # Errors
///
/// Go's messages for a root that is neither an object nor a boolean and for a `$schema` that
/// is not a string, and the refusal of a `$schema` URL [`reference_text`] does not read.
pub(super) fn parse_schema_url(root: &Value) -> Result<Option<Draft>, String> {
    match root {
        Value::Bool(_) => Ok(None),
        Value::Object(members) => match members.get("$schema") {
            None => Ok(None),
            Some(Value::String(url)) => Ok(draft_version(reference_text(url)?)),
            Some(
                Value::Null
                | Value::Bool(_)
                | Value::Number(_)
                | Value::Array(_)
                | Value::Object(_),
            ) => Err("$schema must be of type string".to_owned()),
        },
        Value::Null | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            Err("schema is invalid".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Draft, parse_schema_url};

    #[test]
    fn detects_drafts_like_go() -> Result<(), String> {
        let cases = [
            (
                "http://json-schema.org/draft-04/schema#",
                Some(Draft::Draft4),
            ),
            (
                "http://json-schema.org/draft-04/schema",
                Some(Draft::Draft4),
            ),
            (
                "http://json-schema.org/draft-06/schema#",
                Some(Draft::Draft6),
            ),
            (
                "http://json-schema.org/draft-07/schema#",
                Some(Draft::Draft7),
            ),
            ("https://json-schema.org/draft/2020-12/schema", None),
            ("https://json-schema.org/draft-04/schema#", None),
            ("", None),
            ("draft-04", None),
        ];
        for (url, want) in cases {
            let got = parse_schema_url(&json!({ "$schema": url }))?;
            if got != want {
                return Err(format!("{url}: got {got:?}, want {want:?}"));
            }
        }
        for url in [
            "HTTP://json-schema.org/draft-07/schema#",
            "http://json-schema.org/draft-06/schema#x",
            "http://json-schema.org/draft-04/schema?",
            "1a:b",
            ":x",
            "http://json-schema.org/%64raft-04/schema",
            "http://json-schema.org:80/draft-04/schema",
            "http://host:x/draft-04/schema",
        ] {
            if parse_schema_url(&json!({ "$schema": url })).is_ok() {
                return Err(format!("{url} is read"));
            }
        }
        let errors = [
            (json!({"$schema": 1}), "$schema must be of type string"),
            (json!([]), "schema is invalid"),
        ];
        for (root, want) in errors {
            if parse_schema_url(&root).err().as_deref() != Some(want) {
                return Err(format!("{root}: want {want}"));
            }
        }
        Ok(())
    }
}
