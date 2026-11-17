#!/usr/bin/env bash
# replayharness.sh compares two Prebid Server implementations on the requests
# ReplayHarness sends. Side A is the Go server, the reference. Side B is the
# Go server too, for an A/A run, or the build --bin-b names, such as the Rust
# server with --impl-b rust.
#
# Every server runs the way the parity runner runs it: parity mode on
# 127.0.0.1, a parity base profile, ports from the free-port helper, and the
# mock oracle answering every outbound call (bidders, Prebid Cache, the GVL
# vendor lists, currency rates, stored data) on loopback. The proxy variables
# point at 127.0.0.1:9 (rhtool serve and the runner point a server's at the
# oracle), and a watchdog checks the sockets of every process this script
# starts, and of their children, with lsof every second; it kills them all at
# once if one is not on loopback.
#
# Commands:
#   baseline      boot sides A and B under one profile (rhtool serve -servers 2)
#                 and run, in order:
#                   1. the probes of parity/replayharness/api.yaml on A alone;
#                   2. the probes on B against A: replayharness run --target B
#                      --baseline A --fail-on warning --junit --json;
#                   3. the replays of parity/replayharness/replays on B.
#                 It fails when B diverges from A on a compared probe, or draws
#                 a finding A does not (rhtool gate), or a replay fails.
#   parity        run the parity runner on the replayharness corpus
#                 (parity run --corpus replayharness): every recorded request
#                 and replay step, compared byte for byte through
#                 /__parity/exec; it also lists what the corpus leaves out.
#   selftest      A/A: parity selftest --corpus replayharness, then baseline
#                 with Go on both sides. Both must find no difference.
#   canary        build a Go side B with two seeded bugs (the X-Prebid header
#                 of /openrtb2/auction dropped, the status member of
#                 /info/bidders/{name} renamed) and check that parity and
#                 baseline both catch them.
#   record-saved  record Go's answers to the replay steps that save a value
#                 into parity/replayharness/saved.jsonl (rhtool saved), which
#                 the corpus loader builds the steps that use the value from.
#   cli           build the pinned ReplayHarness CLI and print its path.
#
# Usage: parity/scripts/replayharness.sh COMMAND [--bin-b FILE] [--impl-b go|rust]
#            [--profile NAME] [--out DIR] [-- PARITY-RUN-FLAGS...]
#   --bin-b    side B's server binary (default: the Go server)
#   --impl-b   what side B runs: go or rust (default: go)
#   --profile  the parity base profile (default: default)
#   --out      where to keep the reports (default: a new temporary directory)
#   -- ...     more flags for parity run (the parity command)
#
# Environment:
#   REPLAYHARNESS      a ReplayHarness CLI to use instead of building the pin;
#                      the script warns when its build info does not name the
#                      pinned commit, unmodified
#   REPLAYHARNESS_SRC  the repository to build the pin from (default
#                      https://github.com/aryehlev/ReplayHarness.git)
#
# The CLI is built from commit $rh_commit, the one parity/replayharness was
# recorded against (parity/replayharness/rhreplay.PinnedCommit), in a new
# temporary directory, and the build must record that commit, unmodified, in
# its Go build info. Builds happen before the proxy guard is set, so they may
# fetch modules; nothing after them may leave loopback.
set -euo pipefail

rh_commit=f61ab1997f27b0044eeb801caea62810d75e2d76
rh_src=${REPLAYHARNESS_SRC:-https://github.com/aryehlev/ReplayHarness.git}

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$script_dir/../.." && pwd)
map="$repo/parity/replayharness/api.yaml"
replays="$repo/parity/replayharness/replays"
saved="$repo/parity/replayharness/saved.jsonl"

usage() {
  echo "usage: $0 baseline|parity|selftest|canary|record-saved|cli [--bin-b FILE] [--impl-b go|rust]" \
    "[--profile NAME] [--out DIR] [-- PARITY-RUN-FLAGS...]" >&2
  exit 2
}

(($# > 0)) || usage
command=$1
shift
case "$command" in
  baseline | parity | selftest | canary | record-saved | cli) ;;
  *) usage ;;
esac

bin_b=""
impl_b=go
profile=default
out=""
parity_flags=()
while (($# > 0)); do
  case "$1" in
    --bin-b)
      bin_b=${2:?--bin-b needs a file}
      shift
      ;;
    --impl-b)
      impl_b=${2:?--impl-b needs go or rust}
      shift
      ;;
    --profile)
      profile=${2:?--profile needs a name}
      shift
      ;;
    --out)
      out=${2:?--out needs a directory}
      shift
      ;;
    --)
      shift
      parity_flags=("$@")
      break
      ;;
    *) usage ;;
  esac
  shift
