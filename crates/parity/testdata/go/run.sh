#!/usr/bin/env bash
# Regenerates the Go goldens of pbs-parity (testdata/golden) from the real
# parity packages of the parity-harness branch, and records the Go schema
# validator's verdict on the documents the Rust crate writes
# (testdata/documents).
#
# Usage: rust/crates/parity/testdata/go/run.sh [parity-harness ref]
#
# Steps:
#   1. the program (main.go, and the generated cases of fuzz.go, docfuzz.go,
#      metricsfuzz.go and bodyfuzz.go) is built inside a temporary detached
#      worktree of the ref (it needs the parity packages, which this branch
#      does not have), after a gofmt -s and go vet check;
#   2. it writes testdata/golden, including the scripted and generated
#      sessions, documents, metric deltas, bodies and fake cases the Rust
#      crate replays;
#   3. the Rust golden test rewrites testdata/documents (PBS_PARITY_REGEN=1),
#      including the traces of those sessions;
#   4. the program runs again, so verdicts.json holds the Go schema verdict on
#      exactly those documents;
#   5. the worktree is removed, and the Rust tests run in both modes against
#      the new goldens.
set -euo pipefail

ref="${1:-parity-harness}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
crate="$(cd "${here}/../.." && pwd)"
workspace="$(cd "${crate}/../.." && pwd)"
repo="$(git -C "${crate}" rev-parse --show-toplevel)"
sha="$(git -C "${repo}" rev-parse --verify "${ref}^{commit}")"
tmp="$(mktemp -d)"
tree="${tmp}/parity-harness"

cleanup() {
	git -C "${repo}" worktree remove --force "${tree}" 2>/dev/null || true
	rm -rf "${tmp}"
}
trap cleanup EXIT

golden() {
	(cd "${tree}" && go run ./parity/cmd/rustgolden \
		-contract "${crate}/contract" \
		-documents "${crate}/testdata/documents" \
		-out "${crate}/testdata/golden" \
		-ref "${sha}")
}

git -C "${repo}" worktree add --detach "${tree}" "${sha}" >/dev/null
mkdir -p "${tree}/parity/cmd/rustgolden"
cp "${here}"/*.go "${tree}/parity/cmd/rustgolden/"
unformatted="$(cd "${tree}" && gofmt -s -l parity/cmd/rustgolden)"
if [ -n "${unformatted}" ]; then
	echo "gofmt -s: ${unformatted}" >&2
	exit 1
fi
(cd "${tree}" && go vet ./parity/cmd/rustgolden)

golden
(cd "${workspace}" && PBS_PARITY_REGEN=1 cargo test -p pbs-parity --features trace documents_validate_with_go)
golden

(cd "${workspace}" && cargo test -p pbs-parity && cargo test -p pbs-parity --features trace)
