#!/usr/bin/env bash
# ci-changes.sh: work out what a CI run compares. Step outputs:
#   base            the commit the change is compared with
#   head            the commit under test
#   adapter_dirs    adapter directories the change touches, comma-separated. Upstream changes come
#                   from the pins: when the change moves UPSTREAM_SYNC, every upstream file changed
#                   between the two pinned commits under adapters/<dir>/,
#                   static/bidder-info/<bidder>.yaml and static/bidder-params/<bidder>.json
#                   (aliases resolve to their parent adapter) and openrtb_ext/imp_<dir>.go counts.
#                   Rust changes count too: files under crates/adapters/ named after an adapter
#                   directory (compared case-insensitively, ignoring "_" and "-").
#   port_pr         true for a pull request from this repository's port branch
#   parity_harness  true when the change is on the parity-harness branch: a push to it, a pull
#                   request into it, a merge queue run for it (merge_group.base_ref, or the queue
#                   branch gh-readonly-queue/parity-harness/...), and a manual run on it
#                   (workflow_dispatch from refs/heads/parity-harness)
#   full_history    true for a push that names no commit to compare with (a new branch or a new
#                   repository: the before SHA is all zeros, or a commit the clone lacks): every
#                   commit of head is new to the branch, so the guard checks all of them for
#                   attribution, not just base..head
# Upstream trees and history are read from the bare upstream mirror UPSTREAM_GIT
# (upstream-mirror.sh); this repository never holds them.
# Env: EVENT REF GITHUB_OUTPUT PORT_BRANCH HARNESS_BRANCH REPOSITORY UPSTREAM_GIT, and per event
#      PR_BASE PR_HEAD PR_HEAD_REF PR_HEAD_REPO BASE_REF (pull_request), PUSH_BEFORE (push),
#      MG_BASE MG_HEAD MG_BASE_REF (merge_group).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
event=${EVENT:?}
: "${GITHUB_OUTPUT:?}" "${UPSTREAM_GIT:?}"
port_branch=${PORT_BRANCH:-port/upstream-sync}
harness_branch=${HARNESS_BRANCH:-parity-harness}

commit_or_empty() {
  local candidate=${1-}
  if [[ "${candidate}" =~ ^[0-9a-f]{40}$ ]] && git cat-file -e "${candidate}^{commit}" 2> /dev/null; then
    printf '%s\n' "${candidate}"
  fi
}

# upstream_commit_or_empty SHA: SHA when the mirror has that commit.
upstream_commit_or_empty() {
  local candidate=${1-}
  if [[ "${candidate}" =~ ^[0-9a-f]{40}$ ]] \
    && git --git-dir="${UPSTREAM_GIT}" cat-file -e "${candidate}^{commit}" 2> /dev/null; then
    printf '%s\n' "${candidate}"
  fi
}

head_candidate=HEAD
base_candidate=""
case "${event}" in
  pull_request)
    head_candidate=${PR_HEAD:-HEAD}
    base_candidate=${PR_BASE:-}
    ;;
  merge_group)
    head_candidate=${MG_HEAD:-HEAD}
    base_candidate=${MG_BASE:-}
    ;;
  push) base_candidate=${PUSH_BEFORE:-} ;;
  *) ;;
esac
head=$(git rev-parse --verify --end-of-options "${head_candidate}^{commit}")
base=$(commit_or_empty "${base_candidate}")
full_history=false
if [[ -z "${base}" ]]; then
  base=$(git rev-parse --verify -q "${head}^1" || printf '%s' "${head}")
  if [[ "${event}" == push ]]; then
    full_history=true
  fi
fi
diff_base=$(git merge-base "${base}" "${head}" || printf '%s' "${base}")

head_marker=$(port_marker "${head}")
head_pin=$(upstream_commit_or_empty "${head_marker}")
base_marker=$(port_marker "${diff_base}")
base_pin=$(upstream_commit_or_empty "${base_marker}")
if [[ -z "${head_pin}" ]]; then
  port_warning "ci-changes: UPSTREAM_SYNC at ${head:0:12} does not pin a commit of the upstream mirror; only Rust changes count"
fi

# The adapter directories: those of the head's pin, else of the base's.
adapter_listing=""
listing_pin=${head_pin:-${base_pin}}
if [[ -n "${listing_pin}" ]]; then
  adapter_listing=$(port_up ls-tree -d --name-only "${listing_pin}" adapters/)
