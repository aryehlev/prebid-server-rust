//! Differential test: replays `testdata/golden/seeded.jsonl`, which `testdata/go/seeded` (a Go
//! program running gojsonschema v1.2.0) wrote. For every schema Prebid loads with gojsonschema,
//! 50,000 params documents come from a `SplitMix64` generator seeded as the line says, built from
//! the property names and values the line lists; Go hashed the documents and their outcomes (the
//! verdict and the error strings, sorted, since Go walks maps in random order), and the port must
//! give the same two digests.

#[cfg(test)]
mod tests {
    use std::{fs, thread};

    use pbs_bidder_params::gojsonschema::Schema;
    use pbs_devtools::repo::go_root;
    use serde_json::Value;
    use sha2::{Digest as _, Sha256};

    /// The most members a generated object has (Go: `maxMembers`).
    const MAX_MEMBERS: u64 = 4;
    /// The deepest a generated object or array nests (Go: `maxDepth`).
    const MAX_DEPTH: usize = 2;

    /// Go: `splitmix64`, the generator both sides run.
    struct SplitMix64(u64);

    impl SplitMix64 {
        const fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Go: `below(n)`: `next() % n`.
        const fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// Go: `generator`: params documents from JSON-encoded names and values.
    struct Generator<'l> {
        rng: SplitMix64,
        names: &'l [String],
        values: &'l [String],
    }

    impl<'l> Generator<'l> {
        fn pick(&mut self, list: &'l [String]) -> &'l str {
            let len = u64::try_from(list.len()).unwrap_or(u64::MAX);
            let i = usize::try_from(self.rng.below(len)).unwrap_or(usize::MAX);
            list.get(i).map_or("", String::as_str)
        }

        fn document(&mut self) -> String {
            if self.rng.below(16) == 0 {
                return self.pick(self.values).to_owned();
            }
            self.object(0)
        }

        fn object(&mut self, depth: usize) -> String {
            let n = self.rng.below(MAX_MEMBERS + 1);
            let mut members = Vec::new();
            for _ in 0..n {
                let key = self.pick(self.names);
                let value = self.value(depth + 1);
                members.push(format!("{key}:{value}"));
            }
            format!("{{{}}}", members.join(","))
        }

        fn value(&mut self, depth: usize) -> String {
            let r = self.rng.below(10);
            if r == 0 && depth < MAX_DEPTH {
                return self.object(depth);
            }
            if r == 1 && depth < MAX_DEPTH {
                let n = self.rng.below(3);
                let items: Vec<String> = (0..n).map(|_| self.value(depth + 1)).collect();
                return format!("[{}]", items.join(","));
            }
            self.pick(self.values).to_owned()
        }
    }

    fn hex(digest: &[u8]) -> String {
        digest
            .iter()
            .flat_map(|b| [b >> 4, b & 0xF])
            .filter_map(|nibble| char::from_digit(u32::from(nibble), 16))
            .collect()
    }

    /// One line of seeded.jsonl, run: `None` when the port gives Go's digests.
    fn run(line: &Value) -> Result<Option<String>, String> {
        let field = |key: &str| {
            line.get(key)
                .ok_or_else(|| format!("seeded line without {key}"))
        };
        let file = field("file")?.as_str().unwrap_or_default();
        let seed = field("seed")?.as_u64().ok_or("seed")?;
        let count = field("count")?.as_u64().ok_or("count")?;
        let list = |key: &str| -> Result<Vec<String>, String> {
            field(key)?
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .ok_or_else(|| format!("{key} is not a list"))
        };
        let (names, values) = (list("names")?, list("values")?);
        let path = go_root().map_err(|e| e.to_string())?.join(file);
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let schema = Schema::new(&bytes).map_err(|e| format!("{file}: {e}"))?;
        let mut generator = Generator {
            rng: SplitMix64(seed),
            names: &names,
            values: &values,
        };
        let (mut inputs, mut outcomes) = (Sha256::new(), Sha256::new());
        let mut first: Option<String> = None;
        for _ in 0..count {
            let doc = generator.document();
            inputs.update(format!("{doc}\n"));
            let (kind, mut errors) = match schema.validate(doc.as_bytes()) {
                Ok(result) if result.valid() => ("ok", Vec::new()),
                Ok(result) => (
                    "invalid",
                    result.errors().iter().map(ToString::to_string).collect(),
                ),
                Err(e) => {
                    let _shown = first.get_or_insert_with(|| format!("{doc}: {e}"));
                    ("decode", Vec::new())
                }
            };
            errors.sort();
            let mut fields = vec![doc, kind.to_owned()];
            fields.extend(errors);
            outcomes.update(format!("{}\n", fields.join("\t")));
        }
        let want = |key: &str| field(key).map(|v| v.as_str().unwrap_or_default().to_owned());
        if hex(&inputs.finalize()) != want("inputs")? {
            return Ok(Some(format!("{file}: the documents differ from Go's")));
        }
        if hex(&outcomes.finalize()) != want("outcomes")? {
            let detail = first.unwrap_or_default();
            return Ok(Some(format!(
                "{file}: the outcomes differ from Go's {detail}"
            )));
        }
        Ok(None)
    }

    /// jsonschema.error-texts and jsonschema.formats over 50,000 seeded params per schema.
    #[test]
    fn seeded_params_match_go() -> Result<(), String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden/seeded.jsonl");
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let lines: Vec<Value> = text
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        if lines.len() != 283 {
            return Err(format!("{} seeded schemas, want 283", lines.len()));
        }
        let workers = thread::available_parallelism().map_or(4, usize::from);
        let chunk = lines.len().div_ceil(workers);
        let results: Vec<Result<Vec<String>, String>> = thread::scope(|scope| {
            // Every worker starts before the first is joined.
            let mut handles = Vec::new();
            for part in lines.chunks(chunk) {
                handles.push(scope.spawn(move || {
                    let mut failures = Vec::new();
                    for line in part {
                        if let Some(failure) = run(line)? {
                            failures.push(failure);
                        }
                    }
                    Ok(failures)
                }));
            }
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err("a worker panicked".to_owned()))
                })
                .collect()
        });
        let mut failures = Vec::new();
        for result in results {
            failures.extend(result?);
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }
}
