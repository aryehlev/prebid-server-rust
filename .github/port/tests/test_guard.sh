#!/usr/bin/env bash
# guard.sh: every fork invariant on fake upstream and fork histories, including upstream commits
# that carry Claude trailers (never flagged), fork commits with and without AI attribution, the
# devtools "Generated with" line, the bot rules and the parity-harness exemption.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

dir="${T_ROOT}/repos"
t_fork_fixture "${dir}"
up="${dir}/upstream"
fork="${dir}/fork"
guard="${T_SCRIPTS}/guard.sh"

# An upstream commit with the exact trailers upstream commit 066bbe70 carries.
t_put "${up}" adapters/foo/foo.go 'package foo // v2'
u2=$(t_commit "${up}" "$(printf 'Module: Scope3 Real-Time Data (#4397)\n\n🤖 Generated with [Claude Code](https://claude.ai/code)\n\nCo-Authored-By: Claude <noreply@anthropic.com>')")
t_sync_upstream "${dir}"
f0=$(git -C "${fork}" rev-parse f0)
t_run_in "${fork}" "${T_SCRIPTS}/merge-upstream.sh" "${u2}"
t_eq "fixture: upstream merge" 0 "${T_RC}"
printf '# Last upstream commit ported to Rust.\n%s\n' "${u2}" > "${fork}/rust/UPSTREAM_SYNC"
main=$(t_commit "${fork}" "port: record upstream ${u2:0:12}")
git -C "${fork}" update-ref refs/remotes/origin/main "${main}"

t_run_in "${fork}" "${guard}" --ref main --base "${f0}"
t_eq "a clean fork passes" 0 "${T_RC}"
t_has "a clean fork passes with main rules" "${T_OUT}" "guard: ok for ${main:0:12} (main rules, 2 first-parent commit(s) checked)"

# branch NAME: start a new branch from main.
branch() {
  git -C "${fork}" switch -q -C "${1:?}" "${main}"
}

# check NAME EXPECTED_RC NEEDLE ARGS...: run the guard and check its status and output.
check() {
  local name=${1:?} expected=${2:?} needle=${3?}
  shift 3
  t_run_in "${fork}" "${guard}" "$@"
  t_eq "${name}: exit status" "${expected}" "${T_RC}"
  if [[ -n "${needle}" ]]; then
    t_has "${name}: message" "${T_OUT}${T_ERR}" "${needle}"
  fi
}

# Upstream history with Claude trailers is never checked, even when the range includes the
# upstream merge.
check "upstream trailers are not flagged" 0 "guard: ok" --ref main --base "${f0}"

branch go-edit
t_put "${fork}" main.go 'package main // fork edit'
ref=$(t_commit "${fork}" "Edit upstream Go")
check "a Go change outside the fork paths" 1 "[go-tree]" --ref "${ref}" --base "${main}"
check "the parity harness may change Go" 0 "guard: ok" --ref "${ref}" --base "${main}" --parity-harness

branch rust-change
t_put "${fork}" rust/crates/x/src/lib.rs '//! x v2'
t_put "${fork}" .github/port/README.md 'fork tooling v2'
ref=$(t_commit "${fork}" $'Port the foo change\n\nGenerated with `cargo run -p pbs-devtools -- all`.')
check "a fork change with the devtools generator line" 0 "guard: ok" --ref "${ref}" --base "${main}"

