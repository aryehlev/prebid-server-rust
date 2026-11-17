#!/usr/bin/env bash
# replayharness-context.sh checks parity/replayharness/api.yaml, the
# ReplayHarness API map of the Go server, against the Go source.
#
# ReplayHarness reads its map from <root>/.replayharness/api.yaml and scans
# <root> for route registrations. This script builds such a root in a
# temporary directory outside the repository:
#   - every non-test Go file that could register a route, except rust/ (the
#     port, not the reference) and parity/ (the harness, whose /__parity/*
#     routes are not product routes);
#   - full-line // comments blanked, line numbers kept, so documentation
#     examples such as the one in stored_requests/events/api/api.go are not
#     taken for routes;
#   - the map itself.
# It then runs `replayharness context` on that root. The command fails when
# the report has a "Spec drift" section, that is when the code registers a
# route the map misses or the map lists a route with no handler. Last, it
# checks that every file:line the map cites exists in the repository.
#
# Usage: parity/scripts/replayharness-context.sh [report-file]
#   REPLAYHARNESS  the ReplayHarness CLI (default: replayharness on PATH)
#   report-file    where to keep the full context report (default: not kept)
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$script_dir/../.." && pwd)
map="$repo/parity/replayharness/api.yaml"
cli=${REPLAYHARNESS:-replayharness}
report_out=${1:-}

if [[ ! -f "$map" ]]; then
  echo "replayharness-context: $map is missing" >&2
  exit 1
fi

root=$(mktemp -d "${TMPDIR:-/tmp}/replayharness-context.XXXXXX")
trap 'rm -rf "$root"' EXIT

mkdir -p "$root/.replayharness"
cp "$map" "$root/.replayharness/api.yaml"

# The route patterns ReplayHarness looks for in Go all start with one of
# these calls, so files without one cannot register a route.
route_calls='\.(HandleFunc|Handle|GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS|Any|Get|Post|Put|Patch|Delete|Head|Options)\('

scanned=0
while IFS= read -r -d '' file; do
  case "$file" in
    rust/* | parity/* | *_test.go) continue ;;
  esac
  [[ -f "$repo/$file" ]] || continue
  mkdir -p "$root/$(dirname "$file")"
  sed -E 's#^[[:space:]]*//.*$##' "$repo/$file" >"$root/$file"
  scanned=$((scanned + 1))
done < <(git -C "$repo" grep -lz --untracked -E "$route_calls" -- '*.go')

if ((scanned == 0)); then
  echo "replayharness-context: no Go files to scan" >&2
  exit 1
fi

report="$root/context.md"
"$cli" context "$root" >"$report"
if [[ -n "$report_out" ]]; then
  cp "$report" "$report_out"
fi

# The report starts with "# <name>: <n> endpoint(s)".
endpoints=$(sed -n '1s/.*: \([0-9][0-9]*\) endpoint(s)$/\1/p' "$report")
echo "replayharness-context: scanned $scanned Go files; the report lists ${endpoints:-no} endpoints"

status=0
if grep -q '^## Spec drift' "$report"; then
  echo "replayharness-context: the map and the code disagree:" >&2
  # The section ends at the first blank line.
  awk '/^## Spec drift/ { found = 1 } found && /^$/ { exit } found { print }' "$report" >&2
  status=1
else
  echo "replayharness-context: no spec drift"
fi

# Every citation must name a repository file and lines inside it. A citation
# is path:N or path:N-M, optionally followed by more ", N" or ", N-M" ranges
# of the same file.
cited=0
bad=0
while IFS= read -r ref; do
  cited=$((cited + 1))
  file=${ref%%:*}
  if [[ ! -f "$repo/$file" ]]; then
    echo "replayharness-context: $ref cites a missing file" >&2
    bad=$((bad + 1))
    continue
  fi
  count=$(awk 'END { print NR }' "$repo/$file")
  IFS=',' read -r -a ranges <<<"${ref#*:}"
  for range in "${ranges[@]}"; do
    range=${range# }
    first=${range%-*}
    last=${range#*-}
    if ((first < 1 || first > last || last > count)); then
      echo "replayharness-context: $ref: lines $range are not inside $file ($count lines)" >&2
      bad=$((bad + 1))
    fi
  done
done < <(grep -oE '[A-Za-z0-9_./-]+\.(go|yaml|json|html|md):[0-9]+(-[0-9]+)?(, [0-9]+(-[0-9]+)?)*' "$map" | sort -u)

if ((bad > 0)); then
  status=1
else
  echo "replayharness-context: all $cited cited locations exist"
fi
exit "$status"
