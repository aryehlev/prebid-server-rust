//! Go: `text/template/exec.go`: the walk over a parsed tree, with Go's `at` bookkeeping (the
//! node an error names) and error texts.

use std::{borrow::Cow, mem, slice};

use super::{
    Template, TemplateError,
    node::{Arg, Branch, Command, List, Node, Number, Pipe},
};
use crate::fmt::{MethodKind, Value, errorf, sprintf, sprintf_bytes};

/// Go: `maxExecDepth`, which error texts name.
const GO_MAX_EXEC_DEPTH: i64 = 100_000;
/// The nesting of template invocations this port allows before failing with Go's text.
const MAX_EXEC_DEPTH: usize = 100;

/// Go: a `reflect.Value` during execution.
#[derive(Debug, Clone)]
enum Val<'d> {
    /// Go: the zero `reflect.Value` (no value, printed `<no value>`).
    Invalid,
    /// Go: `missingVal`, the absent final argument of a command.
    Missing,
    /// A value, borrowed from the data when possible.
    V(Cow<'d, Value>),
}

impl Val<'_> {
    fn into_static(self) -> Val<'static> {
        match self {
            Self::Invalid => Val::Invalid,
            Self::Missing => Val::Missing,
            Self::V(v) => Val::V(Cow::Owned(v.into_owned())),
        }
    }

    const fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

const fn owned(v: Value) -> Val<'static> {
    Val::V(Cow::Owned(v))
}

fn text(s: &str) -> Value {
    Value::from(s)
}

