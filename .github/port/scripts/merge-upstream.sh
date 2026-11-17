#!/usr/bin/env bash
# merge-upstream.sh TARGET: merge the upstream commit TARGET into HEAD so that every path the fork
# does not own (.github/port/fork-paths.txt) becomes byte-identical to TARGET, while the fork-owned
# paths keep HEAD's content. The result is a real two-parent merge commit, so later merge-base
# computations see TARGET as merged. It never conflicts.
#
# Upstream changes to fork-owned paths (for example upstream's .github/) are not applied. When CTX
# is set they are listed in ${CTX}/shadowed-upstream-paths.txt for the pull request report.
# The commit identity comes from GIT_AUTHOR_* / GIT_COMMITTER_* or the git configuration.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

if [[ "$#" -ne 1 ]]; then
  printf 'usage: merge-upstream.sh TARGET\n' >&2
  exit 2
fi
port_cd_top
target=$(git rev-parse --verify --end-of-options "${1}^{commit}")
fork_listing=$(port_fork_paths)
mapfile -t fork <<< "${fork_listing}"
spec_listing=$(port_nonfork_spec)
mapfile -t nonfork <<< "${spec_listing}"

if git merge-base --is-ancestor "${target}" HEAD; then
  printf 'merge-upstream: %s is already merged\n' "${target:0:12}"
  exit 0
fi
dirty=$(git status --porcelain --untracked-files=no)
if [[ -n "${dirty}" ]]; then
  port_die "merge-upstream: the work tree has uncommitted changes"
fi
previous=$(git merge-base HEAD "${target}" || true)

tmp=$(mktemp -d)
trap 'rm -rf -- "${tmp}"' EXIT

git merge --no-ff --no-commit --no-verify -s ours "${target}" > /dev/null
git diff -z --name-only --no-renames --diff-filter=D HEAD "${target}" -- "${nonfork[@]}" > "${tmp}/deleted"
if [[ -s "${tmp}/deleted" ]]; then
  git --literal-pathspecs rm -q --pathspec-from-file="${tmp}/deleted" --pathspec-file-nul
fi
git checkout "${target}" -- "${nonfork[@]}"

# Never import or keep the untrusted agent-configuration paths (a .claude/, .mcp.json, hook or
# ripgrep config from either side would otherwise be loaded and run by the port agent session with
# the job's credentials). They are already excluded from the non-fork pathspec, so the checkout
# above does not bring them in; drop any that are still present, from the index and the work tree.
# --ignore-unmatch makes the removal a no-op when, as expected, none of them exist.
agent_listing=$(port_agent_config_paths)
mapfile -t agent_cfg <<< "${agent_listing}"
git --literal-pathspecs rm -rq --cached --ignore-unmatch -- "${agent_cfg[@]}" > /dev/null
for path in "${agent_cfg[@]}"; do
  if [[ -n "${path}" && -e "${path}" ]]; then
    rm -rf -- "${path}"
  fi
done

if ! git diff --cached --quiet "${target}" -- "${nonfork[@]}"; then
  git diff --cached --stat "${target}" -- "${nonfork[@]}" | tail -n 40 >&2
  port_die "merge-upstream: paths outside the fork-owned set differ from ${target:0:12} after the merge"
fi
if ! git diff --cached --quiet HEAD -- "${fork[@]}"; then
  port_die "merge-upstream: fork-owned paths changed during the upstream merge"
fi

if [[ -n "${CTX:-}" ]]; then
  mkdir -p "${CTX}"
  if [[ -n "${previous}" ]]; then
    git diff --name-only --no-renames "${previous}" "${target}" -- "${fork[@]}" > "${CTX}/shadowed-upstream-paths.txt"
  else
    : > "${CTX}/shadowed-upstream-paths.txt"
  fi
fi

subject=$(git log -1 --format=%s "${target}")
subject=$(port_sanitize <<< "${subject}")
git commit --no-verify -q --cleanup=whitespace \
  -m "Merge upstream prebid/prebid-server@${target:0:12}" \
  -m "Upstream head: ${subject}"
merged=$(git rev-parse HEAD)
printf 'merge-upstream: merged %s as %s\n' "${target:0:12}" "${merged:0:12}"
