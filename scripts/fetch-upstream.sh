#!/usr/bin/env bash
# fetch-upstream.sh: check out the upstream Prebid Server Go source at the commit the port is
# pinned to. The Go source is test input only: it is fetched into a directory that is never
# committed.
#
# Usage: scripts/fetch-upstream.sh [--dir DIR] [--sha SHA]
#
#   --dir DIR  the checkout (default: .upstream at the repository root, which .gitignore ignores)
#   --sha SHA  the full commit to check out (default: the one non-comment line of UPSTREAM_SYNC)
#
# DIR becomes a detached checkout of exactly SHA with a clean tree. The script does nothing when
# DIR already is one. It changes nothing and fails when DIR has local changes or untracked files,
# or when DIR is a non-empty directory that is not the top of a git checkout. Progress goes to
# standard error; the last line of standard output is DIR's absolute path.
#
# Source: $PBS_UPSTREAM_URL (default https://github.com/prebid/prebid-server.git). When
# $PBS_UPSTREAM_REFERENCE names a local git repository that has the commit, the commit is fetched
# from it instead, without network access.
#
# The Rust tests, pbs-devtools and the Go tooling modules under crates/ read this checkout as the
# Go root: $PBS_GO_ROOT when it is set, else .upstream at the repository root.
#
# Every git command names DIR with -C, and the variables with which git points a command at one
# repository (GIT_DIR, GIT_WORK_TREE, GIT_INDEX_FILE and the rest of git rev-parse
# --local-env-vars) are cleared first, as the parity scripts do: git sets GIT_DIR and
# GIT_INDEX_FILE for a hook of a linked worktree, and GIT_DIR for the command of git rebase --exec
# there, and with them git -C DIR would initialize, fetch into and check out that worktree's
# repository instead of DIR.
set -euo pipefail
shopt -s inherit_errexit

# Clear the variables with which git points a command at one repository, as git does before it
# runs a command in another repository (a submodule), keeping the configuration passed down.
git_variables=$(git rev-parse --local-env-vars)
while IFS= read -r variable; do
  case "${variable}" in
    "" | GIT_CONFIG_PARAMETERS | GIT_CONFIG_COUNT) ;;
    *) unset "${variable}" ;;
  esac
done <<< "${git_variables}"

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "${here}/.." && pwd)

die() {
  printf 'fetch-upstream: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf 'usage: scripts/fetch-upstream.sh [--dir DIR] [--sha SHA]\n'
}

dir=""
sha=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dir)
      [[ $# -ge 2 && -n "$2" ]] || die "--dir needs a directory"
      dir=$2
      shift 2
      ;;
    --sha)
      [[ $# -ge 2 && -n "$2" ]] || die "--sha needs a commit"
      sha=$2
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "${sha}" ]]; then
  pin_file="${root}/UPSTREAM_SYNC"
  [[ -f "${pin_file}" ]] || die "${pin_file} does not exist"
  pins=$(sed -E -e 's/[[:space:]]+$//' -e '/^[[:space:]]*(#|$)/d' "${pin_file}")
  if [[ -z "${pins}" || "${pins}" == *$'\n'* ]]; then
    die "${pin_file} must hold exactly one non-comment line, the pinned commit"
  fi
  sha=${pins#"${pins%%[![:space:]]*}"}
fi
[[ "${sha}" =~ ^[0-9a-f]{40}$ ]] || die "${sha} is not a full 40-character commit SHA"
dir=${dir:-${root}/.upstream}
url=${PBS_UPSTREAM_URL:-https://github.com/prebid/prebid-server.git}
reference=${PBS_UPSTREAM_REFERENCE:-}

if [[ -e "${dir}" && ! -d "${dir}" ]]; then
  die "${dir} exists and is not a directory"
fi
mkdir -p "${dir}"
abs=$(cd "${dir}" && pwd)
physical=$(cd "${dir}" && pwd -P)

# DIR must be the top of its own checkout. A plain directory inside another repository (such as
# a fresh .upstream inside this one) is not, so git -C would otherwise act on the outer one.
top=$(git -C "${abs}" rev-parse --show-toplevel 2> /dev/null || true)
if [[ -n "${top}" ]]; then
  top=$(cd "${top}" && pwd -P)
fi
if [[ "${top}" != "${physical}" ]]; then
  entry=$(find "${abs}" -mindepth 1 -maxdepth 1 -print -quit)
  [[ -z "${entry}" ]] || die "${abs} is neither empty nor the top of a git checkout"
  printf 'fetch-upstream: creating a checkout in %s\n' "${abs}" >&2
  git -C "${abs}" init --quiet
  git -C "${abs}" remote add origin "${url}"
fi

changes=$(git -C "${abs}" status --porcelain)
if [[ -n "${changes}" ]]; then
  die "${abs} has local changes or untracked files; clean it or remove it, then rerun:
${changes}"
fi

head=$(git -C "${abs}" rev-parse --verify --quiet HEAD || true)
if [[ "${head}" != "${sha}" ]]; then
  if ! git -C "${abs}" cat-file -e "${sha}^{commit}" 2> /dev/null; then
    # Fetch only the commit itself into a new or shallow checkout; a full clone stays complete.
    depth=()
    shallow=$(git -C "${abs}" rev-parse --is-shallow-repository)
    if [[ -z "${head}" || "${shallow}" == true ]]; then
      depth=(--depth 1)
    fi
    source=${url}
    if [[ -n "${reference}" ]]; then
      if git -C "${reference}" cat-file -e "${sha}^{commit}" 2> /dev/null; then
        source=${reference}
      else
        printf 'fetch-upstream: %s does not have %s; fetching from %s\n' \
          "${reference}" "${sha}" "${url}" >&2
      fi
    fi
    printf 'fetch-upstream: fetching %s from %s\n' "${sha}" "${source}" >&2
    git -C "${abs}" fetch --quiet --no-tags "${depth[@]}" "${source}" "${sha}"
  fi
  git -C "${abs}" -c advice.detachedHead=false checkout --quiet --detach "${sha}"
fi

head=$(git -C "${abs}" rev-parse HEAD)
[[ "${head}" == "${sha}" ]] || die "${abs} is at ${head}, not ${sha}"
changes=$(git -C "${abs}" status --porcelain)
[[ -z "${changes}" ]] || die "${abs} is not clean after the checkout"
printf '%s\n' "${abs}"
