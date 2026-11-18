#!/usr/bin/env bash
# lint-go.sh: lint the Go code the fork authors, never upstream Go.
#
# Usage: rust/scripts/lint-go.sh [gofmt] [vet] [golangci]   (no argument runs all three)
#
# Code: every .go file under rust/ (except rust/legacy/) and parity/: the differential test
# programs in rust/**/testdata/go/, the dump tools in rust/crates/devtools/go/ and, on the
# parity-harness branch, the parity harness. Packages are passed by explicit path because
# `./...` skips testdata directories.
#   gofmt     gofmt -s must leave every file unchanged
#   vet       go vet on every package
#   golangci  golangci-lint with .github/linters/golangci.yml (every linter on, no exclusions)
set -euo pipefail
shopt -s inherit_errexit

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(git -C "${here}" rev-parse --show-toplevel)
cd "${root}"

steps=("$@")
if [[ "${#steps[@]}" -eq 0 ]]; then
  steps=(gofmt vet golangci)
fi

listing=$(git ls-files --cached --others --exclude-standard -- 'rust/*.go' 'parity/*.go')
files=()
declare -A seen_dirs=()
dirs=()
while IFS= read -r path; do
  case "${path}" in
    "" | rust/legacy/*) continue ;;
    *) ;;
  esac
  if [[ ! -f "${path}" ]]; then
    continue
  fi
  files+=("${path}")
  dir="./${path%/*}"
  if [[ -z "${seen_dirs[${dir}]:-}" ]]; then
    seen_dirs[${dir}]=1
    dirs+=("${dir}")
  fi
done <<< "${listing}"

if [[ "${#files[@]}" -eq 0 ]]; then
  printf 'lint-go: no Go code to lint\n'
  exit 0
fi
printf 'lint-go: %s files in %s packages\n' "${#files[@]}" "${#dirs[@]}"

failed=0
for step in "${steps[@]}"; do
  case "${step}" in
    gofmt)
      unformatted=$(gofmt -s -l "${files[@]}")
      if [[ -n "${unformatted}" ]]; then
        printf 'gofmt -s would change:\n%s\n' "${unformatted}"
        gofmt -s -d "${files[@]}" || true
        failed=1
      else
        printf 'gofmt -s: clean\n'
      fi
      ;;
    vet)
      if go vet "${dirs[@]}"; then
        printf 'go vet: clean\n'
      else
        failed=1
      fi
      ;;
    golangci)
      if golangci-lint run --config .github/linters/golangci.yml "${dirs[@]}"; then
        printf 'golangci-lint: clean\n'
      else
        failed=1
      fi
      ;;
    *)
      printf 'lint-go: unknown step %s (expected gofmt, vet or golangci)\n' "${step}" >&2
      exit 2
      ;;
  esac
done
exit "${failed}"
