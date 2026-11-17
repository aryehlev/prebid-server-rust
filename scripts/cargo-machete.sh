#!/usr/bin/env bash
# cargo-machete.sh: fail if any workspace member declares a dependency it does not use.
# The member list comes from `cargo metadata`, so only real workspace members are analyzed
# (rust/legacy/ is not one). --with-metadata resolves renamed dependencies correctly.
set -euo pipefail
shopt -s inherit_errexit

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cd "${here}/.."

metadata=$(cargo metadata --no-deps --format-version 1 --locked)
members=$(jq -r '.packages[].manifest_path | sub("/Cargo.toml$"; "")' <<< "${metadata}")
dirs=()
while IFS= read -r dir; do
  if [[ -n "${dir}" ]]; then dirs+=("${dir}"); fi
done <<< "${members}"
if [[ "${#dirs[@]}" -eq 0 ]]; then
  printf 'cargo-machete: cargo metadata lists no workspace members\n' >&2
  exit 1
fi
printf 'cargo-machete: %s workspace members\n' "${#dirs[@]}"
cargo machete --with-metadata "${dirs[@]}"
