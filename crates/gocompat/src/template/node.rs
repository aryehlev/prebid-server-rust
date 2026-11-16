//! Go: `text/template/parse/node.go`: the parse tree and its `String` forms, which Go prints in
//! parse and exec error messages.

use crate::{
    strconv::{parse_float, parse_int, parse_uint, quote, unquote_char},
    strings::trim_space,
};

/// Go: `ListNode`.
#[derive(Debug, Clone, Default)]
pub(super) struct List {
    pub(super) pos: usize,
    pub(super) nodes: Vec<Node>,
}

/// Go: `BranchNode` (`IfNode`, `RangeNode`, `WithNode`).
#[derive(Debug, Clone)]
pub(super) struct Branch {
    pub(super) pos: usize,
    pub(super) pipe: Pipe,
    pub(super) list: List,
    pub(super) else_list: Option<List>,
}

/// Go: the nodes of a template body.
#[derive(Debug, Clone)]
pub(super) enum Node {
    Text {
        pos: usize,
        text: String,
    },
    Action {
        pos: usize,
        pipe: Pipe,
    },
    If(Branch),
    Range(Branch),
    With(Branch),
    Template {
        pos: usize,
        name: String,
        pipe: Option<Pipe>,
    },
    Break {
        pos: usize,
    },
    Continue {
        pos: usize,
    },
    /// Go: `endNode`, returned by the parser but never added to a tree.
    End {
        pos: usize,
    },
    /// Go: `elseNode`, returned by the parser but never added to a tree.
    Else {
        pos: usize,
    },
}

/// A declared variable: Go's `VariableNode` in `PipeNode.Decl`.
#[derive(Debug, Clone)]
pub(super) struct Decl {
    pub(super) name: String,
}

/// Go: `PipeNode`.
#[derive(Debug, Clone, Default)]
pub(super) struct Pipe {
    pub(super) pos: usize,
    pub(super) is_assign: bool,
    pub(super) decl: Vec<Decl>,
    pub(super) cmds: Vec<Command>,
}

/// Go: `CommandNode` (its position is never reported).
#[derive(Debug, Clone)]
pub(super) struct Command {
    pub(super) args: Vec<Arg>,
}

/// Go: `NumberNode`; `Some` stands for Go's `IsInt`/`IsUint`/`IsFloat`.
#[derive(Debug, Clone, Default)]
pub(super) struct Number {
    pub(super) pos: usize,
    pub(super) int64: Option<i64>,
    pub(super) uint64: Option<u64>,
    pub(super) float64: Option<f64>,
    pub(super) is_complex: bool,
    pub(super) text: String,
}

/// Go: the operand nodes of a command.
#[derive(Debug, Clone)]
pub(super) enum Arg {
    Identifier {
        pos: usize,
        ident: String,
    },
    Variable {
        pos: usize,
        ident: Vec<String>,
    },
    Dot {
        pos: usize,
    },
    Nil {
        pos: usize,
    },
    Field {
        pos: usize,
        ident: Vec<String>,
    },
    Chain {
        pos: usize,
        node: Box<Self>,
        field: Vec<String>,
    },
    Bool {
        pos: usize,
        value: bool,
    },
    Number(Number),
    String {
        pos: usize,
        quoted: String,
        text: String,
    },
    Pipe(Box<Pipe>),
}

impl List {
    /// Go: `ListNode.String`.
    pub(super) fn string(&self) -> String {
        let mut sb = String::new();
        self.write_to(&mut sb);
        sb
    }

    fn write_to(&self, sb: &mut String) {
        for n in &self.nodes {
            n.write_to(sb);
        }
    }
}

impl Node {
    pub(super) const fn pos(&self) -> usize {
        match self {
            Self::Text { pos, .. }
            | Self::Action { pos, .. }
            | Self::Template { pos, .. }
            | Self::Break { pos }
            | Self::Continue { pos }
            | Self::End { pos }
            | Self::Else { pos }
            | Self::If(Branch { pos, .. })
            | Self::Range(Branch { pos, .. })
            | Self::With(Branch { pos, .. }) => *pos,
        }
    }

    /// Go: `Node.String`.
    pub(super) fn string(&self) -> String {
        let mut sb = String::new();
        self.write_to(&mut sb);
        sb
    }

    fn write_branch(sb: &mut String, name: &str, b: &Branch) {
        sb.push_str("{{");
        sb.push_str(name);
        sb.push(' ');
        b.pipe.write_to(sb);
        sb.push_str("}}");
        b.list.write_to(sb);
        if let Some(else_list) = &b.else_list {
            sb.push_str("{{else}}");
            else_list.write_to(sb);
        }
        sb.push_str("{{end}}");
    }

