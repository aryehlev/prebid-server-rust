//! Go: `fmt/print.go`: argument dispatch (`printArg`), the reflection printer (`printValue`),
//! and the format interpreters (`doPrintf`, `doPrint`, `doPrintln`).

use super::{
    format::{Flag, Flags, LDIGITS, UDIGITS},
    value::{MethodKind, Value, compare},
};

const COMMA_SPACE: &[u8] = b", ";
const NIL_ANGLE: &str = "<nil>";
const NIL_PAREN: &[u8] = b"(nil)";
const PERCENT_BANG: &[u8] = b"%!";

/// Where Go's `badVerb` finds the operand to print again: `p.arg` (with `printArg`) or `p.value`
/// (with `printValue` at depth 0).
#[derive(Debug, Clone, Copy)]
enum Operand<'a> {
    Arg(&'a Value),
    Reflect(&'a Value),
}

impl<'a> Operand<'a> {
    const fn value(self) -> &'a Value {
        match self {
            Self::Arg(v) | Self::Reflect(v) => v,
        }
    }
}

/// Go: `pp`, the printer state of one `Sprintf`/`Sprint`/`Errorf` call.
#[derive(Debug, Default)]
pub(super) struct Printer {
    pub(super) buf: Vec<u8>,
    fmt: Flags,
    /// Set while printing a `%!verb(...)` operand, so that no methods are called.
    erroring: bool,
    /// Set by `Errorf`, which accepts `%w`.
    wrap_errs: bool,
}

impl Printer {
    pub(super) fn new(wrap_errs: bool) -> Self {
        Self {
            wrap_errs,
            ..Self::default()
        }
    }

