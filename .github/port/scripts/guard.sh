#!/usr/bin/env bash
# guard.sh: check the fork invariants of REF. It only reads git objects and never builds or runs
# code from REF, so it is safe to run on untrusted commits (the publish job runs it on the port
# job's bundle before pushing anything).
#
# Usage: guard.sh --ref REF --base BASE [--bot] [--allow-human] [--parity-harness]
#                 [--strict-dirs a,b] [--pr-text FILE]
#   --ref             the commit to check
#   --base            what REF is compared with: the pull request base, the commit before a push,
#                     or the commit a port run started from
#   --bot             also apply the rules for what the port bot publishes
#   --allow-human     with --bot: commits by other authors are allowed (pull request CI of the port
#                     branch, where people may push fixes); without it every non-merge commit must
#                     be the bot's (the publish job)
#   --parity-harness  skip the go-tree rule (the parity-harness branch changes Go on purpose)
#   --strict-dirs     adapter directories the change touches
#   --pr-text         a file with the pull request title and body
#
# Rules
#   go-tree        every path outside .github/port/fork-paths.txt equals merge-base(REF, upstream)
#   local-state    no tracked .claude/ or .omx/ (local tool state)
#   marker         rust/UPSTREAM_SYNC names an upstream commit that REF has merged, and it did not
#                  move backwards from BASE's
#   known-list     rust/conformance/known_failures.txt is sorted, has no duplicates and lists only
#                  existing files
#   known-strict   no known failures are added for the --strict-dirs adapters
#   attribution    no AI attribution in the messages or identities of the first-parent commits of
#                  BASE..REF that upstream does not have, nor in --pr-text. Upstream commits are
#                  never checked: upstream history is merged as is and never rewritten.
# Bot rules (--bot), measured against merge-base(REF, default branch) so that merges of the
# default branch into the port branch do not count
#   bot-merge      merge commits only merge upstream or the default branch
#   bot-author     non-merge commits are the bot's (unless --allow-human), and the bot's touch
#                  only rust/
#   bot-protected  no change to the protected rust/ paths (lib.sh port_protected_paths)
#   bot-deps       no new external crate in rust/Cargo.lock
#   bot-known      no known failures are added
#   bot-log        rust/upstream-port-log.jsonl only grows: what that merge-base has is unchanged
# Env: UPSTREAM_BRANCH BASE_BRANCH, PORT_GIT_EMAIL (the bot's commit email)
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

usage() {
  printf 'usage: guard.sh --ref REF --base BASE [--bot] [--allow-human] [--parity-harness] [--strict-dirs a,b] [--pr-text FILE]\n' >&2
  exit 2
}

ref_arg=""
base_arg=""
bot=false
allow_human=false
parity=false
strict_csv=""
pr_text=""
while [[ "$#" -gt 0 ]]; do
  case "${1}" in
    --bot | --allow-human | --parity-harness)
      case "${1}" in
        --bot) bot=true ;;
        --allow-human) allow_human=true ;;
        *) parity=true ;;
      esac
      shift
      ;;
    --ref | --base | --strict-dirs | --pr-text)
      if [[ "$#" -lt 2 ]]; then
        usage
      fi
      case "${1}" in
        --ref) ref_arg=${2} ;;
        --base) base_arg=${2} ;;
        --strict-dirs) strict_csv=${2} ;;
        *) pr_text=${2} ;;
      esac
      shift 2
      ;;
    *) usage ;;
  esac
done
if [[ -z "${ref_arg}" || -z "${base_arg}" ]]; then
  usage
fi

port_cd_top
ref=$(git rev-parse --verify --end-of-options "${ref_arg}^{commit}")
base=$(git rev-parse --verify --end-of-options "${base_arg}^{commit}")
up=$(port_upstream_ref)
main=$(port_main_ref)
if ! git rev-parse -q --verify "${up}" > /dev/null; then
  port_die "guard: ${up} is missing; fetch upstream first"
fi
strict_lines=$(port_csv_lines "${strict_csv}")
bot_email=${PORT_GIT_EMAIL:-41898282+github-actions[bot]@users.noreply.github.com}
known_path=rust/conformance/known_failures.txt

tmp=$(mktemp -d)
trap 'rm -rf -- "${tmp}"' EXIT
violations=0

violation() {
  local rule=${1:?}
  shift
  port_error "[${rule}] $*"
  violations=$((violations + 1))
}

# known_entries REV: the sorted entries of the known-failures list at REV.
known_entries() {
  if git cat-file -e "${1:?}:${known_path}" 2> /dev/null; then
    git show "${1}:${known_path}" | port_list | sort -u
  fi
}

# known_added FROM: entries of REF's known-failures list that FROM's list does not have.
known_added() {
  known_entries "${1:?}" > "${tmp}/known-from"
  known_entries "${ref}" > "${tmp}/known-ref"
  comm -13 "${tmp}/known-from" "${tmp}/known-ref"
}

