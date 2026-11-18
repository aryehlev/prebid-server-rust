#!/usr/bin/env bash
# sync-parity-harness.sh: a clean merge of main whose seams patch applies to the pinned upstream
# commit (checked out with scripts/fetch-upstream.sh) is pushed and closes the open issues; a
# merge whose patch does not apply to the new pin, or a harness without the patch, pushes nothing,
# fails and opens the seams issue; a harness that already contains main is checked the same way
# (a broken patch fails, a fixed one closes the issue, nothing is pushed); a conflicting merge
# pushes nothing, is abandoned and opens or comments the conflict issue.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

up="${T_ROOT}/upstream"
t_repo "${up}"
t_put "${up}" main.go 'package main // v1'
t_put "${up}" exchange/exchange.go 'package exchange // v1'
u1=$(t_commit "${up}" "upstream v1")
t_put "${up}" main.go 'package main // v2'
u2=$(t_commit "${up}" "upstream v2")
export PBS_UPSTREAM_URL="${up}"

# seams FROM: a patch of upstream main.go that replaces the line FROM with the harness's line.
seams() {
  printf 'diff --git a/main.go b/main.go\n--- a/main.go\n+++ b/main.go\n@@ -1 +1 @@\n-%s\n+package main // patched by the harness\n' \
    "${1:?}"
}

origin="${T_ROOT}/origin.git"
work="${T_ROOT}/work"
clone="${T_ROOT}/clone"
shim="${T_ROOT}/gh"
t_gh_shim "${shim}"
git init -q --bare "${origin}"
t_repo "${work}"
git -C "${work}" switch -q -c main
printf '# The pin.\n%s\n' "${u1}" > "${work}/UPSTREAM_SYNC"
t_put "${work}" crates/x/src/lib.rs '//! x'
t_put "${work}" README.md 'readme v1'
t_commit "${work}" "m1" > /dev/null
git -C "${work}" switch -q -c parity-harness
t_put "${work}" parity/README.md 'harness'
seams 'package main // v1' > "${work}/parity/upstream-seams.patch"
t_commit "${work}" "h1" > /dev/null
git -C "${work}" remote add origin "${origin}"
git -C "${work}" push -q origin main parity-harness
git clone -q -b main "${origin}" "${clone}"
export GIT_AUTHOR_NAME="${T_BOT_NAME}" GIT_AUTHOR_EMAIL="${T_BOT_EMAIL}" GIT_COMMITTER_NAME="${T_BOT_NAME}" \
  GIT_COMMITTER_EMAIL="${T_BOT_EMAIL}"

# on BRANCH FILE CONTENT MESSAGE: commit FILE on origin's BRANCH in the work repository and push it.
on() {
  git -C "${work}" fetch -q origin
  git -C "${work}" switch -q "${1:?}"
  git -C "${work}" reset -q --hard "origin/${1}"
  t_put "${work}" "${2:?}" "${3:?}"
  t_commit "${work}" "${4:?}" > /dev/null
  git -C "${work}" push -q origin "${1}"
  git -C "${clone}" fetch -q origin
}

# sync [VAR=VALUE...]: run the sync in the clone, with its temporary files in ${scratch}.
scratch="${T_ROOT}/scratch"
mkdir -p "${scratch}"
sync() {
  t_run_in "${clone}" env TMPDIR="${scratch}" RUN_URL=https://example.invalid/runs/5 "$@" \
    "${T_SCRIPTS}/sync-parity-harness.sh"
}

# pushed: whether origin's parity-harness contains origin's main.
pushed() {
  local main_sha
  main_sha=$(git -C "${origin}" rev-parse main)
  if git -C "${origin}" merge-base --is-ancestor "${main_sha}" parity-harness; then
    printf 'yes\n'
  else
    printf 'no\n'
  fi
}

sync
t_eq "up to date: exit status" 0 "${T_RC}"
t_has "up to date: nothing to do" "${T_OUT}" "already contains main"

on main README.md 'readme v2' "m2"
printf '[{"number": 31, "title": "parity-harness: merging main conflicts"}]\n' > "${shim}/issues_parity-harness.json"
harness_before=$(git -C "${origin}" rev-parse parity-harness)
sync DRY_RUN=true
t_eq "dry run: exit status" 0 "${T_RC}"
t_has "dry run: the merge would be pushed" "${T_OUT}" "dry run; would push"
harness_now=$(git -C "${origin}" rev-parse parity-harness)
t_eq "dry run: nothing is pushed" "${harness_before}" "${harness_now}"

sync
t_eq "clean merge: exit status" 0 "${T_RC}"
merged=$(pushed)
t_eq "clean merge: pushed to parity-harness" yes "${merged}"
patch=$(git -C "${origin}" show parity-harness:parity/upstream-seams.patch)
t_has "clean merge: the seams patch survives" "${patch}" "patched by the harness"
parents=$(git -C "${origin}" rev-list --parents -n 1 parity-harness | wc -w)
t_eq "clean merge: a merge commit" 3 "${parents// /}"
calls=$(cat "${shim}/calls.log")
t_has "clean merge: the open conflict issue is closed" "${calls}" "issue close 31"
leftover=$(find "${scratch}" -mindepth 1 -maxdepth 1 | head -n 1)
t_eq "clean merge: the upstream checkout is removed" "" "${leftover}"

