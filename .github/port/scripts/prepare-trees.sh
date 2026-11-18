#!/usr/bin/env bash
# prepare-trees.sh: check out the upstream trees of a port run with scripts/fetch-upstream.sh (from
# the trusted tooling), taking the commits from the upstream mirror:
#   OLD_GO_ROOT  the old pin (BASE_SHA), read-only: what the Rust port covered so far
#   PBS_GO_ROOT  the new pin (TARGET_SHA), read-only: the Go root every test, tool and gate reads;
#                it must be .upstream in the work tree, where the Go tooling modules' replace
#                directives point
#   PROBE_ROOT   a second, writable checkout of the new pin, where the agents may add Go probe
#                tests named zz_parity_scratch_test.go (check-worktree.sh resets it after each
#                session)
# The read-only trees are made read-only on disk (chmod -R a-w), and check-worktree.sh fails the
# run when either is no longer a clean checkout of its pin.
#
# Env: BASE_SHA TARGET_SHA OLD_GO_ROOT PBS_GO_ROOT PROBE_ROOT UPSTREAM_GIT, PBS_UPSTREAM_URL
#      (where a commit the mirror lacks is fetched from).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
top=$(pwd -P)
base=${BASE_SHA:?}
target=${TARGET_SHA:?}
old_root=${OLD_GO_ROOT:?}
new_root=${PBS_GO_ROOT:?}
probe_root=${PROBE_ROOT:?}
mirror=${UPSTREAM_GIT:?}
for sha in "${base}" "${target}"; do
  if [[ ! "${sha}" =~ ^[0-9a-f]{40}$ ]]; then
    port_die "prepare-trees: '${sha}' is not a full commit"
  fi
done
for dir in "${old_root}" "${new_root}" "${probe_root}"; do
  if [[ "${dir}" != /* ]]; then
    port_die "prepare-trees: ${dir} must be an absolute path"
  fi
done

# fetch SHA DIR: a clean detached checkout of SHA in DIR.
fetch() {
  PBS_UPSTREAM_REFERENCE=${mirror} port_fetch_upstream --sha "${1:?}" --dir "${2:?}" > /dev/null
}

fetch "${target}" "${new_root}"
new_abs=$(cd "${new_root}" && pwd -P)
if [[ "${new_abs}" != "${top}/.upstream" ]]; then
  port_die "prepare-trees: PBS_GO_ROOT must be .upstream at the top of the work tree (${top}/.upstream), where the Go tooling modules point; it is ${new_abs}"
fi
fetch "${base}" "${old_root}"
fetch "${target}" "${probe_root}"
chmod -R a-w "${old_root}" "${new_root}"
printf 'prepare-trees: old %s in %s, new %s in %s (both read-only), probes in %s\n' \
  "${base:0:12}" "${old_root}" "${target:0:12}" "${new_root}" "${probe_root}"
