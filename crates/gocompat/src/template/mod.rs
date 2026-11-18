//! Go `text/template`: `template.New(name).Parse(text)` and `Template.Execute` over a
//! [`fmt::Value`](crate::fmt::Value), as Prebid's `macros.ResolveMacros` uses them for bidder
//! endpoint and user sync URL templates.
//!
//! Go: `text/template/parse/{lex,parse,node}.go` and `text/template/{exec,template}.go`. Go 1.26.
//!
//! Parsing is complete: the lexer (default `{{ }}` delimiters, `{{- -}}` trim markers,
//! comments, quoted, raw and character constants, numbers), every action (`if`/`else if`,
//! `range` with `break`/`continue`, `with`/`else with`, `define`, `block`, `template`, variable
//! declarations and assignments, pipelines, parenthesized pipelines, field chains) and every parse
//! error text, e.g. `template: endpointTemplate:1: function "Host" not defined` or
//! `template: t:1: bad character U+005D ']'`. Calls of Go's builtin function names parse, and any
//! other function name is the error Go reports.
//!
//! Execution covers text, field and map-key lookups (`{{.Host}}`, `{{.A.B}}`, `{{$x.C}}`),
//! `Error`/`String` methods of [`Value::Method`] values, `$`
//! variables, constants, pipelines, `if`/`with` truth, `range` over slices, arrays, maps (in key
//! order) and integers, `break`/`continue`, and template invocation. Values print as
//! `fmt.Fprint` prints them, a missing value as `<no value>`, and every failure is a
//! [`TemplateError::Exec`] with Go's text, e.g. `template: t:1:2: executing "t" at <.Foo>:
//! can't evaluate field Foo in type macros.UserSyncPrivacy`.
//!
//! Not modelled, and documented where they differ:
//!
//! - Builtin functions (`and`, `call`, `eq`, `html`, `index`, `len`, `printf`, `urlquery`, ...) and
//!   complex constants parse but do not execute: reaching one is a [`TemplateError::Unsupported`].
//! - [`Value::Nil`] is a nil `any`: an `error`-typed nil field prints `<no value>` instead of Go's
//!   `<nil>`. Non-nil interface fields are their dynamic value, so an error about them names the
//!   dynamic type instead of the interface type.
//! - A nil pointer carries no pointee type, so a field lookup through it always reports `nil
//!   pointer evaluating`, also where Go reports `can't evaluate field` for a field the struct does
//!   not have. Fields promoted from embedded structs are not found.
//! - A map whose type is a defined name (no `map[K]V` in its type) is taken to have string keys
//!   when it is empty or its first key is a string.
//! - Parenthesized pipelines nest at most 100 deep (Go: 10000, with the same `max expression depth
//!   exceeded` text), template invocations nest at most 100 deep (Go: 100000; the error names Go's
//!   limit), and `if`/`range`/`with`/`block` (an `else if` chain counts each link) nest at most 100
//!   deep, where Go has no limit (the parse error is `max control nesting depth exceeded`). Go's
//!   goroutine stacks grow; Rust's threads have fixed stacks, so templates that combine all three
//!   limits can still exhaust a small one.
//! - Execution returns no partial output on error (Go writes what it produced before the error;
//!   `macros.ResolveMacros` discards it).
//!
//! ```
//! use pbs_gocompat::{fmt::Value, template::Template};
//!
//! let params = Value::Struct {
//!     ty: "macros.EndpointTemplateParams".to_owned(),
//!     fields: vec![("Host".to_owned(), Value::from("ads.example.com"))],
//! };
//! let tmpl = Template::parse("endpointTemplate", "https://{{.Host}}/bid")?;
//! assert_eq!(tmpl.execute(&params)?, "https://ads.example.com/bid");
//!
//! let missing = Template::parse("endpointTemplate", "https://{{.Region}}/bid")?;
//! assert_eq!(
//!     missing.execute(&params).map_err(|e| e.to_string()),
//!     Err(
//!         "template: endpointTemplate:1:10: executing \"endpointTemplate\" at <.Region>: \
//!          can't evaluate field Region in type macros.EndpointTemplateParams"
//!             .to_owned()
//!     ),
//! );
//! assert_eq!(
//!     Template::parse("endpointTemplate", "{{Host}}")
//!         .map_err(|e| e.to_string())
//!         .err(),
//!     Some("template: endpointTemplate:1: function \"Host\" not defined".to_owned()),
//! );
//! # Ok::<(), pbs_gocompat::template::TemplateError>(())
//! ```

mod exec;
mod lex;
mod node;
mod parse;
#[cfg(test)]
mod tests;

use std::{collections::BTreeMap, error::Error as StdError, fmt as stdfmt};

use crate::{fmt::Value, utf8::lossy_string};

/// Why parsing or executing a template failed. The text ([`std::fmt::Display`]) is Go's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    /// Go: the error `Template.Parse` returns.
    Parse(String),
    /// Go: `template.ExecError`.
    Exec {
        /// Go: `ExecError.Name`, the template that was executing.
        name: String,
        /// Go: `ExecError.Error()`.
        message: String,
    },
    /// Execution reached a builtin function or a complex constant, which this port does not
    /// evaluate. The text has Go's `template: NAME:LINE:COL: executing ...` prefix.
    Unsupported(String),
}

impl stdfmt::Display for TemplateError {
    fn fmt(&self, f: &mut stdfmt::Formatter<'_>) -> stdfmt::Result {
        match self {
            Self::Parse(message) | Self::Exec { message, .. } | Self::Unsupported(message) => {
                f.write_str(message)
            }
        }
    }
}

impl StdError for TemplateError {}

/// Go: a `*template.Template` after `Parse`: the named template and the templates its text
/// defines.
#[derive(Debug, Clone)]
pub struct Template {
    name: String,
    text: String,
    trees: BTreeMap<String, parse::Tree>,
}

impl Template {
    /// Go: `template.New(name).Parse(text)`.
    ///
    /// # Errors
    ///
    /// [`TemplateError::Parse`] with Go's text.
    pub fn parse(name: &str, text: &str) -> Result<Self, TemplateError> {
        let trees = parse::parse(name, text)?;
        Ok(Self {
            name: name.to_owned(),
            text: text.to_owned(),
            trees,
        })
    }

    /// Go: `Template.Name`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Go: `Template.Execute` into a buffer, as Go's bytes.
    ///
    /// # Errors
    ///
    /// [`TemplateError::Exec`] with Go's text, or [`TemplateError::Unsupported`].
    pub fn execute_bytes(&self, data: &Value) -> Result<Vec<u8>, TemplateError> {
        exec::execute(self, data)
    }

    /// Go: `Template.Execute` into a buffer, as a string (invalid UTF-8 from byte-slice values
    /// becomes U+FFFD).
    ///
    /// # Errors
    ///
    /// As [`Template::execute_bytes`].
    pub fn execute(&self, data: &Value) -> Result<String, TemplateError> {
        self.execute_bytes(data).map(|out| lossy_string(&out))
    }
}
