//! Command-line parsing and dispatch for the `pbs-devtools` binary.

use std::{
    io::{self, Write},
    path::PathBuf,
};

use crate::{
    error::{Result, ToolError},
    godump::{self, Dump},
    manifest::{self, MANIFEST_PATH},
    repo::{self, Mode, Outcome, Repo},
    typegen,
};

const USAGE: &str = "\
usage: pbs-devtools <command> [--check] [--root DIR] [--go-commit SHA]

commands:
  adapter-manifest   regenerate rust/testdata/go_fixture_manifest.json from adapters/*/*_test.go
  go-dump <what>     run a Go dump program; <what> is struct-meta, config-defaults,
                     bidder-infos, bidder-names or all
  typegen            regenerate the Rust types of the Go packages in rust/crates/*/typegen.json
  all                adapter-manifest, go-dump all and typegen
  help               print this text

options:
  --check            do not write; fail if a committed file differs from a fresh generation
  --root DIR         repository root (default: $PBS_GO_ROOT, else found from the current directory)
  --go-commit SHA    commit recorded as go_commit (default: $PBS_GO_COMMIT, else the last commit
                     touching *.go, go.mod, go.sum, static/ or adapters/ outside rust/)
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Manifest,
    GoDump(Vec<Dump>),
    Typegen,
    All,
    Help,
}

#[derive(Debug)]
struct Options {
    command: Command,
    mode: Mode,
    root: Option<PathBuf>,
    go_commit: Option<String>,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut opts = Options {
        command: Command::Help,
        mode: Mode::Write,
        root: None,
        go_commit: None,
    };
    let mut positional = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--check" => opts.mode = Mode::Check,
            "--root" => {
                opts.root = Some(
                    it.next()
                        .ok_or_else(|| ToolError::new("--root needs a directory"))?
                        .into(),
                );
            }
            "--go-commit" => {
                opts.go_commit = Some(
                    it.next()
                        .ok_or_else(|| ToolError::new("--go-commit needs a value"))?
                        .clone(),
                );
            }
            "-h" | "--help" => positional.insert(0, "help".to_owned()),
            a if a.starts_with('-') => {
                return Err(ToolError::new(format!("unknown option {a}\n\n{USAGE}")));
            }
            a => positional.push(a.to_owned()),
        }
    }
    opts.command = match positional
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["help", ..] => Command::Help,
        ["adapter-manifest"] => Command::Manifest,
        ["typegen"] => Command::Typegen,
        ["all"] => Command::All,
        ["go-dump", "all"] => Command::GoDump(Dump::ALL.to_vec()),
        ["go-dump", what] => {
            Command::GoDump(vec![Dump::from_name(what).ok_or_else(|| {
                ToolError::new(format!("unknown dump {what}\n\n{USAGE}"))
            })?])
        }
        other => {
            return Err(ToolError::new(format!(
                "unexpected arguments {other:?}\n\n{USAGE}"
            )));
        }
    };
    Ok(opts)
}

/// Runs the command line `args` (without the program name).
///
/// # Errors
///
/// Fails on bad arguments, on any generation error, and in `--check` mode when a committed file
/// is stale.
pub fn run(args: &[String]) -> Result<()> {
    let opts = parse(args)?;
    let mut out = io::stdout().lock();
    if opts.command == Command::Help {
        out.write_all(USAGE.as_bytes())?;
        return Ok(());
    }
    let repo = Repo::locate(opts.root.as_deref())?;
    let go_commit = repo.go_commit(opts.go_commit.as_deref())?;
    let (manifest, dumps, types) = match opts.command {
        Command::Manifest => (true, Vec::new(), false),
        Command::GoDump(d) => (false, d, false),
        Command::Typegen => (false, Vec::new(), true),
        Command::All => (true, Dump::ALL.to_vec(), true),
        Command::Help => (false, Vec::new(), false),
    };
    let mut stale = Vec::new();
    if manifest {
        let m = manifest::generate(&repo, &go_commit)?;
        let outcome = repo::emit(&repo.path(MANIFEST_PATH), &repo::to_json(&m)?, opts.mode)?;
        report(&mut out, MANIFEST_PATH, outcome, &mut stale)?;
    }
    for dump in dumps {
        let outcome = godump::run(&repo, dump, &go_commit, opts.mode)?;
        report(&mut out, dump.golden(), outcome, &mut stale)?;
    }
    if types {
        for (path, outcome) in typegen::run(&repo, opts.mode)? {
            report(&mut out, &path, outcome, &mut stale)?;
        }
    }
    if !stale.is_empty() {
        return Err(ToolError::new(format!(
            "{} generated file(s) are stale: {}\nregenerate with \
             `cargo run -p pbs-devtools -- all` (go_commit {go_commit}) and commit the result",
            stale.len(),
            stale.join(", ")
        )));
    }
    Ok(())
}

fn report<W: Write>(
    out: &mut W,
    path: &str,
    outcome: Outcome,
    stale: &mut Vec<String>,
) -> Result<()> {
    let word = match outcome {
        Outcome::Written => "wrote",
        Outcome::Unchanged => "unchanged",
        Outcome::Stale => {
            stale.push(path.to_owned());
            "STALE"
        }
    };
    writeln!(out, "{word:>9} {path}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Command, parse};
    use crate::{
        error::Result,
        godump::Dump,
        repo::Mode,
        testutil::{check, check_eq},
    };

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn parses_commands() -> Result<()> {
        check_eq(&parse(&args(""))?.command, &Command::Help, "no args")?;
        let o = parse(&args("adapter-manifest --check --root /x"))?;
        check_eq(&o.command, &Command::Manifest, "manifest")?;
        check_eq(&o.mode, &Mode::Check, "check")?;
        check_eq(&o.root.as_deref(), &Some(Path::new("/x")), "root")?;
        check_eq(
            &parse(&args("typegen --check"))?.command,
            &Command::Typegen,
            "typegen",
        )?;
        check_eq(
            &parse(&args("go-dump all"))?.command,
            &Command::GoDump(Dump::ALL.to_vec()),
            "all",
        )?;
        let one = parse(&args("go-dump struct-meta --go-commit abc"))?;
        check_eq(
            &one.command,
            &Command::GoDump(vec![Dump::StructMeta]),
            "one",
        )?;
        check_eq(&one.go_commit.as_deref(), &Some("abc"), "go commit")?;
        check(parse(&args("go-dump nope")).is_err(), "unknown dump")?;
        check(parse(&args("--bogus")).is_err(), "unknown option")?;
        check(
            parse(&args("adapter-manifest extra")).is_err(),
            "extra argument",
        )
    }
}
