//! `pbs-devtools go-dump`: runs the Go dump programs under `rust/crates/devtools/go/` and writes
//! (or checks) their goldens in `rust/testdata/`.

use std::{env, ffi::OsString, fs, path::Path, process::Command};

use serde_json::Value;

use crate::{
    error::{Result, ToolError},
    repo::{self, Mode, Outcome, Repo, TempDir},
};

/// One Go dump program and the golden it writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dump {
    /// `go_struct_meta.json`: reflect metadata of the openrtb, `openrtb_ext`, adapters and config
    /// types.
    StructMeta,
    /// `go_config_defaults.json`: viper keys, defaults, env bindings and the default
    /// `Configuration`.
    ConfigDefaults,
    /// `go_bidder_infos.json`: `static/bidder-info` as loaded at startup.
    BidderInfos,
    /// `go_bidder_names.json`: the `openrtb_ext` bidder name tables.
    BidderNames,
}

impl Dump {
    /// Every dump, in the order `go-dump all` runs them.
    pub const ALL: [Self; 4] = [
        Self::BidderNames,
        Self::BidderInfos,
        Self::ConfigDefaults,
        Self::StructMeta,
    ];

    /// The command-line name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::StructMeta => "struct-meta",
            Self::ConfigDefaults => "config-defaults",
            Self::BidderInfos => "bidder-infos",
            Self::BidderNames => "bidder-names",
        }
    }

    /// Parses a command-line name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.name() == name)
    }

    /// The golden's repository-relative path.
    #[must_use]
    pub const fn golden(self) -> &'static str {
        match self {
            Self::StructMeta => "rust/testdata/go_struct_meta.json",
            Self::ConfigDefaults => "rust/testdata/go_config_defaults.json",
            Self::BidderInfos => "rust/testdata/go_bidder_infos.json",
            Self::BidderNames => "rust/testdata/go_bidder_names.json",
        }
    }

    /// The Go package (relative to the root) of the program.
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::StructMeta => "./rust/crates/devtools/go/structmeta",
            Self::ConfigDefaults => "./rust/crates/devtools/go/configdefaults",
            Self::BidderInfos => "./rust/crates/devtools/go/bidderinfos",
            Self::BidderNames => "./rust/crates/devtools/go/biddernames",
        }
    }
}

/// Runs `dump` and writes or checks its golden.
///
/// # Errors
///
/// Fails when the Go toolchain is missing, the program fails (its stderr is included), or its
/// output is not a JSON object carrying `go_commit`.
pub fn run(repo: &Repo, dump: Dump, go_commit: &str, mode: Mode) -> Result<Outcome> {
    let tmp = TempDir::new("pbs-devtools-go-dump")?;
    let result = produce(repo, dump, go_commit, tmp.path());
    let removed = tmp.remove();
    let bytes = result?;
    removed?;
    repo::emit(&repo.path(dump.golden()), &bytes, mode)
}

fn produce(repo: &Repo, dump: Dump, go_commit: &str, tmp: &Path) -> Result<Vec<u8>> {
    let out = tmp.join("out.json");
    let root = repo.root().as_os_str().to_owned();
    let mut args: Vec<OsString> = vec!["run".into()];
    if dump == Dump::StructMeta {
        // Step 1 writes the overlay that registers every declaration; step 2 compiles it in.
        let plan = tmp.join("plan");
        let plan_args: [OsString; 6] = [
            "run".into(),
            dump.program().into(),
            "-plan".into(),
            plan.clone().into(),
            "-plan-root".into(),
            root.clone(),
        ];
        go(repo, &plan_args)?;
        args.push("-overlay".into());
        args.push(plan.join("overlay.json").into());
    }
    args.push(dump.program().into());
    args.extend([
        "-out".into(),
        out.clone().into(),
        "-go-commit".into(),
        go_commit.into(),
        "-root".into(),
        root,
    ]);
    go(repo, &args)?;
    let bytes = fs::read(&out).map_err(|e| ToolError::io(&out, &e))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|e| ToolError::new(format!("{}: output is not JSON: {e}", dump.name())))?;
    if value.get("go_commit").and_then(Value::as_str) != Some(go_commit) {
        return Err(ToolError::new(format!(
            "{}: output does not record go_commit {go_commit}",
            dump.name()
        )));
    }
    Ok(bytes)
}

/// Runs `go <args>` in the repository root with every `PBS_*` variable removed.
fn go(repo: &Repo, args: &[OsString]) -> Result<()> {
    let go = env::var_os("GO").unwrap_or_else(|| "go".into());
    let mut cmd = Command::new(&go);
    let _ = cmd.args(args).current_dir(repo.root());
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("PBS_") {
            let _ = cmd.env_remove(key);
        }
    }
    let output = cmd.output().map_err(|e| {
        ToolError::new(format!(
            "running {}: {e} (is the Go toolchain installed? set GO to its path)",
            go.to_string_lossy()
        ))
    })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    let tail = lines
        .get(lines.len().saturating_sub(40)..)
        .unwrap_or_default()
        .join("\n");
    let shown: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    Err(ToolError::new(format!(
        "go {} failed ({}):\n{tail}",
        shown.join(" "),
        output.status
    )))
}

#[cfg(test)]
mod tests {
    use super::Dump;
    use crate::{
        error::Result,
        testutil::{check, check_eq},
    };

    #[test]
    fn names_round_trip() -> Result<()> {
        for d in Dump::ALL {
            check_eq(&Dump::from_name(d.name()), &Some(d), d.name())?;
            check(d.golden().starts_with("rust/testdata/go_"), "golden path")?;
        }
        check_eq(&Dump::from_name("all"), &None, "all is not a single dump")
    }
}
