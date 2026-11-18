#!/usr/bin/env bash
# build-context.sh: write the read-only inputs of the agent sessions to ${CTX}:
#   plan.json                 the plan (from ${PLAN_DIR})
#   commits/<sha>.patch       each planned upstream commit: header, stat and full patch
#   commits/<sha>.msg         its commit message
#   upstream.diff             the whole Go change of the run: upstream BASE_SHA..TARGET_SHA
#   ci-failures.log           the failing CI log of the pull request (repair runs)
#   path-map.md               the Go package to Rust crate map
# The commits are read from the upstream mirror (UPSTREAM_GIT); the trees themselves are the
# checkouts prepare-trees.sh makes (OLD_GO_ROOT, PBS_GO_ROOT, PROBE_ROOT).
#
# Env: CTX PLAN_DIR UPSTREAM_GIT BASE_SHA TARGET_SHA
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
ctx=${CTX:?}
plan_dir=${PLAN_DIR:?}
base=${BASE_SHA:?}
target=${TARGET_SHA:?}
: "${UPSTREAM_GIT:?}"
for sha in "${base}" "${target}"; do
  if [[ ! "${sha}" =~ ^[0-9a-f]{40}$ ]]; then
    port_die "build-context: '${sha}' is not a full commit"
  fi
done
mkdir -p "${ctx}/commits"
cp "${plan_dir}/plan.json" "${ctx}/plan.json"
if [[ -f "${plan_dir}/ci-failures.log" ]]; then
  cp "${plan_dir}/ci-failures.log" "${ctx}/ci-failures.log"
fi
cp "${PORT_DIR}/path-map.md" "${ctx}/path-map.md"
port_up diff --no-color --stat --patch "${base}" "${target}" > "${ctx}/upstream.diff"

shas=$(jq -r '.commits[].sha' "${ctx}/plan.json")
while IFS= read -r sha; do
  if [[ -z "${sha}" ]]; then
    continue
  fi
  if [[ ! "${sha}" =~ ^[0-9a-f]{40}$ ]]; then
    port_die "plan.json lists an invalid commit '${sha}'"
  fi
  port_up show --no-color --format=fuller --stat --patch --first-parent "${sha}" > "${ctx}/commits/${sha}.patch"
  port_up log -1 --format=%B "${sha}" > "${ctx}/commits/${sha}.msg"
done <<< "${shas}"
count=$(find "${ctx}/commits" -name '*.patch' | wc -l)
printf 'build-context: %s upstream commit patch(es) and upstream.diff %s..%s in %s\n' \
  "${count// /}" "${base:0:12}" "${target:0:12}" "${ctx}"
