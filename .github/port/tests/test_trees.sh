#!/usr/bin/env bash
# prepare-trees.sh: the old pin, the new pin (which must be .upstream in the work tree, read-only)
# and the probe tree are clean checkouts made with scripts/fetch-upstream.sh from the upstream
# mirror, and a second run changes nothing.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

t_fork_fixture "${T_ROOT}"
up="${T_ROOT}/upstream"
fork="${T_ROOT}/fork"
t_put "${up}" exchange/exchange.go 'package exchange // v2'
u2=$(t_commit "${up}" "exchange: v2")
t_sync_upstream "${T_ROOT}"
# Without network: a commit the mirror lacks would be fetched from PBS_UPSTREAM_URL.
export PBS_UPSTREAM_URL="${T_ROOT}/no-such-upstream"

# prepare [VAR=VALUE...]: run prepare-trees.sh in the fork for u1..u2.
prepare() {
  t_run_in "${fork}" env BASE_SHA="${T_U1}" TARGET_SHA="${u2}" OLD_GO_ROOT="${T_ROOT}/old" \
    PBS_GO_ROOT="${fork}/.upstream" PROBE_ROOT="${T_ROOT}/probe" "$@" "${T_SCRIPTS}/prepare-trees.sh"
}

# head DIR: the checked-out commit of DIR.
head_of() {
  git -C "${1:?}" rev-parse HEAD
}

# writable DIR: yes when DIR's top directory is writable.
writable() {
  if [[ -w "${1:?}" ]]; then
    printf 'yes\n'
  else
    printf 'no\n'
  fi
}

prepare
t_eq "prepare: exit status" 0 "${T_RC}"
old_head=$(head_of "${T_ROOT}/old")
t_eq "prepare: the old tree is the old pin" "${T_U1}" "${old_head}"
new_head=$(head_of "${fork}/.upstream")
t_eq "prepare: .upstream is the new pin" "${u2}" "${new_head}"
probe_head=$(head_of "${T_ROOT}/probe")
t_eq "prepare: the probe tree is the new pin" "${u2}" "${probe_head}"
exchange=$(cat "${fork}/.upstream/exchange/exchange.go")
t_eq "prepare: the new tree has the new code" "package exchange // v2" "${exchange}"
old_state=$(writable "${T_ROOT}/old")
new_state=$(writable "${fork}/.upstream")
deep_state=$(writable "${fork}/.upstream/exchange")
t_eq "prepare: the old and new trees are read-only" "no no no" "${old_state} ${new_state} ${deep_state}"
state=$(writable "${T_ROOT}/probe")
t_eq "prepare: the probe tree is writable" yes "${state}"
ignored=$(git -C "${fork}" status --porcelain)
t_eq "prepare: the work tree stays clean (.upstream is ignored)" "" "${ignored}"

prepare
t_eq "prepare again: nothing to do" 0 "${T_RC}"

prepare PBS_GO_ROOT="${T_ROOT}/new"
t_eq "elsewhere: PBS_GO_ROOT must be .upstream" 1 "${T_RC}"
t_has "elsewhere: explained" "${T_ERR}" "PBS_GO_ROOT must be .upstream at the top of the work tree"
prepare PROBE_ROOT=probe
t_eq "relative: refused" 1 "${T_RC}"
prepare TARGET_SHA=HEAD
t_eq "symbolic: refused" 1 "${T_RC}"
prepare BASE_SHA=0000000000000000000000000000000000000000
t_fails "a commit neither the mirror nor upstream has fails"

t_done
