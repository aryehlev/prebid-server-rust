//! Go: gojsonschema/draft.go.

use super::{
    go_url::parse_reference,
    json_loader::{Document, Node, NodeId},
    utils::checked_kind,
};

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
    /// keyword of every draft is accepted.
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

/// Go: gojsonschema `parseSchemaURL`: the draft named by the root `$schema`, if any.
///
/// # Errors
///
/// Go's messages for a root that is neither an object nor a boolean, and for a non-string
/// `$schema`.
pub(super) fn parse_schema_url(doc: &Document, root: NodeId) -> Result<Option<Draft>, String> {
    // Go: `isKind(documentNode, reflect.Bool)` panics on a number big.Rat rejects.
    let _root_kind = checked_kind(doc, root)?;
    match doc.node(root) {
        Node::Bool(_) => Ok(None),
        Node::Object(_) => {
            let Some(schema) = doc.get(root, "$schema") else {
                return Ok(None);
            };
            let _schema_kind = checked_kind(doc, schema)?;
            match doc.node(schema) {
                // Go: `gojsonreference.NewJsonReference(url)` then
                // `GetDraftVersion(ref.String())`.
                Node::String(url) => Ok(parse_reference(url)?.as_deref().and_then(draft_version)),
                Node::Null | Node::Bool(_) | Node::Number(_) | Node::Array(_) | Node::Object(_) => {
                    Err("$schema must be of type string".to_owned())
                }
            }
        }
        Node::Null | Node::Number(_) | Node::String(_) | Node::Array(_) => {
            Err("schema is invalid".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Draft, draft_version, parse_reference};

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
                "HTTP://json-schema.org/draft-07/schema#",
                Some(Draft::Draft7),
            ),
            ("http://json-schema.org/draft-06/schema#x", None),
            ("https://json-schema.org/draft/2020-12/schema", None),
        ];
        for (url, want) in cases {
            if parse_reference(url)?.as_deref().and_then(draft_version) != want {
                return Err(format!("{url}: want {want:?}"));
            }
        }
        Ok(())
    }
}
