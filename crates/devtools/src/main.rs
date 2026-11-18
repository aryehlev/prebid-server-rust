//! The `pbs-devtools` command. See the `pbs_devtools` library documentation for what each
//! subcommand produces; `pbs-devtools help` prints the usage.

use std::env;

use pbs_devtools::{cli, error::Result};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    cli::run(&args)
}
