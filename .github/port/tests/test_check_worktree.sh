#!/usr/bin/env bash
# check-worktree.sh: after a simulated agent session, commits are undone with their changes kept,
# Go probes are deleted, changes outside the writable paths (protected paths, the port records,
# tooling) are reverted, added known failures are dropped (removals are kept), the pin is applied
# again, the probe tree is reset, a changed read-only upstream tree fails the run, and every
# correction is recorded.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

t_fork_fixture "${T_ROOT}"
up="${T_ROOT}/upstream"
repo="${T_ROOT}/fork"
t_put "${up}" adapters/foo/foo.go 'package foo // v2'
u2=$(t_commit "${up}" "Change foo")
t_sync_upstream "${T_ROOT}"
version=$("${T_SCRIPTS}/pin-version.sh" "${u2}")
git -C "${repo}" switch -q -c port-work
printf '# known failures\nadapters/foo/footest/exemplary/a.json\nadapters/foo/footest/exemplary/b.json # flaky\n' \
  > "${repo}/conformance/known_failures.txt"
printf '{"upstream":"%s","status":"blocked"}\n' "${T_U1}" > "${repo}/upstream-port-log.jsonl"
start=$(t_commit "${repo}" "start state")
export START_SHA="${start}" BASE_SHA="${T_U1}" TARGET_SHA="${u2}" PIN_VERSION="${version}"
export OLD_GO_ROOT="${T_ROOT}/old" PBS_GO_ROOT="${repo}/.upstream" PROBE_ROOT="${T_ROOT}/probe"
t_run_in "${repo}" "${T_SCRIPTS}/prepare-trees.sh"
t_eq "prepare-trees exits 0" 0 "${T_RC}"
ctx="${T_ROOT}/ctx"
gomod=crates/x/testdata/go/go.mod

# The session: a wanted change, a commit, edits outside the writable paths, protected edits, the
# records, probes in both trees, a tooling module at another version and a known-failures edit
# that removes one entry and adds another.
t_put "${repo}" crates/x/src/lib.rs '//! x ported'
t_put "${repo}" crates/x/src/new.rs '//! new module'
t_commit "${repo}" "agent commit" > /dev/null
t_put "${repo}" crates/x/src/more.rs '//! more'
t_put "${repo}" testdata/new.json '{}'
t_put "${repo}" .github/port/README.md 'agent edit'
t_put "${repo}" notes.txt 'agent notes'
t_put "${repo}" crates/x/zz_parity_scratch_test.go 'package x'
t_put "${repo}" Cargo.toml '[workspace] # agent edit'
t_put "${repo}" scripts/new.sh 'echo new'
t_put "${repo}" UPSTREAM_SYNC '2222222222222222222222222222222222222222'
t_put "${repo}" upstream-port-log.jsonl '{"status":"ported"}'
t_put "${repo}" clippy.toml 'too-many-lines-threshold = 100000'
t_put "${repo}" .cargo/config.toml '[source.crates-io]'
t_go_mod crates/x/testdata/go v4.9.9-0.20200101000000-aaaaaaaaaaaa > "${repo}/${gomod}"
mkdir -p "${repo}/crates/y/testdata/go"
t_go_mod crates/y/testdata/go "${T_U1_VERSION}" > "${repo}/crates/y/testdata/go/go.mod"
printf '# known failures\nadapters/foo/footest/exemplary/b.json # flaky\nadapters/foo/footest/exemplary/c.json\n' \
  > "${repo}/conformance/known_failures.txt"
t_put "${PROBE_ROOT}" adapters/foo/zz_parity_scratch_test.go 'package foo'
t_put "${PROBE_ROOT}" main.go 'package main // probe edit'

t_run_in "${repo}" env CTX="${ctx}" "${T_SCRIPTS}/check-worktree.sh"
t_eq "check-worktree exits 0" 0 "${T_RC}"
head=$(git -C "${repo}" rev-parse HEAD)
t_eq "the session's commit is undone" "${start}" "${head}"
lib=$(cat "${repo}/crates/x/src/lib.rs")
t_eq "a change from the undone commit is kept" "//! x ported" "${lib}"
kept=yes
for path in crates/x/src/new.rs crates/x/src/more.rs testdata/new.json crates/y/testdata/go/go.mod; do
  if [[ ! -f "${repo}/${path}" ]]; then
    kept="no: ${path}"
  fi
