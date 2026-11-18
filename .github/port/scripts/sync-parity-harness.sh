#!/usr/bin/env bash
# sync-parity-harness.sh: merge the default branch into the parity-harness branch, check that the
# harness still fits the upstream commit the merge pins, and push it.
#   1. The merge: a merge commit of the default branch into the harness branch. On a conflict the
#      merge is abandoned, nothing is pushed, and the open "parity-harness: merging main conflicts"
#      issue is commented (or opened) with the conflicting paths. When the harness branch already
#      contains the default branch there is nothing to merge, and the harness head is checked.
#   2. The seams: when the merged tree has parity/, its parity/upstream-seams.patch must exist and
#      apply (git apply --check) to a checkout of the upstream commit the merged UPSTREAM_SYNC
#      pins, made with scripts/fetch-upstream.sh of the default branch. Otherwise nothing is
#      pushed, the open "parity-harness: the seams patch does not apply" issue is commented (or
#      opened), and the run fails.
#   3. The push. A sync that finds the harness fitting, after a merge or with nothing to merge (a
#      person merged and fixed it), closes the open issues.
#
# Env: BASE_BRANCH HARNESS_BRANCH GITHUB_REPOSITORY GH_TOKEN RUN_URL, the commit identity in
#      GIT_AUTHOR_* / GIT_COMMITTER_*, DRY_RUN=true to only report what would happen, and
#      PBS_UPSTREAM_URL / PBS_UPSTREAM_REFERENCE for the upstream checkout (scripts/fetch-upstream.sh).
# Expects refs/remotes/origin/<BASE_BRANCH> and refs/remotes/origin/<HARNESS_BRANCH> to be fetched.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
base_branch=${BASE_BRANCH:?}
harness_branch=${HARNESS_BRANCH:?}
repo=${GITHUB_REPOSITORY:?}
dry_run=${DRY_RUN:-false}
main="refs/remotes/origin/${base_branch}"
harness="refs/remotes/origin/${harness_branch}"
conflict_title="parity-harness: merging ${base_branch} conflicts"
seams_title="parity-harness: the seams patch does not apply"
case "${harness_branch}" in
  main | master | "${base_branch}") port_die "sync-parity-harness: refusing to push to '${harness_branch}'" ;;
  *) ;;
esac

tmp=$(mktemp -d)
cleanup() {
  if [[ -d "${tmp}/upstream" ]]; then
    chmod -R u+w "${tmp}/upstream" 2> /dev/null || true
  fi
  rm -rf -- "${tmp}"
}
trap cleanup EXIT
# The trusted fetch script, taken before the harness branch is checked out.
cp "${PORT_TOP}/scripts/fetch-upstream.sh" "${tmp}/fetch-upstream.sh"

# open_issue TITLE: the number of the open parity-harness issue titled TITLE, if any.
open_issue() {
  local title=${1:?}
  gh issue list --repo "${repo}" --label parity-harness --state open --search "in:title \"${title}\"" \
    --json number,title --jq "[.[] | select(.title == \"${title}\")][0].number // empty"
}

# report TITLE BODY_FILE: comment on the open issue titled TITLE, or open one.
report() {
  local title=${1:?} body=${2:?} issue
  gh label create parity-harness --repo "${repo}" --color 1d76db \
    --description "The parity-harness branch and its sync with main" --force > /dev/null
  issue=$(open_issue "${title}")
  if [[ -n "${issue}" ]]; then
    gh issue comment "${issue}" --repo "${repo}" --body-file "${body}" > /dev/null
  else
    gh issue create --repo "${repo}" --title "${title}" --label parity-harness --body-file "${body}" > /dev/null
  fi
}

# close_issue TITLE TEXT: close the open issue titled TITLE, if any, with TEXT.
close_issue() {
  local title=${1:?} text=${2:?} issue
  issue=$(open_issue "${title}")
  if [[ -n "${issue}" ]]; then
    gh issue close "${issue}" --repo "${repo}" --comment "${text}" > /dev/null
  fi
}

if ! git rev-parse -q --verify "${harness}" > /dev/null; then
  printf 'sync-parity-harness: %s does not exist; nothing to do\n' "${harness_branch}"
  exit 0
fi

up_to_date=false
if git merge-base --is-ancestor "${main}" "${harness}"; then
  up_to_date=true
  merged=$(git rev-parse "${harness}")
  printf 'sync-parity-harness: %s already contains %s; checking its seams\n' "${harness_branch}" "${base_branch}"