for case in \
  "co-authored-by|$(printf 'Port foo\n\nCo-Authored-By: Claude <noreply@anthropic.com>')" \
  "co-authored-by lower case|$(printf 'Port foo\n\nco-authored-by: claude opus <someone@example.com>')" \
  "generated with claude code|$(printf 'Port foo\n\nGenerated with [Claude Code](https://claude.com/claude-code)')" \
  "robot emoji|$(printf 'Port foo\n\n🤖 made by a robot')" \
  "session link|$(printf 'Port foo\n\nhttps://claude.ai/code/session_0123')" \
  "session trailer|$(printf 'Port foo\n\nClaude-Session: abc')" \
  "anthropic noreply address|$(printf 'Port foo\n\nThanks noreply@anthropic.com')"; do
  name=${case%%|*}
  message=${case#*|}
  branch "attr"
  t_put "${fork}" rust/crates/x/src/lib.rs "//! ${name}"
  ref=$(t_commit "${fork}" "${message}")
  check "fork commit with attribution (${name})" 1 "[attribution]" --ref "${ref}" --base "${main}"
done

branch author
t_put "${fork}" rust/crates/x/src/lib.rs '//! by claude'
ref=$(t_commit_as "${fork}" "Claude" "noreply@anthropic.com" "Port foo")
check "a commit authored as Claude" 1 "is authored or committed as 'Claude'" --ref "${ref}" --base "${main}"
branch committer
t_put "${fork}" rust/crates/x/src/lib.rs '//! by claude bot'
ref=$(t_commit_as "${fork}" "claude[bot]" "claude[bot]@users.noreply.github.com" "Port foo")
check "a commit by claude[bot]" 1 "[attribution]" --ref "${ref}" --base "${main}"

printf 'Upstream port\n\nAll fine.\n' > "${T_ROOT}/pr-ok.txt"
printf 'Upstream port\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)\n' > "${T_ROOT}/pr-bad.txt"
check "clean pull request text" 0 "guard: ok" --ref main --base "${f0}" --pr-text "${T_ROOT}/pr-ok.txt"
check "pull request text with attribution" 1 "the pull request title or body carries AI attribution" \
  --ref main --base "${f0}" --pr-text "${T_ROOT}/pr-bad.txt"

# Marker rules.
branch marker-missing
git -C "${fork}" rm -q rust/UPSTREAM_SYNC
ref=$(t_commit "${fork}" "Drop the marker")
check "a missing marker" 1 "names no upstream commit" --ref "${ref}" --base "${main}"
branch marker-fork
printf '%s\n' "${f0}" > "${fork}/rust/UPSTREAM_SYNC"
ref=$(t_commit "${fork}" "Point the marker at a fork commit")
check "a marker that is not upstream" 1 "is not an upstream commit" --ref "${ref}" --base "${main}"
branch marker-back
u1=$(git -C "${fork}" rev-parse u1)
printf '%s\n' "${u1}" > "${fork}/rust/UPSTREAM_SYNC"
ref=$(t_commit "${fork}" "Move the marker back")
check "a marker that moves backwards" 1 "moved backwards" --ref "${ref}" --base "${main}"
t_put "${up}" main.go 'package main // v3'
u3=$(t_commit "${up}" "Upstream change not merged yet")
t_sync_upstream "${dir}"
branch marker-ahead
printf '%s\n' "${u3}" > "${fork}/rust/UPSTREAM_SYNC"
ref=$(t_commit "${fork}" "Point the marker past the merge")
check "a marker that is not merged" 1 "is not merged into" --ref "${ref}" --base "${main}"

# Known-failures list rules.
branch known-unsorted
printf '# known\nadapters/foo/footest/exemplary/b.json\nadapters/foo/footest/exemplary/a.json\n' \
  > "${fork}/rust/conformance/known_failures.txt"
ref=$(t_commit "${fork}" "Unsorted known failures")
check "an unsorted known-failures list" 1 "is not sorted" --ref "${ref}" --base "${main}"
branch known-missing
printf 'adapters/foo/footest/exemplary/zz.json\n' > "${fork}/rust/conformance/known_failures.txt"
ref=$(t_commit "${fork}" "List a fixture that does not exist")
check "a known failure that does not exist" 1 "lists files that do not exist" --ref "${ref}" --base "${main}"
branch known-strict
printf 'adapters/foo/footest/exemplary/a.json\n' > "${fork}/rust/conformance/known_failures.txt"
ref=$(t_commit "${fork}" "List a foo fixture")
check "a known failure for an untouched adapter" 0 "guard: ok" --ref "${ref}" --base "${main}"
check "a known failure for a touched adapter" 1 "[known-strict]" --ref "${ref}" --base "${main}" --strict-dirs foo,bar

branch local-state
t_put "${fork}" .claude/settings.local.json '{}'
ref=$(t_commit "${fork}" "Commit local tool state")
check "tracked .claude state" 1 "[local-state]" --ref "${ref}" --base "${main}"
for cfg in .mcp.json .claude.json .claude.local.json .ripgreprc .husky/pre-commit; do
  branch local-state
  t_put "${fork}" "${cfg}" 'agent config'
  ref=$(t_commit "${fork}" "Commit ${cfg}")
  check "tracked agent config ${cfg}" 1 "[local-state]" --ref "${ref}" --base "${main}"
done

# Bot rules.
bot_commit() {
  t_commit_as "${fork}" "${T_BOT_NAME}" "${T_BOT_EMAIL}" "${1:?}"
}
branch bot-ok
t_put "${fork}" rust/crates/x/src/lib.rs '//! ported'
ref=$(bot_commit "port: Add the foo change")
check "a bot commit inside rust/" 0 "guard: ok for ${ref:0:12} (bot rules" --ref "${ref}" --base "${main}" --bot
branch bot-github
t_put "${fork}" .github/port/README.md 'changed by the bot'
ref=$(bot_commit "port: touch tooling")
check "a bot commit outside rust/" 1 "touches paths outside rust/" --ref "${ref}" --base "${main}" --bot
branch bot-human
t_put "${fork}" rust/crates/x/src/lib.rs '//! human fix'
ref=$(t_commit "${fork}" "Human fix on the port branch")
check "a human commit in publish mode" 1 "is not the bot's" --ref "${ref}" --base "${main}" --bot
check "a human commit on the port pull request" 0 "guard: ok" --ref "${ref}" --base "${main}" --bot --allow-human
for protected in rust/Cargo.toml rust/scripts/a.sh rust/rust-toolchain.toml rust/PORTING.md rust/legacy/old.rs; do
  branch bot-protected
  t_put "${fork}" "${protected}" 'changed'
  ref=$(bot_commit "port: change ${protected}")
  check "a bot change to ${protected}" 1 "[bot-protected]" --ref "${ref}" --base "${main}" --bot
done
branch bot-deps
printf '\n[[package]]\nname = "left-pad"\nversion = "1.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n' \
  >> "${fork}/rust/Cargo.lock"
ref=$(bot_commit "port: add a crate")
check "a bot commit adding an external crate" 1 "[bot-deps]" --ref "${ref}" --base "${main}" --bot
branch bot-known
printf 'adapters/foo/footest/exemplary/a.json\n' > "${fork}/rust/conformance/known_failures.txt"
ref=$(bot_commit "port: hide a failure")
check "a bot commit adding a known failure" 1 "[bot-known]" --ref "${ref}" --base "${main}" --bot
branch side
t_put "${fork}" rust/crates/x/src/side.rs '//! side'
side=$(t_commit "${fork}" "Side branch work")
branch bot-merge
git -C "${fork}" merge -q --no-ff -m "Merge a side branch" "${side}"
ref=$(git -C "${fork}" rev-parse HEAD)
check "a bot-mode merge of a side branch" 1 "which is neither upstream nor main" --ref "${ref}" --base "${main}" --bot --allow-human

# A port branch that merged a newer main is fine in bot mode.
branch port
t_put "${fork}" rust/crates/x/src/lib.rs '//! port work'
bot_commit "port: work" > /dev/null
git -C "${fork}" switch -q main
t_put "${fork}" rust/crates/x/src/other.rs '//! main moved on'
main2=$(t_commit "${fork}" "Main moves on")
git -C "${fork}" update-ref refs/remotes/origin/main "${main2}"
git -C "${fork}" switch -q port
env GIT_AUTHOR_NAME="${T_BOT_NAME}" GIT_AUTHOR_EMAIL="${T_BOT_EMAIL}" GIT_COMMITTER_NAME="${T_BOT_NAME}" \
  GIT_COMMITTER_EMAIL="${T_BOT_EMAIL}" git -C "${fork}" merge -q --no-ff -m "Merge main into port/upstream-sync" "${main2}"
ref=$(git -C "${fork}" rev-parse HEAD)
check "a port branch that merged main" 0 "guard: ok" --ref "${ref}" --base "${main}" --bot

# The attribution pattern matches the real upstream messages, which only the upstream exclusion
# keeps out of the check.
# shellcheck source=../scripts/lib.sh
source "${T_SCRIPTS}/lib.sh"
pattern=$(port_attribution_re)
for real in 0b91679f 066bbe70; do
  if git -C "${T_TOP}" cat-file -e "${real}^{commit}" 2> /dev/null; then
    body=$(git -C "${T_TOP}" log -1 --format=%B "${real}")
    hit=$(grep -Ei -m 1 -e "${pattern}" <<< "${body}" || true)
    if [[ -n "${hit}" ]]; then
      t_ok "the attribution pattern matches upstream ${real}"
    else
      t_fail "the attribution pattern does not match upstream ${real}"
    fi
  else
    t_fail "upstream commit ${real} is missing; run the tests in a clone with full history"
  fi
done

# go-tree does not flag upstream agent configuration that merge-upstream dropped: the agent-config
# paths are excluded from the non-fork pathspec, so the fork lacking upstream's .mcp.json/.claude is
# not counted as a difference from the merged upstream commit. Without the exclusion this would be a
# go-tree violation (the paths would look deleted), so the case fails if the exclusion regresses.
git -C "${fork}" switch -q -C cfg-drop "${main}"
t_put "${up}" .mcp.json '{"mcpServers":{"x":{"command":"sh"}}}'
t_put "${up}" .claude/settings.json '{"permissions":{"allow":["Bash"]}}'
t_put "${up}" main.go 'package main // cfg'
ucfg=$(t_commit "${up}" "Upstream ships agent config")
t_sync_upstream "${dir}"
t_run_in "${fork}" "${T_SCRIPTS}/merge-upstream.sh" "${ucfg}"
t_eq "fixture: merge upstream carrying agent config" 0 "${T_RC}"
printf '# Last upstream commit ported to Rust.\n%s\n' "${ucfg}" > "${fork}/rust/UPSTREAM_SYNC"
cfgref=$(t_commit "${fork}" "port: record upstream ${ucfg:0:12}")
check "go-tree ignores dropped upstream agent config" 0 "guard: ok" --ref "${cfgref}" --base "${main}"
tracked_cfg=$(git -C "${fork}" ls-files -- .mcp.json .claude)
t_eq "the dropped upstream agent config is not tracked on the port branch" "" "${tracked_cfg}"

t_done