done
t_eq "new files in the writable paths are kept" yes "${kept}"
tooling=$(cat "${repo}/.github/port/README.md")
t_eq "an edit outside the writable paths is reverted" "fork tooling" "${tooling}"
leftovers=no
for path in notes.txt crates/x/zz_parity_scratch_test.go scripts/new.sh clippy.toml .cargo/config.toml \
  "${PROBE_ROOT}/adapters/foo/zz_parity_scratch_test.go"; do
  if [[ "${path}" != /* ]]; then
    path="${repo}/${path}"
  fi
  if [[ -e "${path}" ]]; then
    leftovers="yes: ${path}"
  fi
done
t_eq "untracked files outside the writable paths, probes and new protected files are removed" no "${leftovers}"
probe_main=$(cat "${PROBE_ROOT}/main.go")
t_eq "the probe tree is reset" "package main // v1" "${probe_main}"
cargo_toml=$(cat "${repo}/Cargo.toml")
t_eq "a protected edit is reverted" "[workspace]" "${cargo_toml}"
pin=$(cd "${repo}" && "${T_SCRIPTS}/upstream-pin.sh")
t_eq "the pin is the target again" "${u2}" "${pin}"
header=$(grep -c '^#' "${repo}/UPSTREAM_SYNC")
t_eq "the pin's comment lines are kept" 2 "${header}"
port_log=$(cat "${repo}/upstream-port-log.jsonl")
t_eq "an edit to the port log is reverted" "{\"upstream\":\"${T_U1}\",\"status\":\"blocked\"}" "${port_log}"
x_mod=$(grep 'require' "${repo}/${gomod}")
t_eq "a tooling module requires the new pin again" "require github.com/prebid/prebid-server/v4 ${version}" "${x_mod}"
y_mod=$(grep 'require' "${repo}/crates/y/testdata/go/go.mod")
t_eq "a new tooling module requires the new pin" "require github.com/prebid/prebid-server/v4 ${version}" "${y_mod}"
known=$(cat "${repo}/conformance/known_failures.txt")
t_eq "an added known failure is dropped and a removal is kept" \
  $'# known failures\nadapters/foo/footest/exemplary/b.json # flaky' "${known}"
staged=$(git -C "${repo}" diff --cached --name-only)
t_eq "nothing is left staged" "" "${staged}"
violations=$(cat "${ctx}/worktree-violations.txt")
t_has "the undone commit is recorded" "${violations}" "the session created commits"
t_has "the outside change is recorded" "${violations}" "changes outside the writable paths were reverted"
t_has "the protected change is named" "${violations}" "Cargo.toml"
t_has "the known-failures addition is recorded" "${violations}" "adapters/foo/footest/exemplary/c.json"

t_run_in "${repo}" env CTX="${T_ROOT}/ctx2" "${T_SCRIPTS}/check-worktree.sh"
t_eq "a clean tree passes again" 0 "${T_RC}"
t_has "a clean tree records nothing" "${T_OUT}" "0 recorded violation(s)"

# A read-only upstream tree that changed anyway fails the run.
chmod u+w "${OLD_GO_ROOT}"
t_put "${OLD_GO_ROOT}" sneaky.go 'package main'
t_run_in "${repo}" env CTX="${ctx}" "${T_SCRIPTS}/check-worktree.sh"
t_eq "a changed old tree is refused" 1 "${T_RC}"
t_has "the old tree is named" "${T_ERR}" "a session changed a read-only tree"
rm -f "${OLD_GO_ROOT}/sneaky.go"
chmod u+w "${PBS_GO_ROOT}" "${PBS_GO_ROOT}/main.go"
t_put "${PBS_GO_ROOT}" main.go 'package main // agent edit'
t_run_in "${repo}" env CTX="${ctx}" "${T_SCRIPTS}/check-worktree.sh"
t_eq "a changed new tree is refused" 1 "${T_RC}"
t_has "the new tree is named" "${T_ERR}" "${PBS_GO_ROOT} is no longer a clean checkout"
t_put "${PBS_GO_ROOT}" main.go 'package main // v1'

git -C "${repo}" switch -q -c elsewhere
t_run_in "${repo}" env CTX="${ctx}" "${T_SCRIPTS}/check-worktree.sh"
t_eq "another branch is refused" 1 "${T_RC}"
t_has "another branch is named" "${T_ERR}" "expected branch port-work"
git -C "${repo}" switch -q port-work

other=$(git -C "${repo}" commit-tree -m "unrelated" "${start}^{tree}")
git -C "${repo}" reset -q --hard "${other}"
t_run_in "${repo}" env CTX="${ctx}" "${T_SCRIPTS}/check-worktree.sh"
t_eq "a rewritten history is refused" 1 "${T_RC}"
t_has "a rewritten history is named" "${T_ERR}" "no longer descends from"

t_done
