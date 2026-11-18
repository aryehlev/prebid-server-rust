#!/usr/bin/env bash
# guard.sh: every repository invariant on a fake upstream (read from its mirror) and a fake
# repository in this repository's layout: no upstream file (top-level paths, Go outside the
# tooling modules, .upstream, symbolic links, submodules, go.work, vendored code, byte-identical
# copies, the allowed copies), the pin, the known-failures list, AI attribution (with and without
# the devtools "Generated with" line), the bot rules and the parity-harness exemption.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

dir="${T_ROOT}/repos"
t_fork_fixture "${dir}"
up="${dir}/upstream"
fork="${dir}/fork"
guard="${T_SCRIPTS}/guard.sh"

# An upstream commit with the exact trailers upstream commit 066bbe70 carries; upstream history
# lives in the mirror only, so the guard never reads its messages.
t_put "${up}" adapters/foo/foo.go 'package foo // v2'
: > "${up}/static/.gitkeep"
u2=$(t_commit "${up}" "$(printf 'Module: Scope3 Real-Time Data (#4397)\n\n🤖 Generated with [Claude Code](https://claude.ai/code)\n\nCo-Authored-By: Claude <noreply@anthropic.com>')")
t_sync_upstream "${dir}"
f0=$(git -C "${fork}" rev-parse f0)
u2_version=$("${T_SCRIPTS}/pin-version.sh" "${u2}")
t_run_in "${fork}" "${T_SCRIPTS}/advance-pin.sh" "${u2}" "${u2_version}"
t_eq "fixture: advance the pin" 0 "${T_RC}"
main=$(t_commit "${fork}" "port: advance upstream pin to ${u2:0:12}")
git -C "${fork}" update-ref refs/remotes/origin/main "${main}"

t_run_in "${fork}" "${guard}" --ref main --base "${f0}"
t_eq "a clean repository passes" 0 "${T_RC}"
t_has "a clean repository passes with main rules" "${T_OUT}" "guard: ok for ${main:0:12} (main rules, 1 first-parent commit(s) checked)"
t_run_in "${fork}" env -u UPSTREAM_GIT "${guard}" --ref main --base "${f0}"
t_eq "the upstream mirror is required" 1 "${T_RC}"
t_run_in "${fork}" env -u UPSTREAM_GIT "${guard}" --ref main --base "${f0}" --upstream "${dir}/upstream.git"
t_eq "--upstream names the mirror" 0 "${T_RC}"

