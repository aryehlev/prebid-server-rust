#!/usr/bin/env bash
# commit.sh: record the run on the work branch with the bot identity and no AI attribution.
#   advance    the pin advances to TARGET_SHA (advance-pin.sh: UPSTREAM_SYNC and the upstream
#              version every Go tooling module requires), one line per planned upstream commit is
#              appended to upstream-port-log.jsonl, and the Rust changes are committed with them.
#              In pin-only mode no agent ran and every planned commit is logged as
#              pinned-not-ported, which the pull request reports as parity debt.
#   repair     the Rust changes only. A repair that changed nothing is still committed (empty):
#              plan.sh counts port-repair commits to decide when to escalate, and the new head
#              makes CI run again. Without it a repair that finds nothing would be planned again
#              every day and never escalate.
# Only the writable paths and the port records (lib.sh port_writable_paths, port_record_paths)
# are staged; check-worktree.sh has already reverted everything else.
#
# Env: ACTION RUN_MODE CTX START_SHA BASE_SHA TARGET_SHA PIN_VERSION RUN_URL, PORT_JSON / FIX_JSON
#      (structured output of the port and fix sessions, may be empty), and the commit identity in
#      GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
action=${ACTION:?}
run_mode=${RUN_MODE:-auto}
ctx=${CTX:?}
start=${START_SHA:?}
base=${BASE_SHA:?}
target=${TARGET_SHA:?}
: "${GIT_AUTHOR_NAME:?}" "${GIT_AUTHOR_EMAIL:?}" "${GIT_COMMITTER_NAME:?}" "${GIT_COMMITTER_EMAIL:?}"
plan="${ctx}/plan.json"
log_path=upstream-port-log.jsonl
head=$(git rev-parse HEAD)
expected=$(git rev-parse --verify "${start}^{commit}")
if [[ "${head}" != "${expected}" ]]; then
  port_die "commit: HEAD is not ${start:0:12}; run check-worktree.sh first"
fi

# The newest structured result: the fix session's when it ran, else the port session's.
result='{}'
for candidate in "${FIX_JSON:-}" "${PORT_JSON:-}"; do
  if [[ -n "${candidate}" ]]; then
    kind=$(jq -r 'type' <<< "${candidate}" 2> /dev/null) || kind=invalid
    if [[ "${kind}" == object ]]; then
      result=${candidate}
      break
    fi
  fi
done

