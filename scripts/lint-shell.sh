#!/usr/bin/env bash
# lint-shell.sh: run shellcheck at its strictest on every shell script the fork owns: every
# optional check enabled, style severity, sourced files followed, and no rc file (so nothing can
# turn a check off).
#
# Usage: rust/scripts/lint-shell.sh   (from anywhere in the repository)
#
# Scripts: *.sh and *.bash under rust/ (except rust/legacy/), .github/ and parity/. The upstream
# Go server's scripts are never linted. Every script must also be committed executable, because
# the workflows run them directly.
set -euo pipefail
shopt -s inherit_errexit

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(git -C "${here}" rev-parse --show-toplevel)
cd "${root}"

if ! command -v shellcheck > /dev/null; then
  printf 'lint-shell: shellcheck is required but not installed\n' >&2
  exit 2
fi

listing=$(git ls-files --cached --others --exclude-standard -- \
  'rust/*.sh' 'rust/*.bash' '.github/*.sh' '.github/*.bash' 'parity/*.sh' 'parity/*.bash')
scripts=()
while IFS= read -r path; do
  case "${path}" in
    "" | rust/legacy/*) ;;
    *) if [[ -f "${path}" ]]; then scripts+=("${path}"); fi ;;
  esac
done <<< "${listing}"

if [[ "${#scripts[@]}" -eq 0 ]]; then
  printf 'lint-shell: no shell scripts found\n' >&2
  exit 1
fi

failed=0
for path in "${scripts[@]}"; do
  mode=$(git ls-files --stage -- "${path}")
  mode=${mode%% *}
  if [[ -n "${mode}" && "${mode}" != 100755 ]]; then
    printf '%s: must be committed executable (git mode %s; run chmod +x and git add)\n' "${path}" "${mode}"
    failed=1
  elif [[ -z "${mode}" && ! -x "${path}" ]]; then
    printf '%s: must be executable (chmod +x)\n' "${path}"
    failed=1
  fi
done

shellcheck_version=$(shellcheck --version)
shellcheck_version=$(sed -n 's/^version: //p' <<< "${shellcheck_version}")
printf 'lint-shell: %s scripts, shellcheck %s\n' "${#scripts[@]}" "${shellcheck_version}"
if ! shellcheck --norc --enable=all --severity=style --external-sources --source-path=SCRIPTDIR \
  "${scripts[@]}"; then
  failed=1
fi
exit "${failed}"