done

case "$impl_b" in
  go | rust) ;;
  *) usage ;;
esac
if [[ "$impl_b" == rust && -z "$bin_b" ]]; then
  echo "$0: --impl-b rust needs --bin-b, for example rust/target/release/pbs-server" >&2
  exit 2
fi
if [[ -n "$bin_b" ]]; then
  if [[ ! -f "$bin_b" || ! -x "$bin_b" ]]; then
    echo "$0: --bin-b $bin_b is not an executable file" >&2
    exit 2
  fi
  bin_b="$(cd "$(dirname "$bin_b")" && pwd)/$(basename "$bin_b")"
fi
if ((${#parity_flags[@]} > 0)) && [[ "$command" != parity ]]; then
  echo "$0: flags after -- are for the parity command" >&2
  exit 2
fi

say() { echo "replayharness: $*" >&2; }
die() {
  say "$*"
  exit 1
}

state=$(mktemp -d "${TMPDIR:-/tmp}/replayharness.XXXXXX")
if [[ -z "$out" ]]; then
  out="$state/out"
fi
mkdir -p "$out"
out=$(cd "$out" && pwd)

# roots lists the processes the script starts; the watchdog checks them and
# every process under them. seen lists each process the watchdog saw, with
# its start time, so the cleanup can kill what is left without killing a
# process that reused a pid.
roots_file="$state/roots"
seen_file="$state/seen"
: >"$roots_file"
: >"$seen_file"

serve_pid=""
proxy_pid=""
runner_pid=""
watch_pid=""
keep_cli=0
cli=""

stop_pid() {
  if [[ -n "$1" ]]; then
    kill "-$2" "$1" 2>/dev/null || true
    wait "$1" 2>/dev/null || true
  fi
}

# descendants prints a pid and the pids of every process under it.
descendants() {
  local child
  echo "$1"
  while IFS= read -r child; do
    descendants "$child"
  done < <(pgrep -P "$1" 2>/dev/null || true)
}

# sweep kills, with the signal given, every process the watchdog saw that
# still runs with the start time it was seen with.
sweep() {
  local pid started now
  while IFS=$'\t' read -r pid started; do
    now=$(ps -o lstart= -p "$pid" 2>/dev/null || true)
    if [[ -n "$now" && "$now" == "$started" ]]; then
      kill "-$1" "$pid" 2>/dev/null || true
    fi
  done <"$seen_file"
}

cleanup() {
  local status=$?
  stop_pid "$watch_pid" TERM
  stop_pid "$proxy_pid" INT
  stop_pid "$runner_pid" INT
  stop_pid "$serve_pid" INT
  sweep KILL
  # Keep the logs and reports; drop the binaries.
  rm -rf "${state:?}/bin" "${state:?}/canary"
  if ((keep_cli == 0)); then
    rm -rf "${state:?}/replayharness"
  else
    rm -rf "${state:?}/replayharness/src"
  fi
  say "logs and reports in $out"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

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

# watched prints every process to check: the roots and what runs under them.
watched() {
  local root
  while IFS= read -r root; do
    descendants "$root"
  done <"$roots_file"
}

kill_watched() {
  local victims pid
  victims=$(watched)
  for pid in $victims; do
    kill -KILL "$pid" 2>/dev/null || true
  done
}

watchdog() {
  local pid bad started
  while sleep 1; do
    for pid in $(watched); do
      kill -0 "$pid" 2>/dev/null || continue
      if ! grep -q "^$pid"$'\t' "$seen_file"; then
        started=$(ps -o lstart= -p "$pid" 2>/dev/null || true)
        printf '%s\t%s\n' "$pid" "$started" >>"$seen_file"
      fi
      bad=$(off_loopback "$pid" | sort -u)
      if [[ -n "$bad" ]]; then
        say "process $pid has sockets off loopback; killing every process the script started:"
        echo "$bad" >&2
        echo "$pid $bad" >>"$state/breach"
        kill_watched
        return 1
      fi
    done
  done
}

check_breach() {
  if [[ -f "$state/breach" ]]; then
    die "a process opened a socket off loopback; stopped"
  fi
}

# guard sets the proxy guard for every process started from here on and
# starts the watchdog. Requests to 127.0.0.1 never use a proxy.
guard() {
  export HTTP_PROXY=http://127.0.0.1:9 HTTPS_PROXY=http://127.0.0.1:9 NO_PROXY=127.0.0.1,localhost
  export http_proxy=$HTTP_PROXY https_proxy=$HTTPS_PROXY no_proxy=$NO_PROXY
  watchdog &
  watch_pid=$!
}

build_tools() {
  say "building the Go server, rhtool and the parity runner"
  mkdir -p "$state/bin"
  (cd "$repo" && go build -o "$state/bin/pbs-go" . &&
    go build -o "$state/bin/rhtool" ./parity/replayharness/rhtool &&
    go build -o "$state/bin/parity" ./parity/cmd/parity)
}

# cli_check prints why the CLI at $1 is not the pinned commit, unmodified,
# going by its Go build info, or nothing when it is.
cli_check() {
  local info
  if ! info=$(go version -m "$1" 2>/dev/null); then
    echo "it has no Go build info"
  elif ! grep -Eq "path[[:space:]]+github.com/aryehlev/ReplayHarness/cmd/replayharness$" <<<"$info"; then
    echo "it is not github.com/aryehlev/ReplayHarness/cmd/replayharness"
  elif ! grep -q "vcs.revision=$rh_commit" <<<"$info"; then
    echo "its build info does not name commit $rh_commit"
  elif ! grep -q "vcs.modified=false" <<<"$info"; then
    echo "it was built from uncommitted changes on top of commit $rh_commit"
  fi
}

# replayharness_cli sets cli to the pinned ReplayHarness CLI, building it
# unless REPLAYHARNESS names one.
replayharness_cli() {
  local dir="$state/replayharness" why
  if [[ -n "${REPLAYHARNESS:-}" ]]; then
    cli=$REPLAYHARNESS
    why=$(cli_check "$cli")
    if [[ -n "$why" ]]; then
      say "warning: REPLAYHARNESS=$cli: $why"
    fi
  else
    say "building ReplayHarness $rh_commit from $rh_src"
    git clone --quiet --no-checkout --no-hardlinks -- "$rh_src" "$dir/src"
    git -C "$dir/src" -c advice.detachedHead=false checkout --quiet --detach "$rh_commit"
    if [[ "$(git -C "$dir/src" rev-parse HEAD)" != "$rh_commit" ]]; then
      die "the ReplayHarness checkout is not at $rh_commit"
    fi
    (cd "$dir/src" && go build -trimpath -o "$dir/replayharness" ./cmd/replayharness)
    why=$(cli_check "$dir/replayharness")
    if [[ -n "$why" ]]; then
      die "the ReplayHarness build does not verify: $why"
    fi
    cli="$dir/replayharness"
  fi
  say "$("$cli" --version)"
}

# start_stack starts rhtool serve with $1 servers into the stack directory
# $2 and waits for its state file.
start_stack() {
  local servers=$1 stack=$2
  local args=(serve -repo "$repo" -dir "$stack" -bin "$state/bin/pbs-go" -profile "$profile"
    -servers "$servers" -spare-ports 1)
  if ((servers == 2)); then
    args+=(-impl-b "$impl_b")
    if [[ -n "$bin_b" ]]; then
      args+=(-bin-b "$bin_b")
    fi
  fi
  "$state/bin/rhtool" "${args[@]}" >"$stack.log" 2>&1 &
  serve_pid=$!
  echo "$serve_pid" >>"$roots_file"
  for _ in $(seq 1 600); do
    [[ -f "$stack/state.env" ]] && break
    if ! kill -0 "$serve_pid" 2>/dev/null; then
      cat "$stack.log" >&2
      die "rhtool serve exited"
    fi
    sleep 0.5
  done
  [[ -f "$stack/state.env" ]] || die "the servers did not start within 5 minutes"
  # Let the watchdog see the servers before they get a request.
  sleep 2
  check_breach
}

stop_stack() {
  stop_pid "$serve_pid" INT
  serve_pid=""
}

value() { sed -n "s/^$1=//p" "$2/state.env"; }

# run_baseline runs the baseline command into $out/$1 and sets
# baseline_status: 0 when B matches A, 1 when it does not, 3 when the run
# cannot show it.
run_baseline() {
  local label=$1
  local dir="$out/$label" stack="$state/stack-$label" a b rh_status=0 replay_status=0 gate_status=0
  mkdir -p "$dir"
  say "$label: starting side A (go) and side B ($impl_b${bin_b:+ $bin_b}) with profile $profile"
  start_stack 2 "$stack"
  a=$(value RH_SIDE_A "$stack")
  b=$(value RH_SIDE_B "$stack")
  say "$label: side A on $a, side B on $b"

  say "$label: probes on side A alone"
  "$cli" run "$map" --target "$a" --fail-on never --no-color --json "$dir/probes-a.json" >"$dir/probes-a.txt"
  check_breach

  say "$label: probes on side B against side A"
  "$cli" run "$map" --target "$b" --baseline "$a" --fail-on warning --no-color \
    --junit "$dir/baseline.junit.xml" --json "$dir/baseline.json" >"$dir/baseline.txt" || rh_status=$?
  check_breach
  case "$rh_status" in
    0) ;;
    1) say "$label: replayharness run exits 1 under --fail-on warning: side B draws errors or warnings;" \
      "the gate below compares them with side A's" ;;
    *) die "$label: replayharness run failed with status $rh_status (see $dir/baseline.txt)" ;;
  esac

  say "$label: replays on side B"
  "$cli" replay "$replays" --target "$b" --no-color --junit "$dir/replays.junit.xml" >"$dir/replays.txt" ||
    replay_status=$?
  check_breach
  if ((replay_status == 0)); then
    tail -n 1 "$dir/replays.txt" >&2
  else
    grep -v '^✓' "$dir/replays.txt" >&2 || true
  fi

  stop_stack
  check_breach

  "$state/bin/rhtool" gate -report "$dir/baseline.json" -reference "$dir/probes-a.json" \
    -json "$dir/gate.json" >"$dir/gate.txt" || gate_status=$?
  sed "s/^/replayharness: $label: /" "$dir/gate.txt" >&2
  case "$gate_status" in
    0 | 1 | 3) ;;
    *) die "$label: rhtool gate failed with status $gate_status" ;;
  esac

  baseline_status=$gate_status
  if ((replay_status != 0)); then
    baseline_status=1
  fi
  say "$label: $(verdict "$baseline_status")"
}

