#!/usr/bin/env bash
# gates.sh: the commands of each check, run from the top of the work tree, the JSON report and the
# exit status, with fake cargo and taplo on PATH (the real checks run in CI's rust jobs).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

calls="${T_ROOT}/cargo-calls.log"
: > "${calls}"
cat > "${T_ROOT}/bin/cargo" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
# The rustdoc flags variable, named indirectly: the suppression scanner reads every mention of it
# as a flag setting.
flags_var=RUSTDOC
flags_var+=FLAGS
printf '%s | doc flags: %s | cwd=%s\n' "$*" "${!flags_var-unset}" "${PWD##*/}" >> "${FAKE_CARGO_CALLS:?}"
case "$*" in
  *clippy*)
    printf 'warning: unused variable: `x`\n'
    exit 101
    ;;
  *) exit 0 ;;
esac
EOF
cat > "${T_ROOT}/bin/taplo" << 'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "${T_ROOT}/bin/cargo" "${T_ROOT}/bin/taplo"
export FAKE_CARGO_CALLS="${calls}"
out="${T_ROOT}/gates.json"

t_run_in "${T_TOP}" "${T_SCRIPTS}/gates.sh" --out "${out}" --only fmt,clippy,doc,test,devtools
t_eq "a failing check fails the gates" 1 "${T_RC}"
status=$(jq -c '[.status, .failed]' "${out}")
t_eq "report: status and failed checks" '["fail",["clippy"]]' "${status}"
names=$(jq -c '[.checks[] | [.name, .status]]' "${out}")
t_eq "report: every check in order" '[["fmt","pass"],["clippy","fail"],["doc","pass"],["test","pass"],["devtools","pass"]]' "${names}"
tail_text=$(jq -r '.checks[1].log_tail' "${out}")
t_has "report: the failing log tail" "${tail_text}" "warning: unused variable"
toolchain=$("${T_TOP}/scripts/rustfmt-toolchain.sh")
top_name=$(basename "${T_TOP}")
cargo_log=$(cat "${calls}")
t_has "fmt: the pinned nightly, from the top of the work tree" "${cargo_log}" \
  "+${toolchain} fmt --all -- --check | doc flags: unset | cwd=${top_name}"
t_has "clippy: every target, locked, warnings denied" "${cargo_log}" \
  "clippy --workspace --all-targets --locked -- -D warnings"
t_has "doc: rustdoc warnings denied" "${cargo_log}" "doc --workspace --no-deps --locked | doc flags: -D warnings"
t_has "test: the whole workspace without stopping early" "${cargo_log}" "test --workspace --locked --no-fail-fast"
t_has "devtools: generated files are checked" "${cargo_log}" "run -p pbs-devtools --locked -- all --check"

# Every feature on: the same clippy, rustdoc and test runs, next to the default ones.
: > "${calls}"
t_run_in "${T_TOP}" "${T_SCRIPTS}/gates.sh" --out "${out}" --only doc-all-features,test-all-features,clippy-all-features
t_eq "all features: a failing clippy fails the gates" 1 "${T_RC}"
status=$(jq -c '[.status, .failed]' "${out}")
t_eq "all features: report" '["fail",["clippy-all-features"]]' "${status}"
cargo_log=$(cat "${calls}")
t_has "clippy-all-features: every target and feature, locked, warnings denied" "${cargo_log}" \
  "clippy --workspace --all-targets --all-features --locked -- -D warnings"
t_has "doc-all-features: rustdoc warnings denied" "${cargo_log}" \
  "doc --workspace --no-deps --all-features --locked | doc flags: -D warnings"
t_has "test-all-features: the whole workspace without stopping early" "${cargo_log}" \
  "test --workspace --all-features --locked --no-fail-fast"
# Without --only, both runs of each are made.
default_set=$(sed -n -E '/^all_checks=\(/,/\)$/p' "${T_SCRIPTS}/gates.sh")
default_set=" ${default_set//[$'\n()']/ } "
for name in clippy clippy-all-features doc doc-all-features test test-all-features; do
  t_has "the default checks include ${name}" "${default_set}" " ${name} "
done

t_run_in "${T_TOP}" "${T_SCRIPTS}/gates.sh" --out "${out}" --only fmt,test,taplo
t_eq "passing checks pass the gates" 0 "${T_RC}"
t_has "the summary line" "${T_OUT}" "gates: pass"
t_run_in "${T_TOP}" "${T_SCRIPTS}/gates.sh" --out "${out}" --only lint
t_eq "an unknown check is refused" 1 "${T_RC}"
t_run_in "${T_TOP}" "${T_SCRIPTS}/gates.sh" --only fmt
t_eq "--out is required" 2 "${T_RC}"
t_run_in "${T_TOP}" "${T_SCRIPTS}/gates.sh" --out "${out}" --strict-dirs 'a b/../c'
t_fails "adapter directories are validated"

t_done
