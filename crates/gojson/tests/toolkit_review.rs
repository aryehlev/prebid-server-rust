//! Differential replay of the adversarial goldens that `testdata/go/toolkit_review` writes with
//! Go 1.25, json-patch v5.9.0 and jsonparser v1.1.2: JSON escapes in keys and values, every
//! spelling of an array index step, `Set` and `Delete` under every layout, the decoder's token
//! walk, `FindElement`/`DropElement` and `MergeClone` over every shape of existing message. Every
//! output and error must match Go byte for byte.
//!
//! Regenerate the goldens with `go run ./toolkit_review` in `testdata/go`.

#[cfg(test)]
mod toolkit_review {
    use core::str::from_utf8;
    use std::{borrow::Cow, fmt::Write as _, fs, thread};

    use pbs_gojson::{
        GoEncode, GoString, RawJson,
        jsonparser::{self, ParserError},
        jsonutil::{self, ElementError},
        mergepatch, stdjson,
        stream::{Decoder, StreamError, Token},
    };
    use serde_json::Value;

    // ---- golden file support -------------------------------------------------------------

    /// Go's `strconv.QuoteToASCII`, for failure reports.
    fn quote_to_ascii(bytes: &[u8]) -> String {
        let mut out = String::from("\"");
        for chunk in bytes.utf8_chunks() {
            for c in chunk.valid().chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    ' '..='~' => out.push(c),
                    _ => {
                        let _written = write!(out, "\\u{{{:x}}}", u32::from(c));
                    }
                }
            }
            for &b in chunk.invalid() {
                let _written = write!(out, "\\x{b:02x}");
            }
        }
        out.push('"');
        out
    }

    fn hex_value(digits: &[u8]) -> Option<u32> {
        u32::from_str_radix(from_utf8(digits).ok()?, 16).ok()
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

    fn rows(name: &str) -> Result<Vec<Vec<String>>, String> {
        let path = format!(
            "{}/testdata/go/toolkit_review/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
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

    /// A key path written as `<n>:` followed by the keys joined with `\x1f`.
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

    fn paths(encoded: &[u8]) -> Result<Vec<Vec<Vec<u8>>>, String> {
        encoded.split(|&b| b == 0x1E).map(path).collect()
    }

    fn names(encoded: &[u8]) -> Result<Vec<String>, String> {
        path(encoded)?
            .into_iter()
            .map(|n| String::from_utf8(n).map_err(|e| e.to_string()))
            .collect()
    }

    #[derive(Default)]
    struct Replay {
        rows: usize,
        /// Rows where Go panics and Rust takes its documented fallback.
        panics: usize,
        failures: Vec<String>,
    }

    impl Replay {
        fn check(
            &mut self,
            row: &[String],
            first: usize,
            actual: &[Vec<u8>],
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
            if matches!(want.as_slice(), [only] if only.starts_with(b"PANIC: ")) {
                self.panics += 1;
                return Ok(());
            }
            let show = |fields: &[Vec<u8>]| {
                fields
                    .iter()
                    .map(|f| quote_to_ascii(f))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let inputs = row
                .iter()
                .take(first)
                .map(|f| unquote(f))
                .collect::<Result<Vec<_>, _>>()?;
            self.failures.push(format!(
                "{}\n      go:   {}\n      rust: {}",
                show(&inputs),
                show(&want),
                show(actual)
            ));
            Ok(())
        }

        fn finish(self, file: &str, max_panics: usize) -> Result<(), String> {
            if self.rows == 0 {
                return Err(format!("{file}: no rows"));
            }
            if self.panics > max_panics {
                return Err(format!(
                    "{file}: Go panicked on {} rows, expected at most {max_panics}",
                    self.panics
                ));
            }
            if self.failures.is_empty() {
                return Ok(());
            }
            let mut report = format!(
                "{file}: {} of {} rows differ from Go\n",
                self.failures.len(),
                self.rows
            );
            for failure in self.failures.iter().take(30) {
                report.push_str(failure);
                report.push('\n');
            }
            Err(report)
        }
    }

    /// Runs `f` on a thread with a large stack: some goldens nest 10000 deep.
    fn big_stack(f: fn() -> Result<(), String>) -> Result<(), String> {
        thread::Builder::new()
            .stack_size(256 << 20)
            .spawn(f)
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_panic| "the replay thread panicked".to_owned())?
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

    fn replay_mergepatch() -> Result<(), String> {
        let mut replay = Replay::default();
        for row in &rows("mergepatch.txt")? {
            let doc = field(row, 1)?;
            let patch = field(row, 2)?;
            let out = match mergepatch::merge_patch(&doc, &patch) {
                Ok(out) => vec![out, num(false), Vec::new()],
                Err(err) => vec![Vec::new(), num(true), err.as_str().as_bytes().to_vec()],
            };
            replay.check(row, 3, &out)?;
        }
        replay.finish("mergepatch.txt", 0)
    }

    #[test]
    fn mergepatch_review_goldens() -> Result<(), String> {
        big_stack(replay_mergepatch)
    }

    // ---- jsonparser -----------------------------------------------------------------------

    fn parser_err(err: Option<&ParserError>) -> Vec<u8> {
        err.map_or_else(Vec::new, ParserError::go_text)
    }

    fn jsonparser_outputs(op: &[u8], row: &[String]) -> Result<(usize, Vec<Vec<u8>>), String> {
        let data = field(row, 1)?;
        match op {
            b"PARSESTRING" => {
                let out = result_pair(
                    jsonparser::parse_string(&data),
                    GoString::into_bytes,
                    b"",
                    |e| e.go_text(),
                );
                return Ok((2, out));
            }
            b"UNESCAPE" => {
                let out = result_pair(jsonparser::unescape(&data), Cow::into_owned, b"", |e| {
                    e.go_text()
                });
                return Ok((2, out));
            }
            b"EACHKEY" => {
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
            _ => {}
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

    #[test]
    fn jsonparser_review_goldens() -> Result<(), String> {
        let mut replay = Replay::default();
        for row in &rows("jsonparser.txt")? {
            let op = field(row, 0)?;
            let (first, out) = jsonparser_outputs(&op, row)?;
            replay.check(row, first, &out)?;
        }
        replay.finish("jsonparser.txt", 2000)
    }

    // ---- encoding/json Decoder --------------------------------------------------------------

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

    fn tokens(data: &[u8], mixed: bool) -> Vec<Vec<u8>> {
        let mut dec = Decoder::new(data);
        let mut out = Vec::new();
        for i in 0..200 {
            match dec.token() {
                Ok(token) => {
                    out.extend([token_text(&token), num(dec.input_offset())]);
                    if mixed {
                        if matches!(token, Token::String(_)) && i % 2 == 1 {
                            let mut v = Value::Null;
                            let res = dec.decode(&mut v);
                            out.extend([
                                concat(b"V", &marshal_text(&v)),
                                stream_err(&res),
                                num(dec.input_offset()),
                            ]);
                        }
                    } else if dec.more() {
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
            b"STD_RAW" => {
                let mut dec = Decoder::new(data);
                let mut v = RawJson::nil();
                let res = dec.decode(&mut v);
                vec![
                    v.as_bytes().to_vec(),
                    num(v.is_nil()),
                    stream_err(&res),
                    num(dec.input_offset()),
                ]
            }
            b"TOKENS" => tokens(data, false),
            b"MIXED" => tokens(data, true),
            _ => return Err(format!("unknown op {}", quote_to_ascii(op))),
        })
    }

    fn replay_stream() -> Result<(), String> {
        let mut replay = Replay::default();
        for row in &rows("stream.txt")? {
            let op = field(row, 0)?;
            let data = field(row, 1)?;
            let out = stream_outputs(&op, &data)?;
            replay.check(row, 2, &out)?;
        }
        replay.finish("stream.txt", 0)
    }

    #[test]
    fn stream_review_goldens() -> Result<(), String> {
        big_stack(replay_stream)
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
                Ok(out) => vec![out, Vec::new()],
                Err(ElementError::Panic(msg)) => panic(&msg),
                Err(err) => vec![Vec::new(), err.go_text()],
            },
            _ => return Err(format!("unknown op {}", quote_to_ascii(op))),
        })
    }

    #[test]
    fn element_review_goldens() -> Result<(), String> {
        let mut replay = Replay::default();
        for row in &rows("element.txt")? {
            let op = field(row, 0)?;
            let ext = field(row, 1)?;
            let owned = names(&field(row, 2)?)?;
            let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
            let out = element_outputs(&op, &ext, &refs)?;
            // Panics are compared by their runtime error text, so none may be skipped.
            replay.check(row, 3, &out)?;
        }
        replay.finish("element.txt", 0)
    }

    #[test]
    fn mergeclone_review_goldens() -> Result<(), String> {
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
            replay.check(row, 3, &[got, err])?;
        }
        replay.finish("mergeclone.txt", 0)
    }
}
