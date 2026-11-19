//! The loader against Viper v1.12.0, mapstructure v1.5.0 and cast v1.5.0 as `config.SetupViper`
//! and `config.New` use them: the loads and the key matrix the Go program of `testdata/go`
//! records (`testdata/golden/loads.json` and `matrix.json`), and the defaults table at the
//! workspace root (`testdata/go_config_defaults.json`).

pub mod support;

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        env, fs,
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use config::{Config, Environment, File, Map, Source as _};
    use pbs_config::loader::{
        Decoded, GoValue, LoadError, Loader, SUPPORTED_EXTS, Settings, YamlFormat,
    };
    use serde::{Serialize, de::DeserializeOwned};
    use serde_json::{Map as JsonMap, Value as Json, json};

    use super::support::{TestResult, at, check_eq, diff, golden, list, report, text};

    /// One load: the files of the working directory and of the directory that stands for
    /// `/etc/config`, the environment, the bidders, whether a file is searched, and the keys
    /// whose `IsSet` and `Get` are recorded (`loads.go`'s `loadInput`).
    #[derive(Debug, Clone, Default)]
    struct Input {
        /// The load's name.
        name: String,
        /// The working directory's files; a name ending in `/` is a directory.
        files: BTreeMap<String, Vec<u8>>,
        /// The second directory's files, when the load has one.
        second: Option<BTreeMap<String, Vec<u8>>>,
        /// The environment.
        env: BTreeMap<String, String>,
        /// The bidders whose keys are bound.
        bidders: Vec<String>,
        /// Whether no file is searched.
        no_file: bool,
        /// The probed keys.
        probes: Vec<String>,
    }

    /// The object of strings at `value`, empty when absent.
    fn string_map(value: Option<&Json>) -> TestResult<BTreeMap<String, String>> {
        let Some(value) = value else {
            return Ok(BTreeMap::new());
        };
        value
            .as_object()
            .ok_or_else(|| format!("{value} is not an object"))?
            .iter()
            .map(|(k, v)| {
                v.as_str()
                    .map(|v| (k.clone(), v.to_owned()))
                    .ok_or_else(|| format!("{k}: {v} is not a string"))
            })
            .collect()
    }

    /// The list of strings at `value`, empty when absent.
    fn string_list(value: Option<&Json>) -> TestResult<Vec<String>> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        value
            .as_array()
            .ok_or_else(|| format!("{value} is not a list"))?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{v} is not a string"))
            })
            .collect()
    }

    /// Files as bytes.
    fn file_map(value: Option<&Json>) -> TestResult<BTreeMap<String, Vec<u8>>> {
        Ok(string_map(value)?
            .into_iter()
            .map(|(name, contents)| (name, contents.into_bytes()))
            .collect())
    }

    /// The bytes `loads.go` records for a file that is not UTF-8.
    fn file_bytes(value: &Json) -> TestResult<Vec<u8>> {
        value
            .as_array()
            .ok_or_else(|| format!("{value} is not a list"))?
            .iter()
            .map(|b| {
                b.as_u64()
                    .and_then(|b| u8::try_from(b).ok())
                    .ok_or_else(|| format!("{b} is not a byte"))
            })
            .collect()
    }

    impl Input {
        /// The input of a golden load.
        fn from_golden(load: &Json) -> TestResult<Self> {
            let mut files = file_map(load.get("files"))?;
            if let Some(raw) = load.get("file_bytes").and_then(Json::as_object) {
                for (name, bytes) in raw {
                    let _text = files.insert(name.clone(), file_bytes(bytes)?);
                }
            }
            Ok(Self {
                name: text(load, "/name")?.to_owned(),
                files,
                second: load
                    .get("second")
                    .map(|second| file_map(Some(second)))
                    .transpose()?,
                env: string_map(load.get("env"))?,
                bidders: string_list(load.get("bidders"))?,
                no_file: load.get("no_file").and_then(Json::as_bool) == Some(true),
                probes: string_list(load.get("probes"))?,
            })
        }
    }

    /// A temporary directory holding a load's files, removed when dropped.
    #[derive(Debug)]
    struct TempDir(PathBuf);

    impl TempDir {
        /// A new directory holding `files`.
        fn new(files: &BTreeMap<String, Vec<u8>>) -> TestResult<Self> {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("pbs-config-loader-{}-{n}", process::id()));
            let _stale = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            for (name, contents) in files {
                let path = dir.join(name);
                let written = if name.ends_with('/') {
                    fs::create_dir_all(&path)
                } else {
                    fs::write(&path, contents)
                };
                written.map_err(|e| format!("{}: {e}", path.display()))?;
            }
            Ok(Self(dir))
        }

        /// The directory's path, as written and with its symbolic links resolved.
        fn spellings(&self) -> Vec<String> {
            let mut out = vec![self.0.display().to_string()];
            if let Ok(real) = fs::canonicalize(&self.0) {
                out.push(real.display().to_string());
            }
            out
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _removed = fs::remove_dir_all(&self.0);
        }
    }

    /// What one load gave, in the goldens' terms.
    #[derive(Debug)]
    struct Outcome {
        /// The settings, or the loader's error (a format this port does not read).
        settings: Result<Settings, LoadError>,
        /// `ReadInConfig`'s error with the directories named `<wd>` and `<second>`.
        read_error: String,
        /// `first:<file>` or `second:<file>`, or empty.
        file_used: String,
        /// The decoded configuration, or mapstructure's error text.
        decoded: Result<Decoded, String>,
    }

    impl Outcome {
        /// `v.AllSettings()` in the compact form.
        fn settings_json(&self) -> Json {
            self.settings.as_ref().map_or(Json::Null, |s| {
                GoValue::Map(s.all_settings().clone()).compact()
            })
        }

        /// The decode error, empty when it decoded.
        fn error(&self) -> &str {
            self.decoded.as_ref().err().map_or("", String::as_str)
        }

        /// `IsSet` and `Get` of each key.
        fn probes(&self, keys: &[String]) -> Json {
            let Ok(settings) = &self.settings else {
                return Json::Null;
            };
            Json::Object(
                keys.iter()
                    .map(|key| {
                        let get = settings.get(key).map_or(Json::Null, |v| v.compact());
                        (
                            key.clone(),
                            json!({"is_set": settings.is_set(key), "get": get}),
                        )
                    })
                    .collect(),
            )
        }

        /// The `UnmarshalKey` calls of `config.New` for each bidder the decoded configuration
        /// holds, as `loads.go` records them; `None` when it records none.
        fn unmarshal_key(&self) -> Option<Json> {
            let (Ok(settings), Ok(decoded)) = (&self.settings, &self.decoded) else {
                return None;
            };
            let Some(Decoded::Map(Some(bidders))) = decoded.field("adapters") else {
                return None;
            };
            if bidders.is_empty() {
                return None;
            }
            let mut out = JsonMap::new();
            for (bidder, _) in bidders {
                let Decoded::String(bidder) = bidder else {
                    continue;
                };
                for suffix in [".disabled", ".modifyingvastxmlallowed"] {
                    let key = format!("adapters.{bidder}{suffix}");
                    let result = match settings.unmarshal_key_bool(&key) {
                        Ok(value) => json!({"value": value, "error": ""}),
                        Err(error) => json!({"value": null, "error": error}),
                    };
                    let _previous = out.insert(key, result);
                }
            }
            Some(Json::Object(out))
        }
    }

    /// Loads `input` as `loads.go`'s `withViper` and `runOnce` do.
    fn run(input: &Input) -> TestResult<Outcome> {
        let wd = TempDir::new(&input.files)?;
        let second = input.second.as_ref().map(TempDir::new).transpose()?;
        let etc = second
            .as_ref()
            .map_or_else(|| PathBuf::from("/etc/config"), |dir| dir.0.clone());
        let mut loader = Loader::new()
            .search_paths([wd.0.clone(), etc])
            .env(input.env.clone())
            .bidders(input.bidders.clone());
        if input.no_file {
            loader = loader.without_file();
        }
        let settings = match loader.load() {
            Ok(settings) => settings,
            Err(e) => {
                return Ok(Outcome {
                    settings: Err(e),
                    read_error: String::new(),
                    file_used: String::new(),
                    decoded: Err(String::new()),
                });
            }
        };
        let mut read_error = settings.read_error().unwrap_or_default().to_owned();
        for (dir, name) in [(Some(&wd), "<wd>"), (second.as_ref(), "<second>")] {
            for spelling in dir.map_or_default(TempDir::spellings) {
                read_error = read_error.replace(&spelling, name);
            }
        }
        let file_used = match (settings.file_used(), settings.file_search_index()) {
            (Some(path), Some(index)) => {
                let base = path
                    .file_name()
                    .map_or_default(|name| name.to_string_lossy().into_owned());
                match (index, second.is_some()) {
                    (0, _) => format!("first:{base}"),
                    (1, true) => format!("second:{base}"),
                    _ => return Err(format!("{}: read {}", input.name, path.display())),
                }
            }
            _ => String::new(),
        };
        let decoded = settings
            .decode()
            .map_err(|e| e.mapstructure_text().to_owned());
        Ok(Outcome {
            settings: Ok(settings),
            read_error,
            file_used,
            decoded,
        })
    }

    /// The defaults load's trees, the base of every load's diff.
    #[derive(Debug)]
    struct Base {
        /// `v.AllSettings()` of the defaults load.
        settings: Json,
        /// Its decoded configuration.
        decoded: Json,
    }

    impl Base {
        /// The base of `loads.json`.
        fn of(doc: &Json) -> TestResult<Self> {
            Ok(Self {
                settings: at(doc, "/defaults/settings")?.clone(),
                decoded: at(doc, "/defaults/decoded")?.clone(),
            })
        }
    }

    /// The path of a golden diff entry.
    fn path_of(value: &Json) -> TestResult<Vec<String>> {
        string_list(Some(value))
    }

    /// `base` with a golden diff (`loads.go`'s `treeDiff`) applied: Go's whole tree.
    fn apply(base: &Json, change: &Json) -> TestResult<Json> {
        let mut tree = base.clone();
        for path in list(change, "/unset")? {
            remove_leaf(&mut tree, &path_of(path)?);
        }
        for entry in list(change, "/set")? {
            set_leaf(
                &mut tree,
                &path_of(at(entry, "/path")?)?,
                at(entry, "/value")?,
            );
        }
        Ok(tree)
    }

    /// Removes the leaf at `path`, and the objects its removal leaves empty.
    fn remove_leaf(tree: &mut Json, path: &[String]) {
        let Some((first, rest)) = path.split_first() else {
            return;
        };
        let Some(map) = tree.as_object_mut() else {
            return;
        };
        if rest.is_empty() {
            let _removed = map.remove(first);
            return;
        }
        if let Some(child) = map.get_mut(first) {
            remove_leaf(child, rest);
            if child.as_object().is_some_and(JsonMap::is_empty) {
                let _removed = map.remove(first);
            }
        }
    }

    /// Sets the leaf at `path` to `value`, making objects of its parents.
    fn set_leaf(tree: &mut Json, path: &[String], value: &Json) {
        let Some((first, rest)) = path.split_first() else {
            *tree = value.clone();
            return;
        };
        if !tree.is_object() {
            *tree = Json::Object(JsonMap::new());
        }
        if let Some(map) = tree.as_object_mut() {
            let child = map.entry(first.clone()).or_insert(Json::Null);
            set_leaf(child, rest, value);
        }
    }

    /// Whether a read error's text is the one Go writes, rather than only an error in both:
    /// Viper's own text when no file is found is; the readers word their parse errors their own
    /// way (ledger: `config.json-reader`, `config.toml-reader`, `yaml.error-texts`).
    const fn exact_read_error(file_used: &str) -> bool {
        file_used.is_empty()
    }

    /// How `got` differs from the golden result `want` of a load of `input`.
    fn compare(input: &Input, got: &Outcome, want: &Json, base: &Base) -> TestResult<Vec<String>> {
        let mut out = Vec::new();
        if let Err(e) = &got.settings {
            out.push(format!("the load failed: {e}"));
            return Ok(out);
        }
        let go_read = text(want, "/read_error")?;
        let go_file = text(want, "/file_used")?;
        if go_read.is_empty() != got.read_error.is_empty()
            || (exact_read_error(go_file) && go_read != got.read_error)
        {
            out.push(format!(
                "read error: got {:?}, want {go_read:?}",
                got.read_error
            ));
        }
        if go_file != got.file_used {
            out.push(format!(
                "file used: got {:?}, want {go_file:?}",
                got.file_used
            ));
        }
        let go_error = text(want, "/error")?;
        if go_error != got.error() {
            out.push(format!(
                "decode error: got {:?}, want {go_error:?}",
                got.error()
            ));
        }
        let settings = apply(&base.settings, at(want, "/settings")?)?;
        out.extend(
            diff(&got.settings_json(), &settings)
                .into_iter()
                .map(|d| format!("settings {d}")),
        );
        if let (Some(change), Ok(decoded)) = (want.get("decoded"), &got.decoded) {
            let decoded_go = apply(&base.decoded, change)?;
            out.extend(
                diff(&decoded.compact(), &decoded_go)
                    .into_iter()
                    .map(|d| format!("decoded {d}")),
            );
        }
        if let Some(probes) = want.get("probe_results") {
            out.extend(
                diff(&got.probes(&input.probes), probes)
                    .into_iter()
                    .map(|d| format!("probe {d}")),
            );
        }
        let unmarshal_key = got.unmarshal_key();
        if want.get("unmarshal_key") != unmarshal_key.as_ref() {
            out.push(format!(
                "UnmarshalKey: got {unmarshal_key:?}, want {:?}",
                want.get("unmarshal_key")
            ));
        }
        Ok(out)
    }

    /// The loads whose result differs from Go's, each with the ledger entry that records why; the
    /// entry's pin shows both results.
    const LEDGERED_LOADS: &[(&str, &str)] = &[
        (
            "an unread format in the working directory first",
            "config.formats",
        ),
        ("format properties", "config.formats"),
        ("format props", "config.formats"),
        ("format prop", "config.formats"),
        ("format hcl", "config.formats"),
        ("format tfvars", "config.formats"),
        ("format dotenv", "config.formats"),
        ("format env", "config.formats"),
        ("format ini", "config.formats"),
        ("invalid utf-8 in a json string", "config.json-reader"),
        ("a lone surrogate in a json string", "config.json-reader"),
        ("json nested 200 levels", "config.json-reader"),
        ("module json spelling", "config.module-json"),
        ("toml date-time", "config.toml-reader"),
        ("toml local date in modules", "config.toml-reader"),
        ("yaml timestamps", "yaml.scalars"),
        ("failing values are printed", "config.error-texts"),
        (
            "account modules as a list with a key that is not a string",
            "config.hook-panic",
        ),
    ];

    /// The ledger entry of a load whose result differs from Go's.
    fn ledger_entry(name: &str) -> Option<&'static str> {
        LEDGERED_LOADS
            .iter()
            .find(|(load, _)| *load == name)
            .map(|(_, entry)| *entry)
    }

    /// Runs every load of `loads.json` that `select` picks, ordered and unordered: a ledgered
    /// load must still differ from Go's result, and any other must give it.
    fn check_loads(select: &dyn Fn(&str) -> bool) -> TestResult {
        let doc = golden("loads.json")?;
        let base = Base::of(&doc)?;
        let mut failures = Vec::new();
        let mut ran = 0_usize;
        for load in list(&doc, "/loads")? {
            let input = Input::from_golden(load)?;
            if !select(&input.name) {
                continue;
            }
            ran += 1;
            let got = run(&input)?;
            let found = compare(&input, &got, load, &base)?;
            match ledger_entry(&input.name) {
                Some(entry) if found.is_empty() => failures.push(format!(
                    "{}: gives Go's result, so ledger entry {entry} no longer covers it",
                    input.name
                )),
                Some(_) => {}
                None => {
                    for failure in found {
                        failures.push(format!("{}: {failure}", input.name));
                    }
                }
            }
        }
        for load in list(&doc, "/unordered")? {
            let input = Input::from_golden(load)?;
            if !select(&input.name) {
                continue;
            }
            ran += 1;
            let got = run(&input)?;
            let mut nearest: Option<Vec<String>> = None;
            for outcome in list(load, "/outcomes")? {
                let found = compare(&input, &got, outcome, &base)?;
                if nearest.as_ref().is_none_or(|n| found.len() < n.len()) {
                    nearest = Some(found);
                }
            }
            for failure in nearest.unwrap_or_default() {
                failures.push(format!("{} (no outcome of Go's): {failure}", input.name));
            }
        }
        if ran == 0 {
            failures.push("no load selected".to_owned());
        }
        report("loads.json", &failures)
    }

    /// Fails unless `loads.json` has a load of each name.
    fn check_known(names: &[&str]) -> TestResult {
        let doc = golden("loads.json")?;
        let mut known = BTreeSet::new();
        for load in list(&doc, "/loads")?
            .iter()
            .chain(list(&doc, "/unordered")?)
        {
            let _new = known.insert(text(load, "/name")?.to_owned());
        }
        let missing: Vec<String> = names
            .iter()
            .filter(|name| !known.contains(**name))
            .map(|name| format!("loads.json has no load {name:?}"))
            .collect();
        report("the listed loads", &missing)
    }

    /// Runs the loads named in `names` with [`check_loads`], after checking that `loads.json` has
    /// each of them.
    fn check_listed(names: &[&str]) -> TestResult {
        check_known(names)?;
        check_loads(&|name| names.contains(&name))
    }

    /// Every load of `loads.json` gives Go's result.
    // Go: config/config.go:936-1415 (SetupViper) and config/config.go:756-760 (config.New's
    // v.Unmarshal), as main.go:59-66 (loadConfig) runs them
    #[test]
    fn loads_match_viper() -> TestResult {
        let ledgered: Vec<&str> = LEDGERED_LOADS.iter().map(|(name, _)| *name).collect();
        check_known(&ledgered)?;
        check_loads(&|_| true)
    }

    /// The variable Viper reads for `key`: `PBS_` and the key upper-cased, `.` replaced by `_`
    /// (`matrix.go`'s `envName`).
    fn env_name(key: &str) -> String {
        format!("PBS_{}", key.to_uppercase()).replace('.', "_")
    }

    /// `value` at `key` as nested YAML mappings (`matrix.go`'s `nestedYAML`).
    fn nested_yaml(key: &str, value: &str) -> String {
        let parts: Vec<&str> = key.split('.').collect();
        let mut out = String::new();
        for (i, part) in parts.iter().enumerate() {
            out.push_str(&"  ".repeat(i));
            out.push_str(part);
            out.push(':');
            if i + 1 == parts.len() {
                out.push(' ');
                out.push_str(value);
            }
            out.push('\n');
        }
        out
    }

    /// The decoded value at a Viper key, as `encode.go`'s `leaf` follows it: struct fields by
    /// their key under Unicode case folding, map entries by key, through pointers.
    fn leaf(root: &Decoded, key: &str) -> Json {
        let absent = json!({"absent": true});
        let mut value = root;
        for step in key.split('.') {
            while let Decoded::Ptr(target) = value {
                match target {
                    Some(target) => value = target,
                    None => return absent,
                }
            }
            let next = match value {
                Decoded::Struct(fields) => fields
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(step))
                    .map(|(_, field)| field),
                Decoded::Map(Some(entries)) => entries
                    .iter()
                    .find(|(k, _)| matches!(k, Decoded::String(k) if k == step))
                    .map(|(_, entry)| entry),
                Decoded::Bool(_)
                | Decoded::Int(_)
                | Decoded::Uint(_)
                | Decoded::Float(_)
                | Decoded::String(_)
                | Decoded::Slice(_)
                | Decoded::Bytes(_)
                | Decoded::Array(_)
                | Decoded::Map(None)
                | Decoded::Ptr(_)
                | Decoded::Interface(_) => None,
            };
            match next {
                Some(next) => value = next,
                None => return absent,
            }
        }
        json!({"value": value.compact()})
    }

    /// The matrix row's load: `PBS_<KEY>` alone, or `pbs.yaml` with the key nested.
    fn matrix_input(key: &str, via: &str, value: &str) -> TestResult<Input> {
        let mut input = Input {
            name: key.to_owned(),
            bidders: vec!["appnexus".to_owned()],
            ..Input::default()
        };
        match via {
            "env" => {
                let _previous = input.env.insert(env_name(key), value.to_owned());
            }
            "file" => {
                let _previous = input
                    .files
                    .insert("pbs.yaml".to_owned(), nested_yaml(key, value).into_bytes());
            }
            other => return Err(format!("{key}: no channel {other}")),
        }
        Ok(input)
    }

    /// What a matrix row's load gave, as `matrix.go` records it.
    fn matrix_outcome(key: &str, got: &Outcome) -> Json {
        let (get, is_set) = got.settings.as_ref().map_or((Json::Null, false), |s| {
            (
                s.get(key).map_or(Json::Null, |v| v.compact()),
                s.is_set(key),
            )
        });
        let mut out = json!({"get": get, "is_set": is_set, "error": got.error()});
        if let (Ok(decoded), Some(map)) = (&got.decoded, out.as_object_mut()) {
            let _previous = map.insert("leaf".to_owned(), leaf(decoded, key));
        }
        out
    }

    /// Runs every row of `matrix.json` that `select` picks (by key, channel and the number of
    /// Go's outcomes), and fails with each row whose result is none of Go's.
    fn check_matrix(select: &dyn Fn(&str, &str, usize) -> bool) -> TestResult {
        let doc = golden("matrix.json")?;
        let mut failures = Vec::new();
        let mut ran = 0_usize;
        for row in list(&doc, "/rows")? {
            let key = text(row, "/key")?;
            let via = text(row, "/via")?;
            let value = text(row, "/input")?;
            let outcomes = list(row, "/outcomes")?;
            if !select(key, via, outcomes.len()) {
                continue;
            }
            ran += 1;
            let got = run(&matrix_input(key, via, value)?)?;
            if let Err(e) = &got.settings {
                failures.push(format!("{key} {via} {value:?}: the load failed: {e}"));
                continue;
            }
            let mine = matrix_outcome(key, &got);
            if !outcomes.contains(&mine) {
                let want = outcomes.first().cloned().unwrap_or_default();
                let found = diff(&mine, &want).join("; ");
                failures.push(format!("{key} {via} {value:?}: {found}"));
            }
        }
        if ran == 0 {
            failures.push("no row selected".to_owned());
        }
        report("matrix.json", &failures)
    }

    /// Every key Viper knows, set through `PBS_` and through `pbs.yaml` to a typed sample and to
    /// values its field's type cannot take as they are, gives Go's `Get`, `IsSet`, decoded value
    /// or decode error: Viper's cast by the default's type, then mapstructure's weak decode.
    // Go: viper.go:884-929 (Get's cast by the default's type, v1.12.0), mapstructure.go v1.5.0
    // (WeaklyTypedInput), config/config.go:758 (v.Unmarshal with AccountModulesHookFunc)
    #[test]
    fn weak_typing_matrix() -> TestResult {
        check_matrix(&|_, _, _| true)?;
        check_listed(DECODE_LOADS)
    }

    /// The loads of `loads.json` about mapstructure's weak decode and the account modules hook.
    const DECODE_LOADS: &[&str] = &[
        "adapters as a list",
        "adapters as an empty list",
        "account modules",
        "account modules inner not a map",
        "account modules not a map",
        "account modules as a list",
        "host modules",
        "host execution plan",
        "struct from a scalar",
        "struct fields",
        "struct from an empty map",
        "slice from a scalar",
        "slice from a map",
        "slice from an empty map",
        "map from a scalar",
        "nested slices",
        "string from a list",
        "string from a bool and a float",
        "int from a list",
        "int from a bool and a float",
        "bool without a default from a map",
        "several failures",
        "uint overflow wraps",
        "bid adjustments",
        "bidder overrides",
        "bidder uint16 overflow",
        "bidder not in the bidder infos",
        "unexported-looking fields",
        "json numbers",
        "json lists",
        "json big numbers",
        "json modules",
        "yaml numbers",
        "yaml special floats",
        "yaml non-string keys in modules",
    ];

    /// The defaults table at the workspace root.
    fn defaults_table() -> TestResult<Json> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("testdata")
            .join("go_config_defaults.json");
        let data = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&data).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Whether two values of encoding/json's model are equal, numbers by their value (Go writes
    /// the `float64` 1 as `1`).
    fn same_json(a: &Json, b: &Json) -> bool {
        match (a, b) {
            (Json::Number(x), Json::Number(y)) => x == y || x.as_f64() == y.as_f64(),
            (Json::Array(x), Json::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_json(x, y))
            }
            (Json::Object(x), Json::Object(y)) => {
                x.len() == y.len()
                    && x.iter()
                        .all(|(k, v)| y.get(k).is_some_and(|w| same_json(v, w)))
            }
            (
                Json::Null
                | Json::Bool(_)
                | Json::String(_)
                | Json::Number(_)
                | Json::Array(_)
                | Json::Object(_),
                _,
            ) => a == b,
        }
    }

    /// A value as encoding/json writes it.
    fn go_json(value: &GoValue) -> Json {
        match value {
            GoValue::Nil | GoValue::Strings(None) => Json::Null,
            GoValue::Bool(b) => Json::Bool(*b),
            GoValue::Int(i) | GoValue::Int64(i) => Json::from(*i),
            GoValue::Uint64(u) => Json::from(*u),
            GoValue::Float64(f) => Json::from(*f),
            GoValue::String(s) => Json::String(s.clone()),
            GoValue::List(items) => Json::Array(items.iter().map(go_json).collect()),
            GoValue::Map(map) => {
                Json::Object(map.iter().map(|(k, v)| (k.clone(), go_json(v))).collect())
            }
            GoValue::AnyMap(_) => Json::String(value.to_string()),
            GoValue::Strings(Some(items)) => {
                Json::Array(items.iter().cloned().map(Json::String).collect())
            }
            GoValue::StringLists(lists) => Json::Array(
                lists
                    .iter()
                    .map(|l| Json::Array(l.iter().cloned().map(Json::String).collect()))
                    .collect(),
            ),
            GoValue::EmptySlice(_) => Json::Array(Vec::new()),
        }
    }

    /// The defaults load gives Go's whole tree, decoded configuration and probes, and the loader
    /// knows every key of the defaults table with its default, its Go type and whether it is set.
    // Go: config/config.go:936-1415 (SetupViper's SetDefault and BindEnv calls)
    #[test]
    fn defaults_match_viper() -> TestResult {
        let doc = golden("loads.json")?;
        let defaults = at(&doc, "/defaults")?;
        let input = Input::from_golden(defaults)?;
        let got = run(&input)?;
        let mut failures: Vec<String> = diff(&got.settings_json(), at(defaults, "/settings")?)
            .into_iter()
            .map(|d| format!("settings {d}"))
            .collect();
        match &got.decoded {
            Ok(decoded) => failures.extend(
                diff(&decoded.compact(), at(defaults, "/decoded")?)
                    .into_iter()
                    .map(|d| format!("decoded {d}")),
            ),
            Err(e) => failures.push(format!("the defaults do not decode: {e}")),
        }
        check_eq(
            got.error(),
            text(defaults, "/error")?,
            "the defaults' error",
        )?;
        failures.extend(
            diff(&got.probes(&input.probes), at(defaults, "/probe_results")?)
                .into_iter()
                .map(|d| format!("probe {d}")),
        );

        let table = defaults_table()?;
        let bidders = string_list(table.pointer("/bidder_keys/bidders"))?;
        let settings = Loader::new()
            .without_file()
            .bidders(bidders.iter().cloned())
            .load()
            .map_err(|e| e.to_string())?;
        let mut keys = BTreeSet::new();
        for entry in list(&table, "/keys")? {
            let key = text(entry, "/key")?;
            let _new = keys.insert(key.to_owned());
            let has_default = at(entry, "/has_default")?.as_bool() == Some(true);
            if settings.is_set(key) != has_default {
                failures.push(format!("{key}: IsSet is not {has_default}"));
            }
            let value = settings.get(key);
            let encoded = value.as_ref().map_or(Json::Null, go_json);
            if !same_json(&encoded, at(entry, "/default")?) {
                failures.push(format!(
                    "{key}: Get is {encoded}, want {}",
                    at(entry, "/default")?
                ));
            }
            let go_type = value.as_ref().map_or_default(GoValue::go_type);
            if go_type != text(entry, "/go_type")? {
                failures.push(format!(
                    "{key}: the default is a {go_type}, want {}",
                    text(entry, "/go_type")?
                ));
            }
        }
        for bidder in &bidders {
            for suffix in list(&table, "/bidder_keys/suffixes")? {
                let _new = keys.insert(format!("adapters.{bidder}.{}", text(suffix, "/suffix")?));
            }
        }
        let all: BTreeSet<String> = settings.all_keys().into_iter().collect();
        for key in all.symmetric_difference(&keys) {
            failures.push(format!(
                "{key}: AllKeys {} it",
                if all.contains(key) { "has" } else { "lacks" }
            ));
        }
        report("the defaults", &failures)
    }

    /// The sample the environment checks set a key to: a value of the default's type that is
    /// not the default.
    fn env_sample(go_type: &str, default: &Json) -> &'static str {
        match go_type {
            "bool" if default == &Json::Bool(true) => "false",
            "bool" => "true",
            "int" => "12345",
            "float64" => "0.125",
            "[]string" => "a b",
            _ => "from-env",
        }
    }

    /// The loader for the defaults table's environment checks.
    fn env_load(env: &[(&str, &str)]) -> Result<Settings, LoadError> {
        Loader::new()
            .without_file()
            .bidders(["appnexus"])
            .env(
                env.iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect(),
            )
            .load()
    }

    /// Every key's `PBS_` variable, and each fallback variable a `BindEnv` call registers, sets
    /// the key when it is set alone, and an empty variable is ignored; and the environment loads
    /// of `loads.json` and the environment rows of the matrix give Go's results.
    // Go: config/config.go:1167-1168, 1192 and 1258-1259 (BindEnv before SetEnvPrefix) and
    // config/config.go:1318-1321 (SetEnvKeyReplacer, SetTypeByDefaultValue, SetEnvPrefix("PBS"),
    // AutomaticEnv); viper.go:1220-1354 (find, v1.12.0)
    #[test]
    fn env_matrix() -> TestResult {
        let table = defaults_table()?;
        let mut failures = Vec::new();
        for entry in list(&table, "/keys")? {
            let key = text(entry, "/key")?;
            let var = text(entry, "/env")?;
            if var != env_name(key) {
                failures.push(format!(
                    "{key}: the variable is {var}, not {}",
                    env_name(key)
                ));
            }
            let go_type = text(entry, "/go_type")?;
            let default = at(entry, "/default")?;
            let sample = env_sample(go_type, default);
            let mut vars = vec![var.to_owned()];
            for fallback in list(entry, "/env_fallbacks")? {
                vars.push(text(fallback, "/env")?.to_owned());
            }
            for name in &vars {
                let settings = env_load(&[(name, sample)]).map_err(|e| e.to_string())?;
                let got = settings.get(key).as_ref().map_or(Json::Null, go_json);
                if same_json(&got, default) {
                    failures.push(format!("{key}: {name}={sample} did not set it"));
                }
                let empty = env_load(&[(name, "")]).map_err(|e| e.to_string())?;
                let unset = empty.get(key).as_ref().map_or(Json::Null, go_json);
                if !same_json(&unset, default) {
                    failures.push(format!("{key}: the empty {name} set it to {unset}"));
                }
            }
        }
        for suffix in list(&table, "/bidder_keys/suffixes")? {
            let key = format!("adapters.appnexus.{}", text(suffix, "/suffix")?);
            let var = format!("PBS_ADAPTERS_APPNEXUS_{}", text(suffix, "/env_suffix")?);
            let settings = env_load(&[(&var, "from-env")]).map_err(|e| e.to_string())?;
            let got = settings.get(&key);
            if got != Some(GoValue::String("from-env".to_owned())) {
                failures.push(format!("{key}: {var} gave {got:?}"));
            }
        }
        report("the environment", &failures)?;
        check_listed(ENV_LOADS)?;
        check_matrix(&|_, via, _| via == "env")
    }

    /// The loads of `loads.json` about the environment.
    const ENV_LOADS: &[&str] = &[
        "env over file",
        "empty env is ignored",
        "env over default",
        "fallback variable",
        "prefixed variable over the fallback",
        "empty prefixed variable before the fallback",
        "fallback over file",
        "every fallback",
        "fallback spelled with dots is not read",
        "lower-case variable is not read",
        "mixed-case variable is not read",
        "bidder variables",
        "bidder variable without the bidder",
        "bidder variable over the file",
        "env for a key the file names",
        "env for a key nothing names",
        "env names a section",
        "env names a section and a key in it",
        "env names an inner section",
        "empty env for a section is ignored",
        "string slice from env",
        "nested slice from env",
        "struct slice from env",
        "struct from env",
        "bidder name slice from env",
        "env over a file section",
        "numbers from env",
        "bad numbers from env become zero",
        "null section with env",
    ];

    /// The golden load `name` of `loads.json`.
    fn golden_load<'a>(doc: &'a Json, name: &str) -> TestResult<&'a Json> {
        for load in list(doc, "/loads")? {
            if text(load, "/name")? == name {
                return Ok(load);
            }
        }
        Err(format!("loads.json has no load {name:?}"))
    }

    /// One load of `loads.json` by name: the golden load and what the loader gives for it.
    fn load_named(doc: &Json, name: &str) -> TestResult<(Json, Outcome)> {
        let load = golden_load(doc, name)?;
        let got = run(&Input::from_golden(load)?)?;
        Ok((load.clone(), got))
    }

    /// The settings of a load that loaded.
    fn loaded<'a>(name: &str, got: &'a Outcome) -> TestResult<&'a Settings> {
        got.settings
            .as_ref()
            .map_err(|e| format!("{name}: the load failed: {e}"))
    }

    /// Viper reads `pbs.properties`, `.props`, `.prop`, `.hcl`, `.tfvars`, `.dotenv`, `.env` and
    /// `.ini` too, whichever comes first in its search; the loader reads JSON, TOML and YAML and,
    /// when the file Viper would read is in another format, fails naming the file (D-VIPERFMT).
    // Go: viper.go:419 (SupportedExts), viper_go1_15.go:16-45 (findConfigFile and searchInPath,
    // v1.12.0), config/config.go:936-940 and 1322 (SetupViper's search paths and ReadInConfig)
    #[test]
    fn unsupported_format_fails_loudly() -> TestResult {
        let doc = golden("loads.json")?;
        check_eq(
            &string_list(doc.get("supported_exts"))?,
            &SUPPORTED_EXTS
                .iter()
                .map(|ext| (*ext).to_owned())
                .collect::<Vec<_>>(),
            "Viper's extensions, in its search order",
        )?;
        let mut rows = Vec::new();
        for load in list(&doc, "/loads")? {
            let go_file = text(load, "/file_used")?;
            let Some((_, go_ext)) = go_file.rsplit_once('.') else {
                continue;
            };
            if ["json", "toml", "yaml", "yml"].contains(&go_ext) {
                continue;
            }
            let input = Input::from_golden(load)?;
            let got = run(&input)?;
            let go = if text(load, "/read_error")?.is_empty() {
                format!("reads {go_file}")
            } else {
                format!("fails to read {go_file}")
            };
            let rust =
                if let Err(error @ LoadError::UnsupportedFormat { path, ext }) = &got.settings {
                    let named = error.to_string().contains(&path.display().to_string());
                    let file = path
                        .file_name()
                        .map_or_default(|name| name.to_string_lossy().into_owned());
                    format!("fails on {file} ({ext}), naming it: {named}")
                } else {
                    "loads".to_owned()
                };
            rows.push([input.name, go, rust]);
        }
        let want = [
            [
                "an unread format in the working directory first",
                "reads first:pbs.ini",
                "fails on pbs.ini (ini), naming it: true",
            ],
            [
                "format properties",
                "reads first:pbs.properties",
                "fails on pbs.properties (properties), naming it: true",
            ],
            [
                "format props",
                "reads first:pbs.props",
                "fails on pbs.props (props), naming it: true",
            ],
            [
                "format prop",
                "reads first:pbs.prop",
                "fails on pbs.prop (prop), naming it: true",
            ],
            [
                "format hcl",
                "reads first:pbs.hcl",
                "fails on pbs.hcl (hcl), naming it: true",
            ],
            [
                "format tfvars",
                "reads first:pbs.tfvars",
                "fails on pbs.tfvars (tfvars), naming it: true",
            ],
            [
                "format dotenv",
                "reads first:pbs.dotenv",
                "fails on pbs.dotenv (dotenv), naming it: true",
            ],
            [
                "format env",
                "reads first:pbs.env",
                "fails on pbs.env (env), naming it: true",
            ],
            [
                "format ini",
                "reads first:pbs.ini",
                "fails on pbs.ini (ini), naming it: true",
            ],
        ];
        check_eq(
            &rows,
            &want.map(|row| row.map(str::to_owned)).to_vec(),
            "the loads of a format Viper reads and the loader does not: name, Go, Rust",
        )?;
        // A file in a format the loader reads, first in the search, is read as Viper reads it.
        check_listed(&["yml before properties"])
    }

    /// The loads of `loads.json` about the file Viper reads: the search over `.` and the second
    /// path in Viper's extension order, a directory skipped, and a file that cannot be read
    /// ignored.
    const FILE_LOADS: &[&str] = &[
        "no file in the search paths",
        "json before yaml",
        "toml before yaml",
        "json before toml",
        "yaml before yml",
        "yml alone",
        "yml before properties",
        "a directory named pbs.json",
        "the working directory first",
        "the second directory",
        "a file in both directories",
        "invalid yaml is ignored",
        "invalid json is ignored",
        "invalid toml is ignored",
        "duplicate yaml keys are ignored",
        "duplicate json keys",
        "duplicate toml keys are ignored",
        "a yaml list document is ignored",
        "a json list document is ignored",
        "an empty yaml file",
        "an empty json file is ignored",
        "an empty toml file",
        "a yaml null document",
        "a json null document",
        "a yaml comment only",
        "a yaml tab indent is ignored",
        "invalid utf-8 in a json string",
        "invalid utf-8 in yaml is ignored",
        "yaml several documents",
        "yaml document markers",
        "sample/001_banner/app.yaml",
        "modules/fiftyonedegrees/devicedetection/sample/pbs.json",
    ];

    /// The file the loader reads is the one Viper reads: the first `pbs.<ext>` in `.` and then
    /// the second search path, in Viper's extension order, skipping a directory; a file that does
    /// not parse is ignored, as `config.SetupViper` ignores `ReadInConfig`'s error, with Viper's
    /// not-found text when there is none.
    // Go: viper_go1_15.go:16-45 (findConfigFile and searchInPath, v1.12.0), viper.go:1520-1546
    // (ReadInConfig), config/config.go:936-940 and 1322
    #[test]
    fn file_search_like_viper() -> TestResult {
        check_listed(FILE_LOADS)
    }

    /// The loads of `loads.json` about booleans: `cast.ToBool` for keys with a bool default, from
    /// `PBS_` and from `pbs.yaml` after yaml.v3's untyped resolution, and mapstructure's weak
    /// `ParseBool` for keys bound only with `BindEnv` and in the `UnmarshalKey` calls.
    const BOOL_LOADS: &[&str] = &[
        "bools from env",
        "bool without a default from env fails",
        "bidder bool from env",
        "bidder bool from env fails",
        "bidder bool spellings",
        "bidder bool fails",
        "yaml 1.1 bools stay strings",
        "toml integer into a bool",
        "bool without a default from a map",
        "bool default from a map",
    ];

    /// Booleans convert as orchestrator ruling 8 states: keys with a bool default through
    /// `cast.ToBool` (`strconv.ParseBool` with its error dropped: `t` and `1` are true, `yes`,
    /// `on` and anything else false), from `PBS_` and from `pbs.yaml` (where yaml.v3 leaves `yes`,
    /// `on` and `y` strings); keys bound only with `BindEnv` and the `UnmarshalKey` calls through
    /// mapstructure's `ParseBool`, where the empty string is false and anything else fails the
    /// load. Every bool key of the matrix, through both channels, gives Go's result.
    // Go: config/config.go:1319 (SetTypeByDefaultValue), viper.go:884-929 (Get's cast),
    // mapstructure.go v1.5.0 (decodeBool), config/config.go:886-889 (UnmarshalKey)
    #[test]
    fn bools_convert_like_viper() -> TestResult {
        check_listed(BOOL_LOADS)?;
        let doc = golden("matrix.json")?;
        let mut env_inputs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for row in list(&doc, "/rows")? {
            if text(row, "/via")? == "env" {
                env_inputs
                    .entry(text(row, "/key")?.to_owned())
                    .or_default()
                    .push(text(row, "/input")?.to_owned());
            }
        }
        // matrix.go's inputsFor gives a bool field these values.
        let bool_inputs = ["true", "yes", "0", "T"];
        let bool_keys: BTreeSet<&String> = env_inputs
            .iter()
            .filter(|(_, inputs)| inputs.iter().eq(bool_inputs.iter()))
            .map(|(key, _)| key)
            .collect();
        check_eq(
            &bool_keys
                .iter()
                .any(|key| key.as_str() == "adapters.appnexus.disabled"),
            &true,
            "the matrix's bool keys include a BindEnv-only one",
        )?;
        check_matrix(&|key, _, _| bool_keys.iter().any(|k| k.as_str() == key))
    }

    /// The loads of `loads.json` about key case.
    const KEY_LOADS: &[&str] = &[
        "key case Port",
        "key case PORT",
        "key case nested",
        "key case inside a list stays",
        "key case inside modules",
        "key case of a bidder",
        "json keys",
        "toml keys",
        "yaml quoted keys",
        "lower-case variable is not read",
        "mixed-case variable is not read",
        "bidder variables",
    ];

    /// Viper lower-cases every key of the file (inside lists it keeps them as written), and reads
    /// only the upper-case `PBS_` variable of a key.
    // Go: util.go:67-87 (insensitiviseMap, viper v1.12.0), viper.go:1279 (getEnv of the key
    // mergeWithEnvPrefix upper-cases, :521-527)
    #[test]
    fn keys_lower_cased() -> TestResult {
        check_listed(KEY_LOADS)
    }

    /// `config.New` wraps mapstructure's error as `viper failed to unmarshal app config: ...`, and
    /// the loader's decode error reads the same, mapstructure's text included. The error of a file
    /// that does not parse, which `config.SetupViper` ignores, is the JSON, YAML or TOML reader's
    /// own wording after Viper's `While parsing config: `.
    // Go: config/config.go:758-760 (config.New), viper.go:1694 (ConfigParseError)
    #[test]
    fn error_wrapper_exact() -> TestResult {
        let doc = golden("loads.json")?;
        let mut failures = Vec::new();
        let mut wrapped = 0_usize;
        for load in list(&doc, "/loads")? {
            let name = text(load, "/name")?;
            let go_error = text(load, "/error")?;
            if go_error.is_empty() || ledger_entry(name).is_some() {
                continue;
            }
            wrapped += 1;
            let got = run(&Input::from_golden(load)?)?;
            let want = format!("viper failed to unmarshal app config: {go_error}");
            match loaded(name, &got)?.decode() {
                Err(e) if e.to_string() == want => {}
                Err(e) => failures.push(format!("{name}: {e}, want {want}")),
                Ok(_) => failures.push(format!("{name}: decodes, want {want}")),
            }
        }
        check_eq(&(wrapped > 10), &true, "loads with a decode error")?;
        report("the decode errors", &failures)?;
        let mut rows = Vec::new();
        for name in [
            "invalid yaml is ignored",
            "invalid toml is ignored",
            "invalid json is ignored",
        ] {
            let (load, got) = load_named(&doc, name)?;
            let go = text(&load, "/read_error")?;
            let rust = got.read_error.as_str();
            rows.push((name, rust.starts_with("While parsing config: "), rust == go));
        }
        check_eq(
            &rows,
            &vec![
                ("invalid yaml is ignored", true, false),
                ("invalid toml is ignored", true, false),
                ("invalid json is ignored", true, false),
            ],
            "read errors: Viper's prefix, and the text Go's",
        )
    }

    /// The loads of `loads.json` read from `pbs.toml`.
    const TOML_LOADS: &[&str] = &[
        "toml before yaml",
        "invalid toml is ignored",
        "duplicate toml keys are ignored",
        "an empty toml file",
        "toml values",
        "toml keys",
        "toml integer into a bool",
    ];

    /// go-toml v2.0.1 under Viper's TOML codec and the `toml` crate under the `config` crate give
    /// the same trees for `pbs.toml`, integers as Go's `int64`, except for date-times: go-toml
    /// gives an offset date-time as a `time.Time` and a local date or time as its own types,
    /// which mapstructure cannot decode into a string field, and the `config` crate gives the
    /// text, which it can. A file that does not parse is ignored on both sides, worded each its
    /// own way.
    // Go: viper internal/encoding/toml/codec.go (go-toml v2.0.1's Unmarshal), config/config.go:758
    #[test]
    fn toml_reads_like_go_toml() -> TestResult {
        check_listed(TOML_LOADS)?;
        let doc = golden("loads.json")?;
        let (load, got) = load_named(&doc, "toml date-time")?;
        check_eq(
            at(&load, "/settings/set/0/value")?,
            &json!({"$time": "1979-05-27T07:32:00Z"}),
            "go-toml's offset date-time",
        )?;
        check_eq(
            text(&load, "/error")?,
            "1 error(s) decoding:\n\n* 'gdpr.default_value' expected type 'string', got \
             unconvertible type 'time.Time', value: '1979-05-27 07:32:00 +0000 UTC'",
            "Go's decode error",
        )?;
        let settings = loaded("toml date-time", &got)?;
        check_eq(
            &settings.get("gdpr.default_value"),
            &Some(GoValue::String("1979-05-27T07:32:00Z".to_owned())),
            "the config crate's offset date-time",
        )?;
        check_eq(got.error(), "", "the loader's decode error")?;
        let (local, local_got) = load_named(&doc, "toml local date in modules")?;
        check_eq(
            at(&local, "/settings/set/0/value")?,
            &json!({"$toml": "1979-05-27"}),
            "go-toml's local date",
        )?;
        check_eq(
            &loaded("toml local date in modules", &local_got)?
                .get("hooks.modules.vendor.module.day"),
            &Some(GoValue::String("1979-05-27".to_owned())),
            "the config crate's local date",
        )
    }

    /// Where Viper's result depends on the order Go iterates a map in (`AllSettings` writing a
    /// key with a default and a key under it from the file, `null` file sections with
    /// environment variables under them), Go's result varies from run to run; the loader gives
    /// one of Go's outcomes, the same every time.
    // Go: viper.go:1940-1957 and 2021-2038 (AllKeys and AllSettings, v1.12.0)
    #[test]
    fn map_order_is_sorted() -> TestResult {
        let doc = golden("loads.json")?;
        let mut names = Vec::new();
        for load in list(&doc, "/unordered")? {
            let input = Input::from_golden(load)?;
            let first = run(&input)?;
            let second = run(&input)?;
            check_eq(
                &(first.settings_json(), first.error().to_owned()),
                &(second.settings_json(), second.error().to_owned()),
                &format!("{}: two loads", input.name),
            )?;
            names.push(input.name);
        }
        check_eq(&names.len(), &4, "the loads whose result is Go's map order")?;
        check_loads(&|name| names.iter().any(|n| n == name))?;
        check_matrix(&|_, _, outcomes| outcomes > 1)
    }

    /// What the `config` crate gives for `key` read as `T`: the value as JSON, `unset` when no
    /// source sets it, or `error` when it does not convert.
    fn config_get<T: DeserializeOwned + Serialize>(config: &Config, key: &str) -> String {
        match config.get::<T>(key) {
            Ok(value) => serde_json::to_string(&value).unwrap_or_default(),
            Err(config::ConfigError::NotFound(_)) => "unset".to_owned(),
            Err(_) => "error".to_owned(),
        }
    }

    /// The `config` crate converts a value by the type serde asks for: a string into a number in
    /// decimal only, a float into an integer rounded, a scalar never into a list. Viper casts by
    /// the type of the key's default with spf13/cast, which drops its error, and mapstructure
    /// decodes weakly: `0x10` is 16, `4x2` is 0, 4.5 is 4 and a scalar is a one-element list.
    // Go: viper.go:884-929 (Get, v1.12.0), cast v1.5.0 caste.go (ToIntE, ToStringSliceE),
    // config/config.go:758 and 1319
    #[test]
    fn go_difference_weak_typing() -> TestResult {
        let doc = golden("matrix.json")?;
        let mut rows = Vec::new();
        for (key, via, input) in [
            ("port", "env", "0x10"),
            ("port", "env", "4x2"),
            ("port", "file", "4.5"),
            ("port", "file", "\"4x2\""),
            ("gdpr.eea_countries", "file", "a"),
            ("gdpr.eea_countries", "file", "[1, true]"),
        ] {
            let mut go = None;
            for row in list(&doc, "/rows")? {
                if text(row, "/key")? == key
                    && text(row, "/via")? == via
                    && text(row, "/input")? == input
                {
                    go = Some(at(row, "/outcomes/0/leaf/value")?.to_string());
                }
            }
            let go = go.ok_or_else(|| format!("matrix.json has no row {key} {via} {input}"))?;
            let builder = Config::builder();
            let config = if via == "env" {
                let env = Map::from([(env_name(key), input.to_owned())]);
                builder.add_source(Environment::with_prefix("PBS").source(Some(env)))
            } else {
                builder.add_source(File::from_str(&nested_yaml(key, input), YamlFormat))
            }
            .build()
            .map_err(|e| e.to_string())?;
            let rust = if key == "port" {
                config_get::<i64>(&config, key)
            } else {
                config_get::<Vec<String>>(&config, key)
            };
            rows.push([format!("{key} {via} {input}"), go, rust]);
        }
        let want = [
            ["port env 0x10", "16", "error"],
            ["port env 4x2", "0", "error"],
            ["port file 4.5", "4", "5"],
            ["port file \"4x2\"", "0", "error"],
            ["gdpr.eea_countries file a", r#"["a"]"#, "error"],
            [
                "gdpr.eea_countries file [1, true]",
                r#"["1","true"]"#,
                r#"["1","true"]"#,
            ],
        ];
        check_eq(
            &rows,
            &want.map(|row| row.map(str::to_owned)).to_vec(),
            "key, channel and value; Go's decoded value; config's",
        )
    }

    /// The `config` crate's `Environment` source, with Prebid's prefix and `_` as the separator,
    /// matches names in any case, makes every `_` a level (`PBS_GDPR_DEFAULT_VALUE` sets
    /// `gdpr.default.value`), reads no name without the prefix and keeps an empty value. Viper
    /// reads `PBS_` and the upper-case key with `.` replaced by `_` by its exact name, and the
    /// fallbacks `BindEnv` registers, and an empty variable is unset.
    // Go: config/config.go:1192 (BindEnv("gdpr.default_value") before SetEnvPrefix binds
    // GDPR_DEFAULT_VALUE) and :1318-1321 (SetEnvKeyReplacer, SetEnvPrefix("PBS"), AutomaticEnv);
    // viper.go:1220-1354 (find, v1.12.0)
    #[test]
    fn go_difference_env_names() -> TestResult {
        let doc = golden("loads.json")?;
        let mut rows = Vec::new();
        for (name, key) in [
            ("prefixed variable over the fallback", "gdpr.default_value"),
            ("fallback variable", "gdpr.default_value"),
            ("lower-case variable is not read", "port"),
            ("empty env is ignored", "port"),
        ] {
            let load = golden_load(&doc, name)?;
            let env = string_map(load.get("env"))?;
            let vars: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
            let go = at(load, &format!("/probe_results/{key}/get"))?;
            let collected = Environment::with_prefix("PBS")
                .separator("_")
                .source(Some(env.into_iter().collect()))
                .collect()
                .map_err(|e| e.to_string())?;
            let mut rust: Vec<String> = collected.iter().map(|(k, v)| format!("{k}={v}")).collect();
            rust.sort();
            rows.push([vars.join(" "), format!("{key}: {go}"), rust.join(" ")]);
        }
        let want = [
            [
                "GDPR_DEFAULT_VALUE=1 PBS_GDPR_DEFAULT_VALUE=0",
                "gdpr.default_value: \"0\"",
                "gdpr.default.value=0",
            ],
            ["GDPR_DEFAULT_VALUE=1", "gdpr.default_value: \"1\"", ""],
            ["pbs_port=2005", "port: 8000", "port=2005"],
            ["PBS_PORT=", "port: 2003", "port="],
        ];
        check_eq(
            &rows,
            &want.map(|row| row.map(str::to_owned)).to_vec(),
            "the variables; Viper's Get; the keys config collects",
        )
    }

    /// The account modules hook asserts that its data is a `map[string]interface{}`, which panics
    /// for a list element whose keys are not all strings and crashes Go at startup; the loader
    /// fails the load with a decode error that names the key and the same conversion.
    // Go: config/account_module_decoder.go:16-23 (AccountModulesHookFunc), mapstructure.go v1.5.0
    // (decodeMapFromSlice, DecodeHookExec), config/config.go:758 (config.New)
    #[test]
    fn go_difference_hook_panic() -> TestResult {
        let doc = golden("loads.json")?;
        let name = "account modules as a list with a key that is not a string";
        let (load, got) = load_named(&doc, name)?;
        check_eq(
            text(&load, "/error")?,
            "panic: interface conversion: interface {} is map[interface {}]interface {}, not \
             map[string]interface {}",
            "Go's panic",
        )?;
        let rust = loaded(name, &got)?.decode().err().map(|e| e.to_string());
        check_eq(
            &rust,
            &Some(
                "viper failed to unmarshal app config: 1 error(s) decoding:\n\n\
                 * error decoding 'account_defaults.hooks.modules[0]': interface conversion: \
                 interface {} is map[interface {}]interface {}, not map[string]interface {}"
                    .to_owned(),
            ),
            "the loader's error",
        )
    }

    /// A decode error lists the failures in mapstructure's words with `strconv.NumError`'s, but
    /// prints the values in them as the loader prints them: a float as Rust spells it where Go's
    /// `%v` writes `%g` (`1000000` for `1e+06`), and a string quoted as Rust quotes it where
    /// `strconv.Quote` escapes differently (`\u{a0}` for `\u00a0`).
    // Go: mapstructure.go v1.5.0 (Error.Error, decode, decodeInt), fmt's %v, strconv.Quote,
    // config/config.go:758-760 (config.New)
    #[test]
    fn go_difference_error_texts() -> TestResult {
        let doc = golden("loads.json")?;
        let name = "failing values are printed";
        let (load, got) = load_named(&doc, name)?;
        check_eq(
            text(&load, "/error")?,
            "2 error(s) decoding:\n\n\
             * 'account_defaults.cookie_sync.default_limit' expected type 'int', got unconvertible \
             type '[]interface {}', value: '[1e+06 1e-05]'\n\
             * cannot parse 'account_defaults.cookie_sync.max_limit' as int: strconv.ParseInt: \
             parsing \"\\u00a0x\": invalid syntax",
            "mapstructure's error",
        )?;
        let rust = loaded(name, &got)?.decode().err().map(|e| e.to_string());
        check_eq(
            &rust,
            &Some(
                "viper failed to unmarshal app config: 2 error(s) decoding:\n\n\
                 * 'account_defaults.cookie_sync.default_limit' expected type 'int', got \
                 unconvertible type '[]interface {}', value: '[1000000 0.00001]'\n\
                 * cannot parse 'account_defaults.cookie_sync.max_limit' as int: \
                 strconv.ParseInt: parsing \"\\u{a0}x\": invalid syntax"
                    .to_owned(),
            ),
            "the loader's error",
        )
    }

    /// The `config` crate reads every file source it is given and merges them, tries its own
    /// formats in its order at each (`toml` before `json`), and fails the build on a file that
    /// does not parse. Viper reads the first `pbs.<ext>` of `.` and then of `/etc/config` in its
    /// order (`json` before `toml`), only that one, and ignores a file that does not parse.
    // Go: viper.go:419 (SupportedExts), viper_go1_15.go:16-45 (findConfigFile and searchInPath,
    // v1.12.0), config/config.go:936-940 and 1322
    #[test]
    fn go_difference_file_search() -> TestResult {
        let doc = golden("loads.json")?;
        let mut rows = Vec::new();
        for name in [
            "json before toml",
            "a file in both directories",
            "invalid json is ignored",
        ] {
            let load = golden_load(&doc, name)?;
            let input = Input::from_golden(load)?;
            let wd = TempDir::new(&input.files)?;
            let second = input.second.as_ref().map(TempDir::new).transpose()?;
            let mut builder = Config::builder();
            // A later source wins, so the working directory comes last.
            for dir in second.iter().chain([&wd]) {
                let base = dir.0.join("pbs").display().to_string();
                builder = builder.add_source(File::with_name(&base).required(false));
            }
            let rust = builder.build().map_or_else(
                |_| "fails".to_owned(),
                |config| {
                    format!(
                        "port {}, host {}",
                        config_get::<i64>(&config, "port"),
                        config_get::<String>(&config, "host")
                    )
                },
            );
            let go = format!(
                "{}: port {}, host {}",
                text(load, "/file_used")?,
                at(load, "/probe_results/port/get")?,
                at(load, "/probe_results/host/get")?
            );
            rows.push([name.to_owned(), go, rust]);
        }
        let want = [
            [
                "json before toml",
                "first:pbs.json: port 1031, host \"\"",
                "port 1032, host unset",
            ],
            [
                "a file in both directories",
                "first:pbs.json: port 1033, host \"\"",
                "port 1033, host \"h\"",
            ],
            [
                "invalid json is ignored",
                "first:pbs.json: port 8000, host \"\"",
                "fails",
            ],
        ];
        check_eq(
            &rows,
            &want.map(|row| row.map(str::to_owned)).to_vec(),
            "the load; the file Viper read and its values; config's values",
        )
    }

    /// encoding/json under Viper's JSON codec reads a `pbs.json` holding invalid UTF-8 or a lone
    /// surrogate escape (each a U+FFFD) or 200 levels of nesting; `serde_json` refuses each, so the
    /// loader ignores the file, as Viper ignores a file that does not parse, and the defaults
    /// stay. A file neither reads is ignored on both sides, each reader wording its error its own
    /// way.
    // Go: viper internal/encoding/json/codec.go:15 (Decode, json.Unmarshal into
    // map[string]interface{}, v1.12.0), config/config.go:1322 (ReadInConfig's error ignored)
    #[test]
    fn go_difference_json_reader() -> TestResult {
        let doc = golden("loads.json")?;
        let keys = ["port".to_owned(), "host".to_owned()];
        let mut rows = Vec::new();
        for name in [
            "invalid utf-8 in a json string",
            "a lone surrogate in a json string",
            "json nested 200 levels",
            "invalid json is ignored",
        ] {
            let (load, got) = load_named(&doc, name)?;
            let go = format!(
                "{:?}: port {}, host {}",
                text(&load, "/read_error")?,
                at(&load, "/probe_results/port/get")?,
                at(&load, "/probe_results/host/get")?
            );
            let probes = got.probes(&keys);
            let rust = format!(
                "{:?}: port {}, host {}",
                got.read_error,
                at(&probes, "/port/get")?,
                at(&probes, "/host/get")?
            );
            rows.push([name.to_owned(), go, rust]);
        }
        let want = [
            [
                "invalid utf-8 in a json string",
                "\"\": port 8000, host \"a\u{fffd}b\"",
                "\"While parsing config: invalid unicode code point at line 1 column 12\": port \
                 8000, host \"\"",
            ],
            [
                "a lone surrogate in a json string",
                "\"\": port 1041, host \"a\u{fffd}b\"",
                "\"While parsing config: unexpected end of hex escape at line 1 column 32\": port \
                 8000, host \"\"",
            ],
            [
                "json nested 200 levels",
                "\"\": port 1042, host \"\"",
                "\"While parsing config: recursion limit exceeded at line 1 column 780\": port \
                 8000, host \"\"",
            ],
            [
                "invalid json is ignored",
                "\"While parsing config: unexpected end of JSON input\": port 8000, host \"\"",
                "\"While parsing config: EOF while parsing an object at line 1 column 13\": port \
                 8000, host \"\"",
            ],
        ];
        check_eq(
            &rows,
            &want.map(|row| row.map(str::to_owned)).to_vec(),
            "the load; Viper's read error and values; the loader's",
        )
    }

    /// `AccountModulesHookFunc` marshals each module's configuration with `jsonutil.Marshal`,
    /// which writes `<`, `>` and `&` as `\u` escapes and floats as strconv spells them; the loader
    /// writes the bytes with `serde_json`, a whole float that fits an integer as the integer Go
    /// writes and every other float in `serde_json`'s spelling.
    // Go: config/account_module_decoder.go:16-61 (AccountModulesHookFunc and
    // convertToRawMessageMap)
    #[test]
    fn go_difference_module_json() -> TestResult {
        let doc = golden("loads.json")?;
        let (load, got) = load_named(&doc, "module json spelling")?;
        check_eq(
            at(&load, "/decoded/set/0/path")?,
            &json!(["account_defaults", "hooks", "modules", "vendor", "module"]),
            "the module's path in the golden",
        )?;
        let decoded = got
            .decoded
            .as_ref()
            .map_err(|e| format!("the load failed to decode: {e}"))?;
        let rust = leaf(decoded, "account_defaults.hooks.modules.vendor.module");
        check_eq(
            &[
                text(&load, "/decoded/set/0/value/$bytes")?,
                text(&rust, "/value/$bytes")?,
            ],
            &[
                concat!(
                    r#"{"big":1e+21,"html":"\u003ca\u0026b\u003e","huge":12345678901234567000,"#,
                    r#""negzero":-0,"small":1e-07,"whole":3,"wide":100000000000000000000}"#,
                ),
                concat!(
                    r#"{"big":1e+21,"html":"<a&b>","huge":12345678901234567000,"negzero":0,"#,
                    r#""small":1e-7,"whole":3,"wide":1e+20}"#,
                ),
            ],
            "the module's bytes: Go's, the loader's",
        )
    }

    /// A default of the type the layering pin reads its key as.
    #[derive(Debug, Clone, Copy)]
    enum Sample {
        /// An `int` default.
        Int(i64),
        /// A `bool` default.
        Bool(bool),
        /// A `string` default.
        Text(&'static str),
    }

    /// The `config` crate merges its sources into one tree, a later source's value replacing an
    /// earlier one at its path, so a `null` file value replaces the default and fails the read,
    /// and it takes a dotted key below the top level as a key of its own, so the default stays.
    /// Viper resolves each key over the layers: a `null` file value leaves the default, and a
    /// dotted key inside a section sets the key under it.
    // Go: viper.go:1220-1354 (find), :690-713 (searchIndexableWithPathPrefixes, v1.12.0)
    #[test]
    fn go_difference_layering() -> TestResult {
        let doc = golden("loads.json")?;
        let mut rows = Vec::new();
        for (name, key, default) in [
            ("null value", "port", Sample::Int(8000)),
            ("null value", "host", Sample::Text("")),
            ("dotted nested key", "gdpr.tcf2.enabled", Sample::Bool(true)),
            (
                "dotted nested key",
                "gdpr.timeouts_ms.init_vendorlist_fetches",
                Sample::Int(0),
            ),
        ] {
            let load = golden_load(&doc, name)?;
            let go = at(load, &format!("/probe_results/{key}/get"))?.to_string();
            let builder = Config::builder();
            let builder = match default {
                Sample::Int(value) => builder.set_default(key, value),
                Sample::Bool(value) => builder.set_default(key, value),
                Sample::Text(value) => builder.set_default(key, value),
            }
            .map_err(|e| e.to_string())?;
            let config = builder
                .add_source(File::from_str(text(load, "/files/pbs.yaml")?, YamlFormat))
                .build()
                .map_err(|e| e.to_string())?;
            let rust = match default {
                Sample::Int(_) => config_get::<i64>(&config, key),
                Sample::Bool(_) => config_get::<bool>(&config, key),
                Sample::Text(_) => config_get::<String>(&config, key),
            };
            rows.push([format!("{name}: {key}"), go, rust]);
        }
        check_eq(
            &rows,
            &[
                ["null value: port", "8000", "error"],
                ["null value: host", "\"\"", "error"],
                ["dotted nested key: gdpr.tcf2.enabled", "false", "true"],
                [
                    "dotted nested key: gdpr.timeouts_ms.init_vendorlist_fetches",
                    "9",
                    "0",
                ],
            ]
            .map(|row| row.map(str::to_owned))
            .to_vec(),
            "the load and key; Viper's Get; config's value",
        )
    }
}
