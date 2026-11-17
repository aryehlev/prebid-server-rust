#!/usr/bin/env bash
# check-worktree.sh: after an agent session, put the work tree back inside the rules before
# anything is checked or committed. Every correction is recorded in
# ${CTX}/worktree-violations.txt, which the pull request report shows.
#   1. The work branch must still be checked out (exit 1 otherwise).
#   2. Commits made by the session are undone with `git reset --soft MERGED_SHA`: their changes
#      stay in the work tree, but no commit object the agent wrote can be published.
#   3. Go probe files named zz_parity_scratch_test.go are deleted.
#   4. Changes outside rust/ are reverted, and untracked files there are removed.
#   5. Changes to the protected rust/ paths and to the port records (lib.sh port_protected_paths
#      and port_record_paths) are reverted; commit.sh writes the records after the last session.
#   6. Lines added to rust/conformance/known_failures.txt are dropped (removals are kept).
# Env: MERGED_SHA (the commit the session started from), CTX, WORK_BRANCH (default port-work).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
merged=$(git rev-parse --verify "${MERGED_SHA:?}^{commit}")
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
if [[ "${head}" != "${merged}" ]]; then
  if git merge-base --is-ancestor "${merged}" HEAD; then
    made=$(git log --format='%h %s' "${merged}..HEAD" | head -n 20)
    record "the session created commits; they were undone and only their changes kept: ${made//$'\n'/; }"
    git reset -q --soft "${merged}"
  else
    port_die "check-worktree: HEAD ${head:0:12} no longer descends from ${merged:0:12}"
  fi
fi
git reset -q

probes=$(find . -path ./.git -prune -o -path ./rust/target -prune -o -type f -name zz_parity_scratch_test.go -print)
while IFS= read -r probe; do
  if [[ -n "${probe}" ]]; then
    rm -f -- "${probe}"
  fi
done <<< "${probes}"

outside_listing=$(port_outside_rust_spec)
mapfile -t outside <<< "${outside_listing}"
changed=$(git status --porcelain --untracked-files=all -- "${outside[@]}")
if [[ -n "${changed}" ]]; then
  summary=$(head -n 20 <<< "${changed}" | tr '\n' ';')
  record "changes outside rust/ were reverted: ${summary}"
  git checkout -q HEAD -- "${outside[@]}"
  git clean -f -d -q -- "${outside[@]}"
fi

protected_listing=$(port_protected_paths)
record_listing=$(port_record_paths)
protected_listing=$(printf '%s\n%s\n' "${protected_listing}" "${record_listing}")
mapfile -t protected <<< "${protected_listing}"
changed=$(git status --porcelain --untracked-files=all -- "${protected[@]}")
if [[ -n "${changed}" ]]; then
  summary=$(head -n 20 <<< "${changed}" | tr '\n' ';')
  record "changes to protected paths were reverted: ${summary}"
  for path in "${protected[@]}"; do
    if git cat-file -e "HEAD:${path}" 2> /dev/null; then
      git checkout -q HEAD -- "${path}"
    fi
    if [[ -e "${path}" ]]; then
      git clean -f -d -q -- "${path}"
    fi
  done
fi

known=rust/conformance/known_failures.txt
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
count=$(wc -l < "${violations}")
printf 'check-worktree: %s recorded violation(s)\n' "${count// /}"
