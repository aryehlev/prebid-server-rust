#!/usr/bin/env bash
# upstream-mirror.sh DIR: make DIR a bare mirror of the upstream branch and its tags, or bring an
# existing one up to date, and print DIR's absolute path. The planner, the guard and the port job
# read upstream history from it (lib.sh port_up, with UPSTREAM_GIT=DIR), and scripts/fetch-upstream.sh
# checks commits out of it (PBS_UPSTREAM_REFERENCE=DIR). It is a repository of its own, so upstream
# objects never enter this repository.
#
# Env: PBS_UPSTREAM_URL (default https://github.com/prebid/prebid-server.git), UPSTREAM_BRANCH
#      (default master).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

if [[ "$#" -ne 1 || -z "${1}" ]]; then
  printf 'usage: upstream-mirror.sh DIR\n' >&2
  exit 2
fi
dir=${1}
url=${PBS_UPSTREAM_URL:-https://github.com/prebid/prebid-server.git}
branch=${UPSTREAM_BRANCH:-master}
if [[ ! "${branch}" =~ ^[A-Za-z0-9._/-]+$ ]]; then
  port_die "upstream-mirror: invalid branch '${branch}'"
fi

if [[ -e "${dir}" && "$(git --git-dir="${dir}" rev-parse --is-bare-repository 2> /dev/null || true)" != true ]]; then
  port_die "upstream-mirror: ${dir} exists and is not a bare repository"
fi
if [[ ! -e "${dir}" ]]; then
  git init --quiet --bare "${dir}"
fi
abs=$(cd "${dir}" && pwd)
printf 'upstream-mirror: fetching %s %s into %s\n' "${url}" "${branch}" "${abs}" >&2
git --git-dir="${abs}" fetch --quiet --prune --tags "${url}" "+refs/heads/${branch}:refs/heads/${branch}"
head=$(git --git-dir="${abs}" rev-parse --verify --quiet "refs/heads/${branch}^{commit}" || true)
if [[ -z "${head}" ]]; then
  port_die "upstream-mirror: ${url} has no branch ${branch}"
fi
printf 'upstream-mirror: %s is at %s\n' "${branch}" "${head:0:12}" >&2
printf '%s\n' "${abs}"