    fn write(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    fn write_char(&mut self, c: char) {
        self.write(c.encode_utf8(&mut [0; 4]).as_bytes());
    }

    /// `%v`/`%w`: Go moves `#` and `+` aside into `sharpV`/`plusV`.
    const fn move_v_flags(&mut self) {
        let f = &mut self.fmt;
        f.set(Flag::SharpV, f.has(Flag::Sharp));
        f.set(Flag::Sharp, false);
        f.set(Flag::PlusV, f.has(Flag::Plus));
        f.set(Flag::Plus, false);
    }

    /// Go: `badVerb`: `%!verb(type=value)`, or `%!verb(<nil>)`.
    fn bad_verb(&mut self, verb: char, operand: Option<Operand<'_>>) {
        self.erroring = true;
        self.write(PERCENT_BANG);
        self.write_char(verb);
        self.buf.push(b'(');
        match operand {
            Some(Operand::Arg(v)) if !matches!(v, Value::Nil) => {
                self.write(v.type_name().as_bytes());
                self.buf.push(b'=');
                self.print_arg(v, 'v');
            }
            Some(Operand::Reflect(v)) => {
                self.write(v.type_name().as_bytes());
                self.buf.push(b'=');
                self.print_value(v, 'v', 0, true);
            }
            Some(Operand::Arg(_)) | None => self.write(NIL_ANGLE.as_bytes()),
        }
        self.buf.push(b')');
        self.erroring = false;
    }

    /// Go: `fmtBool`.
    fn fmt_bool(&mut self, op: Operand<'_>, v: bool, verb: char) {
        match verb {
            't' | 'v' => self.fmt.fmt_boolean(&mut self.buf, v),
            _ => self.bad_verb(verb, Some(op)),
        }
    }

    /// Go: `fmt0x64`: hexadecimal with or without a leading `0x`.
    fn fmt_0x64(&mut self, v: u64, leading_0x: bool) {
        let mut f = self.fmt;
        f.set(Flag::Sharp, leading_0x);
        f.fmt_integer(&mut self.buf, v, 16, false, 'v', LDIGITS);
    }

    /// Go: `pp.fmtInteger`.
    fn fmt_integer(&mut self, op: Operand<'_>, v: u64, is_signed: bool, verb: char) {
        let f = self.fmt;
        let buf = &mut self.buf;
        match verb {
            'v' if f.has(Flag::SharpV) && !is_signed => self.fmt_0x64(v, true),
            'v' | 'd' => f.fmt_integer(buf, v, 10, is_signed, verb, LDIGITS),
            'b' => f.fmt_integer(buf, v, 2, is_signed, verb, LDIGITS),
            'o' | 'O' => f.fmt_integer(buf, v, 8, is_signed, verb, LDIGITS),
            'x' => f.fmt_integer(buf, v, 16, is_signed, verb, LDIGITS),
            'X' => f.fmt_integer(buf, v, 16, is_signed, verb, UDIGITS),
            'c' => f.fmt_c(buf, v),
            'q' => f.fmt_qc(buf, v),
            'U' => f.fmt_unicode(buf, v),
            _ => self.bad_verb(verb, Some(op)),
        }
    }

    /// Go: `pp.fmtFloat`: the default precision for each verb.
    fn fmt_float(&mut self, op: Operand<'_>, v: f64, size: i64, verb: char) {
        let spec = match verb {
            'v' | 'g' => Some((b'g', -1)),
            'b' => Some((b'b', -1)),
            'G' => Some((b'G', -1)),
            'x' => Some((b'x', -1)),
            'X' => Some((b'X', -1)),
            'f' | 'F' => Some((b'f', 6)),
            'e' => Some((b'e', 6)),
            'E' => Some((b'E', 6)),
            _ => None,
        };
        match spec {
            Some((fmt, prec)) => self.fmt.fmt_float(&mut self.buf, v, size, fmt, prec),
            None => self.bad_verb(verb, Some(op)),
        }
    }

    /// Go: `fmtString`.
    fn fmt_string(&mut self, op: Operand<'_>, s: &[u8], verb: char) {
        let f = self.fmt;
        let buf = &mut self.buf;
        match verb {
            'v' if f.has(Flag::SharpV) => f.fmt_q(buf, s),
            'v' | 's' => f.fmt_s(buf, s),
            'x' => f.fmt_sbx(buf, s, LDIGITS),
            'X' => f.fmt_sbx(buf, s, UDIGITS),
            'q' => f.fmt_q(buf, s),
            _ => self.bad_verb(verb, Some(op)),
        }
    }

    /// Go: `fmtBytes`.
    fn fmt_bytes(&mut self, op: Operand<'_>, v: Option<&[u8]>, verb: char, type_string: &str) {
        let f = self.fmt;
        match verb {
            'v' | 'd' if f.has(Flag::SharpV) => {
                self.write(type_string.as_bytes());
                let Some(v) = v else {
                    self.write(NIL_PAREN);
                    return;
                };
                self.buf.push(b'{');
                for (i, &c) in v.iter().enumerate() {
                    if i > 0 {
                        self.write(COMMA_SPACE);
                    }
                    self.fmt_0x64(c.into(), true);
                }
                self.buf.push(b'}');
            }
            'v' | 'd' => {
                self.buf.push(b'[');
                for (i, &c) in v.unwrap_or_default().iter().enumerate() {
                    if i > 0 {
                        self.buf.push(b' ');
                    }
                    f.fmt_integer(&mut self.buf, c.into(), 10, false, verb, LDIGITS);
                }
                self.buf.push(b']');
            }
            's' => f.fmt_s(&mut self.buf, v.unwrap_or_default()),
            'x' => f.fmt_sbx(&mut self.buf, v.unwrap_or_default(), LDIGITS),
            'X' => f.fmt_sbx(&mut self.buf, v.unwrap_or_default(), UDIGITS),
            'q' => f.fmt_q(&mut self.buf, v.unwrap_or_default()),
            _ => self.print_value(op.value(), verb, 0, true),
        }
    }

    /// Go: `fmtPointer`. Slices and maps carry no address in the model and print as 0.
    fn fmt_pointer(&mut self, op: Operand<'_>, verb: char) {
        let value = op.value();
        let Some(u) = value.pointer_addr() else {
            self.bad_verb(verb, Some(op));
            return;
        };
        match verb {
            'v' if self.fmt.has(Flag::SharpV) => {
                self.buf.push(b'(');
                self.write(value.type_name().as_bytes());
                self.write(b")(");
                if u == 0 {
                    self.write(b"nil");
                } else {
                    self.fmt_0x64(u, true);
                }
                self.buf.push(b')');
            }
            'v' if u == 0 => self.fmt.pad_string(&mut self.buf, NIL_ANGLE),
            'v' | 'p' => self.fmt_0x64(u, !self.fmt.has(Flag::Sharp)),
            'b' | 'o' | 'd' | 'x' | 'X' => self.fmt_integer(op, u, false, verb),
            _ => self.bad_verb(verb, Some(op)),
        }
    }

    /// Go: `handleMethods`: `%w` checks, then `Error()`/`String()` for the string verbs.
    /// `GoString` (for `%#v`) and `Format` methods are not modelled.
    fn handle_methods(&mut self, arg: &Value, verb: char) -> bool {
        if self.erroring {
            return false;
        }
        let method = match arg {
            Value::Method { kind, text, .. } => Some((*kind, text)),
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
            | Value::Struct { .. }
            | Value::Pointer { .. } => None,
        };
        let mut verb = verb;
        if verb == 'w' {
            // It is invalid to use %w other than with Errorf or with a non-error arg.
            if !self.wrap_errs || !matches!(method, Some((MethodKind::Error, _))) {
                self.bad_verb('w', Some(Operand::Arg(arg)));
                return true;
            }
            verb = 'v';
        }
        if self.fmt.has(Flag::SharpV) {
            return false;
        }
        match method {
            Some((_, text)) if matches!(verb, 'v' | 's' | 'x' | 'X' | 'q') => {
                self.fmt_string(Operand::Arg(arg), text.as_bytes(), verb);
                true
            }
            Some(_) | None => false,
        }
    }

    /// Go: `printArg`.
    pub(super) fn print_arg(&mut self, arg: &Value, verb: char) {
        if matches!(arg, Value::Nil) {
            if matches!(verb, 'T' | 'v') {
                self.fmt.pad_string(&mut self.buf, NIL_ANGLE);
            } else {
                self.bad_verb(verb, None);
            }
            return;
        }
        // %T (the value's type) and %p (its address) are special; we always do them first.
        match verb {
            'T' => {
                self.fmt.fmt_s(&mut self.buf, arg.type_name().as_bytes());
                return;
            }
            'p' => {
                self.fmt_pointer(Operand::Arg(arg), 'p');
                return;
            }
            _ => {}
        }
        match arg {
            Value::Bytes(b) => self.fmt_bytes(Operand::Arg(arg), b.as_deref(), verb, "[]byte"),
            Value::Named { .. }
            | Value::Slice { .. }
            | Value::Array { .. }
            | Value::Map { .. }
            | Value::Struct { .. }
            | Value::Pointer { .. }
            | Value::Method { .. } => {
                // If the type is not simple, it might have methods.
                if !self.handle_methods(arg, verb) {
                    self.print_value(arg, verb, 0, true);
                }
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
            | Value::String(_) => self.print_kind(Operand::Arg(arg), arg, verb, 0, true),
        }
    }

    /// Go: `printValue`. `can_interface` is Go's `CanInterface`: false below unexported fields.
    fn print_value(&mut self, value: &Value, verb: char, depth: usize, can_interface: bool) {
        // Handle values with special methods if not already handled by printArg (depth == 0).
        if depth > 0 && can_interface && self.handle_methods(value, verb) {
            return;
        }
        self.print_kind(Operand::Reflect(value), value, verb, depth, can_interface);
    }

    /// The `switch value.Kind()` of Go's `printValue`; `kind` is `op`'s value or what it wraps.
    fn print_kind(
        &mut self,
        op: Operand<'_>,
        kind: &Value,
        verb: char,
        depth: usize,
        can_interface: bool,
    ) {
        match kind {
            Value::Nil if depth == 0 => self.write(b"<invalid reflect.Value>"),
            Value::Nil if self.fmt.has(Flag::SharpV) => self.write(b"interface {}(nil)"),
            Value::Nil => self.write(NIL_ANGLE.as_bytes()),
            Value::Bool(v) => self.fmt_bool(op, *v, verb),
            Value::Int(v) | Value::Int64(v) => self.fmt_integer(op, v.cast_unsigned(), true, verb),
            Value::Int8(v) => self.fmt_integer(op, i64::from(*v).cast_unsigned(), true, verb),
            Value::Int16(v) => self.fmt_integer(op, i64::from(*v).cast_unsigned(), true, verb),
            Value::Int32(v) => self.fmt_integer(op, i64::from(*v).cast_unsigned(), true, verb),
            Value::Uint(v) | Value::Uint64(v) | Value::Uintptr(v) => {
                self.fmt_integer(op, *v, false, verb);
            }
            Value::Uint8(v) => self.fmt_integer(op, (*v).into(), false, verb),
            Value::Uint16(v) => self.fmt_integer(op, (*v).into(), false, verb),
            Value::Uint32(v) => self.fmt_integer(op, (*v).into(), false, verb),
            Value::Float32(v) => self.fmt_float(op, (*v).into(), 32, verb),
            Value::Float64(v) => self.fmt_float(op, *v, 64, verb),
            Value::String(s) => self.fmt_string(op, s.as_bytes(), verb),
            Value::Bytes(b) if matches!(verb, 's' | 'q' | 'x' | 'X') => {
                self.fmt_bytes(op, b.as_deref(), verb, op.value().type_name());
            }
            Value::Bytes(b) => {
                let elems: Option<Vec<Value>> = b
                    .as_ref()
                    .map(|b| b.iter().copied().map(Value::Uint8).collect());
                self.print_list(op, elems.as_deref(), true, verb, depth, can_interface);
            }
            Value::Named { value, .. } | Value::Method { value, .. } => {
                self.print_kind(op, value, verb, depth, can_interface);
            }
            Value::Slice { elems, .. } => {
                self.print_list(op, elems.as_deref(), true, verb, depth, can_interface);
            }
            Value::Array { elems, .. } => {
                self.print_list(op, Some(elems), false, verb, depth, can_interface);
            }
            Value::Map { entries, .. } => {
                self.print_map(op, entries.as_deref(), verb, depth, can_interface);
            }
            Value::Struct { fields, .. } => {
                self.print_struct(op, fields, verb, depth, can_interface);
            }
            Value::Pointer { elem, .. } => {
                // pointer to array or slice or struct? ok at top level but not embedded (avoid
                // loops)
                if depth == 0
                    && let Some(elem) = elem
                    && matches!(
                        elem.underlying(),
                        Value::Array { .. }
                            | Value::Slice { .. }
                            | Value::Bytes(_)
                            | Value::Struct { .. }
                            | Value::Map { .. }
                    )
                {
                    self.buf.push(b'&');
                    self.print_value(elem, verb, depth + 1, can_interface);
                    return;
                }
                self.fmt_pointer(op, verb);
            }
        }
    }

    /// The `reflect.Array, reflect.Slice` case of Go's `printValue`.
    fn print_list(
        &mut self,
        op: Operand<'_>,
        elems: Option<&[Value]>,
        is_slice: bool,
        verb: char,
        depth: usize,
        can_interface: bool,
    ) {
        let ty = op.value().type_name();
        // Handle byte and uint8 slices and arrays special for the string verbs.
        if matches!(verb, 's' | 'q' | 'x' | 'X') && elem_kind_is_uint8(ty, elems) {
            let bytes: Option<Vec<u8>> = elems.map(|elems| {
                elems
                    .iter()
                    .map(|e| e.as_uint().and_then(|u| u8::try_from(u).ok()).unwrap_or(0))
                    .collect()
            });
            self.fmt_bytes(op, bytes.as_deref(), verb, ty);
            return;
        }
        let sharp_v = self.fmt.has(Flag::SharpV);
        if sharp_v {
            self.write(ty.as_bytes());
            if is_slice && elems.is_none() {
                self.write(NIL_PAREN);
                return;
            }
        }
        self.buf.push(if sharp_v { b'{' } else { b'[' });
        for (i, elem) in elems.unwrap_or_default().iter().enumerate() {
            if i > 0 {
                self.write(if sharp_v { COMMA_SPACE } else { b" " });
            }
            self.print_value(elem, verb, depth + 1, can_interface);
        }
        self.buf.push(if sharp_v { b'}' } else { b']' });
    }

    /// The `reflect.Map` case of Go's `printValue`: keys in `fmtsort` order.
    fn print_map(
        &mut self,
        op: Operand<'_>,
        entries: Option<&[(Value, Value)]>,
        verb: char,
        depth: usize,
        can_interface: bool,
    ) {
        let sharp_v = self.fmt.has(Flag::SharpV);
        if sharp_v {
            self.write(op.value().type_name().as_bytes());
            if entries.is_none() {
                self.write(NIL_PAREN);
                return;
            }
            self.buf.push(b'{');
        } else {
            self.write(b"map[");
        }
        let mut sorted: Vec<&(Value, Value)> = entries.unwrap_or_default().iter().collect();
        sorted.sort_by(|a, b| compare(&a.0, &b.0));
        for (i, (key, value)) in sorted.into_iter().enumerate() {
            if i > 0 {
                self.write(if sharp_v { COMMA_SPACE } else { b" " });
            }
            self.print_value(key, verb, depth + 1, can_interface);
            self.buf.push(b':');
            self.print_value(value, verb, depth + 1, can_interface);
        }
        self.buf.push(if sharp_v { b'}' } else { b']' });
    }

    /// The `reflect.Struct` case of Go's `printValue`.
    fn print_struct(
        &mut self,
        op: Operand<'_>,
        fields: &[(String, Value)],
        verb: char,
        depth: usize,
        can_interface: bool,
    ) {
        let sharp_v = self.fmt.has(Flag::SharpV);
        if sharp_v {
            self.write(op.value().type_name().as_bytes());
        }
        self.buf.push(b'{');
        for (i, (name, value)) in fields.iter().enumerate() {
            if i > 0 {
                self.write(if sharp_v { COMMA_SPACE } else { b" " });
            }
            if (sharp_v || self.fmt.has(Flag::PlusV)) && !name.is_empty() {
                self.write(name.as_bytes());
                self.buf.push(b':');
            }
            // Go: token.IsExported, which reflect applies to field names.
            let exported = name.chars().next().is_some_and(char::is_uppercase);
            self.print_value(value, verb, depth + 1, can_interface && exported);
        }
        self.buf.push(b'}');
    }

    /// Go: `doPrintf`.
    pub(super) fn do_printf(&mut self, format: &str, args: &[Value]) {
        let mut state = Printf {
            format,
            args,
            arg_num: 0,
            i: 0,
            reordered: false,
            good_arg_num: true,
            after_index: false,
        };
        state.run(self);
    }

    /// Go: `doPrint`: spaces between operands when neither is a string.
    pub(super) fn do_print(&mut self, args: &[Value]) {
        let mut prev_string = false;
        for (n, arg) in args.iter().enumerate() {
            let is_string = matches!(arg.underlying(), Value::String(_));
            // Add a space between two non-string arguments.
            if n > 0 && !is_string && !prev_string {
                self.buf.push(b' ');
            }
            self.print_arg(arg, 'v');
            prev_string = is_string;
        }
    }

    /// Go: `doPrintln`: spaces between all operands and a newline.
    pub(super) fn do_println(&mut self, args: &[Value]) {
        for (n, arg) in args.iter().enumerate() {
            if n > 0 {
                self.buf.push(b' ');
            }
            self.print_arg(arg, 'v');
        }
        self.buf.push(b'\n');
    }
}

/// Go: `t.Elem().Kind() == reflect.Uint8` for the slice or array type `ty` holding `elems`.
///
/// For a type literal (`[]uint8`, `[4]uint8`, `[][]uint8`, `[]main.b`) the element type follows
/// the first `]`: `uint8` is a byte element, a composite or interface element type is not, and a
/// defined element type (`main.b`) is a byte element when its values are. A defined slice type
/// (`main.Bytes`) has elements of its element type, so its first element decides.
fn elem_kind_is_uint8(ty: &str, elems: Option<&[Value]>) -> bool {
    let first_is_uint8 = || {
        elems
            .and_then(<[Value]>::first)
            .is_some_and(|e| matches!(e.underlying(), Value::Uint8(_)))
    };
    let Some(elem) = ty
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .map(|(_, elem)| elem)
    else {
        return first_is_uint8();
    };
    if elem == "uint8" {
        return true;
    }
    let composite = [
        "[",
        "*",
        "map[",
        "chan ",
        "<-chan",
        "func(",
        "struct {",
        "interface {",
    ]
    .iter()
    .any(|prefix| elem.starts_with(prefix));
    let predeclared = !elem.contains('.');
    !composite && !predeclared && first_is_uint8()
}

/// Go: `tooLarge`: the magnitude of a width or precision is capped at 1e6.
const fn too_large(x: i64) -> bool {
    const MAX: i64 = 1_000_000;
    x > MAX || x < -MAX
}

/// Go: `parsenum`: the decimal number in `s[start..end]`, whether there was one, and where it
/// ended.
fn parse_num(s: &[u8], start: usize, end: usize) -> (i64, bool, usize) {
    if start >= end {
        return (0, false, end);
    }
    let (mut num, mut is_num, mut i) = (0, false, start);
    while i < end
        && let Some(&c) = s.get(i)
        && c.is_ascii_digit()
    {
        if too_large(num) {
            return (0, false, end); // Overflow; crazy long number most likely.
        }
        num = num * 10 + i64::from(c - b'0');
        is_num = true;
        i += 1;
    }
    (num, is_num, i)
}

/// Go: `parseArgNumber`: the zero-based index of the bracketed number at the start of `format`,
/// the bytes to consume, and whether the number parsed.
fn parse_arg_number(format: &[u8]) -> (Option<usize>, usize, bool) {
    // There must be at least 3 bytes: [n].
    if format.len() < 3 {
        return (None, 1, false);
    }
    // Find closing bracket.
    let Some(close) = format
        .iter()
        .skip(1)
        .position(|&c| c == b']')
        .map(|p| p + 1)
    else {
        return (None, 1, false);
    };
    let (width, ok, newi) = parse_num(format, 1, close);
    if !ok || newi != close {
        return (None, close + 1, false);
    }
    // Arg numbers are one-indexed and skip paren.
    let index = usize::try_from(width).ok().and_then(|w| w.checked_sub(1));
    (index, close + 1, true)
}

/// The loop state of Go's `doPrintf`.
struct Printf<'a> {
    format: &'a str,
    args: &'a [Value],
    arg_num: usize,
    i: usize,
    /// The format used an explicit argument index.
    reordered: bool,
    /// The most recent argument index was valid.
    good_arg_num: bool,
    /// The previous item in the format was an index like `[3]`.
    after_index: bool,
}

impl Printf<'_> {
    fn byte_at(&self, i: usize) -> Option<u8> {
        self.format.as_bytes().get(i).copied()
    }

