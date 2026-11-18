#!/usr/bin/env bash
# ci-changes.sh: the compared range per event and the adapter directories a change touches
# (adapter files, bidder info and params through aliasOf, imp params, Rust adapter modules).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

repo="${T_ROOT}/repo"
t_repo "${repo}"
t_put "${repo}" adapters/foo/foo.go 'package foo'
t_put "${repo}" adapters/foo_bar/foo_bar.go 'package foo_bar'
t_put "${repo}" adapters/baz/baz.go 'package baz'
t_put "${repo}" adapters/adapterstest/test_json.go 'package adapterstest'
t_put "${repo}" static/bidder-info/foo.yaml 'endpoint: "http://foo"'
t_put "${repo}" static/bidder-info/fooalias.yaml 'aliasOf: "foo"'
t_put "${repo}" static/bidder-info/orphan.yaml 'endpoint: "http://orphan"'
t_put "${repo}" openrtb_ext/imp_baz.go 'package openrtb_ext'
t_put "${repo}" rust/crates/adapters/src/lib.rs '//! adapters'
base=$(t_commit "${repo}" "base")
t_put "${repo}" static/bidder-info/fooalias.yaml 'aliasOf: "foo" # changed'
t_put "${repo}" static/bidder-info/orphan.yaml 'endpoint: "http://orphan2"'
t_put "${repo}" openrtb_ext/imp_baz.go 'package openrtb_ext // changed'
t_put "${repo}" rust/crates/adapters/src/foobar/mod.rs '//! foo_bar'
t_put "${repo}" adapters/adapterstest/test_json.go 'package adapterstest // changed'
head=$(t_commit "${repo}" "head")

# changes NAME [VAR=VALUE...]: run ci-changes.sh; outputs in ${T_ROOT}/NAME.
changes() {
  local name=${1:?}
  shift
  : > "${T_ROOT}/${name}"
  t_run_in "${repo}" env GITHUB_OUTPUT="${T_ROOT}/${name}" REPOSITORY=test/fork "$@" "${T_SCRIPTS}/ci-changes.sh"
  if [[ "${T_RC}" -ne 0 ]]; then
    printf '%s\n' "${T_ERR}"
  fi
}

# expect NAME OUTPUT EXPECTED
expect() {
  local value
  value=$(t_output "${2:?}" "${T_ROOT}/${1:?}")
  t_eq "${1}: ${2}" "${3?}" "${value}"
}

changes pr EVENT=pull_request PR_BASE="${base}" PR_HEAD="${head}" PR_HEAD_REF=port/upstream-sync \
  PR_HEAD_REPO=test/fork BASE_REF=main
expect pr base "${base}"
expect pr head "${head}"
expect pr adapter_dirs "baz,foo,foo_bar"
expect pr port_pr true
expect pr parity_harness false

changes forkpr EVENT=pull_request PR_BASE="${base}" PR_HEAD="${head}" PR_HEAD_REF=port/upstream-sync \
  PR_HEAD_REPO=someone/fork BASE_REF=main
expect forkpr port_pr false
changes otherpr EVENT=pull_request PR_BASE="${base}" PR_HEAD="${head}" PR_HEAD_REF=feature PR_HEAD_REPO=test/fork \
  BASE_REF=parity-harness
expect otherpr port_pr false
expect otherpr parity_harness true

changes push EVENT=push REF=refs/heads/parity-harness PUSH_BEFORE="${base}"
expect push base "${base}"
expect push head "${head}"
expect push parity_harness true
changes newbranch EVENT=push REF=refs/heads/main PUSH_BEFORE=0000000000000000000000000000000000000000
expect newbranch base "${base}"
expect newbranch parity_harness false
changes mergegroup EVENT=merge_group MG_BASE="${base}" MG_HEAD="${head}"
expect mergegroup base "${base}"
expect mergegroup adapter_dirs "baz,foo,foo_bar"
changes dispatch EVENT=workflow_dispatch
expect dispatch base "${base}"
expect dispatch head "${head}"

t_put "${repo}" README.md 'nothing adapter related'
plain=$(t_commit "${repo}" "plain")
changes plain EVENT=push REF=refs/heads/main PUSH_BEFORE="${head}"
expect plain head "${plain}"
expect plain adapter_dirs ""

t_done
