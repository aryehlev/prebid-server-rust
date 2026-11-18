#!/usr/bin/env bash
# ci-parity.sh on a fake parity-harness checkout: a fake upstream at the pin, parity/ with a seams
# patch, fake go, gofmt, cargo and seams.sh that log what they are asked in one log, in order.
#   - the checks of the Go side: gofmt -s of the Go files the patch leaves, seams.sh check, the
#     parity scripts' own module, the harness, and go test of every package the patch touches
#     (from the patch's file list: a new package, a deleted file, a file with no Go beside it)
#     with the root package;
#   - the Go-versus-Rust run, decided by cargo metadata: skipped with a notice that says why when
#     the workspace has no bin target named pbs-server (a package of that name with no such bin
#     counts as none), and otherwise always run, with --bin-b and the baseline when there is one,
#     where every nonzero exit, 3 (incomplete) included, fails the job.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

log="${T_ROOT}/calls.log"
export FAKE_LOG="${log}"
cat > "${T_ROOT}/bin/go" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'go %s | %s\n' "${PWD}" "$*" >> "${FAKE_LOG:?}"
case "$*" in
  "run ./parity/cmd/parity run "*) exit "${FAKE_PARITY_RC:-0}" ;;
  "test "*)
    if [[ -n "${FAKE_GO_TEST_FAILS:-}" && "$*" == *"${FAKE_GO_TEST_FAILS}"* ]]; then
      printf -- '--- FAIL: TestSomething\n'
      exit 1
    fi
    ;;
  *) ;;
esac
EOF
cat > "${T_ROOT}/bin/gofmt" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'gofmt %s | %s\n' "${PWD}" "$*" >> "${FAKE_LOG:?}"
for arg in "$@"; do
  if [[ " ${FAKE_GOFMT_BAD:-} " == *" ${arg} "* ]]; then
    printf '%s\n' "${arg}"
  fi
done
EOF
cat > "${T_ROOT}/bin/cargo" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'cargo %s | %s\n' "${PWD}" "$*" >> "${FAKE_LOG:?}"
case " $* " in
  *" metadata "*) cat "${FAKE_CARGO_METADATA:?}" ;;
  *) ;;
esac
EOF
chmod +x "${T_ROOT}/bin/go" "${T_ROOT}/bin/gofmt" "${T_ROOT}/bin/cargo"

# The upstream checkout at the pin.
up="${T_ROOT}/upstream"
t_repo "${up}"
printf 'module github.com/prebid/prebid-server/v4\n\ngo 1.25.0\n' > "${up}/go.mod"
t_put "${up}" go.sum 'example.com/m v1.0.0 h1:x'
t_put "${up}" main.go 'package main // v1'
t_put "${up}" config/config.go 'package config // v1'
t_put "${up}" adapters/foo/foo.go 'package foo // v1'
t_put "${up}" adapters/foo/foo_extra.go 'package foo // extra'
t_put "${up}" exchange/exchange.go 'package exchange // v1'
t_put "${up}" static/bidder-info/foo.yaml 'endpoint: "http://foo.example"'
pin=$(t_commit "${up}" "upstream")

# The seams: changed, added and deleted Go files, a new package, and a file with no Go beside it.
t_put "${up}" main.go 'package main // v1, with a seam'
t_put "${up}" config/config.go 'package config // v1, with a seam'
t_put "${up}" config/parity.go 'package config // parity'
t_put "${up}" exchange/entities/parity.go 'package entities // parity'
t_put "${up}" static/bidder-info/parity.yaml 'endpoint: "http://parity.example"'
rm "${up}/adapters/foo/foo_extra.go"
git -C "${up}" add -A
seams_patch=$(git -C "${up}" diff --cached --no-renames HEAD)
git -C "${up}" reset -q --hard
export PBS_GO_ROOT="${up}"

# ws: a parity-harness checkout pinned to the upstream commit.
ws="${T_ROOT}/ws"
t_repo "${ws}"
printf '# The upstream commit the port is pinned to.\n%s\n' "${pin}" > "${ws}/UPSTREAM_SYNC"
t_put "${ws}" parity/cmd/parity/main.go 'package main // the runner'
printf '%s\n' "${seams_patch}" > "${ws}/parity/upstream-seams.patch"
mkdir -p "${ws}/parity/scripts"
printf 'module github.com/aryehlev/prebid-server-rust/parity/scripts\n\ngo 1.25.0\n' > "${ws}/parity/scripts/go.mod"
cat > "${ws}/parity/scripts/seams.sh" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'seams %s | %s\n' "${PWD}" "$*" >> "${FAKE_LOG:?}"
exit "${FAKE_SEAMS_RC:-0}"
EOF
chmod +x "${ws}/parity/scripts/seams.sh"
mkdir -p "${ws}/crates/parity/testdata/go"
printf 'module example.com/paritytool\n\ngo 1.25.0\n' > "${ws}/crates/parity/testdata/go/go.mod"
export RUNNER_TEMP="${T_ROOT}/runner"
mkdir -p "${RUNNER_TEMP}" "${T_ROOT}/tmp"
assembled="${RUNNER_TEMP}/parity-go"

