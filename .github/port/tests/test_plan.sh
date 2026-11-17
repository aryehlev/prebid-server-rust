#!/usr/bin/env bash
# plan.sh: chunking of upstream commits and the pull request state machine (advance, repair,
# escalate, none, restart), with gh answered by gh-shim.sh.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

dir="${T_ROOT}/repos"
t_fork_fixture "${dir}"
up="${dir}/upstream"
fork="${dir}/fork"
shim="${T_ROOT}/gh"
t_gh_shim "${shim}"

t_put "${up}" adapters/foo/footest/exemplary/c.json '{"c": 1}'
u2=$(t_commit "${up}" "foo: add a fixture")
t_put "${up}" adapters/foo/foo.go 'package foo // v2'
u3=$(t_commit "${up}" "foo: change the adapter")
t_put "${up}" exchange/exchange.go 'package exchange // v2'
u4=$(t_commit "${up}" "exchange: core change")
t_put "${up}" README.md 'readme v2'
u5=$(t_commit "${up}" "docs: readme")
t_put "${up}" static/bidder-info/fooalias.yaml 'aliasOf: "foo"'
u6=$(t_commit "${up}" "fooalias: new alias")
t_sync_upstream "${dir}"
u1=$(git -C "${fork}" rev-parse u1)
main=$(git -C "${fork}" rev-parse main)

gh_reset() {
  find "${shim}" -maxdepth 1 -type f ! -name calls.log -delete
}

gh_state() {
  printf '%s\n' "${2?}" > "${shim}/${1:?}"
}

# plan NAME [VAR=VALUE...]: run plan.sh in the fork with default limits plus the overrides; the
# plan lands in ${T_ROOT}/plan-NAME.
plan() {
  local name=${1:?} out
  shift
  out="${T_ROOT}/plan-${name}"
  rm -rf "${out}"
  mkdir -p "${out}"
  : > "${out}/output"
  t_run_in "${fork}" env RUN_MODE=auto MAX_COMMITS=3 MAX_PR_COMMITS=10 MAX_REPAIRS=2 MAX_GO_LINES=3000 \
    OUT_DIR="${out}" GITHUB_OUTPUT="${out}/output" "$@" "${plan_script}"
  if [[ "${T_RC}" -ne 0 ]]; then
    printf '%s\n' "${T_ERR}"
  fi
}

# expect PLAN OUTPUT EXPECTED: step output OUTPUT of plan PLAN equals EXPECTED.
expect() {
  local value
  value=$(t_output "${2:?}" "${T_ROOT}/plan-${1:?}/output")
  t_eq "${1}: ${2}" "${3?}" "${value}"
}

# expect_has PLAN OUTPUT NEEDLE: step output OUTPUT of plan PLAN contains NEEDLE.
expect_has() {
  local value
  value=$(t_output "${2:?}" "${T_ROOT}/plan-${1:?}/output")
  t_has "${1}: ${2}" "${value}" "${3:?}"
}

# expect_commits PLAN EXPECTED: the planned commits of plan PLAN, space-separated.
expect_commits() {
  local value
  value=$(jq -r '[.commits[].sha] | join(" ")' "${T_ROOT}/plan-${1:?}/plan.json")
  t_eq "${1}: commits" "${2?}" "${value}"
}

plan_script="${T_SCRIPTS}/plan.sh"

plan first
t_eq "first: exit status" 0 "${T_RC}"
expect first action advance
expect_commits first "${u2} ${u3}"
expect first base_sha "${u1}"
expect first target_sha "${u3}"
expect first work_sha "${main}"
expect first adapter_dirs foo
expect first needs_claude true
expect first budget_class adapter
expect first restart false
expect first pr_number ""
classes=$(jq -r '.commits[0].classes | join(",")' "${T_ROOT}/plan-first/plan.json")
t_eq "first: a fixture-only commit" adapter-fixture "${classes}"

plan one MAX_COMMITS=1
expect_commits one "${u2}"
expect one needs_claude false

plan target TARGET_INPUT="${u2:0:10}"
expect_commits target "${u2}"
plan badtarget TARGET_INPUT=not-a-sha
t_eq "badtarget: refused" 1 "${T_RC}"
plan forktarget TARGET_INPUT="${main}"
t_eq "forktarget: a fork commit is refused" 1 "${T_RC}"
t_has "forktarget: message" "${T_ERR}" "is not on upstream"
plan zero MAX_COMMITS=0
t_eq "zero: limits must be positive" 1 "${T_RC}"
plan portmain PORT_BRANCH=main
t_eq "portmain: the port branch cannot be the default branch" 1 "${T_RC}"
plan mode RUN_MODE=sometimes
t_eq "mode: unknown run modes are refused" 1 "${T_RC}"

