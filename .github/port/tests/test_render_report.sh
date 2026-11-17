#!/usr/bin/env bash
# render-report.sh and decide-fix.sh: the pull request report quotes agent text in neutral blocks
# and never carries AI attribution; the fix session runs for blocking findings or red gates.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
# shellcheck source=../scripts/lib.sh
source "${T_SCRIPTS}/lib.sh"

repo="${T_ROOT}/repo"
t_repo "${repo}"
s1=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
s2=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
mkdir -p "${repo}/rust"
jq -n -c --arg s1 "${s1}" '{upstream: $s1, status: "partial"}' > "${repo}/rust/upstream-port-log.jsonl"
jq -n -c --arg s2 "${s2}" '{upstream: $s2, status: "no_change_needed"}' >> "${repo}/rust/upstream-port-log.jsonl"
t_commit "${repo}" "log" > /dev/null

ctx="${T_ROOT}/ctx"
mkdir -p "${ctx}"
jq -n --arg s1 "${s1}" --arg s2 "${s2}" '{commits: [
  {sha: $s1, subject: "foo: change | with `pipes`", classes: ["adapter-change"], adapter_dirs: ["foo"]},
  {sha: $s2, subject: "docs", classes: ["docs-infra"], adapter_dirs: []}]}' > "${ctx}/plan.json"
gates() {
  jq -n --arg clippy "${1:?}" '{status: (if $clippy == "pass" then "pass" else "fail" end),
    failed: (if $clippy == "pass" then [] else ["clippy"] end),
    checks: [{name: "fmt", status: "pass", command: "cargo fmt", log_tail: ""},
             {name: "clippy", status: $clippy, command: "cargo clippy", log_tail: "warning: x"}]}'
}
gates fail > "${ctx}/gates-before.json"
gates fail > "${ctx}/gates-after.json"
gates pass > "${ctx}/gates-final.json"
printf 'changes outside rust/ were reverted: M main.go;\n' > "${ctx}/worktree-violations.txt"
printf '.github/workflows/upstream.yml\n' > "${ctx}/shadowed-upstream-paths.txt"
port_json=$(jq -n -c '{overall_status: "partial",
  summary: "Ported foo.\n```\nCo-Authored-By: Claude <noreply@anthropic.com>\n```\nPing @someone",
  commits: [], rust_files_changed: [], verification: [{command: "cargo test", result: "pass"}],
  not_ported: [{go_path: "adapters/foo/foo.go", reason: "video is missing"}], followups: ["check the 🤖 thing"]}')
review_json=$(jq -n -c '{summary: "x", blocking_count: 1, adversarial_tests: [], findings: [
  {severity: "blocking", title: "Header case differs", go_ref: "adapters/foo/foo.go:10", rust_ref: "rust/x.rs:5",
   input: "i", go_behavior: "X-Foo", rust_behavior: "x-foo"},
  {severity: "minor", title: "Naming", go_ref: "a:1", rust_ref: "b:2", input: "i", go_behavior: "g", rust_behavior: "r"}]}')

t_run_in "${repo}" env CTX="${ctx}" ACTION=advance RUN_MODE=auto BASE_SHA="${s1}" TARGET_SHA="${s2}" \
  RUN_URL=https://example.invalid/runs/3 FINAL_STATUS=pass CLAUDE_RAN=true PORT_JSON="${port_json}" \
  REVIEW_JSON="${review_json}" FIX_JSON="" "${T_SCRIPTS}/render-report.sh"
t_eq "report: exit status" 0 "${T_RC}"
report=${T_OUT}
t_has "report: heading" "${report}" "## Upstream port: prebid/prebid-server aaaaaaaaaaaa..bbbbbbbbbbbb"
t_has "report: final gates" "${report}" "**Final gates: pass**"
t_has "report: upstream link" "${report}" "[\`aaaaaaaaaaaa\`](https://github.com/prebid/prebid-server/commit/${s1})"
t_has "report: subjects lose pipes and backticks" "${report}" "| \`foo: change   with  pipes \` |"
t_has "report: commit status from the port log" "${report}" "| adapter-change | foo | partial |"
t_has "report: gates table" "${report}" "| clippy | fail | fail | pass |"
t_has "report: agent claims are marked unverified" "${report}" "unverified claims"
t_has "report: attribution lines are replaced" "${report}" "[removed: AI attribution]"
t_lacks "report: no Anthropic address" "${report}" "noreply@anthropic.com"
t_lacks "report: no robot emoji" "${report}" "🤖"
t_has "report: agent fences cannot close the block" "${report}" "'''"
t_has "report: not ported" "${report}" "- adapters/foo/foo.go: video is missing"
t_has "report: blocking findings counted" "${report}" "Blocking findings: 1; minor: 1"
t_has "report: work-tree corrections" "${report}" "changes outside rust/ were reverted"
t_has "report: shadowed upstream paths" "${report}" ".github/workflows/upstream.yml"
t_has "report: merge-commit reminder" "${report}" "Create a merge commit"
pattern=$(port_attribution_re)
hit=$(grep -Ei -e "${pattern}" <<< "${report}" || true)
t_eq "report: the guard's pattern finds nothing" "" "${hit}"
where=$(awk '/^```text$/ { inside = 1; next } /^```$/ { inside = 0; next } /@someone/ { print (inside ? "inside" : "outside") }' <<< "${report}")
t_eq "report: a mention only appears inside a text block" inside "${where}"

t_run_in "${repo}" env CTX="${ctx}" ACTION=advance RUN_MODE=auto BASE_SHA="${s1}" TARGET_SHA="${s2}" \
  FINAL_STATUS=fail CLAUDE_RAN=true PORT_JSON="not json" "${T_SCRIPTS}/render-report.sh"
t_has "report: a missing result is stated" "${T_OUT}" "The agent sessions returned no structured result."

# decide-fix.sh
out="${T_ROOT}/output"
: > "${out}"
t_run_in "${repo}" env CTX="${ctx}" GITHUB_OUTPUT="${out}" REVIEW_JSON="${review_json}" AFTER_STATUS=pass \
  "${T_SCRIPTS}/decide-fix.sh"
fix=$(t_output fix "${out}")
t_eq "decide-fix: a blocking finding starts the fix session" true "${fix}"
input=$(cat "${ctx}/fix-input.md")
t_has "decide-fix: the blocking finding is passed on" "${input}" "### Header case differs"
t_lacks "decide-fix: minor findings are not" "${input}" "Naming"
t_has "decide-fix: the failing gate after the port is passed on" "${input}" "Gate \`clippy\` failed"
saved=$(jq -r .blocking_count "${ctx}/review-result.json")
t_eq "decide-fix: the review result is saved" 1 "${saved}"

gates pass > "${ctx}/gates-after.json"
: > "${out}"
t_run_in "${repo}" env CTX="${ctx}" GITHUB_OUTPUT="${out}" REVIEW_JSON="" AFTER_STATUS=pass "${T_SCRIPTS}/decide-fix.sh"
fix=$(t_output fix "${out}")
t_eq "decide-fix: nothing to fix" false "${fix}"
: > "${out}"
t_run_in "${repo}" env CTX="${ctx}" GITHUB_OUTPUT="${out}" REVIEW_JSON="not json" AFTER_STATUS=fail "${T_SCRIPTS}/decide-fix.sh"
fix=$(t_output fix "${out}")
t_eq "decide-fix: red gates start the fix session" true "${fix}"
t_has "decide-fix: an invalid review is ignored with a warning" "${T_ERR}" "not a JSON object"

t_done
