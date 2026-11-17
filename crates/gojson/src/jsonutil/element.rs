//! `jsonutil.FindElement` and `jsonutil.DropElement`.

use core::{error::Error, fmt};

use crate::{
    stream::{Decoder, StreamError, Token},
    value::drop_value,
};

/// The error of [`find_element`] and [`drop_element`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElementError {
    /// The `json.Decoder` error Go returns.
    Stream(StreamError),
    /// Where Go panics (no names, or a bracket or comma it searches for is missing), the text
    /// of the runtime error, e.g. `runtime error: index out of range [7] with length 7`.
    Panic(String),
}

impl ElementError {
    /// The exact bytes of Go's `err.Error()` (for a panic, the runtime error's text).
    #[must_use]
    pub fn go_text(&self) -> Vec<u8> {
        match self {
            Self::Stream(err) => err.go_text(),
            Self::Panic(msg) => msg.as_bytes().to_vec(),
        }
    }
}

impl fmt::Display for ElementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stream(err) => fmt::Display::fmt(err, f),
            Self::Panic(msg) => f.write_str(msg),
        }
    }
}

impl Error for ElementError {}

impl From<StreamError> for ElementError {
    fn from(err: StreamError) -> Self {
        Self::Stream(err)
    }
}

/// The four results of [`find_element`] (Go's `found, startIndex, endIndex, err`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundElement {
    /// Whether the element was found.
    pub found: bool,
    /// Where the element (with the comma that precedes it, if any) starts.
    pub start_index: i64,
    /// Where it ends (after the comma that follows it, if any).
    pub end_index: i64,
    /// The error, if any; the indices are then `-1` (offset by the outer element's start
    /// when the error comes from a nested name).
    pub err: Option<ElementError>,
}

impl FoundElement {
    const fn failed(err: ElementError) -> Self {
        Self {
            found: false,
            start_index: -1,
            end_index: -1,
            err: Some(err),
        }
    }
}

/// Go's runtime error for `s[i]` out of range.
fn index_panic(index: i64, len: usize) -> ElementError {
    ElementError::Panic(if index < 0 {
        format!("runtime error: index out of range [{index}]")
    } else {
        format!("runtime error: index out of range [{index}] with length {len}")
    })
}

/// `extension[index]`, or Go's panic.
fn byte_at(extension: &[u8], index: i64) -> Result<u8, ElementError> {
    usize::try_from(index)
        .ok()
        .and_then(|i| extension.get(i).copied())
        .ok_or_else(|| index_panic(index, extension.len()))
}

fn len_i64(extension: &[u8]) -> i64 {
    i64::try_from(extension.len()).unwrap_or(i64::MAX)
}

/// `jsonutil.FindElement(extension, elementNames...)`: finds the first string token equal to
/// `names[0]`, at any depth, and the value after it.
///
/// The token can be an object key or a string value. With one name, the result is the range
/// of the matched member (or element) and its value, with one adjoining comma; with more, the
/// next name is searched for inside that value, and the indices are into `extension`.
#[must_use]
pub fn find_element(extension: &[u8], names: &[&str]) -> FoundElement {
    // Go: util/jsonutil/jsonutil.go FindElement
    let Some((&element_name, rest)) = names.split_first() else {
        return FoundElement::failed(index_panic(0, 0));
    };
    let mut dec = Decoder::new(extension);
    let mut found = false;
    let mut start_index: i64 = 0;
    let mut end_index: i64 = 0;
    loop {
        let token = match dec.token() {
            Ok(token) => token,
            // io.EOF is a successful end.
            Err(StreamError::Eof) => break,
            Err(err) => return FoundElement::failed(err.into()),
        };
        if matches!(&token, Token::String(s) if s == element_name) {
            let mut value = serde_json::Value::Null;
            let decoded = dec.decode(&mut value);
            // The value can be 10000 levels deep; its own drop would recurse per level.
            drop_value(value);
            if let Err(err) = decoded {
                return FoundElement::failed(err.into());
            }
            end_index = dec.input_offset();
            if dec.more()
                && let Err(err) = skip_commas(extension, &mut start_index, &mut end_index)
            {
                return FoundElement::failed(err);
            }
            found = true;
            break;
        }
        start_index = dec.input_offset();
    }
    if !found || rest.is_empty() {
        return FoundElement {
            found,
            start_index,
            end_index,
            err: None,
        };
    }
    match nested_range(extension, start_index, end_index) {
        Ok((start, end)) => {
            let inner = find_element(
                extension
                    .get(usize::try_from(start).unwrap_or(0)..usize::try_from(end).unwrap_or(0))
                    .unwrap_or_default(),
                rest,
            );
            FoundElement {
                found: inner.found,
                start_index: start + inner.start_index,
                end_index: start + inner.end_index,
                err: inner.err,
            }
        }
        Err(err) => FoundElement::failed(err),
    }
}

/// The comma handling after a match that more members follow: drop the comma before the
/// element from the range and take the one after it.
fn skip_commas(
    extension: &[u8],
    start_index: &mut i64,
    end_index: &mut i64,
) -> Result<(), ElementError> {
    if byte_at(extension, *start_index)? == b',' {
        *start_index += 1;
    }
    loop {
        let c = byte_at(extension, *end_index)?;
        *end_index += 1;
        if c == b',' {
            return Ok(());
        }
    }
}

