//! Documents nested 9999, 10000 and 10001 levels deep (the decoders' limit is 10000) through
//! every decode, merge, compare and search entry point, on a 2 MiB thread: Tokio's default
//! worker stack, where Go's growing goroutine stacks have no Rust counterpart. Each case runs
//! on its own thread, named after it, so an overflow names the case that caused it.
//!
//! The expected results and error texts are the goldens that `testdata/go/depth` writes with
//! Go 1.25, json-iterator 1.1.12 and json-patch v5.9.0. Regenerate them with
//! `go run ./depth` in `testdata/go`.

#[cfg(test)]
mod depth {
    use core::{mem, str::from_utf8};
    use std::{collections::BTreeMap, fs, thread};

    use pbs_gojson::{
        GoJson, RawJson, jsoniter,
        jsonutil::{self, drop_element, find_element},
        mergepatch::{equal, merge_patch},
        stdjson, stream,
        value::drop_value,
    };
    use serde_json::Value;

    /// The stack of a Tokio worker thread.
    const STACK: usize = 2 << 20;

    /// The generator's `Wrapped`: the deep value one level down.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Wrapped")]
    struct Wrapped {
        #[gojson(json = "ext,omitempty")]
        ext: RawJson,
        #[gojson(json = "any,omitempty")]
        any: Value,
    }

    impl Wrapped {
        fn dispose(mut self) {
            drop_value(mem::take(&mut self.any));
        }
    }