# metadata FILE PACKAGE:TARGET:KIND...: a cargo metadata document with those targets.
metadata() {
  local file=${1:?} spec packages=() name target kind
  shift
  for spec in "$@"; do
    IFS=: read -r name target kind <<< "${spec}"
    packages+=("$(jq -n -c --arg n "${name}" --arg t "${target}" --arg k "${kind}" \
      '{name: $n, targets: [{name: $t, kind: [$k]}]}')")
  done
  printf '%s\n' "${packages[@]}" | jq -s -c '{packages: ., target_directory: "/fake/target", version: 1}' > "${file}"
}
none="${T_ROOT}/meta-none.json"
metadata "${none}" pbs-parity:pbs_parity:lib pbs-devtools:pbs-devtools:bin pbs-server:pbs_server:lib
server="${T_ROOT}/meta-server.json"
metadata "${server}" pbs-parity:pbs_parity:lib pbs-app:pbs-server:bin
twice="${T_ROOT}/meta-twice.json"
metadata "${twice}" pbs-app:pbs-server:bin pbs-other:pbs-server:bin

# parity NAME [VAR=VALUE...]: run ci-parity.sh in the checkout with a fresh log.
parity() {
  local name=${1:?}
  shift
  : > "${log}"
  t_run_in "${ws}" env TMPDIR="${T_ROOT}/tmp" "$@" "${T_SCRIPTS}/ci-parity.sh"
  CALLS=$(cat "${log}")
  if [[ "${T_RC}" -ne 0 && "${name}" == ok* ]]; then
    printf '%s\n%s\n' "${T_OUT}" "${T_ERR}"
  fi
}

# position NEEDLE: the line of the log NEEDLE is first on (0 when it is on none).
position() {
  local found
  found=$(grep -n -F -m 1 -e "${1:?}" "${log}" || true)
  printf '%s\n' "${found%%:*}"
}

# --- No Rust server: the Go side is checked, the run is skipped with a notice ------------------
parity ok-no-server FAKE_CARGO_METADATA="${none}"
t_eq "no server: exit status" 0 "${T_RC}"
t_has "no server: the notice says why" "${T_OUT}" \
  "::notice::parity run skipped: the workspace has no bin target named pbs-server (cargo metadata)"
last=$(tail -n 1 <<< "${T_OUT}")
t_eq "no server: the last line is the assembled copy" "${assembled}" "${last}"
t_lacks "no server: no parity run" "${CALLS}" "parity run"
t_lacks "no server: nothing is built" "${CALLS}" "| build --release"
t_has "no server: the selftest still runs" "${CALLS}" "go ${assembled} | run ./parity/cmd/parity selftest"
config_go=$(cat "${assembled}/config/config.go")
t_has "the patch is applied in the assembled copy" "${config_go}" "with a seam"
if [[ -e "${assembled}/adapters/foo/foo_extra.go" ]]; then
  t_fail "the deleted file is still in the assembled copy"
else
  t_ok "the deleted file is gone from the assembled copy"
fi
t_has "gofmt -s -l over the Go files the patch leaves, in the assembled copy" "${CALLS}" \
  "gofmt ${assembled} | -s -l -- config/config.go config/parity.go exchange/entities/parity.go main.go"
t_has "seams.sh check of the branch's patch" "${CALLS}" "seams ${ws} | check --patch ${ws}/parity/upstream-seams.patch"
t_has "the parity scripts' own module" "${CALLS}" "go ${ws}/parity/scripts | test -count=1 ./..."
t_has "the harness" "${CALLS}" "go ${assembled} | test -count=1 ./parity/..."
t_has "every package the patch touches, and the root package" "${CALLS}" \
  "go ${assembled} | test -count=1 . ./adapters/foo ./config ./exchange/entities"
t_has "the summary names them" "${T_OUT}" "go test of the 4 package(s) the patch touches"
t_has "the tooling module that needs parity/" "${CALLS}" "go ${ws}/crates/parity/testdata/go | test -count=1 ./..."
order=("gofmt ${assembled}" "seams ${ws}" "go ${ws}/parity/scripts" "test -count=1 ./parity/..."
  "test -count=1 . ./adapters" "go ${ws}/crates/parity/testdata/go | build" "selftest" "metadata")
previous=0
in_order=yes
for needle in "${order[@]}"; do
  at=$(position "${needle}")
  if [[ -z "${at}" || "${at}" -le "${previous}" ]]; then
    in_order="no (${needle} at line ${at:-none})"
  fi
  previous=${at:-0}
done
t_eq "the steps run in order" yes "${in_order}"

