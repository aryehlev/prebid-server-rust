#!/usr/bin/env bash
# conformance.sh: run the Rust Go-fixture harness in report mode and apply the known-failures
# ratchet that rust/conformance/README.md defines. Exit status 0 when the ratchet holds (or the
# harness is absent and CONFORMANCE_REQUIRED is not true), 1 when it does not, 2 on a usage or
# harness error.
#
# Usage: conformance.sh [--strict-dirs a,b] [--dirs a,b] [--results FILE] [--manifest FILE]
#                       [--known FILE] [--out FILE] [--summary FILE]
#   --strict-dirs  adapter directories the change touches: every one of their fixtures must pass
#   --dirs         run only these adapter directories (PBS_FIXTURE_DIRS); completeness and
#                  known-entry checks are then limited to them
#   --results      evaluate this report instead of running the harness
#   --manifest     default rust/testdata/go_fixture_manifest.json
#   --known        default rust/conformance/known_failures.txt
#   --out          write the JSON summary here (default: stdout only)
#   --summary      append a Markdown summary here (for example "${GITHUB_STEP_SUMMARY}")
# Env: PBS_GO_ROOT (default: the repository root), CONFORMANCE_REQUIRED (true makes an absent
# harness fail).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
root=$(pwd)
strict_csv=""
dirs_csv=""
results=""
manifest="rust/testdata/go_fixture_manifest.json"
known="rust/conformance/known_failures.txt"
out=""
summary=""
while [[ "$#" -gt 0 ]]; do
  if [[ "$#" -lt 2 ]]; then
    printf 'conformance: %s needs a value\n' "${1}" >&2
    exit 2
  fi
  case "${1}" in
    --strict-dirs) strict_csv=${2} ;;
    --dirs) dirs_csv=${2} ;;
    --results) results=${2:?--results needs a file} ;;
    --manifest) manifest=${2:?--manifest needs a file} ;;
    --known) known=${2:?--known needs a file} ;;
    --out) out=${2:?--out needs a file} ;;
    --summary) summary=${2:?--summary needs a file} ;;
    *)
      printf 'conformance: unknown argument %s\n' "${1}" >&2
      exit 2
      ;;
  esac
  shift 2
done
strict_lines=$(port_csv_lines "${strict_csv}")
dirs_lines=$(port_csv_lines "${dirs_csv}")
strict_json=$(jq -R -s -c 'split("\n") | map(select(length > 0))' <<< "${strict_lines}")
dirs_json=$(jq -R -s -c 'split("\n") | map(select(length > 0))' <<< "${dirs_lines}")
go_root=${PBS_GO_ROOT:-${root}}

tmp=$(mktemp -d)
trap 'rm -rf -- "${tmp}"' EXIT

