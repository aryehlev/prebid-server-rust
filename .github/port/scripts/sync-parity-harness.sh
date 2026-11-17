#!/usr/bin/env bash
# sync-parity-harness.sh: merge the default branch into the parity-harness branch and push it.
# The parity harness patches upstream Go on purpose, so a merge can conflict; on a conflict the
# merge is abandoned, nothing is pushed, and the open "parity-harness: merging main conflicts" issue
# is commented (or opened) with the conflicting paths. A later clean merge closes that issue.
#
# Env: BASE_BRANCH HARNESS_BRANCH GITHUB_REPOSITORY GH_TOKEN RUN_URL, the commit identity in
#      GIT_AUTHOR_* / GIT_COMMITTER_*, and DRY_RUN=true to only report what would happen.
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
title="parity-harness: merging ${base_branch} conflicts"
case "${harness_branch}" in
  main | master | "${base_branch}") port_die "sync-parity-harness: refusing to push to '${harness_branch}'" ;;
  *) ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf -- "${tmp}"' EXIT

open_issue() {
  gh issue list --repo "${repo}" --label parity-harness --state open --search "in:title \"${title}\"" \
    --json number,title --jq "[.[] | select(.title == \"${title}\")][0].number // empty"
}

if ! git rev-parse -q --verify "${harness}" > /dev/null; then
  printf 'sync-parity-harness: %s does not exist; nothing to do\n' "${harness_branch}"
  exit 0
fi
if git merge-base --is-ancestor "${main}" "${harness}"; then
  printf 'sync-parity-harness: %s already contains %s\n' "${harness_branch}" "${base_branch}"
  exit 0
fi

git switch -q -C "${harness_branch}" "${harness}"
if git merge --no-ff --no-verify -q -m "Merge ${base_branch} into ${harness_branch}" "${main}" > "${tmp}/merge.log" 2>&1; then
  merged=$(git rev-parse HEAD)
  if [[ "${dry_run}" == true ]]; then
    printf 'sync-parity-harness: dry run; would push %s to %s\n' "${merged:0:12}" "${harness_branch}"
    exit 0
  fi
  git push -q origin "HEAD:refs/heads/${harness_branch}"
  printf 'sync-parity-harness: merged %s into %s (%s)\n' "${base_branch}" "${harness_branch}" "${merged:0:12}"
  issue=$(open_issue)
  if [[ -n "${issue}" ]]; then
    printf '%s merged cleanly into %s again (%s).\n\nRun: %s\n' "${base_branch}" "${harness_branch}" \
      "${merged:0:12}" "${RUN_URL:-}" > "${tmp}/close.md"
    close_text=$(cat "${tmp}/close.md")
    gh issue close "${issue}" --repo "${repo}" --comment "${close_text}" > /dev/null
  fi
  exit 0
fi

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
gh label create parity-harness --repo "${repo}" --color 1d76db \
  --description "The parity-harness branch and its sync with main" --force > /dev/null
issue=$(open_issue)
if [[ -n "${issue}" ]]; then
  gh issue comment "${issue}" --repo "${repo}" --body-file "${tmp}/issue.md" > /dev/null
else
  gh issue create --repo "${repo}" --title "${title}" --label parity-harness --body-file "${tmp}/issue.md" > /dev/null
fi