# --- A Rust server: the run always happens, and only exit 0 passes ----------------------------
parity ok-server FAKE_CARGO_METADATA="${server}"
t_eq "server: exit status" 0 "${T_RC}"
t_has "server: built in release mode from the workspace" "${CALLS}" \
  "cargo ${ws} | build --release --locked -p pbs-app --bin pbs-server"
t_has "server: run with --bin-b" "${CALLS}" \
  "go ${assembled} | run ./parity/cmd/parity run --impl-b rust --bin-b /fake/target/release/pbs-server"
t_lacks "server: no baseline on the branch, none passed" "${CALLS}" "--baseline"
t_lacks "server: no skip notice" "${T_OUT}" "parity run skipped"
build_at=$(position "| build --release")
run_at=$(position "parity run --impl-b")
if [[ -n "${build_at}" && -n "${run_at}" && "${build_at}" -lt "${run_at}" ]]; then
  t_ok "server: built before the run"
else
  t_fail "server: built before the run (build at line ${build_at:-none}, run at line ${run_at:-none})"
fi

t_put "${ws}" parity/baseline.json '{"cases": {}}'
parity ok-baseline FAKE_CARGO_METADATA="${server}"
t_eq "baseline: exit status" 0 "${T_RC}"
t_has "baseline: the run is ratcheted against it" "${CALLS}" \
  "run --impl-b rust --bin-b /fake/target/release/pbs-server --baseline parity/baseline.json"

for case in "3|incomplete (exit status 3)" "1|Go and Rust differ" "2|refused its arguments (exit status 2)" \
  "7|exited with status 7"; do
  rc=${case%%|*}
  message=${case#*|}
  parity "exit-${rc}" FAKE_CARGO_METADATA="${server}" FAKE_PARITY_RC="${rc}"
  t_eq "the run exits ${rc}: the job fails" 1 "${T_RC}"
  t_has "the run exits ${rc}: the error says so" "${T_ERR}" "${message}"
done
rm "${ws}/parity/baseline.json"

parity twice FAKE_CARGO_METADATA="${twice}"
t_eq "two pbs-server bins: refused" 1 "${T_RC}"
t_has "two pbs-server bins: both named" "${T_ERR}" "pbs-app, pbs-other"
t_lacks "two pbs-server bins: no run" "${CALLS}" "parity run"

# --- The Go checks fail the job before anything later runs ------------------------------------
parity gofmt FAKE_CARGO_METADATA="${none}" FAKE_GOFMT_BAD=config/parity.go
t_eq "gofmt: a patched file that is not gofmt -s clean fails" 1 "${T_RC}"
t_has "gofmt: the file is named" "${T_ERR}" "config/parity.go"
t_lacks "gofmt: seams.sh does not run" "${CALLS}" "seams "
parity seams FAKE_CARGO_METADATA="${none}" FAKE_SEAMS_RC=1
t_eq "seams: a patch that is not in canonical form fails" 1 "${T_RC}"
t_lacks "seams: the harness tests do not run" "${CALLS}" "./parity/..."
parity scripts-test FAKE_CARGO_METADATA="${none}" FAKE_GO_TEST_FAILS="test -count=1 ./..."
t_eq "the parity scripts' tests fail: the job fails" 1 "${T_RC}"
parity package-test FAKE_CARGO_METADATA="${none}" FAKE_GO_TEST_FAILS="./exchange/entities"
t_eq "a patched package's tests fail: the job fails" 1 "${T_RC}"
t_lacks "a patched package's tests fail: no selftest" "${CALLS}" "selftest"

mv "${ws}/parity/scripts/go.mod" "${T_ROOT}/scripts-go.mod"
parity no-scripts-module FAKE_CARGO_METADATA="${none}"
t_eq "parity/scripts without its go.mod: refused" 1 "${T_RC}"
t_has "parity/scripts without its go.mod: said" "${T_ERR}" "parity/scripts/go.mod does not exist"
mv "${T_ROOT}/scripts-go.mod" "${ws}/parity/scripts/go.mod"

cp "${ws}/parity/upstream-seams.patch" "${T_ROOT}/seams.keep"
sed -e 's|^-package main // v1$|-package main // v0|' "${T_ROOT}/seams.keep" > "${ws}/parity/upstream-seams.patch"
parity stale FAKE_CARGO_METADATA="${none}"
t_eq "a patch that does not apply: refused" 1 "${T_RC}"
t_has "a patch that does not apply: said" "${T_ERR}" "does not apply to upstream ${pin:0:12}"
cp "${T_ROOT}/seams.keep" "${ws}/parity/upstream-seams.patch"

# --- No parity/: nothing to run ---------------------------------------------------------------
mv "${ws}/parity" "${T_ROOT}/parity.keep"
parity no-parity FAKE_CARGO_METADATA="${none}"
t_eq "no parity/: exit status" 0 "${T_RC}"
t_has "no parity/: the notice" "${T_OUT}" "parity/ does not exist on this branch"
t_eq "no parity/: nothing runs" "" "${CALLS}"

t_done
