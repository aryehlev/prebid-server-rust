//! A small Go parser over [`crate::golex`] tokens.
//!
//! It reads top-level `func`/`const`/`var` declarations, the statements of a function body, and
//! the expression forms adapter tests use to build bidders (literals, identifiers, selectors,
//! calls, composite literals, `&`, `+`). Anything else parses as [`Expr::Other`] carrying its
//! source text, so callers can report exactly what they could not evaluate.

use std::ops::Range;

use crate::{
    error::{Result, ToolError},
    golex::{self, Kind, Token},
};

/// A parsed Go source file.
#[derive(Debug, Clone)]
pub struct File {
    /// The path the file was read from, as given to [`File::parse`].
    pub path: String,
    /// The source text.
    pub src: String,
    /// The tokens of `src`.
    pub tokens: Vec<Token>,
    /// The package clause name.
    pub package: String,
    /// Top-level functions (methods excluded), in source order.
    pub funcs: Vec<Func>,
    /// Top-level `const` and `var` specs, in source order.
    pub values: Vec<ValueSpec>,
}

/// A top-level function declaration.
#[derive(Debug, Clone)]
pub struct Func {
    /// The function name.
    pub name: String,
    /// Parameter names in order (empty when the parameters are unnamed).
    pub params: Vec<String>,
    /// The statements of the body.
    pub body: Vec<Stmt>,
}

/// One name of a top-level `const` or `var` spec.
#[derive(Debug, Clone)]
pub struct ValueSpec {
    /// The declared name.
    pub name: String,
    /// Whether it was declared with `const`.
    pub is_const: bool,
    /// The declared type as written (whitespace collapsed); empty when omitted.
    pub ty: String,
    /// The value expression; `None` when the spec has no value or repeats an earlier one.
    pub value: Option<Expr>,
}

/// One statement of a function body.
#[derive(Debug, Clone)]
pub struct Stmt {
    /// Token range of the statement within the file's tokens.
    pub tokens: Range<usize>,
    /// What kind of statement it is.
    pub kind: StmtKind,
}

/// The statement forms the manifest generator distinguishes.
#[derive(Debug, Clone)]
pub enum StmtKind {
    /// `a, b := x, y`, `a = x`, or `var a = x`, with every left-hand side a plain identifier.
    Bind {
        /// The names bound (`_` included).
        names: Vec<String>,
        /// The right-hand side expressions.
        values: Vec<Expr>,
    },
    /// `return x, ...`.
    Return(Vec<Expr>),
    /// An expression statement (usually a call).
    Expr(Expr),
    /// An `if` statement.
    If,
    /// Anything else.
    Other,
}

/// An expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// A string literal, decoded.
    Str(String),
    /// An integer literal.
    Int(i64),
    /// An identifier.
    Ident(String),
    /// `x.name`.
    Selector(Box<Self>, String),
    /// `f(args)`.
    Call {
        /// The called expression.
        func: Box<Self>,
        /// The arguments.
        args: Vec<Self>,
    },
    /// `T{...}` (the type text is empty when elided).
    Composite {
        /// The literal's type as written, without spaces.
        ty: String,
        /// The elements.
        elems: Vec<Elem>,
    },
    /// A unary operation such as `&x`.
    Unary {
        /// The operator.
        op: String,
        /// The operand.
        expr: Box<Self>,
    },
    /// A binary operation such as `a + b`.
    Binary {
        /// The operator.
        op: String,
        /// The left operand.
        lhs: Box<Self>,
        /// The right operand.
        rhs: Box<Self>,
    },
    /// An expression this parser does not model, as source text.
    Other(String),
}

/// One element of a composite literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elem {
    /// The key for `key: value` elements.
    pub key: Option<Expr>,
    /// The value.
    pub value: Expr,
}

impl File {
    /// Tokenizes and parses `src`.
    ///
    /// # Errors
    ///
    /// Fails when the source does not tokenize or its top-level structure is malformed.
    pub fn parse(path: &str, src: String) -> Result<Self> {
        let tokens = golex::tokenize(&src).map_err(|e| ToolError::new(format!("{path}: {e}")))?;
        let mut file = Self {
            path: path.to_owned(),
            src,
            tokens,
            package: String::new(),
            funcs: Vec::new(),
            values: Vec::new(),
        };
        file.parse_top_level()
            .map_err(|e| ToolError::new(format!("{path}: {e}")))?;
        Ok(file)
    }

