#!/usr/bin/env bash
# report-failure.sh: record a failed or cancelled port job (a crash or a timeout, not failing gates,
# which the pull request shows) on the open "Upstream port run failed" issue, creating it if needed.
# Env: GH_TOKEN GITHUB_REPOSITORY RUN_URL, PORT_RESULT (the port job's result), REASON (the plan).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

repo=${GITHUB_REPOSITORY:?}
: "${GH_TOKEN:?}"
tmp=$(mktemp -d)
trap 'rm -rf -- "${tmp}"' EXIT

gh label create port:run-failed --repo "${repo}" --color d73a4a \
  --description "An upstream-port workflow run failed" --force > /dev/null
printf 'The port job ended with **%s** while planned as: %s\n\nRun: %s\n' \
  "${PORT_RESULT:-unknown}" "${REASON:-unknown}" "${RUN_URL:-unknown}" | port_sanitize > "${tmp}/body.md"
issue=$(gh issue list --repo "${repo}" --label port:run-failed --state open --json number --jq '.[0].number // empty')
if [[ -n "${issue}" ]]; then
  gh issue comment "${issue}" --repo "${repo}" --body-file "${tmp}/body.md" > /dev/null
  printf 'report-failure: commented on issue #%s\n' "${issue}"
else
  gh issue create --repo "${repo}" --title "Upstream port run failed" --label port:run-failed \
    --body-file "${tmp}/body.md" > /dev/null
  printf 'report-failure: opened an issue\n'
fi
