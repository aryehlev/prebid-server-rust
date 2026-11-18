#!/usr/bin/env bash
# check-worktree.sh: after a simulated agent session, commits are undone with their changes kept,
# Go probes are deleted, changes outside rust/ and to protected paths are reverted, added known
# failures are dropped (removals are kept), and every correction is recorded.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

repo="${T_ROOT}/repo"
t_repo "${repo}"
git -C "${repo}" switch -q -c port-work
t_put "${repo}" main.go 'package main'
t_put "${repo}" adapters/foo/foo.go 'package foo'
t_put "${repo}" adapters/foo/footest/exemplary/a.json '{}'
t_put "${repo}" adapters/foo/footest/exemplary/b.json '{}'
t_put "${repo}" adapters/foo/footest/exemplary/c.json '{}'
t_put "${repo}" rust/Cargo.toml '[workspace]'
t_put "${repo}" rust/scripts/a.sh 'echo a'
t_put "${repo}" rust/crates/x/src/lib.rs '//! x'
t_put "${repo}" rust/UPSTREAM_SYNC '1111111111111111111111111111111111111111'
t_put "${repo}" rust/upstream-port-log.jsonl '{"upstream":"1111111111111111111111111111111111111111","status":"blocked"}'
mkdir -p "${repo}/rust/conformance"
printf '# known failures\nadapters/foo/footest/exemplary/a.json\nadapters/foo/footest/exemplary/b.json # flaky\n' \
  > "${repo}/rust/conformance/known_failures.txt"
merged=$(t_commit "${repo}" "merged state")
ctx="${T_ROOT}/ctx"

# The session: a wanted change, a commit, edits outside rust/, protected edits, probes and a
# known-failures edit that removes one entry and adds another.
t_put "${repo}" rust/crates/x/src/lib.rs '//! x ported'
t_put "${repo}" rust/crates/x/src/new.rs '//! new module'
t_commit "${repo}" "agent commit" > /dev/null
t_put "${repo}" rust/crates/x/src/more.rs '//! more'
t_put "${repo}" main.go 'package main // agent edit'
t_put "${repo}" notes.txt 'agent notes'
t_put "${repo}" adapters/foo/zz_parity_scratch_test.go 'package foo'
t_put "${repo}" rust/crates/x/zz_parity_scratch_test.go 'package x'
t_put "${repo}" rust/Cargo.toml '[workspace] # agent edit'
t_put "${repo}" rust/scripts/new.sh 'echo new'
t_put "${repo}" rust/UPSTREAM_SYNC '2222222222222222222222222222222222222222'
t_put "${repo}" rust/upstream-port-log.jsonl '{"upstream":"1111111111111111111111111111111111111111","status":"ported"}'
t_put "${repo}" rust/clippy.toml 'too-many-lines-threshold = 100000'
t_put "${repo}" rust/.cargo/config.toml '[source.crates-io]'
printf '# known failures\nadapters/foo/footest/exemplary/b.json # flaky\nadapters/foo/footest/exemplary/c.json\n' \
  > "${repo}/rust/conformance/known_failures.txt"

t_run_in "${repo}" env MERGED_SHA="${merged}" CTX="${ctx}" "${T_SCRIPTS}/check-worktree.sh"
t_eq "check-worktree exits 0" 0 "${T_RC}"
head=$(git -C "${repo}" rev-parse HEAD)
t_eq "the session's commit is undone" "${merged}" "${head}"
lib=$(cat "${repo}/rust/crates/x/src/lib.rs")
t_eq "a change from the undone commit is kept" "//! x ported" "${lib}"
new_kept=no
if [[ -f "${repo}/rust/crates/x/src/new.rs" && -f "${repo}/rust/crates/x/src/more.rs" ]]; then
  new_kept=yes
fi
t_eq "new rust files are kept" yes "${new_kept}"
main_go=$(cat "${repo}/main.go")
t_eq "an edit outside rust/ is reverted" "package main" "${main_go}"
leftovers=no
for path in notes.txt adapters/foo/zz_parity_scratch_test.go rust/crates/x/zz_parity_scratch_test.go rust/scripts/new.sh \
  rust/clippy.toml rust/.cargo/config.toml; do
  if [[ -e "${repo}/${path}" ]]; then
    leftovers="yes: ${path}"
  fi
done
t_eq "untracked files outside rust/, probes and new protected files are removed" no "${leftovers}"
cargo_toml=$(cat "${repo}/rust/Cargo.toml")
t_eq "a protected edit is reverted" "[workspace]" "${cargo_toml}"
marker=$(cat "${repo}/rust/UPSTREAM_SYNC")
t_eq "an edit to the marker is reverted" "1111111111111111111111111111111111111111" "${marker}"
port_log=$(cat "${repo}/rust/upstream-port-log.jsonl")
t_eq "an edit to the port log is reverted" \
  '{"upstream":"1111111111111111111111111111111111111111","status":"blocked"}' "${port_log}"
known=$(cat "${repo}/rust/conformance/known_failures.txt")
t_eq "an added known failure is dropped and a removal is kept" \
  $'# known failures\nadapters/foo/footest/exemplary/b.json # flaky' "${known}"
staged=$(git -C "${repo}" diff --cached --name-only)
t_eq "nothing is left staged" "" "${staged}"
violations=$(cat "${ctx}/worktree-violations.txt")
t_has "the undone commit is recorded" "${violations}" "the session created commits"
t_has "the outside change is recorded" "${violations}" "changes outside rust/ were reverted"
t_has "the protected change is recorded" "${violations}" "changes to protected paths were reverted"
t_has "the known-failures addition is recorded" "${violations}" "adapters/foo/footest/exemplary/c.json"

t_run_in "${repo}" env MERGED_SHA="${merged}" CTX="${T_ROOT}/ctx2" "${T_SCRIPTS}/check-worktree.sh"
t_eq "a clean tree passes again" 0 "${T_RC}"
t_has "a clean tree records nothing" "${T_OUT}" "0 recorded violation(s)"

git -C "${repo}" switch -q -c elsewhere
t_run_in "${repo}" env MERGED_SHA="${merged}" CTX="${ctx}" "${T_SCRIPTS}/check-worktree.sh"
t_eq "another branch is refused" 1 "${T_RC}"
t_has "another branch is named" "${T_ERR}" "expected branch port-work"
git -C "${repo}" switch -q port-work

other=$(git -C "${repo}" commit-tree -m "unrelated" "${merged}^{tree}")
git -C "${repo}" reset -q --hard "${other}"
t_run_in "${repo}" env MERGED_SHA="${merged}" CTX="${ctx}" "${T_SCRIPTS}/check-worktree.sh"
t_eq "a rewritten history is refused" 1 "${T_RC}"
t_has "a rewritten history is named" "${T_ERR}" "no longer descends from"

t_done