emit() {
  local json=${1:?} status markdown
  if [[ -n "${out}" ]]; then
    printf '%s\n' "${json}" > "${out}"
  fi
  printf '%s\n' "${json}"
  if [[ -n "${summary}" ]]; then
    markdown=$(jq -r '
      def list($title; $items): if ($items | length) == 0 then empty else
        "\n**\($title)** (\($items | length))\n\n" + ($items[:50] | map("- `\(.)`") | join("\n"))
        + (if ($items | length) > 50 then "\n- ..." else "" end) end;
      "### Go fixture conformance: \(.status)\n",
      (if .harness == "absent" then "The `go_fixtures` harness of pbs-adapters does not exist yet." else
        "\(.results) results, \(.fixtures) fixtures, \(.failing) failing, \(.known) known failures." end),
      (if .incomplete then "\n**Incomplete run:** \(.incomplete)" else empty end),
      list("New failures"; .new_failures), list("Strict adapter failures"; .strict_failures),
      list("Stale known failures (now passing)"; .stale), list("Known failures that did not run"; .missing),
      list("Problems in the known-failures list"; .known_errors), list("Bad report lines"; .bad_lines)
    ' <<< "${json}")
    printf '%s\n' "${markdown}" >> "${summary}"
  fi
  status=$(jq -r '.status' <<< "${json}")
  if [[ "${status}" == pass ]]; then
    exit 0
  fi
  exit 1
}

absent() {
  local required=${CONFORMANCE_REQUIRED:-false} status=pass
  if [[ "${required}" == true ]]; then
    status=fail
    port_error "conformance: the go_fixtures harness is absent and CONFORMANCE_REQUIRED is true"
  else
    port_warning "conformance: harness absent (pbs-adapters has no go_fixtures test); passing"
  fi
  json=$(jq -n -c --arg status "${status}" '{harness: "absent", status: $status, results: 0, fixtures: 0,
    failing: 0, known: 0, new_failures: [], strict_failures: [], stale: [], missing: [],
    known_errors: [], bad_lines: [], incomplete: null}')
  emit "${json}"
}

if [[ -z "${results}" ]]; then
  metadata=$(cd rust && cargo metadata --no-deps --format-version 1 --locked)
  present=$(jq -r '[.packages[] | select(.name == "pbs-adapters") | .targets[]
    | select(.name == "go_fixtures" and (.kind | index("test")))] | length > 0' <<< "${metadata}")
  if [[ "${present}" != true ]]; then
    absent
  fi
  results="${tmp}/results.jsonl"
  dirs_env=$(jq -r 'join(",")' <<< "${dirs_json}")
  printf 'conformance: running the go_fixtures harness%s\n' "${dirs_env:+ for ${dirs_env}}" >&2
  if ! (cd rust && PBS_GO_ROOT="${go_root}" PBS_FIXTURE_DIRS="${dirs_env}" PBS_FIXTURE_RESULTS="${results}" \
    cargo test -p pbs-adapters --test go_fixtures --locked); then
    port_error "conformance: the harness failed in report mode (it must exit 0 when fixtures fail)"
    exit 2
  fi
fi
if [[ ! -f "${results}" ]]; then
  port_error "conformance: no report at ${results}"
  exit 2
fi
if [[ ! -f "${known}" ]]; then
  port_error "conformance: no known-failures list at ${known}"
  exit 2
fi
totals='null'
if [[ -f "${manifest}" ]]; then
  totals=$(jq -c '.totals // null' "${manifest}")
fi

# Problems with the list itself: order, duplicates and entries that do not exist.
known_entries=$(port_list "${known}")
known_errors="${tmp}/known-errors"
: > "${known_errors}"
if [[ -n "${known_entries}" ]]; then
  sorted=$(sort -u <<< "${known_entries}")
  if [[ "${sorted}" != "${known_entries}" ]]; then
    printf '%s is not sorted (LC_ALL=C) or has duplicate lines\n' "${known}" >> "${known_errors}"
  fi
  while IFS= read -r entry; do
    if [[ ! -f "${go_root}/${entry}" ]]; then
      printf 'listed fixture does not exist: %s\n' "${entry}" >> "${known_errors}"
    fi
  done <<< "${known_entries}"
fi

json=$(jq -n -c \
  --rawfile report "${results}" --arg known "${known_entries}" --rawfile known_errors "${known_errors}" \
  --argjson strict "${strict_json}" --argjson subset "${dirs_json}" --argjson totals "${totals}" '
  def lines($text): $text | split("\n") | map(select(test("\\S")));
  def under($dirs): . as $f | any($dirs[]; . as $d | $f | startswith("adapters/" + $d + "/"));
  def set: map({key: ., value: true}) | from_entries;
  (lines($report) | map(. as $line | (try (fromjson) catch null) as $row
    | if ($row | type) == "object" and ($row.fixture | type) == "string" and $row.fixture != ""
         and ($row.status == "pass" or $row.status == "fail")
      then {row: $row} else {bad: ($line | .[:200])} end)) as $parsed
  | ($parsed | map(select(has("row")) | .row)) as $rows
  | ($parsed | map(select(has("bad")) | .bad)) as $bad
  | ($rows | group_by(.fixture) | map({fixture: .[0].fixture, fail: any(.[]; .status == "fail")})) as $fixtures
  | ($fixtures | map(select(.fail) | .fixture)) as $failing
  | ($fixtures | map(select(.fail | not) | .fixture) | set) as $passing
  | ($fixtures | map(.fixture) | set) as $ran
  | (lines($known)) as $known_list
  | ($known_list | set) as $known_set
  | (if ($subset | length) == 0 then $known_list else $known_list | map(select(under($subset))) end) as $in_scope
  | (if ($subset | length) > 0 or $totals == null then null
     elif ($rows | length) != $totals.fixture_runs_total or ($fixtures | length) != $totals.distinct_fixture_files
     then "\($rows | length) results for \($fixtures | length) fixtures; the manifest has \($totals.fixture_runs_total) fixture runs of \($totals.distinct_fixture_files) fixtures"
     else null end) as $incomplete
  | {harness: "present",
     results: ($rows | length), fixtures: ($fixtures | length), failing: ($failing | length),
     known: ($known_list | length),
     new_failures: ($failing | map(select($known_set[.] | not))),
     strict_failures: ($failing | map(select(under($strict)))),
     stale: ($in_scope | map(select($passing[.]))),
     missing: ($in_scope | map(select($ran[.] | not))),
     known_errors: lines($known_errors),
     bad_lines: $bad,
     incomplete: $incomplete}
  | .status = (if (.new_failures + .strict_failures + .stale + .missing + .known_errors + .bad_lines | length) == 0
                  and .incomplete == null then "pass" else "fail" end)')
emit "${json}"
