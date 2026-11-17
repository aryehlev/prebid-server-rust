//! Decoders replacing or discarding a value nested 9990 levels deep (a duplicate key, a field
//! decoded twice, a slice that shrinks, `null` into a pointer, slice or map, and `MergeClone`
//! resetting a slice or merging a map), each case on its own 2 MiB thread: Tokio's default
//! worker stack. Dropping such a `serde_json::Value` recursively overflows that stack in debug
//! builds, so the decoders drop what they discard with heap memory instead.
//!
//! The expected results and error texts are the goldens that `testdata/go/deepdrop` writes
//! with Go 1.25 and json-iterator 1.1.12. Regenerate them from the repository root with
//! `go run ./rust/crates/gojson/testdata/go/deepdrop`.

#[cfg(test)]
mod deepdrop {
    use core::mem;
    use std::{collections::BTreeMap, fs, thread};

    use pbs_gojson::{GoJson, jsonutil, stdjson, value::drop_value};
    use serde_json::{Map, Value};

    /// The stack of a Tokio worker thread.
    const STACK: usize = 2 << 20;

    /// How deep the generator nests `D`.
    const LEVELS: usize = 9990;

    /// The generator's `Deep`: the deep value in every kind of field that can discard one.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Deep")]
    struct Deep {
        #[gojson(json = "m,omitempty")]
        m: Option<BTreeMap<String, Value>>,
        #[gojson(json = "n,omitempty")]
        n: BTreeMap<String, Value>,
        #[gojson(json = "o,omitempty")]
        o: Option<Map<String, Value>>,
        #[gojson(json = "l,omitempty")]
        l: Option<Vec<Value>>,
        #[gojson(json = "p,omitempty")]
        p: Option<Value>,
        #[gojson(json = "v,omitempty")]
        v: Value,
    }

    impl Deep {
        /// Drops the struct without recursing through a deep value a decoder failed to drop
        /// (the results are compared first, so such a value fails the case instead).
        fn dispose(mut self) {
            self.m
                .take()
                .into_iter()
                .flatten()
                .for_each(|(_key, v)| drop_value(v));
            mem::take(&mut self.n).into_values().for_each(drop_value);
            if let Some(o) = self.o.take() {
                drop_value(Value::Object(o));
            }
            self.l.take().into_iter().flatten().for_each(drop_value);
            if let Some(p) = self.p.take() {
                drop_value(p);
            }
            drop_value(mem::take(&mut self.v));
        }
    }

    /// `D`: `{"a":{"a":...1}}` nested [`LEVELS`] deep.
    fn deep() -> String {
        format!("{}1{}", r#"{"a":"#.repeat(LEVELS), "}".repeat(LEVELS))
    }

    /// Go's `strconv.Unquote` for the generator's `QuoteToASCII` output (which only escapes
    /// `"` and `\` here).
    fn unquote(field: &str) -> Result<String, String> {
        let inner = field
            .strip_prefix('"')
            .and_then(|f| f.strip_suffix('"'))
            .ok_or_else(|| format!("not a quoted field: {field}"))?;
        let mut out = String::with_capacity(inner.len());
        let mut escaped = false;
        for c in inner.chars() {
            match (escaped, c) {
                (false, '\\') => escaped = true,
                (true, '"' | '\\') | (false, _) => {
                    out.push(c);
                    escaped = false;
                }
                (true, _) => return Err(format!("unexpected escape in {field}")),
            }
        }
        Ok(out)
    }

    /// The golden rows: api, target, base, input, result and error.
    fn rows() -> Result<Vec<[String; 6]>, String> {
        let path = format!(
            "{}/testdata/go/deepdrop/deepdrop.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        text.lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .map(|line| {
                let fields = line
                    .split('\t')
                    .map(unquote)
                    .collect::<Result<Vec<_>, _>>()?;
                <[String; 6]>::try_from(fields).map_err(|_fields| format!("bad row {line}"))
            })
            .collect()
    }

    fn jsonutil_err(res: Result<(), jsonutil::FailedToUnmarshal>) -> String {
        res.err().map_or_else(String::new, |e| e.to_string())
    }

