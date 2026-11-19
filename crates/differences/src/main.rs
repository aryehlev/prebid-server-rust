//! The `pbs-differences` command: validates every `crates/<crate>/differences.toml` and writes
//! (`--write`) or checks (`--check`) `docs/DIFFERENCES.md` and `docs/differences.json`. See the
//! `pbs_differences::ledger` documentation for the fragment schema; `pbs-differences --help`
//! prints the usage.

use std::{env, io};

use pbs_differences::ledger::{self, LedgerError};

fn main() -> Result<(), LedgerError> {
    let args: Vec<String> = env::args().skip(1).collect();
    ledger::run(&args, &mut io::stdout().lock())
}
