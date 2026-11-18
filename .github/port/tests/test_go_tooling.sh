#!/usr/bin/env bash
# go-tooling.sh: gofmt -s over every tracked or new .go file, go build (into a temporary
# directory) and go test with -mod=readonly in every Go tooling module except
# crates/parity/testdata/go, nothing written into the tree, and no lint step, with fake go and
# gofmt on PATH (CI's Go tooling job runs the real ones).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

calls="${T_ROOT}/go-calls.log"
: > "${calls}"
cat > "${T_ROOT}/bin/go" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s | %s | GOFLAGS=%s\n' "${PWD#"${FAKE_TOP:?}/"}" "$*" "${GOFLAGS-unset}" >> "${FAKE_GO_CALLS:?}"
case "${1:-}" in
  build)
    if [[ "${2:-}" == -o ]]; then
      mkdir -p "${3:?}"
      printf 'binary\n' > "${3}/program"
    fi
    if [[ "${PWD}" == *"${FAKE_DIRTY:-none}"* ]]; then
      printf 'binary\n' > program
    fi
    ;;
  test)
    if [[ "${PWD}" == *"${FAKE_FAIL:-none}"* ]]; then
      printf -- '--- FAIL: TestDump\n'
      exit 1
    fi
    ;;
  *) exit 3 ;;
esac
EOF
cat > "${T_ROOT}/bin/gofmt" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'gofmt %s\n' "$*" >> "${FAKE_GO_CALLS:?}"
for file in "$@"; do
  if [[ "${file}" == "${FAKE_UNFORMATTED:-none}" ]]; then
    printf '%s\n' "${file}"
  fi
done
EOF
chmod +x "${T_ROOT}/bin/go" "${T_ROOT}/bin/gofmt"

repo="${T_ROOT}/repo"
t_repo "${repo}"
printf '/target/\n/.upstream/\n' > "${repo}/.gitignore"
for dir in crates/a/testdata/go crates/devtools/go crates/parity/testdata/go crates/b/target/go; do
  mkdir -p "${repo}/${dir}"
  t_go_mod "${dir}" v4.0.0-20260101000000-aaaaaaaaaaaa > "${repo}/${dir}/go.mod"
done
t_put "${repo}" crates/a/testdata/go/main.go 'package main'
t_put "${repo}" crates/devtools/go/dump.go 'package main'
t_put "${repo}" crates/parity/testdata/go/parity_test.go 'package main'
t_put "${repo}" .upstream/go.mod 'module github.com/prebid/prebid-server/v4'
t_commit "${repo}" "tooling" > /dev/null
export FAKE_GO_CALLS="${calls}" FAKE_TOP="${repo}"

t_run_in "${repo}" env GOFLAGS=-mod=mod "${T_SCRIPTS}/go-tooling.sh"
t_eq "clean: exit status" 0 "${T_RC}"
t_has "clean: modules counted" "${T_OUT}" "go-tooling: 2 module(s) built and tested"
t_has "clean: files counted" "${T_OUT}" "gofmt -s checked 3 file(s)"
log=$(cat "${calls}")
t_has "gofmt: simplify mode over the Go files" "${log}" \
  "gofmt -s -l crates/a/testdata/go/main.go crates/devtools/go/dump.go crates/parity/testdata/go/parity_test.go"
t_has "build: a tooling module, readonly, into a temporary directory" "${log}" \
  "crates/a/testdata/go | build -o "
t_has "build: readonly modules whatever GOFLAGS says" "${log}" "/bin/ ./... | GOFLAGS=-mod=readonly"
t_has "test: the devtools module" "${log}" "crates/devtools/go | test ./... | GOFLAGS=-mod=readonly"
t_lacks "the parity tooling is skipped" "${log}" "crates/parity/testdata/go |"
t_lacks "build output directories are skipped" "${log}" "crates/b/target/go |"
t_lacks "no lint step" "${log}" "vet"
leftover=$(git -C "${repo}" status --porcelain --untracked-files=all)
t_eq "clean: nothing is written into the tree" "" "${leftover}"

t_put "${repo}" crates/a/testdata/go/new.go 'package main // new'
: > "${calls}"
t_run_in "${repo}" env FAKE_UNFORMATTED=crates/a/testdata/go/new.go "${T_SCRIPTS}/go-tooling.sh"
t_eq "unformatted: exit status" 1 "${T_RC}"
t_has "unformatted: a new file is checked and named" "${T_ERR}${T_OUT}" "crates/a/testdata/go/new.go"
rm -f "${repo}/crates/a/testdata/go/new.go"

: > "${calls}"
t_run_in "${repo}" env FAKE_FAIL=crates/a/testdata/go "${T_SCRIPTS}/go-tooling.sh"
t_eq "failing test: exit status" 1 "${T_RC}"
t_has "failing test: the module is named" "${T_ERR}" "crates/a/testdata/go does not build or its tests fail"
log=$(cat "${calls}")
t_has "failing test: the other modules still run" "${log}" "crates/devtools/go | test ./..."

t_run_in "${repo}" env FAKE_DIRTY=crates/devtools/go "${T_SCRIPTS}/go-tooling.sh"
t_eq "dirty: exit status" 1 "${T_RC}"
t_has "dirty: explained" "${T_ERR}" "the Go runs changed the work tree"
rm -f "${repo}/crates/devtools/go/program"

rm -rf "${repo}/.upstream"
t_run_in "${repo}" "${T_SCRIPTS}/go-tooling.sh"
t_eq "no upstream checkout: exit status" 1 "${T_RC}"
t_has "no upstream checkout: explained" "${T_ERR}" "run scripts/fetch-upstream.sh first"

t_done
