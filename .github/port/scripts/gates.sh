#!/usr/bin/env bash
# gates.sh: run the checks a port must pass, the same ones CI runs on the Rust workspace, and write
# a JSON report:
#   {"status": "pass|fail", "failed": [names], "checks": [{"name", "status", "seconds", "command",
#    "log_tail", "details"}]}
# Exit status 0 when every check passes, 1 otherwise.
#
# Usage: gates.sh --out FILE [--strict-dirs a,b] [--only name,name]
# Checks (run from the top of the work tree with the toolchain rust-toolchain.toml pins, against
# the Go root in PBS_GO_ROOT or .upstream):
#   fmt          cargo +<nightly from rustfmt.toml> fmt --all -- --check
#   clippy       cargo clippy --workspace --all-targets --locked -- -D warnings
#   doc          cargo doc --workspace --no-deps --locked, with rustdoc warnings denied
#   test         cargo test --workspace --locked --no-fail-fast
#   clippy-all-features, doc-all-features, test-all-features
#                the same three with --all-features, so the code behind a feature (pbs-parity's
#                trace) is linted, documented and tested too. The default runs stay: only they
#                build the code a feature leaves out.
#   suppression  scripts/check-no-lint-suppression.sh
#   taplo        taplo fmt --check --config taplo.toml
#   machete      scripts/cargo-machete.sh
#   devtools     cargo run -p pbs-devtools --locked -- all --check (generated files match Go)
#   conformance  conformance.sh --strict-dirs <touched adapters>
#   go           go-tooling.sh (gofmt -s, go build and go test of the Go tooling modules)
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
out=""
strict_csv=""
only_csv=""
while [[ "$#" -gt 0 ]]; do
  if [[ "$#" -lt 2 ]]; then
    printf 'gates: %s needs a value\n' "${1}" >&2
    exit 2
  fi
  case "${1}" in
    --out) out=${2} ;;
    --strict-dirs) strict_csv=${2} ;;
    --only) only_csv=${2} ;;
    *)
      printf 'gates: unknown argument %s\n' "${1}" >&2
      exit 2
      ;;
  esac
  shift 2
done
if [[ -z "${out}" ]]; then
  printf 'usage: gates.sh --out FILE [--strict-dirs a,b] [--only name,name]\n' >&2
  exit 2
fi
strict_lines=$(port_csv_lines "${strict_csv}")
strict_dirs=$(paste -s -d, - <<< "${strict_lines}")
all_checks=(fmt clippy clippy-all-features doc doc-all-features test test-all-features suppression taplo
  machete devtools conformance go)
if [[ -n "${only_csv}" ]]; then
  only_lines=$(tr ',' '\n' <<< "${only_csv}")
  mapfile -t checks <<< "${only_lines}"
else
  checks=("${all_checks[@]}")
fi

work=$(mktemp -d)
trap 'rm -rf -- "${work}"' EXIT
rustfmt_toolchain=$(scripts/rustfmt-toolchain.sh)
: > "${work}/checks.jsonl"

run_check() {
  local name=${1:?} log="${work}/${1}.log" details='null' status started seconds tail_text
  local -a cmd=()
  case "${name}" in
    fmt) cmd=(cargo "+${rustfmt_toolchain}" fmt --all -- --check) ;;
    clippy) cmd=(cargo clippy --workspace --all-targets --locked -- -D warnings) ;;
    clippy-all-features) cmd=(cargo clippy --workspace --all-targets --all-features --locked -- -D warnings) ;;
    doc) cmd=(env RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked) ;;
    doc-all-features) cmd=(env RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked) ;;
    test) cmd=(cargo test --workspace --locked --no-fail-fast) ;;
    test-all-features) cmd=(cargo test --workspace --all-features --locked --no-fail-fast) ;;
    suppression) cmd=(scripts/check-no-lint-suppression.sh) ;;
    taplo) cmd=(taplo fmt --check --config taplo.toml) ;;
    machete) cmd=(scripts/cargo-machete.sh) ;;
    devtools) cmd=(cargo run -p pbs-devtools --locked -- all --check) ;;
    conformance)
      cmd=("${PORT_SCRIPTS}/conformance.sh" --strict-dirs "${strict_dirs}" --out "${work}/conformance.json")
      ;;
    go) cmd=("${PORT_SCRIPTS}/go-tooling.sh") ;;
    *) port_die "gates: unknown check ${name}" ;;
  esac
  printf '::group::gate %s: %s\n' "${name}" "${cmd[*]}"
  started=${SECONDS}
  status=pass
  if ! "${cmd[@]}" > "${log}" 2>&1; then
    status=fail
  fi
  seconds=$((SECONDS - started))
  tail -n 200 "${log}"
  printf '::endgroup::\n'
  printf 'gate %s: %s (%ss)\n' "${name}" "${status}" "${seconds}"
  if [[ "${name}" == conformance && -s "${work}/conformance.json" ]]; then
    details=$(jq -c '.' "${work}/conformance.json")
  fi
  tail_text=$(tail -n 80 "${log}" | cut -c1-400 | tail -c 12000)
  jq -n -c --arg name "${name}" --arg status "${status}" --argjson seconds "${seconds}" \
    --arg command "${cmd[*]}" --arg log_tail "${tail_text}" --argjson details "${details}" \
    '{name: $name, status: $status, seconds: $seconds, command: $command, log_tail: $log_tail,
      details: $details}' >> "${work}/checks.jsonl"
}

for check in "${checks[@]}"; do
  if [[ -n "${check}" ]]; then
    run_check "${check}"
  fi
done

jq -s '{status: (if all(.[]; .status == "pass") then "pass" else "fail" end),
        failed: map(select(.status != "pass") | .name), checks: .}' "${work}/checks.jsonl" > "${out}"
status=$(jq -r '.status' "${out}")
failed=$(jq -r '.failed | join(", ")' "${out}")
printf 'gates: %s%s\n' "${status}" "${failed:+ (failed: ${failed})}"
if [[ "${status}" != pass ]]; then
  exit 1
fi
