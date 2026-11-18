#!/usr/bin/env bash
# rustfmt-toolchain.sh: print the nightly toolchain that checks formatting. It is pinned once, in
# the header comment of rustfmt.toml that reads
#   cargo +nightly-YYYY-MM-DD fmt --all -- --check
# so CI, the port gates and developers all use the same rustfmt.
set -euo pipefail
shopt -s inherit_errexit

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
file="${here}/../rustfmt.toml"
toolchain=$(sed -n -E 's/.*cargo [+](nightly-[0-9]{4}-[0-9]{2}-[0-9]{2}) fmt.*/\1/p' "${file}")
toolchain=${toolchain%%$'\n'*}
if [[ -z "${toolchain}" ]]; then
  printf 'rustfmt-toolchain: no "cargo +nightly-YYYY-MM-DD fmt" line in %s\n' "${file}" >&2
  exit 1
fi
printf '%s\n' "${toolchain}"