    fn write_to(&self, sb: &mut String) {
        match self {
            Self::Text { text, .. } => sb.push_str(text),
            Self::Action { pipe, .. } => {
                sb.push_str("{{");
                pipe.write_to(sb);
                sb.push_str("}}");
            }
            Self::If(b) => Self::write_branch(sb, "if", b),
            Self::Range(b) => Self::write_branch(sb, "range", b),
            Self::With(b) => Self::write_branch(sb, "with", b),
            Self::Template { name, pipe, .. } => {
                sb.push_str("{{template ");
                sb.push_str(&quote(name));
                if let Some(pipe) = pipe {
                    sb.push(' ');
                    pipe.write_to(sb);
                }
                sb.push_str("}}");
            }
            Self::Break { .. } => sb.push_str("{{break}}"),
            Self::Continue { .. } => sb.push_str("{{continue}}"),
            Self::End { .. } => sb.push_str("{{end}}"),
            Self::Else { .. } => sb.push_str("{{else}}"),
        }
    }
}

/// Go: `IsEmptyTree`: only space text and comments.
pub(super) fn is_empty_tree(list: &List) -> bool {
    list.nodes.iter().all(|n| match n {
        Node::Text { text, .. } => trim_space(text).is_empty(),
        Node::Action { .. }
        | Node::If(_)
        | Node::Range(_)
        | Node::With(_)
        | Node::Template { .. }
        | Node::Break { .. }
        | Node::Continue { .. }
        | Node::End { .. }
        | Node::Else { .. } => false,
    })
}

impl Pipe {
    pub(super) fn string(&self) -> String {
        let mut sb = String::new();
        self.write_to(&mut sb);
        sb
    }

    fn write_to(&self, sb: &mut String) {
        if !self.decl.is_empty() {
            let names: Vec<&str> = self.decl.iter().map(|d| d.name.as_str()).collect();
            sb.push_str(&names.join(", "));
            sb.push_str(if self.is_assign { " = " } else { " := " });
        }
        for (i, c) in self.cmds.iter().enumerate() {
            if i > 0 {
                sb.push_str(" | ");
            }
            c.write_to(sb);
        }
    }
}

impl Command {
    fn write_to(&self, sb: &mut String) {
        for (i, arg) in self.args.iter().enumerate() {
            if i > 0 {
                sb.push(' ');
            }
            if let Arg::Pipe(p) = arg {
                sb.push('(');
                p.write_to(sb);
                sb.push(')');
                continue;
            }
            arg.write_to(sb);
        }
    }
}

impl Arg {
    pub(super) const fn pos(&self) -> usize {
        match self {
            Self::Identifier { pos, .. }
            | Self::Variable { pos, .. }
            | Self::Dot { pos }
            | Self::Nil { pos }
            | Self::Field { pos, .. }
            | Self::Chain { pos, .. }
            | Self::Bool { pos, .. }
            | Self::Number(Number { pos, .. })
            | Self::String { pos, .. } => *pos,
            Self::Pipe(p) => p.pos,
        }
    }

    pub(super) fn string(&self) -> String {
        let mut sb = String::new();
        self.write_to(&mut sb);
        sb
    }

    fn write_to(&self, sb: &mut String) {
        match self {
            Self::Identifier { ident, .. } => sb.push_str(ident),
            Self::Variable { ident, .. } => sb.push_str(&ident.join(".")),
            Self::Dot { .. } => sb.push('.'),
            Self::Nil { .. } => sb.push_str("nil"),
            Self::Field { ident, .. } => {
                for id in ident {
                    sb.push('.');
                    sb.push_str(id);
                }
            }
            Self::Chain { node, field, .. } => {
                if let Self::Pipe(p) = node.as_ref() {
                    sb.push('(');
                    p.write_to(sb);
                    sb.push(')');
                } else {
                    node.write_to(sb);
                }
                for f in field {
                    sb.push('.');
                    sb.push_str(f);
                }
            }
            Self::Bool { value, .. } => sb.push_str(if *value { "true" } else { "false" }),
            Self::Number(n) => sb.push_str(&n.text),
            Self::String { quoted, .. } => sb.push_str(quoted),
            Self::Pipe(p) => p.write_to(sb),
        }
    }
}

