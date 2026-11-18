#!/usr/bin/env bash
# Regenerates the Go goldens of pbs-parity (testdata/golden) from the real
# parity packages, and records the Go schema validator's verdict on the
# documents the Rust crate writes (testdata/documents).
#
# The parity packages are the harness of the parity-harness branch, the only
# branch that holds it: parity/, the seams it needs in upstream files
# (parity/upstream-seams.patch) and parity/scripts/build-go.sh, which
# assembles the two with the pinned upstream checkout. pbs-parity's goldens
# are regenerated on that branch; on a branch without the harness this
# script stops and says so.
#
# Usage: crates/parity/testdata/go/run.sh [ref]
#
# The ref (default HEAD) names the commit of this repository whose parity/
# and UPSTREAM_SYNC the goldens are built from, and which they record. The
# pinned upstream checkout is $PBS_GO_ROOT when it is set, else .upstream,
# where scripts/fetch-upstream.sh checks it out.
#
# Steps:
#   1. the ref's parity/ and UPSTREAM_SYNC are exported into a temporary
#      directory, whose parity/scripts/build-go.sh assembles a tree from them
#      and the pinned upstream checkout there (build-go.sh --no-build), and a
#      temporary go.work uses this module (main.go, and the generated cases
#      of fuzz.go, docfuzz.go, metricsfuzz.go and bodyfuzz.go) with the tree
#      in place of the upstream module: the program needs the parity
#      packages, which the upstream checkout its go.mod names does not have.
#      The sources must be gofmt -s clean;
#   2. it writes testdata/golden, including the scripted and generated
#      sessions, documents, metric deltas, bodies and fake cases the Rust
#      crate replays;
#   3. the Rust golden test rewrites testdata/documents (PBS_PARITY_REGEN=1),
#      including the traces of those sessions;
#   4. the program runs again, so verdicts.json holds the Go schema verdict on
#      exactly those documents;
#   5. the temporary directory is removed, and the Rust tests run in both
#      modes against the new goldens.
set -euo pipefail
shopt -s inherit_errexit

ref="${1:-HEAD}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
crate="$(cd "${here}/../.." && pwd)"
workspace="$(cd "${crate}/../.." && pwd)"
repo="$(git -C "${crate}" rev-parse --show-toplevel)"
sha="$(git -C "${repo}" rev-parse --verify "${ref}^{commit}")"
upstream_module="github.com/prebid/prebid-server/v4"
go_version="$(sed -n 's/^go //p' "${here}/go.mod")"

if ! git -C "${repo}" cat-file -e "${sha}:parity/scripts/build-go.sh" 2>/dev/null; then
	echo "${ref} (${sha}) does not hold the parity harness (parity/scripts/build-go.sh):" \
		"pbs-parity's goldens are regenerated on the parity-harness branch; run this script there" >&2
	exit 1
fi

# The pinned upstream checkout, named for the exported build-go.sh: a
# relative PBS_GO_ROOT is taken from this repository's root, as everywhere.
go_root="${PBS_GO_ROOT:-${workspace}/.upstream}"
if [[ "${go_root}" != /* ]]; then
	go_root="${workspace}/${go_root}"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT
harness="${tmp}/harness"
tree="${tmp}/tree"

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

mkdir -p "${harness}"
git -C "${repo}" archive --format=tar "${sha}" -- UPSTREAM_SYNC parity >"${tmp}/harness.tar"
tar -x -f "${tmp}/harness.tar" -C "${harness}"
PBS_GO_ROOT="${go_root}" "${harness}/parity/scripts/build-go.sh" --no-build --tree "${tree}" >/dev/null
tree_module="$(sed -n 1p "${tree}/go.mod")"
if [[ "${tree_module}" != "module ${upstream_module}" || ! -d "${tree}/parity" ]]; then
	echo "build-go.sh of ${ref} (${sha}) did not assemble the upstream server with the parity packages" >&2
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
