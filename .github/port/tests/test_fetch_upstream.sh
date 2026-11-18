#!/usr/bin/env bash
# scripts/fetch-upstream.sh under git's repository variables: a hook of a linked worktree runs
# with GIT_DIR and GIT_INDEX_FILE exported, and the command of git rebase --exec there with
# GIT_DIR, so a script started from one sees them. The script clears every variable git rev-parse
# --local-env-vars names, so the checkout is still made in --dir (or .upstream), from the
# reference, and the caller's repository keeps its HEAD, index, work tree, configuration and
# objects.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

t_fork_fixture "${T_ROOT}"
fork="${T_ROOT}/fork"
fetch="${fork}/scripts/fetch-upstream.sh"
# Offline: the commit comes from the mirror, and nothing else is reachable.
export PBS_UPSTREAM_URL="${T_ROOT}/no-such-upstream"
export PBS_UPSTREAM_REFERENCE="${UPSTREAM_GIT}"

# A linked worktree of the fork, on a branch of its own, with a change in its index.
wt="${T_ROOT}/linked"
git -C "${fork}" worktree add -q -b feature "${wt}" main
t_put "${wt}" crates/x/src/lib.rs '//! x, staged'
git -C "${wt}" add crates/x/src/lib.rs
wt_git=$(git -C "${wt}" rev-parse --absolute-git-dir)

# state REPO: what a run must not change in REPO: HEAD, the branch, the index and work tree, the
# remotes, the shallow and the alternates files, and whether it has the upstream commit.
state() {
  local repo=${1:?} has=no head branch status remotes
  if git -C "${repo}" cat-file -e "${T_U1}^{commit}" 2> /dev/null; then
    has=yes
  fi
  head=$(git -C "${repo}" rev-parse HEAD)
  branch=$(git -C "${repo}" symbolic-ref --short HEAD)
  status=$(git -C "${repo}" status --porcelain)
  remotes=$(git -C "${repo}" remote)
  printf 'head=%s branch=%s status=[%s] remotes=[%s] upstream-commit=%s\n' \
    "${head}" "${branch}" "${status//$'\n'/;}" "${remotes//$'\n'/ }" "${has}"
}

fork_before=$(state "${fork}")
wt_before=$(state "${wt}")
t_has "the fixture: the fork does not have the upstream commit" "${fork_before}" "upstream-commit=no"

# As a hook of the linked worktree: GIT_DIR, GIT_INDEX_FILE and GIT_WORK_TREE point at it.
t_run_in "${wt}" env GIT_DIR="${wt_git}" GIT_INDEX_FILE="${wt_git}/index" GIT_WORK_TREE="${wt}" \
  "${fetch}" --dir "${T_ROOT}/elsewhere"
t_eq "hook: exit status" 0 "${T_RC}"
if [[ "${T_RC}" -ne 0 ]]; then
  printf '%s\n' "${T_ERR}"
fi
last=$(tail -n 1 <<< "${T_OUT}")
t_eq "hook: the last line is the checkout" "${T_ROOT}/elsewhere" "${last}"
head=$(git -C "${T_ROOT}/elsewhere" rev-parse HEAD 2> /dev/null || true)
t_eq "hook: --dir is a checkout of the pin" "${T_U1}" "${head}"
top=$(git -C "${T_ROOT}/elsewhere" rev-parse --show-toplevel 2> /dev/null || true)
t_eq "hook: --dir is the top of its own repository" "${T_ROOT}/elsewhere" "${top}"
wt_after=$(state "${wt}")
t_eq "hook: the linked worktree is unchanged" "${wt_before}" "${wt_after}"
fork_after=$(state "${fork}")
t_eq "hook: the main worktree is unchanged" "${fork_before}" "${fork_after}"

# As the command of git rebase --exec in the linked worktree: GIT_DIR only, and the default
# directory, .upstream at the top of the repository the script belongs to.
t_run_in "${wt}" env GIT_DIR="${wt_git}" "${wt}/scripts/fetch-upstream.sh"
t_eq "rebase --exec: exit status" 0 "${T_RC}"
head=$(git -C "${wt}/.upstream" rev-parse HEAD 2> /dev/null || true)
t_eq "rebase --exec: .upstream is a checkout of the pin" "${T_U1}" "${head}"
top=$(git -C "${wt}/.upstream" rev-parse --show-toplevel 2> /dev/null || true)
t_eq "rebase --exec: .upstream is the top of its own repository" "${wt}/.upstream" "${top}"
wt_after=$(state "${wt}")
t_eq "rebase --exec: the linked worktree is unchanged (.upstream is ignored)" "${wt_before}" "${wt_after}"

# A second run under the variables finds the checkout and changes nothing.
t_run_in "${wt}" env GIT_DIR="${wt_git}" GIT_INDEX_FILE="${wt_git}/index" "${fetch}" --dir "${T_ROOT}/elsewhere"
t_eq "again: exit status" 0 "${T_RC}"
t_lacks "again: nothing is fetched" "${T_ERR}" "fetching"
fork_after=$(state "${fork}")
wt_after=$(state "${wt}")
t_eq "again: both worktrees are unchanged" "${fork_before}${wt_before}" "${fork_after}${wt_after}"

# The configuration passed down to git commands (git -c) still reaches the script's own: a
# fetch that must go to the network fails at the configured URL rewrite, not elsewhere.
t_run_in "${wt}" env GIT_DIR="${wt_git}" GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=url."${T_ROOT}/rewritten".insteadOf \
  GIT_CONFIG_VALUE_0="${T_ROOT}/no-such-upstream" PBS_UPSTREAM_REFERENCE= "${fetch}" --dir "${T_ROOT}/third"
t_fails "config: a commit only the network has cannot be fetched"
t_has "config: the URL rewrite passed down applies" "${T_ERR}" "${T_ROOT}/rewritten"

t_done
