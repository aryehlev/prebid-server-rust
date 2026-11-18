#!/usr/bin/env bash
# build-context.sh: write the read-only inputs of the agent sessions to ${CTX}:
#   plan.json                 the plan (from ${PLAN_DIR})
#   commits/<sha>.patch       each planned upstream commit: header, stat and full patch
#   commits/<sha>.msg         its commit message
#   ci-failures.log           the failing CI log of the pull request (repair runs)
#   path-map.md               the Go package to Rust crate map
#   shadowed-upstream-paths.txt  upstream changes to fork-owned paths (left by merge-upstream.sh)
#
# Env: CTX PLAN_DIR
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
ctx=${CTX:?}
plan_dir=${PLAN_DIR:?}
mkdir -p "${ctx}/commits"
cp "${plan_dir}/plan.json" "${ctx}/plan.json"
if [[ -f "${plan_dir}/ci-failures.log" ]]; then
  cp "${plan_dir}/ci-failures.log" "${ctx}/ci-failures.log"
fi
cp "${PORT_DIR}/path-map.md" "${ctx}/path-map.md"
if [[ ! -f "${ctx}/shadowed-upstream-paths.txt" ]]; then
  : > "${ctx}/shadowed-upstream-paths.txt"
fi

shas=$(jq -r '.commits[].sha' "${ctx}/plan.json")
while IFS= read -r sha; do
  if [[ -z "${sha}" ]]; then
    continue
  fi
  if [[ ! "${sha}" =~ ^[0-9a-f]{40}$ ]]; then
    port_die "plan.json lists an invalid commit '${sha}'"
  fi
  git show --no-color --format=fuller --stat --patch --first-parent "${sha}" > "${ctx}/commits/${sha}.patch"
  git log -1 --format=%B "${sha}" > "${ctx}/commits/${sha}.msg"
done <<< "${shas}"
count=$(find "${ctx}/commits" -name '*.patch' | wc -l)
printf 'build-context: %s upstream commit patch(es) in %s\n' "${count// /}" "${ctx}"