    /// `levels` levels of `shape`: `array` (`[[...]]`), `object` (`{"a":{"a":...1}}`) or
    /// `mixed` (`{"a":[{"a":[...]}]}`), as the generator builds them.
    fn nested(shape: &[u8], levels: usize) -> Vec<u8> {
        let mut open = Vec::new();
        let mut close = Vec::new();
        for i in 0..levels {
            if shape == b"array" || (shape == b"mixed" && i % 2 == 1) {
                open.push(b'[');
                close.push(b']');
            } else {
                open.extend_from_slice(br#"{"a":"#);
                close.push(b'}');
            }
        }
        if shape == b"object" || (shape == b"mixed" && levels % 2 == 1) {
            open.push(b'1');
        }
        close.reverse();
        open.extend_from_slice(&close);
        open
    }

    /// The generator's `result`: `=` when `out` is the input document.
    fn result(input: &[u8], out: Vec<u8>) -> Vec<u8> {
        if out == input { b"=".to_vec() } else { out }
    }

    fn wrap(key: &str, inner: &[u8]) -> Vec<u8> {
        [format!(r#"{{"{key}":"#).as_bytes(), inner, b"}"].concat()
    }

    fn marshal_or_empty<T: pbs_gojson::GoEncode + ?Sized>(v: &T) -> Vec<u8> {
        jsonutil::marshal(v).unwrap_or_default()
    }

    fn unmarshal_err(res: Result<(), jsonutil::FailedToUnmarshal>) -> Vec<u8> {
        res.err().map_or_else(Vec::new, |e| e.message.into_bytes())
    }

    /// One case: the result and the error text, as the generator records them.
    fn run(api: &[u8], shape: &[u8], levels: usize) -> Result<(Vec<u8>, Vec<u8>), String> {
        let doc = nested(shape, levels);
        let inner = nested(shape, levels.saturating_sub(1));
        let spaced: Vec<u8> = doc
            .iter()
            .flat_map(|&b| if b == b':' { vec![b':', b' '] } else { vec![b] })
            .collect();
        Ok(match api {
            b"jsonutil.Unmarshal" => {
                let mut v = Value::Null;
                let err = unmarshal_err(jsonutil::unmarshal(&doc, &mut v));
                let out = marshal_or_empty(&v);
                drop_value(v);
                (result(&doc, out), err)
            }
            b"jsonutil.Unmarshal map" => {
                let mut m: Option<BTreeMap<String, Value>> = None;
                let err = unmarshal_err(jsonutil::unmarshal(&doc, &mut m));
                let out = marshal_or_empty(&m);
                m.into_iter().flatten().for_each(|(_key, v)| drop_value(v));
                (result(&doc, out), err)
            }
            b"jsonutil.Unmarshal any field" | b"jsonutil.Unmarshal ext field" => {
                let input = wrap(
                    if api.ends_with(b"any field") {
                        "any"
                    } else {
                        "ext"
                    },
                    &inner,
                );
                let mut w = Wrapped::default();
                let err = unmarshal_err(jsonutil::unmarshal(&input, &mut w));
                let out = marshal_or_empty(&w);
                w.dispose();
                (result(&input, out), err)
            }
            b"jsoniter.Unmarshal" => {
                let mut v = Value::Null;
                let err = jsoniter::unmarshal(&doc, &mut v)
                    .err()
                    .map_or_else(Vec::new, |e| e.go_text());
                let out = marshal_or_empty(&v);
                drop_value(v);
                (result(&doc, out), err)
            }
            b"json.Unmarshal" | b"json.Decoder.Decode" => {
                let mut v = Value::Null;
                let err = if api == b"json.Unmarshal" {
                    stdjson::unmarshal(&doc, &mut v)
                        .err()
                        .map_or_else(Vec::new, |e| e.go_text())
                } else {
                    stream::decode_std(&doc, &mut v)
                        .err()
                        .map_or_else(Vec::new, |e| e.go_text())
                };
                let out = stdjson::marshal(&v).unwrap_or_default();
                drop_value(v);
                (result(&doc, out), err)
            }
            b"jsonutil.MergeClone" => {
                let input = wrap("ext", &inner);
                let mut w = Wrapped {
                    ext: RawJson::from_slice(&inner),
                    any: Value::Null,
                };
                let err = unmarshal_err(jsonutil::merge_clone(&mut w, &input));
                let out = marshal_or_empty(&w);
                w.dispose();
                (result(&input, out), err)
            }
            b"jsonpatch.MergePatch" => match merge_patch(&doc, &doc) {
                Ok(out) => (result(&doc, out), Vec::new()),
                Err(e) => (Vec::new(), e.as_str().as_bytes().to_vec()),
            },
            b"jsonpatch.Equal" => (equal(&doc, &doc).to_string().into_bytes(), Vec::new()),
            b"jsonutil.FindElement" => {
                let found = find_element(&doc, &["a"]);
                let out = format!("{} {} {}", found.found, found.start_index, found.end_index);
                let err = found.err.map_or_else(Vec::new, |e| e.go_text());
                (out.into_bytes(), err)
            }
            b"jsonutil.DropElement" => match drop_element(&doc, &["a"]) {
                Ok(out) => (result(&doc, out), Vec::new()),
                Err(e) => (Vec::new(), e.go_text()),
            },
            b"json.Valid" => (stdjson::valid(&doc).to_string().into_bytes(), Vec::new()),
            b"json.Compact" => match stdjson::compact(&spaced) {
                Ok(out) => (result(&doc, out), Vec::new()),
                Err(e) => (Vec::new(), e.go_text()),
            },
            b"jsonutil.Marshal raw" => match jsonutil::marshal(&RawJson::from_slice(&spaced)) {
                Ok(out) => (result(&doc, out), Vec::new()),
                Err(e) => (Vec::new(), e.message.into_bytes()),
            },
            _ => return Err(format!("unknown api {}", String::from_utf8_lossy(api))),
        })
    }

    /// Go's `strconv.Unquote` for the generator's `QuoteToASCII` output (which only escapes
    /// `"` and `\` here).
    fn unquote(field: &str) -> Result<Vec<u8>, String> {
        let inner = field
            .strip_prefix('"')
            .and_then(|f| f.strip_suffix('"'))
            .ok_or_else(|| format!("not a quoted field: {field}"))?;
        let mut out = Vec::with_capacity(inner.len());
        let mut escaped = false;
        for &b in inner.as_bytes() {
            match (escaped, b) {
                (false, b'\\') => escaped = true,
                (true, b'"' | b'\\') | (false, _) => {
                    out.push(b);
                    escaped = false;
                }
                (true, _) => return Err(format!("unexpected escape in {field}")),
            }
        }
        Ok(out)
    }

    /// A short, printable form of a field for failure messages.
    fn show(bytes: &[u8]) -> String {
        let text = String::from_utf8_lossy(bytes);
        if text.len() > 160 {
            let head: String = text.chars().take(80).collect();
            let tail: String = text
                .chars()
                .skip(text.chars().count().saturating_sub(60))
                .collect();
            format!("{head}...{tail} ({} bytes)", bytes.len())
        } else {
            text.into_owned()
        }
    }

    /// The entry points the generator covers, in its order.
    const APIS: [&str; 15] = [
        "jsonutil.Unmarshal",
        "jsonutil.Unmarshal map",
        "jsonutil.Unmarshal any field",
        "jsonutil.Unmarshal ext field",
        "jsoniter.Unmarshal",
        "json.Unmarshal",
        "json.Decoder.Decode",
        "jsonutil.MergeClone",
        "jsonpatch.MergePatch",
        "jsonpatch.Equal",
        "jsonutil.FindElement",
        "jsonutil.DropElement",
        "json.Valid",
        "json.Compact",
        "jsonutil.Marshal raw",
    ];

    /// The golden rows: api, shape, levels, result and error.
    fn rows() -> Result<Vec<[Vec<u8>; 5]>, String> {
        let path = format!("{}/testdata/go/depth/depth.txt", env!("CARGO_MANIFEST_DIR"));
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        text.lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .map(|line| {
                let fields = line
                    .split('\t')
                    .map(unquote)
                    .collect::<Result<Vec<_>, _>>()?;
                <[Vec<u8>; 5]>::try_from(fields).map_err(|_fields| format!("bad row {line}"))
            })
            .collect()
    }

    /// Replays the goldens of `api` (three shapes, each 9999, 10000 and 10001 levels deep),
    /// each case on its own 2 MiB thread.
    fn replay(api: &str) -> Result<(), String> {
        let mut cases = 0_usize;
        let mut failures = Vec::new();
        for [row_api, shape, levels, want, want_err] in rows()? {
            if row_api != api.as_bytes() {
                continue;
            }
            let levels: usize = from_utf8(&levels)
                .map_err(|e| e.to_string())?
                .parse()
                .map_err(|e| format!("bad levels: {e}"))?;
            let name = format!("{api} {} {levels}", String::from_utf8_lossy(&shape));
            let (got, got_err) = thread::Builder::new()
                .name(name.clone())
                .stack_size(STACK)
                .spawn(move || run(&row_api, &shape, levels))
                .map_err(|e| e.to_string())?
                .join()
                .map_err(|_panic| format!("{name}: the thread panicked"))??;
            cases += 1;
            if got != want || got_err != want_err {
                failures.push(format!(
                    "{name}\n  got  {} err {}\n  want {} err {}",
                    show(&got),
                    show(&got_err),
                    show(&want),
                    show(&want_err)
                ));
            }
        }
        if cases != 9 {
            return Err(format!("{api}: {cases} cases, want 9"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "{} of {cases} cases differ from Go:\n{}",
                failures.len(),
                failures.join("\n")
            ))
        }
    }

    #[test]
    fn every_row_is_replayed() -> Result<(), String> {
        let rows = rows()?;
        for [api, ..] in &rows {
            if !APIS.iter().any(|known| known.as_bytes() == api.as_slice()) {
                return Err(format!("no test replays {}", String::from_utf8_lossy(api)));
            }
        }
        if rows.len() == APIS.len() * 9 {
            Ok(())
        } else {
            Err(format!("{} rows, want {}", rows.len(), APIS.len() * 9))
        }
    }

    #[test]
    fn jsonutil_unmarshal() -> Result<(), String> {
        replay("jsonutil.Unmarshal")
    }

    #[test]
    fn jsonutil_unmarshal_map() -> Result<(), String> {
        replay("jsonutil.Unmarshal map")
    }

    #[test]
    fn jsonutil_unmarshal_any_field() -> Result<(), String> {
        replay("jsonutil.Unmarshal any field")
    }

    #[test]
    fn jsonutil_unmarshal_ext_field() -> Result<(), String> {
        replay("jsonutil.Unmarshal ext field")
    }

    #[test]
    fn jsoniter_unmarshal() -> Result<(), String> {
        replay("jsoniter.Unmarshal")
    }

    #[test]
    fn json_unmarshal() -> Result<(), String> {
        replay("json.Unmarshal")
    }

    #[test]
    fn json_decoder_decode() -> Result<(), String> {
        replay("json.Decoder.Decode")
    }

    #[test]
    fn jsonutil_merge_clone() -> Result<(), String> {
        replay("jsonutil.MergeClone")
    }

    #[test]
    fn jsonpatch_merge_patch() -> Result<(), String> {
        replay("jsonpatch.MergePatch")
    }

    #[test]
    fn jsonpatch_equal() -> Result<(), String> {
        replay("jsonpatch.Equal")
    }

    #[test]
    fn jsonutil_find_element() -> Result<(), String> {
        replay("jsonutil.FindElement")
    }

    #[test]
    fn jsonutil_drop_element() -> Result<(), String> {
        replay("jsonutil.DropElement")
    }

    #[test]
    fn json_valid() -> Result<(), String> {
        replay("json.Valid")
    }

    #[test]
    fn json_compact() -> Result<(), String> {
        replay("json.Compact")
    }

    #[test]
    fn jsonutil_marshal_raw() -> Result<(), String> {
        replay("jsonutil.Marshal raw")
    }
}
