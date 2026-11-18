#!/usr/bin/env bash
# commit.sh: the pin (UPSTREAM_SYNC and the Go tooling modules' upstream version) and the port log
# for advance runs (ported, no change needed, pin-only), the repair message, the bot identity, no
# AI attribution, only the writable paths and the records staged, and nothing committed when
# nothing changed.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# ver SHA: a pseudo-version of SHA (commit.sh only checks its form).
ver() {
  printf 'v4.0.1-0.20260101000000-%s\n' "${1:0:12}"
}

repo="${T_ROOT}/repo"
t_repo "${repo}"
git -C "${repo}" switch -q -c port-work
old=1111111111111111111111111111111111111111
s1=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
s2=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
s3=cccccccccccccccccccccccccccccccccccccccc
s4=dddddddddddddddddddddddddddddddddddddddd
header='# The upstream commit the port is pinned to.'
printf '%s\n%s\n' "${header}" "${old}" > "${repo}/UPSTREAM_SYNC"
: > "${repo}/upstream-port-log.jsonl"
t_put "${repo}" crates/x/src/lib.rs '//! x'
gomod=crates/x/testdata/go/go.mod
mkdir -p "${repo}/crates/x/testdata/go"
old_version=$(ver "${old}")
t_go_mod crates/x/testdata/go "${old_version}" > "${repo}/${gomod}"
t_put "${repo}" README.md 'readme'
t_commit "${repo}" "start" > /dev/null
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

# run NAME TARGET [VAR=VALUE...]: run commit.sh in the repository at HEAD with the bot identity,
# for the range old..TARGET.
run() {
  local head target=${2:?} version
  head=$(git -C "${repo}" rev-parse HEAD)
  version=$(ver "${target}")
  shift 2
  t_run_in "${repo}" env "${bot[@]}" CTX="${ctx}" START_SHA="${head}" BASE_SHA="${old}" TARGET_SHA="${target}" \
    PIN_VERSION="${version}" RUN_URL=https://example.invalid/runs/1 "$@" "${T_SCRIPTS}/commit.sh"
}

# log_field N JQ: JQ applied to the N-th line from the end of the port log.
log_field() {
  tail -n "${1:?}" "${repo}/upstream-port-log.jsonl" | head -n 1 | jq -c "${2:?}"
}

# required: the upstream requirement of the tooling module.
required() {
  grep '^require' "${repo}/${gomod}"
}

t_put "${repo}" crates/x/src/lib.rs '//! x ported'
run advance "${s2}" ACTION=advance RUN_MODE=auto PORT_JSON="${port_json}"
t_eq "advance: exit status" 0 "${T_RC}"
pin=$(cat "${repo}/UPSTREAM_SYNC")
t_eq "advance: the pin moves and keeps its header" "${header}"$'\n'"${s2}" "${pin}"
requirement=$(required)
s2_version=$(ver "${s2}")
t_eq "advance: the tooling module requires the new pin" \
  "require github.com/prebid/prebid-server/v4 ${s2_version}" "${requirement}"
files=$(git -C "${repo}" show --name-only --format= HEAD)
t_eq "advance: the code, the module, the pin and the log are committed" \
  $'UPSTREAM_SYNC\ncrates/x/src/lib.rs\ncrates/x/testdata/go/go.mod\nupstream-port-log.jsonl' "${files}"
message=$(git -C "${repo}" log -1 --format=%B)
subject=$(git -C "${repo}" log -1 --format=%s)
t_eq "advance: subject" "port: New Adapter: Foo (#1) (+1 more)" "${subject}"
t_has "advance: per-commit status" "${message}" "upstream aaaaaaaaaaaa New Adapter: Foo (#1) (partial)"
t_has "advance: no-change status" "${message}" "upstream bbbbbbbbbbbb [removed: AI attribution]"
t_has "advance: range trailer" "${message}" "Upstream-Range: 111111111111..bbbbbbbbbbbb"
t_has "advance: version trailer" "${message}" "Upstream-Version: ${s2_version}"
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

# No Rust change and no agent result: the pin still advances, and a go.mod that only changes the
# upstream version is not a Rust change.
jq -n --arg s3 "${s3}" '{commits: [{sha: $s3, subject: "foo: fixture", classes: ["adapter-fixture"], adapter_dirs: ["foo"], needs_claude: false}]}' \
  > "${ctx}/plan.json"
run pin "${s3}" ACTION=advance RUN_MODE=auto
t_eq "pin only: exit status" 0 "${T_RC}"
subject=$(git -C "${repo}" log -1 --format=%s)
t_eq "pin only: subject" "port: advance upstream pin to cccccccccccc (no Rust changes needed)" "${subject}"
status=$(log_field 1 '.status')
t_eq "pin only: status" '"no_change_needed"' "${status}"
requirement=$(required)
s3_version=$(ver "${s3}")
t_has "pin only: the module follows the pin" "${requirement}" "${s3_version}"

# An agent that reported nothing for a commit that needed it.
jq -n --arg s3 "${s3}" '{commits: [{sha: $s3, subject: "foo: change", classes: ["adapter-change"], adapter_dirs: ["foo"], needs_claude: true}]}' \
  > "${ctx}/plan.json"
