//! Differential replay of the goldens that `testdata/go` writes with Go 1.26.2, viper v1.12.0,
//! cast v1.5.0 and mapstructure v1.5.0: every result must match Go's.
//!
//! Each scenario runs its operations on a new [`Viper`] in a new temporary directory holding the
//! scenario's files, with the scenario's variables as the whole environment
//! ([`Environment::from_vars`]). Values are compared through Go's `dump` rendering
//! (`testdata/go/main.go`), which this file reproduces from the Rust values; the structs below
//! mirror the generator's Go types.
//!
//! Regenerate the goldens from the repository root with
//! `GOARCH=amd64 go run ./rust/crates/goviper/testdata/go`.

#[cfg(test)]
mod golden {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::{
        cmp::Ordering,
        collections::BTreeMap,
        env,
        fmt::Display,
        fs, io,
        num::TryFromIntError,
        path::PathBuf,
        process,
        sync::atomic::{self, AtomicUsize},
    };

    use pbs_gocompat::{
        math, strconv,
        strings::Replacer,
        time::{self, Duration, Time},
    };
    use pbs_goviper::{Environment, Viper, ViperError};
    use pbs_goyaml::{GoMapstructure, Type, Value, mapstructure::MsDecode};
    use serde_json::Value as Json;

    // ---- Go's dump ----------------------------------------------------------------------------

    /// Go's `dump` of a value of the Go type the Rust type stands for.
    trait Dump {
        /// Whether `Option<Self>` stands for a nil-able `Self` (a slice or a map), not `*Self`.
        const NILABLE: bool = false;

        /// Appends the rendering.
        fn dump(&self, out: &mut String);

        /// Go: `reflect.Value.IsZero`.
        fn is_zero(&self) -> bool;
    }

    fn dump<T: Dump + ?Sized>(v: &T) -> String {
        let mut out = String::new();
        v.dump(&mut out);
        out
    }

    macro_rules! dump_integers {
        ($($ty:ty),*) => {$(
            impl Dump for $ty {
                fn dump(&self, out: &mut String) {
                    out.push_str(&self.to_string());
                }

                fn is_zero(&self) -> bool {
                    *self == 0
                }
            }
        )*};
    }

    dump_integers!(i8, i16, i32, i64, isize, u8, u16, u32, u64);

    impl Dump for f64 {
        fn dump(&self, out: &mut String) {
            out.push_str(&strconv::format_float(*self, b'g', -1, 64));
        }

        fn is_zero(&self) -> bool {
            // Go: `v.Float() == 0` (-0 is zero, NaN is not).
            self.partial_cmp(&0.0) == Some(Ordering::Equal)
        }
    }

    impl Dump for f32 {
        fn dump(&self, out: &mut String) {
            out.push_str(&strconv::format_float(f64::from(*self), b'g', -1, 32));
        }

        fn is_zero(&self) -> bool {
            self.partial_cmp(&0.0) == Some(Ordering::Equal)
        }
    }

    impl Dump for bool {
        fn dump(&self, out: &mut String) {
            out.push_str(if *self { "true" } else { "false" });
        }

        fn is_zero(&self) -> bool {
            !*self
        }
    }

    impl Dump for String {
        fn dump(&self, out: &mut String) {
            out.push_str(&strconv::quote(self));
        }

        fn is_zero(&self) -> bool {
            self.is_empty()
        }
    }

    impl Dump for Duration {
        fn dump(&self, out: &mut String) {
            self.0.dump(out);
        }

        fn is_zero(&self) -> bool {
            self.0 == 0
        }
    }

    impl Dump for Time {
        fn dump(&self, out: &mut String) {
            out.push_str("time(");
            out.push_str(&self.to_string());
            out.push(')');
        }

        fn is_zero(&self) -> bool {
            *self == Self::default()
        }
    }

    impl Dump for Value {
        fn dump(&self, out: &mut String) {
            dump_value(Type::Interface, self, out);
        }

