//! Differential replay of the goldens that `testdata/go/toolkit` writes with Go 1.26.2,
//! json-patch v5.9.0, jsonparser v1.1.2 and json-iterator 1.1.12: every output and error of
//! `mergepatch`, `jsonparser`, `stream` and `jsonutil`'s element and merge-clone helpers must
//! match Go byte for byte.
//!
//! Regenerate the goldens with `go run ./toolkit` in `testdata/go`.

#[cfg(test)]
mod toolkit {
    use core::str::from_utf8;
    use std::{borrow::Cow, fmt::Write as _, fs, thread};

    use pbs_gojson::{
        GoEncode, GoJson, GoString, RawJson,
        jsonparser::{self, ParserError},
        jsonutil::{self, ElementError},
        mergepatch, stdjson,
        stream::{self, Decoder, StreamError, Token},
    };
    use serde_json::Value;

    // ---- golden file support -------------------------------------------------------------

    /// Go's `strconv.QuoteToASCII`.
    fn quote_to_ascii(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::from("\"");
        let hex = |buf: &mut String, b: u8| {
            buf.push_str("\\x");
            buf.push(char::from(
                HEX.get(usize::from(b >> 4)).copied().unwrap_or(b'0'),
            ));
            buf.push(char::from(
                HEX.get(usize::from(b & 0xF)).copied().unwrap_or(b'0'),
            ));
        };
        for chunk in bytes.utf8_chunks() {
            for c in chunk.valid().chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\u{7}' => out.push_str("\\a"),
                    '\u{8}' => out.push_str("\\b"),
                    '\u{c}' => out.push_str("\\f"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\u{b}' => out.push_str("\\v"),
                    ' '..='~' => out.push(c),
                    '\0'..='\u{1f}' | '\u{7f}' => hex(&mut out, u8::try_from(c).unwrap_or(0)),
                    '\u{80}'..='\u{ffff}' => {
                        let _written = write!(out, "\\u{:04x}", u32::from(c));
                    }
                    '\u{10000}'.. => {
                        let _written = write!(out, "\\U{:08x}", u32::from(c));
                    }
                }
            }
            for &b in chunk.invalid() {
                hex(&mut out, b);
            }
        }
        out.push('"');
        out
    }

    fn hex_value(digits: &[u8]) -> Option<u32> {
        let text = from_utf8(digits).ok()?;
        u32::from_str_radix(text, 16).ok()
    }

    /// Go's `strconv.Unquote` for `QuoteToASCII` output.
    fn unquote(field: &str) -> Result<Vec<u8>, String> {
        let inner = field
            .strip_prefix('"')
            .and_then(|f| f.strip_suffix('"'))
            .ok_or_else(|| format!("not a quoted field: {field}"))?
            .as_bytes();
        let mut out = Vec::with_capacity(inner.len());
        let mut i = 0;
        while let Some(&b) = inner.get(i) {
            if b != b'\\' {
                out.push(b);
                i += 1;
                continue;
            }
            let escape = inner.get(i + 1).copied().unwrap_or(0);
            let simple = match escape {
                b'a' => Some(7),
                b'b' => Some(8),
                b'f' => Some(12),
                b'n' => Some(b'\n'),
                b'r' => Some(b'\r'),
                b't' => Some(b'\t'),
                b'v' => Some(11),
                b'\\' => Some(b'\\'),
                b'"' => Some(b'"'),
                _ => None,
            };
            if let Some(s) = simple {
                out.push(s);
                i += 2;
                continue;
            }
            let width = match escape {
                b'x' => 2,
                b'u' => 4,
                b'U' => 8,
                _ => return Err(format!("bad escape in {field}")),
            };
            let digits = inner.get(i + 2..i + 2 + width).unwrap_or_default();
            let value = hex_value(digits).ok_or_else(|| format!("bad hex in {field}"))?;
            if escape == b'x' {
                out.push(u8::try_from(value).map_err(|e| e.to_string())?);
            } else {
                let c = char::from_u32(value).ok_or_else(|| format!("bad rune in {field}"))?;
                let mut buf = [0_u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            i += 2 + width;
        }
        Ok(out)
    }

    /// The rows of a golden file (comment lines skipped), each a list of quoted fields.
    fn rows(name: &str) -> Result<Vec<Vec<String>>, String> {
        let path = format!("{}/testdata/go/toolkit/{name}", env!("CARGO_MANIFEST_DIR"));
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        Ok(text
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .map(|line| line.split('\t').map(str::to_owned).collect())
            .collect())
    }

    fn field(row: &[String], index: usize) -> Result<Vec<u8>, String> {
        unquote(row.get(index).ok_or_else(|| format!("short row {row:?}"))?)
    }

    fn num<T: ToString + Copy>(n: T) -> Vec<u8> {
        n.to_string().into_bytes()
    }

    fn concat(prefix: &[u8], rest: &[u8]) -> Vec<u8> {
        let mut out = prefix.to_vec();
        out.extend_from_slice(rest);
        out
    }

    fn quoted(fields: &[Vec<u8>]) -> String {
        fields
            .iter()
            .map(|f| quote_to_ascii(f))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Decodes a key path the generator wrote as `<n>:` followed by the keys joined with
    /// `\x1f`.
    fn path(encoded: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        let colon = encoded
            .iter()
            .position(|&b| b == b':')
            .ok_or("path without a count")?;
        let count = from_utf8(encoded.get(..colon).unwrap_or_default())
            .map_err(|e| e.to_string())?
            .parse::<usize>()
            .map_err(|e| e.to_string())?;
        if count == 0 {
            return Ok(Vec::new());
        }
        let keys: Vec<Vec<u8>> = encoded
            .get(colon + 1..)
            .unwrap_or_default()
            .split(|&b| b == 0x1F)
            .map(<[u8]>::to_vec)
            .collect();
        if keys.len() != count {
            return Err(format!("bad path {}", quote_to_ascii(encoded)));
        }
        Ok(keys)
    }

    /// Decodes several paths joined with `\x1e`.
    fn paths(encoded: &[u8]) -> Result<Vec<Vec<Vec<u8>>>, String> {
        encoded.split(|&b| b == 0x1E).map(path).collect()
    }

    fn names(encoded: &[u8]) -> Result<Vec<String>, String> {
        path(encoded)?
            .into_iter()
            .map(|n| String::from_utf8(n).map_err(|e| e.to_string()))
            .collect()
    }

    /// The comparison of one golden file.
    #[derive(Default)]
    struct Replay {
        rows: usize,
        /// Rows that differ from Go in a documented way (see each caller).
        gaps: usize,
        failures: Vec<String>,
    }

    /// Whether Go panicked on the row (its only output is `PANIC: ...`), where Rust takes the
    /// fallback documented on the function instead.
    fn go_panicked(want: &[Vec<u8>]) -> bool {
        matches!(want, [only] if only.starts_with(b"PANIC: "))
    }

    const fn exact(_want: &[Vec<u8>]) -> bool {
        false
    }

    impl Replay {
        /// Compares `actual` with the Go outputs from column `first` of `row`; a difference
        /// that `gap` accepts is counted instead of reported.
        fn check(
            &mut self,
            row: &[String],
            first: usize,
            actual: &[Vec<u8>],
            gap: impl FnOnce(&[Vec<u8>]) -> bool,
        ) -> Result<(), String> {
            self.rows += 1;
            let want = row
                .iter()
                .skip(first)
                .map(|f| unquote(f))
                .collect::<Result<Vec<_>, _>>()?;
            if want == actual {
                return Ok(());
            }
            if gap(&want) {
                self.gaps += 1;
                return Ok(());
            }
            let inputs = row
                .iter()
                .take(first)
                .map(|f| unquote(f))
                .collect::<Result<Vec<_>, _>>()?;
            self.failures.push(format!(
                "{}\n      go:   {}\n      rust: {}",
                quoted(&inputs),
                quoted(&want),
                quoted(actual)
            ));
            Ok(())
        }

        fn finish(self, file: &str, max_gaps: usize) -> Result<(), String> {
            if self.gaps > max_gaps {
                return Err(format!(
                    "{file}: {} rows differ as documented, expected at most {max_gaps}",
                    self.gaps
                ));
            }
            if self.rows == 0 {
                return Err(format!("{file}: no rows"));
            }
            if self.failures.is_empty() {
                return Ok(());
            }
            let mut report = format!(
                "{file}: {} of {} rows differ from Go\n",
                self.failures.len(),
                self.rows
            );
            for failure in self.failures.iter().take(40) {
                report.push_str(failure);
                report.push('\n');
            }
            Err(report)
        }
    }

    fn parser_err(err: Option<&ParserError>) -> Vec<u8> {
        err.map_or_else(Vec::new, ParserError::go_text)
    }

    fn result_pair<T, E>(
        res: Result<T, E>,
        ok: impl FnOnce(T) -> Vec<u8>,
        zero: &[u8],
        err: impl FnOnce(E) -> Vec<u8>,
    ) -> Vec<Vec<u8>> {
        match res {
            Ok(v) => vec![ok(v), Vec::new()],
            Err(e) => vec![zero.to_vec(), err(e)],
        }
    }

    // ---- json-patch -----------------------------------------------------------------------

    #[test]
    fn mergepatch_goldens() -> Result<(), String> {
        let mut replay = Replay::default();
        for row in &rows("mergepatch.txt")? {
            let a = field(row, 1)?;
            let b = field(row, 2)?;
            match field(row, 0)?.as_slice() {
                b"MP" => {
                    let out = result_pair(
                        mergepatch::merge_patch(&a, &b),
                        |v| v,
                        b"",
                        |e| e.as_str().as_bytes().to_vec(),
                    );
                    replay.check(row, 3, &out, exact)?;
                }
                b"EQ" => {
                    // Go's Equal dereferences a nil node for a null array element and panics;
                    // Rust's equal returns false there.
                    let out = vec![num(mergepatch::equal(&a, &b))];
                    replay.check(row, 3, &out, go_panicked)?;
                }
                op => return Err(format!("unknown op {}", quote_to_ascii(op))),
            }
        }
        replay.finish("mergepatch.txt", 40)
    }

    // ---- jsonparser -----------------------------------------------------------------------

    /// Rust's outputs for a jsonparser row, and the column where Go's start.
    fn jsonparser_outputs(op: &[u8], row: &[String]) -> Result<(usize, Vec<Vec<u8>>), String> {
        let data = field(row, 1)?;
        if let Some(parsed) = parse_outputs(op, &data) {
            return Ok((2, parsed));
        }
        if op == b"EACHKEY" {
            let ps = paths(&field(row, 2)?)?;
            let mut items = Vec::new();
            let offset = jsonparser::each_key(&data, &ps, |idx, value, data_type, err| {
                items.extend([
                    num(idx),
                    value.to_vec(),
                    num(data_type.go_value()),
                    parser_err(err.as_ref()),
                ]);
            });
            let mut out = vec![num(offset)];
            out.extend(items);
            return Ok((3, out));
        }
        let keys = path(&field(row, 2)?)?;
        let out = match op {
            b"GET" => {
                let got = jsonparser::get(&data, &keys);
                vec![
                    got.value.to_vec(),
                    num(got.data_type.go_value()),
                    num(got.offset),
                    parser_err(got.err.as_ref()),
                ]
            }
            b"GETSTRING" => result_pair(
                jsonparser::get_string(&data, &keys),
                GoString::into_bytes,
                b"",
                |e| e.go_text(),
            ),
            b"GETINT" => result_pair(jsonparser::get_int(&data, &keys), num, b"0", |e| {
                e.go_text()
            }),
            b"GETBOOL" => result_pair(jsonparser::get_boolean(&data, &keys), num, b"false", |e| {
                e.go_text()
            }),
            b"ARRAYEACH" => {
                let mut items = Vec::new();
                let (offset, err) = jsonparser::array_each(&data, &keys, |value, t, off| {
                    items.extend([value.to_vec(), num(t.go_value()), num(off)]);
                });
                let mut out = vec![num(offset), parser_err(err.as_ref())];
                out.extend(items);
                out
            }
            b"OBJECTEACH" => {
                let mut items = Vec::new();
                let res = jsonparser::object_each::<_, ParserError, _>(
                    &data,
                    &keys,
                    |key, value, t, off| {
                        items.extend([key.to_vec(), value.to_vec(), num(t.go_value()), num(off)]);
                        Ok(())
                    },
                );
                let mut out = vec![parser_err(res.err().as_ref())];
                out.extend(items);
                out
            }
            b"DELETE" => vec![jsonparser::delete(&data, &keys).into_owned()],
            b"SET" => {
                let value = field(row, 3)?;
                let out = result_pair(
                    jsonparser::set(&data, &value, &keys),
                    |v| v,
                    b"",
                    |e| e.go_text(),
                );
                return Ok((4, out));
            }
            _ => return Err(format!("unknown op {}", quote_to_ascii(op))),
        };
        Ok((3, out))
    }

    fn parse_outputs(op: &[u8], input: &[u8]) -> Option<Vec<Vec<u8>>> {
        Some(match op {
            b"PARSESTRING" => result_pair(
                jsonparser::parse_string(input),
                GoString::into_bytes,
                b"",
                |e| e.go_text(),
            ),
            b"PARSEINT" => result_pair(jsonparser::parse_int(input), num, b"0", |e| e.go_text()),
            b"UNESCAPE" => result_pair(jsonparser::unescape(input), Cow::into_owned, b"", |e| {
                e.go_text()
            }),
            _ => return None,
        })
    }

    #[test]
    fn jsonparser_goldens() -> Result<(), String> {
        let mut replay = Replay::default();
        for row in &rows("jsonparser.txt")? {
            let op = field(row, 0)?;
            let (first, out) = jsonparser_outputs(&op, row)?;
            // Go panics for an empty key where an array is, and Delete can index past the end
            // of malformed input; Rust's fallbacks are documented on the functions.
            replay.check(row, first, &out, go_panicked)?;
        }
        replay.finish("jsonparser.txt", 250)
    }

    // ---- streaming decoders -----------------------------------------------------------------

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.streamSub")]
    struct StreamSub {
        #[gojson(go = "ID", json = "id")]
        id: GoString,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.streamDoc")]
    struct StreamDoc {
        #[gojson(go = "Status", json = "status")]
        status: GoString,
        #[gojson(go = "N", json = "n")]
        n: isize,
        #[gojson(go = "F", json = "f")]
        f: f64,
        #[gojson(go = "List", json = "list")]
        list: Option<Vec<GoString>>,
        #[gojson(go = "Ext", json = "ext")]
        ext: RawJson,
        #[gojson(go = "Sub", json = "sub")]
        sub: Option<Box<StreamSub>>,
    }

    fn marshal_text<T: GoEncode + ?Sized>(v: &T) -> Vec<u8> {
        stdjson::marshal(v).unwrap_or_else(|e| concat(b"marshal error: ", &e.go_text()))
    }

    fn stream_err(res: &Result<(), StreamError>) -> Vec<u8> {
        res.as_ref()
            .err()
            .map_or_else(Vec::new, StreamError::go_text)
    }

    fn token_text(token: &Token) -> Vec<u8> {
        match token {
            Token::Delim(c) => vec![b'D', *c],
            Token::Bool(b) => concat(b"B", &num(b)),
            Token::Number(f) => concat(b"N", format!("{:x}", f.to_bits()).as_bytes()),
            Token::String(s) => concat(b"S", s.as_bytes()),
            Token::Null => b"Z".to_vec(),
        }
    }

    fn std_multi(data: &[u8]) -> Vec<Vec<u8>> {
        let mut dec = Decoder::new(data);
        let mut out: Vec<Vec<u8>> = Vec::new();
        for i in 0..8 {
            let mut v = Value::Null;
            let res = dec.decode(&mut v);
            let err = stream_err(&res);
            let repeated = out
                .len()
                .checked_sub(2)
                .and_then(|j| out.get(j))
                .is_some_and(|prev| *prev == err);
            out.extend([marshal_text(&v), err, num(dec.input_offset())]);
            if res.is_err() && i > 0 && repeated {
                break;
            }
        }
        out
    }

    fn tokens(data: &[u8]) -> Vec<Vec<u8>> {
        let mut dec = Decoder::new(data);
        let mut out = Vec::new();
        for _ in 0..64 {
            match dec.token() {
                Ok(token) => {
                    out.extend([token_text(&token), num(dec.input_offset())]);
                    if dec.more() {
                        out.push(b"M".to_vec());
                    }
                }
                Err(err) => {
                    out.extend([concat(b"E", &err.go_text()), num(dec.input_offset())]);
                    break;
                }
            }
        }
        out
    }

    fn stream_outputs(op: &[u8], data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        Ok(match op {
            b"STD_ANY" => {
                let mut dec = Decoder::new(data);
                let mut v = Value::Null;
                let res = dec.decode(&mut v);
                vec![marshal_text(&v), stream_err(&res), num(dec.input_offset())]
            }
            b"STD_DOC" => {
                let mut v = StreamDoc::default();
                let res = stream::decode_std(data, &mut v);
                vec![marshal_text(&v), stream_err(&res)]
            }
            b"STD_MULTI" => std_multi(data),
            b"TOKENS" => tokens(data),
            b"JI_ANY" => {
                let mut v = Value::Null;
                let res = stream::decode_jsoniter(data, &mut v);
                vec![marshal_text(&v), stream_err(&res)]
            }
            b"JI_DOC" => {
                let mut v = StreamDoc::default();
                let res = stream::decode_jsoniter(data, &mut v);
                vec![marshal_text(&v), stream_err(&res)]
            }
            _ => return Err(format!("unknown op {}", quote_to_ascii(op))),
        })
    }

    /// `json.Marshal`'s spelling of U+FFFD for an invalid byte: `�`.
    const ESCAPED_FFFD: &[u8] = b"\x5cufffd";

    /// The documented differences of [`stream::decode_jsoniter`] (`want` and `got` are the
    /// marshaled value and the error).
    ///
    /// Go's jsoniter `Decoder` reads through a 512-byte buffer and, when the input ends, leaves
    /// its position where it was instead of moving it to the end as the in-memory iterator
    /// does: a decode that runs into the end of the input reports another position and context
    /// (and may re-read a byte, or capture a raw message twice). Both fail, which is all that
    /// is compared then. And a Go `interface{}` keeps an invalid UTF-8 byte, which
    /// `json.Marshal` escapes, where `serde_json::Value` holds U+FFFD, which it writes raw.
    fn jsoniter_gap(want: &[Vec<u8>], got: &[Vec<u8>], data: &[u8]) -> bool {
        let failed = |out: &[Vec<u8>]| out.get(1).is_some_and(|e| !e.is_empty());
        if failed(want) && failed(got) {
            return true;
        }
        let mut buf = [0_u8; 4];
        let raw = char::REPLACEMENT_CHARACTER.encode_utf8(&mut buf).as_bytes();
        let escaped = got.first().map(|value| {
            let mut out = Vec::with_capacity(value.len());
            let mut rest = value.as_slice();
            while let Some(i) = rest.windows(raw.len()).position(|w| w == raw) {
                out.extend_from_slice(rest.get(..i).unwrap_or_default());
                out.extend_from_slice(ESCAPED_FFFD);
                rest = rest.get(i + raw.len()..).unwrap_or_default();
            }
            out.extend_from_slice(rest);
            out
        });
        from_utf8(data).is_err() && escaped.as_ref() == want.first() && got.get(1) == want.get(1)
    }

    fn replay_stream() -> Result<(), String> {
        let mut replay = Replay::default();
        for row in &rows("stream.txt")? {
            let op = field(row, 0)?;
            let data = field(row, 1)?;
            let out = stream_outputs(&op, &data)?;
            if op.starts_with(b"JI_") {
                replay.check(row, 2, &out, |want| jsoniter_gap(want, &out, &data))?;
            } else {
                replay.check(row, 2, &out, exact)?;
            }
        }
        replay.finish("stream.txt", 30)
    }

    /// The stream cases nest arrays 10001 deep (both Go libraries stop at 10000).
    #[test]
    fn stream_goldens() -> Result<(), String> {
        thread::Builder::new()
            .stack_size(256 << 20)
            .spawn(replay_stream)
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_panic| "the replay thread panicked".to_owned())?
    }

    // ---- FindElement, DropElement, MergeClone -----------------------------------------------

    fn element_outputs(op: &[u8], ext: &[u8], names: &[&str]) -> Result<Vec<Vec<u8>>, String> {
        let panic = |msg: &str| vec![concat(b"PANIC: ", msg.as_bytes())];
        Ok(match op {
            b"FIND" => {
                let found = jsonutil::find_element(ext, names);
                match found.err {
                    Some(ElementError::Panic(msg)) => panic(&msg),
                    err => vec![
                        num(found.found),
                        num(found.start_index),
                        num(found.end_index),
                        err.map_or_else(Vec::new, |e| e.go_text()),
                    ],
                }
            }
            b"DROP" => match jsonutil::drop_element(ext, names) {
                // Go copies the input with append([]byte(nil), ...), which is nil when empty.
                Ok(out) => vec![out, num(ext.is_empty()), Vec::new()],
                Err(ElementError::Panic(msg)) => panic(&msg),
                Err(err) => vec![Vec::new(), num(true), err.go_text()],
            },
            _ => return Err(format!("unknown op {}", quote_to_ascii(op))),
        })
    }

    #[test]
    fn element_goldens() -> Result<(), String> {
        let mut replay = Replay::default();
        for row in &rows("element.txt")? {
            let op = field(row, 0)?;
            let ext = field(row, 1)?;
            let owned = names(&field(row, 2)?)?;
            let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
            let out = element_outputs(&op, &ext, &refs)?;
            replay.check(row, 3, &out, exact)?;
        }
        replay.finish("element.txt", 0)
    }

    #[test]
    fn mergeclone_goldens() -> Result<(), String> {
        let mut replay = Replay::default();
        for row in &rows("mergeclone.txt")? {
            let given = field(row, 1)?;
            let data = field(row, 2)?;
            let mut raw = given
                .strip_prefix(b"=")
                .map_or_else(RawJson::nil, RawJson::from_slice);
            let res = jsonutil::merge_clone_raw(&mut raw, &data);
            let got = if raw.is_nil() {
                b"nil".to_vec()
            } else {
                concat(b"=", raw.as_bytes())
            };
            let err = res.err().map_or_else(Vec::new, |e| e.message.into_bytes());
            replay.check(row, 3, &[got, err], exact)?;
        }
        replay.finish("mergeclone.txt", 0)
    }

    // ---- ported Go tests --------------------------------------------------------------------

    #[test]
    fn json_patch_merge_tests() -> Result<(), String> {
        // Go: json-patch.v5 merge_test.go (the cases the goldens do not already replay)
        let cases: [(&str, &str, &str); 6] = [
            // TestMergePatchNilArray
            (
                r#"{"a": [ {"b":"c"} ] }"#,
                r#"{"a": [null]}"#,
                r#"{"a":[null]}"#,
            ),
            // TestMergePatchRecursesIntoObjects
            (
                r#"{ "person": { "title": "hello", "age": 18 } }"#,
                r#"{ "person": { "title": "goodbye" } }"#,
                r#"{"person":{"age":18,"title":"goodbye"}}"#,
            ),
            // TestMergePatchReplacesNonObjectsWholesale
            (r#"{ "a": 1, "b": 1 }"#, r#"{ "a": 2 }"#, r#"{"a":2,"b":1}"#),
            // TestMergePatchEmptyArray
            (
                r#"{ "array": ["one", "two"] }"#,
                r#"{ "array": [] }"#,
                r#"{"array":[]}"#,
            ),
            // TestMergeObjArray
            ("[1,2]", r#"{"a":"b","c":null}"#, r#"{"a":"b"}"#),
            // TestMergePatchReplaceKeyNotEscaping
            (
                r#"{ "obj": { "title/escaped": "hello" } }"#,
                r#"{ "obj": { "title/escaped": "goodbye" } }"#,
                r#"{"obj":{"title/escaped":"goodbye"}}"#,
            ),
        ];
        for (doc, patch, want) in cases {
            let got = mergepatch::merge_patch(doc.as_bytes(), patch.as_bytes())
                .map_err(|e| format!("{doc} + {patch}: {e}"))?;
            if got != want.as_bytes() {
                return Err(format!(
                    "{doc} + {patch}: got {}",
                    String::from_utf8_lossy(&got)
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn stream_decoder_walk() -> Result<(), String> {
        let data = br#" {"a": [1, "x", true, null], "b": {}} 7 "#;
        let mut dec = Decoder::new(data);
        let mut seen = String::new();
        loop {
            match dec.token() {
                Ok(token) => {
                    let _written = write!(seen, "{token:?}@{} ", dec.input_offset());
                }
                Err(StreamError::Eof) => break,
                Err(err) => return Err(err.to_string()),
            }
        }
        let want = concat!(
            "Delim(123)@2 String(\"a\")@5 Delim(91)@8 Number(1.0)@9 String(\"x\")@14 ",
            "Bool(true)@20 Null@26 Delim(93)@27 String(\"b\")@32 Delim(123)@35 Delim(125)@36 ",
            "Delim(125)@37 Number(7.0)@39 "
        );
        if seen != want {
            return Err(format!("got {seen}"));
        }
        Ok(())
    }
}
