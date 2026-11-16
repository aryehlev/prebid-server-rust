//! Go standard library behaviour that Prebid Server's observable output depends on.
//!
//! Each module mirrors one Go package, or the part of it that Prebid uses, and reproduces its
//! results byte for byte, error texts included. The reference is Go 1.26 (the behaviour below
//! has been stable since Go 1.21). Tests replay golden files written by the real Go toolchain:
//! the generators live under `testdata/go/` and document how to rerun them.
//!
//! # Modules
//!
//! - [`strconv`]: Go `strconv`. `FormatFloat` (formats `e E f g G b x X`, any precision, 32 and
//!   64 bit), `ParseFloat` (decimal, hex, `inf`/`nan`, underscores), `FormatInt`/`FormatUint`/
//!   `Itoa`, `ParseInt`/`ParseUint`/`Atoi` (base prefixes, underscores, bit sizes),
//!   `ParseBool`/`FormatBool`, the `Quote` family and `Unquote`/`UnquoteChar`. Errors are
//!   [`strconv::NumError`] with Go's text (`strconv.ParseInt: parsing "x": invalid syntax`); the
//!   `*_pair` variants also return the value Go returns alongside an error.
//! - [`math`]: the Go `math` functions and conversions whose Rust counterparts differ: NaN bits,
//!   `Max`/`Min` special cases, `float32(x)` and `int64(x)`.
//! - [`unicode`]: Go `unicode` predicates and case mapping (`IsLetter`, `IsDigit`, `IsSpace`,
//!   `IsPrint`, `IsGraphic`, `ToLower`, `ToUpper`, `ToTitle`, `SimpleFold`) from tables
//!   generated from Go's own tables, which can lag or lead Rust's Unicode version.
//!
//! # Example
//!
//! ```
//! use pbs_gocompat::strconv;
//!
//! assert_eq!(strconv::format_float(0.1 + 0.2, b'f', -1, 64), "0.30000000000000004");
//! assert_eq!(strconv::format_float(1.005, b'f', 2, 64), "1.00");
//! assert_eq!(
//!     strconv::atoi("12a").map_err(|e| e.to_string()),
//!     Err(r#"strconv.Atoi: parsing "12a": invalid syntax"#.to_owned()),
//! );
//! assert_eq!(strconv::quote("tab\there"), r#""tab\there""#);
//! ```

#[cfg(test)]
pub mod golden;
pub mod math;
pub mod strconv;
#[cfg(test)]
pub mod testutil;
pub mod unicode;