    /// The source text covering a token range, with whitespace runs collapsed to one space.
    #[must_use]
    pub fn text(&self, range: Range<usize>) -> String {
        source_text(&self.src, &self.tokens, range)
    }

    /// The tokens of a range.
    #[must_use]
    pub fn toks(&self, range: Range<usize>) -> &[Token] {
        self.tokens.get(range).unwrap_or_default()
    }

    /// The statements inside the `{` at token index `open`, and the index of its `}`.
    #[must_use]
    pub fn block(&self, open: usize) -> Option<(Vec<Stmt>, usize)> {
        if !self.tok(open)?.is("{") {
            return None;
        }
        let close = matching(&self.tokens, open)?;
        Some((self.statements(open + 1..close), close))
    }

    /// For `if init; cond { then } [else ...]` at `range`: the init statement (if any) and the
    /// statements of the `then` block.
    #[must_use]
    pub fn if_parts(&self, range: Range<usize>) -> Option<(Option<Stmt>, Vec<Stmt>)> {
        if !self.tok(range.start)?.is("if") {
            return None;
        }
        let mut i = range.start + 1;
        while i < range.end && !self.tok(i)?.is("{") {
            i = matching(&self.tokens, i).map_or(i + 1, |m| m + 1);
        }
        let header = split(&self.tokens, range.start + 1..i, Kind::Semi, "");
        let init = match header.as_slice() {
            [init, _cond] => Some(Stmt {
                kind: self.classify(init.clone()),
                tokens: init.clone(),
            }),
            _ => None,
        };
        let (then, _) = self.block(i)?;
        Some((init, then))
    }

    fn tok(&self, i: usize) -> Option<&Token> {
        self.tokens.get(i)
    }

    fn parse_top_level(&mut self) -> Result<()> {
        let mut i = 0;
        while let Some(t) = self.tok(i) {
            if t.kind == Kind::Semi {
                i += 1;
                continue;
            }
            match t.text.as_str() {
                "package" => {
                    self.package = self
                        .tok(i + 1)
                        .map_or_else(String::new, |name| name.text.clone());
                    i = self.statement_end(i);
                }
                "func" => i = self.parse_func(i)?,
                "const" | "var" => i = self.parse_values(i)?,
                _ => i = self.statement_end(i),
            }
        }
        if self.package.is_empty() {
            return Err(ToolError::new("missing package clause"));
        }
        Ok(())
    }

    /// Index just past the `;` ending the declaration or statement starting at `i`.
    fn statement_end(&self, i: usize) -> usize {
        let mut j = i;
        while let Some(t) = self.tok(j) {
            if t.kind == Kind::Semi {
                return j + 1;
            }
            j = matching(&self.tokens, j).map_or(j + 1, |m| m + 1);
        }
        j
    }

    fn parse_func(&mut self, start: usize) -> Result<usize> {
        let mut i = start + 1;
        let is_method = self.tok(i).is_some_and(|t| t.is("("));
        if is_method {
            i = after_group(&self.tokens, i)?;
        }
        let name = self
            .tok(i)
            .filter(|t| t.kind == Kind::Ident)
            .map(|t| t.text.clone());
        let Some(name) = name else {
            return Err(ToolError::new(format!("func without a name at token {i}")));
        };
        i += 1;
        if self.tok(i).is_some_and(|t| t.is("[")) {
            i = after_group(&self.tokens, i)?;
        }
        if !self.tok(i).is_some_and(|t| t.is("(")) {
            return Err(ToolError::new(format!("func {name}: expected parameters")));
        }
        let params_end = after_group(&self.tokens, i)?;
        let params = param_names(self.toks(i + 1..params_end - 1));
        i = params_end;
        // Results run up to the body's `{`; skip struct{...} and interface{...} result types.
        while let Some(t) = self.tok(i) {
            if t.is("{") || t.kind == Kind::Semi {
                break;
            }
            if t.is("struct") || t.is("interface") {
                i += 1;
                if self.tok(i).is_some_and(|next| next.is("{")) {
                    i = after_group(&self.tokens, i)?;
                }
                continue;
            }
            i = matching(&self.tokens, i).map_or(i + 1, |m| m + 1);
        }
        if !self.tok(i).is_some_and(|t| t.is("{")) {
            // A declaration without a body (assembly); nothing to record.
            return Ok(self.statement_end(i));
        }
        let body_end = after_group(&self.tokens, i)?;
        let body = self.statements(i + 1..body_end - 1);
        if !is_method {
            self.funcs.push(Func { name, params, body });
        }
        Ok(self.statement_end(body_end))
    }