fi
declare -A adapter_dirs=() normalized=() touched=()
while IFS= read -r path; do
  name=${path#adapters/}
  if [[ "${name}" =~ ^[A-Za-z0-9_]+$ && "${name}" != adapterstest ]]; then
    adapter_dirs[${name}]=1
    key=${name,,}
    key=${key//[^a-z0-9]/}
    normalized[${key}]=${name}
  fi
done <<< "${adapter_listing}"

# parent_of BIDDER: the adapter directory of BIDDER (its aliasOf parent or itself), if any.
parent_of() {
  local bidder=${1:?} text parent
  text=$(git --git-dir="${UPSTREAM_GIT}" show "${head_pin}:static/bidder-info/${bidder}.yaml" 2> /dev/null) || text=""
  parent=$(sed -n -E 's/^aliasOf:[[:space:]]*"?([A-Za-z0-9_]+)"?[[:space:]]*(#.*)?$/\1/p' <<< "${text}" | head -n 1)
  if [[ -n "${parent}" && -n "${adapter_dirs[${parent}]:-}" ]]; then
    printf '%s\n' "${parent}"
  elif [[ -n "${adapter_dirs[${bidder}]:-}" ]]; then
    printf '%s\n' "${bidder}"
  fi
}

upstream_changes=""
if [[ -n "${head_pin}" && -n "${base_pin}" && "${head_pin}" != "${base_pin}" ]]; then
  upstream_changes=$(port_up diff --name-only --no-renames "${base_pin}" "${head_pin}")
fi
while IFS= read -r path; do
  name=""
  case "${path}" in
    adapters/*/*)
      name=${path#adapters/}
      name=${name%%/*}
      if [[ ! "${name}" =~ ^[A-Za-z0-9_]+$ || -z "${adapter_dirs[${name}]:-}" ]]; then
        name=""
      fi
      ;;
    static/bidder-info/*.yaml | static/bidder-params/*.json)
      name=${path##*/}
      name=${name%.*}
      if [[ "${name}" =~ ^[A-Za-z0-9_]+$ ]]; then
        name=$(parent_of "${name}")
      else
        name=""
      fi
      ;;
    openrtb_ext/imp_*.go)
      name=${path#openrtb_ext/imp_}
      name=${name%.go}
      if [[ ! "${name}" =~ ^[A-Za-z0-9_]+$ || -z "${adapter_dirs[${name}]:-}" ]]; then
        name=""
      fi
      ;;
    *) ;;
  esac
  if [[ -n "${name}" ]]; then
    touched[${name}]=1
  fi
done <<< "${upstream_changes}"

rust_changes=$(git diff --name-only --no-renames "${diff_base}" "${head}" -- ':(top)crates/adapters')
while IFS= read -r path; do
  if [[ -z "${path}" ]]; then
    continue
  fi
  parts=${path#crates/adapters/}
  parts=${parts%.rs}
  IFS='/' read -r -a components <<< "${parts}"
  for component in "${components[@]}"; do
    key=${component,,}
    key=${key//[^a-z0-9]/}
    if [[ -n "${key}" && -n "${normalized[${key}]:-}" ]]; then
      touched[${normalized[${key}]}]=1
    fi
  done
done <<< "${rust_changes}"

touched_list=$(printf '%s\n' "${!touched[@]}" | sort | sed '/^$/d' | paste -s -d, -)
port_pr=false
if [[ "${event}" == pull_request && "${PR_HEAD_REF:-}" == "${port_branch}" && -n "${REPOSITORY:-}" \
  && "${PR_HEAD_REPO:-}" == "${REPOSITORY:-}" ]]; then
  port_pr=true
fi
parity_harness=false
case "${event}" in
  push | workflow_dispatch)
    if [[ "${REF:-}" == "refs/heads/${harness_branch}" ]]; then
      parity_harness=true
    fi
    ;;
  pull_request)
    if [[ "${BASE_REF:-}" == "${harness_branch}" ]]; then
      parity_harness=true
    fi
    ;;
  merge_group)
    if [[ "${MG_BASE_REF:-}" == "refs/heads/${harness_branch}" \
      || "${REF:-}" == "refs/heads/gh-readonly-queue/${harness_branch}/"* ]]; then
      parity_harness=true
    fi
    ;;
  *) ;;
esac

port_output base "${base}"
port_output head "${head}"
port_output adapter_dirs "${touched_list}"
port_output port_pr "${port_pr}"
port_output parity_harness "${parity_harness}"
port_output full_history "${full_history}"
printf 'ci-changes: %s..%s (upstream %s..%s), adapters [%s], port pull request %s, parity harness %s, full history %s\n' \
  "${base:0:12}" "${head:0:12}" "${base_pin:0:12}" "${head_pin:0:12}" "${touched_list}" "${port_pr}" "${parity_harness}" \
  "${full_history}"
