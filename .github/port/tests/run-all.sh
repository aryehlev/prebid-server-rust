#!/usr/bin/env bash
# run-all.sh: run every .github/port/tests/test_*.sh in its own bash process (the bash running this
# script) and fail when any of them fails. Each test builds its fixtures in a temporary directory
# with git isolated from the user's configuration; nothing outside it is changed. Requires bash 4.4
# or newer, git, jq and perl, and the full history of this repository (test_classify and
# test_guard read real upstream commits).
#
# Usage: .github/port/tests/run-all.sh [NAME...]   (NAME: a test without test_ and .sh)
set -euo pipefail
shopt -s inherit_errexit

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
tests=()
if [[ "$#" -gt 0 ]]; then
  for name in "$@"; do
    tests+=("${here}/test_${name}.sh")
  done
else
  for test in "${here}"/test_*.sh; do
    tests+=("${test}")
  done
fi

failed=()
for test in "${tests[@]}"; do
  name=$(basename "${test}" .sh)
  if [[ ! -f "${test}" ]]; then
    printf '::error::no test %s\n' "${test}"
    failed+=("${name}")
    continue
  fi
  printf '::group::%s\n' "${name}"
  status=0
  "${BASH}" "${test}" || status=$?
  printf '::endgroup::\n'
  if [[ "${status}" -ne 0 ]]; then
    printf '::error::%s failed (exit status %s)\n' "${name}" "${status}"
    failed+=("${name}")
  else
    printf '%s: ok\n' "${name}"
  fi
done

if [[ "${#failed[@]}" -gt 0 ]]; then
  printf 'run-all: %s of %s test file(s) failed: %s\n' "${#failed[@]}" "${#tests[@]}" "${failed[*]}"
  exit 1
fi
printf 'run-all: %s test file(s) passed\n' "${#tests[@]}"
