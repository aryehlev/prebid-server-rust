#!/usr/bin/env bash
# replayharness-replay.sh sends ReplayHarness requests to the Go server and
# checks what it answers.
#
# It starts the Go server the way the parity runner does, with
# parity/replayharness/rhtool serve: parity mode on 127.0.0.1, a parity base
# profile, the parity stored data plus parity/replayharness/stored, ports
# from the free-port helper, and the mock oracle answering every outbound
# call on loopback. The proxy variables also point at the oracle, so a client
# the parity transport does not wrap reaches the oracle, not the network.
# While the servers run, a watchdog checks their sockets with lsof every
# second and kills them at once if one is not on loopback.
#
# Then it runs, in order:
#   1. the built-in probes on every endpoint of parity/replayharness/api.yaml
#      (informational: Go is the reference, so a finding is not a failure);
#   2. the replays in parity/replayharness/replays, which must all pass;
#   3. with --baseline, the probes against two Go servers with
#      `replayharness run --baseline`; a divergence fails, except a shape
#      divergence on /cookie_sync, which Go answers nondeterministically
#      (see below);
#   4. with --record, the probes and the replays again through
#      `replayharness record`, then the recording replayed with --mutate
#      through the same proxy, and the result compacted into
#      parity/replayharness/traffic.jsonl by `rhtool traffic`: repeated
#      requests, request bodies over 64 KiB (size probes, which the replays
#      and probes send again) and repeats of a response body over 64 KiB on
#      the same endpoint are dropped; a secret fails it, and so does a result
#      over 5 MB.
# With --serve it only starts the servers, prints their URLs and waits for
# Ctrl-C.
#
# Usage: parity/scripts/replayharness-replay.sh [--baseline] [--record] [--serve]
#            [--profile NAME] [--out DIR]
#   REPLAYHARNESS  the ReplayHarness CLI (default: replayharness on PATH)
#   --profile      the parity base profile (default: default)
#   --out          where to keep the reports (default: a new temporary directory)
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$script_dir/../.." && pwd)
cli=${REPLAYHARNESS:-replayharness}
map="$repo/parity/replayharness/api.yaml"
replays="$repo/parity/replayharness/replays"
traffic="$repo/parity/replayharness/traffic.jsonl"

baseline=0
record=0
serve_only=0
profile=default
out=""
while (($# > 0)); do
  case "$1" in
    --baseline) baseline=1 ;;
    --record) record=1 ;;
    --serve) serve_only=1 ;;
    --profile)
      profile=${2:?--profile needs a name}
      shift
      ;;
    --out)
      out=${2:?--out needs a directory}
      shift
      ;;
    *)
      echo "usage: $0 [--baseline] [--record] [--serve] [--profile NAME] [--out DIR]" >&2
      exit 2
      ;;
  esac
  shift
done

say() { echo "replayharness-replay: $*" >&2; }

state=$(mktemp -d "${TMPDIR:-/tmp}/replayharness-replay.XXXXXX")
if [[ -z "$out" ]]; then
  out="$state/out"
fi
mkdir -p "$out"

serve_pid=""
proxy_pid=""
watch_pid=""

cleanup() {
  local status=$?
  if [[ -n "$watch_pid" ]]; then
    kill "$watch_pid" 2>/dev/null || true
    wait "$watch_pid" 2>/dev/null || true
  fi
  if [[ -n "$proxy_pid" ]]; then
    kill -INT "$proxy_pid" 2>/dev/null || true
    wait "$proxy_pid" 2>/dev/null || true
  fi
  if [[ -n "$serve_pid" ]]; then
    kill -INT "$serve_pid" 2>/dev/null || true
    wait "$serve_pid" 2>/dev/null || true
  fi
  # Keep the logs and reports; drop the binaries.
  rm -f "$state/rhtool" "$state/stack/pbs-go"
  say "logs and reports in $state"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# Every process this script starts gets the proxy guard; requests to
# 127.0.0.1 never use a proxy.
export HTTP_PROXY=http://127.0.0.1:9 HTTPS_PROXY=http://127.0.0.1:9 NO_PROXY=127.0.0.1,localhost
export http_proxy=$HTTP_PROXY https_proxy=$HTTPS_PROXY no_proxy=$NO_PROXY

say "building rhtool"
(cd "$repo" && go build -o "$state/rhtool" ./parity/replayharness/rhtool)

servers=1
if ((baseline)); then
  servers=2
fi
say "starting $servers Go server(s) with profile $profile (the first start builds the server)"
"$state/rhtool" serve -repo "$repo" -dir "$state/stack" -profile "$profile" -servers "$servers" -spare-ports 1 \
  >"$state/serve.log" 2>&1 &
serve_pid=$!

for _ in $(seq 1 600); do
  [[ -f "$state/stack/state.env" ]] && break
  if ! kill -0 "$serve_pid" 2>/dev/null; then
    say "rhtool serve exited:"
    cat "$state/serve.log" >&2
    exit 1
  fi
  sleep 0.5
done
if [[ ! -f "$state/stack/state.env" ]]; then
  say "the servers did not start within 5 minutes"
  exit 1
fi

value() { sed -n "s/^$1=//p" "$state/stack/state.env"; }
target=$(value RH_TARGET)
baseline_url=$(value RH_BASELINE)
token_file=$(value RH_TOKEN_FILE)
read -r -a main_ports <<<"$(value RH_MAIN_PORTS)"
read -r -a spare_ports <<<"$(value RH_SPARE_PORTS)"

pids_file="$state/pids"
echo "$serve_pid" >"$pids_file"
for port in "${main_ports[@]}"; do
  pid=$(lsof -nP -iTCP:"$port" -sTCP:LISTEN -t | head -n 1 || true)
  if [[ -z "$pid" ]]; then
    say "no process listens on 127.0.0.1:$port"
    exit 1
  fi
  echo "$pid" >>"$pids_file"
done

# off_loopback prints the sockets of pid whose local or remote address is
# not 127.0.0.0/8 or ::1, one per line. lsof -F n prints one n<name> line
# per socket: <host>:<port>, optionally followed by -><host>:<port>.
off_loopback() {
  lsof -nP -a -p "$1" -i -F n 2>/dev/null | sed -n 's/^n//p' | while IFS= read -r name; do
    read -r -a ends <<<"${name//->/ }"
    for end in "${ends[@]}"; do
      case "$end" in
        127.*:* | '[::1]':*) ;;
        *) echo "$name" ;;
      esac
    done
  done
}

