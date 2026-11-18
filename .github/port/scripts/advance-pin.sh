#!/usr/bin/env bash
# advance-pin.sh SHA VERSION: pin the work tree to upstream commit SHA. UPSTREAM_SYNC gets SHA as
# its one non-comment line (its # comment lines are kept), and every Go tooling module under
# crates/ that requires the upstream module gets VERSION, SHA's pseudo-version (pin-version.sh).
# Nothing else changes: the replace directive keeps pointing the module at .upstream, and go.sum
# has no line for a module replaced by a directory. Running it again changes nothing.
#
# The port job runs it before the agent sessions (so the gates and the agents work against the new
# pin), check-worktree.sh after each session, and commit.sh before committing.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

sha=${1:-}
version=${2:-}
if [[ "$#" -ne 2 || ! "${sha}" =~ ^[0-9a-f]{40}$ ]]; then
  printf 'usage: advance-pin.sh SHA VERSION\n' >&2
  exit 2
fi
if [[ ! "${version}" =~ ^v4\.[0-9]+\.[0-9]+-[0-9A-Za-z.-]*[0-9]{14}-${sha:0:12}(\+[0-9A-Za-z.-]+)?$ ]]; then
  port_die "advance-pin: '${version}' is not a pseudo-version of ${sha:0:12}"
fi
port_cd_top
if [[ ! -f UPSTREAM_SYNC ]]; then
  port_die "advance-pin: UPSTREAM_SYNC does not exist"
fi

current=$(cat UPSTREAM_SYNC)
port_sync_text "${sha}" <<< "${current}" > UPSTREAM_SYNC.tmp
mv UPSTREAM_SYNC.tmp UPSTREAM_SYNC

module=$(port_go_module)
modules=$(find crates \( -name target -o -name '.*' \) -prune -o -type f -name go.mod -print 2> /dev/null | sort)
changed=0
while IFS= read -r gomod; do
  if [[ -z "${gomod}" ]]; then
    continue
  fi
  PIN_MODULE=${module} PIN_VERSION=${version} perl -i -ne '
    BEGIN { $module = quotemeta($ENV{PIN_MODULE}); }
    s{\A(\s*(?:require\s+)?$module\s+)v\S+}{$1$ENV{PIN_VERSION}} unless /=>/;
    print;
  ' "${gomod}"
  if ! git diff --quiet -- "${gomod}" 2> /dev/null; then
    changed=$((changed + 1))
  fi
done <<< "${modules}"
printf 'advance-pin: UPSTREAM_SYNC pins %s; %s tooling go.mod file(s) differ from HEAD (%s)\n' \
  "${sha:0:12}" "${changed}" "${version}"