else
  git switch -q -C "${harness_branch}" "${harness}"
  if ! git merge --no-ff --no-verify -q -m "Merge ${base_branch} into ${harness_branch}" "${main}" > "${tmp}/merge.log" 2>&1; then
    conflicts=$(git diff --name-only --diff-filter=U | head -n 100)
    if [[ -z "${conflicts}" ]]; then
      conflicts=$(tail -n 20 "${tmp}/merge.log")
    fi
    if git rev-parse -q --verify MERGE_HEAD > /dev/null; then
      git merge --abort
    fi
    main_sha=$(git rev-parse "${main}")
    harness_sha=$(git rev-parse "${harness}")
    {
      printf 'Merging %s (%s) into %s (%s) conflicts, so the parity harness no longer tracks %s.\n\n' \
        "${base_branch}" "${main_sha:0:12}" "${harness_branch}" "${harness_sha:0:12}" "${base_branch}"
      printf 'Conflicting paths:\n\n~~~text\n%s\n~~~\n\n' "${conflicts}"
      printf 'Resolve it by merging %s into %s by hand (a merge commit, not a rebase).\n\nRun: %s\n' \
        "${base_branch}" "${harness_branch}" "${RUN_URL:-}"
    } > "${tmp}/issue.md"
    port_warning "sync-parity-harness: merging ${base_branch} into ${harness_branch} conflicts"
    cat "${tmp}/issue.md"
    if [[ "${dry_run}" == true ]]; then
      exit 0
    fi
    report "${conflict_title}" "${tmp}/issue.md"
    exit 0
  fi
  merged=$(git rev-parse HEAD)
fi

# The seams patch must fit the upstream commit the merge pins. It is read from the commit, so the
# check is the same with or without a merge.
pin=$(port_marker "${merged}")
seams_problem=""
has_parity=false
if git cat-file -e "${merged}:parity" 2> /dev/null; then
  has_parity=true
  if [[ -z "${pin}" ]]; then
    seams_problem="UPSTREAM_SYNC of ${merged:0:12} does not pin an upstream commit."
  elif ! git cat-file -e "${merged}:parity/upstream-seams.patch" 2> /dev/null; then
    seams_problem="${harness_branch} has parity/ but no parity/upstream-seams.patch."
  else
    git show "${merged}:parity/upstream-seams.patch" > "${tmp}/upstream-seams.patch"
    "${tmp}/fetch-upstream.sh" --sha "${pin}" --dir "${tmp}/upstream" > /dev/null
    if ! git -C "${tmp}/upstream" apply --check "${tmp}/upstream-seams.patch" > "${tmp}/apply.log" 2>&1; then
      apply_log=$(head -n 60 "${tmp}/apply.log")
      seams_problem=$(printf 'parity/upstream-seams.patch does not apply to upstream %s, which %s pins:\n\n~~~text\n%s\n~~~' \
        "${pin:0:12}" "${base_branch}" "${apply_log}")
    fi
  fi
fi
if [[ -n "${seams_problem}" ]]; then
  {
    if [[ "${up_to_date}" == true ]]; then
      printf '%s already contains %s (%s), but the parity harness does not fit the upstream commit it pins.\n\n' \
        "${harness_branch}" "${base_branch}" "${merged:0:12}"
    else
      printf 'Merging %s into %s merges cleanly, but the parity harness does not fit it, so nothing was pushed.\n\n' \
        "${base_branch}" "${harness_branch}"
    fi
    printf '%s\n\n' "${seams_problem}"
    printf 'Rebase the seams on the new pin (.github/port/scripts/ci-parity.sh assembles the tree), commit the\n'
    printf 'new patch on %s together with a merge of %s, and push it. The next sync checks it again and\n' \
      "${harness_branch}" "${base_branch}"
    printf 'closes this issue.\n\nRun: %s\n' "${RUN_URL:-}"
  } > "${tmp}/seams.md"
  port_error "sync-parity-harness: the seams patch does not fit upstream ${pin:0:12}; nothing pushed"
  cat "${tmp}/seams.md"
  if [[ "${dry_run}" != true ]]; then
    report "${seams_title}" "${tmp}/seams.md"
  fi
  exit 1
fi

if [[ "${dry_run}" == true ]]; then
  if [[ "${up_to_date}" == true ]]; then
    printf 'sync-parity-harness: dry run; nothing to push, the seams fit\n'
  else
    printf 'sync-parity-harness: dry run; would push %s to %s\n' "${merged:0:12}" "${harness_branch}"
  fi
  exit 0
fi
if [[ "${up_to_date}" == true ]]; then
  printf 'sync-parity-harness: nothing to push; %s fits upstream %s\n' "${harness_branch}" "${pin:0:12}"
else
  git push -q origin "HEAD:refs/heads/${harness_branch}"
  printf 'sync-parity-harness: merged %s into %s (%s)\n' "${base_branch}" "${harness_branch}" "${merged:0:12}"
fi
close_text=$(printf '%s contains %s again (%s).\n\nRun: %s\n' \
  "${harness_branch}" "${base_branch}" "${merged:0:12}" "${RUN_URL:-}")
close_issue "${conflict_title}" "${close_text}"
if [[ "${has_parity}" == true ]]; then
  close_text=$(printf 'The seams patch applies to upstream %s again (%s).\n\nRun: %s\n' \
    "${pin:0:12}" "${merged:0:12}" "${RUN_URL:-}")
  close_issue "${seams_title}" "${close_text}"
fi
