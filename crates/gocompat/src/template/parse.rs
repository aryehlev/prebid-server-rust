//! Go: `text/template/parse/parse.go`: the parser, with Go's three-token lookahead (whose most
//! recent token gives the line of every parse error) and error texts.

use std::{collections::BTreeMap, mem};

use super::{
    TemplateError,
    lex::{Item, ItemType, Lexer},
    node::{Arg, Branch, Command, Decl, List, Node, Number, Pipe, is_empty_tree},
};
use crate::{
    fmt::{Value, errorf},
    strconv::unquote,
};

/// Go: a parsed `parse.Tree`.
#[derive(Debug, Clone)]
pub(super) struct Tree {
    pub(super) root: List,
}

/// Go: the names of `text/template`'s builtin functions, which `Parse` accepts.
const BUILTINS: [&str; 19] = [
    "and", "call", "html", "index", "slice", "js", "len", "not", "or", "print", "printf",
    "println", "urlquery", "eq", "ge", "gt", "le", "lt", "ne",
];

/// Go allows 10000 nested parenthesized pipelines; this parser (and the executor, which
/// recurses the same way) runs on Rust's smaller thread stacks and stops at 100, with Go's
/// message.
const MAX_STACK_DEPTH: usize = 100;

/// Go nests `if`/`range`/`with`/`block` without limit; past this depth this parser fails with
/// [`NESTING_ERROR`] instead of exhausting the stack.
const MAX_NESTING: usize = 100;
const NESTING_ERROR: &str = "max control nesting depth exceeded";

type Res<T> = Result<T, TemplateError>;

/// The per-tree state of Go's `Tree` while parsing (a `define` or `block` parses its own tree
/// from the shared lexer).
#[derive(Debug, Default)]
struct TreeState {
    name: String,
    token: [Item; 3],
    peek_count: usize,
    vars: Vec<String>,
    action_line: usize,
    range_depth: usize,
    stack_depth: usize,
}

impl TreeState {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            vars: vec!["$".to_owned()],
            ..Self::default()
        }
    }
}

/// Go: `parse.Parse(name, text, "", "", builtins())`: the trees by template name.
pub(super) fn parse(name: &str, text: &str) -> Res<BTreeMap<String, Tree>> {
    let mut p = Parser {
        parse_name: name,
        lex: Lexer::new(text),
        trees: BTreeMap::new(),
        t: TreeState::new(name),
        nesting: 0,
    };
    let root = p.parse_body()?;
    p.add(root)?;
    Ok(p.trees)
}

fn text(s: &str) -> Value {
    Value::from(s)
}

fn split_dots(s: &str) -> Vec<String> {
    s.split('.').map(str::to_owned).collect()
}

struct Parser<'a> {
    parse_name: &'a str,
    lex: Lexer<'a>,
    trees: BTreeMap<String, Tree>,
    t: TreeState,
    /// Open control structures, across `block` trees.
    nesting: usize,
}