nonfork_listing=$(port_nonfork_spec)
mapfile -t nonfork <<< "${nonfork_listing}"
outside_listing=$(port_outside_rust_spec)
mapfile -t outside_rust <<< "${outside_listing}"
protected_listing=$(port_protected_paths)
mapfile -t protected <<< "${protected_listing}"

# go-tree
upstream_base=$(git merge-base "${ref}" "${up}" || true)
if [[ -z "${upstream_base}" ]]; then
  violation go-tree "${ref:0:12} shares no history with ${up}"
elif [[ "${parity}" == false ]] && ! git diff --quiet "${upstream_base}" "${ref}" -- "${nonfork[@]}"; then
  stat=$(git diff --stat=160 "${upstream_base}" "${ref}" -- "${nonfork[@]}" | tail -n 40)
  violation go-tree "paths outside the fork-owned set differ from upstream ${upstream_base:0:12}; only the paths in .github/port/fork-paths.txt may differ (see .github/README.md):"
  printf '%s\n' "${stat}" >&2
fi

# local-state: no committed agent configuration (Claude settings/hooks/MCP, ripgrep config) or
# local tool state. These are never tracked; upstream copies are dropped by merge-upstream.sh so a
# port agent session cannot load them, and the fork must not add them either.
agent_cfg_listing=$(port_agent_config_paths)
mapfile -t agent_cfg <<< "${agent_cfg_listing}"
tracked=$(git ls-tree -r --name-only "${ref}" -- "${agent_cfg[@]}" | head -n 10)
if [[ -n "${tracked}" ]]; then
  violation local-state "agent configuration or local tool state is committed: ${tracked//$'\n'/, }"
fi

# marker
marker=$(port_marker "${ref}")
if [[ -z "${marker}" ]]; then
  violation marker "rust/UPSTREAM_SYNC at ${ref:0:12} names no upstream commit"
elif ! git cat-file -e "${marker}^{commit}" 2> /dev/null; then
  violation marker "rust/UPSTREAM_SYNC names ${marker:0:12}, which is not in this repository"
else
  if ! git merge-base --is-ancestor "${marker}" "${up}"; then
    violation marker "rust/UPSTREAM_SYNC ${marker:0:12} is not an upstream commit"
  fi
  if ! git merge-base --is-ancestor "${marker}" "${ref}"; then
    violation marker "rust/UPSTREAM_SYNC ${marker:0:12} is not merged into ${ref:0:12}"
  fi
  base_marker=$(port_marker "${base}")
  if [[ -n "${base_marker}" && "${base_marker}" != "${marker}" ]] \
    && ! git merge-base --is-ancestor "${base_marker}" "${marker}" 2> /dev/null; then
    violation marker "rust/UPSTREAM_SYNC moved backwards from ${base_marker:0:12} to ${marker:0:12}"
  fi
fi

# known-list
if git cat-file -e "${ref}:${known_path}" 2> /dev/null; then
  git show "${ref}:${known_path}" | port_list > "${tmp}/known-raw"
  sort -u "${tmp}/known-raw" > "${tmp}/known-sorted"
  if ! cmp -s "${tmp}/known-raw" "${tmp}/known-sorted"; then
    violation known-list "${known_path} is not sorted (LC_ALL=C) or has duplicate lines"
  fi
  sed "s|^|${ref}:|" "${tmp}/known-sorted" | git cat-file --batch-check > "${tmp}/known-check"
  missing=$(sed -n -E 's/^[0-9a-f]{40}:(.*) missing$/\1/p' "${tmp}/known-check" | head -n 20)
  if [[ -n "${missing}" ]]; then
    violation known-list "${known_path} lists files that do not exist: ${missing//$'\n'/, }"
  fi
fi

# known-strict
if [[ -n "${strict_lines}" ]]; then
  pr_base=$(git merge-base "${base}" "${ref}" || printf '%s' "${base}")
  added=$(known_added "${pr_base}")
  while IFS= read -r dir; do
    hits=$(grep -F -e "adapters/${dir}/" <<< "${added}" | head -n 10 || true)
    if [[ -n "${hits}" ]]; then
      violation known-strict "known failures were added for adapters/${dir}, which this change touches: ${hits//$'\n'/, }"
    fi
  done <<< "${strict_lines}"
fi

