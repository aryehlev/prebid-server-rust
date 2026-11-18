//! The per-crate indexes: a Rust file listing every ported type, constant and function (for the
//! crate's parity tests), and a Go file listing the same types (for its Go golden generators).

use std::collections::{BTreeMap, BTreeSet};

use super::{
    config::{Config, GoIndex},
    docs::WIDTH,
    emit::{GENERATED, Generated, imports, literal},
    model::Package,
};
use crate::error::{Result, ToolError};

/// The Rust index: the `for_each_type!`, `for_each_struct!` and `for_each_const!` macros, the
/// `TYPES` and `CONSTANTS` name tables, and a signature check per ported function.
#[must_use]
pub fn rust(config: &Config, generated: &[Generated]) -> String {
    let krate = &config.crate_name;
    let mut lines = vec![
        GENERATED.to_owned(),
        "//".to_owned(),
        format!("// Config: {}", config.source),
        String::new(),
        format!("//! Every Go type, constant and function that `{krate}` ports, for its tests."),
        String::new(),
    ];
    let roots: BTreeSet<String> = generated
        .iter()
        .filter_map(|g| g.module.split("::").next())
        .map(|root| format!("{krate}::{root}"))
        .collect();
    lines.extend(imports(&roots));
    lines.push(String::new());
    lines.push(
        "/// Calls `$m!(RustType)` for every ported Go type, in the order of [`TYPES`].".to_owned(),
    );
    lines.push("macro_rules! for_each_type {".to_owned());
    lines.push("    ($m:ident) => {".to_owned());
    for t in generated.iter().flat_map(|g| &g.types) {
        lines.push(format!("        $m!({});", t.rust));
    }
    lines.push("    };".to_owned());
    lines.push("}".to_owned());
    lines.push(String::new());
    lines.push("pub(super) use for_each_type;".to_owned());
    lines.push(String::new());
    lines.push(
        "/// Calls `$m!(RustType)` for every ported Go struct type, in generation order."
            .to_owned(),
    );
    lines.push("macro_rules! for_each_struct {".to_owned());
    lines.push("    ($m:ident) => {".to_owned());
    for t in generated
        .iter()
        .flat_map(|g| &g.types)
        .filter(|t| t.is_struct)
    {
        lines.push(format!("        $m!({});", t.rust));
    }
    lines.push("    };".to_owned());
    lines.push("}".to_owned());
    lines.push(String::new());
    lines.push("pub(super) use for_each_struct;".to_owned());
    lines.push(String::new());
    lines.push(
        "/// The Go names (`reflect.Type.String()`) of the types of [`for_each_type`], in order."
            .to_owned(),
    );
    lines.extend(str_slice(
        "TYPES",
        generated.iter().flat_map(|g| &g.types).map(|t| &t.go_name),
    ));
    lines.push(String::new());
    lines.push(
        "/// Calls `$m!(CONSTANT)` for every ported Go constant, in the order of [`CONSTANTS`]."
            .to_owned(),
    );
    lines.push("macro_rules! for_each_const {".to_owned());
    lines.push("    ($m:ident) => {".to_owned());
    for c in generated.iter().flat_map(|g| &g.consts) {
        lines.push(format!("        $m!({});", c.rust));
    }
    lines.push("    };".to_owned());
    lines.push("}".to_owned());
    lines.push(String::new());
    lines.push("pub(super) use for_each_const;".to_owned());
    lines.push(String::new());
    lines.push(
        "/// The Go names (`pkg.Name`) of the constants of [`for_each_const`], in order."
            .to_owned(),
    );
    lines.extend(str_slice(
        "CONSTANTS",
        generated.iter().flat_map(|g| &g.consts).map(|c| &c.go_name),
    ));
    let funcs: Vec<_> = generated.iter().flat_map(|g| &g.funcs).collect();
    if !funcs.is_empty() {
        lines.push(String::new());
        lines.push(
            "// The hand-written port of every Go function and method, with its signature."
                .to_owned(),
        );
        for f in funcs {
            lines.push(format!("// {}", f.go_name));
            let line = format!("const _: {} = {};", f.signature, f.rust);
            if line.len() <= WIDTH {
                lines.push(line);
            } else {
                lines.push(format!("const _: {} =", f.signature));
                lines.push(format!("    {};", f.rust));
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// `pub(super) const NAME: &[&str] = &[...];`, one element per line as rustfmt lays out
/// string elements longer than `short_array_element_width_threshold`.
fn str_slice<'a, I: Iterator<Item = &'a String>>(name: &str, items: I) -> Vec<String> {
    let items: Vec<String> = items.map(|s| format!("    {},", literal(s))).collect();
    if items.is_empty() {
        return vec![format!("pub(super) const {name}: &[&str] = &[];")];
    }
    let mut out = vec![format!("pub(super) const {name}: &[&str] = &[")];
    out.extend(items);
    out.push("];".to_owned());
    out
}

/// The Go index: `portedTypes`, every ported type with the Rust path of its port.
///
/// # Errors
///
/// Fails when a generated package was not described.
pub fn go(
    config: &Config,
    index: &GoIndex,
    generated: &[Generated],
    packages: &BTreeMap<String, Package>,
) -> Result<String> {
    let krate = &config.crate_name;
    let mut qualifiers = BTreeMap::new();
    let mut specs = Vec::new();
    for g in generated {
        let pkg = packages
            .get(&g.go_path)
            .ok_or_else(|| ToolError::new(format!("package {} was not described", g.go_path)))?;
        let alias: String = g.module.split("::").collect();
        let last = g.go_path.rsplit('/').next().unwrap_or_default();
        let qualifier = if alias == pkg.name && last == pkg.name {
            specs.push((g.go_path.clone(), format!("\t\"{}\"", g.go_path)));
            pkg.name.clone()
        } else {
            specs.push((g.go_path.clone(), format!("\t{alias} \"{}\"", g.go_path)));
            alias
        };
        drop(qualifiers.insert(g.go_path.clone(), qualifier));
    }
    specs.sort();
    let mut lines = vec![
        GENERATED.to_owned(),
        "//".to_owned(),
        format!("// Config: {}", config.source),
        String::new(),
        format!("package {}", index.package),
        String::new(),
        "import (".to_owned(),
        "\t\"reflect\"".to_owned(),
        String::new(),
    ];
    lines.extend(specs.into_iter().map(|s| s.1));
    lines.push(")".to_owned());
    lines.push(String::new());
    lines.push(format!(
        "// portedType is a Go type that {krate} ports, with the Rust path of the port."
    ));
    lines.push("type portedType struct {".to_owned());
    lines.push("\tgoType reflect.Type".to_owned());
    lines.push("\trust   string".to_owned());
    lines.push("}".to_owned());
    lines.push(String::new());
    lines.push(format!(
        "// portedTypes are the Go types {krate} ports, in generation order."
    ));
    lines.push("var portedTypes = []portedType{".to_owned());
    for g in generated {
        let q = qualifiers.get(&g.go_path).cloned().unwrap_or_default();
        for t in &g.types {
            lines.push(format!(
                "\t{{reflect.TypeOf((*{q}.{})(nil)).Elem(), \"{}\"}},",
                t.go, t.rust
            ));
        }
    }
    lines.push("}".to_owned());
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}
