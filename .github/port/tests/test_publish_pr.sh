#!/usr/bin/env bash
# publish-pr.sh and report-failure.sh: the port pull request is created as a draft when red,
# updated, labelled and made ready when green; parity debt and failed runs land on issues; text
# that reaches GitHub carries no AI attribution and quotes agent text in fences.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

shim="${T_ROOT}/gh"
t_gh_shim "${shim}"
result="${T_ROOT}/result"
mkdir -p "${result}"
target=cccccccccccccccccccccccccccccccccccccccc
jq -n --arg target "${target}" '{target_sha: $target, commits: []}' > "${result}/plan.json"
printf '## Upstream port\n\nCo-Authored-By: Claude <noreply@anthropic.com>\nBody text\n' > "${result}/report.md"
printf 'fail' > "${result}/final-status"
jq -n '{overall_status: "partial", not_ported: [
  {go_path: "adapters/foo/foo.go", reason: "video ~~~ ping @someone"}]}' > "${result}/port-result.json"
: > "${result}/fix-result.json"

publish() {
  t_run_in "${T_ROOT}" env RUN_URL=https://example.invalid/runs/7 "${T_SCRIPTS}/publish-pr.sh" --result "${result}"
  if [[ "${T_RC}" -ne 0 ]]; then
    printf '%s\n' "${T_ERR}"
  fi
}

publish
t_eq "new red pull request: exit status" 0 "${T_RC}"
calls=$(cat "${shim}/calls.log")
t_has "new red pull request: created as a draft" "${calls}" \
  "pr create --repo test/fork --base main --head port/upstream-sync --title Upstream port: prebid/prebid-server through cccccccccccc --body-file"
t_has "new red pull request: draft flag" "${calls}" "--draft"
t_has "new red pull request: red and partial labels" "${calls}" \
  "pr edit 101 --repo test/fork --add-label upstream-port,port:red,port:partial --remove-label port:green"
t_has "new red pull request: labels exist" "${calls}" "label create port:needs-human"
body=$(cat "${shim}/body-1.md")
t_has "new red pull request: body from the report" "${body}" "Body text"
t_has "new red pull request: attribution is removed from the body" "${body}" "[removed: AI attribution]"
t_lacks "new red pull request: no Anthropic address" "${body}" "noreply@anthropic.com"
comment=$(cat "${shim}/body-2.md")
t_has "new red pull request: run comment" "${comment}" "final gates **fail**, agent status **partial**"
debt=$(cat "${shim}/body-3.md")
t_has "parity debt: issue body" "${debt}" "leaves these Go behaviours unported"
t_has "parity debt: agent text is fenced" "${debt}" $'~~~text\n- adapters/foo/foo.go: video ~ ~ ~ ping @someone\n~~~'
t_has "parity debt: a new issue" "${calls}" "issue create --repo test/fork --title Parity debt from upstream ports --label parity-debt"

: > "${shim}/calls.log"
rm -f "${shim}"/body-*.md
printf 'pass' > "${result}/final-status"
jq -n '{overall_status: "ported", not_ported: []}' > "${result}/fix-result.json"
printf '[{"number": 7}]\n' > "${shim}/prs.json"
printf '{"labels": [], "isDraft": true}\n' > "${shim}/pr_view.json"
publish
calls=$(cat "${shim}/calls.log")
t_has "green update: the pull request is edited" "${calls}" "pr edit 7 --repo test/fork --title Upstream port: prebid/prebid-server through cccccccccccc --body-file"
t_has "green update: green labels" "${calls}" "--add-label upstream-port,port:green --remove-label port:red,port:partial"
t_has "green update: ready for review" "${calls}" "pr ready 7 --repo test/fork"
t_lacks "green update: not created again" "${calls}" "pr create"
t_lacks "green update: no debt without gaps" "${calls}" "parity-debt --state open"

: > "${shim}/calls.log"
printf 'fail' > "${result}/final-status"
printf '{"labels": [], "isDraft": false}\n' > "${shim}/pr_view.json"
publish
calls=$(cat "${shim}/calls.log")
t_has "red update: back to draft" "${calls}" "pr ready 7 --repo test/fork --undo"