if [[ "${action}" == advance ]]; then
  "${PORT_SCRIPTS}/advance-pin.sh" "${target}" "${PIN_VERSION:?}"
  now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  attribution=$(port_attribution_re)
  # A result entry belongs to a planned commit when its sha is that commit or a prefix of at least
  # 7 characters; a not_ported item without a sha belongs to every planned commit. Subjects lose
  # AI attribution as port_sanitize removes it (plan.sh already does; this keeps the log clean
  # whatever wrote the plan).
  jq -c --argjson result "${result}" --arg mode "${run_mode}" --arg run "${RUN_URL:-}" --arg date "${now}" \
    --arg attribution "${attribution}" '
    def sha_text: if type == "string" then . else "" end;
    def clean: if test($attribution; "i") then "[removed: AI attribution]" else . end;
    .commits[] as $c
    | ([$result.commits[]? | (.sha | sha_text) as $s
        | select($s == $c.sha or (($s | length) >= 7 and ($c.sha | startswith($s))))][0]) as $r
    | {upstream: $c.sha, subject: ($c.subject | clean), classes: $c.classes, adapter_dirs: $c.adapter_dirs,
       status: (if $c.classes == ["skipped"] then "skipped"
                elif $mode == "pin-only" then "pinned-not-ported"
                elif $r != null then $r.status
                elif ($c.needs_claude | not) then "no_change_needed"
                else "blocked" end),
       not_ported: (if $mode == "pin-only" or $c.classes == ["skipped"] then []
                    else [$result.not_ported[]? | (.sha | sha_text) as $s
                          | select($s == "" or ($c.sha | startswith($s))) | {go_path, reason}] end),
       notes: ($r.notes // (if $r == null and $c.needs_claude and $mode != "pin-only" then "the agent reported no result for this commit" else "" end)),
       run: $run, date: $date}' "${plan}" >> "${log_path}"
fi

# Stage the writable paths and the records that exist in the work tree or in HEAD (a pathspec
# that matches nothing would make git add fail).
writable_listing=$(port_writable_paths)
record_listing=$(port_record_paths)
stage=()
while IFS= read -r path; do
  if [[ -e "${path}" ]] || git cat-file -e "HEAD:${path}" 2> /dev/null; then
    stage+=(":(top)${path}")
  fi
done <<< "${writable_listing}"$'\n'"${record_listing}"
git add -A -- "${stage[@]}"
empty=()
if git diff --cached --quiet; then
  if [[ "${action}" != repair ]]; then
    printf 'commit: nothing to commit\n'
    exit 0
  fi
  empty=(--allow-empty)
fi

# The Rust changes: everything staged except the records and Go tooling go.mod files whose only
# change is the upstream version advance-pin.sh wrote.
module=$(port_go_module)
module_re="^[-+][[:space:]]*(require[[:space:]]+)?${module//./[.]}[[:space:]]+v"
staged=$(git diff --cached --name-only -- ':(top)' ':(top,exclude)UPSTREAM_SYNC' ":(top,exclude)${log_path}")
code_changes=""
while IFS= read -r path; do
  if [[ -z "${path}" ]]; then
    continue
  fi
  if [[ "${path}" == */go.mod || "${path}" == go.mod ]]; then
    hunk=$(git diff --cached -U0 -- ":(top)${path}")
    other=$(grep -E '^[-+]' <<< "${hunk}" | grep -v -E -e '^(---|[+]{3}) ' -e "${module_re}" || true)
    if [[ -z "${other}" ]]; then
      continue
    fi
  fi
  code_changes+="${path}"$'\n'
done <<< "${staged}"

message="${ctx}/commit-message.txt"
first_subject=$(jq -r '.commits[0].subject // ""' "${plan}" | port_sanitize)
count=$(jq -r '.commits | length' "${plan}")
statuses=$(tail -n "${count}" "${log_path}" 2> /dev/null || true)
# Commits of this run that the log does not record as handled: an agent that reported them partial
# or blocked, or that never reported (its session failed or did not run).
unported=0
if [[ "${action}" == advance && -n "${statuses}" ]]; then
  unported=$(jq -s '[.[] | select(.status == "partial" or .status == "blocked")] | length' <<< "${statuses}")
fi
if [[ "${action}" == repair ]]; then
  failing=$(jq -r '[.failed[]?] | join(", ")' "${ctx}/gates-before.json" 2> /dev/null || true)
  if [[ "${#empty[@]}" -gt 0 ]]; then
    printf 'port-repair: no change found for %s\n\n' "${failing:-the CI failures}" > "${message}"
    printf 'The repair session of upstream %s..%s changed nothing. This empty commit records the\n' \
      "${base:0:12}" "${target:0:12}" >> "${message}"
    printf 'attempt, so the planner escalates after PORT_MAX_REPAIRS attempts, and lets CI run again.\n' >> "${message}"
  else
    printf 'port-repair: fix %s\n\n' "${failing:-CI failures}" > "${message}"
    printf 'Repairs the port of upstream %s..%s without porting new commits.\n' "${base:0:12}" "${target:0:12}" >> "${message}"
  fi
elif [[ "${run_mode}" == pin-only ]]; then
  printf 'port: advance upstream pin to %s without porting (pin-only run)\n\n' "${target:0:12}" > "${message}"
elif [[ -z "${code_changes}" && "${unported}" -gt 0 ]]; then
  printf 'port: advance upstream pin to %s (no Rust changes; %s commit(s) not ported)\n\n' \
    "${target:0:12}" "${unported}" > "${message}"
elif [[ -z "${code_changes}" ]]; then
  printf 'port: advance upstream pin to %s (no Rust changes needed)\n\n' "${target:0:12}" > "${message}"
elif [[ "${count}" -gt 1 ]]; then
  printf 'port: %s (+%s more)\n\n' "${first_subject}" "$((count - 1))" > "${message}"
else
  printf 'port: %s\n\n' "${first_subject}" > "${message}"
fi
if [[ "${action}" == advance ]]; then
  {
    jq -r '"upstream \(.upstream[0:12]) \(.subject) (\(.status))"' <<< "${statuses}" | port_sanitize
    printf '\nUpstream-Range: %s..%s\n' "${base:0:12}" "${target:0:12}"
    printf 'Upstream-Version: %s\n' "${PIN_VERSION}"
  } >> "${message}"
fi
if [[ -n "${RUN_URL:-}" ]]; then
  printf 'Port-Run: %s\n' "${RUN_URL}" >> "${message}"
fi
git -c commit.gpgsign=false commit --no-verify -q --cleanup=whitespace "${empty[@]}" -F "${message}"
new_head=$(git rev-parse HEAD)
printf 'commit: %s\n' "${new_head:0:12}"
