#!/usr/bin/env bash
# conformance.sh: the known-failures ratchet of rust/conformance/README.md, evaluated on prepared
# reports (--results), plus the absent-harness, harness and harness-failure paths with a fake
# cargo.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

repo="${T_ROOT}/repo"
t_repo "${repo}"
mkdir -p "${repo}/rust"
a=adapters/foo/footest/exemplary/a.json
b=adapters/foo/footest/supplemental/b.json
c=adapters/bar/bartest/exemplary/c.json
for fixture in "${a}" "${b}" "${c}"; do
  t_put "${repo}" "${fixture}" '{}'
done
t_commit "${repo}" "fixtures" > /dev/null
export PBS_GO_ROOT="${repo}"
printf '{"totals": {"fixture_runs_total": 3, "distinct_fixture_files": 3}}\n' > "${T_ROOT}/manifest.json"

# row FIXTURE STATUS [RUN]: one report line.
row() {
  jq -n -c --arg fixture "${1:?}" --arg status "${2:?}" --arg run "${3:-foo/foo}" \
    '{run: $run, fixture: $fixture, status: $status, detail: (if $status == "fail" then "differs" else "" end)}'
}

# check NAME EXPECTED_RC REPORT KNOWN [ARGS...]: evaluate REPORT (lines) against the known list
# KNOWN (lines); the summary is in T_OUT.
check() {
  local name=${1:?} expected=${2:?} report=${3?} known=${4?}
  shift 4
  printf '%s\n' "${report}" > "${T_ROOT}/results.jsonl"
  printf '# known failures\n%s\n' "${known}" > "${T_ROOT}/known.txt"
  t_run_in "${repo}" "${T_SCRIPTS}/conformance.sh" --results "${T_ROOT}/results.jsonl" \
    --manifest "${T_ROOT}/manifest.json" --known "${T_ROOT}/known.txt" "$@"
  t_eq "${name}: exit status" "${expected}" "${T_RC}"
}

# field JQ: a value of the last summary.
field() {
  jq -c "${1:?}" <<< "${T_OUT}"
}

pass_a=$(row "${a}" pass)
fail_a=$(row "${a}" fail)
pass_b=$(row "${b}" pass)
fail_b=$(row "${b}" fail)
pass_c=$(row "${c}" pass)
fail_c=$(row "${c}" fail)
nl=$'\n'

check "all pass" 0 "${pass_a}${nl}${pass_b}${nl}${pass_c}" ""
status=$(field .status)
t_eq "all pass: status" '"pass"' "${status}"
counts=$(field '[.results, .fixtures, .failing, .known]')
t_eq "all pass: counts" '[3,3,0,0]' "${counts}"

check "new failure" 1 "${fail_a}${nl}${pass_b}${nl}${pass_c}" ""
value=$(field .new_failures)
t_eq "new failure: listed" "[\"${a}\"]" "${value}"

check "known failure" 0 "${fail_a}${nl}${pass_b}${nl}${pass_c}" "${a}"
check "stale entry" 1 "${pass_a}${nl}${pass_b}${nl}${pass_c}" "${a}"
value=$(field .stale)
t_eq "stale entry: listed" "[\"${a}\"]" "${value}"

printf '{"totals": {"fixture_runs_total": 2, "distinct_fixture_files": 2}}\n' > "${T_ROOT}/manifest.json"
check "missing entry" 1 "${pass_a}${nl}${pass_b}" "${c}"
value=$(field .missing)
t_eq "missing entry: listed" "[\"${c}\"]" "${value}"
printf '{"totals": {"fixture_runs_total": 3, "distinct_fixture_files": 3}}\n' > "${T_ROOT}/manifest.json"

check "strict adapter" 1 "${fail_a}${nl}${pass_b}${nl}${pass_c}" "${a}" --strict-dirs foo
value=$(field .strict_failures)
t_eq "strict adapter: listed" "[\"${a}\"]" "${value}"
check "strict elsewhere" 0 "${fail_a}${nl}${pass_b}${nl}${pass_c}" "${a}" --strict-dirs bar

bad=$(jq -n -c --arg fixture "${c}" '{run: "bar/bar", fixture: $fixture, status: "skip"}')
check "bad report line" 1 "${pass_a}${nl}${pass_b}${nl}${bad}" ""
value=$(field '.bad_lines | length')
t_eq "bad report line: counted" 1 "${value}"
check "not json" 1 "${pass_a}${nl}${pass_b}${nl}${pass_c}${nl}not json" ""

check "incomplete run" 1 "${pass_a}${nl}${pass_b}" ""
value=$(field '.incomplete | type')
t_eq "incomplete run: explained" '"string"' "${value}"
check "subset run" 0 "${pass_a}${nl}${pass_b}" "${c}" --dirs foo

