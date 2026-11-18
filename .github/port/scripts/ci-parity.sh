#!/usr/bin/env bash
# ci-parity.sh: run the Go-versus-Rust parity harness of the parity-harness branch (parity/, see
# parity/README.md there). Exits 0 with a notice when parity/ does not exist.
#
# The parity packages are Go code of the upstream module (github.com/prebid/prebid-server/v4/
# parity/...), and the harness needs small seams in upstream code, which the branch keeps as a
# patch (parity/upstream-seams.patch). Neither is ever applied in this repository: they are built
# in an assembled copy, the way parity/scripts/build-go.sh builds the parity tools:
#   1. the pinned upstream tree (PBS_GO_ROOT, default .upstream) is exported with git archive into
#      ${RUNNER_TEMP}/parity-go (a new temporary directory without RUNNER_TEMP), parity/ is copied
#      into it, and the patch is applied there (git apply, which fails on any rejected hunk)
#   2. gofmt -s -l prints nothing for the Go files the patch adds or changes, as the assembled
#      copy has them
#   3. parity/scripts/seams.sh check: the patch is in the form seams.sh write gives it
#   4. go test in parity/scripts, the module of the parity scripts' own tests (in this checkout:
#      build-go.sh leaves it out of the trees it assembles)
#   5. go test ./parity/...
#   6. go test -race on the harness packages that run concurrently
#   7. go test on every upstream package the patch touches (the directory of each file it names,
#      or the nearest one above it with Go files, from the patch's own file list) and on the root
#      package, the server's main
#   8. the Go tooling module crates/parity/testdata/go builds and its tests pass against the
#      assembled copy (a temporary go.work replaces the upstream module with it, as its run.sh
#      does)
#   9. parity selftest over the sample-requests, adapters and exchangetest corpora
#  10. parity run against the Rust server, decided by the workspace (cargo metadata): with no bin
#      target named pbs-server (the binary the harness runs as --bin-b) there is no Rust server to
#      compare, and the step is skipped with a notice that says so; with one, it is built and the
#      run always happens, with --bin-b and against parity/baseline.json when the branch has one,
#      and any nonzero exit fails the job: 1 (a mismatch, or a regression from the baseline), 2
#      (usage) and 3 (incomplete: cases the run could not decide) alike
# Steps 5-10 run in the assembled copy; its parity/out/ holds the artifacts. The last line of
# standard output is the assembled copy's path.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
top=$(pwd)
if [[ ! -d parity ]]; then
  printf '::notice::parity/ does not exist on this branch; nothing to run\n'
  exit 0
fi
patch="${top}/parity/upstream-seams.patch"
if [[ ! -f "${patch}" ]]; then
  port_die "ci-parity: parity/ exists but parity/upstream-seams.patch does not"
fi
go_root=${PBS_GO_ROOT:-${top}/.upstream}
if [[ ! -f "${go_root}/go.mod" ]]; then
  port_die "ci-parity: ${go_root} is not an upstream checkout; run scripts/fetch-upstream.sh first"
fi
pin=$(port_pin_parse < UPSTREAM_SYNC)
go_head=$(git -C "${go_root}" rev-parse HEAD)
if [[ "${go_head}" != "${pin}" ]]; then
  port_die "ci-parity: ${go_root} is at ${go_head:0:12}, not at the pin ${pin:0:12}"
fi

if [[ -n "${RUNNER_TEMP:-}" ]]; then
  assembled="${RUNNER_TEMP}/parity-go"
  rm -rf -- "${assembled}"
  mkdir -p "${assembled}"
else
  assembled=$(mktemp -d)
fi
work=$(mktemp -d)
trap 'rm -rf -- "${work}"' EXIT

# 1. Assemble.
git -C "${go_root}" archive --format=tar HEAD | tar -x -C "${assembled}"
cp -R parity "${assembled}/parity"
ceiling=$(dirname "${assembled}")
if ! (cd "${assembled}" && env GIT_CEILING_DIRECTORIES="${ceiling}" git apply --check "${patch}"); then
  port_die "ci-parity: parity/upstream-seams.patch does not apply to upstream ${pin:0:12}"
fi
(cd "${assembled}" && env GIT_CEILING_DIRECTORIES="${ceiling}" git apply "${patch}")
printf 'ci-parity: assembled upstream %s, parity/ and the seams patch in %s\n' "${pin:0:12}" "${assembled}"

# The files the patch names, from git apply's own reading of it (-z: every path verbatim; a
# rename names its old and its new path, and counts as the new one).
numstat=$(cd "${assembled}" && env GIT_CEILING_DIRECTORIES="${ceiling}" git apply --numstat -z "${patch}" | tr '\0' '\n')
patched=()
pending_rename=0
while IFS= read -r line; do
  if [[ "${pending_rename}" -gt 0 ]]; then
    pending_rename=$((pending_rename - 1))
    if [[ "${pending_rename}" -eq 0 ]]; then
      patched+=("${line}")
    fi
    continue
  fi
  if [[ -z "${line}" ]]; then
    continue
  fi
  IFS=$'\t' read -r _ _ path <<< "${line}"
  if [[ -z "${path}" ]]; then
    pending_rename=2
  else
    patched+=("${path}")
  fi
done <<< "${numstat}"
if [[ "${#patched[@]}" -eq 0 ]]; then
  port_die "ci-parity: parity/upstream-seams.patch names no file"
fi

