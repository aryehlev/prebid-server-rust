#!/usr/bin/env bash
# sync-parity-harness.sh: a clean merge of main is pushed and closes the conflict issue; a
# conflicting merge pushes nothing, is abandoned and opens or comments the issue.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

origin="${T_ROOT}/origin.git"
work="${T_ROOT}/work"
clone="${T_ROOT}/clone"
shim="${T_ROOT}/gh"
t_gh_shim "${shim}"
git init -q --bare "${origin}"
t_repo "${work}"
git -C "${work}" switch -q -c main
t_put "${work}" main.go 'package main // v1'
t_put "${work}" README.md 'readme v1'
t_commit "${work}" "m1" > /dev/null
git -C "${work}" switch -q -c parity-harness
t_put "${work}" main.go 'package main // patched by the harness'
t_put "${work}" parity/README.md 'harness'
t_commit "${work}" "h1" > /dev/null
git -C "${work}" remote add origin "${origin}"
git -C "${work}" push -q origin main parity-harness
git clone -q "${origin}" "${clone}"
export GIT_AUTHOR_NAME="${T_BOT_NAME}" GIT_AUTHOR_EMAIL="${T_BOT_EMAIL}" GIT_COMMITTER_NAME="${T_BOT_NAME}" \
  GIT_COMMITTER_EMAIL="${T_BOT_EMAIL}"

# main_commit FILE CONTENT MESSAGE: commit on main in the work repository and push it.
main_commit() {
  git -C "${work}" switch -q main
  t_put "${work}" "${1:?}" "${2:?}"
  t_commit "${work}" "${3:?}" > /dev/null
  git -C "${work}" push -q origin main
  git -C "${clone}" fetch -q origin
}

sync() {
  t_run_in "${clone}" env RUN_URL=https://example.invalid/runs/5 "$@" "${T_SCRIPTS}/sync-parity-harness.sh"
}

harness_before=$(git -C "${origin}" rev-parse parity-harness)
sync
t_eq "up to date: exit status" 0 "${T_RC}"
t_has "up to date: nothing to do" "${T_OUT}" "already contains main"

main_commit README.md 'readme v2' "m2"
printf '[{"number": 31, "title": "parity-harness: merging main conflicts"}]\n' > "${shim}/issues_parity-harness.json"
sync DRY_RUN=true
t_eq "dry run: exit status" 0 "${T_RC}"
harness_now=$(git -C "${origin}" rev-parse parity-harness)
t_eq "dry run: nothing is pushed" "${harness_before}" "${harness_now}"

sync
t_eq "clean merge: exit status" 0 "${T_RC}"
main_sha=$(git -C "${origin}" rev-parse main)
merged=no
if git -C "${origin}" merge-base --is-ancestor "${main_sha}" parity-harness; then
  merged=yes
fi
t_eq "clean merge: pushed to parity-harness" yes "${merged}"
harness_main=$(git -C "${origin}" show parity-harness:main.go)
t_eq "clean merge: the harness patch survives" "package main // patched by the harness" "${harness_main}"
parents=$(git -C "${origin}" rev-list --parents -n 1 parity-harness | wc -w)
t_eq "clean merge: a merge commit" 3 "${parents// /}"
calls=$(cat "${shim}/calls.log")
t_has "clean merge: the open conflict issue is closed" "${calls}" "issue close 31"

main_commit main.go 'package main // v2 on main' "m3"
harness_before=$(git -C "${origin}" rev-parse parity-harness)
: > "${shim}/calls.log"
sync
t_eq "conflict: exit status" 0 "${T_RC}"
harness_now=$(git -C "${origin}" rev-parse parity-harness)
t_eq "conflict: nothing is pushed" "${harness_before}" "${harness_now}"
merge_head=no
if git -C "${clone}" rev-parse -q --verify MERGE_HEAD > /dev/null; then
  merge_head=yes
fi
t_eq "conflict: the merge is abandoned" no "${merge_head}"
calls=$(cat "${shim}/calls.log")
t_has "conflict: an issue is opened" "${calls}" "issue create --repo test/fork --title parity-harness: merging main conflicts"
body=$(cat "${shim}/body-1.md")
t_has "conflict: the issue names the path" "${body}" "main.go"
t_has "conflict: paths are fenced" "${body}" "~~~text"
: > "${shim}/calls.log"
sync
calls=$(cat "${shim}/calls.log")
t_has "conflict again: the open issue is commented" "${calls}" "issue comment 201"

git -C "${clone}" update-ref -d refs/remotes/origin/parity-harness
sync
t_has "no harness branch: nothing to do" "${T_OUT}" "does not exist"

t_run_in "${clone}" env HARNESS_BRANCH=main "${T_SCRIPTS}/sync-parity-harness.sh"
t_eq "the default branch is never the harness" 1 "${T_RC}"

t_done