    /// Go: `argNumber`.
    fn arg_number(&mut self) {
        if self.byte_at(self.i) != Some(b'[') {
            self.after_index = false;
            return;
        }
        self.reordered = true;
        let rest = self.format.as_bytes().get(self.i..).unwrap_or_default();
        let (index, wid, ok) = parse_arg_number(rest);
        self.i += wid;
        if ok && let Some(index) = index.filter(|&n| n < self.args.len()) {
            self.arg_num = index;
            self.after_index = true;
            return;
        }
        self.good_arg_num = false;
        self.after_index = ok;
    }

    /// Go: `intFromArg`: the integer operand of a `*` width or precision.
    fn int_from_arg(&mut self) -> (i64, bool) {
        let Some(arg) = self.args.get(self.arg_num) else {
            return (0, false);
        };
        self.arg_num += 1;
        let num = arg
            .as_int()
            .or_else(|| arg.as_uint().and_then(|n| i64::try_from(n).ok()));
        match num {
            Some(n) if !too_large(n) => (n, true),
            Some(_) | None => (0, false),
        }
    }

    fn run(&mut self, p: &mut Printer) {
        let end = self.format.len();
        while self.i < end {
            self.good_arg_num = true;
            let lasti = self.i;
            while self.i < end && self.byte_at(self.i) != Some(b'%') {
                self.i += 1;
            }
            p.write(
                self.format
                    .as_bytes()
                    .get(lasti..self.i)
                    .unwrap_or_default(),
            );
            if self.i >= end {
                // done processing format string
                break;
            }
            // Process one verb
            self.i += 1;
            p.fmt.clear();
            if self.simple_format(p) {
                continue;
            }
            if !self.directive(p) {
                break;
            }
        }
        self.extra_args(p);
    }

