//! Go: gojsonschema/jsonContext.go.

/// Go: gojsonschema `STRING_CONTEXT_ROOT`.
pub(super) const STRING_CONTEXT_ROOT: &str = "(root)";

/// Go: gojsonschema `JsonContext`: the path of the value being validated, as a linked list that
/// lives on the validation call stack.
#[derive(Debug, Clone, Copy)]
pub(super) struct JsonContext<'a> {
    head: &'a str,
    tail: Option<&'a Self>,
}

impl<'a> JsonContext<'a> {
    /// The root context, `(root)`.
    pub(super) const fn root() -> Self {
        Self {
            head: STRING_CONTEXT_ROOT,
            tail: None,
        }
    }

    /// Go: `NewJsonContext(head, tail)`.
    pub(super) const fn child(&'a self, head: &'a str) -> Self {
        Self {
            head,
            tail: Some(self),
        }
    }

    /// Go: `String()`: the heads joined with `.`.
    pub(super) fn to_go_string(self) -> String {
        let mut parts = vec![self.head];
        let mut next = self.tail;
        while let Some(ctx) = next {
            parts.push(ctx.head);
            next = ctx.tail;
        }
        parts.reverse();
        parts.join(".")
    }
}