# ratcheted says whether the parity flags after -- name a -baseline to
# ratchet against.
ratcheted() {
  local f
  for f in ${parity_flags[@]+"${parity_flags[@]}"}; do
    case "$f" in
      -baseline | --baseline | -baseline=* | --baseline=*) return 0 ;;
    esac
  done
  return 1
}

# run_parity runs parity $2 (run or selftest) on the replayharness corpus
# into $out/$1 and sets parity_status: 0 no difference, 1 a difference or an
# error, 3 nothing compared. selftest judges the run itself, and so does the
# ratchet when -baseline is given; otherwise parity run exits 0 whenever the
# run completes, so its counts decide.
run_parity() {
  local label=$1 sub=$2
  local dir="$out/$label"
  local args=("$sub" -repo "$repo" -corpus replayharness -bin-a "$state/bin/pbs-go" -profiles "$profile"
    -out "$dir" -run-id "$label")
  if [[ "$sub" == run ]]; then
    args+=(-impl-b "$impl_b")
    if [[ -n "$bin_b" ]]; then
      args+=(-bin-b "$bin_b")
    elif [[ "$impl_b" == go ]]; then
      # Without -bin-b the runner builds a Go side B into parity/out.
      args+=(-bin-b "$state/bin/pbs-go")
    fi
  fi
  say "$label: parity $sub --corpus replayharness (side B: ${impl_b}${bin_b:+ $bin_b})"
  parity_status=0
  "$state/bin/parity" "${args[@]}" ${parity_flags[@]+"${parity_flags[@]}"} >"$dir.log" 2>&1 &
  runner_pid=$!
  echo "$runner_pid" >>"$roots_file"
  wait "$runner_pid" || parity_status=$?
  runner_pid=""
  check_breach
  tail -n 15 "$dir.log" | sed "s/^/replayharness: $label: /" >&2
  if ((parity_status == 0)) && [[ "$sub" == run ]] && ! ratcheted; then
    local mismatch errors passing
    mismatch=$(run_json_count "$dir" mismatch)
    errors=$(run_json_count "$dir" error)
    passing=$(run_json_count "$dir" passing)
    if ((${mismatch:-0} > 0 || ${errors:-0} > 0)); then
      parity_status=1
    elif ((${passing:-0} == 0)); then
      parity_status=3
    fi
  fi
  say "$label: $(verdict "$parity_status")"
}