    fn std_err(res: Result<(), pbs_gojson::JsonError>) -> String {
        res.err().map_or_else(String::new, |e| {
            String::from_utf8_lossy(&e.go_text()).into_owned()
        })
    }

    fn text(out: &[u8]) -> String {
        String::from_utf8_lossy(out).into_owned()
    }

    /// Decodes `input` into a zero `target` with `std` (`encoding/json`) or jsonutil, and
    /// returns the re-encoded result and the error text.
    fn decode(std: bool, target: &str, input: &[u8]) -> Result<(String, String), String> {
        Ok(match target {
            "any" => {
                let mut v = Value::Null;
                let out = if std {
                    let err = std_err(stdjson::unmarshal(input, &mut v));
                    (text(&stdjson::marshal(&v).map_err(|e| e.to_string())?), err)
                } else {
                    let err = jsonutil_err(jsonutil::unmarshal(input, &mut v));
                    (
                        text(&jsonutil::marshal(&v).map_err(|e| e.to_string())?),
                        err,
                    )
                };
                drop_value(v);
                out
            }
            "map" => {
                let mut m: Option<BTreeMap<String, Value>> = None;
                let out = if std {
                    let err = std_err(stdjson::unmarshal(input, &mut m));
                    (text(&stdjson::marshal(&m).map_err(|e| e.to_string())?), err)
                } else {
                    let err = jsonutil_err(jsonutil::unmarshal(input, &mut m));
                    (
                        text(&jsonutil::marshal(&m).map_err(|e| e.to_string())?),
                        err,
                    )
                };
                m.into_iter().flatten().for_each(|(_key, v)| drop_value(v));
                out
            }
            "struct" => {
                let mut d = Deep::default();
                let out = if std {
                    let err = std_err(stdjson::unmarshal(input, &mut d));
                    (text(&stdjson::marshal(&d).map_err(|e| e.to_string())?), err)
                } else {
                    let err = jsonutil_err(jsonutil::unmarshal(input, &mut d));
                    (
                        text(&jsonutil::marshal(&d).map_err(|e| e.to_string())?),
                        err,
                    )
                };
                d.dispose();
                out
            }
            other => return Err(format!("unknown target {other}")),
        })
    }

    /// `jsonutil.MergeClone(base, input)` onto a `Deep` decoded from `base`.
    fn merge(base: &[u8], input: &[u8]) -> Result<(String, String), String> {
        let mut d = Deep::default();
        jsonutil::unmarshal(base, &mut d).map_err(|e| format!("base: {e}"))?;
        let err = jsonutil_err(jsonutil::merge_clone(&mut d, input));
        let out = text(&jsonutil::marshal(&d).map_err(|e| e.to_string())?);
        d.dispose();
        Ok((out, err))
    }

    /// One golden row with `D` substituted.
    fn run(api: &str, target: &str, base: &str, input: &str) -> Result<(String, String), String> {
        let d = deep();
        let input = input.replace('D', &d);
        match api {
            "jsonutil.Unmarshal" => decode(false, target, input.as_bytes()),
            "json.Unmarshal" => decode(true, target, input.as_bytes()),
            "jsonutil.MergeClone" => merge(base.replace('D', &d).as_bytes(), input.as_bytes()),
            other => Err(format!("unknown api {other}")),
        }
    }

    #[test]
    fn deep_values_are_dropped_on_a_small_stack() -> Result<(), String> {
        let mut cases = 0_usize;
        let mut failures = Vec::new();
        for [api, target, base, input, want, want_err] in rows()? {
            let name = format!("{api} {target} {base} {input}");
            let (got, got_err) = thread::Builder::new()
                .name(name.clone())
                .stack_size(STACK)
                .spawn(move || run(&api, &target, &base, &input))
                .map_err(|e| e.to_string())?
                .join()
                .map_err(|_panic| format!("{name}: the thread panicked"))??;
            cases += 1;
            if got != want || got_err != want_err {
                failures.push(format!(
                    "{name}\n  got  {got} err {got_err}\n  want {want} err {want_err}"
                ));
            }
        }
        if cases != 47 {
            return Err(format!("{cases} cases, want 47"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }
}
