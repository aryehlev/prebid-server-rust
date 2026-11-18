#!/usr/bin/env bash
# publish-pr.sh: maintain the single upstream-port pull request (after the port branch was pushed).
#
# Usage
#   publish-pr.sh --result DIR   open or update the pull request from a port result directory
#                                (report.md, final-status, plan.json, port-result.json,
#                                fix-result.json, and port-log.jsonl: this run's lines of
#                                upstream-port-log.jsonl), set its labels and draft state,
#                                comment the run, and record anything not ported on the
#                                parity-debt issue. A commit the log records as partial, blocked or
#                                pinned-not-ported counts as not ported even when the agents
#                                returned no result (a session that failed or never ran, or a
#                                pin-only run), so the pin never moves past it silently.
#   publish-pr.sh --escalate     label the open pull request port:needs-human and comment REASON
# Env: GH_TOKEN GITHUB_REPOSITORY BASE_BRANCH PORT_BRANCH RUN_URL, and PR_NUMBER REASON for
#      --escalate.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

repo=${GITHUB_REPOSITORY:?}
base_branch=${BASE_BRANCH:?}
port_branch=${PORT_BRANCH:?}
run_url=${RUN_URL:-}
: "${GH_TOKEN:?}"
case "${port_branch}" in
  main | master | "${base_branch}") port_die "publish-pr: refusing to publish from '${port_branch}'" ;;
  *) ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf -- "${tmp}"' EXIT

ensure_labels() {
  local spec name color description
  for spec in \
    "upstream-port|0e8a16|Automated port of upstream prebid/prebid-server changes" \
    "port:green|0e8a16|Port gates pass" \
    "port:red|d73a4a|Port gates fail" \
    "port:partial|fbca04|Parts of the upstream change are not ported" \
    "port:needs-human|b60205|The port workflow stopped and needs a person" \
    "parity-debt|5319e7|Upstream behaviour the Rust port does not have yet" \
    "port:run-failed|d73a4a|An upstream-port workflow run failed"; do
    IFS='|' read -r name color description <<< "${spec}"
    gh label create "${name}" --repo "${repo}" --color "${color}" --description "${description}" --force > /dev/null
  done
}

open_pr() {
  gh pr list --repo "${repo}" --head "${port_branch}" --base "${base_branch}" --state open \
    --json number --jq '.[0].number // empty'
}

escalate() {
  local pr=${PR_NUMBER:-} reason=${REASON:-the port workflow needs a person}
  if [[ ! "${pr}" =~ ^[0-9]+$ ]]; then
    port_die "publish-pr: --escalate needs PR_NUMBER"
  fi
  ensure_labels
  gh pr edit "${pr}" --repo "${repo}" --add-label port:needs-human > /dev/null
  printf 'The upstream-port workflow stopped: %s\n\nRun: %s\n\nRemove the port:needs-human label to let the workflow continue.\n' \
    "${reason}" "${run_url}" | port_sanitize > "${tmp}/comment.md"
  gh pr comment "${pr}" --repo "${repo}" --body-file "${tmp}/comment.md" > /dev/null
  printf 'publish-pr: escalated pull request #%s\n' "${pr}"
}