    fn parse_values(&mut self, start: usize) -> Result<usize> {
        let is_const = self.tok(start).is_some_and(|t| t.is("const"));
        let i = start + 1;
        if self.tok(i).is_some_and(|t| t.is("(")) {
            let end = after_group(&self.tokens, i)?;
            for spec in split(&self.tokens, i + 1..end - 1, Kind::Semi, "") {
                self.push_spec(spec, is_const);
            }
            return Ok(self.statement_end(end));
        }
        let end = self.statement_end(i);
        let spec_end = if self
            .tok(end.saturating_sub(1))
            .is_some_and(|t| t.kind == Kind::Semi)
        {
            end - 1
        } else {
            end
        };
        self.push_spec(i..spec_end, is_const);
        Ok(end)
    }

    fn push_spec(&mut self, spec: Range<usize>, is_const: bool) {
        let toks = self.toks(spec.clone());
        let eq = toks.iter().position(|t| t.is("="));
        // IdentifierList = identifier { "," identifier }.
        let mut names = Vec::new();
        let mut names_end = 0;
        while let Some(t) = toks.get(names_end).filter(|t| t.kind == Kind::Ident) {
            names.push(t.text.clone());
            names_end += 1;
            if !toks.get(names_end).is_some_and(|sep| sep.is(",")) {
                break;
            }
            names_end += 1;
        }
        let values: Vec<Expr> = eq.map_or_else(Vec::new, |eq| {
            split(&self.tokens, spec.start + eq + 1..spec.end, Kind::Op, ",")
                .into_iter()
                .map(|r| parse_expr(&self.src, &self.tokens, r))
                .collect()
        });
        let ty_end = eq.unwrap_or(toks.len()).max(names_end);
        let ty = self.text(spec.start + names_end..spec.start + ty_end);
        for (k, name) in names.into_iter().enumerate() {
            let value = values.get(k).cloned();
            self.values.push(ValueSpec {
                name,
                is_const,
                ty: ty.clone(),
                value,
            });
        }
    }

    fn statements(&self, range: Range<usize>) -> Vec<Stmt> {
        self.statement_spans(range)
            .into_iter()
            .filter(|r| !r.is_empty())
            .map(|r| Stmt {
                kind: self.classify(r.clone()),
                tokens: r,
            })
            .collect()
    }

    /// Splits a statement list at `;`, keeping `if`/`for`/`switch`/`select` statements whole:
    /// their headers may contain `;`, and an `if` continues through its `else` chain.
    fn statement_spans(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut start = range.start;
        let mut i = range.start;
        while i < range.end {
            let Some(t) = self.tok(i) else { break };
            if t.kind == Kind::Semi {
                out.push(start..i);
                i += 1;
                start = i;
                continue;
            }
            if i == start && (t.is("if") || t.is("for") || t.is("switch") || t.is("select")) {
                i = self.compound_end(i, range.end);
                continue;
            }
            i = matching(&self.tokens, i)
                .map_or(i + 1, |m| m + 1)
                .min(range.end);
        }
        if start < range.end {
            out.push(start..range.end);
        }
        out
    }

    /// Index just past a compound statement starting at keyword `i`.
    fn compound_end(&self, mut i: usize, end: usize) -> usize {
        loop {
            // Header: up to the block's `{` at depth zero, skipping composite literals of
            // slice/array/map types (`range []string{"a"}`), which Go allows there unparenthesized.
            i += 1;
            while i < end {
                let Some(t) = self.tok(i) else { return end };
                if t.is("{") && !self.opens_type_literal(i) {
                    break;
                }
                i = matching(&self.tokens, i).map_or(i + 1, |m| m + 1);
            }
            let Some(close) = matching(&self.tokens, i) else {
                return end;
            };
            i = close + 1;
            if !self.tok(i).is_some_and(|t| t.is("else")) {
                return i.min(end);
            }
            i += 1;
            if !self.tok(i).is_some_and(|t| t.is("if")) {
                // Final `else { ... }`.
                return matching(&self.tokens, i).map_or(end, |m| (m + 1).min(end));
            }
        }
    }

