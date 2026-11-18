#!/usr/bin/env bash
# go-tooling.sh: the checks of the Go code this repository holds, which is test tooling only (the
# programs that write goldens from the real Go code, and pbs-devtools' dump programs):
#   1. gofmt -s -l prints nothing for any tracked or new .go file;
#   2. in every Go tooling module (a go.mod under crates/), `go build ./...` (programs are written
#      to a temporary directory, never into the tree) and `go test ./...` pass with
#      GOFLAGS=-mod=readonly, against the Go root the replace directives point at;
#   3. the runs leave the work tree as they found it.
# crates/parity/testdata/go is skipped: it imports the parity packages, which only the
# parity-harness branch has (ci-parity.sh builds it there). The Go code is not linted: there is
# no go vet and no golangci-lint (see .github/README.md).
#
# Env: PBS_GO_ROOT is not read: the modules build against .upstream at the top of the work tree,
# which scripts/fetch-upstream.sh makes. GOFLAGS is forced to -mod=readonly.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
top=$(pwd)
if [[ ! -f .upstream/go.mod ]]; then
  port_die "go-tooling: .upstream is not an upstream checkout; run scripts/fetch-upstream.sh first"
fi

listing=$(git ls-files -z --cached --others --exclude-standard -- '*.go' | tr '\0' '\n')
files=()
while IFS= read -r path; do
  if [[ -n "${path}" && -f "${path}" ]]; then
    files+=("${path}")
  fi
done <<< "${listing}"
failed=0
if [[ "${#files[@]}" -gt 0 ]]; then
  unformatted=$(gofmt -s -l "${files[@]}")
  if [[ -n "${unformatted}" ]]; then
    port_error "go-tooling: gofmt -s would change these files (run gofmt -s -w):"
    printf '%s\n' "${unformatted}" >&2
    failed=1
  fi
fi
printf 'go-tooling: gofmt -s checked %s file(s)\n' "${#files[@]}"

tmp=$(mktemp -d)
trap 'rm -rf -- "${tmp}"' EXIT
before=$(git status --porcelain --untracked-files=all)

modules=$(find crates \( -name target -o -name '.*' \) -prune -o -type f -name go.mod -print | sort)
count=0
while IFS= read -r gomod; do
  dir=${gomod%/go.mod}
  if [[ -z "${gomod}" || "${dir}" == crates/parity/testdata/go ]]; then
    continue
  fi
  count=$((count + 1))
  printf '::group::go build and go test in %s\n' "${dir}"
  if ! (cd "${top}/${dir}" && env GOFLAGS=-mod=readonly go build -o "${tmp}/bin/" ./... \
    && env GOFLAGS=-mod=readonly go test ./...); then
    port_error "go-tooling: ${dir} does not build or its tests fail"
    failed=1
  fi
  printf '::endgroup::\n'
done <<< "${modules}"
if [[ "${count}" -eq 0 ]]; then
  port_die "go-tooling: no Go tooling modules under crates/"
fi
after=$(git status --porcelain --untracked-files=all)
if [[ "${after}" != "${before}" ]]; then
  port_error "go-tooling: the Go runs changed the work tree:"
  diff <(printf '%s\n' "${before}") <(printf '%s\n' "${after}") >&2 || true
  failed=1
fi
printf 'go-tooling: %s module(s) built and tested\n' "${count}"
exit "${failed}"