        fn is_zero(&self) -> bool {
            self.is_nil()
        }
    }

    impl<T: Dump> Dump for Option<T> {
        fn dump(&self, out: &mut String) {
            match self {
                None => out.push_str("nil"),
                Some(v) => {
                    if !T::NILABLE {
                        out.push('&');
                    }
                    v.dump(out);
                }
            }
        }

        fn is_zero(&self) -> bool {
            self.is_none()
        }
    }

    impl<T: Dump> Dump for Vec<T> {
        const NILABLE: bool = true;

        fn dump(&self, out: &mut String) {
            out.push('[');
            for (i, item) in self.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                item.dump(out);
            }
            out.push(']');
        }

        fn is_zero(&self) -> bool {
            false
        }
    }

    impl<K: Dump, V: Dump> Dump for BTreeMap<K, V> {
        const NILABLE: bool = true;

        fn dump(&self, out: &mut String) {
            let entries = self
                .iter()
                .map(|(k, v)| {
                    let mut entry = dump(k);
                    entry.push_str(": ");
                    v.dump(&mut entry);
                    entry
                })
                .collect();
            push_map(Some(entries), out);
        }

        fn is_zero(&self) -> bool {
            false
        }
    }

    /// Go's rendering of a map from its rendered entries: sorted, or `nil`.
    fn push_map(entries: Option<Vec<String>>, out: &mut String) {
        let Some(mut entries) = entries else {
            out.push_str("nil");
            return;
        };
        entries.sort();
        out.push('{');
        out.push_str(&entries.join(", "));
        out.push('}');
    }

    /// Go's `dump` of `v` held in a value of the static type `ty`: an interface shows the
    /// dynamic type.
    fn dump_value(ty: Type, v: &Value, out: &mut String) {
        if ty == Type::Interface {
            let Some(dynamic) = v.type_of() else {
                out.push_str("nil");
                return;
            };
            out.push('<');
            dynamic.write_name(out);
            out.push('>');
            dump_value(dynamic, v, out);
            return;
        }
        match v {
            Value::Nil => out.push_str("nil"),
            Value::Bool(b) => b.dump(out),
            Value::Int(i) | Value::Int64(i) => i.dump(out),
            Value::Int8(i) => i.dump(out),
            Value::Int16(i) => i.dump(out),
            Value::Int32(i) => i.dump(out),
            Value::Uint(u) | Value::Uint64(u) => u.dump(out),
            Value::Uint8(u) => u.dump(out),
            Value::Uint16(u) => u.dump(out),
            Value::Uint32(u) => u.dump(out),
            Value::Float32(f) => f.dump(out),
            Value::Float64(f) => f.dump(out),
            Value::String(s) => s.dump(out),
            Value::Time(t) => t.dump(out),
            Value::Duration(d) => d.dump(out),
            Value::Slice(elem, items) => {
                let Some(items) = items else {
                    out.push_str("nil");
                    return;
                };
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    dump_value(**elem, item, out);
                }
                out.push(']');
            }
            Value::StringMap(elem, entries) => {
                let rendered = entries.as_ref().map(|m| {
                    m.iter()
                        .map(|(k, item)| {
                            let mut entry = strconv::quote(k);
                            entry.push_str(": ");
                            dump_value(**elem, item, &mut entry);
                            entry
                        })
                        .collect()
                });
                push_map(rendered, out);
            }
            Value::Map(key, elem, entries) => {
                let rendered = entries.as_ref().map(|m| {
                    m.iter()
                        .map(|(k, item)| {
                            let mut entry = String::new();
                            dump_value(**key, k, &mut entry);
                            entry.push_str(": ");
                            dump_value(**elem, item, &mut entry);
                            entry
                        })
                        .collect()
                });
                push_map(rendered, out);
            }
        }
    }

    /// Declares the mirror of a Go struct of the generator: the struct, deriving
    /// `GoMapstructure`, and its [`Dump`]. Each field gives its Go name and, when it has one,
    /// its `mapstructure` tag.
    macro_rules! mirror {
        ($(#[$meta:meta])* struct $name:ident {
            $($field:ident: $ty:ty => $go:literal $($tag:literal)?,)*
        }) => {
            #[derive(Debug, Default, GoMapstructure)]
            $(#[$meta])*
            struct $name {
                $(
                    #[goyaml(go = $go)]
                    $(#[goyaml(mapstructure = $tag)])?
                    $field: $ty,
                )*
            }

            impl Dump for $name {
                fn dump(&self, out: &mut String) {
                    let mut fields = Vec::new();
                    $(
                        if !self.$field.is_zero() {
                            let mut text = String::from($go);
                            text.push(':');
                            self.$field.dump(&mut text);
                            fields.push(text);
                        }
                    )*
                    out.push('{');
                    out.push_str(&fields.join(" "));
                    out.push('}');
                }

                fn is_zero(&self) -> bool {
                    true $(&& self.$field.is_zero())*
                }
            }
        };
    }

    mirror! {
        #[goyaml(go_type = "main.Inner")]
        struct Inner {
            name: String => "Name",
            count: isize => "Count",
            enabled: bool => "Enabled",
            ratio: f64 => "Ratio",
            tags: Option<Vec<String>> => "Tags",
            labels: Option<BTreeMap<String, String>> => "Labels",
            timeout: Duration => "Timeout",
            ptr: Option<isize> => "Ptr",
            any: Value => "Any",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.Outer")]
        struct Outer {
            host: String => "Host",
            port: isize => "Port",
            flag: Option<bool> => "Flag",
            nums: Option<Vec<isize>> => "Nums",
            uints: u32 => "Uints",
            inner: Inner => "Inner",
            items: Option<Vec<Inner>> => "Items",
            by_name: Option<BTreeMap<String, Inner>> => "ByName" "by_name",
            misc: Option<BTreeMap<String, Value>> => "Misc",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.Unused")]
        struct Unused {
            existing: bool => "Existing",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.ViperConfig")]
        struct ViperConfig {
            port: isize => "Port",
            name: String => "Name",
            duration: Duration => "Duration",
            modes: Option<Vec<isize>> => "Modes",
        }
    }

    // ---- values -------------------------------------------------------------------------------

    /// The Go type a spec names.
    fn go_type(name: &str) -> Result<&'static Type, String> {
        Ok(match name {
            "bool" => &Type::Bool,
            "int" => &Type::Int,
            "int8" => &Type::Int8,
            "int16" => &Type::Int16,
            "int32" => &Type::Int32,
            "int64" => &Type::Int64,
            "uint" => &Type::Uint,
            "uint8" => &Type::Uint8,
            "uint16" => &Type::Uint16,
            "uint32" => &Type::Uint32,
            "uint64" => &Type::Uint64,
            "float32" => &Type::Float32,
            "float64" => &Type::Float64,
            "string" => &Type::String,
            "interface {}" => &Type::Interface,
            "time.Time" => &Type::Time,
            "time.Duration" => &Type::Duration,
            "[]string" => &Type::Slice(&Type::String),
            "[]int" => &Type::Slice(&Type::Int),
            "[]interface {}" => &Type::Slice(&Type::Interface),
            _ => return Err(format!("no Rust type for the Go type {name:?}")),
        })
    }

    /// The Go scalar `[type, text]` stands for.
    fn scalar(ty: &str, payload: &Json) -> Result<Value, String> {
        let text = payload.as_str().unwrap_or_default();
        let bad = |e: &dyn Display| format!("{ty} {payload}: {e}");
        let int = |bits| strconv::parse_int(text, 10, bits).map_err(|e| bad(&e));
        let uint = |bits| strconv::parse_uint(text, 10, bits).map_err(|e| bad(&e));
        let narrow = |e: TryFromIntError| bad(&e);
        Ok(match ty {
            "bool" => Value::Bool(payload.as_bool().ok_or_else(|| bad(&"not a bool"))?),
            "int" => Value::Int(int(64)?),
            "int8" => Value::Int8(i8::try_from(int(8)?).map_err(narrow)?),
            "int16" => Value::Int16(i16::try_from(int(16)?).map_err(narrow)?),
            "int32" => Value::Int32(i32::try_from(int(32)?).map_err(narrow)?),
            "int64" => Value::Int64(int(64)?),
            "uint" => Value::Uint(uint(64)?),
            "uint8" => Value::Uint8(u8::try_from(uint(8)?).map_err(narrow)?),
            "uint16" => Value::Uint16(u16::try_from(uint(16)?).map_err(narrow)?),
            "uint32" => Value::Uint32(u32::try_from(uint(32)?).map_err(narrow)?),
            "uint64" => Value::Uint64(uint(64)?),
            "float32" => Value::Float32(math::float32(
                strconv::parse_float(text, 32).map_err(|e| bad(&e))?,
            )),
            "float64" => Value::Float64(strconv::parse_float(text, 64).map_err(|e| bad(&e))?),
            "string" => Value::String(text.to_owned()),
            "time.Duration" => Value::Duration(Duration(int(64)?)),
            "time.Time" => Value::Time(time::parse(time::RFC3339_NANO, text).map_err(|e| bad(&e))?),
            _ => return Err(format!("unknown scalar type {ty:?}")),
        })
    }

    /// The value a spec stands for (see `spec` in `testdata/go/main.go`).
    fn value(spec: &Json) -> Result<Value, String> {
        let parts = spec
            .as_array()
            .ok_or_else(|| format!("bad value spec {spec}"))?;
        let part = |i: usize| parts.get(i).unwrap_or(&Json::Null);
        let tag = part(0).as_str().unwrap_or_default();
        match tag {
            "nil" => Ok(Value::Nil),
            "slice" => {
                let elem = go_type(part(1).as_str().unwrap_or_default())?;
                let items = part(2)
                    .as_array()
                    .map(|items| items.iter().map(value).collect::<Result<Vec<_>, _>>())
                    .transpose()?;
                Ok(Value::Slice(elem, items))
            }
            "map" => {
                let key = go_type(part(1).as_str().unwrap_or_default())?;
                let elem = go_type(part(2).as_str().unwrap_or_default())?;
                let pairs = part(3)
                    .as_array()
                    .map(|pairs| {
                        pairs
                            .iter()
                            .map(|pair| {
                                let k = value(pair.get(0).unwrap_or(&Json::Null))?;
                                let v = value(pair.get(1).unwrap_or(&Json::Null))?;
                                Ok((k, v))
                            })
                            .collect::<Result<Vec<_>, String>>()
                    })
                    .transpose()?;
                if *key != Type::String {
                    return Ok(Value::Map(key, elem, pairs));
                }
                let entries = pairs.map(|pairs| {
                    pairs
                        .into_iter()
                        .map(|(k, v)| (k.as_str().unwrap_or_default().to_owned(), v))
                        .collect()
                });
                Ok(Value::StringMap(elem, entries))
            }
            _ => scalar(tag, part(1)),
        }
    }

    // ---- operations ---------------------------------------------------------------------------

    fn text_of<'a>(j: &'a Json, name: &str) -> &'a str {
        j.get(name).and_then(Json::as_str).unwrap_or_default()
    }

    fn strings_of(j: &Json, name: &str) -> Vec<String> {
        j.get(name)
            .and_then(Json::as_array)
            .map_or_else(Vec::new, |items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap_or_default().to_owned())
                    .collect()
            })
    }

    fn object_of<'a>(j: &'a Json, name: &str) -> Vec<(&'a str, &'a str)> {
        j.get(name)
            .and_then(Json::as_object)
            .map_or_else(Vec::new, |m| {
                m.iter()
                    .map(|(k, v)| (k.as_str(), v.as_str().unwrap_or_default()))
                    .collect()
            })
    }

    /// Go's `err.Error()`, or `""` for no error.
    fn error_text(result: Result<(), ViperError>) -> Json {
        Json::String(result.err().map_or_else(String::new, |e| {
            String::from_utf8_lossy(&e.go_text()).into_owned()
        }))
    }

    fn dumped<T: Dump + ?Sized>(v: &T) -> Json {
        Json::String(dump(v))
    }

    /// Decodes into a new `T` as the operation says: the dump of `T` and the error text.
    fn decode<T: MsDecode + Dump + Default>(v: &Viper, op: &str, key: &str) -> Json {
        let mut target = T::default();
        let result = match op {
            "unmarshal" => v.unmarshal(&mut target),
            "unmarshal_exact" => v.unmarshal_exact(&mut target),
            _ => v.unmarshal_key(key, &mut target),
        };
        let mut out = dump(&target);
        out.push_str(" | ");
        if let Err(e) = result {
            out.push_str(&e.to_string());
        }
        Json::String(out)
    }

    /// Runs one operation: `null` for a setter, else its outcome as the golden writes it.
    fn run_op(v: &mut Viper, op: &Json, tmp: &str) -> Result<Json, String> {
        let name = text_of(op, "op");
        let key = text_of(op, "key");
        let arg = text_of(op, "arg").replace("@TMP@", tmp);
        match name {
            "set_default" => v.set_default(key, value(op.get("value").unwrap_or(&Json::Null))?),
            "set" => v.set(key, value(op.get("value").unwrap_or(&Json::Null))?),
            "bind_env" => {
                let args = strings_of(op, "args");
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                return Ok(error_text(v.bind_env(&args)));
            }
            "bind_bidder_envs" => {
                // config.setBidderDefaults, for each bidder.
                let mut result = Ok(());
                'bidders: for bidder in strings_of(op, "args") {
                    for suffix in strings_of(op, "suffixes") {
                        result = v.bind_env(&[&format!("adapters.{bidder}.{suffix}")]);
                        if result.is_err() {
                            break 'bidders;
                        }
                    }
                }
                return Ok(error_text(result));
            }
            "set_env_prefix" => v.set_env_prefix(&arg),
            "set_env_key_replacer" => {
                let pairs = strings_of(op, "pairs");
                let (pairs, _odd) = pairs.as_chunks::<2>();
                let pairs = pairs.iter().map(|[old, new]| (old.clone(), new.clone()));
                v.set_env_key_replacer(Some(Replacer::new(pairs)));
            }
            "automatic_env" => v.automatic_env(),
            "allow_empty_env" => v.allow_empty_env(arg == "true"),
            "set_type_by_default_value" => v.set_type_by_default_value(arg == "true"),
            "set_config_name" => v.set_config_name(&arg),
            "set_config_type" => v.set_config_type(&arg),
            "set_config_file" => v.set_config_file(&arg),
            "add_config_path" => v.add_config_path(&arg),
            "read_in_config" => return Ok(error_text(v.read_in_config())),
            "read_config" => return Ok(error_text(v.read_config(arg.as_bytes()))),
            _ => return observe(v, op, name, key),
        }
        Ok(Json::Null)
    }

    /// Runs one observation, giving its outcome as the golden writes it.
    fn observe(v: &Viper, op: &Json, name: &str, key: &str) -> Result<Json, String> {
        Ok(match name {
            "config_file_used" => Json::String(v.config_file_used().to_owned()),
            "get" => dumped(&v.get(key).unwrap_or_default()),
            "get_string" => dumped(&v.get_string(key)),
            "get_bool" => dumped(&v.get_bool(key)),
            "get_int" => dumped(&v.get_int(key)),
            "get_int32" => dumped(&v.get_int32(key)),
            "get_int64" => dumped(&v.get_int64(key)),
            "get_uint" => dumped(&v.get_uint(key)),
            "get_uint32" => dumped(&v.get_uint32(key)),
            "get_uint64" => dumped(&v.get_uint64(key)),
            "get_float64" => dumped(&v.get_float64(key)),
            "get_time" => dumped(&v.get_time(key)),
            "get_duration" => dumped(&v.get_duration(key)),
            "get_int_slice" => dumped(&v.get_int_slice(key)),
            "get_string_slice" => dumped(&v.get_string_slice(key)),
            "get_string_map" => dumped(&v.get_string_map(key)),
            "get_string_map_string" => dumped(&v.get_string_map_string(key)),
            "get_string_map_string_slice" => dumped(&v.get_string_map_string_slice(key)),
            "get_size_in_bytes" => dumped(&v.get_size_in_bytes(key)),
            "is_set" => Json::Bool(v.is_set(key)),
            "in_config" => Json::Bool(v.in_config(key)),
            "all_keys" => Json::Array(v.all_keys().into_iter().map(Json::String).collect()),
            "all_settings" => dumped(&v.all_settings()),
            "sub" => v.sub(key).map_or_else(
                || Json::String("nil".to_owned()),
                |sub| dumped(&sub.all_settings()),
            ),
            "unmarshal" | "unmarshal_exact" | "unmarshal_key" => match text_of(op, "target") {
                "outer" => decode::<Outer>(v, name, key),
                "inner" => decode::<Inner>(v, name, key),
                "unused" => decode::<Unused>(v, name, key),
                "viper_config" => decode::<ViperConfig>(v, name, key),
                "map" => decode::<Option<BTreeMap<String, Value>>>(v, name, key),
                "strings" => decode::<Option<Vec<String>>>(v, name, key),
                "ints" => decode::<Option<Vec<isize>>>(v, name, key),
                "int" => decode::<isize>(v, name, key),
                "string" => decode::<String>(v, name, key),
                "bool_ptr" => decode::<Option<bool>>(v, name, key),
                "duration" => decode::<Duration>(v, name, key),
                "string_map_string" => decode::<Option<BTreeMap<String, String>>>(v, name, key),
                target => return Err(format!("unknown target {target:?}")),
            },
            _ => return Err(format!("unknown operation {name:?}")),
        })
    }

    /// Writes the temporary directory of a result as `@TMP@`, as the generator does.
    fn untmp(got: Json, tmp: &str) -> Json {
        if let Some(s) = got.as_str() {
            return Json::String(s.replace(tmp, "@TMP@"));
        }
        if let Some(items) = got.as_array() {
            return Json::Array(
                items
                    .iter()
                    .map(|item| {
                        Json::String(item.as_str().unwrap_or_default().replace(tmp, "@TMP@"))
                    })
                    .collect(),
            );
        }
        got
    }

    // ---- scenarios ----------------------------------------------------------------------------

    /// The scenarios' symbolic links need a Unix file system.
    #[cfg(not(unix))]
    fn symlink(_target: &str, link: PathBuf) -> io::Result<()> {
        Err(io::Error::other(format!(
            "{}: symbolic links need Unix",
            link.display()
        )))
    }

    /// A new temporary directory, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Result<Self, String> {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, atomic::Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("goviper-golden-{}-{n}", process::id()));
            let _stale = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            // The kernel reports the working directory with symbolic links resolved, and so
            // does the generator's placeholder.
            fs::canonicalize(&dir)
                .map(Self)
                .map_err(|e| format!("{}: {e}", dir.display()))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _removed = fs::remove_dir_all(&self.0);
        }
    }

    /// The mismatches of one golden file.
    struct Report {
        file: &'static str,
        scenarios: usize,
        outputs: usize,
        failures: Vec<String>,
    }

    impl Report {
        const fn new(file: &'static str) -> Self {
            Self {
                file,
                scenarios: 0,
                outputs: 0,
                failures: Vec::new(),
            }
        }

        fn finish(self, min_outputs: usize) -> Result<(), String> {
            if self.outputs < min_outputs {
                return Err(format!(
                    "{}: only {} outputs in {} scenarios",
                    self.file, self.outputs, self.scenarios
                ));
            }
            if self.failures.is_empty() {
                return Ok(());
            }
            let mut report = format!(
                "{}: {} of {} outputs differ from Go\n",
                self.file,
                self.failures.len(),
                self.outputs
            );
            for failure in self.failures.iter().take(40) {
                report.push_str(failure);
                report.push('\n');
            }
            Err(report)
        }
    }

    /// Runs a scenario's operations, comparing each outcome with Go's.
    fn run_scenario(scenario: &Json, report: &mut Report) -> Result<(), String> {
        let name = text_of(scenario, "name");
        let dir = TempDir::new()?;
        let tmp = dir
            .0
            .to_str()
            .ok_or_else(|| format!("{}: not UTF-8", dir.0.display()))?
            .to_owned();
        let io = |e: io::Error| format!("{name}: {e}");
        for sub in strings_of(scenario, "dirs") {
            fs::create_dir_all(dir.0.join(sub)).map_err(io)?;
        }
        for (rel, content) in object_of(scenario, "files") {
            let path = dir.0.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(io)?;
            }
            fs::write(&path, content).map_err(io)?;
        }
        for (rel, target) in object_of(scenario, "links") {
            symlink(target, dir.0.join(rel)).map_err(io)?;
        }
        let vars = object_of(scenario, "env")
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.replace("@TMP@", &tmp)));
        let cwd = text_of(scenario, "cwd");
        let wd = if cwd.is_empty() {
            dir.0.clone()
        } else {
            dir.0.join(cwd)
        };
        let mut v = Viper::with_environment(Environment::from_vars(vars).with_working_dir(wd));
        let ops = scenario
            .get("ops")
            .and_then(Json::as_array)
            .ok_or_else(|| format!("{name}: no operations"))?;
        report.scenarios += 1;
        for (i, op) in ops.iter().enumerate() {
            let got = untmp(run_op(&mut v, op, &tmp)?, &tmp);
            let want = op.get("want").unwrap_or(&Json::Null);
            if got.is_null() && want.is_null() {
                continue;
            }
            report.outputs += 1;
            if got != *want {
                report.failures.push(format!(
                    "{name} #{i} {} {:?}\n      go:   {want}\n      rust: {got}",
                    text_of(op, "op"),
                    text_of(op, "key"),
                ));
            }
        }
        Ok(())
    }

    /// Replays every scenario of a golden file.
    fn replay(file: &'static str, min_outputs: usize) -> Result<(), String> {
        let path = format!("{}/testdata/golden/{file}", env!("CARGO_MANIFEST_DIR"));
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        let mut report = Report::new(file);
        for (n, line) in text.lines().enumerate() {
            let scenario: Json =
                serde_json::from_str(line).map_err(|e| format!("{file} line {}: {e}", n + 1))?;
            run_scenario(&scenario, &mut report)?;
        }
        report.finish(min_outputs)
    }

    /// viper's own tests (`viper_test.go`, `overrides_test.go`), cast's conversions of every
    /// value type, configuration file searches and errors, and seeded random combinations of
    /// every source.
    #[test]
    fn scenarios() -> Result<(), String> {
        replay("scenarios.jsonl", 20_000)
    }

    /// `config.SetupViper`'s operations (rebuilt from `rust/testdata/go_config_defaults.json`,
    /// which the generator checks against `SetupViper` itself, `[]config.AgmaAnalyticsAccount{}`
    /// and `[]openrtb_ext.BidderName{}` defaults standing as `[]interface{}{}`): `main_test.go`'s
    /// `TestViperInit` and `TestViperEnv`, and `main.loadConfig` with no configuration, with a
    /// `pbs.yaml` and `PBS_*` variables, with a `pbs.json`, and with a malformed file.
    #[test]
    fn prebid_setup_viper() -> Result<(), String> {
        replay("prebid.jsonl", 7_500)
    }
}
