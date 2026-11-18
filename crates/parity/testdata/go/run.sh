#!/usr/bin/env bash
# Regenerates the Go goldens of pbs-parity (testdata/golden) from the real
# parity packages of the parity-harness branch, and records the Go schema
# validator's verdict on the documents the Rust crate writes
# (testdata/documents).
#
# Usage: crates/parity/testdata/go/run.sh [parity-harness ref]
#
# The ref (default parity-harness) names a commit of this repository that holds
# the upstream Go server with the parity packages (module
# github.com/prebid/prebid-server/v4, parity/ included), such as
# origin/parity-harness in a clone.
#
# Steps:
#   1. a temporary detached worktree of the ref is checked out, and a temporary
#      go.work uses this module (main.go, and the generated cases of fuzz.go,
#      docfuzz.go, metricsfuzz.go and bodyfuzz.go) with the worktree in place of
#      the upstream module: the program needs the parity packages, which the
#      upstream checkout its go.mod names does not have. The sources must be
#      gofmt -s clean;
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
upstream_module="github.com/prebid/prebid-server/v4"
go_version="$(sed -n 's/^go //p' "${here}/go.mod")"
tmp="$(mktemp -d)"
tree="${tmp}/parity-harness"

cleanup() {
	git -C "${repo}" worktree remove --force "${tree}" 2>/dev/null || true
	rm -rf "${tmp}"
}
trap cleanup EXIT

# go_quote PATH prints PATH as a Go string literal, the form go.work takes
# paths in.
go_quote() {
	local s="${1//\\/\\\\}"
	s="${s//\"/\\\"}"
	printf '"%s"' "${s}"
}

golden() {
	(cd "${here}" && GOWORK="${tmp}/go.work" go run . \
		-contract "${crate}/contract" \
		-documents "${crate}/testdata/documents" \
		-out "${crate}/testdata/golden" \
		-ref "${sha}")
}

unformatted="$(gofmt -s -l "${here}")"
if [[ -n "${unformatted}" ]]; then
	echo "gofmt -s: ${unformatted}" >&2
	exit 1
fi

git -C "${repo}" worktree add --detach "${tree}" "${sha}" >/dev/null
tree_module="$(sed -n 1p "${tree}/go.mod")"
if [[ "${tree_module}" != "module ${upstream_module}" || ! -d "${tree}/parity" ]]; then
	echo "${ref} (${sha}) is not the upstream server with the parity packages" >&2
	exit 1
fi
# The go.work.sum starts as the tree's go.sum, for the packages of the tree
# that this module does not import itself.
use_dir="$(go_quote "${here}")"
replace_dir="$(go_quote "${tree}")"
printf 'go %s\n\nuse %s\n\nreplace %s => %s\n' "${go_version}" "${use_dir}" \
	"${upstream_module}" "${replace_dir}" >"${tmp}/go.work"
cp "${tree}/go.sum" "${tmp}/go.work.sum"

golden
(cd "${workspace}" && PBS_PARITY_REGEN=1 cargo test -p pbs-parity --features trace documents_validate_with_go)
golden

(cd "${workspace}" && cargo test -p pbs-parity && cargo test -p pbs-parity --features trace)
