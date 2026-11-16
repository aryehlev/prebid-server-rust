//! Go standard library behaviour that Prebid Server's observable output depends on.
//!
//! Each module mirrors one Go package, or the part of it that Prebid uses, and reproduces its
//! results byte for byte, error texts included. The reference is Go 1.26.2, the toolchain the
//! `Dockerfile` builds with, under the GODEBUG defaults that the repository's `go.mod`
//! (`go 1.25.0`) selects. Tests replay golden files written by that toolchain: the generators
//! live under `testdata/go/` and document how to rerun them.
//!
//! Go strings are bytes while Rust strings are UTF-8. Functions that can produce invalid UTF-8
//! in Go (unescaping, unquoting) have `_bytes` forms with Go's exact bytes; their `String` forms
//! replace each invalid byte with `U+FFFD` ([`utf8::lossy_string`]), the same text Go's JSON
//! encoder writes for such strings.
//!
//! # Modules
//!
//! - [`base64`]: Go `encoding/base64` (`StdEncoding`, `URLEncoding`, `RawStdEncoding`,
//!   `RawURLEncoding`) with Go's lenient decoder (newlines skipped, non-zero trailing bits
//!   accepted) and `illegal base64 data at input byte N` errors.
//! - [`fmt`]: Go `fmt`. [`fmt::sprintf`], [`fmt::errorf`] (the text of `fmt.Errorf`, where `%w`
//!   prints an error), [`fmt::sprint`] and [`fmt::sprintln`] over [`fmt::Value`], a model of the Go
//!   operand as `reflect` sees it: basic types, `[]byte`, defined types, slices, arrays, maps
//!   (printed in Go's sorted key order), structs (unexported fields included), pointers, nil
//!   interfaces, and values with `Error`/`String` methods. All verbs, flags, widths, precisions,
//!   argument indexes and error forms (`%!d(string=x)`, `%!v(MISSING)`, `%!(EXTRA int=1)`) match
//!   Go.
//! - [`http`]: Go `net/http` header semantics. [`http::canonical_mime_header_key`] (the `textproto`
//!   rule, including leaving keys with non-token bytes untouched), and [`http::Header`]:
//!   `Add`/`Set`/`Get`/`Values`/`Del` canonicalize, while the `*_raw` methods act on the map
//!   directly as a Go `http.Header{"key": ...}` literal does. [`http::Header::write`] produces
//!   `Header.Write`'s wire form: keys sorted, invalid names dropped, CR/LF replaced and values
//!   trimmed.
//! - [`strconv`]: Go `strconv`. `FormatFloat` (formats `e E f g G b x X`, any precision, 32 and 64
//!   bit), `ParseFloat` (decimal, hex, `inf`/`nan`, underscores), `FormatInt`, `FormatUint`,
//!   `Itoa`, `ParseInt`/`ParseUint`/`Atoi` (base prefixes, underscores, bit sizes),
//!   `ParseBool`/`FormatBool`, the `Quote` family and `Unquote`/`UnquoteChar`. Errors are
//!   [`strconv::NumError`] with Go's text (`strconv.ParseInt: parsing "x": invalid syntax`); the
//!   `*_pair` variants also return the value Go returns alongside an error.
//! - [`strings`]: the Go `strings` functions whose Rust counterparts differ: `ToLower`/`ToUpper`
//!   (simple per-rune case mapping), `EqualFold`, `TrimSpace`/`Fields` (`unicode.IsSpace`),
//!   `Split`/`SplitN`/`SplitAfter`/`SplitAfterN` (`n` and empty-separator rules) and `NewReplacer`.
//! - [`math`]: the Go `math` functions and conversions whose Rust counterparts differ: NaN bits,
//!   `Max`/`Min` special cases, `float32(x)` and `int64(x)`.
//! - [`url`]: Go `net/url`. `QueryEscape`/`PathEscape` and their inverses with
//!   [`url::EscapeError`] texts; [`url::Url`] with `Parse`, `ParseRequestURI`, `String`,
//!   `Query`, `Hostname`, `Port`, `RequestURI`, `EscapedPath` and Go's error texts
//!   (`parse "http://[::1": missing ']' in host`), including the `netip.ParseAddr` messages for
//!   bracketed hosts; [`url::Values`] with `Get`/`Set`/`Add`/`Del`/`Has` and the key-sorted
//!   `Encode`; [`url::parse_query`] with the semicolon and 10000-parameter rules.
//! - [`utf8`]: Go's rune decoding of byte strings that are not valid UTF-8.
//! - [`unicode`]: Go `unicode` predicates and case mapping (`IsLetter`, `IsDigit`, `IsSpace`,
//!   `IsPrint`, `IsGraphic`, `ToLower`, `ToUpper`, `ToTitle`, `SimpleFold`) from tables generated
//!   from Go's own tables, which can lag or lead Rust's Unicode version.
//!
//! # Example
//!
//! ```
//! use pbs_gocompat::strconv;
//!
//! assert_eq!(
//!     strconv::format_float(0.1 + 0.2, b'f', -1, 64),
//!     "0.30000000000000004"
//! );
//! assert_eq!(strconv::format_float(1.005, b'f', 2, 64), "1.00");
//! assert_eq!(
//!     strconv::atoi("12a").map_err(|e| e.to_string()),
//!     Err(r#"strconv.Atoi: parsing "12a": invalid syntax"#.to_owned()),
//! );
//! assert_eq!(strconv::quote("tab\there"), r#""tab\there""#);
//! ```

pub mod base64;
pub mod fmt;
#[cfg(test)]
pub mod golden;
pub mod http;
pub mod math;
pub mod strconv;
pub mod strings;
#[cfg(test)]
pub mod testutil;
pub mod unicode;
pub mod url;
pub mod utf8;