check "unsorted list" 1 "${fail_a}${nl}${fail_b}${nl}${pass_c}" "${b}${nl}${a}"
value=$(field .known_errors)
t_has "unsorted list: explained" "${value}" "is not sorted"
check "duplicate entry" 1 "${fail_a}${nl}${pass_b}${nl}${pass_c}" "${a}${nl}${a}"
check "entry that does not exist" 1 "${pass_a}${nl}${pass_b}${nl}${pass_c}" "adapters/foo/footest/exemplary/zz.json"
value=$(field .known_errors)
t_has "entry that does not exist: explained" "${value}" "listed fixture does not exist"

two_runs="${pass_a}${nl}$(row "${a}" fail foo/fooalias)${nl}${pass_b}${nl}${pass_c}"
printf '{"totals": {"fixture_runs_total": 4, "distinct_fixture_files": 3}}\n' > "${T_ROOT}/manifest.json"
check "one failing run fails the fixture" 1 "${two_runs}" ""
value=$(field .new_failures)
t_eq "one failing run: listed" "[\"${a}\"]" "${value}"
check "one failing run, known" 0 "${two_runs}" "${a}"
printf '{"totals": {"fixture_runs_total": 3, "distinct_fixture_files": 3}}\n' > "${T_ROOT}/manifest.json"

summary="${T_ROOT}/summary.md"
check "summary and out files" 1 "${fail_c}${nl}${pass_a}${nl}${pass_b}" "" --summary "${summary}" --out "${T_ROOT}/out.json"
markdown=$(cat "${summary}")
t_has "summary: heading" "${markdown}" "### Go fixture conformance: fail"
t_has "summary: new failure" "${markdown}" "${c}"
out_status=$(jq -r .status "${T_ROOT}/out.json")
t_eq "out: status" fail "${out_status}"

t_run_in "${repo}" "${T_SCRIPTS}/conformance.sh" --results
t_eq "a flag without a value is a usage error" 2 "${T_RC}"
t_run_in "${repo}" "${T_SCRIPTS}/conformance.sh" --results "${T_ROOT}/none.jsonl" --known "${T_ROOT}/known.txt"
t_eq "a missing report is a harness error" 2 "${T_RC}"

# A fake cargo: metadata from FAKE_METADATA, and the harness writes FAKE_RESULTS to
# PBS_FIXTURE_RESULTS and exits FAKE_TEST_RC.
cat > "${T_ROOT}/bin/cargo" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  metadata) cat "${FAKE_METADATA:?}" ;;
  test)
    printf '%s\n' "${PBS_FIXTURE_DIRS-}" > "${FAKE_DIRS_SEEN:?}"
    if [[ -n "${PBS_FIXTURE_RESULTS:-}" ]]; then
      cp "${FAKE_RESULTS:?}" "${PBS_FIXTURE_RESULTS}"
    fi
    exit "${FAKE_TEST_RC:-0}"
    ;;
  *) exit 3 ;;
esac
EOF
chmod +x "${T_ROOT}/bin/cargo"
export FAKE_METADATA="${T_ROOT}/metadata.json" FAKE_RESULTS="${T_ROOT}/fake-results.jsonl" \
  FAKE_DIRS_SEEN="${T_ROOT}/dirs-seen.txt"
printf '{"packages": [{"name": "pbs-x", "targets": [{"name": "pbs_x", "kind": ["lib"]}]}]}\n' > "${FAKE_METADATA}"
printf '# known failures\n' > "${T_ROOT}/known.txt"

t_run_in "${repo}" "${T_SCRIPTS}/conformance.sh" --known "${T_ROOT}/known.txt"
t_eq "absent harness passes" 0 "${T_RC}"
harness=$(field .harness)
t_eq "absent harness is reported" '"absent"' "${harness}"
t_run_in "${repo}" env CONFORMANCE_REQUIRED=true "${T_SCRIPTS}/conformance.sh" --known "${T_ROOT}/known.txt"
t_eq "absent harness fails when required" 1 "${T_RC}"

printf '{"packages": [{"name": "pbs-adapters", "targets": [{"name": "go_fixtures", "kind": ["test"]}]}]}\n' > "${FAKE_METADATA}"
printf '%s\n%s\n%s\n' "${pass_a}" "${pass_b}" "${pass_c}" > "${FAKE_RESULTS}"
t_run_in "${repo}" "${T_SCRIPTS}/conformance.sh" --manifest "${T_ROOT}/manifest.json" --known "${T_ROOT}/known.txt"
t_eq "present harness passes" 0 "${T_RC}"
harness=$(field .harness)
t_eq "present harness is reported" '"present"' "${harness}"
t_run_in "${repo}" "${T_SCRIPTS}/conformance.sh" --manifest "${T_ROOT}/manifest.json" --known "${T_ROOT}/known.txt" \
  --dirs foo,bar
seen=$(cat "${FAKE_DIRS_SEEN}")
t_eq "--dirs reaches the harness" "foo,bar" "${seen}"
t_run_in "${repo}" env FAKE_TEST_RC=101 "${T_SCRIPTS}/conformance.sh" --manifest "${T_ROOT}/manifest.json" \
  --known "${T_ROOT}/known.txt"
t_eq "a harness that fails in report mode is a harness error" 2 "${T_RC}"

t_done
