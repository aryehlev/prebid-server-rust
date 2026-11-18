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
#      `replayharness run --baseline`; any divergence fails: in parity mode
#      Go answers every probe alike, /cookie_sync included (see below);
#   4. with --record, the probes and the replays again through
#      `replayharness record`, then the recording replayed with --mutate
#      through the same proxy, and the result compacted into
#      parity/replayharness/traffic.jsonl by `rhtool traffic`: repeated
#      requests, request bodies over 64 KiB (size probes, which the replays
#      and probes send again) and repeats of a response body over 64 KiB on
#      the same endpoint are dropped; a secret fails it, and so does a result
#      over 5 MB.
# With --serve it only starts the servers, prints their URLs and waits for
# Ctrl-C. With --case-profile NAME the server also takes the case profile
# parity/replayharness/profiles/NAME/profile.json over the base profile
# (rhtool serve -case-profile), and the script runs only the replays of
# parity/replayharness/profiles/NAME/replays: the probes run under no case
# profile, and a case profile's replays are not recorded (the corpus builds
# their cases from the steps), so --baseline and --record do not go with it.
#
# Usage: parity/scripts/replayharness-replay.sh [--baseline] [--record] [--serve]
#            [--profile NAME] [--case-profile NAME] [--out DIR] [--keep-state]
#   REPLAYHARNESS  the ReplayHarness CLI (default: replayharness on PATH)
#   PBS_GO_ROOT    the pinned upstream checkout (default .upstream, which
#                  scripts/fetch-upstream.sh checks out)
#   --profile      the parity base profile (default: default)
#   --case-profile a case profile of parity/replayharness/profiles (default: none)
#   --out          where to keep the reports (default: a new temporary directory)
#   --keep-state   keep the state directory after the run, for debugging
#
# The state directory, a new temporary directory, holds the parity token
# file, rhtool serve's state.env, outbound log and server logs, the raw
# recording and the builds: the tree parity/scripts/build-go.sh assembles
# from the pinned upstream checkout, the seams and parity/, and the Go
# server and rhtool built from it. It is deleted on every exit, unless
# --keep-state is given, which deletes only the builds; the script never
# prints the token. The probes, the replays and the case profiles are read
# from this checkout's parity/replayharness, and --record writes its
# traffic.jsonl there.
set -euo pipefail
shopt -s inherit_errexit

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# ws is this checkout, the workspace; the Go code runs from the tree built
# into the state directory.
ws=$(cd "${script_dir}/../.." && pwd -P)
cli=${REPLAYHARNESS:-replayharness}
map="${ws}/parity/replayharness/api.yaml"
replays="${ws}/parity/replayharness/replays"
traffic="${ws}/parity/replayharness/traffic.jsonl"

baseline=0
record=0
serve_only=0
profile=default
case_profile=""
out=""
keep_state=0
while (($# > 0)); do
  case "$1" in
    --keep-state) keep_state=1 ;;
    --baseline) baseline=1 ;;
    --record) record=1 ;;
    --serve) serve_only=1 ;;
    --profile)
      profile=${2:?--profile needs a name}
      shift
      ;;
    --case-profile)
      case_profile=${2:?--case-profile needs a name}
      shift
      ;;
    --out)
      out=${2:?--out needs a directory}
      shift
      ;;
    *)
      echo "usage: $0 [--baseline] [--record] [--serve] [--profile NAME] [--case-profile NAME] [--out DIR]" \
        "[--keep-state]" >&2
      exit 2
      ;;
  esac
  shift
done

serve_args=()
if [[ -n "${case_profile}" ]]; then
  if ((baseline || record)); then
    echo "$0: --baseline and --record do not go with --case-profile" >&2
    exit 2
  fi
  case_dir="${ws}/parity/replayharness/profiles/${case_profile}"
  if [[ ! -f "${case_dir}/profile.json" ]]; then
    echo "$0: no case profile ${case_dir}/profile.json" >&2
    exit 2
  fi
  replays="${case_dir}/replays"
  serve_args=(-case-profile "${case_dir}/profile.json")
fi

say() { echo "replayharness-replay: $*" >&2; }

# failures prints the replay report $1 without its passing replays. A failed
# step prints the response body, which can be binary (/setuid answers a PNG),
# so grep reads the report as text, and the control bytes other than tab and
# newline are dropped before they reach the terminal.
failures() {
  LC_ALL=C grep -a -v '^✓' "$1" | LC_ALL=C tr -d '\000-\010\013-\037\177' >&2 || true
}

