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

/// Go: `isKind(m[key], kinds...)`.
pub(super) fn is_kind(doc: &Document, id: NodeId, kinds: &[Kind]) -> bool {
    kinds.contains(&kind(doc.node(id)))
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