fn int(n: usize) -> Value {
    Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

/// Why a walk stopped: an error, or Go's `walkBreak`/`walkContinue` panics.
#[derive(Debug)]
enum Flow {
    Fail(TemplateError),
    Break,
    Continue,
}

type Res<T> = Result<T, Flow>;

/// A failed lookup: Go's `errorf` format and arguments.
type Failure = (&'static str, Vec<Value>);

/// Go: the node `state.at` records, for error context.
#[derive(Debug, Clone, Copy)]
enum At<'t> {
    None,
    Node(&'t Node),
    List(&'t List),
    Pipe(&'t Pipe),
    Arg(&'t Arg),
}

impl At<'_> {
    fn pos_and_string(self) -> Option<(usize, String)> {
        Some(match self {
            Self::None => return None,
            Self::Node(n) => (n.pos(), n.string()),
            Self::List(l) => (l.pos, l.string()),
            Self::Pipe(p) => (p.pos, p.string()),
            Self::Arg(a) => (a.pos(), a.string()),
        })
    }
}

/// Go: `doublePercent`.
fn double_percent(s: &str) -> String {
    s.replace('%', "%%")
}

/// Go: `Template.execute`.
pub(super) fn execute(tmpl: &Template, data: &Value) -> Result<Vec<u8>, TemplateError> {
    let dot = if matches!(data, Value::Nil) {
        Val::Invalid
    } else {
        Val::V(Cow::Borrowed(data))
    };
    let mut state = State {
        tmpl,
        name: &tmpl.name,
        node: At::None,
        vars: vec![("$", dot.clone())],
        depth: 0,
        out: Vec::new(),
    };
    let Some(tree) = tmpl.trees.get(&tmpl.name) else {
        return Err(state.exec_error("%q is an incomplete or empty template", &[text(&tmpl.name)]));
    };
    match state.walk_list(&dot, &tree.root) {
        // Go panics with walkBreak/walkContinue outside a range; the parser rejects those.
        Ok(()) | Err(Flow::Break | Flow::Continue) => Ok(state.out),
        Err(Flow::Fail(e)) => Err(e),
    }
}

/// Go: `state`.
struct State<'t, 'd> {
    tmpl: &'t Template,
    name: &'t str,
    node: At<'t>,
    vars: Vec<(&'t str, Val<'d>)>,
    depth: usize,
    out: Vec<u8>,
}

impl<'t, 'd> State<'t, 'd> {
    /// Go: `Tree.ErrorContext`: `NAME:LINE:COL` (COL a byte offset) and the node's text.
    fn error_context(&self, pos: usize) -> String {
        let before = self.tmpl.text.get(..pos).unwrap_or_default();
        let byte_num = before.rfind('\n').map_or(pos, |i| pos - (i + 1));
        let line_num = 1 + before.bytes().filter(|&b| b == b'\n').count();
        format!("{}:{line_num}:{byte_num}", self.tmpl.name)
    }

    /// Go's `executing` prefix for the current node.
    fn prefix(&self, format: &str) -> String {
        let name = text(&double_percent(self.name));
        match self.node.pos_and_string() {
            None => sprintf("template: %s: %s", &[name, text(format)]),
            Some((pos, context)) => sprintf(
                "template: %s: executing %q at <%s>: %s",
                &[
                    text(&self.error_context(pos)),
                    name,
                    text(&double_percent(&context)),
                    text(format),
                ],
            ),
        }
    }

    /// Go: `state.errorf`'s `ExecError`.
    fn exec_error(&self, format: &str, args: &[Value]) -> TemplateError {
        TemplateError::Exec {
            name: self.name.to_owned(),
            message: errorf(&self.prefix(format), args),
        }
    }

    /// Go: `state.errorf`.
    fn errorf(&self, format: &str, args: &[Value]) -> Flow {
        Flow::Fail(self.exec_error(format, args))
    }

    fn unsupported(&self, what: &str) -> Flow {
        let message = self.prefix(&format!("{} is not supported", double_percent(what)));
        Flow::Fail(TemplateError::Unsupported(errorf(&message, &[])))
    }

    fn fail_with(&self, (format, args): Failure) -> Flow {
        self.errorf(format, &args)
    }

    /// Go: `state.varValue`.
    fn var_value(&self, name: &str) -> Res<Val<'d>> {
        match self.vars.iter().rev().find(|(n, _)| *n == name) {
            Some((_, v)) => Ok(v.clone()),
            None => Err(self.errorf("undefined variable: %s", &[text(name)])),
        }
    }

    /// Go: `state.setVar`.
    fn set_var(&mut self, name: &str, value: Val<'d>) -> Res<()> {
        if let Some((_, v)) = self.vars.iter_mut().rev().find(|(n, _)| *n == name) {
            *v = value;
            return Ok(());
        }
        Err(self.errorf("undefined variable: %s", &[text(name)]))
    }

    /// Go: `state.setTopVar`.
    fn set_top_var(&mut self, n: usize, value: Val<'d>) {
        let i = self.vars.len().saturating_sub(n);
        if let Some((_, v)) = self.vars.get_mut(i) {
            *v = value;
        }
    }

    /// Go: `walk` for a `ListNode`.
    fn walk_list(&mut self, dot: &Val<'d>, list: &'t List) -> Res<()> {
        self.node = At::List(list);
        for node in &list.nodes {
            self.walk(dot, node)?;
        }
        Ok(())
    }

    /// Go: `walk`.
    fn walk(&mut self, dot: &Val<'d>, node: &'t Node) -> Res<()> {
        self.node = At::Node(node);
        match node {
            Node::Action { pipe, .. } => {
                // Do not pop variables so they persist until next end. Also, if the action
                // declares variables, don't print the result.
                let val = self.eval_pipeline(dot, Some(pipe))?;
                if pipe.decl.is_empty() {
                    self.print_value(node, &val);
                }
                Ok(())
            }
            Node::Break { .. } => Err(Flow::Break),
            Node::Continue { .. } => Err(Flow::Continue),
            Node::If(b) => self.walk_if_or_with(false, dot, b),
            Node::With(b) => self.walk_if_or_with(true, dot, b),
            Node::Range(b) => self.walk_range(dot, node, b),
            Node::Template { name, pipe, .. } => self.walk_template(dot, node, name, pipe.as_ref()),
            Node::Text { text, .. } => {
                self.out.extend_from_slice(text.as_bytes());
                Ok(())
            }
            Node::End { .. } | Node::Else { .. } => {
                Err(self.errorf("unknown node: %s", &[text(&node.string())]))
            }
        }
    }

    /// Go: `walkIfOrWith`.
    fn walk_if_or_with(&mut self, with: bool, dot: &Val<'d>, b: &'t Branch) -> Res<()> {
        let mark = self.vars.len();
        let walked = self.if_or_with_body(with, dot, b);
        self.vars.truncate(mark);
        walked
    }

    fn if_or_with_body(&mut self, with: bool, dot: &Val<'d>, b: &'t Branch) -> Res<()> {
        let val = self.eval_pipeline(dot, Some(&b.pipe))?;
        if is_true(&indirect_interface(val.clone())) {
            if with {
                return self.walk_list(&val, &b.list);
            }
            return self.walk_list(dot, &b.list);
        }
        if let Some(else_list) = &b.else_list {
            return self.walk_list(dot, else_list);
        }
        Ok(())
    }

    /// Go: `walkRange`.
    fn walk_range(&mut self, dot: &Val<'d>, node: &'t Node, r: &'t Branch) -> Res<()> {
        self.node = At::Node(node);
        let mark = self.vars.len();
        let walked = self.range_body(dot, r);
        self.vars.truncate(mark);
        match walked {
            Err(Flow::Break) => Ok(()),
            other => other,
        }
    }

    fn range_body(&mut self, dot: &Val<'d>, r: &'t Branch) -> Res<()> {
        let val = indirect(self.eval_pipeline(dot, Some(&r.pipe))?);
        // mark top of stack before any variables in the body are pushed.
        let mark = self.vars.len();
        let items: Vec<(Val<'d>, Val<'d>)> = match val {
            Val::Invalid => Vec::new(), // An invalid value is likely a nil map, etc.
            Val::Missing => return Err(self.cannot_range(&Value::Nil)),
            Val::V(Cow::Borrowed(v)) => self.range_items(v, r)?,
            Val::V(Cow::Owned(v)) => {
                let items = self.range_items(&v, r)?;
                items
                    .into_iter()
                    .map(|(i, e)| (i.into_static(), e.into_static()))
                    .collect()
            }
        };
        if items.is_empty() {
            if let Some(else_list) = &r.else_list {
                return self.walk_list(dot, else_list);
            }
            return Ok(());
        }
        for (index, elem) in items {
            self.one_iteration(r, mark, index, &elem)?;
        }
        Ok(())
    }

    fn cannot_range(&self, v: &Value) -> Flow {
        self.errorf("range can't iterate over %v", slice::from_ref(v))
    }

    /// The `(index, element)` pairs `range` visits.
    fn range_items<'x>(&self, v: &'x Value, r: &Branch) -> Res<Vec<(Val<'x>, Val<'x>)>> {
        let index = |i: usize| owned(int(i));
        let elems = |elems: &'x [Value]| -> Vec<(Val<'x>, Val<'x>)> {
            elems
                .iter()
                .enumerate()
                .map(|(i, e)| (index(i), Val::V(Cow::Borrowed(e))))
                .collect()
        };
        Ok(match v.underlying() {
            Value::Array { elems: list, .. }
            | Value::Slice {
                elems: Some(list), ..
            } => elems(list),
            Value::Slice { elems: None, .. }
            | Value::Bytes(None)
            | Value::Map { entries: None, .. } => Vec::new(),
            Value::Bytes(Some(bytes)) => bytes
                .iter()
                .enumerate()
                .map(|(i, &b)| (index(i), owned(Value::Uint8(b))))
                .collect(),
            Value::Map {
                entries: Some(entries),
                ..
            } => {
                let mut sorted: Vec<&(Value, Value)> = entries.iter().collect();
                sorted.sort_by(|a, b| a.0.sort_cmp(&b.0));
                sorted
                    .into_iter()
                    .map(|(k, e)| (Val::V(Cow::Borrowed(k)), Val::V(Cow::Borrowed(e))))
                    .collect()
            }
            Value::Int(_)
            | Value::Int8(_)
            | Value::Int16(_)
            | Value::Int32(_)
            | Value::Int64(_)
            | Value::Uint(_)
            | Value::Uint8(_)
            | Value::Uint16(_)
            | Value::Uint32(_)
            | Value::Uint64(_)
            | Value::Uintptr(_) => {
                if r.pipe.decl.len() > 1 {
                    return Err(self.errorf(
                        "can't use %v to iterate over more than one variable",
                        slice::from_ref(v),
                    ));
                }
                // Pass element as second value, as we do for channels.
                integer_range(v)
                    .into_iter()
                    .map(|e| (Val::Invalid, owned(e)))
                    .collect()
            }
            Value::Nil
            | Value::Bool(_)
            | Value::Float32(_)
            | Value::Float64(_)
            | Value::String(_)
            | Value::Named { .. }
            | Value::Struct { .. }
            | Value::Pointer { .. }
            | Value::Method { .. } => return Err(self.cannot_range(v)),
        })
    }

    /// Go: `oneIteration` in `walkRange`.
    fn one_iteration(
        &mut self,
        r: &'t Branch,
        mark: usize,
        index: Val<'d>,
        elem: &Val<'d>,
    ) -> Res<()> {
        let decl = &r.pipe.decl;
        if let Some(first) = decl.first() {
            if r.pipe.is_assign {
                // With two variables, index comes first. With one, we use the element.
                let value = if decl.len() > 1 {
                    index.clone()
                } else {
                    elem.clone()
                };
                self.set_var(&first.name, value)?;
            } else {
                // Set top var (lexically the second if there are two) to the element.
                self.set_top_var(1, elem.clone());
            }
        }
        if let Some(second) = decl.get(1) {
            if r.pipe.is_assign {
                self.set_var(&second.name, elem.clone())?;
            } else {
                // Set next var (lexically the first if there are two) to the index.
                self.set_top_var(2, index);
            }
        }
        let walked = self.walk_list(elem, &r.list);
        self.vars.truncate(mark);
        match walked {
            Err(Flow::Continue) => Ok(()),
            other => other,
        }
    }

    /// Go: `walkTemplate`.
    fn walk_template(
        &mut self,
        dot: &Val<'d>,
        node: &'t Node,
        name: &str,
        pipe: Option<&'t Pipe>,
    ) -> Res<()> {
        self.node = At::Node(node);
        let tmpl: &'t Template = self.tmpl;
        let Some((tmpl_name, tree)) = tmpl.trees.get_key_value(name) else {
            return Err(self.errorf("template %q not defined", &[text(name)]));
        };
        if self.depth == MAX_EXEC_DEPTH {
            return Err(self.errorf(
                "exceeded maximum template depth (%v)",
                &[Value::Int(GO_MAX_EXEC_DEPTH)],
            ));
        }
        // Variables declared by the pipeline persist.
        let dot = self.eval_pipeline(dot, pipe)?;
        let mut inner = State {
            tmpl,
            name: tmpl_name,
            node: self.node,
            // No dynamic scoping: template invocations inherit no variables.
            vars: vec![("$", dot.clone())],
            depth: self.depth + 1,
            out: mem::take(&mut self.out),
        };
        let walked = inner.walk_list(&dot, &tree.root);
        self.out = inner.out;
        walked
    }

    /// Go: `evalPipeline`.
    fn eval_pipeline(&mut self, dot: &Val<'d>, pipe: Option<&'t Pipe>) -> Res<Val<'d>> {
        let Some(pipe) = pipe else {
            return Ok(Val::Invalid);
        };
        self.node = At::Pipe(pipe);
        let mut value = Val::Missing;
        for cmd in &pipe.cmds {
            // previous value is this one's final arg.
            value = self.eval_command(dot, cmd, &value)?;
            // If the object has type interface{}, dig down one level to the thing inside.
            if matches!(&value, Val::V(v) if matches!(**v, Value::Nil)) {
                value = Val::Invalid;
            }
        }
        for variable in &pipe.decl {
            if pipe.is_assign {
                self.set_var(&variable.name, value.clone())?;
            } else {
                self.vars.push((&variable.name, value.clone()));
            }
        }
        Ok(value)
    }

    /// Go: `notAFunction`.
    fn not_a_function(&self, args: &[Arg], final_: &Val<'d>) -> Res<()> {
        if args.len() > 1 || !final_.is_missing() {
            let first = args.first().map_or_else(String::new, Arg::string);
            return Err(self.errorf("can't give argument to non-function %s", &[text(&first)]));
        }
        Ok(())
    }

    /// Go: `evalCommand`.
    fn eval_command(&mut self, dot: &Val<'d>, cmd: &'t Command, final_: &Val<'d>) -> Res<Val<'d>> {
        let Some(first) = cmd.args.first() else {
            return Err(self.errorf("empty command", &[]));
        };
        match first {
            Arg::Field { ident, .. } => {
                self.node = At::Arg(first);
                return self.eval_field_chain(dot.clone(), ident, &cmd.args, final_);
            }
            Arg::Chain { node, field, .. } => {
                return self.eval_chain(dot, first, node, field, &cmd.args, final_);
            }
            Arg::Identifier { ident, .. } => {
                // Must be a function.
                self.node = At::Arg(first);
                return Err(self.unsupported(&format!("builtin function {}", quote_name(ident))));
            }
            Arg::Pipe(pipe) => {
                // Parenthesized pipeline. The arguments are all inside the pipeline; final
                // must be absent.
                self.not_a_function(&cmd.args, final_)?;
                return self.eval_pipeline(dot, Some(pipe));
            }
            Arg::Variable { ident, .. } => {
                return self.eval_variable(first, ident, &cmd.args, final_);
            }
            Arg::Bool { .. }
            | Arg::Dot { .. }
            | Arg::Nil { .. }
            | Arg::Number(_)
            | Arg::String { .. } => {}
        }
        self.node = At::Arg(first);
        self.not_a_function(&cmd.args, final_)?;
        match first {
            Arg::Bool { value, .. } => Ok(owned(Value::Bool(*value))),
            Arg::Dot { .. } => Ok(dot.clone()),
            Arg::Nil { .. } => Err(self.errorf("nil is not a command", &[])),
            Arg::Number(n) => self.ideal_constant(first, n),
            Arg::String { text: s, .. } => Ok(owned(Value::from(s.as_str()))),
            Arg::Field { .. }
            | Arg::Chain { .. }
            | Arg::Identifier { .. }
            | Arg::Pipe(_)
            | Arg::Variable { .. } => {
                Err(self.errorf("can't evaluate command %q", &[text(&first.string())]))
            }
        }
    }

    /// Go: `idealConstant`: an untyped constant takes the type its syntax implies.
    fn ideal_constant(&mut self, at: &'t Arg, constant: &Number) -> Res<Val<'d>> {
        self.node = At::Arg(at);
        let t = &constant.text;
        if constant.is_complex {
            return Err(self.unsupported("complex constant"));
        }
        if let Some(f) = constant.float64
            && !is_hex_int(t)
            && !t.starts_with('\'')
            && t.contains(['.', 'e', 'E', 'p', 'P'])
        {
            return Ok(owned(Value::Float64(f)));
        }
        if let Some(i) = constant.int64 {
            return Ok(owned(Value::Int(i)));
        }
        if constant.uint64.is_some() {
            return Err(self.errorf("%s overflows int", &[text(t)]));
        }
        Ok(Val::Invalid)
    }

    /// Go: `evalChainNode`.
    fn eval_chain(
        &mut self,
        dot: &Val<'d>,
        chain: &'t Arg,
        node: &'t Arg,
        field: &'t [String],
        args: &'t [Arg],
        final_: &Val<'d>,
    ) -> Res<Val<'d>> {
        self.node = At::Arg(chain);
        if field.is_empty() {
            return Err(self.errorf("internal error: no fields in evalChainNode", &[]));
        }
        if matches!(node, Arg::Nil { .. }) {
            return Err(self.errorf(
                "indirection through explicit nil in %s",
                &[text(&chain.string())],
            ));
        }
        // (pipe).Field1.Field2 has pipe as .Node, fields as .Field. Eval the pipeline, then
        // the fields.
        let pipe = self.eval_arg(dot, node)?;
        self.eval_field_chain(pipe, field, args, final_)
    }

    /// Go: `evalArg` with no target type (the operand of a chain).
    fn eval_arg(&mut self, dot: &Val<'d>, n: &'t Arg) -> Res<Val<'d>> {
        self.node = At::Arg(n);
        match n {
            Arg::Dot { .. } => Ok(dot.clone()),
            Arg::Field { ident, .. } => {
                self.node = At::Arg(n);
                self.eval_field_chain(dot.clone(), ident, slice::from_ref(n), &Val::Missing)
            }
            Arg::Variable { ident, .. } => self.eval_variable(n, ident, &[], &Val::Missing),
            Arg::Pipe(pipe) => self.eval_pipeline(dot, Some(pipe)),
            Arg::Identifier { ident, .. } => {
                Err(self.unsupported(&format!("builtin function {}", quote_name(ident))))
            }
            Arg::Chain { node, field, .. } => {
                self.eval_chain(dot, n, node, field, &[], &Val::Missing)
            }
            Arg::Bool { value, .. } => Ok(owned(Value::Bool(*value))),
            Arg::Number(number) => self.ideal_constant(n, number),
            Arg::String { text: s, .. } => Ok(owned(Value::from(s.as_str()))),
            Arg::Nil { .. } => Err(self.errorf("cannot assign nil to %s", &[text("<nil>")])),
        }
    }

    /// Go: `evalVariableNode`.
    fn eval_variable(
        &mut self,
        variable: &'t Arg,
        ident: &'t [String],
        args: &'t [Arg],
        final_: &Val<'d>,
    ) -> Res<Val<'d>> {
        // $x.Field has $x as the first ident, Field as the second. Eval the var, then the
        // fields.
        self.node = At::Arg(variable);
        let name = ident.first().map_or("", String::as_str);
        let value = self.var_value(name)?;
        let fields = ident.get(1..).unwrap_or_default();
        if fields.is_empty() {
            self.not_a_function(args, final_)?;
            return Ok(value);
        }
        self.eval_field_chain(value, fields, args, final_)
    }

    /// Go: `evalFieldChain`: `.X.Y.Z`, where only the last field gets the arguments.
    fn eval_field_chain(
        &self,
        mut receiver: Val<'d>,
        ident: &'t [String],
        args: &'t [Arg],
        final_: &Val<'d>,
    ) -> Res<Val<'d>> {
        let Some((last, init)) = ident.split_last() else {
            return Ok(receiver);
        };
        for name in init {
            receiver = self.eval_field(name, 0, receiver)?;
        }
        // The command's operands after the field itself, plus the piped-in value.
        let num_in = args.len().saturating_sub(1) + usize::from(!final_.is_missing());
        self.eval_field(last, num_in, receiver)
    }

    /// Go: `evalField`.
    fn eval_field(&self, name: &str, num_in: usize, receiver: Val<'d>) -> Res<Val<'d>> {
        match receiver {
            Val::Invalid => Ok(Val::Invalid),
            Val::Missing => Err(self.errorf(
                "can't evaluate field %s in type %s",
                &[text(name), text("template.missingValType")],
            )),
            Val::V(Cow::Borrowed(v)) => field(v, name, num_in).map_err(|f| self.fail_with(f)),
            Val::V(Cow::Owned(v)) => field(&v, name, num_in)
                .map(Val::into_static)
                .map_err(|f| self.fail_with(f)),
        }
    }

    /// Go: `printValue`.
    fn print_value(&mut self, node: &'t Node, val: &Val<'d>) {
        self.node = At::Node(node);
        match val {
            Val::Invalid | Val::Missing => self.out.extend_from_slice(b"<no value>"),
            Val::V(v) => {
                let printed = sprintf_bytes("%v", slice::from_ref(printable(v)));
                self.out.extend_from_slice(&printed);
            }
        }
    }
}

