#!/usr/bin/env bash
# upstream-pin.sh: print the upstream commit UPSTREAM_SYNC pins in the work tree, the way
# scripts/fetch-upstream.sh reads it, and set it as the step output "sha" when GITHUB_OUTPUT is
# set. CI keys the cache of the .upstream checkout on it.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
if [[ ! -f UPSTREAM_SYNC ]]; then
  port_die "upstream-pin: UPSTREAM_SYNC does not exist"
fi
pin=$(port_pin_parse < UPSTREAM_SYNC)
if [[ -z "${pin}" ]]; then
  port_die "upstream-pin: UPSTREAM_SYNC must hold exactly one non-comment line, a full 40-character commit"
fi
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  port_output sha "${pin}"
fi
printf '%s\n' "${pin}"