impl Parser<'_> {
    fn next(&mut self) -> Item {
        if self.t.peek_count > 0 {
            self.t.peek_count -= 1;
        } else {
            self.t.token[0] = self.lex.next_item();
        }
        self.t
            .token
            .get(self.t.peek_count)
            .cloned()
            .unwrap_or_default()
    }

    const fn backup(&mut self) {
        self.t.peek_count += 1;
    }

    fn backup2(&mut self, t1: Item) {
        self.t.token[1] = t1;
        self.t.peek_count = 2;
    }

    /// Reverse order: we're pushing back.
    fn backup3(&mut self, t2: Item, t1: Item) {
        self.t.token[1] = t1;
        self.t.token[2] = t2;
        self.t.peek_count = 3;
    }

    fn peek(&mut self) -> Item {
        if let Some(i) = self.t.peek_count.checked_sub(1) {
            return self.t.token.get(i).cloned().unwrap_or_default();
        }
        self.t.peek_count = 1;
        self.t.token[0] = self.lex.next_item();
        self.t.token[0].clone()
    }

    fn next_non_space(&mut self) -> Item {
        loop {
            let token = self.next();
            if token.typ != ItemType::Space {
                return token;
            }
        }
    }

    fn peek_non_space(&mut self) -> Item {
        let token = self.next_non_space();
        self.backup();
        token
    }

    /// Go: `Tree.errorf`: `template: name:line: ...`, formatted as `fmt.Errorf` does.
    fn errorf(&self, format: &str, args: &[Value]) -> TemplateError {
        let format = format!(
            "template: {}:{}: {format}",
            self.parse_name, self.t.token[0].line
        );
        TemplateError::Parse(errorf(&format, args))
    }

    fn expect(&mut self, expected: ItemType, context: &str) -> Res<Item> {
        let token = self.next_non_space();
        if token.typ != expected {
            return Err(self.unexpected(&token, context));
        }
        Ok(token)
    }

    fn expect_one_of(&mut self, e1: ItemType, e2: ItemType, context: &str) -> Res<Item> {
        let token = self.next_non_space();
        if token.typ != e1 && token.typ != e2 {
            return Err(self.unexpected(&token, context));
        }
        Ok(token)
    }

    /// Go: `unexpected`: a lexer error, or `unexpected <token> in <context>`.
    fn unexpected(&self, token: &Item, context: &str) -> TemplateError {
        if token.typ == ItemType::Error {
            let mut extra = String::new();
            if self.t.action_line != 0 && self.t.action_line != token.line {
                extra = format!(
                    " in action started at {}:{}",
                    self.parse_name, self.t.action_line
                );
                if token.val.ends_with(" action") {
                    extra = extra.split_off(" in action".len()); // avoid "action in action"
                }
            }
            return self.errorf("%s%s", &[text(&token.describe()), text(&extra)]);
        }
        self.errorf(
            "unexpected %s in %s",
            &[text(&token.describe()), text(context)],
        )
    }

    /// Go: `Tree.add`: a non-empty tree may not replace another non-empty one.
    fn add(&mut self, root: List) -> Res<()> {
        let name = self.t.name.clone();
        if let Some(tree) = self.trees.get(&name)
            && !is_empty_tree(&tree.root)
        {
            if !is_empty_tree(&root) {
                return Err(self.errorf(
                    "template: multiple definition of template %q",
                    &[text(&name)],
                ));
            }
            return Ok(());
        }
        let _previous = self.trees.insert(name, Tree { root });
        Ok(())
    }

    /// Go: `Tree.parse`: the top-level list, with `{{define}}` clauses parsed into their own
    /// trees.
    fn parse_body(&mut self) -> Res<List> {
        let mut root = List {
            pos: self.peek().pos,
            nodes: Vec::new(),
        };
        while self.peek().typ != ItemType::Eof {
            if self.peek().typ == ItemType::LeftDelim {
                let delim = self.next();
                if self.next_non_space().typ == ItemType::Define {
                    let outer = mem::replace(&mut self.t, TreeState::new("definition"));
                    let defined = self.parse_definition();
                    self.t = outer;
                    defined?;
                    continue;
                }
                self.backup2(delim);
            }
            let n = self.text_or_action()?;
            if matches!(n, Node::End { .. } | Node::Else { .. }) {
                return Err(self.errorf("unexpected %s", &[text(&n.string())]));
            }
            root.nodes.push(n);
        }
        Ok(root)
    }

    /// Go: `parseDefinition`.
    fn parse_definition(&mut self) -> Res<()> {
        const CONTEXT: &str = "define clause";
        let name = self.expect_one_of(ItemType::String, ItemType::RawString, CONTEXT)?;
        self.t.name = match unquote(&name.val) {
            Ok(name) => name,
            Err(e) => return Err(self.errorf("%s", &[text(&e.to_string())])),
        };
        let _delim = self.expect(ItemType::RightDelim, CONTEXT)?;
        let (root, end) = self.item_list()?;
        if !matches!(end, Node::End { .. }) {
            return Err(self.errorf("unexpected %s in %s", &[text(&end.string()), text(CONTEXT)]));
        }
        self.add(root)
    }

    /// Go: `itemList`: nodes up to an `{{end}}` or `{{else}}`, which is returned too.
    fn item_list(&mut self) -> Res<(List, Node)> {
        let mut list = List {
            pos: self.peek_non_space().pos,
            nodes: Vec::new(),
        };
        while self.peek_non_space().typ != ItemType::Eof {
            let n = self.text_or_action()?;
            if matches!(n, Node::End { .. } | Node::Else { .. }) {
                return Ok((list, n));
            }
            list.nodes.push(n);
        }
        Err(self.errorf("unexpected EOF", &[]))
    }

    fn text_or_action(&mut self) -> Res<Node> {
        let token = self.next_non_space();
        if token.typ == ItemType::Text {
            return Ok(Node::Text {
                pos: token.pos,
                text: token.val,
            });
        }
        if token.typ != ItemType::LeftDelim {
            return Err(self.unexpected(&token, "input"));
        }
        self.t.action_line = token.line;
        let action = self.action();
        self.t.action_line = 0;
        action
    }

    /// Go: `action`: a control structure or a pipeline.
    fn action(&mut self) -> Res<Node> {
        let token = self.next_non_space();
        let typ = token.typ;
        if typ == ItemType::Block {
            return self.block_control();
        }
        if typ == ItemType::Break || typ == ItemType::Continue {
            return self.break_control(token.pos, typ == ItemType::Break);
        }
        if typ == ItemType::Else {
            return self.else_control();
        }
        if typ == ItemType::End {
            let pos = self.expect(ItemType::RightDelim, "end")?.pos;
            return Ok(Node::End { pos });
        }
        if typ == ItemType::If {
            return Ok(Node::If(self.parse_control("if")?));
        }
        if typ == ItemType::Range {
            return Ok(Node::Range(self.parse_control("range")?));
        }
        if typ == ItemType::Template {
            return self.template_control();
        }
        if typ == ItemType::With {
            return Ok(Node::With(self.parse_control("with")?));
        }
        self.backup();
        let start = self.peek();
        let pipe = self.pipeline("command", ItemType::RightDelim)?;
        Ok(Node::Action {
            pos: start.pos,
            pipe,
        })
    }

    /// Go: `breakControl` and `continueControl`.
    fn break_control(&mut self, pos: usize, is_break: bool) -> Res<Node> {
        let (keyword, node) = if is_break {
            ("{{break}}", Node::Break { pos })
        } else {
            ("{{continue}}", Node::Continue { pos })
        };
        let token = self.next_non_space();
        if token.typ != ItemType::RightDelim {
            return Err(self.unexpected(&token, keyword));
        }
        if self.t.range_depth == 0 {
            return Err(self.errorf(&format!("{keyword} outside {{{{range}}}}"), &[]));
        }
        Ok(node)
    }

    /// Go: `pipeline`.
    fn pipeline(&mut self, context: &str, end: ItemType) -> Res<Pipe> {
        let token = self.peek_non_space();
        let mut pipe = Pipe {
            pos: token.pos,
            ..Pipe::default()
        };
        self.declarations(&mut pipe, context)?;
        loop {
            let next = self.next_non_space();
            if next.typ == end {
                // At this point, the pipeline is complete
                self.check_pipeline(&pipe, context)?;
                return Ok(pipe);
            }
            if !matches!(
                next.typ,
                ItemType::Bool
                    | ItemType::CharConstant
                    | ItemType::Complex
                    | ItemType::Dot
                    | ItemType::Field
                    | ItemType::Identifier
                    | ItemType::Number
                    | ItemType::Nil
                    | ItemType::RawString
                    | ItemType::String
                    | ItemType::Variable
                    | ItemType::LeftParen
            ) {
                return Err(self.unexpected(&next, context));
            }
            self.backup();
            let command = self.command()?;
            pipe.cmds.push(command);
        }
    }

    /// The `decls:` part of Go's `pipeline`: `$x :=`, `$x =`, and `$i, $e :=` for range.
    fn declarations(&mut self, pipe: &mut Pipe, context: &str) -> Res<()> {
        loop {
            let v = self.peek_non_space();
            if v.typ != ItemType::Variable {
                return Ok(());
            }
            let _variable = self.next();
            // Since space is a token, we need 3-token look-ahead here in the worst case: in
            // "$x foo" we need to read "foo" (as opposed to ":=") to know that $x is an
            // argument variable rather than a declaration.
            let token_after_variable = self.peek();
            let next = self.peek_non_space();
            if next.typ == ItemType::Assign || next.typ == ItemType::Declare {
                pipe.is_assign = next.typ == ItemType::Assign;
                let _assign = self.next_non_space();
                pipe.decl.push(Decl {
                    name: v.val.clone(),
                });
                self.t.vars.push(v.val);
                return Ok(());
            }
            if next.typ == ItemType::Char && next.val == "," {
                let _comma = self.next_non_space();
                pipe.decl.push(Decl {
                    name: v.val.clone(),
                });
                self.t.vars.push(v.val);
                if context == "range" && pipe.decl.len() < 2 {
                    if matches!(
                        self.peek_non_space().typ,
                        ItemType::Variable | ItemType::RightDelim | ItemType::RightParen
                    ) {
                        // second initialized variable in a range pipeline
                        continue;
                    }
                    return Err(self.errorf("range can only initialize variables", &[]));
                }
                return Err(self.errorf("too many declarations in %s", &[text(context)]));
            }
            if token_after_variable.typ == ItemType::Space {
                self.backup3(v, token_after_variable);
            } else {
                self.backup2(v);
            }
            return Ok(());
        }
    }

    /// Go: `checkPipeline`.
    fn check_pipeline(&self, pipe: &Pipe, context: &str) -> Res<()> {
        // Reject empty pipelines
        if pipe.cmds.is_empty() {
            return Err(self.errorf("missing value for %s", &[text(context)]));
        }
        // Only the first command of a pipeline can start with a non executable operand
        for (i, c) in pipe.cmds.iter().enumerate().skip(1) {
            if matches!(
                c.args.first(),
                Some(
                    Arg::Bool { .. }
                        | Arg::Dot { .. }
                        | Arg::Nil { .. }
                        | Arg::Number(_)
                        | Arg::String { .. }
                )
            ) {
                let stage = Value::Int(i64::try_from(i + 1).unwrap_or(i64::MAX));
                return Err(self.errorf("non executable command in pipeline stage %d", &[stage]));
            }
        }
        Ok(())
    }

    /// Go: `parseControl` for `if`, `range` and `with`.
    fn parse_control(&mut self, context: &str) -> Res<Branch> {
        if self.nesting >= MAX_NESTING {
            return Err(self.errorf(NESTING_ERROR, &[]));
        }
        let vars = self.t.vars.len();
        self.nesting += 1;
        let branch = self.control_body(context);
        self.nesting -= 1;
        self.t.vars.truncate(vars);
        branch
    }

    fn control_body(&mut self, context: &str) -> Res<Branch> {
        let pipe = self.pipeline(context, ItemType::RightDelim)?;
        if context == "range" {
            self.t.range_depth += 1;
        }
        let listed = self.item_list();
        if context == "range" {
            self.t.range_depth -= 1;
        }
        let (list, next) = listed?;
        let mut else_list = None;
        if let Node::Else { pos } = next {
            // Special case for "else if" and "else with": treat as if the next action began
            // with that keyword.
            if context == "if" && self.peek().typ == ItemType::If {
                let _if = self.next();
                let nested = Node::If(self.parse_control("if")?);
                else_list = Some(List {
                    pos,
                    nodes: vec![nested],
                });
            } else if context == "with" && self.peek().typ == ItemType::With {
                let _with = self.next();
                let nested = Node::With(self.parse_control("with")?);
                else_list = Some(List {
                    pos,
                    nodes: vec![nested],
                });
            } else {
                let (else_items, else_end) = self.item_list()?;
                if !matches!(else_end, Node::End { .. }) {
                    return Err(self.errorf("expected end; found %s", &[text(&else_end.string())]));
                }
                else_list = Some(else_items);
            }
        }
        Ok(Branch {
            pos: pipe.pos,
            pipe,
            list,
            else_list,
        })
    }

    /// Go: `elseControl`.
    fn else_control(&mut self) -> Res<Node> {
        let peek = self.peek_non_space();
        // The "{{else if ... " and "{{else with ..." will be treated as "{{else}}{{if ..." and
        // "{{else}}{{with ...". So return the else node here.
        if peek.typ == ItemType::If || peek.typ == ItemType::With {
            return Ok(Node::Else { pos: peek.pos });
        }
        let token = self.expect(ItemType::RightDelim, "else")?;
        Ok(Node::Else { pos: token.pos })
    }

    /// Go: `blockControl`: `{{block "name" pipeline}}` defines a template and invokes it.
    fn block_control(&mut self) -> Res<Node> {
        const CONTEXT: &str = "block clause";
        let token = self.next_non_space();
        let name = self.parse_template_name(&token, CONTEXT)?;
        let pipe = self.pipeline(CONTEXT, ItemType::RightDelim)?;
        if self.nesting >= MAX_NESTING {
            return Err(self.errorf(NESTING_ERROR, &[]));
        }
        let outer = mem::replace(&mut self.t, TreeState::new(&name));
        self.nesting += 1;
        let listed = self.item_list();
        self.nesting -= 1;
        let block = mem::replace(&mut self.t, outer);
        let (root, end) = listed?;
        if !matches!(end, Node::End { .. }) {
            return Err(self.errorf("unexpected %s in %s", &[text(&end.string()), text(CONTEXT)]));
        }
        let parent = mem::replace(&mut self.t, block);
        let added = self.add(root);
        self.t = parent;
        added?;
        Ok(Node::Template {
            pos: token.pos,
            name,
            pipe: Some(pipe),
        })
    }

    /// Go: `templateControl`.
    fn template_control(&mut self) -> Res<Node> {
        const CONTEXT: &str = "template clause";
        let token = self.next_non_space();
        let name = self.parse_template_name(&token, CONTEXT)?;
        let pipe = if self.next_non_space().typ == ItemType::RightDelim {
            None
        } else {
            self.backup();
            // Do not pop variables; they persist until "end".
            Some(self.pipeline(CONTEXT, ItemType::RightDelim)?)
        };
        Ok(Node::Template {
            pos: token.pos,
            name,
            pipe,
        })
    }

    fn parse_template_name(&self, token: &Item, context: &str) -> Res<String> {
        if !matches!(token.typ, ItemType::String | ItemType::RawString) {
            return Err(self.unexpected(token, context));
        }
        unquote(&token.val).map_err(|e| self.errorf("%s", &[text(&e.to_string())]))
    }

    /// Go: `command`: space-separated operands up to a pipe or the end of the pipeline.
    fn command(&mut self) -> Res<Command> {
        let _start = self.peek_non_space();
        let mut cmd = Command { args: Vec::new() };
        loop {
            let _space = self.peek_non_space(); // skip leading spaces.
            if let Some(operand) = self.operand()? {
                cmd.args.push(operand);
            }
            let token = self.next();
            if token.typ == ItemType::Space {
                continue;
            }
            if token.typ == ItemType::RightDelim || token.typ == ItemType::RightParen {
                self.backup();
            } else if token.typ != ItemType::Pipe {
                return Err(self.unexpected(&token, "operand"));
            }
            break;
        }
        if cmd.args.is_empty() {
            return Err(self.errorf("empty command", &[]));
        }
        Ok(cmd)
    }

    /// Go: `operand`: a term possibly followed by field accesses.
    fn operand(&mut self) -> Res<Option<Arg>> {
        let Some(node) = self.term()? else {
            return Ok(None);
        };
        if self.peek().typ != ItemType::Field {
            return Ok(Some(node));
        }
        let pos = self.peek().pos;
        let mut field = Vec::new();
        while self.peek().typ == ItemType::Field {
            let val = self.next().val;
            field.push(val.get(1..).unwrap_or_default().to_owned());
        }
        // Compatibility with original API: If the term is of type NodeField or
        // NodeVariable, just put more fields on the original. Otherwise, keep the Chain node.
        match node {
            Arg::Field { .. } | Arg::Variable { .. } => {
                let joined = Arg::Chain {
                    pos,
                    node: Box::new(node.clone()),
                    field,
                }
                .string();
                if matches!(node, Arg::Field { .. }) {
                    let ident = split_dots(joined.get(1..).unwrap_or_default());
                    return Ok(Some(Arg::Field { pos, ident }));
                }
                Ok(Some(Arg::Variable {
                    pos,
                    ident: split_dots(&joined),
                }))
            }
            Arg::Bool { .. }
            | Arg::String { .. }
            | Arg::Number(_)
            | Arg::Nil { .. }
            | Arg::Dot { .. } => {
                Err(self.errorf("unexpected . after term %q", &[text(&node.string())]))
            }
            Arg::Identifier { .. } | Arg::Chain { .. } | Arg::Pipe(_) => Ok(Some(Arg::Chain {
                pos,
                node: Box::new(node),
                field,
            })),
        }
    }

    /// Go: `term`.
    fn term(&mut self) -> Res<Option<Arg>> {
        let token = self.next_non_space();
        let pos = token.pos;
        let arg = match token.typ {
            ItemType::Identifier => {
                if !BUILTINS.contains(&token.val.as_str()) {
                    return Err(self.errorf("function %q not defined", &[text(&token.val)]));
                }
                Arg::Identifier {
                    pos,
                    ident: token.val,
                }
            }
            ItemType::Dot => Arg::Dot { pos },
            ItemType::Nil => Arg::Nil { pos },
            ItemType::Variable => self.use_var(pos, &token.val)?,
            ItemType::Field => Arg::Field {
                pos,
                ident: split_dots(token.val.get(1..).unwrap_or_default()),
            },
            ItemType::Bool => Arg::Bool {
                pos,
                value: token.val == "true",
            },
            ItemType::CharConstant | ItemType::Complex | ItemType::Number => {
                let char_constant = token.typ == ItemType::CharConstant;
                let complex = token.typ == ItemType::Complex;
                Number::parse(pos, &token.val, char_constant, complex)
                    .map(Arg::Number)
                    .map_err(|e| self.errorf("%s", &[text(&e)]))?
            }
            ItemType::LeftParen => {
                if self.t.stack_depth >= MAX_STACK_DEPTH {
                    return Err(self.errorf("max expression depth exceeded", &[]));
                }
                self.t.stack_depth += 1;
                let pipe = self.pipeline("parenthesized pipeline", ItemType::RightParen);
                self.t.stack_depth -= 1;
                Arg::Pipe(Box::new(pipe?))
            }
            ItemType::String | ItemType::RawString => {
                let s =
                    unquote(&token.val).map_err(|e| self.errorf("%s", &[text(&e.to_string())]))?;
                Arg::String {
                    pos,
                    quoted: token.val,
                    text: s,
                }
            }
            ItemType::Error
            | ItemType::Char
            | ItemType::Comment
            | ItemType::Assign
            | ItemType::Declare
            | ItemType::Eof
            | ItemType::LeftDelim
            | ItemType::Pipe
            | ItemType::RightDelim
            | ItemType::RightParen
            | ItemType::Space
            | ItemType::Text
            | ItemType::Keyword
            | ItemType::Block
            | ItemType::Break
            | ItemType::Continue
            | ItemType::Define
            | ItemType::Else
            | ItemType::End
            | ItemType::If
            | ItemType::Range
            | ItemType::Template
            | ItemType::With => {
                self.backup();
                return Ok(None);
            }
        };
        Ok(Some(arg))
    }

    /// Go: `useVar`.
    fn use_var(&self, pos: usize, name: &str) -> Res<Arg> {
        let ident = split_dots(name);
        let first = ident.first().cloned().unwrap_or_default();
        if self.t.vars.contains(&first) {
            return Ok(Arg::Variable { pos, ident });
        }
        Err(self.errorf("undefined variable %q", &[text(&first)]))
    }
}