# 2. gofmt -s over the Go files the patch leaves in the assembled copy.
go_files=()
for path in "${patched[@]}"; do
  if [[ "${path}" == *.go && -f "${assembled}/${path}" ]]; then
    go_files+=("${path}")
  fi
done
if [[ "${#go_files[@]}" -gt 0 ]]; then
  unformatted=$(cd "${assembled}" && gofmt -s -l -- "${go_files[@]}")
  if [[ -n "${unformatted}" ]]; then
    port_error "ci-parity: gofmt -s would change these files of the patched tree (fix them in the tree and save the seams with parity/scripts/seams.sh write):"
    printf '%s\n' "${unformatted}" >&2
    exit 1
  fi
fi
printf 'ci-parity: gofmt -s is clean on the %s Go file(s) the patch touches\n' "${#go_files[@]}"

# 3. The patch is in its canonical form.
parity/scripts/seams.sh check --patch "${patch}"

# 4. The tests of the parity scripts, a module of their own.
if [[ ! -f parity/scripts/go.mod ]]; then
  port_die "ci-parity: parity/scripts/go.mod does not exist; the parity scripts' tests are a Go module of their own"
fi
(cd parity/scripts && go test -count=1 ./...)

# 5-6. The harness.
cd "${assembled}"
go test -count=1 ./parity/...
race=()
for package in trace transport diff runner minimize report matrix; do
  if [[ -d "parity/${package}" ]]; then
    race+=("./parity/${package}/...")
  fi
done
if [[ "${#race[@]}" -gt 0 ]]; then
  go test -count=1 -race "${race[@]}"
fi

# 7. The upstream packages the patch touches, and the root package.
# parent PATH: the directory PATH is in, relative to the assembled copy (. at the top).
parent() {
  local path=${1:?}
  if [[ "${path}" == */* ]]; then
    printf '%s\n' "${path%/*}"
  else
    printf '.\n'
  fi
}
declare -A seen=([.]=1)
packages=(.)
for path in "${patched[@]}"; do
  dir=$(parent "${path}")
  while [[ "${dir}" != . ]]; do
    if [[ -d "${dir}" ]] && compgen -G "${dir}/*.go" > /dev/null; then
      break
    fi
    dir=$(parent "${dir}")
  done
  if [[ -z "${seen[${dir}]:-}" ]]; then
    seen[${dir}]=1
    packages+=("./${dir}")
  fi
done
sorted=$(printf '%s\n' "${packages[@]}" | sort)
mapfile -t packages <<< "${sorted}"
printf 'ci-parity: go test of the %s package(s) the patch touches, with the root package: %s\n' \
  "${#packages[@]}" "${packages[*]}"
go test -count=1 "${packages[@]}"

# 8. The Go tooling module that needs parity/.
tooling="${top}/crates/parity/testdata/go"
if [[ -f "${tooling}/go.mod" ]]; then
  go_version=$(sed -n 's/^go //p' "${tooling}/go.mod")
  module=$(port_go_module)
  printf 'go %s\n\nuse "%s"\n\nreplace %s => "%s"\n' "${go_version}" "${tooling}" "${module}" "${assembled}" \
    > "${work}/go.work"
  cp "${assembled}/go.sum" "${work}/go.work.sum"
  (cd "${tooling}" && env GOWORK="${work}/go.work" go build -o "${work}/bin/" ./... \
    && env GOWORK="${work}/go.work" go test -count=1 ./...)
fi

# 9. The harness on its own corpora.
go run ./parity/cmd/parity selftest --corpus sample-requests,adapters,exchangetest

# 10. Go against Rust, when the workspace has a Rust server.
metadata=$(cd "${top}" && cargo metadata --no-deps --format-version 1 --locked)
servers=$(jq -r '.packages[] | .name as $package | .targets[]
  | select((.kind | index("bin")) and .name == "pbs-server") | $package' <<< "${metadata}")
if [[ -z "${servers}" ]]; then
  printf '::notice::parity run skipped: the workspace has no bin target named pbs-server (cargo metadata), so there is no Rust server for the harness to run as --bin-b; the Go side is checked by the steps above, and the run starts once the server binary exists\n'
  printf '%s\n' "${assembled}"
  exit 0
fi
if [[ "${servers}" == *$'\n'* ]]; then
  port_die "ci-parity: more than one workspace package has a bin target named pbs-server: ${servers//$'\n'/, }"
fi
(cd "${top}" && cargo build --release --locked -p "${servers}" --bin pbs-server)
target_dir=$(jq -r '.target_directory' <<< "${metadata}")
run=(go run ./parity/cmd/parity run --impl-b rust --bin-b "${target_dir}/release/pbs-server")
if [[ -f parity/baseline.json ]]; then
  run+=(--baseline parity/baseline.json)
fi
printf 'ci-parity: %s\n' "${run[*]}"
rc=0
"${run[@]}" || rc=$?
case "${rc}" in
  0) ;;
  1) port_die "ci-parity: the parity run failed (exit status 1): Go and Rust differ, or a case regressed from the baseline; see parity/out/" ;;
  2) port_die "ci-parity: the parity run refused its arguments (exit status 2)" ;;
  3) port_die "ci-parity: the parity run is incomplete (exit status 3): it could not decide every case against the Rust server; an incomplete run is a failure here" ;;
  *) port_die "ci-parity: the parity run exited with status ${rc}" ;;
esac
printf '%s\n' "${assembled}"