    /// The flags loop of Go's `doPrintf` and its fast path for a lowercase verb without width,
    /// precision or index. Returns whether it printed the verb.
    fn simple_format(&mut self, p: &mut Printer) -> bool {
        while let Some(c) = self.byte_at(self.i) {
            let flag = match c {
                b'#' => Flag::Sharp,
                b'0' => Flag::Zero,
                b'+' => Flag::Plus,
                b'-' => Flag::Minus,
                b' ' => Flag::Space,
                _ => {
                    if c.is_ascii_lowercase()
                        && let Some(arg) = self.args.get(self.arg_num)
                    {
                        if c == b'v' || c == b'w' {
                            p.move_v_flags();
                        }
                        p.print_arg(arg, char::from(c));
                        self.arg_num += 1;
                        self.i += 1;
                        return true;
                    }
                    // Format is more complex than simple flags and a verb or is malformed.
                    return false;
                }
            };
            p.fmt.set(flag, true);
            self.i += 1;
        }
        false
    }

    /// The rest of one directive: index, width, precision and verb. Returns false when the
    /// format ended before the verb.
    fn directive(&mut self, p: &mut Printer) -> bool {
        let bytes = self.format.as_bytes();
        let end = bytes.len();
        // Do we have an explicit argument index?
        self.arg_number();
        // Do we have width?
        if self.byte_at(self.i) == Some(b'*') {
            self.i += 1;
            let (wid, ok) = self.int_from_arg();
            p.fmt.wid = ok.then_some(wid);
            if !ok {
                p.write(b"%!(BADWIDTH)");
            }
            // We have a negative width, so take its value and ensure that the minus flag is set
            if wid < 0 {
                p.fmt.wid = Some(-wid);
                p.fmt.set(Flag::Minus, true);
                p.fmt.set(Flag::Zero, false); // Do not pad with zeros to the right.
            }
            self.after_index = false;
        } else {
            let (wid, present, newi) = parse_num(bytes, self.i, end);
            p.fmt.wid = present.then_some(wid);
            self.i = newi;
            if self.after_index && present {
                self.good_arg_num = false; // "%[3]2d"
            }
        }
        // Do we have precision?
        if self.i + 1 < end && self.byte_at(self.i) == Some(b'.') {
            self.i += 1;
            if self.after_index {
                self.good_arg_num = false; // "%[3].2d"
            }
            self.arg_number();
            if self.byte_at(self.i) == Some(b'*') {
                self.i += 1;
                let (prec, ok) = self.int_from_arg();
                // Negative precision arguments don't make sense
                let ok = ok && prec >= 0;
                p.fmt.prec = ok.then_some(prec);
                if !ok {
                    p.write(b"%!(BADPREC)");
                }
                self.after_index = false;
            } else {
                let (prec, present, newi) = parse_num(bytes, self.i, end);
                p.fmt.prec = Some(if present { prec } else { 0 });
                self.i = newi;
            }
        }
        if !self.after_index {
            self.arg_number();
        }
        let Some(verb) = self.format.get(self.i..).and_then(|s| s.chars().next()) else {
            p.write(b"%!(NOVERB)");
            return false;
        };
        self.i += verb.len_utf8();
        if verb == '%' {
            // Percent does not absorb operands and ignores f.wid and f.prec.
            p.buf.push(b'%');
        } else if !self.good_arg_num {
            p.write(PERCENT_BANG);
            p.write_char(verb);
            p.write(b"(BADINDEX)");
        } else if let Some(arg) = self.args.get(self.arg_num) {
            if verb == 'v' || verb == 'w' {
                p.move_v_flags();
            }
            p.print_arg(arg, verb);
            self.arg_num += 1;
        } else {
            // No argument left over to print for the current verb.
            p.write(PERCENT_BANG);
            p.write_char(verb);
            p.write(b"(MISSING)");
        }
        true
    }

    /// Go: the `%!(EXTRA type=value, ...)` check at the end of `doPrintf`, skipped when the
    /// format used argument indexes.
    fn extra_args(&self, p: &mut Printer) {
        let rest = self.args.get(self.arg_num..).unwrap_or_default();
        if self.reordered || rest.is_empty() {
            return;
        }
        p.fmt.clear();
        p.write(b"%!(EXTRA ");
        for (i, arg) in rest.iter().enumerate() {
            if i > 0 {
                p.write(COMMA_SPACE);
            }
            if matches!(arg, Value::Nil) {
                p.write(NIL_ANGLE.as_bytes());
            } else {
                p.write(arg.type_name().as_bytes());
                p.buf.push(b'=');
                p.print_arg(arg, 'v');
            }
        }
        p.buf.push(b')');
    }
}