    /// Whether the `{` at `i` starts the body of a `[]T{`, `[N]T{` or `map[K]V{` literal.
    ///
    /// The element type is an identifier, possibly qualified, possibly behind `*`s, preceded by
    /// `[...]`; that bracket must start a type (`map[`, or `[` after an operator or `range`), not
    /// index an operand (`a < b[i]*c {`).
    fn opens_type_literal(&self, i: usize) -> bool {
        let is_ident = |k: usize| self.tok(k).is_some_and(|t| t.kind == Kind::Ident);
        let is_op =
            |k: usize, op: &str| self.tok(k).is_some_and(|t| t.kind == Kind::Op && t.is(op));
        let Some(mut j) = i.checked_sub(1).filter(|&k| is_ident(k)) else {
            return false;
        };
        if j >= 2 && is_op(j - 1, ".") && is_ident(j - 2) {
            j -= 2;
        }
        while j > 0 && is_op(j - 1, "*") {
            j -= 1;
        }
        let Some(mut close) = j.checked_sub(1).filter(|&k| is_op(k, "]")) else {
            return false;
        };
        // Walk back over consecutive brackets (`map[K][]V`, `[2][3]T`) to what precedes them.
        loop {
            let Some(open) = self.opening(close) else {
                return false;
            };
            let Some(before) = open.checked_sub(1).and_then(|k| self.tok(k)) else {
                return true;
            };
            match before.kind {
                Kind::Op if before.is("]") => close = open - 1,
                Kind::Ident => return before.is("map") || before.is("range"),
                Kind::Op => return !(before.is(")") || before.is("}")),
                Kind::Semi => return true,
                Kind::Int
                | Kind::Float
                | Kind::Imag
                | Kind::Char
                | Kind::String
                | Kind::RawString => {
                    return false;
                }
            }
        }
    }

    /// Index of the `[` that the `]` at `close` closes.
    fn opening(&self, close: usize) -> Option<usize> {
        let mut depth = 0_usize;
        for k in (0..=close).rev() {
            let t = self.tok(k)?;
            if t.kind != Kind::Op {
                continue;
            }
            if t.is("]") {
                depth += 1;
            } else if t.is("[") {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(k);
                }
            }
        }
        None
    }

    fn classify(&self, r: Range<usize>) -> StmtKind {
        let toks = self.toks(r.clone());
        let Some(first) = toks.first() else {
            return StmtKind::Other;
        };
        if first.is("if") {
            return StmtKind::If;
        }
        if first.is("return") {
            let values = split(&self.tokens, r.start + 1..r.end, Kind::Op, ",")
                .into_iter()
                .filter(|v| !v.is_empty())
                .map(|v| parse_expr(&self.src, &self.tokens, v))
                .collect();
            return StmtKind::Return(values);
        }
        if first.is("var") {
            let Some(eq) = toks.iter().position(|t| t.is("=")) else {
                return StmtKind::Other;
            };
            let name = toks
                .get(1)
                .filter(|t| t.kind == Kind::Ident)
                .map(|t| t.text.clone());
            return name.map_or(StmtKind::Other, |name| StmtKind::Bind {
                names: vec![name],
                values: vec![parse_expr(&self.src, &self.tokens, r.start + eq + 1..r.end)],
            });
        }
        if is_keyword_start(first) {
            return StmtKind::Other;
        }
        let assign = depth_zero_positions(toks)
            .into_iter()
            .find(|&k| toks.get(k).is_some_and(|t| t.is(":=") || t.is("=")));
        let Some(assign) = assign else {
            return StmtKind::Expr(parse_expr(&self.src, &self.tokens, r));
        };
        let lhs = split(&self.tokens, r.start..r.start + assign, Kind::Op, ",");
        let mut names = Vec::new();
        for l in lhs {
            match self.toks(l) {
                [t] if t.kind == Kind::Ident => names.push(t.text.clone()),
                _ => return StmtKind::Other,
            }
        }
        let values = split(&self.tokens, r.start + assign + 1..r.end, Kind::Op, ",")
            .into_iter()
            .map(|v| parse_expr(&self.src, &self.tokens, v))
            .collect();
        StmtKind::Bind { names, values }
    }
}

fn is_keyword_start(t: &Token) -> bool {
    t.kind == Kind::Ident && !t.is_ident()
}