# advance_main TARGET: merge TARGET into main, record it as ported and publish main.
advance_main() {
  local commit
  git -C "${fork}" switch -q main
  t_run_in "${fork}" "${T_SCRIPTS}/merge-upstream.sh" "${1:?}"
  printf '%s\n' "${1}" > "${fork}/rust/UPSTREAM_SYNC"
  commit=$(t_commit "${fork}" "port: through ${1:0:12}")
  git -C "${fork}" update-ref refs/remotes/origin/main "${commit}"
}

advance_main "${u3}"
plan heavy
expect_commits heavy "${u4}"
expect heavy budget_class core
advance_main "${u4}"
plan rest
expect_commits rest "${u5} ${u6}"
expect rest adapter_dirs foo
expect rest needs_claude true
advance_main "${u6}"
plan uptodate
expect uptodate action none
expect_has uptodate reason "up to date with upstream"

git -C "${fork}" switch -q -C squashed "${main}"
printf '%s\n' "${u3}" > "${fork}/rust/UPSTREAM_SYNC"
squashed=$(t_commit "${fork}" "marker without the merge")
git -C "${fork}" update-ref refs/remotes/origin/main "${squashed}"
plan squashed
t_eq "squashed: a marker that main has not merged is refused" 1 "${T_RC}"
t_has "squashed: message" "${T_ERR}" "squash- or rebase-merged"

# Back to main at u1, with a port branch through u3 and an open pull request #7.
git -C "${fork}" switch -q main
git -C "${fork}" reset -q --hard "${main}"
git -C "${fork}" update-ref refs/remotes/origin/main "${main}"
git -C "${fork}" switch -q -C port-local "${main}"
t_run_in "${fork}" "${T_SCRIPTS}/merge-upstream.sh" "${u3}"
printf '%s\n' "${u3}" > "${fork}/rust/UPSTREAM_SYNC"
t_put "${fork}" rust/crates/x/src/lib.rs '//! ported through u3'
port=$(t_commit_as "${fork}" "${T_BOT_NAME}" "${T_BOT_EMAIL}" "port: foo: change the adapter (+1 more)")
git -C "${fork}" update-ref refs/remotes/origin/port/upstream-sync "${port}"
git -C "${fork}" switch -q main

gh_reset
gh_state prs.json '[{"number": 7}]'
gh_state checks.json '[{"name": "build", "bucket": "fail"}, {"name": "ci-ok", "bucket": "pass"}]'
plan green
expect green action advance
expect green work_sha "${port}"
expect green base_sha "${u3}"
expect_commits green "${u4}"
expect green pr_number 7
expect green port_branch_sha "${port}"
plan full MAX_PR_COMMITS=2
expect full action none
expect_has full reason "waiting for it to be merged"

gh_state checks.json '[{"name": "ci-ok", "bucket": "pending"}]'
gh_state checks_rc 8
plan pending
expect pending action none
expect_has pending reason "'pending'"
gh_state checks.json '[]'
plan nochecks
expect nochecks action none