/// `%q` of a function name.
fn quote_name(name: &str) -> String {
    sprintf("%q", &[text(name)])
}

/// Go: `isHexInt`.
fn is_hex_int(s: &str) -> bool {
    let b = s.as_bytes();
    matches!(b, [b'0', b'x' | b'X', _, ..]) && !s.contains(['p', 'P'])
}

/// Go: the type `reflect` reports for a value; a nil interface is `interface {}`.
fn type_of(v: &Value) -> &str {
    if matches!(v, Value::Nil) {
        return "interface {}";
    }
    v.type_name()
}

/// Go: `indirect`: through pointers, stopping at a nil pointer or nil interface.
fn indirect_ref(v: &Value) -> (&Value, bool) {
    let mut cur = v;
    loop {
        match cur.underlying() {
            Value::Pointer { elem: Some(e), .. } => cur = e,
            Value::Pointer { elem: None, .. } | Value::Nil => return (cur, true),
            Value::Bool(_)
            | Value::Int(_)
            | Value::Int8(_)
            | Value::Int16(_)
            | Value::Int32(_)
            | Value::Int64(_)
            | Value::Uint(_)
            | Value::Uint8(_)
            | Value::Uint16(_)
            | Value::Uint32(_)
            | Value::Uint64(_)
            | Value::Uintptr(_)
            | Value::Float32(_)
            | Value::Float64(_)
            | Value::String(_)
            | Value::Bytes(_)
            | Value::Named { .. }
            | Value::Slice { .. }
            | Value::Array { .. }
            | Value::Map { .. }
            | Value::Struct { .. }
            | Value::Method { .. } => return (cur, false),
        }
    }
}

