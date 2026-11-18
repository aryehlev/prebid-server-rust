#!/usr/bin/env bash
# render-report.sh: print the pull request report of a port run (Markdown) on stdout.
#
# Everything an agent wrote is shown as a quoted, unverified claim inside text blocks, so it can
# neither mention people nor format the page; only the gate results are authoritative. The report
# never carries AI attribution: lines that would are replaced (port_sanitize), and the publish job's
# guard checks the result again.
#
# Env: CTX ACTION RUN_MODE BASE_SHA TARGET_SHA RUN_URL FINAL_STATUS CLAUDE_RAN, and PORT_JSON
#      REVIEW_JSON FIX_JSON (structured outputs of the sessions, may be empty).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
ctx=${CTX:?}
action=${ACTION:?}
base=${BASE_SHA:-}
target=${TARGET_SHA:-}
final=${FINAL_STATUS:-unknown}

# json_or_null TEXT: TEXT when it is a JSON object, else null.
json_or_null() {
  local kind
  kind=$(jq -r 'type' <<< "${1:-null}" 2> /dev/null) || kind=invalid
  if [[ "${kind}" == object ]]; then
    jq -c '.' <<< "${1}"
  else
    printf 'null\n'
  fi
}

file_or_null() {
  if [[ -s "${1:?}" ]]; then
    jq -c '.' "${1}"
  else
    printf 'null\n'
  fi
}

port_result=$(json_or_null "${PORT_JSON:-}")
review_result=$(json_or_null "${REVIEW_JSON:-}")
fix_result=$(json_or_null "${FIX_JSON:-}")
before=$(file_or_null "${ctx}/gates-before.json")
after=$(file_or_null "${ctx}/gates-after.json")
final_gates=$(file_or_null "${ctx}/gates-final.json")
plan=$(file_or_null "${ctx}/plan.json")
log_lines='[]'
if [[ "${action}" == advance ]]; then
  count=$(jq -r '.commits | length' <<< "${plan}")
  if [[ "${count}" -gt 0 && -f rust/upstream-port-log.jsonl ]]; then
    log_lines=$(tail -n "${count}" rust/upstream-port-log.jsonl | jq -s -c '.')
  fi
fi
violations=""
if [[ -s "${ctx}/worktree-violations.txt" ]]; then
  violations=$(head -c 6000 "${ctx}/worktree-violations.txt")
fi
shadowed=""
if [[ -s "${ctx}/shadowed-upstream-paths.txt" ]]; then
  shadowed=$(head -n 100 "${ctx}/shadowed-upstream-paths.txt")
fi

report=$(jq -n -r \
  --argjson plan "${plan}" --argjson log "${log_lines}" \
  --argjson port "${port_result}" --argjson review "${review_result}" --argjson fix "${fix_result}" \
  --argjson before "${before}" --argjson after "${after}" --argjson final_gates "${final_gates}" \
  --arg action "${action}" --arg mode "${RUN_MODE:-auto}" --arg base "${base}" --arg target "${target}" \
  --arg final "${final}" --arg ran "${CLAUDE_RAN:-false}" --arg run "${RUN_URL:-}" \
  --arg violations "${violations}" --arg shadowed "${shadowed}" --arg fence "'''" '
  def code: tostring | gsub("[`|\r\n]"; " ") | "`\(.)`";
  def block($text): ($text | tostring | gsub("```"; $fence) | .[:8000]) as $t
    | "```text\n\($t)\n```";
  def status($g; $name): if $g == null then "-" else
    ([$g.checks[]? | select(.name == $name) | .status][0] // "-") end;
  def upstream($sha): "[`\($sha[0:12])`](https://github.com/prebid/prebid-server/commit/\($sha))";
  ($fix // $port) as $result
  | ($log | map({key: .upstream, value: .}) | from_entries) as $status
  | ([$before, $after, $final_gates] | map(select(. != null) | .checks[]?.name) | unique) as $gate_names
  | [
    "## Upstream port: prebid/prebid-server \($base[0:12])..\($target[0:12])",
    "",
    "- Action: `\($action)` (mode `\($mode)`); agent sessions ran: `\($ran)`",
    "- **Final gates: \($final)**" + (if $final_gates != null and ($final_gates.failed | length) > 0
        then " (failed: " + ($final_gates.failed | join(", ")) + ")" else "" end),
    (if $run != "" then "- Run: \($run)" else empty end),
    "",
    "### Upstream commits",
    "",
    "| Commit | Subject | Classes | Adapter dirs | Status |",
    "|---|---|---|---|---|",
    ($plan.commits[]? | "| \(upstream(.sha)) | \(.subject | code) | \(.classes | join(", ")) | \(.adapter_dirs | join(", ")) | \($status[.sha].status // "-") |"),
    "",
    "### Gates",
    "",
    "| Check | Before the agents | After the port | Final |",
    "|---|---|---|---|",
    ($gate_names[] as $n | "| \($n) | \(status($before; $n)) | \(status($after; $n)) | \(status($final_gates; $n)) |"),
    "",
    (if $result != null then
      "### Agent report (unverified claims; the gates above are authoritative)",
      "",
      "Overall status claimed: `\($result.overall_status // "unknown")`",
      "",
      block($result.summary // ""),
      (if ($result.not_ported // [] | length) > 0 then "", "**Not ported**", "",
        block($result.not_ported | map("- \(.go_path): \(.reason)") | join("\n")) else empty end),
      (if ($result.followups // [] | length) > 0 then "", "**Follow-ups**", "",
        block($result.followups | map("- \(.)") | join("\n")) else empty end),
      (if ($result.verification // [] | length) > 0 then "", "**Verification the agent reports**", "",
        block($result.verification | map("- [\(.result)] \(.command)") | join("\n")) else empty end),
      ""
     elif $ran == "true" then "### Agent report", "", "The agent sessions returned no structured result.", ""
     else empty end),
    (if $review != null then
      "### Adversarial review",
      "",
      "Blocking findings: \([$review.findings[]? | select(.severity == "blocking")] | length); minor: \([$review.findings[]? | select(.severity != "blocking")] | length)",
      "",
      block([$review.findings[]? | "- [\(.severity)] \(.title)\n  Go: \(.go_ref)\n  Rust: \(.rust_ref)"] | join("\n")),
      ""
     else empty end),
    (if $violations != "" then "### Work-tree corrections", "", block($violations), "" else empty end),
    (if $shadowed != "" then "### Upstream changes in fork-owned paths (not applied)", "", block($shadowed), "" else empty end),
    "### Reviewer checklist",
    "",
    "- [ ] **Merge with \"Create a merge commit\".** Squash or rebase merging drops the upstream ancestry the next sync depends on.",
    "- [ ] Every upstream commit above is ported, or its gap is recorded as parity debt.",
    "- [ ] New adversarial tests encode real Go behaviour.",
    "- [ ] Nothing outside `rust/` changed apart from the upstream merge."
  ] | .[]')
port_sanitize <<< "${report}" | head -c 60000