state=""
serve_pid=""
proxy_pid=""
watch_pid=""

# cleanup stops the servers and deletes the state directory, or with
# --keep-state only the builds in it. It runs on every exit.
cleanup() {
  local status=$?
  set +e
  if [[ -n "${watch_pid}" ]]; then
    kill "${watch_pid}" 2>/dev/null || true
    wait "${watch_pid}" 2>/dev/null || true
  fi
  if [[ -n "${proxy_pid}" ]]; then
    kill -INT "${proxy_pid}" 2>/dev/null || true
    wait "${proxy_pid}" 2>/dev/null || true
  fi
  if [[ -n "${serve_pid}" ]]; then
    kill -INT "${serve_pid}" 2>/dev/null || true
    wait "${serve_pid}" 2>/dev/null || true
  fi
  if [[ -n "${state}" ]]; then
    if ((keep_state)); then
      rm -rf "${state:?}/bin" "${state:?}/tree"
      say "state kept in ${state}; it holds the parity token file, so delete it when done"
    else
      rm -rf "${state:?}"
    fi
  fi
  if [[ -n "${out}" ]]; then
    say "reports in ${out}"
  fi
  exit "${status}"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
trap 'exit 129' HUP

state=$(mktemp -d "${TMPDIR:-/tmp}/replayharness-replay.XXXXXX")
if [[ -z "${out}" ]]; then
  out=$(mktemp -d "${TMPDIR:-/tmp}/replayharness-replay-out.XXXXXX")
fi
mkdir -p "${out}"

# Every process this script starts gets the proxy guard; requests to
# 127.0.0.1 never use a proxy.
export HTTP_PROXY=http://127.0.0.1:9 HTTPS_PROXY=http://127.0.0.1:9 NO_PROXY=127.0.0.1,localhost
export http_proxy=${HTTP_PROXY} https_proxy=${HTTPS_PROXY} no_proxy=${NO_PROXY}

say "assembling the tree and building the Go server and rhtool"
"${script_dir}/build-go.sh" --tree "${state}/tree" --bin "${state}/bin" >/dev/null
tree="${state}/tree"

servers=1
if ((baseline)); then
  servers=2
fi
say "starting ${servers} Go server(s) with profile ${profile}${case_profile:+ and case profile ${case_profile}}"
"${state}/bin/rhtool" serve -repo "${tree}" -dir "${state}/stack" -bin "${state}/bin/pbs-go" -profile "${profile}" \
  -servers "${servers}" -spare-ports 1 ${serve_args[@]+"${serve_args[@]}"} >"${state}/serve.log" 2>&1 &
serve_pid=$!

for _ in $(seq 1 600); do
  [[ -f "${state}/stack/state.env" ]] && break
  if ! kill -0 "${serve_pid}" 2>/dev/null; then
    say "rhtool serve exited:"
    cat "${state}/serve.log" >&2
    exit 1
  fi
  sleep 0.5
done
if [[ ! -f "${state}/stack/state.env" ]]; then
  say "the servers did not start within 5 minutes"
  exit 1
fi

value() { sed -n "s/^$1=//p" "${state}/stack/state.env"; }
target=$(value RH_TARGET)
baseline_url=$(value RH_BASELINE)
token_file=$(value RH_TOKEN_FILE)
main_ports_value=$(value RH_MAIN_PORTS)
spare_ports_value=$(value RH_SPARE_PORTS)
read -r -a main_ports <<<"${main_ports_value}"
read -r -a spare_ports <<<"${spare_ports_value}"

pids_file="${state}/pids"
echo "${serve_pid}" >"${pids_file}"
for port in "${main_ports[@]}"; do
  pid=$(lsof -nP -iTCP:"${port}" -sTCP:LISTEN -t | head -n 1 || true)
  if [[ -z "${pid}" ]]; then
    say "no process listens on 127.0.0.1:${port}"
    exit 1
  fi
  echo "${pid}" >>"${pids_file}"
done

# off_loopback prints the sockets of pid whose local or remote address is
# not 127.0.0.0/8 or ::1, one per line. lsof -F n prints one n<name> line
# per socket: <host>:<port>, optionally followed by -><host>:<port>. lsof
# exits 1 for a process with no socket, which must not stop the watchdog.
off_loopback() {
  { lsof -nP -a -p "$1" -i -F n 2>/dev/null || true; } | sed -n 's/^n//p' | while IFS= read -r name; do
    read -r -a ends <<<"${name//->/ }"
    for end in "${ends[@]}"; do
      case "${end}" in
        127.*:* | '[::1]':*) ;;
        *) echo "${name}" ;;
      esac
    done
  done
}