# Agents that never reported (a session that failed or did not run): the port log records the
# commit as blocked, so the pull request is labelled partial and the commit becomes parity debt,
# although the gates are green and no agent result exists.
: > "${shim}/calls.log"
rm -f "${shim}"/body-*.md
printf 'pass' > "${result}/final-status"
: > "${result}/port-result.json"
: > "${result}/fix-result.json"
jq -n -c '{upstream: "dddddddddddddddddddddddddddddddddddddddd", subject: "New Adapter: Bar ~~~ @someone",
  status: "blocked", not_ported: [], notes: "the agent reported no result for this commit"}' > "${result}/port-log.jsonl"
jq -n -c '{upstream: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", subject: "docs: readme", status: "no_change_needed",
  not_ported: [], notes: ""}' >> "${result}/port-log.jsonl"
printf '{"labels": [], "isDraft": true}\n' > "${shim}/pr_view.json"
publish
calls=$(cat "${shim}/calls.log")
t_has "silent agents: partial label although the gates are green" "${calls}" \
  "pr edit 7 --repo test/fork --add-label upstream-port,port:green,port:partial --remove-label port:red"
comment=$(cat "${shim}/body-2.md")
t_has "silent agents: the comment gives the derived status" "${comment}" "agent status **blocked**"
t_has "silent agents: the comment counts the commits" "${comment}" "1 upstream commit(s) are not fully ported"
debt=$(cat "${shim}/body-3.md")
t_has "silent agents: the blocked commit is parity debt" "${debt}" \
  "- upstream dddddddddddd New Adapter: Bar ~ ~ ~ @someone: blocked (the agent reported no result for this commit)"
t_lacks "silent agents: a handled commit is not debt" "${debt}" "eeeeeeeeeeee"
t_has "silent agents: debt is fenced" "${debt}" "~~~text"
rm -f "${result}/port-log.jsonl"

: > "${shim}/calls.log"
t_run_in "${T_ROOT}" env PR_NUMBER=7 REASON="CI is still red" RUN_URL=https://example.invalid/runs/8 \
  "${T_SCRIPTS}/publish-pr.sh" --escalate
t_eq "escalate: exit status" 0 "${T_RC}"
calls=$(cat "${shim}/calls.log")
t_has "escalate: label" "${calls}" "pr edit 7 --repo test/fork --add-label port:needs-human"
t_has "escalate: comment" "${calls}" "pr comment 7 --repo test/fork --body-file"
t_run_in "${T_ROOT}" env PR_NUMBER="" "${T_SCRIPTS}/publish-pr.sh" --escalate
t_eq "escalate: needs a pull request" 1 "${T_RC}"
t_run_in "${T_ROOT}" env PORT_BRANCH=main "${T_SCRIPTS}/publish-pr.sh" --result "${result}"
t_eq "publishing from the default branch is refused" 1 "${T_RC}"
t_run_in "${T_ROOT}" "${T_SCRIPTS}/publish-pr.sh"
t_eq "usage error" 2 "${T_RC}"

: > "${shim}/calls.log"
rm -f "${shim}"/body-*.md
t_run_in "${T_ROOT}" env PORT_RESULT=failure REASON="porting 2 upstream commit(s)" \
  RUN_URL=https://example.invalid/runs/9 "${T_SCRIPTS}/report-failure.sh"
t_eq "report-failure: exit status" 0 "${T_RC}"
calls=$(cat "${shim}/calls.log")
t_has "report-failure: a new issue" "${calls}" "issue create --repo test/fork --title Upstream port run failed --label port:run-failed"
failure=$(cat "${shim}/body-1.md")
t_has "report-failure: the result and the plan" "${failure}" "ended with **failure** while planned as: porting 2 upstream commit(s)"
: > "${shim}/calls.log"
t_run_in "${T_ROOT}" env PORT_RESULT=cancelled "${T_SCRIPTS}/report-failure.sh"
calls=$(cat "${shim}/calls.log")
t_has "report-failure: later failures comment on the open issue" "${calls}" "issue comment 201"

t_done