/// Go: `indirect` on an execution value (the `isNil` result unused).
fn indirect(v: Val<'_>) -> Val<'_> {
    match v {
        Val::V(Cow::Borrowed(b)) => Val::V(Cow::Borrowed(indirect_ref(b).0)),
        Val::V(Cow::Owned(o)) => Val::V(Cow::Owned(indirect_ref(&o).0.clone())),
        other @ (Val::Invalid | Val::Missing) => other,
    }
}

/// Go: `indirectInterface`: a nil interface is no value.
fn indirect_interface(v: Val<'_>) -> Val<'_> {
    if matches!(&v, Val::V(c) if matches!(**c, Value::Nil)) {
        return Val::Invalid;
    }
    v
}

/// Go: the `Error`/`String` method a value (or what it points to) has, and its result.
fn find_method<'x>(v: &'x Value, name: &str) -> Option<&'x str> {
    let mut cur = v;
    loop {
        if let Value::Method { kind, text, value } = cur {
            let method = match kind {
                MethodKind::Error => "Error",
                MethodKind::Stringer => "String",
            };
            if method == name {
                return Some(text);
            }
            cur = value;
        } else if let Value::Pointer { elem: Some(e), .. } = cur {
            cur = e;
        } else {
            return None;
        }
    }
}

/// The key type of a `map[K]V` type name.
fn map_key_type(ty: &str) -> Option<&str> {
    let rest = ty.strip_prefix("map[")?;
    let mut depth = 0_usize;
    for (i, c) in rest.char_indices() {
        match c {
            '[' => depth += 1,
            ']' if depth == 0 => return rest.get(..i),
            ']' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Go: `reflect.TypeOf("").AssignableTo(mapType.Key())`.
fn string_key_map(ty: &str, entries: Option<&Vec<(Value, Value)>>) -> bool {
    map_key_type(ty).map_or_else(
        // A defined map type: judge by its keys.
        || {
            entries
                .and_then(|e| e.first())
                .is_none_or(|(k, _)| matches!(k, Value::String(_)))
        },
        |key| key == "string" || key == "interface {}",
    )
}

/// Go: `evalField` on a valid receiver.
fn field<'x>(receiver: &'x Value, name: &str, num_in: usize) -> Result<Val<'x>, Failure> {
    let typ = type_of(receiver).to_owned();
    let (r, is_nil) = indirect_ref(receiver);
    if is_nil && matches!(r, Value::Nil) {
        // Calling a method on a nil interface can't work.
        return Err(("nil pointer evaluating %s.%s", vec![text(&typ), text(name)]));
    }
    if let Some(result) = find_method(receiver, name) {
        if num_in != 0 {
            return Err((
                "wrong number of args for %s: want %d got %d",
                vec![text(name), Value::Int(0), int(num_in)],
            ));
        }
        return Ok(owned(Value::from(result)));
    }
    let has_args = num_in > 0;
    // It's not a method; must be a field of a struct or an element of a map.
    match r.underlying() {
        Value::Struct { fields, .. } => {
            if let Some((_, value)) = fields.iter().find(|(n, _)| n == name) {
                if !name.chars().next().is_some_and(char::is_uppercase) {
                    return Err((
                        "%s is an unexported field of struct type %s",
                        vec![text(name), text(&typ)],
                    ));
                }
                if has_args {
                    return Err((
                        "%s has arguments but cannot be invoked as function",
                        vec![text(name)],
                    ));
                }
                return Ok(Val::V(Cow::Borrowed(value)));
            }
        }
        Value::Map { ty, entries } if string_key_map(ty, entries.as_ref()) => {
            if has_args {
                return Err(("%s is not a method but has arguments", vec![text(name)]));
            }
            let found = entries
                .iter()
                .flatten()
                .find(|(k, _)| matches!(k, Value::String(s) if s == name));
            return Ok(found.map_or(Val::Invalid, |(_, v)| Val::V(Cow::Borrowed(v))));
        }
        Value::Pointer { .. } => {
            return Err(("nil pointer evaluating %s.%s", vec![text(&typ), text(name)]));
        }
        Value::Nil
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Int8(_)
        | Value::Int16(_)
        | Value::Int32(_)
        | Value::Int64(_)
        | Value::Uint(_)
        | Value::Uint8(_)
        | Value::Uint16(_)
        | Value::Uint32(_)
        | Value::Uint64(_)
        | Value::Uintptr(_)
        | Value::Float32(_)
        | Value::Float64(_)
        | Value::String(_)
        | Value::Bytes(_)
        | Value::Named { .. }
        | Value::Slice { .. }
        | Value::Array { .. }
        | Value::Map { .. }
        | Value::Method { .. } => {}
    }
    Err((
        "can't evaluate field %s in type %s",
        vec![text(name), text(&typ)],
    ))
}

/// Go: `isTrue` (every modelled kind has a truth value).
fn is_true(v: &Val<'_>) -> bool {
    let Val::V(v) = v else {
        // The zero Value is a form of nil; missingVal is a struct, which is always true.
        return matches!(v, Val::Missing);
    };
    match v.underlying() {
        Value::Bool(b) => *b,
        Value::Int(i) | Value::Int64(i) => *i != 0,
        Value::Int8(i) => *i != 0,
        Value::Int16(i) => *i != 0,
        Value::Int32(i) => *i != 0,
        Value::Uint(u) | Value::Uint64(u) | Value::Uintptr(u) => *u != 0,
        Value::Uint8(u) => *u != 0,
        Value::Uint16(u) => *u != 0,
        Value::Uint32(u) => *u != 0,
        Value::Float32(f) => f.is_nan() || f.abs() > 0.0,
        Value::Float64(f) => f.is_nan() || f.abs() > 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Bytes(b) => b.as_ref().is_some_and(|b| !b.is_empty()),
        Value::Slice { elems, .. } => elems.as_ref().is_some_and(|e| !e.is_empty()),
        Value::Array { elems, .. } => !elems.is_empty(),
        Value::Map { entries, .. } => entries.as_ref().is_some_and(|e| !e.is_empty()),
        Value::Struct { .. } => true, // Struct values are always true.
        Value::Pointer { elem, .. } => elem.is_some(),
        // A nil interface is false; underlying strips the Named and Method wrappers.
        Value::Nil | Value::Named { .. } | Value::Method { .. } => false,
    }
}

/// Go: `printableValue`: a pointer prints what it points to (a nil pointer prints `<nil>`),
/// unless it has an `Error`/`String` method.
fn printable(v: &Value) -> &Value {
    let mut cur = v;
    while let Value::Pointer { elem: Some(e), .. } = cur {
        cur = e;
    }
    cur
}

/// The elements of `range` over an integer: `0..n` in the integer's type.
fn integer_range(v: &Value) -> Vec<Value> {
    let n = match *v.underlying() {
        Value::Int(i) | Value::Int64(i) => u64::try_from(i).unwrap_or(0),
        Value::Int8(i) => u64::try_from(i).unwrap_or(0),
        Value::Int16(i) => u64::try_from(i).unwrap_or(0),
        Value::Int32(i) => u64::try_from(i).unwrap_or(0),
        Value::Uint(u) | Value::Uint64(u) | Value::Uintptr(u) => u,
        Value::Uint8(u) => u.into(),
        Value::Uint16(u) => u.into(),
        Value::Uint32(u) => u.into(),
        Value::Nil
        | Value::Bool(_)
        | Value::Float32(_)
        | Value::Float64(_)
        | Value::String(_)
        | Value::Bytes(_)
        | Value::Named { .. }
        | Value::Slice { .. }
        | Value::Array { .. }
        | Value::Map { .. }
        | Value::Struct { .. }
        | Value::Pointer { .. }
        | Value::Method { .. } => 0,
    };
    (0..n).filter_map(|i| same_type(v, i)).collect()
}

/// `i` converted to the integer type of `v` (keeping a defined type's name).
fn same_type(v: &Value, i: u64) -> Option<Value> {
    Some(match v {
        Value::Named { ty, value } => Value::Named {
            ty: ty.clone(),
            value: Box::new(same_type(value, i)?),
        },
        Value::Method { value, .. } => same_type(value, i)?,
        Value::Int(_) => Value::Int(i64::try_from(i).ok()?),
        Value::Int64(_) => Value::Int64(i64::try_from(i).ok()?),
        Value::Int8(_) => Value::Int8(i8::try_from(i).ok()?),
        Value::Int16(_) => Value::Int16(i16::try_from(i).ok()?),
        Value::Int32(_) => Value::Int32(i32::try_from(i).ok()?),
        Value::Uint(_) => Value::Uint(i),
        Value::Uint64(_) => Value::Uint64(i),
        Value::Uintptr(_) => Value::Uintptr(i),
        Value::Uint8(_) => Value::Uint8(u8::try_from(i).ok()?),
        Value::Uint16(_) => Value::Uint16(u16::try_from(i).ok()?),
        Value::Uint32(_) => Value::Uint32(u32::try_from(i).ok()?),
        Value::Nil
        | Value::Bool(_)
        | Value::Float32(_)
        | Value::Float64(_)
        | Value::String(_)
        | Value::Bytes(_)
        | Value::Slice { .. }
        | Value::Array { .. }
        | Value::Map { .. }
        | Value::Struct { .. }
        | Value::Pointer { .. } => return None,
    })
}
