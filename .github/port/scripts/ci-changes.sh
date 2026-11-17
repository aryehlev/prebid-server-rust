#!/usr/bin/env bash
# ci-changes.sh: work out what a CI run compares. Step outputs:
#   base            the commit the change is compared with
#   head            the commit under test
#   adapter_dirs    adapter directories the change touches, comma-separated: changed files under
#                   adapters/<dir>/, static/bidder-info/<bidder>.yaml and static/bidder-params/
#                   <bidder>.json (aliases resolve to their parent adapter), openrtb_ext/imp_<dir>.go,
#                   and Rust files under rust/crates/adapters/ named after an adapter directory
#                   (compared case-insensitively, ignoring "_" and "-")
#   port_pr         true for a pull request from this repository's port branch
#   parity_harness  true for a push to the parity-harness branch or a pull request into it
# Env: EVENT REF GITHUB_OUTPUT PORT_BRANCH HARNESS_BRANCH REPOSITORY, and per event PR_BASE PR_HEAD
#      PR_HEAD_REF PR_HEAD_REPO BASE_REF (pull_request), PUSH_BEFORE (push), MG_BASE MG_HEAD
#      (merge_group).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
event=${EVENT:?}
: "${GITHUB_OUTPUT:?}"
port_branch=${PORT_BRANCH:-port/upstream-sync}
harness_branch=${HARNESS_BRANCH:-parity-harness}

commit_or_empty() {
  local candidate=${1-}
  if [[ "${candidate}" =~ ^[0-9a-f]{40}$ ]] && git cat-file -e "${candidate}^{commit}" 2> /dev/null; then
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
if [[ -z "${base}" ]]; then
  base=$(git rev-parse --verify -q "${head}^1" || printf '%s' "${head}")
fi
diff_base=$(git merge-base "${base}" "${head}" || printf '%s' "${base}")

adapter_listing=$(git ls-tree -d --name-only "${head}" adapters/)
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
  text=$(git show "${head}:static/bidder-info/${bidder}.yaml" 2> /dev/null) || text=""
  parent=$(sed -n -E 's/^aliasOf:[[:space:]]*"?([A-Za-z0-9_]+)"?[[:space:]]*(#.*)?$/\1/p' <<< "${text}" | head -n 1)
  if [[ -n "${parent}" && -n "${adapter_dirs[${parent}]:-}" ]]; then
    printf '%s\n' "${parent}"
  elif [[ -n "${adapter_dirs[${bidder}]:-}" ]]; then
    printf '%s\n' "${bidder}"
  fi
}

changes=$(git diff --name-only --no-renames "${diff_base}" "${head}")
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
    rust/crates/adapters/*)
      parts=${path#rust/crates/adapters/}
      parts=${parts%.rs}
      IFS='/' read -r -a components <<< "${parts}"
      for component in "${components[@]}"; do
        key=${component,,}
        key=${key//[^a-z0-9]/}
        if [[ -n "${key}" && -n "${normalized[${key}]:-}" ]]; then
          touched[${normalized[${key}]}]=1
        fi
      done
      ;;
    *) ;;
  esac
  if [[ -n "${name}" ]]; then
    touched[${name}]=1
  fi
done <<< "${changes}"

touched_list=$(printf '%s\n' "${!touched[@]}" | sort | sed '/^$/d' | paste -s -d, -)
port_pr=false
if [[ "${event}" == pull_request && "${PR_HEAD_REF:-}" == "${port_branch}" && -n "${REPOSITORY:-}" \
  && "${PR_HEAD_REPO:-}" == "${REPOSITORY:-}" ]]; then
  port_pr=true
fi
parity_harness=false
if [[ "${event}" == push && "${REF:-}" == "refs/heads/${harness_branch}" ]] \
  || [[ "${event}" == pull_request && "${BASE_REF:-}" == "${harness_branch}" ]]; then
  parity_harness=true
fi

port_output base "${base}"
port_output head "${head}"
port_output adapter_dirs "${touched_list}"
port_output port_pr "${port_pr}"
port_output parity_harness "${parity_harness}"
printf 'ci-changes: %s..%s, adapters [%s], port pull request %s, parity harness %s\n' \
  "${base:0:12}" "${head:0:12}" "${touched_list}" "${port_pr}" "${parity_harness}"
