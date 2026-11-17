#!/usr/bin/env bash
# commit.sh: record the run on the work branch with the bot identity and no AI attribution.
#   advance    rust/UPSTREAM_SYNC := TARGET_SHA (not in merge-only mode), one line per planned
#              upstream commit appended to rust/upstream-port-log.jsonl, and the Rust changes
#   repair     the Rust changes only
# Nothing is committed when nothing changed. Only rust/ is staged; check-worktree.sh has already
# reverted everything else.
#
# Env: ACTION RUN_MODE CTX MERGED_SHA BASE_SHA TARGET_SHA RUN_URL, PORT_JSON / FIX_JSON (structured
#      output of the port and fix sessions, may be empty), and the commit identity in
#      GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
action=${ACTION:?}
run_mode=${RUN_MODE:-auto}
ctx=${CTX:?}
merged=${MERGED_SHA:?}
base=${BASE_SHA:?}
target=${TARGET_SHA:?}
: "${GIT_AUTHOR_NAME:?}" "${GIT_AUTHOR_EMAIL:?}" "${GIT_COMMITTER_NAME:?}" "${GIT_COMMITTER_EMAIL:?}"
plan="${ctx}/plan.json"
head=$(git rev-parse HEAD)
expected=$(git rev-parse --verify "${merged}^{commit}")
if [[ "${head}" != "${expected}" ]]; then
  port_die "commit: HEAD is not ${merged:0:12}; run check-worktree.sh first"
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
                elif $mode == "merge-only" then "merged-not-ported"
                elif $r != null then $r.status
                elif ($c.needs_claude | not) then "no_change_needed"
                else "blocked" end),
       not_ported: (if $mode == "merge-only" or $c.classes == ["skipped"] then []
                    else [$result.not_ported[]? | (.sha | sha_text) as $s
                          | select($s == "" or ($c.sha | startswith($s))) | {go_path, reason}] end),
       notes: ($r.notes // (if $r == null and $c.needs_claude and $mode != "merge-only" then "the agent reported no result for this commit" else "" end)),
       run: $run, date: $date}' "${plan}" >> rust/upstream-port-log.jsonl
  if [[ "${run_mode}" != merge-only ]]; then
    header=$(sed -n '/^#/p' rust/UPSTREAM_SYNC)
    printf '%s\n%s\n' "${header}" "${target}" | sed '/^$/d' > rust/UPSTREAM_SYNC
  fi
fi

git add -A -- rust
if git diff --cached --quiet; then
  printf 'commit: nothing to commit\n'
  exit 0
fi
code_changes=$(git diff --cached --name-only -- rust ':(exclude)rust/UPSTREAM_SYNC' ':(exclude)rust/upstream-port-log.jsonl')

message="${ctx}/commit-message.txt"
first_subject=$(jq -r '.commits[0].subject // ""' "${plan}" | port_sanitize)
count=$(jq -r '.commits | length' "${plan}")
statuses=$(tail -n "${count}" rust/upstream-port-log.jsonl 2> /dev/null || true)
if [[ "${action}" == repair ]]; then
  failing=$(jq -r '[.failed[]?] | join(", ")' "${ctx}/gates-before.json" 2> /dev/null || true)
  printf 'port-repair: fix %s\n\n' "${failing:-CI failures}" > "${message}"
  printf 'Repairs the port of upstream %s..%s without porting new commits.\n' "${base:0:12}" "${target:0:12}" >> "${message}"
elif [[ "${run_mode}" == merge-only ]]; then
  printf 'port: record upstream %s..%s as merged, not ported\n\n' "${base:0:12}" "${target:0:12}" > "${message}"
elif [[ -z "${code_changes}" ]]; then
  printf 'port: advance upstream marker to %s (no Rust changes needed)\n\n' "${target:0:12}" > "${message}"
elif [[ "${count}" -gt 1 ]]; then
  printf 'port: %s (+%s more)\n\n' "${first_subject}" "$((count - 1))" > "${message}"
else
  printf 'port: %s\n\n' "${first_subject}" > "${message}"
fi
if [[ "${action}" == advance ]]; then
  jq -r '"upstream \(.upstream[0:12]) \(.subject) (\(.status))"' <<< "${statuses}" | port_sanitize >> "${message}"
  printf '\nUpstream-Range: %s..%s\n' "${base:0:12}" "${target:0:12}" >> "${message}"
fi
if [[ -n "${RUN_URL:-}" ]]; then
  printf 'Port-Run: %s\n' "${RUN_URL}" >> "${message}"
fi
git -c commit.gpgsign=false commit --no-verify -q --cleanup=whitespace -F "${message}"
new_head=$(git rev-parse HEAD)
printf 'commit: %s\n' "${new_head:0:12}"