/// The text of `range` with whitespace runs collapsed.
fn source_text(src: &str, tokens: &[Token], range: Range<usize>) -> String {
    let toks = tokens.get(range).unwrap_or_default();
    let (Some(first), Some(last)) = (toks.first(), toks.iter().rev().find(|t| !t.text.is_empty()))
    else {
        return String::new();
    };
    let raw = src.get(first.start..last.end).unwrap_or_default();
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Index of the bracket closing the one at `i`, if `i` opens a bracket.
fn matching(tokens: &[Token], i: usize) -> Option<usize> {
    let open = tokens.get(i)?;
    let close = match open.text.as_str() {
        "(" if open.kind == Kind::Op => ")",
        "[" if open.kind == Kind::Op => "]",
        "{" if open.kind == Kind::Op => "}",
        _ => return None,
    };
    let mut depth = 0_usize;
    for (j, t) in tokens.iter().enumerate().skip(i) {
        if t.kind != Kind::Op {
            continue;
        }
        if t.text == open.text {
            depth += 1;
        } else if t.text == close {
            depth -= 1;
            if depth == 0 {
                return Some(j);
            }
        }
    }
    None
}

fn after_group(tokens: &[Token], i: usize) -> Result<usize> {
    matching(tokens, i)
        .map(|m| m + 1)
        .ok_or_else(|| ToolError::new(format!("unbalanced bracket at token {i}")))
}

/// Positions (relative to `toks`) of tokens outside any bracket.
fn depth_zero_positions(toks: &[Token]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut k = 0;
    while let Some(t) = toks.get(k) {
        out.push(k);
        k = if t.kind == Kind::Op {
            matching(toks, k).map_or(k + 1, |m| m + 1)
        } else {
            k + 1
        };
    }
    out
}

/// Splits `range` at depth-zero tokens of `kind` (and text `text`, unless empty).
fn split(tokens: &[Token], range: Range<usize>, kind: Kind, text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = range.start;
    let mut i = range.start;
    while i < range.end {
        let Some(t) = tokens.get(i) else { break };
        if t.kind == kind && (text.is_empty() || t.text == text) {
            out.push(start..i);
            start = i + 1;
            i += 1;
            continue;
        }
        i = matching(tokens, i).map_or(i + 1, |m| m + 1).min(range.end);
    }
    if start < range.end {
        out.push(start..range.end);
    }
    out
}

fn param_names(toks: &[Token]) -> Vec<String> {
    let groups: Vec<&[Token]> = split(toks, 0..toks.len(), Kind::Op, ",")
        .into_iter()
        .map(|r| toks.get(r).unwrap_or_default())
        .collect();
    let named = groups.iter().any(|g| match g {
        [first, second, ..] => first.is_ident() && !second.is("."),
        _ => false,
    });
    if !named {
        return Vec::new();
    }
    groups
        .iter()
        .filter_map(|g| g.first())
        .map(|t| t.text.clone())
        .collect()
}

/// Parses the tokens of `range` as one expression; unparsed input yields [`Expr::Other`].
#[must_use]
pub fn parse_expr(src: &str, tokens: &[Token], range: Range<usize>) -> Expr {
    let toks = tokens.get(range.clone()).unwrap_or_default();
    let mut p = Parser { src, toks, pos: 0 };
    match p.expr(0) {
        Some(e) if p.pos == toks.len() => e,
        _ => Expr::Other(source_text(src, tokens, range)),
    }
}

struct Parser<'a> {
    src: &'a str,
    toks: &'a [Token],
    pos: usize,
}

