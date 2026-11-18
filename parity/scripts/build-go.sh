#!/usr/bin/env bash
# build-go.sh: assemble the instrumented Go server from the pinned upstream checkout, the harness
# in parity/ and the seams the harness needs in upstream files, then build it.
#
# This repository never holds an upstream Go file, so the harness cannot compile in place: its
# packages are github.com/prebid/prebid-server/v4/parity/... inside the upstream module, and its
# hooks in upstream files are parity/upstream-seams.patch. The script puts the three together in
# a work directory, the assembled tree, and builds there.
#
# Usage: parity/scripts/build-go.sh [--tree DIR] [--bin DIR] [--patch FILE] [--3way] [--no-build]
#
#   --tree DIR    the assembled tree (default parity/out/tree). It is deleted and assembled anew
#                 on every run, so an existing DIR must be empty or a tree this script assembled
#                 that still finds the objects it borrows from the Go root and holds no seam
#                 edits that the patch lacks (see seams.sh). Inside this repository it must be
#                 under parity/out/, which git ignores.
#   --bin DIR     where the builds go (default parity/out/bin)
#   --patch FILE  the seams to apply (default parity/upstream-seams.patch)
#   --3way        apply the seams with git apply --3way, to refresh them after UPSTREAM_SYNC moved
#                 (parity/README.md, "The upstream seams"); it stops after the patch, conflicts
#                 or not, and builds nothing
#   --no-build    assemble the tree only
#
# Steps:
#   1. The Go root is $PBS_GO_ROOT when it is set (a relative path is taken from the repository
#      root), else .upstream at the repository root, where scripts/fetch-upstream.sh checks it out.
#      It must be a git checkout of github.com/prebid/prebid-server/v4 that has the commit
#      UPSTREAM_SYNC pins. Nothing in it is written.
#   2. The tree is a git repository that borrows every object of the Go root, checked out at that
#      commit whatever the Go root has checked out, so it holds exactly the pinned upstream files
#      and, for --3way, the blobs of earlier pins the Go root still has. .parity-tree at its
#      top names this repository, the upstream commit and the seams. The harness writes its
#      reports into this repository's parity/out/ and finds the Rust build here (parity/corpus
#      Repo.HarnessRoot).
#   3. The seams are applied with git apply --index, and must apply cleanly. The tree's index then
#      holds them: parity/scripts/seams.sh diff prints them, and .parity-tree records the blob ID
#      of what it prints.
#   4. parity/ is copied in as the package tree github.com/prebid/prebid-server/v4/parity/...: its
#      tracked files and its untracked files that git does not ignore, without parity/scripts/
#      (the shell scripts and their tests, a Go module of their own), parity/out/ and the patch.
#      The tree's .git/info/exclude lists parity/, so git in the tree shows the seams only.
#   5. Unless --no-build, it builds from the tree, with GOWORK=off and -buildvcs=false (the tree
#      is a scratch repository, nothing to stamp a binary with), into the --bin directory:
#        pbs-go   the instrumented server (the module root, .)
#        parity   the runner (./parity/cmd/parity)
#        rhtool   the ReplayHarness tool (./parity/replayharness/rhtool)
#
# Edit the harness in parity/ and the seams in the tree, and save seam edits with seams.sh write
# before the next run: the tree's copy of parity/ is replaced every time.
#
# Every git command names its repository with -C, and the variables with which git points a
# command at one repository (GIT_DIR, GIT_INDEX_FILE and the rest of git rev-parse
# --local-env-vars) are cleared first: git sets GIT_DIR and GIT_INDEX_FILE for a hook of a linked
# worktree, and GIT_DIR for the command of git rebase --exec there, and with them git -C on the
# tree would move that worktree's HEAD to the pin and write the upstream files into its index.
#
# The last line of standard output is the tree's absolute path; progress goes to standard error.
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
root=$(cd "${here}/../.." && pwd -P)
module="github.com/prebid/prebid-server/v4"
marker=".parity-tree"