# branch NAME [START]: start a new branch from START (default main).
branch() {
  git -C "${fork}" switch -q -C "${1:?}" "${2:-${main}}"
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

# Upstream files.
branch top-go
cp "${up}/main.go" "${fork}/main.go"
ref=$(t_commit "${fork}" "Bring in the upstream main package")
check "an upstream file at the top level" 1 "top-level paths outside the allowed set (lib.sh port_top_level_paths): main.go" \
  --ref "${ref}" --base "${main}"
t_has "an upstream file at the top level: the copy is named" "${T_ERR}" "files byte-identical to an upstream file"
branch crate-go
t_put "${fork}" crates/x/src/gen.go 'package x'
ref=$(t_commit "${fork}" "Go next to the Rust code")
check "Go code outside the tooling modules" 1 "Go code outside the Go tooling modules" --ref "${ref}" --base "${main}"
branch tooling-go
t_put "${fork}" crates/x/testdata/go/dump/dump.go 'package dump'
t_put "${fork}" crates/devtools/go/tool.go 'package tool'
ref=$(t_commit "${fork}" "Go tooling")
check "Go code in the tooling modules" 0 "guard: ok" --ref "${ref}" --base "${main}"
branch parity
t_put "${fork}" parity/cmd/parity/main.go 'package main // parity'
ref=$(t_commit "${fork}" "The parity harness")
check "parity/ outside the parity-harness branch" 1 "top-level paths outside the allowed set (lib.sh port_top_level_paths): parity" \
  --ref "${ref}" --base "${main}"
check "parity/ on the parity-harness branch" 0 "guard: ok" --ref "${ref}" --base "${main}" --parity-harness
branch dot-upstream
mkdir -p "${fork}/.upstream"
cp "${up}/go.mod" "${fork}/.upstream/go.mod"
git -C "${fork}" add -f .upstream/go.mod
ref=$(t_commit "${fork}" "Commit the upstream checkout")
check "a committed .upstream path" 1 "outside the allowed set (lib.sh port_top_level_paths): .upstream" --ref "${ref}" --base "${main}"
rm -rf "${fork}/.upstream"
branch symlink
ln -s ../../.upstream "${fork}/crates/x/upstream"
ref=$(t_commit "${fork}" "Link to the upstream checkout")
check "a symbolic link" 1 "symbolic links and submodules are not allowed: crates/x/upstream (120000)" --ref "${ref}" --base "${main}"
branch submodule
git -C "${fork}" update-index --add --cacheinfo "160000,${u2},crates/x/upstream"
printf '[submodule "upstream"]\n\tpath = crates/x/upstream\n\turl = https://github.com/prebid/prebid-server.git\n' > "${fork}/.gitmodules"
git -C "${fork}" add .gitmodules
git -C "${fork}" commit -q -m "Add upstream as a submodule"
ref=$(git -C "${fork}" rev-parse HEAD)
check "a submodule" 1 "crates/x/upstream (160000)" --ref "${ref}" --base "${main}"
t_has "a submodule: .gitmodules is named" "${T_ERR}" "a .gitmodules, go.work or vendor/ directory brings in code from elsewhere: .gitmodules"
git -C "${fork}" rm -q --cached crates/x/upstream
rm -f "${fork}/.gitmodules"
branch vendor
t_put "${fork}" crates/x/testdata/go/vendor/github.com/prebid/prebid-server/v4/exchange/exchange.go 'package exchange // vendored'
t_put "${fork}" crates/x/testdata/go/vendor/modules.txt '# github.com/prebid/prebid-server/v4'
ref=$(t_commit "${fork}" "Vendor the upstream module")
check "a vendored module" 1 "crates/x/testdata/go/vendor/modules.txt" --ref "${ref}" --base "${main}"
branch gowork
t_put "${fork}" crates/x/testdata/go/go.work 'go 1.25.0'
ref=$(t_commit "${fork}" "A Go workspace")
check "a go.work file" 1 "brings in code from elsewhere: crates/x/testdata/go/go.work" --ref "${ref}" --base "${main}"
branch copy
cp "${up}/adapters/foo/footest/exemplary/a.json" "${fork}/testdata/foo-a.json"
ref=$(t_commit "${fork}" "Copy an upstream fixture")
check "a renamed copy of an upstream file" 1 "byte-identical to an upstream file (upstream code is read from the Go root, never copied; lib.sh port_upstream_copies lists the exceptions, each for the bytes of its upstream file): testdata/foo-a.json" \
  --ref "${ref}" --base "${main}"
branch empty
: > "${fork}/testdata/.gitkeep"
ref=$(t_commit "${fork}" "An empty file, as upstream has one")
check "an empty file" 0 "guard: ok" --ref "${ref}" --base "${main}"
# The pin of the merge-base counts too: a change that advances the pin may not keep a copy of a
# file the old pin had.
branch old-copy "${f0}"
t_advance "${fork}" "${u2}"
t_put "${fork}" crates/x/testdata/go/foo.go 'package foo // v1'
ref=$(t_commit "${fork}" "Advance the pin and keep the old foo")
check "a copy of a file of the old pin" 1 "crates/x/testdata/go/foo.go" --ref "${ref}" --base "${f0}"
# Any version in upstream history counts, not only the pinned trees: neither main's pin (u2) nor
# this change's has foo.go v1 any more.
branch history-copy
t_put "${fork}" crates/x/testdata/go/foo.go 'package foo // v1'
ref=$(t_commit "${fork}" "Keep a copy of an old upstream foo")
check "a copy of an older upstream version that no pin has" 1 \
  "its upstream file): crates/x/testdata/go/foo.go" --ref "${ref}" --base "${main}"
# An allowed copy covers the bytes of its own upstream file, not whatever is put at its path.
branch license-other
cp "${up}/main.go" "${fork}/LICENSE"
ref=$(t_commit "${fork}" "Put another upstream file at the license path")
check "an allowed path holding another upstream file" 1 "its upstream file): LICENSE" \
  --ref "${ref}" --base "${main}"
branch license-pixel
mkdir -p "${fork}/crates/util/src/httputil"
cp "${up}/exchange/exchange.go" "${fork}/crates/util/src/httputil/pixel.png"
ref=$(t_commit "${fork}" "Put upstream code at the pixel path")
check "an allowed path whose upstream file never had these bytes" 1 \
  "its upstream file): crates/util/src/httputil/pixel.png" --ref "${ref}" --base "${main}"

# Pin rules.
branch pin-missing
git -C "${fork}" rm -q UPSTREAM_SYNC
ref=$(t_commit "${fork}" "Drop the pin")
check "a missing pin" 1 "must hold exactly one line that is not a # comment" --ref "${ref}" --base "${main}"
branch pin-two
printf '%s\n%s\n' "${T_U1}" "${u2}" > "${fork}/UPSTREAM_SYNC"
ref=$(t_commit "${fork}" "Two pins")
check "two pins" 1 "must hold exactly one line that is not a # comment" --ref "${ref}" --base "${main}"
branch pin-unknown
printf '%s\n' "${f0}" > "${fork}/UPSTREAM_SYNC"
ref=$(t_commit "${fork}" "Pin a commit upstream does not have")
check "a pin that is not upstream" 1 "which is not in the upstream mirror" --ref "${ref}" --base "${main}"
branch pin-back
printf '%s\n' "${T_U1}" > "${fork}/UPSTREAM_SYNC"
ref=$(t_commit "${fork}" "Move the pin back")
check "a pin that moves backwards" 1 "moved backwards" --ref "${ref}" --base "${main}"
git -C "${up}" switch -q -c side "${T_U1}"
t_put "${up}" side.go 'package main // side'
side_sha=$(t_commit "${up}" "An upstream side branch")
git -C "${up}" tag side-tag
git -C "${up}" switch -q master
t_put "${up}" main.go 'package main // v3'
u3=$(t_commit "${up}" "Upstream moves on")
t_sync_upstream "${dir}"
branch pin-side
printf '%s\n' "${side_sha}" > "${fork}/UPSTREAM_SYNC"
ref=$(t_commit "${fork}" "Pin a tagged side commit")
check "a pin that is not on the upstream branch" 1 "which is not on upstream master" --ref "${ref}" --base "${main}"
branch pin-ahead
printf '%s\n' "${u3}" > "${fork}/UPSTREAM_SYNC"
ref=$(t_commit "${fork}" "Advance the pin")
check "a pin that moves forward" 0 "guard: ok" --ref "${ref}" --base "${main}"

# Known-failures list rules.
branch known-unsorted
printf '# known\nadapters/foo/footest/exemplary/b.json\nadapters/foo/footest/exemplary/a.json\n' \
  > "${fork}/conformance/known_failures.txt"
ref=$(t_commit "${fork}" "Unsorted known failures")
check "an unsorted known-failures list" 1 "is not sorted" --ref "${ref}" --base "${main}"
branch known-missing
printf 'adapters/foo/footest/exemplary/zz.json\n' > "${fork}/conformance/known_failures.txt"
ref=$(t_commit "${fork}" "List a fixture that does not exist")
check "a known failure that does not exist" 1 "lists files the pinned upstream tree does not have" --ref "${ref}" --base "${main}"
branch known-strict
printf 'adapters/foo/footest/exemplary/a.json\n' > "${fork}/conformance/known_failures.txt"
ref=$(t_commit "${fork}" "List a foo fixture")
check "a known failure for an untouched adapter" 0 "guard: ok" --ref "${ref}" --base "${main}"
check "a known failure for a touched adapter" 1 "[known-strict]" --ref "${ref}" --base "${main}" --strict-dirs foo,bar

# Attribution.
branch crate-change
t_put "${fork}" crates/x/src/lib.rs '//! x v2'
t_put "${fork}" .github/port/README.md 'tooling v2'
ref=$(t_commit "${fork}" $'Port the foo change\n\nGenerated with `cargo run -p pbs-devtools -- all`.')
check "a change with the devtools generator line" 0 "guard: ok" --ref "${ref}" --base "${main}"
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
  t_put "${fork}" crates/x/src/lib.rs "//! ${name}"
  ref=$(t_commit "${fork}" "${message}")
  check "a commit with attribution (${name})" 1 "[attribution]" --ref "${ref}" --base "${main}"
done
branch author
t_put "${fork}" crates/x/src/lib.rs '//! by claude'
ref=$(t_commit_as "${fork}" "Claude" "noreply@anthropic.com" "Port foo")
check "a commit authored as Claude" 1 "is authored or committed as 'Claude'" --ref "${ref}" --base "${main}"
branch committer
t_put "${fork}" crates/x/src/lib.rs '//! by claude bot'
ref=$(t_commit_as "${fork}" "claude[bot]" "claude[bot]@users.noreply.github.com" "Port foo")
check "a commit by claude[bot]" 1 "[attribution]" --ref "${ref}" --base "${main}"
# CI skip markers: GitHub runs no workflow on a head commit that carries one, so nothing would check
# it. Only the head counts, as only the head counts for GitHub.
for case in \
  "skip ci|Port foo [skip ci]" \
  "ci skip in capitals|Port foo [CI SKIP]" \
  "no ci|Port foo [no ci]" \
  "skip actions|Port foo [skip actions]" \
  "actions skip|Port foo [actions skip]" \
  "skip-checks trailer|$(printf 'Port foo\n\n\nskip-checks: true')"; do
  name=${case%%|*}
  message=${case#*|}
  branch skip
  t_put "${fork}" crates/x/src/lib.rs "//! ${name}"
  ref=$(t_commit "${fork}" "${message}")
  check "a head commit with a CI skip marker (${name})" 1 "[ci-skip]" --ref "${ref}" --base "${main}"
done
branch skip-earlier
t_put "${fork}" crates/x/src/lib.rs '//! skipped'
t_commit "${fork}" "Port foo [skip ci]" > /dev/null
t_put "${fork}" crates/x/src/lib.rs '//! checked'
ref=$(t_commit "${fork}" "Port foo (skip ci) after all")
check "a CI skip marker before the head, and a defused one at the head" 0 "guard: ok" --ref "${ref}" --base "${main}"
printf 'Upstream port\n\nAll fine.\n' > "${T_ROOT}/pr-ok.txt"
printf 'Upstream port\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)\n' > "${T_ROOT}/pr-bad.txt"
check "clean pull request text" 0 "guard: ok" --ref main --base "${f0}" --pr-text "${T_ROOT}/pr-ok.txt"
check "pull request text with attribution" 1 "the pull request title or body carries AI attribution" \
  --ref main --base "${f0}" --pr-text "${T_ROOT}/pr-bad.txt"

branch local-state
t_put "${fork}" .claude/settings.local.json '{}'
git -C "${fork}" add -f .claude/settings.local.json
ref=$(t_commit "${fork}" "Commit local tool state")
check "tracked .claude state" 1 "[local-state]" --ref "${ref}" --base "${main}"
rm -rf "${fork}/.claude"
for cfg in .mcp.json .claude.json .claude.local.json .ripgreprc .husky/pre-commit .omx/state.json; do
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
t_put "${fork}" crates/x/src/lib.rs '//! ported'
t_put "${fork}" testdata/generated.json '{"generated": true}'
ref=$(bot_commit "port: Add the foo change")
check "a bot commit in the writable paths" 0 "guard: ok for ${ref:0:12} (bot rules" --ref "${ref}" --base "${main}" --bot
branch bot-pin
t_advance "${fork}" "${u3}"
printf '{"upstream":"%s","status":"no_change_needed"}\n' "${u3}" >> "${fork}/upstream-port-log.jsonl"
ref=$(bot_commit "port: advance upstream pin to ${u3:0:12} (no Rust changes needed)")
check "a bot commit advancing the pin" 0 "guard: ok" --ref "${ref}" --base "${main}" --bot
branch bot-github
t_put "${fork}" .github/port/README.md 'changed by the bot'
ref=$(bot_commit "port: touch tooling")
check "a bot commit outside the writable paths" 1 "touches paths the bot may not change: .github/port/README.md" \
  --ref "${ref}" --base "${main}" --bot
branch bot-human
t_put "${fork}" crates/x/src/lib.rs '//! human fix'
ref=$(t_commit "${fork}" "Human fix on the port branch")
check "a human commit in publish mode" 1 "is not the bot's" --ref "${ref}" --base "${main}" --bot
check "a human commit on the port pull request" 0 "guard: ok" --ref "${ref}" --base "${main}" --bot --allow-human
for protected in Cargo.toml scripts/a.sh scripts/fetch-upstream.sh rust-toolchain.toml PORTING.md README.md \
  LICENSE deny.toml taplo.toml clippy.toml .clippy.toml .cargo/config.toml rust-toolchain .rustfmt.toml \
  patches/README.md conformance/README.md .gitignore; do
  branch bot-protected
  t_put "${fork}" "${protected}" 'changed'
  ref=$(t_commit "${fork}" "Change ${protected}")
  check "a change to ${protected} on the port branch" 1 "[bot-protected]" --ref "${ref}" --base "${main}" --bot --allow-human
done
branch bot-deps
printf '\n[[package]]\nname = "left-pad"\nversion = "1.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n' \
  >> "${fork}/Cargo.lock"
ref=$(bot_commit "port: add a crate")
check "a bot commit adding an external crate" 1 "[bot-deps]" --ref "${ref}" --base "${main}" --bot
branch bot-known
printf 'adapters/foo/footest/exemplary/a.json\n' > "${fork}/conformance/known_failures.txt"
ref=$(bot_commit "port: hide a failure")
check "a bot commit adding a known failure" 1 "[bot-known]" --ref "${ref}" --base "${main}" --bot

# bot-log: the bot appends to the port log and never rewrites what the default branch has.
branch log-main
printf '{"upstream":"%s","status":"blocked"}\n' "${u2}" > "${fork}/upstream-port-log.jsonl"
log_main=$(t_commit "${fork}" "Main has a port log line")
git -C "${fork}" update-ref refs/remotes/origin/main "${log_main}"
branch log-append "${log_main}"
printf '{"upstream":"%s","status":"ported"}\n' "${u2}" >> "${fork}/upstream-port-log.jsonl"
ref=$(bot_commit "port: append to the log")
check "a bot commit appending to the port log" 0 "guard: ok" --ref "${ref}" --base "${log_main}" --bot
branch log-rewrite "${log_main}"
printf '{"upstream":"%s","status":"ported"}\n' "${u2}" > "${fork}/upstream-port-log.jsonl"
ref=$(bot_commit "port: rewrite the log")
check "a bot commit rewriting the port log" 1 "[bot-log]" --ref "${ref}" --base "${log_main}" --bot
branch log-delete "${log_main}"
git -C "${fork}" rm -q upstream-port-log.jsonl
ref=$(bot_commit "port: drop the log")
check "a bot commit deleting the port log" 1 "[bot-log]" --ref "${ref}" --base "${log_main}" --bot
git -C "${fork}" update-ref refs/remotes/origin/main "${main}"
branch side
t_put "${fork}" crates/x/src/side.rs '//! side'
side=$(t_commit "${fork}" "Side branch work")
branch bot-merge
git -C "${fork}" merge -q --no-ff -m "Merge a side branch" "${side}"
ref=$(git -C "${fork}" rev-parse HEAD)
check "a bot-mode merge of a side branch" 1 "which is not on main" --ref "${ref}" --base "${main}" --bot --allow-human

# A port branch that merged a newer main is fine in bot mode.
branch port
t_put "${fork}" crates/x/src/lib.rs '//! port work'
bot_commit "port: work" > /dev/null
git -C "${fork}" switch -q main
t_put "${fork}" crates/x/src/other.rs '//! main moved on'
main2=$(t_commit "${fork}" "Main moves on")
git -C "${fork}" update-ref refs/remotes/origin/main "${main2}"
git -C "${fork}" switch -q port
env GIT_AUTHOR_NAME="${T_BOT_NAME}" GIT_AUTHOR_EMAIL="${T_BOT_EMAIL}" GIT_COMMITTER_NAME="${T_BOT_NAME}" \
  GIT_COMMITTER_EMAIL="${T_BOT_EMAIL}" git -C "${fork}" merge -q --no-ff -m "Merge main into port/upstream-sync" "${main2}"
ref=$(git -C "${fork}" rev-parse HEAD)
check "a port branch that merged main" 0 "guard: ok" --ref "${ref}" --base "${main}" --bot

# Upstream changes its LICENSE: the copy the port ships may stay at the older text (a version its
# upstream file had) or take the new one, while the old text at another path is still a copy.
# Upstream also adds two fixtures and removes them again, so only its history has them.
git -C "${up}" switch -q master
t_put "${up}" static/tiny.json 'null'
t_put "${up}" static/old.json '{"an old upstream fixture": true}'
t_commit "${up}" "Add two fixtures" > /dev/null
rm -f "${up}/static/tiny.json" "${up}/static/old.json"
t_put "${up}" LICENSE 'Apache License, Version 2.0 (test copy, reworded)'
u4=$(t_commit "${up}" "Reword the license; drop the fixtures")
t_sync_upstream "${dir}"
branch history-tiny
t_put "${fork}" testdata/null.json 'null'
ref=$(t_commit "${fork}" "A tiny golden that an old upstream fixture also held")
check "a file under 16 bytes that only upstream history has" 0 "guard: ok" --ref "${ref}" --base "${main}"
branch history-removed
t_put "${fork}" testdata/old.json '{"an old upstream fixture": true}'
ref=$(t_commit "${fork}" "A copy of a removed upstream fixture")
check "a copy of a file upstream has removed" 1 "its upstream file): testdata/old.json" --ref "${ref}" --base "${main}"
branch license-old
t_advance "${fork}" "${u4}"
ref=$(t_commit "${fork}" "Advance the pin over the license change")
check "the allowed copy at an older version of its upstream file" 0 "guard: ok" --ref "${ref}" --base "${main}"
cp "${up}/LICENSE" "${fork}/LICENSE"
ref=$(t_commit "${fork}" "Take the new license text")
check "the allowed copy at the new version" 0 "guard: ok" --ref "${ref}" --base "${main}"
git -C "${fork}" show "${main}:LICENSE" > "${fork}/testdata/LICENSE.old"
ref=$(t_commit "${fork}" "Keep the old license text elsewhere")
check "an older upstream license at another path" 1 "its upstream file): testdata/LICENSE.old" \
  --ref "${ref}" --base "${main}"

# The attribution pattern matches the real upstream messages, which the guard never reads: they
# stay in the upstream mirror.
# shellcheck source=../scripts/lib.sh
source "${T_SCRIPTS}/lib.sh"
pattern=$(port_attribution_re)
real_git=$(t_real_upstream)
for real in 0b91679f 066bbe70; do
  if [[ -n "${real_git}" ]] && git --git-dir="${real_git}" cat-file -e "${real}^{commit}" 2> /dev/null; then
    body=$(git --git-dir="${real_git}" log -1 --format=%B "${real}")
    hit=$(grep -Ei -m 1 -e "${pattern}" <<< "${body}" || true)
    if [[ -n "${hit}" ]]; then
      t_ok "the attribution pattern matches upstream ${real}"
    else
      t_fail "the attribution pattern does not match upstream ${real}"
    fi
  else
    t_fail "upstream commit ${real} is missing; set PORT_TESTS_UPSTREAM_GIT to an upstream mirror (see run-all.sh)"
  fi
done

t_done
