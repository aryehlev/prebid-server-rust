#!/usr/bin/env bash
# seams.sh: keep parity/upstream-seams.patch, the harness's changes to upstream files, in step with
# the assembled tree that parity/scripts/build-go.sh makes.
#
# Usage:
#   parity/scripts/seams.sh diff [--tree DIR]     print the tree's changes to upstream files as a
#                                                 patch, in the form the patch file is kept in
#   parity/scripts/seams.sh write [--tree DIR]    write that patch to parity/upstream-seams.patch
#   parity/scripts/seams.sh check [--patch FILE]  check that the patch (default
#                                                 parity/upstream-seams.patch) applies cleanly to
#                                                 the commit UPSTREAM_SYNC pins and is in that form
#
# DIR is the assembled tree (default parity/out/tree). Its changes are every change to upstream
# files against the pinned commit, its HEAD: diff and write stage them first (git add -A, which
# leaves parity/ out, since the tree's .git/info/exclude lists it), so a file added to the tree is
# part of the seams too. write also records the new patch in the tree's .parity-tree, so
# build-go.sh knows the tree holds no seam edits the patch lacks.
#
# The form: git diff with every option that git config could change set explicitly (prefixes,
# full blob IDs, no renames, the myers algorithm with the indent heuristic, three lines of context,
# no order file), so the same changes always give the same bytes, and git apply --3way finds the
# blobs a patch was made against when the pin moves. check assembles a tree from the patch in a
# temporary directory (build-go.sh --no-build) and compares the tree's diff with the patch byte for
# byte.
set -euo pipefail
shopt -s inherit_errexit

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "${here}/../.." && pwd -P)
patch_file="${root}/parity/upstream-seams.patch"
marker=".parity-tree"

die() {
  printf 'seams: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf 'usage: parity/scripts/seams.sh diff|write [--tree DIR]\n'
  printf '       parity/scripts/seams.sh check [--patch FILE]\n'
}

[[ $# -gt 0 ]] || {
  usage >&2
  exit 2
}
command=$1
shift
case "${command}" in
  diff | write | check) ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac

tree=""
patch=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --tree)
      [[ "${command}" != check && $# -ge 2 && -n "$2" ]] || die "--tree needs a directory and goes with diff or write"
      tree=$2
      shift 2
      ;;
    --patch)
      [[ "${command}" == check && $# -ge 2 && -n "$2" ]] || die "--patch needs a file and goes with check"
      patch=$2
      shift 2
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
done

# canonical_diff prints the seams of the tree $1: its staged changes against HEAD, the pinned
# commit, after staging every change to an upstream file.
canonical_diff() {
  git -C "$1" add --all
  git -C "$1" -c core.quotePath=true -c diff.noprefix=false -c diff.mnemonicPrefix=false \
    -c diff.relative=false -c diff.suppressBlankEmpty=false \
    diff --cached --binary --full-index --no-renames --no-color --no-ext-diff --no-textconv \
    --diff-algorithm=myers --indent-heuristic --unified=3 --inter-hunk-context=0 -O/dev/null \
    --src-prefix=a/ --dst-prefix=b/ HEAD --
}

# tree_dir checks that $1 is a tree build-go.sh assembled and prints its absolute path.
tree_dir() {
  local dir=$1
  [[ -d "${dir}" && -f "${dir}/${marker}" && -d "${dir}/.git" ]] ||
    die "${dir} is not a tree parity/scripts/build-go.sh assembled; run it first"
  (cd "${dir}" && pwd -P)
}

case "${command}" in
  diff)
    dir=$(tree_dir "${tree:-${root}/parity/out/tree}")
    canonical_diff "${dir}"
    ;;
  write)
    dir=$(tree_dir "${tree:-${root}/parity/out/tree}")
    tmp=$(mktemp -d)
    trap 'rm -rf -- "${tmp}"' EXIT
    canonical_diff "${dir}" > "${tmp}/seams.patch"
    [[ -s "${tmp}/seams.patch" ]] || die "${dir} has no changes to upstream files"
    cp -- "${tmp}/seams.patch" "${patch_file}"
    seams=$(git hash-object -- "${patch_file}")
    sed -i.bak -e "s/^seams=.*/seams=${seams}/" "${dir}/${marker}"
    rm -f -- "${dir}/${marker}.bak"
    files=$(grep -c '^diff --git ' "${patch_file}" || true)
    printf 'seams: wrote %s (%s files, blob %s)\n' "${patch_file}" "${files}" "${seams}" >&2
    ;;
  check)
    patch=${patch:-${patch_file}}
    [[ -f "${patch}" ]] || die "${patch} does not exist"
    patch_dir=.
    if [[ "${patch}" == */* ]]; then
      patch_dir=${patch%/*}
    fi
    patch_dir=$(cd "${patch_dir:-/}" && pwd -P)
    patch="${patch_dir}/${patch##*/}"
    tmp=$(mktemp -d)
    trap 'rm -rf -- "${tmp}"' EXIT
    "${here}/build-go.sh" --no-build --patch "${patch}" --tree "${tmp}/tree" > /dev/null
    canonical_diff "${tmp}/tree" > "${tmp}/seams.patch"
    if ! cmp -s "${patch}" "${tmp}/seams.patch"; then
      printf 'seams: %s applies, but is not in the form seams.sh write gives it:\n' "${patch}" >&2
      diff -u "${patch}" "${tmp}/seams.patch" | head -n 40 >&2 || true
      exit 1
    fi
    files=$(grep -c '^diff --git ' "${patch}" || true)
    printf 'seams: %s applies cleanly to the pinned commit (%s files)\n' "${patch}" "${files}"
    ;;
  *) die "unknown command ${command}" ;;
esac