# main advances the pin to a commit the patch does not fit.
on main UPSTREAM_SYNC "${u2}" "m3: advance the pin"
harness_before=$(git -C "${origin}" rev-parse parity-harness)
: > "${shim}/calls.log"
sync
t_eq "stale seams: the run fails" 1 "${T_RC}"
harness_now=$(git -C "${origin}" rev-parse parity-harness)
t_eq "stale seams: nothing is pushed" "${harness_before}" "${harness_now}"
calls=$(cat "${shim}/calls.log")
t_has "stale seams: an issue is opened" "${calls}" \
  "issue create --repo test/fork --title parity-harness: the seams patch does not apply"
body=$(cat "${shim}/body-1.md")
t_has "stale seams: the issue names the pin" "${body}" "does not apply to upstream ${u2:0:12}"
t_has "stale seams: git apply's error is fenced" "${body}" "~~~text"
t_has "stale seams: the issue names the patched file" "${body}" "main.go"
sync DRY_RUN=true
t_eq "stale seams, dry run: still fails" 1 "${T_RC}"

# The harness rebases its seams on the new pin.
patch=$(seams 'package main // v2')
on parity-harness parity/upstream-seams.patch "${patch}" "h2: seams for v2"
: > "${shim}/calls.log"
sync
t_eq "fixed seams: exit status" 0 "${T_RC}"
merged=$(pushed)
t_eq "fixed seams: pushed" yes "${merged}"
calls=$(cat "${shim}/calls.log")
t_has "fixed seams: the seams issue is closed" "${calls}" "issue close 201"

# A harness without the patch.
git -C "${work}" fetch -q origin
git -C "${work}" switch -q parity-harness
git -C "${work}" reset -q --hard origin/parity-harness
git -C "${work}" rm -q parity/upstream-seams.patch
t_commit "${work}" "h3: drop the patch" > /dev/null
git -C "${work}" push -q origin parity-harness
on main README.md 'readme v3' "m4"
sync
t_eq "no patch: the run fails" 1 "${T_RC}"
t_has "no patch: named" "${T_OUT}${T_ERR}" "has parity/ but no parity/upstream-seams.patch"
git -C "${work}" switch -q parity-harness
seams 'package main // v2' > "${work}/parity/upstream-seams.patch"
t_commit "${work}" "h4: restore the patch" > /dev/null
git -C "${work}" push -q origin parity-harness
git -C "${clone}" fetch -q origin
sync
t_eq "patch restored: exit status" 0 "${T_RC}"

# The harness already contains main: its seams are still checked against the pin.
patch=$(seams 'package main // v1')
on parity-harness parity/upstream-seams.patch "${patch}" "h4b: break the seams"
harness_before=$(git -C "${origin}" rev-parse parity-harness)
rm -f "${shim}"/body-*.md
: > "${shim}/calls.log"
sync
t_eq "up to date, broken seams: the run fails" 1 "${T_RC}"
t_has "up to date, broken seams: nothing to merge" "${T_OUT}" "already contains main"
calls=$(cat "${shim}/calls.log")
t_has "up to date, broken seams: the seams issue is opened" "${calls}" \
  "issue create --repo test/fork --title parity-harness: the seams patch does not apply"
body=$(cat "${shim}/body-1.md")
t_has "up to date, broken seams: the issue says why" "${body}" \
  "parity-harness already contains main (${harness_before:0:12}), but the parity harness does not fit"
patch=$(seams 'package main // v2')
on parity-harness parity/upstream-seams.patch "${patch}" "h4c: a person fixes the seams"
harness_before=$(git -C "${origin}" rev-parse parity-harness)
sync DRY_RUN=true
t_eq "up to date, fixed seams, dry run: exit status" 0 "${T_RC}"
t_has "up to date, fixed seams, dry run: reported" "${T_OUT}" "dry run; nothing to push, the seams fit"
: > "${shim}/calls.log"
sync
t_eq "up to date, fixed seams: exit status" 0 "${T_RC}"
t_has "up to date, fixed seams: nothing to push" "${T_OUT}" "nothing to push; parity-harness fits upstream ${u2:0:12}"
harness_now=$(git -C "${origin}" rev-parse parity-harness)
t_eq "up to date, fixed seams: nothing is pushed" "${harness_before}" "${harness_now}"
calls=$(cat "${shim}/calls.log")
t_has "up to date, fixed seams: the seams issue is closed" "${calls}" "issue close 201"

# A conflicting merge.
on main README.md 'readme v4 on main' "m5"
on parity-harness README.md 'readme of the harness' "h5"
harness_before=$(git -C "${origin}" rev-parse parity-harness)
rm -f "${shim}"/body-*.md
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
t_has "conflict: the issue names the path" "${body}" "README.md"
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
leftover=$(find "${scratch}" -mindepth 1 -maxdepth 1 | head -n 1)
t_eq "no temporary files are left" "" "${leftover}"

t_done
