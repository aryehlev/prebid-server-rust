#!/usr/bin/env bash
# merge-upstream.sh: the merge makes every non-fork path equal to the upstream commit, keeps the
# fork-owned paths, records upstream changes to fork-owned paths, never conflicts, and sanitizes
# the upstream subject it quotes.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

dir="${T_ROOT}/repos"
t_fork_fixture "${dir}"
up="${dir}/upstream"
fork="${dir}/fork"

# Upstream: Go changes, a new adapter, a deleted file whose name has spaces, a changed upstream
# workflow and AGENTS.md (both fork-owned paths), and a change to a file the fork also changed.
t_put "${up}" main.go 'package main // v2'
t_put "${up}" adapters/bar/bar.go 'package bar'
t_put "${up}" adapters/bar/bartest/exemplary/x.json '{}'
git -C "${up}" rm -q "docs/a file.md"
t_put "${up}" .github/workflows/upstream.yml 'name: upstream v2'
t_put "${up}" AGENTS.md 'upstream agents v2'
u2=$(t_commit "${up}" "Add the bar adapter")
t_put "${up}" README.md 'readme v3'
u3=$(t_commit "${up}" "Robot subject 🤖 Generated with [Claude Code](https://claude.ai/code)")
t_sync_upstream "${dir}"
before=$(git -C "${fork}" rev-parse HEAD)

ctx="${T_ROOT}/ctx"
t_run_in "${fork}" env CTX="${ctx}" "${T_SCRIPTS}/merge-upstream.sh" "${u2}"
t_eq "merge exits 0" 0 "${T_RC}"
t_has "merge reports the merge" "${T_OUT}" "merged ${u2:0:12}"
parents=$(git -C "${fork}" rev-list --parents -n 1 HEAD)
merge=$(git -C "${fork}" rev-parse HEAD)
t_eq "merge commit has the fork and upstream as parents" "${merge} ${before} ${u2}" "${parents}"
nonfork=$(git -C "${fork}" diff --name-only "${u2}" HEAD -- ':(top)' ':(top,exclude)rust' ':(top,exclude).github' \
  ':(top,exclude).planning' ':(top,exclude)AGENTS.md' ':(top,exclude)CLAUDE.md')
t_eq "non-fork paths equal upstream" "" "${nonfork}"
forked=$(git -C "${fork}" diff --name-only "${before}" HEAD -- rust .github AGENTS.md)
t_eq "fork-owned paths are unchanged" "" "${forked}"
deleted=$(git -C "${fork}" ls-tree --name-only HEAD -- "docs/a file.md")
t_eq "the upstream deletion of a file with spaces is applied" "" "${deleted}"
added=$(git -C "${fork}" ls-tree --name-only HEAD -- adapters/bar/bar.go)
t_eq "the new upstream adapter is added" "adapters/bar/bar.go" "${added}"
workflows=$(git -C "${fork}" ls-tree -r --name-only HEAD -- .github/workflows)
t_eq "upstream workflows stay out of the fork" "" "${workflows}"
shadowed=$(cat "${ctx}/shadowed-upstream-paths.txt")
t_has "upstream's workflow change is recorded as shadowed" "${shadowed}" ".github/workflows/upstream.yml"
t_has "upstream's AGENTS.md change is recorded as shadowed" "${shadowed}" "AGENTS.md"
message=$(git -C "${fork}" log -1 --format=%B HEAD)
t_has "merge subject names the upstream commit" "${message}" "Merge upstream prebid/prebid-server@${u2:0:12}"
t_has "merge body quotes the upstream subject" "${message}" "Upstream head: Add the bar adapter"
status=$(git -C "${fork}" status --porcelain)
t_eq "the work tree is clean after the merge" "" "${status}"

head_before=$(git -C "${fork}" rev-parse HEAD)
t_run_in "${fork}" "${T_SCRIPTS}/merge-upstream.sh" "${u2}"
t_eq "merging the same commit again exits 0" 0 "${T_RC}"
t_has "merging the same commit again reports it" "${T_OUT}" "already merged"
head_after=$(git -C "${fork}" rev-parse HEAD)
t_eq "merging the same commit again changes nothing" "${head_before}" "${head_after}"

t_put "${fork}" main.go 'dirty'
t_run_in "${fork}" "${T_SCRIPTS}/merge-upstream.sh" "${u3}"
t_eq "a dirty work tree is refused" 1 "${T_RC}"
t_has "a dirty work tree is named" "${T_ERR}" "uncommitted changes"
git -C "${fork}" checkout -q -- main.go

t_run_in "${fork}" "${T_SCRIPTS}/merge-upstream.sh" "${u3}"
t_eq "the next upstream commit merges" 0 "${T_RC}"
message=$(git -C "${fork}" log -1 --format=%B HEAD)
t_has "an attributed upstream subject is replaced" "${message}" "Upstream head: [removed: AI attribution]"
t_lacks "the quoted subject carries no robot emoji" "${message}" "🤖"
readme=$(cat "${fork}/README.md")
t_eq "the later upstream change is applied" "readme v3" "${readme}"

# The fork changed a non-fork path by mistake: the merge restores upstream's content instead of
# conflicting.
t_put "${fork}" exchange/exchange.go 'package exchange // fork edit'
t_commit "${fork}" "Edit Go by mistake" > /dev/null
t_put "${up}" exchange/exchange.go 'package exchange // v4'
u4=$(t_commit "${up}" "Change the exchange")
t_sync_upstream "${dir}"
t_run_in "${fork}" "${T_SCRIPTS}/merge-upstream.sh" "${u4}"
t_eq "a fork edit of Go never conflicts" 0 "${T_RC}"
exchange=$(cat "${fork}/exchange/exchange.go")
t_eq "upstream's Go wins over a fork edit" "package exchange // v4" "${exchange}"

# Untrusted agent configuration carried by upstream (a .claude/, .mcp.json, hook or ripgrep config)
# must never reach the work tree the port agent session runs in, or it would be loaded and run with
# the job's credentials. The merge drops every such path, from the index and from disk.
t_put "${up}" .claude/settings.json '{"x":1}'
t_put "${up}" .claude/settings.local.json '{"x":2}'
t_put "${up}" .mcp.json '{"mcpServers":{}}'
t_put "${up}" .ripgreprc '--pre=sh'
t_put "${up}" .husky/pre-commit 'run'
t_put "${up}" main.go 'package main // v5'
u5=$(t_commit "${up}" "Upstream change that also ships agent config")
t_sync_upstream "${dir}"
t_run_in "${fork}" "${T_SCRIPTS}/merge-upstream.sh" "${u5}"
t_eq "merging upstream that ships agent config exits 0" 0 "${T_RC}"
tracked_cfg=$(git -C "${fork}" ls-files -- .claude .claude.json .claude.local.json .mcp.json .ripgreprc .husky .omx)
t_eq "no agent configuration is tracked after the merge" "" "${tracked_cfg}"
present_cfg=""
for p in .claude .mcp.json .claude.local.json .ripgreprc .husky; do
  if [[ -e "${fork}/${p}" ]]; then present_cfg+="${p} "; fi
done
t_eq "no agent configuration is left in the work tree" "" "${present_cfg}"
main_v5=$(cat "${fork}/main.go")
t_eq "the benign upstream change still lands" "package main // v5" "${main_v5}"
status=$(git -C "${fork}" status --porcelain)
t_eq "the work tree is clean after dropping agent config" "" "${status}"

t_done