publish() {
  local dir=${1:?} final status target title pr labels_add labels_remove not_ported="" issue result is_draft
  local unported=0 log_debt="" debt="" unported_note=""
  local -a draft=()
  final=$(cat "${dir}/final-status" 2> /dev/null || printf 'unknown')
  target=$(jq -r '.target_sha // ""' "${dir}/plan.json")
  if [[ ! "${target}" =~ ^[0-9a-f]{40}$ ]]; then
    port_die "publish-pr: plan.json has no target commit"
  fi
  status=unknown
  for result in "${dir}/fix-result.json" "${dir}/port-result.json"; do
    if [[ -s "${result}" ]]; then
      status=$(jq -r '.overall_status // "unknown"' "${result}" 2> /dev/null || printf 'unknown')
      not_ported=$(jq -r '[.not_ported[]? | "- \(.go_path | tostring | gsub("[\r\n]"; " ")): \(.reason | tostring | gsub("[\r\n]"; " "))"]
        | join("\n") | gsub("~~~"; "~ ~ ~")' "${result}" 2> /dev/null || true)
      break
    fi
  done
  # This run's port log: commits left partial or blocked, including the ones no agent reported on,
  # and the commits a pin-only run advanced the pin over without porting.
  # Those without not_ported items of their own are added to the parity debt by name.
  if [[ -s "${dir}/port-log.jsonl" ]]; then
    unported=$(jq -s '[.[] | select(.status == "partial" or .status == "blocked" or .status == "pinned-not-ported")] | length' \
      "${dir}/port-log.jsonl" 2> /dev/null || printf '0')
    log_debt=$(jq -s -r '[.[] | select((.status == "partial" or .status == "blocked" or .status == "pinned-not-ported") and ((.not_ported // []) | length) == 0)
      | "- upstream \(.upstream | tostring | .[0:12]) \(.subject | tostring | gsub("[\r\n]"; " ")): \(.status | tostring)"
        + (if ((.notes // "") | tostring) != "" then " (\(.notes | tostring | gsub("[\r\n]"; " ")))" else "" end)]
      | join("\n") | gsub("~~~"; "~ ~ ~")' "${dir}/port-log.jsonl" 2> /dev/null || true)
  fi
  if [[ ! "${unported}" =~ ^[0-9]+$ ]]; then
    unported=0
  fi
  if [[ "${status}" == unknown && "${unported}" -gt 0 ]]; then
    status=blocked
  fi
  debt=$(printf '%s\n%s\n' "${not_ported}" "${log_debt}" | sed '/^$/d')
  if [[ "${unported}" -gt 0 ]]; then
    unported_note=" ${unported} upstream commit(s) are not fully ported; see the parity-debt issue."
  fi
  title="Upstream port: prebid/prebid-server through ${target:0:12}"
  port_sanitize < "${dir}/report.md" | head -c 60000 > "${tmp}/body.md"

  ensure_labels
  pr=$(open_pr)
  if [[ -z "${pr}" ]]; then
    if [[ "${final}" != pass ]]; then
      draft=(--draft)
    fi
    gh pr create --repo "${repo}" --base "${base_branch}" --head "${port_branch}" --title "${title}" \
      --body-file "${tmp}/body.md" "${draft[@]}" > /dev/null
    pr=$(open_pr)
    if [[ -z "${pr}" ]]; then
      port_die "publish-pr: the pull request was not created"
    fi
  else
    gh pr edit "${pr}" --repo "${repo}" --title "${title}" --body-file "${tmp}/body.md" > /dev/null
  fi

  labels_add=upstream-port
  labels_remove=""
  if [[ "${final}" == pass ]]; then
    labels_add+=",port:green"
    labels_remove="port:red"
  else
    labels_add+=",port:red"
    labels_remove="port:green"
  fi
  if [[ "${status}" == partial || "${status}" == blocked || "${unported}" -gt 0 ]]; then
    labels_add+=",port:partial"
  else
    labels_remove+=",port:partial"
  fi
  gh pr edit "${pr}" --repo "${repo}" --add-label "${labels_add}" --remove-label "${labels_remove}" > /dev/null
  is_draft=$(gh pr view "${pr}" --repo "${repo}" --json isDraft --jq '.isDraft')
  if [[ "${final}" == pass && "${is_draft}" == true ]]; then
    gh pr ready "${pr}" --repo "${repo}" > /dev/null
  elif [[ "${final}" != pass && "${is_draft}" != true ]]; then
    gh pr ready "${pr}" --repo "${repo}" --undo > /dev/null
  fi

  printf 'Port run finished: final gates **%s**, agent status **%s**, upstream through %s.%s\n\nRun: %s\n' \
    "${final}" "${status}" "${target:0:12}" "${unported_note}" "${run_url}" | port_sanitize > "${tmp}/comment.md"
  gh pr comment "${pr}" --repo "${repo}" --body-file "${tmp}/comment.md" > /dev/null

  if [[ -n "${debt}" ]]; then
    {
      printf 'Upstream port pull request #%s (through %s) leaves these Go behaviours unported, as the agent and the port log report them:\n\n' \
        "${pr}" "${target:0:12}"
      printf '~~~text\n%s\n~~~\n\nRun: %s\n' "${debt}" "${run_url}"
    } | port_sanitize | head -c 60000 > "${tmp}/debt.md"
    issue=$(gh issue list --repo "${repo}" --label parity-debt --state open --json number --jq '.[0].number // empty')
    if [[ -n "${issue}" ]]; then
      gh issue comment "${issue}" --repo "${repo}" --body-file "${tmp}/debt.md" > /dev/null
    else
      gh issue create --repo "${repo}" --title "Parity debt from upstream ports" --label parity-debt \
        --body-file "${tmp}/debt.md" > /dev/null
    fi
  fi
  printf 'publish-pr: pull request #%s updated (final gates %s, agent status %s)\n' "${pr}" "${final}" "${status}"
}

case "${1:-}" in
  --escalate) escalate ;;
  --result)
    if [[ -z "${2:-}" ]]; then
      port_die "publish-pr: --result needs a directory"
    fi
    publish "${2}"
    ;;
  *)
    printf 'usage: publish-pr.sh --result DIR | --escalate\n' >&2
    exit 2
    ;;
esac