/// The range `FindElement` searches for the next name: from after the first colon at or after
/// `start_index` to before the last bracket or comma.
fn nested_range(
    extension: &[u8],
    mut start_index: i64,
    mut end_index: i64,
) -> Result<(i64, i64), ElementError> {
    loop {
        let c = byte_at(extension, start_index)?;
        start_index += 1;
        if c == b':' {
            break;
        }
    }
    loop {
        if end_index == len_i64(extension) {
            end_index -= 1;
        }
        match byte_at(extension, end_index)? {
            b']' | b'}' => break,
            b',' => {
                end_index -= 1;
                break;
            }
            _ => end_index -= 1,
        }
    }
    if start_index > end_index {
        return Err(ElementError::Panic(format!(
            "runtime error: slice bounds out of range [{start_index}:{end_index}]"
        )));
    }
    Ok((start_index, end_index))
}

/// `jsonutil.DropElement(extension, elementNames...)`: `extension` without the element
/// [`find_element`] finds (unchanged when it is not found).
///
/// # Errors
///
/// [`find_element`]'s error.
pub fn drop_element(extension: &[u8], names: &[&str]) -> Result<Vec<u8>, ElementError> {
    // Go: util/jsonutil/jsonutil.go DropElement
    let found = find_element(extension, names);
    if let Some(err) = found.err {
        return Err(err);
    }
    if !found.found {
        return Ok(extension.to_vec());
    }
    let start = usize::try_from(found.start_index).unwrap_or(0);
    let end = usize::try_from(found.end_index).unwrap_or(0);
    let (Some(head), Some(tail)) = (extension.get(..start), extension.get(end..)) else {
        return Err(ElementError::Panic(format!(
            "runtime error: slice bounds out of range [{}:{}]",
            found.start_index, found.end_index
        )));
    };
    let mut out = Vec::with_capacity(head.len() + tail.len());
    out.extend_from_slice(head);
    out.extend_from_slice(tail);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{drop_element, find_element};

    /// A case: input, names and the expected output (or error text).
    type Case<'a> = (&'a str, &'a [&'a str], &'a str);

    #[test]
    fn drop_element_cases() -> Result<(), String> {
        // Go: util/jsonutil/jsonutil_test.go TestDropElement (tests/toolkit.rs replays every
        // case byte for byte)
        let cases: [Case<'_>; 5] = [
            (
                r#"{"consent": "X","settings": {"test": 1,"providers": [1608,765,492]}}"#,
                &["providers"],
                r#"{"consent": "X","settings": {"test": 1}}"#,
            ),
            (
                r#"{"settings": {"providers": [1608,765,492], "test": 1},"test": 123}"#,
                &["settings"],
                r#"{"test": 123}"#,
            ),
            (
                r#"{"settings":{"providers":[1608],"test":1},"data": [{"test":5}]}"#,
                &["settings", "test"],
                r#"{"settings":{"providers":[1608]},"data": [{"test":5}]}"#,
            ),
            (
                r#"{"settings": {"providers": [1608,765,492], "test": 1}}"#,
                &["test2"],
                r#"{"settings": {"providers": [1608,765,492], "test": 1}}"#,
            ),
            (
                r#"{"consent": "X","settings": {"providers": "test"}}"#,
                &["providers"],
                r#"{"consent": "X","settings": {}}"#,
            ),
        ];
        for (input, names, want) in cases {
            let got = drop_element(input.as_bytes(), names).map_err(|e| e.to_string())?;
            if got != want.as_bytes() {
                return Err(format!("{input}: got {}", String::from_utf8_lossy(&got)));
            }
        }
        let errors: [Case<'_>; 2] = [
            (
                r#"{"settings": {"providers": ["123",1,,1365], "test": 1}}"#,
                &["providers"],
                "invalid character ',' looking for beginning of value",
            ),
            (
                r#"{settings: {"providers": [1365], "test": 1}}"#,
                &["providers"],
                // Go's tokenError has no context for the state after `{`.
                "invalid character 's'",
            ),
        ];
        for (input, names, want) in errors {
            let got = drop_element(input.as_bytes(), names).map_err(|e| e.to_string());
            if got != Err(want.to_owned()) {
                return Err(format!("{input}: got {got:?}"));
            }
        }
        Ok(())
    }

    #[test]
    fn find_element_panics() -> Result<(), String> {
        // Where Go panics, the error carries the runtime error's text.
        let cases: [Case<'_>; 2] = [
            (
                r#"{"a":1}"#,
                &[],
                "runtime error: index out of range [0] with length 0",
            ),
            (
                r#"{"a":1 "b":2}"#,
                &["a"],
                "runtime error: index out of range [13] with length 13",
            ),
        ];
        for (input, names, want) in cases {
            let found = find_element(input.as_bytes(), names);
            let got = found.err.map(|e| e.to_string());
            if got.as_deref() != Some(want) || found.start_index != -1 {
                return Err(format!("{input}: got {got:?}"));
            }
        }
        Ok(())
    }
}