gh_state checks.json '[{"name": "ci-ok", "bucket": "fail"}]'
gh_state checks_rc 1
gh_state runs_main.json '[{"conclusion": "success"}]'
# The newest run is on an older head, then a cancelled run and the failed one on the head.
gh_state runs_port_upstream-sync.json "[{\"databaseId\": 44, \"headSha\": \"${u1}\", \"status\": \"in_progress\"},
  {\"databaseId\": 43, \"headSha\": \"${port}\", \"status\": \"completed\", \"conclusion\": \"cancelled\"},
  {\"databaseId\": 42, \"headSha\": \"${port}\", \"status\": \"completed\", \"conclusion\": \"failure\"}]"
gh_state run_log.txt 'error: clippy failed on rust/crates/x'
plan red
expect red action repair
expect_commits red "${u2} ${u3}"
expect red target_sha "${u3}"
expect red budget_class repair
failures=$(cat "${T_ROOT}/plan-red/ci-failures.log")
t_has "red: the failing CI log is kept" "${failures}" "clippy failed"
viewed=$(grep 'run view' "${shim}/calls.log" | tail -n 1)
t_has "red: the log is the failed run's on the branch head" "${viewed}" "run view 42 "
# ci-ok of a run that the concurrency group cancelled fails while the newer run on the same head
# still runs: no verdict yet.
gh_state runs_port_upstream-sync.json "[{\"databaseId\": 45, \"headSha\": \"${port}\", \"status\": \"in_progress\"},
  {\"databaseId\": 43, \"headSha\": \"${port}\", \"status\": \"completed\", \"conclusion\": \"cancelled\"}]"
plan rerunning
expect rerunning action none
expect_has rerunning reason "CI is still running on ${port:0:12}"
gh_state runs_port_upstream-sync.json "[{\"databaseId\": 42, \"headSha\": \"${port}\", \"status\": \"completed\", \"conclusion\": \"failure\"}]"
plan forced RUN_MODE=advance
expect forced action advance
plan repairmode RUN_MODE=repair
expect repairmode action repair

gh_state runs_main.json '[{"conclusion": "failure"}]'
plan mainred
expect mainred action none
gh_state runs_main.json '[{"conclusion": "success"}]'

git -C "${fork}" switch -q -C port-local "${port}"
t_put "${fork}" rust/crates/x/src/lib.rs '//! repair one'
t_commit_as "${fork}" "${T_BOT_NAME}" "${T_BOT_EMAIL}" "port-repair: fix clippy" > /dev/null
t_put "${fork}" rust/crates/x/src/lib.rs '//! repair two'
repaired=$(t_commit_as "${fork}" "${T_BOT_NAME}" "${T_BOT_EMAIL}" "port-repair: fix clippy")
git -C "${fork}" update-ref refs/remotes/origin/port/upstream-sync "${repaired}"
git -C "${fork}" switch -q main
plan exhausted
expect exhausted action escalate
git -C "${fork}" update-ref refs/remotes/origin/port/upstream-sync "${port}"

gh_state pr_view.json '{"labels": [{"name": "upstream-port"}, {"name": "port:needs-human"}], "isDraft": false}'
plan human
expect human action none
expect_has human reason "port:needs-human"
rm -f "${shim}/pr_view.json"

gh_reset
gh_state prs.json '[{"number": 7}]'
gh_state checks.json '[{"name": "ci-ok", "bucket": "pass"}]'
plan repairgreen RUN_MODE=repair
expect repairgreen action none
gh_state prs.json '[]'
plan stale
expect stale action advance
expect stale restart true
expect stale work_sha "${main}"
expect stale port_branch_sha "${port}"

# The port branch conflicts with main.
git -C "${fork}" switch -q -C main-moved "${main}"
t_put "${fork}" rust/crates/x/src/lib.rs '//! main changed this line'
moved=$(t_commit "${fork}" "Change the crate on main")
git -C "${fork}" update-ref refs/remotes/origin/main "${moved}"
git -C "${fork}" switch -q main
gh_state prs.json '[{"number": 7}]'
plan conflict
expect conflict action advance
expect conflict restart true
expect conflict work_sha "${moved}"
git -C "${fork}" switch -q -C port-local "${port}"
t_put "${fork}" rust/crates/x/src/extra.rs '//! a person helped'
helped=$(t_commit "${fork}" "A person's fix")
git -C "${fork}" update-ref refs/remotes/origin/port/upstream-sync "${helped}"
git -C "${fork}" switch -q main
plan conflicthuman
expect conflicthuman action escalate
expect_has conflicthuman reason "1 human commit(s)"
git -C "${fork}" update-ref refs/remotes/origin/main "${main}"
git -C "${fork}" update-ref refs/remotes/origin/port/upstream-sync "${port}"

# skip-commits.txt marks a commit as skipped: it is merged but never ported.
gh_reset
tooling="${T_ROOT}/tooling"
cp -R "${T_PORT}" "${tooling}"
printf '%s skipped in this test\n' "${u3}" >> "${tooling}/skip-commits.txt"
plan_script="${tooling}/scripts/plan.sh"
git -C "${fork}" update-ref -d refs/remotes/origin/port/upstream-sync
plan skipped
expect_commits skipped "${u2} ${u3}"
skipped_classes=$(jq -r '.commits[1].classes | join(",")' "${T_ROOT}/plan-skipped/plan.json")
t_eq "skipped: classes" skipped "${skipped_classes}"
expect skipped needs_claude false

calls=$(cat "${shim}/calls.log")
t_has "gh was asked for the open pull request" "${calls}" \
  "pr list --repo test/fork --head port/upstream-sync --base main --state open"

t_done
