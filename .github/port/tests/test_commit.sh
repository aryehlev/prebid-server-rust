#!/usr/bin/env bash
# commit.sh: the marker and port log for advance runs (ported, merge-only, no change needed), the
# repair message, the bot identity, no AI attribution, and nothing committed when nothing changed.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

repo="${T_ROOT}/repo"
t_repo "${repo}"
git -C "${repo}" switch -q -c port-work
mkdir -p "${repo}/rust"
old=1111111111111111111111111111111111111111
s1=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
s2=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
printf '# Last upstream commit ported to Rust.\n%s\n' "${old}" > "${repo}/rust/UPSTREAM_SYNC"
: > "${repo}/rust/upstream-port-log.jsonl"
t_put "${repo}" rust/crates/x/src/lib.rs '//! x'
start=$(t_commit "${repo}" "start")
ctx="${T_ROOT}/ctx"
mkdir -p "${ctx}"
jq -n --arg s1 "${s1}" --arg s2 "${s2}" '{commits: [
  {sha: $s1, subject: "New Adapter: Foo (#1)", classes: ["new-adapter"], adapter_dirs: ["foo"], needs_claude: true},
  {sha: $s2, subject: "Docs 🤖 Generated with [Claude Code]", classes: ["docs-infra"], adapter_dirs: [], needs_claude: false}]}' \
  > "${ctx}/plan.json"
printf '{"status": "fail", "failed": ["clippy", "test"], "checks": []}\n' > "${ctx}/gates-before.json"
bot=(GIT_AUTHOR_NAME="${T_BOT_NAME}" GIT_AUTHOR_EMAIL="${T_BOT_EMAIL}" GIT_COMMITTER_NAME="${T_BOT_NAME}"
  GIT_COMMITTER_EMAIL="${T_BOT_EMAIL}")
port_json=$(jq -n -c --arg s1 "${s1}" '{overall_status: "partial", summary: "ok",
  commits: [{sha: ($s1[0:12]), status: "partial", notes: "imp ext ported"}],
  rust_files_changed: [], verification: [],
  not_ported: [{sha: ($s1[0:9]), go_path: "adapters/foo/foo.go", reason: "video"}, {go_path: "exchange/x.go", reason: "general"}],
  followups: []}')

# run NAME [VAR=VALUE...]: run commit.sh in the repository at HEAD with the bot identity.
run() {
  local head
  head=$(git -C "${repo}" rev-parse HEAD)
  shift
  t_run_in "${repo}" env "${bot[@]}" CTX="${ctx}" MERGED_SHA="${head}" BASE_SHA="${old}" TARGET_SHA="${s2}" \
    RUN_URL=https://example.invalid/runs/1 "$@" "${T_SCRIPTS}/commit.sh"
}

# log_field N JQ: JQ applied to the N-th line from the end of the port log.
log_field() {
  tail -n "${1:?}" "${repo}/rust/upstream-port-log.jsonl" | head -n 1 | jq -c "${2:?}"
}

t_put "${repo}" rust/crates/x/src/lib.rs '//! x ported'
run advance ACTION=advance RUN_MODE=auto PORT_JSON="${port_json}"
t_eq "advance: exit status" 0 "${T_RC}"
marker=$(cat "${repo}/rust/UPSTREAM_SYNC")
expected_marker=$(printf '# Last upstream commit ported to Rust.\n%s' "${s2}")
t_eq "advance: the marker moves and keeps its header" "${expected_marker}" "${marker}"
message=$(git -C "${repo}" log -1 --format=%B)
subject=$(git -C "${repo}" log -1 --format=%s)
t_eq "advance: subject" "port: New Adapter: Foo (#1) (+1 more)" "${subject}"
t_has "advance: per-commit status" "${message}" "upstream aaaaaaaaaaaa New Adapter: Foo (#1) (partial)"
t_has "advance: no-change status" "${message}" "upstream bbbbbbbbbbbb [removed: AI attribution]"
t_has "advance: range trailer" "${message}" "Upstream-Range: 111111111111..bbbbbbbbbbbb"
t_has "advance: run trailer" "${message}" "Port-Run: https://example.invalid/runs/1"
t_lacks "advance: no robot emoji" "${message}" "🤖"
t_lacks "advance: no Co-Authored-By" "${message}" "Co-Authored-By"
identity=$(git -C "${repo}" log -1 --format='%an <%ae> / %cn <%ce>')
t_eq "advance: bot identity" "${T_BOT_NAME} <${T_BOT_EMAIL}> / ${T_BOT_NAME} <${T_BOT_EMAIL}>" "${identity}"
first=$(log_field 2 '[.upstream, .status, .notes]')
t_eq "advance: log line of the ported commit" "[\"${s1}\",\"partial\",\"imp ext ported\"]" "${first}"
not_ported=$(log_field 2 '.not_ported')
t_eq "advance: not-ported items attach to their commit or to every commit" \
  '[{"go_path":"adapters/foo/foo.go","reason":"video"},{"go_path":"exchange/x.go","reason":"general"}]' "${not_ported}"
