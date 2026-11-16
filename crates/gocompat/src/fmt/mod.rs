//! Go `fmt`: `Sprintf`, `Sprint`, `Sprintln` and the text of `Errorf`, over a model of Go
//! values ([`Value`]).
//!
//! Go: `fmt/print.go`, `fmt/format.go`, `fmt/errors.go` and `internal/fmtsort`. Go 1.26.
//!
//! Every verb, flag, width, precision and argument index behaves as in Go, error forms included:
//! `%!d(string=x)`, `%!v(MISSING)`, `%!(EXTRA int=1)`, `%!(BADINDEX)`, `%!(BADWIDTH)`,
//! `%!(BADPREC)`, `%!(NOVERB)`, `<nil>`. Composite values print as Go prints them: maps in
//! sorted key order, `%+v` struct field names, `&{...}` for a top-level pointer to a struct,
//! slice, array or map, and `Error()`/`String()` texts for [`Value::Method`] values (except below
//! unexported struct fields, where Go cannot call them).
//!
//! Not modelled: `Formatter` and `GoStringer` methods, complex numbers, channels and functions,
//! and the panic recovery of methods (a nil-pointer receiver whose method panics prints `<nil>`
//! in Go). Slices and maps carry no address, so `%p` prints them as `0x0`.
//!
//! ```
//! use pbs_gocompat::fmt::{Value, sprintf};
//!
//! let args = [Value::from("imp-1"), Value::Int(3), Value::Float64(0.5)];
//! assert_eq!(
//!     sprintf("%s: %d bids at %.2f", &args),
//!     "imp-1: 3 bids at 0.50"
//! );
//! assert_eq!(sprintf("%d", &[Value::from("x")]), "%!d(string=x)");
//! assert_eq!(sprintf("%v %v", &[Value::Nil]), "<nil> %!v(MISSING)");
//! ```

mod format;
mod print;
mod value;

use print::Printer;
pub use value::{ERROR_ADDR, MethodKind, Value};

use crate::utf8::lossy_string;

/// Go: `fmt.Sprintf`, as Go's bytes (byte-slice operands may hold invalid UTF-8).
#[must_use]
pub fn sprintf_bytes(format: &str, args: &[Value]) -> Vec<u8> {
    let mut p = Printer::new(false);
    p.do_printf(format, args);
    p.buf
}

/// Go: `fmt.Sprintf`. Invalid UTF-8 from byte-slice operands becomes U+FFFD per byte.
#[must_use]
pub fn sprintf(format: &str, args: &[Value]) -> String {
    lossy_string(&sprintf_bytes(format, args))
}

/// Go: `fmt.Errorf(format, args...).Error()`: `Sprintf` except that `%w` prints an error operand
/// as `%v` does.
#[must_use]
pub fn errorf(format: &str, args: &[Value]) -> String {
    let mut p = Printer::new(true);
    p.do_printf(format, args);
    lossy_string(&p.buf)
}

/// Go: `fmt.Sprint`: operands as `%v`, with a space between two operands when neither is a
/// string.
#[must_use]
pub fn sprint(args: &[Value]) -> String {
    let mut p = Printer::new(false);
    p.do_print(args);
    lossy_string(&p.buf)
}

/// Go: `fmt.Sprintln`: operands as `%v` separated by spaces, then a newline.
#[must_use]
pub fn sprintln(args: &[Value]) -> String {
    let mut p = Printer::new(false);
    p.do_println(args);
    lossy_string(&p.buf)
}

#[cfg(test)]
mod tests;