verdict() {
  case "$1" in
    0) echo "no difference" ;;
    1) echo "FAILED: the sides differ, or the run had errors" ;;
    3) echo "INCOMPLETE: nothing could be compared" ;;
    *) echo "FAILED with status $1" ;;
  esac
}

# run_json_count prints a count of the parity run summary $1: counts.$2.
run_json_count() {
  sed -n "s/^ *\"$2\": *\([0-9][0-9]*\),*$/\1/p" "$1/run.json" | head -n 1
}

not_compared() {
  "$state/bin/rhtool" cases -repo "$repo" -json "$out/not-compared.json" >"$out/not-compared.txt"
  head -n 1 "$out/not-compared.txt" | sed 's/^/replayharness: /' >&2
  say "the list, with each reason: $out/not-compared.json"
}

# build_canary builds a Go server with two seeded bugs through a build
# overlay, so nothing in the checkout changes.
build_canary() {
  local c="$state/canary"
  local auction="$repo/endpoints/openrtb2/auction.go" info="$repo/endpoints/info/bidders_detail.go"
  mkdir -p "$c"
  sed 's|w.Header().Set("X-Prebid", version.BuildXPrebidHeader(version.Ver))|_ = version.BuildXPrebidHeader(version.Ver)|' \
    "$auction" >"$c/auction.go"
  sed '/^[[:space:]]*Status[[:space:]]/s|json:"status"|json:"state"|' "$info" >"$c/bidders_detail.go"
  if cmp -s "$auction" "$c/auction.go" || cmp -s "$info" "$c/bidders_detail.go"; then
    die "a canary patch did not apply"
  fi
  printf '{"Replace": {"%s": "%s", "%s": "%s"}}\n' "$auction" "$c/auction.go" "$info" "$c/bidders_detail.go" \
    >"$c/overlay.json"
  say "building the canary: X-Prebid dropped from /openrtb2/auction, status renamed state in /info/bidders/{name}"
  (cd "$repo" && go build -overlay "$c/overlay.json" -o "$state/bin/pbs-go-canary" .)
}

