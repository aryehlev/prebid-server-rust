//! Go: gojsonschema/utils.go.

use super::{
    json_loader::{Document, Node, NodeId},
    rat::Rat,
};

/// Go: `reflect.Kind` of a decoded value as gojsonschema's `isKind` sees it (a `json.Number`
/// is converted to a `big.Rat`, which is a struct).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// `nil` (`reflect.Invalid`).
    Invalid,
    /// `bool`.
    Bool,
    /// `json.Number` (as `big.Rat`).
    Struct,
    /// `string`.
    String,
    /// `[]interface{}`.
    Slice,
    /// `map[string]interface{}`.
    Map,
}

/// Go: `isKind(what, kinds...)` for a single kind lookup.
pub(super) const fn kind(node: &Node) -> Kind {
    match node {
        Node::Null => Kind::Invalid,
        Node::Bool(_) => Kind::Bool,
        Node::Number(_) => Kind::Struct,
        Node::String(_) => Kind::String,
        Node::Array(_) => Kind::Slice,
        Node::Object(_) => Kind::Map,
    }
}

/// Go's panic text when `isKind` meets a `json.Number` that `big.Rat` rejects (such as
/// `1e1000001`): `*mustBeNumber(what)` dereferences nil. Load errors with a `panic: ` prefix
/// stand for a Go panic.
pub(super) const IS_KIND_PANIC: &str =
    "panic: runtime error: invalid memory address or nil pointer dereference";

/// Go: the kind `isKind(what, ...)` computes for `what`.
///
/// # Errors
///
/// [`IS_KIND_PANIC`] where Go panics: `what` is a number `big.Rat` cannot parse.
pub(super) fn checked_kind(doc: &Document, id: NodeId) -> Result<Kind, String> {
    let node = doc.node(id);
    if let Node::Number(literal) = node
        && Rat::parse(literal).is_none()
    {
        return Err(IS_KIND_PANIC.to_owned());
    }
    Ok(kind(node))
}

/// Go: `isKind(m[key], kinds...)`.
///
/// # Errors
///
/// [`IS_KIND_PANIC`] where Go panics (see [`checked_kind`]).
pub(super) fn is_kind(doc: &Document, id: NodeId, kinds: &[Kind]) -> Result<bool, String> {
    Ok(kinds.contains(&checked_kind(doc, id)?))
}

/// Go: `checkJSONInteger`.
pub(super) fn check_json_integer(literal: &str) -> bool {
    Rat::parse(literal).is_some_and(|r| r.is_int())
}

/// Go: `mustBeNumber`.
pub(super) fn must_be_number(node: &Node) -> Option<Rat> {
    match node {
        Node::Number(literal) => Rat::parse(literal),
        Node::Null | Node::Bool(_) | Node::String(_) | Node::Array(_) | Node::Object(_) => None,
    }
}

/// Go: `mustBeInteger`: an integral `json.Number` that `Int64()` (`strconv.ParseInt`) accepts.
pub(super) fn must_be_integer(node: &Node) -> Option<i64> {
    match node {
        Node::Number(literal) if check_json_integer(literal) => literal.parse().ok(),
        Node::Null
        | Node::Bool(_)
        | Node::Number(_)
        | Node::String(_)
        | Node::Array(_)
        | Node::Object(_) => None,
    }
}
