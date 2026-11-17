#!/usr/bin/env bash
# decide-fix.sh: decide whether the fix session runs and write its input, ${CTX}/fix-input.md:
# the blocking findings of the adversarial review and the checks that failed after the port.
# Step output fix=true|false.
# Env: CTX GITHUB_OUTPUT, REVIEW_JSON (structured output of the review session, may be empty),
#      AFTER_STATUS (status of the gates after the port session).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

ctx=${CTX:?}
review=${REVIEW_JSON:-}
after=${AFTER_STATUS:-unknown}
input="${ctx}/fix-input.md"

blocking=0
findings=""
if [[ -n "${review}" ]]; then
  valid=$(jq -r 'if type == "object" then "yes" else "no" end' <<< "${review}" 2> /dev/null) || valid=no
  if [[ "${valid}" == yes ]]; then
    printf '%s\n' "${review}" > "${ctx}/review-result.json"
    blocking=$(jq -r '[.findings[]? | select(.severity == "blocking")] | length' <<< "${review}")
    findings=$(jq -r '.findings[]? | select(.severity == "blocking")
      | "### \(.title)\n\n- Go: \(.go_ref)\n- Rust: \(.rust_ref)\n- Input: \(.input)\n- Go behaviour: \(.go_behavior)\n- Rust behaviour: \(.rust_behavior)\n- Evidence: \(.evidence // "")\n"' \
      <<< "${review}")
  else
    port_warning "decide-fix: the review result is not a JSON object; ignoring it"
  fi
fi

failed_gates=""
if [[ -f "${ctx}/gates-after.json" ]]; then
  failed_gates=$(jq -r '.checks[] | select(.status != "pass")
    | "### Gate `\(.name)` failed\n\nCommand: `\(.command)`\n\n```text\n\(.log_tail)\n```\n"' "${ctx}/gates-after.json")
fi

fix=false
if [[ "${blocking}" -gt 0 || "${after}" != pass ]]; then
  fix=true
fi
{
  printf '# Problems to fix\n\n'
  printf 'Gates after the port session: %s\n\n' "${after}"
  if [[ -n "${findings}" ]]; then
    printf '## Blocking adversarial review findings (%s)\n\n%s\n' "${blocking}" "${findings}"
  fi
  if [[ -n "${failed_gates}" ]]; then
    printf '## Failing gates\n\n%s\n' "${failed_gates}"
  fi
} > "${input}"
port_output fix "${fix}"
printf 'decide-fix: fix=%s (blocking findings %s, gates after %s)\n' "${fix}" "${blocking}" "${after}"