watchdog() {
  while sleep 1; do
    while IFS= read -r pid; do
      kill -0 "${pid}" 2>/dev/null || continue
      bad=$(off_loopback "${pid}" | sort -u)
      if [[ -n "${bad}" ]]; then
        say "process ${pid} has sockets off loopback; killing every server:"
        echo "${bad}" >&2
        echo "${pid} ${bad}" >>"${state}/breach"
        while IFS= read -r victim; do
          kill -KILL "${victim}" 2>/dev/null || true
        done <"${pids_file}"
        return 1
      fi
    done <"${pids_file}"
  done
}
watchdog &
watch_pid=$!

check_breach() {
  if [[ -f "${state}/breach" ]]; then
    say "a server opened a socket off loopback; stopped"
    exit 1
  fi
}

say "Go server on ${target}${baseline_url:+, baseline Go server on ${baseline_url}}"
if ((serve_only)); then
  say "serving until Ctrl-C; the token is in ${token_file}, which goes when the script exits"
  wait "${serve_pid}"
  exit 0
fi

status=0

if [[ -z "${case_profile}" ]]; then
  say "probes on ${target}"
  "${cli}" run "${map}" --target "${target}" --fail-on never --no-color --json "${out}/probes.json" >"${out}/probes.txt"
  tail -n 2 "${out}/probes.txt" >&2
  check_breach
fi

say "replays on ${target}"
if "${cli}" replay "${replays}" --target "${target}" --no-color --junit "${out}/replays.junit.xml" >"${out}/replays.txt"; then
  tail -n 1 "${out}/replays.txt" >&2
else
  failures "${out}/replays.txt"
  status=1
fi
check_breach

# Cookie sync evaluates the syncers in the order the chooser lists them and
# then shuffles them (usersync/chooser.go, usersync/shuffler.go). A Go server
# in parity mode lists them in bidder order and keeps that order on its main
# port (parity/schema/fakes.json usersync_bidders, decisions.json
# usersync.shuffle outside_case), so two of them answer /cookie_sync alike
# and its divergences, of the status or of the shape of the body, fail the
# run as any other endpoint's do.

# divergences prints the divergence findings of the baseline report, each
# with its endpoint.
divergences() {
  awk '
    /^[A-Z]+ \// { section = $0; next }
    /^ +[^ ]+ (error|warning) +divergence / { print section ":" $0; getline; print }' "${out}/baseline.txt"
}

if ((baseline)); then
  say "probes on ${target} against the baseline ${baseline_url}"
  "${cli}" run "${map}" --target "${target}" --baseline "${baseline_url}" --fail-on never --no-color \
    --json "${out}/baseline.json" >"${out}/baseline.txt"
  found=$(divergences)
  if [[ -n "${found}" ]]; then
    say "divergence between the two Go servers:"
    echo "${found}" >&2
    status=1
  else
    say "no divergence between the two Go servers"
  fi
  check_breach
fi

if ((record)); then
  proxy="http://127.0.0.1:${spare_ports[0]}"
  raw="${state}/traffic.raw.jsonl"
  say "recording through ${proxy}"
  "${cli}" record --upstream "${target}" --listen "127.0.0.1:${spare_ports[0]}" -o "${raw}" -q 2>"${state}/record.log" &
  proxy_pid=$!
  echo "${proxy_pid}" >>"${pids_file}"
  for _ in $(seq 1 100); do
    grep -q '^Recording ' "${state}/record.log" 2>/dev/null && break
    kill -0 "${proxy_pid}" 2>/dev/null || break
    sleep 0.1
  done
  if ! grep -q '^Recording ' "${state}/record.log"; then
    say "the recording proxy did not start:"
    cat "${state}/record.log" >&2
    exit 1
  fi
  "${cli}" run "${map}" --target "${proxy}" --fail-on never --no-color >"${out}/record-probes.txt"
  "${cli}" replay "${replays}" --target "${proxy}" --fail-on never --no-color >"${out}/record-replays.txt"
  say "replaying the recording with --mutate through ${proxy}"
  "${cli}" replay "${raw}" --target "${proxy}" --mutate --fail-on never --no-color >"${out}/record-mutate.txt"
  kill -INT "${proxy_pid}"
  wait "${proxy_pid}" || true
  proxy_pid=""
  check_breach
  "${state}/rhtool" traffic -token-file "${token_file}" -o "${traffic}" "${raw}" >&2
fi

check_breach
if ((status != 0)); then
  exit "${status}"
fi