/// Go: `int64(f)` when `float64(int64(f)) == f`, with amd64's conversion of out-of-range
/// values (which never round-trip).
fn exact_int(f: f64) -> Option<i64> {
    if !f.is_finite() || f.fract().abs() > 0.0 {
        return None;
    }
    // Rust's string conversions are exact for integral floats and reject values outside i64.
    format!("{f:.0}").parse().ok()
}

/// Go: `uint64(f)` when `float64(uint64(f)) == f`.
fn exact_uint(f: f64) -> Option<u64> {
    if !f.is_finite() || f.fract().abs() > 0.0 || f < 0.0 {
        return None;
    }
    // abs: -0 converts to 0.
    format!("{:.0}", f.abs()).parse().ok()
}

/// Go: `float64(i)`, rounded to nearest even as Go converts.
fn int_to_float<T: ToString>(i: &T) -> f64 {
    i.to_string().parse().unwrap_or(0.0)
}

impl Number {
    /// Go: `simplifyComplex` for a complex constant with the given parts.
    fn simplify_complex(&mut self, re: f64, im: f64) {
        self.is_complex = true;
        if !im.is_nan() && im.abs() <= 0.0 {
            self.float64 = Some(re);
            self.int64 = exact_int(re);
            self.uint64 = exact_uint(re);
        }
    }

    /// Go: `Tree.newNumber`: the error is Go's text.
    pub(super) fn parse(
        pos: usize,
        text: &str,
        char_constant: bool,
        complex: bool,
    ) -> Result<Self, String> {
        let mut n = Self {
            pos,
            text: text.to_owned(),
            ..Self::default()
        };
        if char_constant {
            let mut chars = text.chars();
            let quote = chars
                .next()
                .and_then(|q| u8::try_from(q).ok())
                .unwrap_or(b'\'');
            let (r, _, tail) = unquote_char(chars.as_str(), quote).map_err(|e| e.to_string())?;
            if tail != "'" {
                return Err(format!("malformed character constant: {text}"));
            }
            let code = u32::from(r);
            n.int64 = Some(code.into());
            n.uint64 = Some(code.into());
            n.float64 = Some(code.into()); // odd but those are the rules.
            return Ok(n);
        }
        if complex {
            let (re, im) = split_complex(text).ok_or("expected complex constant")?;
            n.simplify_complex(re, im);
            return Ok(n);
        }
        // Imaginary constants can only be complex unless they are zero.
        if let Some(im) = text.strip_suffix('i').and_then(|t| parse_float(t, 64).ok()) {
            n.simplify_complex(0.0, im);
            return Ok(n);
        }
        Self::parse_real(n, text)
    }

    fn parse_real(mut n: Self, text: &str) -> Result<Self, String> {
        let u = parse_uint(text, 0, 64).ok();
        n.uint64 = u;
        if let Ok(i) = parse_int(text, 0, 64) {
            n.int64 = Some(i);
            if i == 0 {
                n.uint64 = Some(u.unwrap_or(0)); // in case of -0.
            }
        }
        // If an integer extraction succeeded, promote the float.
        if let Some(i) = n.int64 {
            n.float64 = Some(int_to_float(&i));
        } else if let Some(unsigned) = n.uint64 {
            n.float64 = Some(int_to_float(&unsigned));
        } else if let Ok(f) = parse_float(text, 64) {
            // If we parsed it as a float but it looks like an integer, it's a huge number too
            // large to fit in an int. Reject it.
            if !text.contains(['.', 'e', 'E', 'p', 'P']) {
                return Err(format!("integer overflow: {}", quote(text)));
            }
            n.float64 = Some(f);
            // If a floating-point extraction succeeded, extract the int if needed.
            if n.int64.is_none() {
                n.int64 = exact_int(f);
            }
            if n.uint64.is_none() {
                n.uint64 = exact_uint(f);
            }
        }
        if n.int64.is_none() && n.uint64.is_none() && n.float64.is_none() {
            return Err(format!("illegal number syntax: {}", quote(text)));
        }
        Ok(n)
    }
}

/// The real and imaginary parts of a lexed complex constant such as `1+2i` (Go reads it with
/// `fmt.Sscan`).
fn split_complex(text: &str) -> Option<(f64, f64)> {
    let body = text.strip_suffix('i')?;
    let bytes = body.as_bytes();
    let split = (1..bytes.len()).rev().find(|&i| {
        matches!(bytes.get(i), Some(b'+' | b'-'))
            && !matches!(bytes.get(i - 1), Some(b'e' | b'E' | b'p' | b'P'))
    })?;
    let re = parse_float(body.get(..split)?, 64).ok()?;
    let im = parse_float(body.get(split..)?, 64).ok()?;
    Some((re, im))
}
