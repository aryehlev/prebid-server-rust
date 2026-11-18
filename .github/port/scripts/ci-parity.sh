#!/usr/bin/env bash
# ci-parity.sh: run the Go-versus-Rust parity harness that lives on the parity-harness branch
# (parity/, see parity/README.md there). Exits 0 with a notice when parity/ does not exist.
#   1. go test ./parity/...
#   2. go test -race on the harness packages that run concurrently
#   3. go test on the upstream packages the harness patches (config, router, server, exchange)
#   4. parity selftest over the sample-requests, adapters and exchangetest corpora
#   5. parity run against the Rust server (built first when the workspace has pbs-server; without
#      it every case is not-yet-instrumented), ratcheted against parity/baseline.json
# Artifacts are written to parity/out/.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
if [[ ! -d parity ]]; then
  printf '::notice::parity/ does not exist on this branch; nothing to run\n'
  exit 0
fi

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
go run ./parity/cmd/parity selftest --corpus sample-requests,adapters,exchangetest

metadata=$(cd rust && cargo metadata --no-deps --format-version 1 --locked)
has_server=$(jq -r '[.packages[] | select(.name == "pbs-server")] | length > 0' <<< "${metadata}")
if [[ "${has_server}" == true ]]; then
  (cd rust && cargo build --release --locked -p pbs-server)
else
  printf '::notice::the workspace has no pbs-server yet; the Rust side of the parity run is not-yet-instrumented\n'
fi
run=(go run ./parity/cmd/parity run --impl-b rust)
if [[ -f parity/baseline.json ]]; then
  run+=(--baseline parity/baseline.json)
fi
"${run[@]}"
