#!/usr/bin/env bash
# check-worktree.sh: after an agent session, put the work tree and the upstream trees back inside
# the rules before anything is checked or committed. Every correction is recorded in
# ${CTX}/worktree-violations.txt, which the pull request report shows.
#   1. The work branch must still be checked out (exit 1 otherwise).
#   2. Commits made by the session are undone with `git reset --soft START_SHA`: their changes
#      stay in the work tree, but no commit object the agent wrote can be published.
#   3. Go probe files named zz_parity_scratch_test.go are deleted from the work tree.
#   4. Changes outside the writable paths (lib.sh port_writable_paths) are reverted, and untracked
#      files there are removed. That covers the protected paths, the port records, the workflows,
#      the scripts and everything else a session may not change.
#   5. Lines added to conformance/known_failures.txt are dropped (removals are kept).
#   6. The pin is applied again with advance-pin.sh TARGET_SHA PIN_VERSION: step 4 reverted
#      UPSTREAM_SYNC, and a session may have changed the upstream version a Go tooling module
#      requires.
#   7. The upstream trees: OLD_GO_ROOT and PBS_GO_ROOT must still be clean checkouts of BASE_SHA
#      and TARGET_SHA (exit 1 otherwise: they are read-only, so a change means a session got
#      around that), and PROBE_ROOT, where sessions may add Go probes, is reset to TARGET_SHA and
#      cleaned.
# Env: START_SHA (the commit the sessions started from), BASE_SHA TARGET_SHA PIN_VERSION,
#      OLD_GO_ROOT PBS_GO_ROOT PROBE_ROOT, CTX, WORK_BRANCH (default port-work).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
start=$(git rev-parse --verify "${START_SHA:?}^{commit}")
base_sha=${BASE_SHA:?}
target_sha=${TARGET_SHA:?}
pin_version=${PIN_VERSION:?}
old_root=${OLD_GO_ROOT:?}
new_root=${PBS_GO_ROOT:?}
probe_root=${PROBE_ROOT:?}
ctx=${CTX:?}
work_branch=${WORK_BRANCH:-port-work}
violations="${ctx}/worktree-violations.txt"
mkdir -p "${ctx}"
touch "${violations}"

record() {
  printf '%s\n' "$*" >> "${violations}"
  port_warning "check-worktree: $*"
}

branch=$(git symbolic-ref --quiet --short HEAD || true)
if [[ "${branch}" != "${work_branch}" ]]; then
  port_die "check-worktree: expected branch ${work_branch} to be checked out, found '${branch:-detached HEAD}'"
fi

head=$(git rev-parse HEAD)
if [[ "${head}" != "${start}" ]]; then
  if git merge-base --is-ancestor "${start}" HEAD; then
    made=$(git log --format='%h %s' "${start}..HEAD" | head -n 20)
    record "the session created commits; they were undone and only their changes kept: ${made//$'\n'/; }"
    git reset -q --soft "${start}"
  else
    port_die "check-worktree: HEAD ${head:0:12} no longer descends from ${start:0:12}"
  fi
fi
git reset -q

probes=$(find . \( -path ./.git -o -path ./target -o -path ./.upstream \) -prune -o -type f -name zz_parity_scratch_test.go -print)
while IFS= read -r probe; do
  if [[ -n "${probe}" ]]; then
    rm -f -- "${probe}"
  fi
done <<< "${probes}"

outside_listing=$(port_outside_writable_spec)
mapfile -t outside <<< "${outside_listing}"
# UPSTREAM_SYNC pinning TARGET_SHA is the state advance-pin.sh made before the sessions, not a
# change of theirs.
head_sync=$(git show HEAD:UPSTREAM_SYNC 2> /dev/null || true)
expected_sync=$(port_sync_text "${target_sha}" <<< "${head_sync}")
current_sync=$(cat UPSTREAM_SYNC 2> /dev/null || true)
status_spec=("${outside[@]}")
if [[ "${current_sync}" == "${expected_sync}" ]]; then
  status_spec+=(':(top,exclude)UPSTREAM_SYNC')
fi
changed=$(git status --porcelain --untracked-files=all -- "${status_spec[@]}")
if [[ -n "${changed}" ]]; then
  summary=$(head -n 20 <<< "${changed}" | tr '\n' ';')
  record "changes outside the writable paths were reverted: ${summary}"
fi
git checkout -q HEAD -- "${outside[@]}"
git clean -f -d -q -- "${outside[@]}"

known=conformance/known_failures.txt
if [[ -f "${known}" ]] && git cat-file -e "HEAD:${known}" 2> /dev/null; then
  scratch=$(mktemp -d)
  git show "HEAD:${known}" > "${scratch}/before"
  port_list "${scratch}/before" | sort -u > "${scratch}/before.sorted"
  port_list "${known}" | sort -u > "${scratch}/after.sorted"
  added=$(comm -13 "${scratch}/before.sorted" "${scratch}/after.sorted")
  after_entries=$(cat "${scratch}/after.sorted")
  before=$(cat "${scratch}/before")
  rm -rf -- "${scratch}"
  if [[ -n "${added}" ]]; then
    record "entries added to ${known} were dropped: ${added//$'\n'/, }"
    PORT_KEEP=${after_entries} perl -e '
      my %keep = map { $_ => 1 } split /\n/, $ENV{PORT_KEEP};
      while (my $line = <STDIN>) {
        my $entry = $line;
        $entry =~ s/\s*#.*$//;
        $entry =~ s/^\s+|\s+$//g;
        print $line if $entry eq "" || $keep{$entry};
      }' <<< "${before}" > "${known}"
  fi
fi

"${PORT_SCRIPTS}/advance-pin.sh" "${target_sha}" "${pin_version}" > /dev/null

# tree_state DIR: "<HEAD> <number of changed, untracked or ignored paths>" of the checkout DIR.
tree_state() {
  local head_sha status lines=0
  head_sha=$(git -C "${1:?}" rev-parse --verify --quiet HEAD || true)
  status=$(git -C "${1}" status --porcelain --untracked-files=all --ignored 2> /dev/null || printf 'unreadable\n')
  if [[ -n "${status}" ]]; then
    lines=$(grep -c '' <<< "${status}")
  fi
  printf '%s %s\n' "${head_sha:-none}" "${lines}"
}
old_state=$(tree_state "${old_root}")
if [[ "${old_state}" != "${base_sha} 0" ]]; then
  port_die "check-worktree: the old upstream tree ${old_root} is no longer a clean checkout of ${base_sha:0:12} (${old_state}); a session changed a read-only tree"
fi
new_state=$(tree_state "${new_root}")
if [[ "${new_state}" != "${target_sha} 0" ]]; then
  port_die "check-worktree: the upstream tree ${new_root} is no longer a clean checkout of ${target_sha:0:12} (${new_state}); a session changed a read-only tree"
fi
git -C "${probe_root}" reset -q --hard "${target_sha}"
git -C "${probe_root}" clean -q -f -d -x
count=$(wc -l < "${violations}")
printf 'check-worktree: %s recorded violation(s)\n' "${count// /}"