run silent "${s3}" ACTION=advance RUN_MODE=auto PORT_JSON='not json'
status=$(log_field 1 '[.status, .notes]')
t_eq "silent agent: blocked with a note" '["blocked","the agent reported no result for this commit"]' "${status}"
subject=$(git -C "${repo}" log -1 --format=%s)
t_eq "silent agent: the subject does not claim that nothing was needed" \
  "port: advance upstream pin to cccccccccccc (no Rust changes; 1 commit(s) not ported)" "${subject}"

# A go.mod change beyond the upstream version is a change of the tooling.
jq -n --arg s4 "${s4}" '{commits: [{sha: $s4, subject: "Bump a dependency", classes: ["dependency"], adapter_dirs: [], needs_claude: true}]}' \
  > "${ctx}/plan.json"
printf 'require github.com/stretchr/testify v1.8.1\n' >> "${repo}/${gomod}"
run gomod "${s4}" ACTION=advance RUN_MODE=auto
subject=$(git -C "${repo}" log -1 --format=%s)
t_eq "a go.mod change is a code change" "port: Bump a dependency" "${subject}"

# Pin-only runs advance the pin and log every commit as pinned, not ported.
s5=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee
jq -n --arg s5 "${s5}" '{commits: [{sha: $s5, subject: "exchange: change", classes: ["core"], adapter_dirs: [], needs_claude: true}]}' \
  > "${ctx}/plan.json"
run pinonly "${s5}" ACTION=advance RUN_MODE=pin-only
t_eq "pin-only: exit status" 0 "${T_RC}"
pin=$(sed -n '$p' "${repo}/UPSTREAM_SYNC")
t_eq "pin-only: the pin advances" "${s5}" "${pin}"
subject=$(git -C "${repo}" log -1 --format=%s)
t_eq "pin-only: subject" "port: advance upstream pin to eeeeeeeeeeee without porting (pin-only run)" "${subject}"
status=$(log_field 1 '[.status, .notes, .not_ported]')
t_eq "pin-only: status" '["pinned-not-ported","",[]]' "${status}"

# Repairs change Rust only.
t_put "${repo}" crates/x/src/lib.rs '//! x repaired'
log_lines=$(wc -l < "${repo}/upstream-port-log.jsonl")
pin_before=$(cat "${repo}/UPSTREAM_SYNC")
run repair "${s5}" ACTION=repair RUN_MODE=auto
subject=$(git -C "${repo}" log -1 --format=%s)
t_eq "repair: subject" "port-repair: fix clippy, test" "${subject}"
log_after=$(wc -l < "${repo}/upstream-port-log.jsonl")
t_eq "repair: the port log is unchanged" "${log_lines}" "${log_after}"
pin_after=$(cat "${repo}/UPSTREAM_SYNC")
t_eq "repair: the pin is unchanged" "${pin_before}" "${pin_after}"
# A repair that changes nothing is recorded anyway: plan.sh counts port-repair commits, so without
# this commit a repair that finds nothing would be planned every day and never escalate.
head=$(git -C "${repo}" rev-parse HEAD)
run nothing "${s5}" ACTION=repair RUN_MODE=auto
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
run emptyplan "${s5}" ACTION=advance RUN_MODE=auto
t_has "advance with nothing to record: nothing is committed" "${T_OUT}" "commit: nothing to commit"
head_after=$(git -C "${repo}" rev-parse HEAD)
t_eq "advance with nothing to record: HEAD stays" "${head}" "${head_after}"
mv "${ctx}/plan.saved.json" "${ctx}/plan.json"

# Only the writable paths and the records are staged.
t_put "${repo}" outside.txt 'outside'
t_put "${repo}" README.md 'readme changed'
t_put "${repo}" crates/x/src/lib.rs '//! x repaired again'
run outside "${s5}" ACTION=repair RUN_MODE=auto
files=$(git -C "${repo}" show --name-only --format= HEAD)
t_eq "only the writable paths are committed" "crates/x/src/lib.rs" "${files}"
rm -f "${repo}/outside.txt"
git -C "${repo}" checkout -q -- README.md

head=$(git -C "${repo}" rev-parse HEAD)
t_run_in "${repo}" env "${bot[@]}" CTX="${ctx}" START_SHA="${head}" BASE_SHA="${old}" TARGET_SHA="${s5}" \
  ACTION=advance RUN_MODE=auto "${T_SCRIPTS}/commit.sh"
t_fails "an advance needs PIN_VERSION"
t_run_in "${repo}" env "${bot[@]}" CTX="${ctx}" START_SHA="${head}" BASE_SHA="${old}" TARGET_SHA="${s5}" \
  PIN_VERSION="${old_version}" ACTION=advance RUN_MODE=auto "${T_SCRIPTS}/commit.sh"
t_fails "a version of another commit is refused"
start=$(git -C "${repo}" rev-parse HEAD~2)
t_run_in "${repo}" env "${bot[@]}" CTX="${ctx}" START_SHA="${start}" BASE_SHA="${old}" TARGET_SHA="${s5}" \
  ACTION=repair "${T_SCRIPTS}/commit.sh"
t_eq "HEAD other than START_SHA is refused" 1 "${T_RC}"

t_done
