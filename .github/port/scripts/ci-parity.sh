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
#      into it, and the patch is applied there (git apply, which fails on any rejected hunk);
#   2. go test ./parity/...
#   3. go test -race on the harness packages that run concurrently
#   4. go test on the upstream packages the seams change (config, router, server, exchange)
#   5. the Go tooling module crates/parity/testdata/go builds and its tests pass against the
#      assembled copy (a temporary go.work replaces the upstream module with it, as its run.sh
#      does)
#   6. parity selftest over the sample-requests, adapters and exchangetest corpora
#   7. parity run against the Rust server (built first when the workspace has pbs-server; without
#      it every case is not-yet-instrumented), ratcheted against parity/baseline.json
# Steps 2-7 run in the assembled copy; its parity/out/ holds the artifacts. The last line of
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

git -C "${go_root}" archive --format=tar HEAD | tar -x -C "${assembled}"
cp -R parity "${assembled}/parity"
ceiling=$(dirname "${assembled}")
if ! (cd "${assembled}" && env GIT_CEILING_DIRECTORIES="${ceiling}" git apply --check "${patch}"); then
  port_die "ci-parity: parity/upstream-seams.patch does not apply to upstream ${pin:0:12}"
fi
(cd "${assembled}" && env GIT_CEILING_DIRECTORIES="${ceiling}" git apply "${patch}")
printf 'ci-parity: assembled upstream %s, parity/ and the seams patch in %s\n' "${pin:0:12}" "${assembled}"

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
go test -count=1 ./config ./router ./server ./exchange

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

go run ./parity/cmd/parity selftest --corpus sample-requests,adapters,exchangetest

metadata=$(cd "${top}" && cargo metadata --no-deps --format-version 1 --locked)
has_server=$(jq -r '[.packages[] | select(.name == "pbs-server")] | length > 0' <<< "${metadata}")
run=(go run ./parity/cmd/parity run --impl-b rust)
if [[ "${has_server}" == true ]]; then
  (cd "${top}" && cargo build --release --locked -p pbs-server)
  target_dir=$(jq -r '.target_directory' <<< "${metadata}")
  run+=(--bin-b "${target_dir}/release/pbs-server")
else
  printf '::notice::the workspace has no pbs-server yet; the Rust side of the parity run is not-yet-instrumented\n'
fi
if [[ -f parity/baseline.json ]]; then
  run+=(--baseline parity/baseline.json)
fi
"${run[@]}"
printf '%s\n' "${assembled}"