second=$(log_field 1 '[.upstream, .status, .run]')
t_eq "advance: log line of a commit that needs no agent" "[\"${s2}\",\"no_change_needed\",\"https://example.invalid/runs/1\"]" "${second}"
second_np=$(log_field 1 '.not_ported')
t_eq "advance: a commit gets only the items without a sha" '[{"go_path":"exchange/x.go","reason":"general"}]' "${second_np}"
second_subject=$(log_field 1 '.subject')
t_eq "advance: the port log carries no attribution" '"[removed: AI attribution]"' "${second_subject}"

# No Rust change and no agent result: the marker still advances.
s3=cccccccccccccccccccccccccccccccccccccccc
jq -n --arg s3 "${s3}" '{commits: [{sha: $s3, subject: "foo: fixture", classes: ["adapter-fixture"], adapter_dirs: ["foo"], needs_claude: false}]}' \
  > "${ctx}/plan.json"
run marker ACTION=advance RUN_MODE=auto TARGET_SHA="${s3}"
t_eq "marker only: exit status" 0 "${T_RC}"
subject=$(git -C "${repo}" log -1 --format=%s)
t_eq "marker only: subject" "port: advance upstream marker to cccccccccccc (no Rust changes needed)" "${subject}"
status=$(log_field 1 '.status')
t_eq "marker only: status" '"no_change_needed"' "${status}"

# An agent that reported nothing for a commit that needed it.
jq -n --arg s3 "${s3}" '{commits: [{sha: $s3, subject: "foo: change", classes: ["adapter-change"], adapter_dirs: ["foo"], needs_claude: true}]}' \
  > "${ctx}/plan.json"
run silent ACTION=advance RUN_MODE=auto PORT_JSON='not json'
status=$(log_field 1 '[.status, .notes]')
t_eq "silent agent: blocked with a note" '["blocked","the agent reported no result for this commit"]' "${status}"

# Merge-only runs record the commits but keep the marker.
before_marker=$(cat "${repo}/rust/UPSTREAM_SYNC")
run mergeonly ACTION=advance RUN_MODE=merge-only
t_eq "merge-only: exit status" 0 "${T_RC}"
after_marker=$(cat "${repo}/rust/UPSTREAM_SYNC")
t_eq "merge-only: the marker stays" "${before_marker}" "${after_marker}"
subject=$(git -C "${repo}" log -1 --format=%s)
t_eq "merge-only: subject" "port: record upstream 111111111111..bbbbbbbbbbbb as merged, not ported" "${subject}"
status=$(log_field 1 '.status')
t_eq "merge-only: status" '"merged-not-ported"' "${status}"

# Repairs change Rust only.
t_put "${repo}" rust/crates/x/src/lib.rs '//! x repaired'
log_lines=$(wc -l < "${repo}/rust/upstream-port-log.jsonl")
run repair ACTION=repair RUN_MODE=auto
subject=$(git -C "${repo}" log -1 --format=%s)
t_eq "repair: subject" "port-repair: fix clippy, test" "${subject}"
log_after=$(wc -l < "${repo}/rust/upstream-port-log.jsonl")
t_eq "repair: the port log is unchanged" "${log_lines}" "${log_after}"
# A repair that changes nothing is recorded anyway: plan.sh counts port-repair commits, so without
# this commit a repair that finds nothing would be planned every day and never escalate.
head=$(git -C "${repo}" rev-parse HEAD)
run nothing ACTION=repair RUN_MODE=auto
t_eq "repair without changes: exit status" 0 "${T_RC}"
subject=$(git -C "${repo}" log -1 --format=%s)
t_eq "repair without changes: an empty commit records the attempt" \
  "port-repair: no change found for clippy, test" "${subject}"
parent=$(git -C "${repo}" rev-parse HEAD^)
t_eq "repair without changes: exactly one new commit" "${head}" "${parent}"
files=$(git -C "${repo}" show --name-only --format= HEAD)
t_eq "repair without changes: the commit changes no file" "" "${files}"
identity=$(git -C "${repo}" log -1 --format='%an <%ae>')
t_eq "repair without changes: bot identity" "${T_BOT_NAME} <${T_BOT_EMAIL}>" "${identity}"

# An advance run with nothing to record commits nothing.
cp "${ctx}/plan.json" "${ctx}/plan.saved.json"
jq -n '{commits: []}' > "${ctx}/plan.json"
head=$(git -C "${repo}" rev-parse HEAD)
run emptyplan ACTION=advance RUN_MODE=merge-only
t_has "advance with nothing to record: nothing is committed" "${T_OUT}" "commit: nothing to commit"
head_after=$(git -C "${repo}" rev-parse HEAD)
t_eq "advance with nothing to record: HEAD stays" "${head}" "${head_after}"
mv "${ctx}/plan.saved.json" "${ctx}/plan.json"

# Only rust/ is staged.
t_put "${repo}" outside.txt 'outside'
t_put "${repo}" rust/crates/x/src/lib.rs '//! x repaired again'
run outside ACTION=repair RUN_MODE=auto
files=$(git -C "${repo}" show --name-only --format= HEAD)
t_eq "only rust/ is committed" "rust/crates/x/src/lib.rs" "${files}"
rm -f "${repo}/outside.txt"

t_run_in "${repo}" env "${bot[@]}" CTX="${ctx}" MERGED_SHA="${start}" BASE_SHA="${old}" TARGET_SHA="${s2}" \
  ACTION=repair "${T_SCRIPTS}/commit.sh"
t_eq "HEAD other than MERGED_SHA is refused" 1 "${T_RC}"

t_done
