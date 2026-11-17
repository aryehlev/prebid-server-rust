#!/usr/bin/env bash
# Self-tests for rust/scripts/check-no-lint-suppression.sh.
#
# Every directory under fixtures/ is one case:
#   tree/         files laid out like the repository. Each file has an extra .in suffix, which is
#                 removed when the case is copied to a temporary directory, so that real scans,
#                 cargo, taplo, actionlint and the linters never treat the fixtures as live files.
#   expected.txt  the exact findings, "<path>:<line>: [<rule>]", one line per finding, that the
#                 scanner must print for the tree. Lines starting with # are comments. A case
#                 without findings must scan clean and exit 0; any other case must exit 1.
#
# The run also fails when a rule listed in the scanner's header has no fixture that triggers it.
set -euo pipefail
shopt -s inherit_errexit

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
scanner="${here}/../check-no-lint-suppression.sh"
tmp=$(mktemp -d)
trap 'rm -rf -- "${tmp}"' EXIT

failures=0
cases=0
: > "${tmp}/all-expected"

for case_dir in "${here}"/fixtures/*/; do
  name=$(basename "${case_dir}")
  cases=$((cases + 1))
  tree="${tmp}/trees/${name}"
  mkdir -p "${tree}"
  (cd "${case_dir}tree" && find . -type f -name '*.in' -print0) > "${tmp}/${name}.files"
  while IFS= read -r -d '' fixture; do
    target="${tree}/${fixture%.in}"
    mkdir -p "$(dirname "${target}")"
    cp "${case_dir}tree/${fixture}" "${target}"
  done < "${tmp}/${name}.files"

  grep -v -e '^#' -e '^[[:space:]]*$' "${case_dir}expected.txt" | LC_ALL=C sort > "${tmp}/${name}.expected" || true
  cat "${tmp}/${name}.expected" >> "${tmp}/all-expected"

  status=0
  "${scanner}" --root "${tree}" > "${tmp}/${name}.out" 2>&1 || status=$?
  sed -n -E 's/^([^ ]+:[0-9]+: \[[a-z0-9-]+\]).*/\1/p' "${tmp}/${name}.out" | LC_ALL=C sort > "${tmp}/${name}.got"

  want_status=1
  if [[ ! -s "${tmp}/${name}.expected" ]]; then
    want_status=0
  fi

  if [[ "${status}" -ne "${want_status}" ]]; then
    printf 'FAIL %s: exit status %s, want %s\n' "${name}" "${status}" "${want_status}"
    sed 's/^/    /' "${tmp}/${name}.out"
    failures=$((failures + 1))
  elif ! diff -u "${tmp}/${name}.expected" "${tmp}/${name}.got" > "${tmp}/${name}.diff"; then
    printf 'FAIL %s: findings differ from expected.txt (- expected, + reported)\n' "${name}"
    sed 's/^/    /' "${tmp}/${name}.diff"
    failures=$((failures + 1))
  else
    found=$(wc -l < "${tmp}/${name}.got")
    printf 'ok   %s (%s finding(s))\n' "${name}" "${found//[[:space:]]/}"
  fi
done

# Every documented rule must be proven by at least one fixture.
sed -n -E 's/^#     ([a-z][a-z0-9-]+) .*/\1/p' "${scanner}" | LC_ALL=C sort -u > "${tmp}/rules"
if [[ ! -s "${tmp}/rules" ]]; then
  printf 'FAIL: no rule ids found in the header of %s\n' "${scanner}"
  failures=$((failures + 1))
fi
while IFS= read -r rule; do
  if ! grep -q -F "[${rule}]" "${tmp}/all-expected"; then
    printf 'FAIL: rule %s has no fixture that triggers it\n' "${rule}"
    failures=$((failures + 1))
  fi
done < "${tmp}/rules"
rules=$(wc -l < "${tmp}/rules")

if [[ "${failures}" -gt 0 ]]; then
  printf '\n%s failure(s)\n' "${failures}"
  exit 1
fi
printf '\nall %s cases pass; every one of the %s rules is covered\n' "${cases}" "${rules//[[:space:]]/}"