record_saved() {
  local stack="$state/stack-record" target token_file port raw="$state/saved.raw.jsonl"
  say "record-saved: starting the Go server with profile $profile"
  start_stack 1 "$stack"
  target=$(value RH_TARGET "$stack")
  token_file=$(value RH_TOKEN_FILE "$stack")
  port=$(value RH_SPARE_PORTS "$stack")
  say "record-saved: recording the replays through 127.0.0.1:$port"
  "$cli" record --upstream "$target" --listen "127.0.0.1:$port" -o "$raw" -q --keep-secrets 2>"$state/record.log" &
  proxy_pid=$!
  echo "$proxy_pid" >>"$roots_file"
  for _ in $(seq 1 100); do
    grep -q '^Recording ' "$state/record.log" 2>/dev/null && break
    kill -0 "$proxy_pid" 2>/dev/null || break
    sleep 0.1
  done
  if ! grep -q '^Recording ' "$state/record.log"; then
    cat "$state/record.log" >&2
    die "the recording proxy did not start"
  fi
  sleep 2
  check_breach
  "$cli" replay "$replays" --target "http://127.0.0.1:$port" --fail-on never --no-color >"$out/record-saved.txt"
  tail -n 1 "$out/record-saved.txt" >&2
  stop_pid "$proxy_pid" INT
  proxy_pid=""
  check_breach
  "$state/bin/rhtool" saved -repo "$repo" -token-file "$token_file" -o "$saved" "$raw" >&2
  rm -f "$raw"
  stop_stack
  check_breach
}

baseline_status=0
parity_status=0

case "$command" in
  cli)
    keep_cli=1
    replayharness_cli
    echo "$cli"
    ;;
  baseline)
    build_tools
    replayharness_cli
    guard
    run_baseline baseline
    exit "$baseline_status"
    ;;
  parity)
    build_tools
    guard
    run_parity parity run
    not_compared
    exit "$parity_status"
    ;;
  selftest)
    impl_b=go
    bin_b=""
    build_tools
    replayharness_cli
    guard
    run_parity selftest selftest
    selftest_status=$parity_status
    run_baseline baseline-aa
    not_compared
    say "selftest: parity $(verdict "$selftest_status"); baseline $(verdict "$baseline_status")"
    if ((selftest_status != 0)); then
      exit "$selftest_status"
    fi
    exit "$baseline_status"
    ;;
  canary)
    build_tools
    build_canary
    replayharness_cli
    guard
    impl_b=go
    bin_b="$state/bin/pbs-go-canary"
    run_parity canary-parity run
    mismatches=$(run_json_count "$out/canary-parity" mismatch)
    run_baseline canary-baseline
    say "canary: parity status $parity_status with ${mismatches:-no} mismatching cases; baseline status $baseline_status"
    if ((parity_status == 1 && ${mismatches:-0} > 0 && baseline_status == 1)); then
      say "canary: caught by both"
      exit 0
    fi
    die "canary: NOT caught by both"
    ;;
  record-saved)
    build_tools
    replayharness_cli
    guard
    record_saved
    ;;
esac