# attribution
commits=$(git rev-list --first-parent "${ref}" "^${base}" "^${up}")
attribution_re=$(port_attribution_re)
name_re=$(port_ai_name_re)
email_re=$(port_ai_email_re)
checked=0
while IFS= read -r commit; do
  if [[ -z "${commit}" ]]; then
    continue
  fi
  checked=$((checked + 1))
  body=$(git log -1 --format=%B "${commit}")
  hit=$(grep -Ei -m 1 -e "${attribution_re}" <<< "${body}" || true)
  if [[ -n "${hit}" ]]; then
    violation attribution "commit ${commit:0:12} carries AI attribution: ${hit}"
  fi
  names=$(git log -1 --format='%an%n%cn' "${commit}")
  hit=$(grep -Ei -m 1 -e "${name_re}" <<< "${names}" || true)
  if [[ -n "${hit}" ]]; then
    violation attribution "commit ${commit:0:12} is authored or committed as '${hit}'"
  fi
  emails=$(git log -1 --format='%ae%n%ce' "${commit}")
  hit=$(grep -Ei -m 1 -e "${email_re}" <<< "${emails}" || true)
  if [[ -n "${hit}" ]]; then
    violation attribution "commit ${commit:0:12} is authored or committed by '${hit}'"
  fi
done <<< "${commits}"
if [[ -n "${pr_text}" ]]; then
  hit=$(grep -Ei -m 1 -e "${attribution_re}" "${pr_text}" || true)
  if [[ -n "${hit}" ]]; then
    violation attribution "the pull request title or body carries AI attribution: ${hit}"
  fi
fi

# bot rules
if [[ "${bot}" == true ]]; then
  if ! git rev-parse -q --verify "${main}" > /dev/null; then
    port_die "guard: ${main} is missing; fetch the default branch first"
  fi
  main_base=$(git merge-base "${ref}" "${main}" || true)
  if [[ -z "${main_base}" ]]; then
    violation bot-merge "${ref:0:12} shares no history with ${main}"
    main_base=${base}
  fi
  while IFS= read -r commit; do
    if [[ -z "${commit}" ]]; then
      continue
    fi
    parent_line=$(git rev-list --parents -n 1 "${commit}")
    read -r -a fields <<< "${parent_line}"
    if [[ "${#fields[@]}" -gt 2 ]]; then
      for parent in "${fields[@]:2}"; do
        if ! git merge-base --is-ancestor "${parent}" "${up}" && ! git merge-base --is-ancestor "${parent}" "${main}"; then
          violation bot-merge "merge ${commit:0:12} merges ${parent:0:12}, which is neither upstream nor ${main#refs/remotes/origin/}"
        fi
      done
      continue
    fi
    author=$(git log -1 --format=%ae "${commit}")
    if [[ "${author}" == "${bot_email}" ]]; then
      touched=$(git diff-tree --no-commit-id --name-only -r "${commit}" -- "${outside_rust[@]}" | head -n 10)
      if [[ -n "${touched}" ]]; then
        violation bot-author "bot commit ${commit:0:12} touches paths outside rust/: ${touched//$'\n'/, }"
      fi
    elif [[ "${allow_human}" == false ]]; then
      violation bot-author "commit ${commit:0:12} by ${author} is not the bot's (${bot_email})"
    fi
  done <<< "${commits}"
  changed=$(git diff --name-only "${main_base}" "${ref}" -- "${protected[@]}" | head -n 20)
  if [[ -n "${changed}" ]]; then
    violation bot-protected "protected paths changed: ${changed//$'\n'/, }"
  fi
  lock_diff=$(git diff "${main_base}" "${ref}" -- rust/Cargo.lock)
  sources=$(grep -E '^\+source = ' <<< "${lock_diff}" | sort -u | head -n 10 || true)
  if [[ -n "${sources}" ]]; then
    violation bot-deps "rust/Cargo.lock gained external packages: ${sources//$'\n'/, }"
  fi
  added=$(known_added "${main_base}")
  if [[ -n "${added}" ]]; then
    violation bot-known "known failures were added: ${added//$'\n'/, }"
  fi
  # An append adds lines and deletes none; an edited, removed or reordered line counts as deleted.
  log_path=rust/upstream-port-log.jsonl
  log_stat=$(git diff --numstat --no-renames "${main_base}" "${ref}" -- "${log_path}")
  log_deleted=0
  if [[ -n "${log_stat}" ]]; then
    read -r _ log_deleted _ <<< "${log_stat}"
  fi
  if [[ "${log_deleted}" != 0 ]]; then
    violation bot-log "${log_path} changed lines that ${main#refs/remotes/origin/} already has; the port only appends to it"
  fi
fi

if [[ "${violations}" -gt 0 ]]; then
  printf 'guard: %s violation(s) in %s (%s first-parent commit(s) checked)\n' "${violations}" "${ref:0:12}" "${checked}" >&2
  exit 1
fi
mode=main
if [[ "${bot}" == true ]]; then
  mode=bot
fi
printf 'guard: ok for %s (%s rules, %s first-parent commit(s) checked)\n' "${ref:0:12}" "${mode}" "${checked}"