fn precedence(op: &str) -> Option<u8> {
    match op {
        "||" => Some(1),
        "&&" => Some(2),
        "==" | "!=" | "<" | "<=" | ">" | ">=" => Some(3),
        "+" | "-" | "|" | "^" => Some(4),
        "*" | "/" | "%" | "<<" | ">>" | "&" | "&^" => Some(5),
        _ => None,
    }
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    fn text(&self, range: Range<usize>) -> String {
        source_text(self.src, self.toks, range)
    }

    fn expr(&mut self, min_prec: u8) -> Option<Expr> {
        let mut lhs = self.unary()?;
        while let Some(t) = self.peek() {
            let Some(prec) = (t.kind == Kind::Op).then(|| precedence(&t.text)).flatten() else {
                break;
            };
            if prec <= min_prec {
                break;
            }
            let op = t.text.clone();
            self.pos += 1;
            let rhs = self.expr(prec)?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Some(lhs)
    }

    fn unary(&mut self) -> Option<Expr> {
        let t = self.peek()?;
        if t.kind == Kind::Op && matches!(t.text.as_str(), "&" | "-" | "+" | "!" | "^" | "*" | "<-")
        {
            let op = t.text.clone();
            self.pos += 1;
            let expr = self.unary()?;
            return Some(Expr::Unary {
                op,
                expr: Box::new(expr),
            });
        }
        let start = self.pos;
        let mut e = self.primary()?;
        while let Some(next_tok) = self.peek() {
            if next_tok.is(".") {
                let next = self.toks.get(self.pos + 1)?;
                if next.kind == Kind::Ident {
                    e = Expr::Selector(Box::new(e), next.text.clone());
                    self.pos += 2;
                } else if next.is("(") {
                    // Type assertion.
                    self.pos = matching(self.toks, self.pos + 1)? + 1;
                    e = Expr::Other(self.text(start..self.pos));
                } else {
                    return None;
                }
            } else if next_tok.is("(") {
                let end = matching(self.toks, self.pos)?;
                let args = self.list(self.pos + 1..end);
                self.pos = end + 1;
                e = Expr::Call {
                    func: Box::new(e),
                    args,
                };
            } else if next_tok.is("[") {
                self.pos = matching(self.toks, self.pos)? + 1;
                e = Expr::Other(self.text(start..self.pos));
            } else if next_tok.is("{") && matches!(e, Expr::Ident(_) | Expr::Selector(..)) {
                let ty = self.text(start..self.pos).replace(' ', "");
                e = self.composite(ty)?;
            } else {
                break;
            }
        }
        Some(e)
    }

    fn primary(&mut self) -> Option<Expr> {
        let t = self.peek()?.clone();
        match t.kind {
            Kind::String | Kind::RawString => {
                self.pos += 1;
                golex::unquote(&t.text).ok().map(Expr::Str)
            }
            Kind::Int => {
                self.pos += 1;
                Some(golex::parse_int(&t.text).map_or(Expr::Other(t.text), Expr::Int))
            }
            Kind::Float | Kind::Imag | Kind::Char => {
                self.pos += 1;
                Some(Expr::Other(t.text))
            }
            Kind::Ident if t.is_ident() => {
                self.pos += 1;
                Some(Expr::Ident(t.text))
            }
            Kind::Op if t.is("(") => {
                let end = matching(self.toks, self.pos)?;
                let inner = Parser {
                    src: self.src,
                    toks: self.toks.get(..end)?,
                    pos: self.pos + 1,
                }
                .full_expr()?;
                self.pos = end + 1;
                Some(inner)
            }
            Kind::Op if t.is("[") => self.typed_composite(),
            Kind::Ident if t.is("map") || t.is("struct") => self.typed_composite(),
            Kind::Ident if t.is("func") => {
                // Function literal: signature, then the body.
                let start = self.pos;
                while let Some(tok) = self.peek() {
                    if tok.is("{") {
                        self.pos = matching(self.toks, self.pos)? + 1;
                        return Some(Expr::Other(self.text(start..self.pos)));
                    }
                    self.pos = matching(self.toks, self.pos).map_or(self.pos + 1, |m| m + 1);
                }
                None
            }
            Kind::Ident | Kind::Op | Kind::Semi => None,
        }
    }

    fn full_expr(mut self) -> Option<Expr> {
        let e = self.expr(0)?;
        (self.pos == self.toks.len()).then_some(e)
    }

    /// `[]T{...}`, `[N]T{...}`, `map[K]V{...}`, `struct{...}{...}`.
    fn typed_composite(&mut self) -> Option<Expr> {
        let start = self.pos;
        while let Some(t) = self.peek() {
            let after_struct =
                self.pos > start && self.toks.get(self.pos - 1).is_some_and(|p| p.is("struct"));
            if t.is("{") && !after_struct {
                break;
            }
            self.pos = matching(self.toks, self.pos).map_or(self.pos + 1, |m| m + 1);
        }
        let ty = self.text(start..self.pos).replace(' ', "");
        if self.peek().is_some_and(|t| t.is("{")) {
            return self.composite(ty);
        }
        Some(Expr::Other(self.text(start..self.pos)))
    }

    fn composite(&mut self, ty: String) -> Option<Expr> {
        let end = matching(self.toks, self.pos)?;
        let mut elems = Vec::new();
        for r in split(self.toks, self.pos + 1..end, Kind::Op, ",") {
            if r.is_empty() {
                continue;
            }
            let toks = self.toks.get(r.clone())?;
            let colon = depth_zero_positions(toks)
                .into_iter()
                .find(|&k| toks.get(k).is_some_and(|t| t.is(":")));
            let elem = if let Some(c) = colon {
                Elem {
                    key: Some(self.sub_expr(r.start..r.start + c)),
                    value: self.sub_expr(r.start + c + 1..r.end),
                }
            } else {
                Elem {
                    key: None,
                    value: self.sub_expr(r),
                }
            };
            elems.push(elem);
        }
        self.pos = end + 1;
        Some(Expr::Composite { ty, elems })
    }

    fn sub_expr(&self, range: Range<usize>) -> Expr {
        let toks = self.toks.get(range.clone()).unwrap_or_default();
        if toks.first().is_some_and(|t| t.is("{")) && matching(toks, 0) == Some(toks.len() - 1) {
            let mut p = Parser {
                src: self.src,
                toks,
                pos: 0,
            };
            if let Some(e) = p.composite(String::new()) {
                return e;
            }
        }
        parse_expr(self.src, self.toks, range)
    }

    fn list(&self, range: Range<usize>) -> Vec<Expr> {
        split(self.toks, range, Kind::Op, ",")
            .into_iter()
            .filter(|r| !r.is_empty())
            .map(|r| parse_expr(self.src, self.toks, r))
            .collect()
    }
}

/// Renders an expression back to compact Go-like text (for messages and callee names).
#[must_use]
pub fn render(e: &Expr) -> String {
    match e {
        Expr::Str(s) => format!("{s:?}"),
        Expr::Int(i) => i.to_string(),
        Expr::Ident(s) | Expr::Other(s) => s.clone(),
        Expr::Selector(x, name) => format!("{}.{name}", render(x)),
        Expr::Call { func, args } => {
            let args: Vec<String> = args.iter().map(render).collect();
            format!("{}({})", render(func), args.join(", "))
        }
        Expr::Composite { ty, elems } => {
            let elems: Vec<String> = elems
                .iter()
                .map(|el| {
                    el.key.as_ref().map_or_else(
                        || render(&el.value),
                        |k| format!("{}: {}", render(k), render(&el.value)),
                    )
                })
                .collect();
            format!("{ty}{{{}}}", elems.join(", "))
        }
        Expr::Unary { op, expr } => format!("{op}{}", render(expr)),
        Expr::Binary { op, lhs, rhs } => format!("{} {op} {}", render(lhs), render(rhs)),
    }
}

#[cfg(test)]
mod tests {
    use super::{Expr, File, Func, StmtKind, render};
    use crate::{
        error::{Result, ToolError},
        testutil::{check, check_eq},
    };

    const SRC: &str = r#"package demo

import (
	"testing"
)

const (
	testsDir = "demotest"
	endpoint = "http://" + "host" + "/path"
)

var testURL = `raw`

type x struct{ A int }

func (x) Method() {}

func helper(t *testing.T, ep string) interface{ M() } {
	b, err := Builder(openrtb_ext.BidderDemo, config.Adapter{Endpoint: ep,
		XAPI: config.AdapterXAPI{Username: "u"}}, config.Server{GvlID: 12})
	if err != nil {
		t.Fatalf("x")
	}
	return b
}

func TestJsonSamples(t *testing.T) {
	adapterstest.RunJSONBidderTest(t, testsDir, helper(t, endpoint))
}

func TestCompound(t *testing.T) {
	if bidder, err := Builder(openrtb_ext.BidderDemo,
		config.Adapter{}, config.Server{}); err == nil {
		adapterstest.RunJSONBidderTest(t, "demotest", bidder)
	} else if err != nil {
		t.Fatalf("x")
	} else {
		t.Log("y")
	}
	for _, s := range []string{"a", "b"} {
		_ = s
	}
	for k, v := range map[string][]int{"a": {1}} {
		_, _ = k, v
	}
	for i := 0; i < n; i++ {
		_ = i
	}
	if a < b[1]*c {
		_ = a
	}
	done := true
}
"#;

    fn func<'a>(f: &'a File, name: &str) -> Result<&'a Func> {
        f.funcs
            .iter()
            .find(|g| g.name == name)
            .ok_or_else(|| ToolError::new(format!("no func {name}")))
    }

    #[test]
    fn parses_declarations() -> Result<()> {
        let f = File::parse("demo_test.go", SRC.to_owned())?;
        check_eq(f.package.as_str(), "demo", "package")?;
        let names: Vec<&str> = f.funcs.iter().map(|g| g.name.as_str()).collect();
        check_eq(
            names.as_slice(),
            &["helper", "TestJsonSamples", "TestCompound"][..],
            "methods are skipped",
        )?;
        let helper = func(&f, "helper")?;
        check_eq(
            helper.params.as_slice(),
            &["t".to_owned(), "ep".to_owned()][..],
            "params",
        )?;
        check_eq(&helper.body.len(), &3, "statements")?;
        let values: Vec<(&str, Option<String>)> = f
            .values
            .iter()
            .map(|v| (v.name.as_str(), v.value.as_ref().map(render)))
            .collect();
        let want = [
            ("testsDir", Some("\"demotest\"".to_owned())),
            (
                "endpoint",
                Some("\"http://\" + \"host\" + \"/path\"".to_owned()),
            ),
            ("testURL", Some("\"raw\"".to_owned())),
        ];
        check_eq(values.as_slice(), &want[..], "values")
    }

    #[test]
    fn parses_builder_assignment() -> Result<()> {
        let f = File::parse("demo_test.go", SRC.to_owned())?;
        let helper = func(&f, "helper")?;
        let first = helper
            .body
            .first()
            .ok_or_else(|| ToolError::new("no statement"))?;
        let StmtKind::Bind { names, values } = &first.kind else {
            return Err(ToolError::new(format!("not a binding: {:?}", first.kind)));
        };
        check_eq(
            names.as_slice(),
            &["b".to_owned(), "err".to_owned()][..],
            "names",
        )?;
        let rendered: Vec<String> = values.iter().map(render).collect();
        let want = "Builder(openrtb_ext.BidderDemo, config.Adapter{Endpoint: ep, XAPI: \
                    config.AdapterXAPI{Username: \"u\"}}, config.Server{GvlID: 12})";
        check_eq(rendered.as_slice(), &[want.to_owned()][..], "rhs")?;
        check(
            matches!(helper.body.get(1).map(|s| &s.kind), Some(StmtKind::If)),
            "if",
        )?;
        let ret = helper.body.get(2).map(|s| &s.kind);
        check(
            matches!(ret, Some(StmtKind::Return(v)) if v == &[Expr::Ident("b".to_owned())]),
            "return",
        )
    }

    #[test]
    fn statement_text_collapses_whitespace() -> Result<()> {
        let f = File::parse("demo_test.go", SRC.to_owned())?;
        let helper = func(&f, "helper")?;
        let first = helper
            .body
            .first()
            .ok_or_else(|| ToolError::new("no statement"))?;
        let want = "b, err := Builder(openrtb_ext.BidderDemo, config.Adapter{Endpoint: ep, XAPI: \
                    config.AdapterXAPI{Username: \"u\"}}, config.Server{GvlID: 12})";
        check_eq(f.text(first.tokens.clone()).as_str(), want, "text")
    }

    #[test]
    fn compound_statements_stay_whole() -> Result<()> {
        let f = File::parse("demo_test.go", SRC.to_owned())?;
        let body = &func(&f, "TestCompound")?.body;
        let starts: Vec<String> = body
            .iter()
            .map(|s| {
                f.toks(s.tokens.clone())
                    .first()
                    .map_or_else(String::new, |t| t.text.clone())
            })
            .collect();
        let want = ["if", "for", "for", "for", "if", "done"].map(str::to_owned);
        check_eq(starts.as_slice(), &want[..], "statement starts")?;
        check(
            matches!(body.last().map(|s| &s.kind), Some(StmtKind::Bind { .. })),
            "trailing binding",
        )
    }

    #[test]
    fn if_parts_expose_init_and_then_block() -> Result<()> {
        let f = File::parse("demo_test.go", SRC.to_owned())?;
        let body = &func(&f, "TestCompound")?.body;
        let first = body.first().ok_or_else(|| ToolError::new("no statement"))?;
        let (init, then) = f
            .if_parts(first.tokens.clone())
            .ok_or_else(|| ToolError::new("not an if"))?;
        let init = init.ok_or_else(|| ToolError::new("no init statement"))?;
        check(
            matches!(&init.kind, StmtKind::Bind { names, .. } if names == &["bidder", "err"]),
            "init binds bidder, err",
        )?;
        let calls: Vec<String> = then
            .iter()
            .map(|s| {
                if let StmtKind::Expr(e) = &s.kind {
                    render(e)
                } else {
                    format!("{:?}", s.kind)
                }
            })
            .collect();
        check_eq(
            calls.as_slice(),
            &["adapterstest.RunJSONBidderTest(t, \"demotest\", bidder)".to_owned()][..],
            "then block",
        )?;
        let plain = body
            .get(4)
            .ok_or_else(|| ToolError::new("no fifth statement"))?;
        let (no_init, _) = f
            .if_parts(plain.tokens.clone())
            .ok_or_else(|| ToolError::new("not an if"))?;
        check(no_init.is_none(), "an if without init has none")
    }
}