watchdog() {
  while sleep 1; do
    while IFS= read -r pid; do
      kill -0 "$pid" 2>/dev/null || continue
      bad=$(off_loopback "$pid" | sort -u)
      if [[ -n "$bad" ]]; then
        say "process $pid has sockets off loopback; killing every server:"
        echo "$bad" >&2
        echo "$pid $bad" >>"$state/breach"
        while IFS= read -r victim; do
          kill -KILL "$victim" 2>/dev/null || true
        done <"$pids_file"
        return 1
      fi
    done <"$pids_file"
  done
}
watchdog &
watch_pid=$!

check_breach() {
  if [[ -f "$state/breach" ]]; then
    say "a server opened a socket off loopback; stopped"
    exit 1
  fi
}

say "Go server on $target${baseline_url:+, baseline Go server on $baseline_url}"
if ((serve_only)); then
  say "serving until Ctrl-C; the token is in $token_file"
  wait "$serve_pid"
  exit 0
fi

status=0

say "probes on $target"
"$cli" run "$map" --target "$target" --fail-on never --no-color --json "$out/probes.json" >"$out/probes.txt"
tail -n 2 "$out/probes.txt" >&2
check_breach

say "replays on $target"
if "$cli" replay "$replays" --target "$target" --no-color --junit "$out/replays.junit.xml" >"$out/replays.txt"; then
  tail -n 1 "$out/replays.txt" >&2
else
  grep -v '^✓' "$out/replays.txt" >&2 || true
  status=1
fi
check_breach

# Cookie sync shuffles the syncers it evaluates (usersync/chooser.go,
# usersync/shuffler.go), so with a limit, or with no bidder list, two Go
# servers pick different bidders and the debug list comes and goes. That
# changes the shape of the body, never the status: its shape divergences
# (severity warning) are printed but do not fail the run; any other
# divergence does, a status divergence (severity error) there included.
nondeterministic='^POST /cookie_sync$'

# divergences prints the divergence findings of the baseline report, each
# with its endpoint, that are (known) or are not (unexpected) shape
# divergences on an endpoint matching $nondeterministic.
divergences() {
  awk -v skip="$nondeterministic" -v want="$1" '
    /^[A-Z]+ \// { section = $0; next }
    /^ +[^ ]+ (error|warning) +divergence / {
      known = (section ~ skip && $2 == "warning") ? "known" : "unexpected"
      if (known == want) { print section ":" $0; getline; print }
    }' "$out/baseline.txt"
}

if ((baseline)); then
  say "probes on $target against the baseline $baseline_url"
  "$cli" run "$map" --target "$target" --baseline "$baseline_url" --fail-on never --no-color \
    --json "$out/baseline.json" >"$out/baseline.txt"
  known=$(divergences known)
  unexpected=$(divergences unexpected)
  if [[ -n "$known" ]]; then
    say "divergence on an endpoint Go answers nondeterministically (not a failure):"
    echo "$known" >&2
  fi
  if [[ -n "$unexpected" ]]; then
    say "divergence between the two Go servers:"
    echo "$unexpected" >&2
    status=1
  else
    say "no unexpected divergence between the two Go servers"
  fi
  check_breach
fi

if ((record)); then
  proxy="http://127.0.0.1:${spare_ports[0]}"
  raw="$state/traffic.raw.jsonl"
  say "recording through $proxy"
  "$cli" record --upstream "$target" --listen "127.0.0.1:${spare_ports[0]}" -o "$raw" -q 2>"$state/record.log" &
  proxy_pid=$!
  echo "$proxy_pid" >>"$pids_file"
  for _ in $(seq 1 100); do
    grep -q '^Recording ' "$state/record.log" 2>/dev/null && break
    kill -0 "$proxy_pid" 2>/dev/null || break
    sleep 0.1
  done
  if ! grep -q '^Recording ' "$state/record.log"; then
    say "the recording proxy did not start:"
    cat "$state/record.log" >&2
    exit 1
  fi
  "$cli" run "$map" --target "$proxy" --fail-on never --no-color >"$out/record-probes.txt"
  "$cli" replay "$replays" --target "$proxy" --fail-on never --no-color >"$out/record-replays.txt"
  say "replaying the recording with --mutate through $proxy"
  "$cli" replay "$raw" --target "$proxy" --mutate --fail-on never --no-color >"$out/record-mutate.txt"
  kill -INT "$proxy_pid"
  wait "$proxy_pid" || true
  proxy_pid=""
  check_breach
  "$state/rhtool" traffic -token-file "$token_file" -o "$traffic" "$raw" >&2
fi

check_breach
if ((status != 0)); then
  exit "$status"
fi