die() {
  printf 'build-go: %s\n' "$*" >&2
  exit 1
}

say() {
  printf 'build-go: %s\n' "$*" >&2
}

usage() {
  printf 'usage: parity/scripts/build-go.sh [--tree DIR] [--bin DIR] [--patch FILE] [--3way] [--no-build]\n'
}

tree=""
bin=""
patch="${root}/parity/upstream-seams.patch"
three_way=0
build=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --tree)
      [[ $# -ge 2 && -n "$2" ]] || die "--tree needs a directory"
      tree=$2
      shift 2
      ;;
    --bin)
      [[ $# -ge 2 && -n "$2" ]] || die "--bin needs a directory"
      bin=$2
      shift 2
      ;;
    --patch)
      [[ $# -ge 2 && -n "$2" ]] || die "--patch needs a file"
      patch=$2
      shift 2
      ;;
    --3way)
      three_way=1
      build=0
      shift
      ;;
    --no-build)
      build=0
      shift
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

# The toolchain first: nothing is worth assembling without it.
if [[ "${build}" -eq 1 ]]; then
  go_version=$(go version)
  say "${go_version}"
fi

pin_file="${root}/UPSTREAM_SYNC"
[[ -f "${pin_file}" ]] || die "${pin_file} does not exist"
pins=$(sed -E -e 's/[[:space:]]+$//' -e '/^[[:space:]]*(#|$)/d' "${pin_file}")
if [[ -z "${pins}" || "${pins}" == *$'\n'* ]]; then
  die "${pin_file} must hold exactly one non-comment line, the pinned commit"
fi
pin=${pins#"${pins%%[![:space:]]*}"}
[[ "${pin}" =~ ^[0-9a-f]{40}$ ]] || die "${pin_file}: ${pin} is not a full 40-character commit SHA"
[[ -f "${patch}" ]] || die "${patch} does not exist"
patch_dir=.
if [[ "${patch}" == */* ]]; then
  patch_dir=${patch%/*}
fi
patch_dir=$(cd "${patch_dir:-/}" && pwd -P)
patch="${patch_dir}/${patch##*/}"

# 1. The Go root, found the way the Rust tests and pbs-devtools find it.
fetch="run scripts/fetch-upstream.sh to check out the commit UPSTREAM_SYNC pins into .upstream"
if [[ -n "${PBS_GO_ROOT:-}" ]]; then
  go_root=${PBS_GO_ROOT}
  if [[ "${go_root}" != /* ]]; then
    go_root="${root}/${go_root}"
  fi
  source="PBS_GO_ROOT"
  fetch="run scripts/fetch-upstream.sh --dir ${go_root} to check out the commit UPSTREAM_SYNC pins there"
else
  go_root="${root}/.upstream"
  source="the default"
fi
[[ -d "${go_root}" ]] || die "the Go root ${go_root} (${source}) does not exist; ${fetch}"
go_root=$(cd "${go_root}" && pwd -P)
first_line=""
if [[ -f "${go_root}/go.mod" ]]; then
  IFS= read -r first_line < "${go_root}/go.mod" || true
fi
if [[ "${first_line}" != "module ${module}" ]]; then
  die "the Go root ${go_root} (${source}) is not a checkout of ${module}; ${fetch}"
fi
if ! git -C "${go_root}" cat-file -e "${pin}^{commit}" 2> /dev/null; then
  die "the Go root ${go_root} (${source}) is not a git checkout that has ${pin}; ${fetch}"
fi

# physical prints the absolute path of $1 with symbolic links resolved, without creating it: its
# deepest existing ancestor, resolved, and the rest of the path.
physical() {
  local path=$1 rest=""
  if [[ "${path}" != /* ]]; then
    path="${PWD}/${path}"
  fi
  while [[ "${path}" == */ && "${path}" != / ]]; do
    path=${path%/}
  done
  while [[ ! -d "${path}" ]]; do
    rest="/${path##*/}${rest}"
    path=${path%/*}
    path=${path:-/}
  done
  path=$(cd "${path}" && pwd -P)
  printf '%s%s\n' "${path%/}" "${rest}"
}

# The tree may be replaced only when it is empty or a tree this script assembled that holds no
# unsaved seam edits, and inside this repository only under parity/out/, which git ignores (the
# lint scans must never see upstream code). Its place is checked before anything is created.
tree=$(physical "${tree:-${root}/parity/out/tree}")
bin=${bin:-${root}/parity/out/bin}
if [[ -e "${tree}" && ! -d "${tree}" ]]; then
  die "${tree} exists and is not a directory"
fi
case "${tree}/" in
  "${go_root}/"*) die "the tree ${tree} would be inside the Go root ${go_root}" ;;
  "${root}/parity/out/"?*) ;;
  "${root}/"*) die "the tree ${tree} is inside the repository but not under parity/out/" ;;
  *) ;;
esac
mkdir -p "${root}/parity/out"
if [[ ! -f "${root}/parity/out/.gitignore" ]]; then
  printf '*\n' > "${root}/parity/out/.gitignore"
fi
mkdir -p "${tree}"
entry=$(find "${tree}" -mindepth 1 -maxdepth 1 -print -quit)
if [[ -n "${entry}" ]]; then
  [[ -f "${tree}/${marker}" ]] || die "${tree} is neither empty nor a tree this script assembled (it has no ${marker})"
  if [[ -d "${tree}/.git" ]]; then
    # The tree borrows the Go root's objects (step 2). Once they are gone, because the Go root was
    # deleted, moved or fetched anew at another commit, git cannot tell the tree's seam edits apart.
    if ! git -C "${tree}" cat-file -e "HEAD^{tree}" 2> /dev/null; then
      die "${tree} borrows objects its Go root no longer has, so its seam edits cannot be told apart; save any by hand, then delete the tree"
    fi
    recorded=$(sed -n 's/^seams=//p' "${tree}/${marker}")
    "${here}/seams.sh" diff --tree "${tree}" > "${tree}/.git/parity-seams"
    current=$(git hash-object -- "${tree}/.git/parity-seams")
    if [[ -s "${tree}/.git/parity-seams" && "${current}" != "${recorded}" ]]; then
      die "${tree} holds seam edits that the patch lacks; save them with parity/scripts/seams.sh write --tree ${tree}, or delete the tree"
    fi
  fi
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/build-go.XXXXXX")
trap 'rm -rf -- "${tmp}"' EXIT

# 2. The pinned upstream files. The tree is a repository of its own that borrows every object of
# the Go root (objects/info/alternates), not a clone of it: a clone of a shallow Go root, which is
# what fetch-upstream.sh makes, copies only the objects its HEAD reaches, and git apply --3way
# needs the blobs of the previous pin, which the Go root keeps after a move but no ref reaches.
# The Go root's shallow file comes along, so git knows where the borrowed history stops. git init
# copies no templates and the checkout runs no hooks and converts no line endings, so the patch
# sees the committed bytes.
say "assembling ${tree} from ${go_root} at ${pin}"
objects=$(git -C "${go_root}" rev-parse --path-format=absolute --git-path objects)
shallow=$(git -C "${go_root}" rev-parse --path-format=absolute --git-path shallow)
find "${tree}" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
git init --quiet --template= --initial-branch=parity-tree -- "${tree}"
mkdir -p "${tree}/.git/objects/info" "${tree}/.git/info"
printf '%s\n' "${objects}" > "${tree}/.git/objects/info/alternates"
if [[ -f "${shallow}" ]]; then
  cp -- "${shallow}" "${tree}/.git/shallow"
fi
git -C "${tree}" -c core.hooksPath=/dev/null -c core.autocrlf=false -c advice.detachedHead=false \
  checkout --quiet --detach "${pin}"
head=$(git -C "${tree}" rev-parse HEAD)
[[ "${head}" == "${pin}" ]] || die "the tree is at ${head}, not ${pin}"
[[ ! -e "${tree}/parity" ]] || die "upstream ${pin} has a parity/ of its own; the harness cannot be copied in"
printf '/parity/\n/%s\n' "${marker}" >> "${tree}/.git/info/exclude"
# Until the seams are recorded, the tree counts as holding edits (a --3way refresh in progress).
printf 'workspace=%s\nupstream=%s\nseams=none\n' "${root}" "${pin}" > "${tree}/${marker}"

# 3. The seams.
if [[ "${three_way}" -eq 1 ]]; then
  status=0
  git -C "${tree}" apply --3way -- "${patch}" || status=$?
  conflicts=$(git -C "${tree}" diff --name-only --diff-filter=U)
  if [[ "${status}" -ne 0 && -z "${conflicts}" ]]; then
    die "git apply --3way could not apply ${patch} to ${pin}: the Go root lacks the blobs the patch was made against, which fetching the previous pin into it adds (parity/README.md, \"The upstream seams\")"
  fi
  if [[ -n "${conflicts}" ]]; then
    say "the seams conflict with ${pin} in:"
    printf '  %s\n' "${conflicts}" >&2
    say "resolve the conflicts in ${tree}, then save the seams with parity/scripts/seams.sh write --tree ${tree}"
    exit 1
  fi
  say "the seams apply to ${pin} by a three-way merge; save them with parity/scripts/seams.sh write --tree ${tree}"
  printf '%s\n' "${tree}"
  exit 0
fi
if ! git -C "${tree}" apply --index -- "${patch}"; then
  die "${patch} does not apply to ${pin}. When UPSTREAM_SYNC moves, the seams are refreshed with --3way: see \"The upstream seams\" in parity/README.md"
fi
"${here}/seams.sh" diff --tree "${tree}" > "${tree}/.git/parity-seams"
seams=$(git hash-object -- "${tree}/.git/parity-seams")
printf 'workspace=%s\nupstream=%s\nseams=%s\n' "${root}" "${pin}" "${seams}" > "${tree}/${marker}"

# 4. The harness.
list="${tmp}/parity-files"
top=""
if git -C "${root}" rev-parse --is-inside-work-tree > /dev/null 2>&1; then
  top=$(git -C "${root}" rev-parse --show-toplevel)
  top=$(cd "${top}" && pwd -P)
fi
if [[ "${top}" == "${root}" ]]; then
  git -C "${root}" ls-files -z --cached --others --exclude-standard -- parity > "${tmp}/listed"
else
  (cd "${root}" && find parity -type f -print0) > "${tmp}/listed"
fi
: > "${list}"
copied=0
while IFS= read -r -d '' path; do
  case "${path}" in
    parity/scripts/* | parity/out/* | parity/upstream-seams.patch) continue ;;
    *) ;;
  esac
  # A tracked file deleted from the work tree is not part of the harness any more.
  [[ -f "${root}/${path}" ]] || continue
  printf '%s\0' "${path}" >> "${list}"
  copied=$((copied + 1))
done < "${tmp}/listed"
[[ "${copied}" -gt 0 ]] || die "found no harness files under ${root}/parity"
(cd "${root}" && COPYFILE_DISABLE=1 tar -c -f - --null -T "${list}") | tar -x -f - -C "${tree}"
say "applied the seams (blob ${seams}) and copied ${copied} harness files"

# 5. The builds.
if [[ "${build}" -eq 1 ]]; then
  mkdir -p "${bin}"
  bin=$(cd "${bin}" && pwd -P)
  for target in "pbs-go=." "parity=./parity/cmd/parity" "rhtool=./parity/replayharness/rhtool"; do
    name=${target%%=*}
    package=${target#*=}
    say "building ${bin}/${name} from ${package}"
    (cd "${tree}" && GOWORK=off go build -buildvcs=false -o "${bin}/${name}" "${package}")
  done
fi

printf '%s\n' "${tree}"
