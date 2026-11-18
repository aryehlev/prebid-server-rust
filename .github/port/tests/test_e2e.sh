#!/usr/bin/env bash
# The upstream-port workflow end to end. The run: blocks of .github/workflows/upstream-port.yml,
# of ci.yml (the change detection, the guard and the Go tooling job) and of
# parity-harness-sync.yml are extracted by step name and run in order the way the runner runs
# them: bash --noprofile --norc -eo pipefail, the job env, GITHUB_ENV and GITHUB_OUTPUT files, a
# fresh clone of a bare origin for every job, artifacts copied between jobs, and a step after a
# failed one skipped (an if: always() step still runs). The agent sessions are fake (they edit the
# work tree and return structured output); cargo, go, gofmt and taplo are fake; gh is gh-shim.sh;
# upstream is a local repository that the workflows mirror and check out with
# scripts/fetch-upstream.sh, exactly as they would the real one.
#
# One scheduled run is a "day". The story: no new upstream commits; a session that copies,
# symlinks, hard-links and vendors upstream code into the crates and nests a repository there (the
# guard fails the port job) and one that changes a read-only
# upstream tree (check-worktree.sh fails it); a small Go change, with upstream agent configuration
# and an upstream commit carrying Claude trailers, ported by a misbehaving agent; a conflict with
# main that rebuilds the branch; a huge change split into chunks (a fixture, a heavy core commit,
# a commit with too many Go lines), the first after a clean merge of main; a red port that is
# repaired twice without finding anything and escalates; a person fixing the branch; a conflict
# with main on a branch that carries a person's commit; merging the pull request; agents that
# never report; dry-run and pin-only modes; a publish that fails after pushing, the next day's
# restart and the re-run of the failed job; a failed port job; and the parity-harness sync: clean,
# a seams patch the new pin no longer fits, a person's fix, and a conflicting merge. No upstream
# object ever enters the repository.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

E2E="${T_ROOT}/e2e"
mkdir -p "${E2E}/days"
up="${E2E}/upstream"
mirror="${E2E}/mirror.git"
origin="${E2E}/origin.git"
human="${E2E}/human"
shim="${E2E}/gh"
workflows="${T_TOP}/.github/workflows"
extractor="${E2E}/extract.pl"
bot_name=${T_BOT_NAME}
bot_email=${T_BOT_EMAIL}
pin_header=$'# The upstream commit the port is pinned to.\n# Only the upstream-port workflow advances it.'
attribution_re=$(
  # shellcheck source=../scripts/lib.sh
  source "${T_SCRIPTS}/lib.sh"
  port_attribution_re
)
t_gh_shim "${shim}"
printf '[{"conclusion": "success"}]\n' > "${shim}/runs_main.json"
export E2E_CARGO_LOG="${E2E}/cargo.log" E2E_GO_LOG="${E2E}/go.log" E2E_FAIL_FETCH="${E2E}/fail-fetch" \
  E2E_GOMODCACHE="${E2E}/gomodcache"
mkdir -p "${E2E_GOMODCACHE}"
: > "${E2E_CARGO_LOG}"
: > "${E2E_GO_LOG}"

# extract.pl WORKFLOW JOB STEP: print the run: block of step STEP of job JOB (a literal block, a
# single-quoted or a plain scalar). A block with a ${{ }} expression is refused: the runner would
# evaluate it, the test cannot.
cat > "${extractor}" << 'EOF'
use strict;
use warnings;
my ($file, $job, $step) = @ARGV;
open(my $fh, "<", $file) or die "$file: $!\n";
my @lines = <$fh>;
close $fh;
my ($in_jobs, $current_job, $current_step) = (0, "", "");
for (my $i = 0; $i < @lines; $i++) {
  my $line = $lines[$i];
  if ($line =~ /^jobs:\s*$/) { $in_jobs = 1; next; }
  next unless $in_jobs;
  if ($line =~ /^  ([A-Za-z0-9_-]+):\s*$/) { ($current_job, $current_step) = ($1, ""); next; }
  if ($line =~ /^      - name:\s*(.*?)\s*$/) {
    $current_step = $1;
    $current_step =~ s/^"(.*)"$/$1/;
    next;
  }
  next unless $current_job eq $job && $current_step eq $step;
  next unless $line =~ /^        run:\s*(.*?)\s*$/;
  my $value = $1;
  my $script;
  if ($value eq "|" || $value eq "|-") {
    $script = "";
    for ($i++; $i < @lines; $i++) {
      my $body = $lines[$i];
      if ($body =~ /^\s*$/) { $script .= "\n"; next; }
      last unless $body =~ s/^          //;
      $script .= $body;
    }
  } elsif ($value =~ /^'(.*)'$/) {
    ($script = $1) =~ s/''/'/g;
    $script .= "\n";
  } else {
    $script = "$value\n";
  }
  die "step '$step' of job '$job' uses an expression the test cannot evaluate\n" if $script =~ /\$\{\{/;
  print $script;
  exit 0;
}
die "no run: block for step '$step' in job '$job' of $file\n";
EOF

# Fake tools. cargo logs where it runs and what; clippy and test fail while a crate source says
# CLIPPY_FAILS or TEST_FAILS; cargo fetch fails while ${E2E_FAIL_FETCH} exists; metadata lists no
# go_fixtures harness. go logs its calls and answers go env GOMODCACHE; gofmt finds nothing to
# change.
cat > "${T_ROOT}/bin/cargo" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s | %s\n' "${PWD}" "$*" >> "${E2E_CARGO_LOG:?}"
case " $* " in
  *" metadata "*) printf '{"packages": []}\n' ;;
  *" fetch "*)
    if [[ -f "${E2E_FAIL_FETCH:?}" ]]; then
      printf 'error: failed to download crates\n' >&2
      exit 101
    fi
    ;;
  *" clippy "*)
    if grep -rqs CLIPPY_FAILS crates; then
      printf 'error: a crate says CLIPPY_FAILS\n'
      exit 101
    fi
    ;;
  *" test "*)
    if grep -rqs TEST_FAILS crates; then
      printf 'test x ... FAILED\n'
      exit 101
    fi
    ;;
  *) ;;
esac
EOF
cat > "${T_ROOT}/bin/go" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s | %s\n' "${PWD}" "$*" >> "${E2E_GO_LOG:?}"
if [[ "${1:-} ${2:-}" == "env GOMODCACHE" ]]; then
  printf '%s\n' "${E2E_GOMODCACHE:?}"
fi
EOF
printf '#!/usr/bin/env bash\nexit 0\n' > "${T_ROOT}/bin/gofmt"
printf '#!/usr/bin/env bash\nexit 0\n' > "${T_ROOT}/bin/taplo"
chmod +x "${T_ROOT}/bin/cargo" "${T_ROOT}/bin/go" "${T_ROOT}/bin/gofmt" "${T_ROOT}/bin/taplo"

# The fake upstream (branch master, tag v4.0.0 on its first commit).
t_repo "${up}"
printf 'module github.com/prebid/prebid-server/v4\n\ngo 1.25.0\n' > "${up}/go.mod"
t_put "${up}" Dockerfile 'RUN wget https://dl.google.com/go/go1.26.2.linux-amd64.tar.gz'
t_put "${up}" main.go 'package main // v1'
t_put "${up}" exchange/exchange.go 'package exchange // v1'
t_put "${up}" adapters/foo/foo.go 'package foo // v1'
t_put "${up}" adapters/foo/footest/exemplary/a.json '{"a": 1}'
t_put "${up}" static/bidder-info/foo.yaml 'endpoint: "http://foo.example"'
t_put "${up}" "docs/a file.md" 'docs v1'
t_put "${up}" .github/workflows/upstream.yml 'name: upstream'
t_put "${up}" AGENTS.md 'upstream agents v1'
t_put "${up}" README.md 'readme v1'
t_put "${up}" LICENSE 'Apache License, Version 2.0 (test copy)'
u1=$(t_commit "${up}" "Initial upstream")
git -C "${up}" tag v4.0.0

# version_of SHA: the pseudo-version of upstream commit SHA, from a mirror of the test's own.
version_of() {
  env PBS_UPSTREAM_URL="${up}" "${T_SCRIPTS}/upstream-mirror.sh" "${mirror}" > /dev/null 2>&1
  env UPSTREAM_GIT="${mirror}" "${T_SCRIPTS}/pin-version.sh" "${1:?}"
}
u1_version=$(version_of "${u1}")

# seams FROM: a parity seams patch of upstream main.go that turns the line FROM into the harness's.
seams() {
  printf 'diff --git a/main.go b/main.go\n--- a/main.go\n+++ b/main.go\n@@ -1 +1 @@\n-%s\n+package main // patched by the parity harness\n' \
    "${1:?}"
}

# The repository: a bare origin whose main holds the Rust port's layout (this repository's port
# tooling, a small workspace with a Go tooling module, the pin on u1) and no upstream file, and a
# parity-harness branch with parity/, its seams patch and its own AGENTS.md.
git init -q --bare "${origin}"
git -C "${origin}" symbolic-ref HEAD refs/heads/main
git init -q -b main "${human}"
mkdir -p "${human}/.github" "${human}/scripts"
cp -R "${T_PORT}" "${human}/.github/port"
printf '%s\n%s\n' "${pin_header}" "${u1}" > "${human}/UPSTREAM_SYNC"
: > "${human}/upstream-port-log.jsonl"
printf '/target/\n/.upstream/\n' > "${human}/.gitignore"
t_put "${human}" Cargo.toml '[workspace]'
t_put "${human}" Cargo.lock 'version = 4'
t_put "${human}" taplo.toml '# taplo'
cp "${T_TOP}/rust-toolchain.toml" "${T_TOP}/rustfmt.toml" "${human}/"
cp "${T_TOP}/scripts/rustfmt-toolchain.sh" "${T_TOP}/scripts/fetch-upstream.sh" "${human}/scripts/"
printf '#!/usr/bin/env bash\nexit 0\n' > "${human}/scripts/check-no-lint-suppression.sh"
printf '#!/usr/bin/env bash\nexit 0\n' > "${human}/scripts/cargo-machete.sh"
chmod +x "${human}/scripts/check-no-lint-suppression.sh" "${human}/scripts/cargo-machete.sh"
t_put "${human}" crates/x/src/lib.rs '//! x'
mkdir -p "${human}/crates/x/testdata/go"
t_go_mod crates/x/testdata/go "${u1_version}" > "${human}/crates/x/testdata/go/go.mod"
t_put "${human}" crates/x/testdata/go/main.go 'package main // writes the goldens'
t_put "${human}" conformance/known_failures.txt '# known failures'
t_put "${human}" conformance/README.md 'the conformance contract'
t_put "${human}" AGENTS.md 'fork agents'
t_put "${human}" README.md 'the Rust port'
cp "${up}/LICENSE" "${human}/LICENSE"
t_commit "${human}" "Fork setup" > /dev/null
git -C "${human}" remote add origin "${origin}"
git -C "${human}" push -q origin main
git -C "${human}" switch -q -c parity-harness
t_put "${human}" parity/README.md 'the parity harness'
seams 'package main // v1' > "${human}/parity/upstream-seams.patch"
t_put "${human}" AGENTS.md 'fork agents and the parity harness'
t_commit "${human}" "Parity harness: the seams patch" > /dev/null
git -C "${human}" push -q origin parity-harness
git -C "${human}" switch -q main

# --- The runner -------------------------------------------------------------------------------

DAY=setup
D="${E2E}/days/0-setup"
mkdir -p "${D}"
day_count=0
RUN_URL=https://example.invalid/test/fork/actions/runs/0
MODE=auto
JOB_ENV=()
JOB_FAILED=""
STEP_LOG=""
WS=""
RUNNER_TEMP=""
GITHUB_ENV=""
GITHUB_STEP_SUMMARY=""

# github_env_args: what earlier steps of the job wrote to GITHUB_ENV (NAME=value lines), in
# GITHUB_ENV_ARGS for env(1).
github_env_args() {
  local line
  GITHUB_ENV_ARGS=()
  while IFS= read -r line; do
    case "${line}" in
      *'<<'*)
        printf 'github_env_args: multi-line values are not supported: %s\n' "${line}" >&2
        exit 1
        ;;
      [A-Za-z_]*=*) GITHUB_ENV_ARGS+=("${line}") ;;
      *) ;;
    esac
  done < "${GITHUB_ENV}"
}

# env_value NAME: the value earlier steps of the job wrote to GITHUB_ENV for NAME.
env_value() {
  local values
  values=$(sed -n "s/^${1:?}=//p" "${GITHUB_ENV}")
  printf '%s\n' "${values##*$'\n'}"
}

# job NAME: a job on a fresh runner (an empty workspace, as before actions/checkout, a temporary
# directory, GITHUB_ENV, summary).
job() {
  local name=${1:?}
  WS="${D}/${name}"
  RUNNER_TEMP="${D}/${name}-temp"
  GITHUB_ENV="${D}/${name}-env"
  GITHUB_STEP_SUMMARY="${D}/${name}-summary.md"
  JOB_FAILED=""
  # A port job makes upstream trees read-only.
  if [[ -e "${WS}" || -e "${RUNNER_TEMP}" ]]; then
    chmod -R u+w -- "${WS}" "${RUNNER_TEMP}" 2> /dev/null || true
  fi
  rm -rf -- "${WS}" "${RUNNER_TEMP}"
  mkdir -p "${WS}" "${RUNNER_TEMP}"
  : > "${GITHUB_ENV}"
  : > "${GITHUB_STEP_SUMMARY}"
}

# checkout REV: actions/checkout with fetch-depth 0: every branch of origin is fetched, and main
# or the commit REV (detached) is checked out in ${WS}.
checkout() {
  git clone -q "${origin}" "${WS}"
  if [[ "${1:?}" != main ]]; then
    git -C "${WS}" checkout -q --detach "${1}"
  fi
}

# run_step WORKFLOW JOB STEP [VAR=VALUE...]: run one step's run: block from ${WS} with the job env,
# GITHUB_ENV, the step env (the arguments) and a fresh GITHUB_OUTPUT. A step after a failed one is
# skipped. JOB_FAILED names the first failed step; STEP_LOG holds the step's output.
run_step() {
  local workflow=${1:?} job_name=${2:?} name=${3:?} script="${E2E}/step.sh" rc=0
  shift 3
  : > "${E2E}/step-output"
  STEP_LOG=""
  if [[ -n "${JOB_FAILED}" ]]; then
    printf '::step %s / %s: skipped\n' "${job_name}" "${name}" >> "${D}/log.txt"
    return 0
  fi
  perl "${extractor}" "${workflows}/${workflow}" "${job_name}" "${name}" > "${script}"
  github_env_args
  # errexit does not apply on the left of ||, so the cd that must not fail says so. The step env
  # wins over the job env, which wins over GITHUB_ENV.
  (
    cd "${WS}" || exit 1
    export GITHUB_OUTPUT="${E2E}/step-output" GITHUB_ENV GITHUB_STEP_SUMMARY RUNNER_TEMP GITHUB_WORKSPACE="${WS}"
    exec env "${GITHUB_ENV_ARGS[@]}" "${JOB_ENV[@]}" "$@" bash --noprofile --norc -eo pipefail "${script}"
  ) > "${E2E}/step.log" 2>&1 || rc=$?
  STEP_LOG=$(cat "${E2E}/step.log")
  {
    printf '::step %s / %s: exit status %s\n' "${job_name}" "${name}" "${rc}"
    printf '%s\n' "${STEP_LOG}"
  } >> "${D}/log.txt"
  if [[ "${rc}" -ne 0 ]]; then
    JOB_FAILED=${name}
  fi
}

# run_always WORKFLOW JOB STEP [VAR=VALUE...]: run_step for a step with if: always(), which also
# runs after a failed step; JOB_FAILED keeps the first failure.
run_always() {
  local failed=${JOB_FAILED}
  JOB_FAILED=""
  run_step "$@"
  if [[ -n "${failed}" ]]; then
    JOB_FAILED=${failed}
  fi
}

# out NAME: step output NAME of the last step.
out() {
  t_output "${1:?}" "${E2E}/step-output"
}

port_workflow_env() {
  JOB_ENV=("PBS_UPSTREAM_URL=${up}" UPSTREAM_BRANCH=master BASE_BRANCH=main PORT_BRANCH=port/upstream-sync
    "RUN_MODE=${MODE}" "PORT_GIT_NAME=${bot_name}" "PORT_GIT_EMAIL=${bot_email}" "RUN_URL=${RUN_URL}")
}

# ci_workflow_env: ci.yml's workflow env for the job in ${WS}.
ci_workflow_env() {
  JOB_ENV=(CARGO_TERM_COLOR=always CARGO_INCREMENTAL=0 RUST_BACKTRACE=1 "PBS_UPSTREAM_URL=${up}"
    "PBS_GO_ROOT=${WS}/.upstream" UPSTREAM_BRANCH=master BASE_BRANCH=main PORT_BRANCH=port/upstream-sync
    HARNESS_BRANCH=parity-harness "PORT_GIT_EMAIL=${bot_email}" GOTOOLCHAIN=local GOFLAGS=-mod=readonly)
}

# The plan job; its outputs land in P_*.
plan_job() {
  local wf=upstream-port.yml
  job plan
  port_workflow_env
  checkout main
  run_step "${wf}" plan "Mirror upstream"
  run_step "${wf}" plan "Compute the plan" GH_TOKEN=test-token "MAX_COMMITS=${MAX_COMMITS_INPUT:-3}" \
    MAX_PR_COMMITS=10 MAX_REPAIRS=2 MAX_GO_LINES=3000 TARGET_INPUT= "OUT_DIR=${RUNNER_TEMP}/port-plan"
  P_action=$(out action)
  P_reason=$(out reason)
  P_work_sha=$(out work_sha)
  P_port_branch_sha=$(out port_branch_sha)
  P_restart=$(out restart)
  P_pr_number=$(out pr_number)
  P_base_sha=$(out base_sha)
  P_target_sha=$(out target_sha)
  P_target_version=$(out target_version)
  P_adapter_dirs=$(out adapter_dirs)
  P_needs_claude=$(out needs_claude)
  P_budget_class=$(out budget_class)
  run_step "${wf}" plan "Stage the default branch's tooling (a tar keeps the executable bits)"
  if [[ -z "${JOB_FAILED}" ]]; then
    rm -rf -- "${D}/artifact-port-plan"
    cp -R "${RUNNER_TEMP}/port-plan" "${D}/artifact-port-plan"
  fi
  run_step "${wf}" plan "Summary" "ACTION=${P_action}" "REASON=${P_reason}"
  R_plan=ok
  if [[ -n "${JOB_FAILED}" ]]; then
    R_plan=fail
  fi
}

# agent SESSION BEHAVIOUR: one claude-code-action session in the port workspace. It edits the work
# tree as BEHAVIOUR says and sets AGENT_OUTCOME (the step outcome; the action fails without a
# structured result) and AGENT_JSON (its structured_output).
agent() {
  local session=${1:?} behaviour=${2:?} lib="${WS}/crates/x/src/lib.rs" status=ported evil=false probe vendor
  AGENT_OUTCOME=success
  AGENT_JSON=""
  printf '%s %s\n' "${session}" "${behaviour}" >> "${D}/agents.txt"
  case "${behaviour}" in
    port) printf '//! x ported through %s\n' "${P_target_sha:0:12}" > "${lib}" ;;
    break) printf '//! x ported through %s CLIPPY_FAILS\n' "${P_target_sha:0:12}" > "${lib}" ;;
    noop) status=partial ;;
    evil)
      # Everything a session must not do; check-worktree.sh and the guard undo or catch it all.
      evil=true
      printf '//! x ported through %s\n' "${P_target_sha:0:12}" > "${lib}"
      git -C "${WS}" add -A
      env GIT_AUTHOR_NAME=Claude GIT_AUTHOR_EMAIL=noreply@anthropic.com GIT_COMMITTER_NAME=Claude \
        GIT_COMMITTER_EMAIL=noreply@anthropic.com \
        git -C "${WS}" commit -q -m $'Agent commit\n\nCo-Authored-By: Claude <noreply@anthropic.com>'
      t_put "${WS}" README.md 'rewritten by the agent'
      t_put "${WS}" .claude/settings.json '{"hooks": {}}'
      t_put "${WS}" .mcp.json '{"mcpServers": {}}'
      t_put "${WS}" clippy.toml 'too-many-lines-threshold = 100000'
      t_put "${WS}" UPSTREAM_SYNC '0000000000000000000000000000000000000000'
      printf '{"upstream": "forged"}\n' >> "${WS}/upstream-port-log.jsonl"
      printf 'adapters/foo/footest/exemplary/a.json\n' >> "${WS}/conformance/known_failures.txt"
      t_go_mod crates/x/testdata/go "${u1_version}" > "${WS}/crates/x/testdata/go/go.mod"
      t_put "${WS}" crates/x/src/zz_parity_scratch_test.go 'package x'
      probe=$(env_value PROBE_ROOT)
      t_put "${probe}" adapters/foo/zz_parity_scratch_test.go 'package foo'
      ;;
    vendor)
      # Upstream code copied into the writable paths: check-worktree.sh keeps it, the guard fails it.
      printf '//! x ported through %s\n' "${P_target_sha:0:12}" > "${lib}"
      cp "${WS}/.upstream/adapters/foo/foo.go" "${WS}/crates/x/src/foo.go"
      mkdir -p "${WS}/testdata/foo"
      cp "${WS}/.upstream/adapters/foo/footest/exemplary/a.json" "${WS}/testdata/foo/a.json"
      ln -s ../../../.upstream "${WS}/crates/x/src/upstream"
      vendor="${WS}/crates/x/testdata/go/vendor/github.com/prebid/prebid-server/v4"
      mkdir -p "${vendor}"
      cp "${WS}/.upstream/go.mod" "${vendor}/go.mod"
      printf 'go 1.25.0\n\nuse .\n' > "${WS}/crates/x/testdata/go/go.work"
      # A nested repository, which git stages as a gitlink, and a hard link to a file of the
      # read-only upstream tree, which git stores as a copy of its bytes.
      git init -q "${WS}/crates/x/nested"
      t_put "${WS}/crates/x/nested" README.md 'a repository of its own'
      t_commit "${WS}/crates/x/nested" "A repository of its own" > /dev/null
      ln "${WS}/.upstream/adapters/foo/footest/exemplary/a.json" "${WS}/crates/x/testdata/a-link.json"
      ;;
    tamper)
      # The session gets around the read-only new pin's tree and edits it.
      printf '//! x ported through %s\n' "${P_target_sha:0:12}" > "${lib}"
      chmod u+w "${WS}/.upstream" "${WS}/.upstream/main.go"
      printf 'package main // edited by the agent\n' > "${WS}/.upstream/main.go"
      ;;
    fail)
      AGENT_OUTCOME=failure
      return 0
      ;;
    clean)
      AGENT_JSON='{"summary": "no difference found", "blocking_count": 0, "findings": [], "adversarial_tests": []}'
      return 0
      ;;
    blocking)
      t_put "${WS}" crates/x/tests/adversarial_foo.rs '//! adversarial'
      AGENT_JSON=$(jq -n -c '{summary: "one difference", blocking_count: 1, findings: [{severity: "blocking",
        title: "foo drops an empty imp.ext", go_ref: "adapters/foo/foo.go:1", rust_ref: "crates/x/src/lib.rs:1",
        input: "imp.ext = {}", go_behavior: "an error", rust_behavior: "no error", evidence: "probe output"}],
        adversarial_tests: [{path: "crates/x/tests/adversarial_foo.rs", status: "fails_now"}]}')
      return 0
      ;;
    repair) perl -pi -e 's/ CLIPPY_FAILS//' "${lib}" ;;
    *)
      printf 'agent: unknown behaviour %s\n' "${behaviour}" >&2
      exit 2
      ;;
  esac
  AGENT_JSON=$(jq -c --arg status "${status}" --arg session "${session}" --argjson evil "${evil}" '{
    overall_status: $status,
    summary: (if $evil then "Ported.\nCo-Authored-By: Claude <noreply@anthropic.com>" else "fake \($session) session" end),
    commits: [.commits[] | {sha, status: (if .needs_claude then $status else "no_change_needed" end), notes: "fake"}],
    rust_files_changed: ["crates/x/src/lib.rs"],
    verification: [{command: "cargo test --workspace --locked", result: "pass"}],
    not_ported: (if $evil then [{go_path: "adapters/bar/bar.go", reason: "video ~~~ ping @someone"}] else [] end),
    followups: []}' "${RUNNER_TEMP}/port-ctx/plan.json")
}

# The port job; PORT_RESULT is its result and PORT_FAILED its first failed step.
port_job() {
  local wf=upstream-port.yml after_status fix
  job port
  cp -R "${D}/artifact-port-plan" "${RUNNER_TEMP}/port-plan"
  checkout "${P_work_sha}"
  port_workflow_env
  JOB_ENV+=("ACTION=${P_action}" "BASE_SHA=${P_base_sha}" "TARGET_SHA=${P_target_sha}"
    "PIN_VERSION=${P_target_version}" "WORK_SHA=${P_work_sha}" "PR_NUMBER=${P_pr_number}"
    "ADAPTER_DIRS=${P_adapter_dirs}" "NEEDS_CLAUDE=${P_needs_claude}" "BUDGET_CLASS=${P_budget_class}"
    PORT_MODEL=claude-opus-5-5 REVIEW_MODEL=claude-opus-5-5 PORT_FALLBACK_MODEL=claude-sonnet-5-5
    PORT_BUDGET_USD=30 PORT_CORE_BUDGET_USD=60 REVIEW_BUDGET_USD=10 FIX_BUDGET_USD=15
    "GIT_AUTHOR_NAME=${bot_name}" "GIT_AUTHOR_EMAIL=${bot_email}" "GIT_COMMITTER_NAME=${bot_name}"
    "GIT_COMMITTER_EMAIL=${bot_email}" CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1 CARGO_TERM_COLOR=never
    CARGO_INCREMENTAL=0 RUST_BACKTRACE=1 GOTOOLCHAIN=local GOFLAGS=-mod=readonly)
  run_step "${wf}" port "Prepare the work branch"
  run_step "${wf}" port "Merge the default branch"
  run_step "${wf}" port "Check out the upstream trees (old pin, new pin, probe tree)"
  run_step "${wf}" port "Advance the pin (UPSTREAM_SYNC and the Go tooling modules)"
  run_step "${wf}" port "Go version of the upstream Dockerfile"
  GO_VERSION=$(out version)
  # Installing Go, the Rust toolchains, taplo and cargo-machete and restoring the cache are
  # actions and downloads; the fake tools stand in for them.
  run_step "${wf}" port "Build the porting context"
  run_step "${wf}" port "Prefetch dependencies (the last network use before the agents)"
  run_step "${wf}" port "Gates before the agents"
  run_step "${wf}" port "Render the agent inputs"
  RUN_CLAUDE=$(out run_claude)
  PORT_PROMPT=$(out port_prompt)
  PORT_JSON=""
  REVIEW_JSON=""
  FIX_JSON=""
  AFTER_LOG=""
  if [[ -z "${JOB_FAILED}" && "${RUN_CLAUDE}" == true ]]; then
    agent port "${AGENT_PORT}"
    PORT_JSON=${AGENT_JSON}
    run_step "${wf}" port "Gates after the port"
    AFTER_LOG=${STEP_LOG}
    after_status=$(out status)
    if [[ -z "${JOB_FAILED}" && "${AGENT_OUTCOME}" == success ]]; then
      agent review "${AGENT_REVIEW}"
      REVIEW_JSON=${AGENT_JSON}
    fi
    run_step "${wf}" port "Decide on the fix session" "REVIEW_JSON=${REVIEW_JSON}" "AFTER_STATUS=${after_status}"
    fix=$(out fix)
    if [[ -z "${JOB_FAILED}" && "${fix}" == true ]]; then
      agent fix "${AGENT_FIX}"
      FIX_JSON=${AGENT_JSON}
    fi
  fi
  run_step "${wf}" port "Final gates (authoritative)" "CLAUDE_RAN=${RUN_CLAUDE}"
  FINAL_STATUS=$(out status)
  run_step "${wf}" port "Commit with the bot identity and no AI attribution" "PORT_JSON=${PORT_JSON}" \
    "FIX_JSON=${FIX_JSON}"
  COMMIT_LOG=${STEP_LOG}
  run_step "${wf}" port "Package the result" "PORT_JSON=${PORT_JSON}" "REVIEW_JSON=${REVIEW_JSON}" \
    "FIX_JSON=${FIX_JSON}" "FINAL_STATUS=${FINAL_STATUS}" "CLAUDE_RAN=${RUN_CLAUDE}"
  if [[ -z "${JOB_FAILED}" ]]; then
    rm -rf -- "${D}/artifact-port-result"
    cp -R "${RUNNER_TEMP}/out" "${D}/artifact-port-result"
  fi
  PORT_FAILED=${JOB_FAILED}
  run_always "${wf}" port "Make the upstream trees writable again"
  PORT_RESULT=success
  R_port=ok
  if [[ -n "${JOB_FAILED}" ]]; then
    PORT_RESULT=failure
    R_port=fail
  fi
}

# publish_job [NAME]: the publish job (NAME publish-rerun re-runs it with the same inputs).
publish_job() {
  local wf=upstream-port.yml no_changes
  job "${1:-publish}"
  port_workflow_env
  JOB_ENV+=("ACTION=${P_action}" "REASON=${P_reason}" "PR_NUMBER=${P_pr_number}" "RESTART=${P_restart}"
    "PORT_BRANCH_SHA=${P_port_branch_sha}" "WORK_SHA=${P_work_sha}" "ADAPTER_DIRS=${P_adapter_dirs}"
    "PORT_RESULT=${PORT_RESULT}" HAS_APP_KEY=false HAS_PUSH_TOKEN=true)
  PUSH_LOG=""
  run_step "${wf}" publish "Require a push credential" HAS_APP_TOKEN=false
  checkout main
  if [[ "${P_action}" == escalate ]]; then
    run_step "${wf}" publish "Escalate to a person" GH_TOKEN=test-token
  fi
  if [[ "${P_action}" != escalate && "${PORT_RESULT}" =~ ^(failure|cancelled)$ ]]; then
    run_step "${wf}" publish "Report the failed port job" GH_TOKEN=test-token
  fi
  if [[ "${PORT_RESULT}" == success ]]; then
    mkdir -p "${RUNNER_TEMP}/result"
    cp -R "${D}/artifact-port-result/." "${RUNNER_TEMP}/result/"
    run_step "${wf}" publish "Import the bundle and run the authoritative guard"
    no_changes=$(sed -n 's/^NO_CHANGES=//p' "${GITHUB_ENV}")
    if [[ "${no_changes}" != 1 ]]; then
      run_step "${wf}" publish "Push the port branch (never the default branch)"
      PUSH_LOG=${STEP_LOG}
      run_step "${wf}" publish "Open or update the pull request" GH_TOKEN=test-token
    fi
  fi
  R_publish=ok
  if [[ -n "${JOB_FAILED}" ]]; then
    R_publish=fail
  fi
}

# day NAME [mode=M] [port=B] [review=B] [fix=B] [max_commits=N]: one run of upstream-port.yml.
day() {
  local setting
  DAY=${1:?}
  shift
  MODE=auto
  AGENT_PORT=port
  AGENT_REVIEW=clean
  AGENT_FIX=port
  MAX_COMMITS_INPUT=""
  for setting in "$@"; do
    case "${setting}" in
      mode=*) MODE=${setting#*=} ;;
      port=*) AGENT_PORT=${setting#*=} ;;
      review=*) AGENT_REVIEW=${setting#*=} ;;
      fix=*) AGENT_FIX=${setting#*=} ;;
      max_commits=*) MAX_COMMITS_INPUT=${setting#*=} ;;
      *)
        printf 'day: unknown setting %s\n' "${setting}" >&2
        exit 2
        ;;
    esac
  done
  day_count=$((day_count + 1))
  D="${E2E}/days/${day_count}-${DAY}"
  mkdir -p "${D}"
  RUN_URL="https://example.invalid/test/fork/actions/runs/${day_count}"
  : > "${shim}/calls.log"
  find "${shim}" -maxdepth 1 -name 'body-*.md' -delete
  R_plan=skip
  R_port=skip
  R_publish=skip
  PORT_RESULT=skipped
  PORT_FAILED=""
  RUN_CLAUDE=""
  FINAL_STATUS=""
  COMMIT_LOG=""
  AFTER_LOG=""
  plan_job
  if [[ "${R_plan}" == ok && "${P_action}" =~ ^(advance|repair)$ ]]; then
    port_job
  fi
  if [[ "${R_plan}" == ok && "${P_action}" =~ ^(advance|repair|escalate)$ && "${MODE}" != dry-run ]]; then
    publish_job
  fi
  CALLS=$(cat "${shim}/calls.log")
}

# ci: ci.yml on the port pull request: the changes job (mirror and compare), the guard job, and the
# Go tooling job with the pinned upstream checkout, in which the gates stand in for the Rust jobs.
# ci-ok lands in the gh shim's checks.json; CI_GUARD is ok or the failed step, CI_DIRS the adapter
# directories, CI_RESULT pass or fail.
ci() {
  local wf=ci.yml head base pr title body c_base c_head c_parity go_status=fail gates_status=fail
  head=$(git -C "${origin}" rev-parse refs/heads/port/upstream-sync)
  base=$(git -C "${origin}" rev-parse refs/heads/main)
  pr=$(jq -r '.[0].number' "${shim}/prs.json")
  title=$(cat "${shim}/pr_title.txt")
  body=$(cat "${shim}/pr_body.md")
  job ci-changes
  ci_workflow_env
  checkout "${head}"
  run_step "${wf}" changes "Mirror upstream"
  run_step "${wf}" changes Compare EVENT=pull_request "REF=refs/pull/${pr}/merge" REPOSITORY=test/fork \
    "PR_BASE=${base}" "PR_HEAD=${head}" PR_HEAD_REF=port/upstream-sync PR_HEAD_REPO=test/fork BASE_REF=main
  c_base=$(out base)
  c_head=$(out head)
  CI_DIRS=$(out adapter_dirs)
  C_PORT_PR=$(out port_pr)
  c_parity=$(out parity_harness)
  job ci-guard
  ci_workflow_env
  checkout "${head}"
  run_step "${wf}" guard "Mirror upstream"
  run_step "${wf}" guard "Check the repository invariants (no upstream file, pin, attribution)" "BASE=${c_base}" \
    "HEAD_SHA=${c_head}" "ADAPTER_DIRS=${CI_DIRS}" "PORT_PR=${C_PORT_PR}" "PARITY_HARNESS=${c_parity}" \
    "PR_TITLE=${title}" "PR_BODY=${body}"
  CI_GUARD=${JOB_FAILED:-ok}
  job ci-go
  ci_workflow_env
  checkout "${head}"
  run_step "${wf}" go-tooling "Pinned upstream commit"
  run_step "${wf}" go-tooling "Fetch upstream"
  run_step "${wf}" go-tooling "Go version of the upstream Dockerfile"
  run_step "${wf}" go-tooling "gofmt -s, go build and go test of every Go tooling module"
  if [[ -z "${JOB_FAILED}" ]]; then
    go_status=pass
  fi
  t_run_in "${WS}" env "${JOB_ENV[@]}" .github/port/scripts/gates.sh --out "${RUNNER_TEMP}/gates.json" \
    --strict-dirs "${CI_DIRS}" --only fmt,clippy,doc,test,suppression,taplo,machete,devtools,conformance
  if [[ "${T_RC}" -eq 0 ]]; then
    gates_status=pass
  fi
  CI_RESULT=fail
  if [[ "${CI_GUARD}" == ok && "${go_status}" == pass && "${gates_status}" == pass ]]; then
    CI_RESULT=pass
  fi
  printf '[{"name": "ci-ok", "bucket": "%s"}]\n' "${CI_RESULT}" > "${shim}/checks.json"
  jq -n -c --argjson id "$((day_count + 1000))" --arg head "${head}" --arg result "${CI_RESULT}" \
    '[{databaseId: $id, headSha: $head, status: "completed", conclusion: (if $result == "pass" then "success" else "failure" end)}]' \
    > "${shim}/runs_port_upstream-sync.json"
  printf '%s\n' "${T_OUT}" > "${shim}/run_log.txt"
  {
    printf '::ci on %s: guard %s, go tooling %s, gates %s\n' "${head:0:12}" "${CI_GUARD}" "${go_status}" "${gates_status}"
    printf '%s\n' "${T_OUT}"
  } >> "${D}/log.txt"
}

# ci_main BEFORE: ci.yml's guard on a push to main from BEFORE (main rules).
ci_main() {
  local wf=ci.yml c_base c_head c_dirs c_port c_parity
  job ci-main-changes
  ci_workflow_env
  checkout main
  run_step "${wf}" changes "Mirror upstream"
  run_step "${wf}" changes Compare EVENT=push REF=refs/heads/main REPOSITORY=test/fork "PUSH_BEFORE=${1:?}"
  c_base=$(out base)
  c_head=$(out head)
  c_dirs=$(out adapter_dirs)
  c_port=$(out port_pr)
  c_parity=$(out parity_harness)
  job ci-main-guard
  ci_workflow_env
  checkout main
  run_step "${wf}" guard "Mirror upstream"
  run_step "${wf}" guard "Check the repository invariants (no upstream file, pin, attribution)" "BASE=${c_base}" \
    "HEAD_SHA=${c_head}" "ADAPTER_DIRS=${c_dirs}" "PORT_PR=${c_port}" "PARITY_HARNESS=${c_parity}" PR_TITLE= PR_BODY=
  CI_MAIN=${JOB_FAILED:-ok}
}

# parity_sync: parity-harness-sync.yml. SYNC_FAILED is its failed step, SYNC_LOG the sync's output.
parity_sync() {
  local wf=parity-harness-sync.yml
  : > "${shim}/calls.log"
  find "${shim}" -maxdepth 1 -name 'body-*.md' -delete
  job sync
  JOB_ENV=(BASE_BRANCH=main HARNESS_BRANCH=parity-harness "PBS_UPSTREAM_URL=${up}" DRY_RUN=false
    "RUN_URL=${RUN_URL}" "GIT_AUTHOR_NAME=${bot_name}" "GIT_AUTHOR_EMAIL=${bot_email}"
    "GIT_COMMITTER_NAME=${bot_name}" "GIT_COMMITTER_EMAIL=${bot_email}" HAS_APP_KEY=false HAS_PUSH_TOKEN=true)
  run_step "${wf}" sync "Require a push credential" HAS_APP_TOKEN=false
  checkout main
  run_step "${wf}" sync "Merge, check the seams patch against the new pin, and push (or report)" GH_TOKEN=test-token
  SYNC_FAILED=${JOB_FAILED}
  SYNC_LOG=${STEP_LOG}
  CALLS=$(cat "${shim}/calls.log")
}

# --- People and upstream ----------------------------------------------------------------------

human_fetch() {
  git -C "${human}" fetch -q --prune origin
}

# human_main_commit PATH CONTENT MESSAGE: a person changes PATH on main (a merged pull request).
human_main_commit() {
  human_fetch
  git -C "${human}" switch -q -C main origin/main
  t_put "${human}" "${1:?}" "${2:?}"
  t_commit "${human}" "${3:?}" > /dev/null
  git -C "${human}" push -q origin main
}

# merge_pr: a person merges the port pull request with a merge commit, GitHub deletes the head
# branch, and CI runs on main.
merge_pr() {
  local number before
  number=$(jq -r '.[0].number' "${shim}/prs.json")
  human_fetch
  git -C "${human}" switch -q -C main origin/main
  before=$(git -C "${human}" rev-parse HEAD)
  git -C "${human}" merge -q --no-ff -m "Merge pull request #${number} from test/fork/port/upstream-sync" \
    origin/port/upstream-sync
  git -C "${human}" push -q origin main
  git -C "${human}" push -q origin --delete port/upstream-sync
  printf '[]\n' > "${shim}/prs.json"
  rm -f -- "${shim}/pr_view.json" "${shim}/checks.json"
  ci_main "${before}"
}

# remove_label NAME: a person removes a label from the pull request.
remove_label() {
  jq -c --arg name "${1:?}" '.labels |= map(select(.name != $name))' "${shim}/pr_view.json" > "${shim}/pr_view.tmp"
  mv "${shim}/pr_view.tmp" "${shim}/pr_view.json"
}

up_commit() {
  t_commit "${up}" "${1:?}"
}

# --- Assertions -------------------------------------------------------------------------------

check() {
  t_eq "${DAY}: ${1:?}" "${2?}" "${3?}"
}

contains() {
  t_has "${DAY}: ${1:?}" "${2?}" "${3?}"
}

lacks() {
  t_lacks "${DAY}: ${1:?}" "${2?}" "${3?}"
}

# jobs_are PLAN PORT PUBLISH: the day's job results (ok, fail or skip); the log is shown when they
# differ.
jobs_are() {
  local actual="${R_plan} ${R_port} ${R_publish}"
  check "job results" "${1:?} ${2:?} ${3:?}" "${actual}"
  if [[ "${actual}" != "${1} ${2} ${3}" ]]; then
    tail -n 80 "${D}/log.txt"
  fi
}

branch_head() {
  git -C "${origin}" rev-parse -q --verify refs/heads/port/upstream-sync || true
}

# pin_of REV: the commit UPSTREAM_SYNC pins at REV of origin.
pin_of() {
  local text
  text=$(git -C "${origin}" show "${1:?}:UPSTREAM_SYNC")
  printf '%s\n' "${text##*$'\n'}"
}

# plan_commits: the commits of the day's plan, space-separated.
plan_commits() {
  jq -r '[.commits[].sha] | join(" ")' "${D}/artifact-port-plan/plan.json"
}

# no_attribution WHAT TEXT: TEXT carries no AI attribution.
no_attribution() {
  local hit
  hit=$(grep -Ei -m 1 -e "${attribution_re}" <<< "${2?}" || true)
  check "no AI attribution in ${1:?}" "" "${hit}"
}

# branch_messages REF: the identities and messages of the first-parent commits of REF that main
# does not have.
branch_messages() {
  git -C "${origin}" log --first-parent --format='%an <%ae> / %cn <%ce>%n%B' "${1:?}" --not refs/heads/main
}

# --- The story --------------------------------------------------------------------------------

day uptodate
jobs_are ok skip skip
check "action" none "${P_action}"
contains "reason" "${P_reason}" "up to date with upstream at ${u1:0:12}"
branches=$(git -C "${origin}" for-each-ref --format='%(refname:short)' refs/heads)
check "nothing is pushed" $'main\nparity-harness' "${branches}"
lacks "no pull request is touched" "${CALLS}" "pr create"
parity_sync
check "parity sync with nothing to merge" "" "${SYNC_FAILED}"
contains "parity sync: the seams fit the pin" "${SYNC_LOG}" "nothing to push; parity-harness fits upstream ${u1:0:12}"

# A small Go change, an upstream file deletion with upstream changes to its workflows, AGENTS.md
# and agent configuration, and a new adapter whose upstream commit carries Claude trailers.
t_put "${up}" adapters/foo/foo.go 'package foo // v2'
u2=$(up_commit "foo: change the adapter")
git -C "${up}" rm -q "docs/a file.md"
t_put "${up}" .github/workflows/upstream.yml 'name: upstream v2'
t_put "${up}" AGENTS.md 'upstream agents v2'
t_put "${up}" .mcp.json '{"mcpServers": {"x": {"command": "sh"}}}'
t_put "${up}" .claude/settings.json '{"permissions": {"allow": ["Bash"]}}'
u3=$(up_commit "Remove a doc; update the workflows and the agent configuration")
t_put "${up}" adapters/bar/bar.go 'package bar // v1'
t_put "${up}" adapters/bar/bartest/exemplary/a.json '{"bar": 1}'
t_put "${up}" static/bidder-info/bar.yaml 'endpoint: "http://bar.example"'
u4=$(up_commit $'New Adapter: Bar (#4397)\n\n\xf0\x9f\xa4\x96 Generated with [Claude Code](https://claude.ai/code)\n\nCo-Authored-By: Claude <noreply@anthropic.com>')

# A session that copies upstream code into the crates and testdata/: all of it is in the writable
# paths, so check-worktree.sh keeps it, and the guard the commit step runs fails the port job.
day vendor port=vendor
jobs_are ok fail ok
check "action" advance "${P_action}"
check "the commit step fails" "Commit with the bot identity and no AI attribution" "${PORT_FAILED}"
contains "the guard: Go code outside the Go tooling modules" "${COMMIT_LOG}" "Go code outside the Go tooling modules"
contains "the guard: the copied Go file" "${COMMIT_LOG}" "crates/x/src/foo.go"
contains "the guard: copies of upstream files" "${COMMIT_LOG}" "files byte-identical to an upstream file"
contains "the guard: the copied fixture" "${COMMIT_LOG}" "testdata/foo/a.json"
contains "the guard: a nested repository" "${COMMIT_LOG}" "symbolic links and submodules are not allowed: crates/x/nested (160000)"
contains "the guard: a symbolic link" "${COMMIT_LOG}" "crates/x/src/upstream (120000)"
contains "the guard: a hard link into the upstream tree" "${COMMIT_LOG}" "crates/x/testdata/a-link.json"
contains "the guard: vendor/ and go.work" "${COMMIT_LOG}" "a .gitmodules, go.work or vendor/ directory brings in code from elsewhere"
vendor_head=$(branch_head)
check "nothing is pushed" "" "${vendor_head}"
contains "the failure is reported" "${CALLS}" "issue create --repo test/fork --title Upstream port run failed --label port:run-failed"
lacks "no pull request" "${CALLS}" "pr create"

# A session that gets around the read-only tree of the new pin: check-worktree.sh fails the port
# job right after the session, before anything is committed.
day tamper port=tamper
jobs_are ok fail ok
check "the port job stops after the session" "Gates after the port" "${PORT_FAILED}"
contains "the changed tree is refused" "${AFTER_LOG}" "a session changed a read-only tree"
contains "the open failure issue is commented" "${CALLS}" "issue comment 201"
tamper_head=$(branch_head)
check "nothing is pushed" "" "${tamper_head}"
# A person reads the issue and closes it.
rm -f -- "${shim}/issues_port:run-failed.json"

# The misbehaving agent commits, edits outside the writable paths, adds agent configuration and a
# clippy.toml, moves the pin, forges a port log line, adds a known failure, sets the Go tooling
# module back to the old version, leaves Go probes and writes attribution.
day small port=evil
jobs_are ok ok ok
check "action" advance "${P_action}"
commits=$(plan_commits)
check "the chunk" "${u2} ${u3} ${u4}" "${commits}"
check "adapter directories" "bar,foo" "${P_adapter_dirs}"
u4_version=$(version_of "${u4}")
check "the version of the new pin" "${u4_version}" "${P_target_version}"
check "the Go version comes from the new pin's Dockerfile" 1.26.2 "${GO_VERSION}"
check "the agents ran" true "${RUN_CLAUDE}"
contains "the port prompt lists the commits" "${PORT_PROMPT}" "${u4:0:12}"
contains "the port prompt names the old pin's tree" "${PORT_PROMPT}" "upstream-old: the old pin ${u1}"
upstream_diff=$(cat "${D}/port-temp/port-ctx/upstream.diff")
contains "the agents get the whole Go change" "${upstream_diff}" "adapters/bar/bar.go"
check "final gates" pass "${FINAL_STATUS}"
go_tests=$(grep -c -F -e "${D}/port/crates/x/testdata/go | test ./..." "${E2E_GO_LOG}" || true)
check "the gates test the Go tooling module before, after and at the end" 3 "${go_tests}"
port1=$(branch_head)
main_now=$(git -C "${origin}" rev-parse refs/heads/main)
parents=$(git -C "${origin}" rev-list --parents -n 1 "${port1}")
check "one commit on main, nothing of upstream merged" "${port1} ${main_now}" "${parents}"
listing=$(git -C "${origin}" ls-tree -r --name-only "${port1}")
copied=$(grep -E -e '^(main\.go|go\.mod|Dockerfile|exchange/|adapters/|static/|docs/|\.mcp\.json|\.claude/|\.github/workflows/)' <<< "${listing}" || true)
check "the branch holds no upstream file" "" "${copied}"
pin=$(git -C "${origin}" show "${port1}:UPSTREAM_SYNC")
check "the pin moves to the chunk's last commit, its comments kept" "${pin_header}"$'\n'"${u4}" "${pin}"
gomod=$(git -C "${origin}" show "${port1}:crates/x/testdata/go/go.mod")
requirement=$(grep '^require' <<< "${gomod}" || true)
check "the Go tooling module requires the new pin" "require github.com/prebid/prebid-server/v4 ${u4_version}" "${requirement}"
readme=$(git -C "${origin}" show "${port1}:README.md")
check "the agent's edit of a protected file is reverted" "the Rust port" "${readme}"
tracked=$(git -C "${origin}" ls-tree -r --name-only "${port1}" -- .mcp.json .claude clippy.toml crates/x/src/zz_parity_scratch_test.go)
check "no agent configuration, clippy.toml or probe is tracked" "" "${tracked}"
known=$(git -C "${origin}" show "${port1}:conformance/known_failures.txt")
check "the added known failure is dropped" "# known failures" "${known}"
port_log=$(git -C "${origin}" show "${port1}:upstream-port-log.jsonl")
statuses=$(jq -r -s 'map(.status) | join(",")' <<< "${port_log}")
check "one port log line per commit, the forged line dropped" "ported,ported,ported" "${statuses}"
probe_left=gone
if [[ -e "${D}/port-temp/upstream-probe/adapters/foo/zz_parity_scratch_test.go" ]]; then
  probe_left=present
fi
check "the probe tree is reset after the session" gone "${probe_left}"
subject=$(git -C "${origin}" log -1 --format=%s "${port1}")
check "the port commit" "port: foo: change the adapter (+2 more)" "${subject}"
identity=$(git -C "${origin}" log -1 --format='%an <%ae> / %cn <%ce>' "${port1}")
check "the bot commits" "${bot_name} <${bot_email}> / ${bot_name} <${bot_email}>" "${identity}"
messages=$(branch_messages "${port1}")
no_attribution "the branch's commits" "${messages}"
lacks "the agent's own commit is not published" "${messages}" "Agent commit"
contains "the commit names the upstream range" "${messages}" "Upstream-Range: ${u1:0:12}..${u4:0:12}"
contains "the commit names the upstream version" "${messages}" "Upstream-Version: ${u4_version}"
contains "an upstream commit is listed by its subject only" "${messages}" "upstream ${u4:0:12} New Adapter: Bar (#4397) (ported)"
report=$(cat "${D}/artifact-port-result/report.md")
contains "the report: the undone session commit" "${report}" "the session created commits"
contains "the report: the reverted changes" "${report}" "changes outside the writable paths were reverted"
contains "the report: the protected file" "${report}" "README.md"
contains "the report: the agent configuration" "${report}" ".mcp.json"
contains "the report: the dropped known failure" "${report}" "entries added to conformance/known_failures.txt were dropped"
contains "pull request created" "${CALLS}" "pr create --repo test/fork --base main --head port/upstream-sync"
lacks "a green pull request is not a draft" "${CALLS}" "--draft"
contains "green labels" "${CALLS}" "--add-label upstream-port,port:green --remove-label port:red,port:partial"
pr_body=$(cat "${shim}/pr_body.md")
no_attribution "the pull request body" "${pr_body}"
contains "the agent's attribution is removed" "${pr_body}" "[removed: AI attribution]"
debt=$(cat "${shim}/body-3.md")
contains "the gap is parity debt, fenced" "${debt}" $'~~~text\n- adapters/bar/bar.go: video ~ ~ ~ ping @someone\n~~~'
cargo_runs=$(grep -c -E -e '/(plan|publish|publish-rerun)( |/)' "${E2E_CARGO_LOG}" || true)
go_runs=$(grep -c -E -e '/(plan|publish|publish-rerun)( |/)' "${E2E_GO_LOG}" || true)
check "the plan and publish jobs never run cargo or go" "0 0" "${cargo_runs} ${go_runs}"
ci
check "CI: guard on the port pull request" ok "${CI_GUARD}"
check "CI: the port pull request gets the bot rules" true "${C_PORT_PR}"
check "CI: the adapters the pin change touches" "bar,foo" "${CI_DIRS}"
check "CI: ci-ok" pass "${CI_RESULT}"

day idle
jobs_are ok skip skip
check "a green pull request with nothing new waits" none "${P_action}"
contains "reason" "${P_reason}" "up to date with upstream at ${u4:0:12}"

# main changes the line the port branch changed: the bot-only branch is rebuilt from main.
human_main_commit crates/x/src/lib.rs '//! x changed on main' "Rework crate x on main"
main_now=$(git -C "${origin}" rev-parse refs/heads/main)
day conflict
jobs_are ok ok ok
check "restart" true "${P_restart}"
contains "reason" "${P_reason}" "conflicts with main; rebuilding it from main"
check "work starts from main" "${main_now}" "${P_work_sha}"
check "the lease is the old branch" "${port1}" "${P_port_branch_sha}"
commits=$(plan_commits)
check "the chunk is ported again" "${u2} ${u3} ${u4}" "${commits}"
port2=$(branch_head)
rebuilt=yes
if git -C "${origin}" merge-base --is-ancestor "${port1}" "${port2}"; then
  rebuilt=no
fi
check "the branch is rebuilt (force-pushed)" yes "${rebuilt}"
contains "the same pull request is updated" "${CALLS}" "pr edit 101 --repo test/fork --title"
lacks "no second pull request" "${CALLS}" "pr create"
ci
check "CI: ci-ok" pass "${CI_RESULT}"

# A huge change: a fixture, two adapter changes, a docs commit whose subject carries Claude
# attribution, a heavy core commit and a commit with more than 1500 Go lines. Before it, a person
# changes main in a way that does not conflict.
t_put "${up}" adapters/foo/footest/exemplary/c.json '{"c": 1}'
u5=$(up_commit "foo: add a fixture")
t_put "${up}" adapters/bar/bar.go 'package bar // v2'
u6=$(up_commit "bar: tweak")
t_put "${up}" README.md 'readme v2'
u7=$(up_commit $'docs: readme \xf0\x9f\xa4\x96 Generated with [Claude Code]')
t_put "${up}" adapters/foo/foo.go 'package foo // v3'
u8=$(up_commit "foo: tweak")
t_put "${up}" exchange/exchange.go 'package exchange // v2'
u9=$(up_commit "exchange: core change")
mkdir -p "${up}/adapters/huge/hugetest/exemplary"
awk 'BEGIN { print "package huge"; for (i = 1; i <= 3200; i++) print "// line " i }' > "${up}/adapters/huge/huge.go"
t_put "${up}" adapters/huge/hugetest/exemplary/a.json '{"huge": 1}'
t_put "${up}" static/bidder-info/huge.yaml 'endpoint: "http://huge.example"'
u10=$(up_commit "New Adapter: Huge")
human_main_commit conformance/README.md 'the conformance contract, clarified' "Clarify the conformance contract"
main_now=$(git -C "${origin}" rev-parse refs/heads/main)

day chunk1
jobs_are ok ok ok
commits=$(plan_commits)
check "at most three commits" "${u5} ${u6} ${u7}" "${commits}"
check "work continues on the branch" "${port2}" "${P_work_sha}"
check "no restart" false "${P_restart}"
port3=$(branch_head)
merge_subject=$(git -C "${origin}" log -1 --format=%s "${port3}^")
check "main is merged into the branch first" "Merge main into port/upstream-sync" "${merge_subject}"
merged_main=$(git -C "${origin}" rev-parse "${port3}^^2")
check "the merge brings in main" "${main_now}" "${merged_main}"
messages=$(branch_messages "${port3}")
no_attribution "the branch's commits" "${messages}"
contains "the attribution in an upstream subject is removed" "${messages}" "upstream ${u7:0:12} [removed: AI attribution]"
pr_body=$(cat "${shim}/pr_body.md")
no_attribution "the pull request body" "${pr_body}"
ci
check "CI: a merge of main passes the bot rules" ok "${CI_GUARD}"
check "CI: ci-ok" pass "${CI_RESULT}"

day chunk2 review=blocking fix=port
jobs_are ok ok ok
commits=$(plan_commits)
check "the chunk stops before a heavy commit" "${u8}" "${commits}"
agents=$(cat "${D}/agents.txt")
contains "a blocking review finding runs the fix session" "${agents}" "fix port"
adversarial=$(git -C "${origin}" ls-tree -r --name-only refs/heads/port/upstream-sync -- crates/x/tests)
check "the adversarial test is kept" crates/x/tests/adversarial_foo.rs "${adversarial}"
ci
check "CI: ci-ok" pass "${CI_RESULT}"

day chunk3
jobs_are ok ok ok
commits=$(plan_commits)
check "a heavy core commit gets a chunk of its own" "${u9}" "${commits}"
check "budget" core "${P_budget_class}"
ci
check "CI: ci-ok" pass "${CI_RESULT}"

# The agent breaks clippy and the fix session does not repair it: the pull request turns red.
day chunk4 port=break fix=noop
jobs_are ok ok ok
commits=$(plan_commits)
check "a commit with too many Go lines gets a chunk of its own" "${u10}" "${commits}"
check "budget" core "${P_budget_class}"
check "final gates" fail "${FINAL_STATUS}"
contains "red labels" "${CALLS}" "--add-label upstream-port,port:red,port:partial --remove-label port:green"
contains "a red pull request goes back to draft" "${CALLS}" "pr ready 101 --repo test/fork --undo"
ci
check "CI: ci-ok" fail "${CI_RESULT}"

# Two repairs that change nothing: each is recorded as an empty port-repair commit, so the counter
# grows and the third day escalates.
for attempt in 1 2; do
  before=$(branch_head)
  day "repair${attempt}" port=noop fix=noop
  jobs_are ok ok ok
  check "action" repair "${P_action}"
  contains "reason" "${P_reason}" "repair ${attempt} of 2"
  check "a repair covers the pull request's commits" "${u1} ${u10}" "${P_base_sha} ${P_target_sha}"
  after=$(branch_head)
  subject=$(git -C "${origin}" log -1 --format=%s "${after}")
  check "the attempt is recorded" "port-repair: no change found for clippy" "${subject}"
  parent=$(git -C "${origin}" rev-parse "${after}^")
  check "one new commit" "${before}" "${parent}"
  changed=$(git -C "${origin}" diff-tree --no-commit-id --name-only -r "${after}")
  check "the commit is empty" "" "${changed}"
  ci
  check "CI: ci-ok" fail "${CI_RESULT}"
done

day escalate
jobs_are ok skip ok
check "action" escalate "${P_action}"
contains "reason" "${P_reason}" "after 2 repair attempt(s)"
contains "labelled for a person" "${CALLS}" "pr edit 101 --repo test/fork --add-label port:needs-human"
comment=$(cat "${shim}/body-1.md")
contains "the escalation comment" "${comment}" "Remove the port:needs-human label"

day waiting
jobs_are ok skip skip
check "a pull request waiting for a person is left alone" none "${P_action}"
contains "reason" "${P_reason}" "labelled port:needs-human"

# A person fixes the branch and hands it back.
human_fetch
git -C "${human}" switch -q -C fix origin/port/upstream-sync
perl -pi -e 's/ CLIPPY_FAILS//' "${human}/crates/x/src/lib.rs"
t_commit "${human}" "Fix clippy by hand" > /dev/null
git -C "${human}" push -q origin HEAD:refs/heads/port/upstream-sync
remove_label port:needs-human
ci
check "CI: a person's commit on the port pull request passes the guard" ok "${CI_GUARD}"
check "CI: ci-ok" pass "${CI_RESULT}"

day fixed
jobs_are ok skip skip
check "action" none "${P_action}"

# main conflicts with a branch that carries a person's commit: escalate instead of rebuilding.
human_main_commit crates/x/src/lib.rs '//! x changed on main again' "Rework crate x on main again"
day conflict-human
jobs_are ok skip ok
check "action" escalate "${P_action}"
contains "reason" "${P_reason}" "carries 1 human commit(s)"

# The person merges main into the branch, resolves the conflict and the pull request is merged.
human_fetch
git -C "${human}" switch -q fix
git -C "${human}" reset -q --hard origin/port/upstream-sync
merge_rc=0
git -C "${human}" merge -q --no-ff -m "Merge main into port/upstream-sync" origin/main > /dev/null 2>&1 || merge_rc=$?
check "the person's merge conflicts" 1 "${merge_rc}"
t_put "${human}" crates/x/src/lib.rs '//! x resolved by hand'
git -C "${human}" add crates/x/src/lib.rs
git -C "${human}" commit -q --no-edit
git -C "${human}" push -q origin HEAD:refs/heads/port/upstream-sync
remove_label port:needs-human
ci
check "CI: a person's merge of main passes the guard" ok "${CI_GUARD}"
check "CI: ci-ok" pass "${CI_RESULT}"
merge_pr
check "CI on main after merging the port pull request" ok "${CI_MAIN}"
main_pin=$(pin_of refs/heads/main)
check "main's pin" "${u10}" "${main_pin}"

parity_sync
check "a clean parity sync" "" "${SYNC_FAILED}"
contains "the merge is pushed" "${SYNC_LOG}" "merged main into parity-harness"
merged=no
if git -C "${origin}" merge-base --is-ancestor refs/heads/main refs/heads/parity-harness; then
  merged=yes
fi
check "the parity harness contains main" yes "${merged}"
harness_patch=$(git -C "${origin}" show refs/heads/parity-harness:parity/upstream-seams.patch)
contains "the seams patch survives" "${harness_patch}" "patched by the parity harness"
harness_pin=$(pin_of refs/heads/parity-harness)
check "the harness pins what main pins" "${u10}" "${harness_pin}"

# Agents that never report (an expired token, a refused actor): the gates stay green, but the
# commit is logged blocked, the pull request is labelled partial and the commit is parity debt.
t_put "${up}" adapters/foo/foo.go 'package foo // v4'
u11=$(up_commit "foo: another change")
printf '102\n' > "${shim}/create_number"
day silent port=fail
jobs_are ok ok ok
check "work starts from main again" false "${P_restart}"
contains "a new pull request" "${CALLS}" "pr create --repo test/fork"
contains "green but partial" "${CALLS}" "--add-label upstream-port,port:green,port:partial --remove-label port:red"
subject=$(git -C "${origin}" log -1 --format=%s refs/heads/port/upstream-sync)
check "the subject does not claim nothing was needed" \
  "port: advance upstream pin to ${u11:0:12} (no Rust changes; 1 commit(s) not ported)" "${subject}"
debt=$(cat "${shim}/body-3.md")
contains "the unreported commit is parity debt" "${debt}" \
  "- upstream ${u11:0:12} foo: another change: blocked (the agent reported no result for this commit)"
ci
check "CI: ci-ok" pass "${CI_RESULT}"

# dry-run ports but publishes nothing; pin-only advances the pin without agents and records the
# commit as parity debt, so the next run has nothing to port.
t_put "${up}" adapters/bar/bar.go 'package bar // v3'
u12=$(up_commit "bar: change")
before=$(branch_head)
day dryrun mode=dry-run
jobs_are ok ok skip
after=$(branch_head)
check "a dry run pushes nothing" "${before}" "${after}"
day pinonly mode=pin-only
jobs_are ok ok ok
check "action" advance "${P_action}"
check "no agent runs" false "${RUN_CLAUDE}"
pinonly_head=$(branch_head)
subject=$(git -C "${origin}" log -1 --format=%s "${pinonly_head}")
check "the pin-only commit" "port: advance upstream pin to ${u12:0:12} without porting (pin-only run)" "${subject}"
pinonly_pin=$(pin_of "${pinonly_head}")
check "the pin moves" "${u12}" "${pinonly_pin}"
port_log=$(git -C "${origin}" show "${pinonly_head}:upstream-port-log.jsonl")
last_status=$(jq -r -s 'last.status' <<< "${port_log}")
check "the log records the commit as pinned, not ported" pinned-not-ported "${last_status}"
contains "green but partial" "${CALLS}" "--add-label upstream-port,port:green,port:partial --remove-label port:red"
debt=$(cat "${shim}/body-3.md")
contains "the pinned commit is parity debt" "${debt}" "- upstream ${u12:0:12} bar: change: pinned-not-ported"
ci
check "CI: ci-ok" pass "${CI_RESULT}"
day afterpin
jobs_are ok skip skip
check "a pinned commit is not planned again" none "${P_action}"
contains "reason" "${P_reason}" "up to date with upstream at ${u12:0:12}"
merge_pr
check "CI on main" ok "${CI_MAIN}"

# A publish that fails after pushing; the next day finds the branch without a pull request,
# rebuilds it and force-pushes with a lease; that publish fails too, and re-running the failed job
# finds the push already done.
t_put "${up}" main.go 'package main // v2'
u13=$(up_commit "main: change the server")
printf '103\n' > "${shim}/create_number"
touch "${shim}/fail_pr_create"
day half
jobs_are ok ok fail
half_head=$(branch_head)
half_pin=""
if [[ -n "${half_head}" ]]; then
  half_pin=$(pin_of "${half_head}")
fi
check "the branch was pushed" "${u13}" "${half_pin}"
prs=$(cat "${shim}/prs.json")
check "no pull request" "[]" "${prs}"
touch "${shim}/fail_pr_create"
day restart
jobs_are ok ok fail
check "restart" true "${P_restart}"
contains "reason" "${P_reason}" "exists without an open pull request"
check "the lease" "${half_head}" "${P_port_branch_sha}"
commits=$(plan_commits)
check "the same commit is ported again" "${u13}" "${commits}"
restart_head=$(branch_head)
rebuilt=yes
if git -C "${origin}" merge-base --is-ancestor "${half_head}" "${restart_head}"; then
  rebuilt=no
fi
check "the branch is rebuilt" yes "${rebuilt}"
: > "${shim}/calls.log"
publish_job publish-rerun
check "re-running the failed publish job" ok "${R_publish}"
contains "the push is already done" "${PUSH_LOG}" "Everything up-to-date"
calls=$(cat "${shim}/calls.log")
contains "the pull request is created" "${calls}" "pr create --repo test/fork"
pr_number=$(jq -r '.[0].number' "${shim}/prs.json")
check "pull request number" 103 "${pr_number}"
ci
check "CI: ci-ok" pass "${CI_RESULT}"

# A failed port job pushes nothing and lands on the run-failed issue; the next day recovers.
t_put "${up}" adapters/foo/footest/exemplary/d.json '{"d": 1}'
u14=$(up_commit "foo: add another fixture")
before=$(branch_head)
touch "${E2E_FAIL_FETCH}"
day portfail
rm -f -- "${E2E_FAIL_FETCH}"
jobs_are ok fail ok
check "the prefetch fails" "Prefetch dependencies (the last network use before the agents)" "${PORT_FAILED}"
contains "the failure is reported" "${CALLS}" "issue create --repo test/fork --title Upstream port run failed --label port:run-failed"
after=$(branch_head)
check "nothing is pushed" "${before}" "${after}"
day recover
jobs_are ok ok ok
commits=$(plan_commits)
check "the same commit is planned again" "${u14}" "${commits}"
check "a fixture needs no agent" false "${RUN_CLAUDE}"
subject=$(git -C "${origin}" log -1 --format=%s refs/heads/port/upstream-sync)
check "the pin advances" "port: advance upstream pin to ${u14:0:12} (no Rust changes needed)" "${subject}"
ci
check "CI: ci-ok" pass "${CI_RESULT}"
merge_pr
check "CI on main" ok "${CI_MAIN}"

# main now pins u14, whose main.go (changed by u13) the seams patch no longer fits: the sync
# merges cleanly but pushes nothing, fails and opens the seams issue.
DAY=parity-stale
harness_before=$(git -C "${origin}" rev-parse refs/heads/parity-harness)
parity_sync
check "a stale seams patch fails the sync" "Merge, check the seams patch against the new pin, and push (or report)" \
  "${SYNC_FAILED}"
harness_after=$(git -C "${origin}" rev-parse refs/heads/parity-harness)
check "nothing is pushed" "${harness_before}" "${harness_after}"
contains "the seams issue" "${CALLS}" \
  "issue create --repo test/fork --title parity-harness: the seams patch does not apply --label parity-harness"
issue=$(cat "${shim}/body-1.md")
contains "the issue names the pin" "${issue}" "does not apply to upstream ${u14:0:12}"
contains "the issue names the patched file" "${issue}" "main.go"

# A person merges main into the harness and rebases the seams on the new pin; the next sync finds
# nothing to merge, checks the seams and closes the issue.
DAY=parity-fixed
human_fetch
git -C "${human}" switch -q -C parity-harness origin/parity-harness
git -C "${human}" merge -q --no-ff -m "Merge main into parity-harness" origin/main
seams 'package main // v2' > "${human}/parity/upstream-seams.patch"
t_commit "${human}" "Parity harness: rebase the seams on ${u14:0:12}" > /dev/null
git -C "${human}" push -q origin parity-harness
harness_before=$(git -C "${origin}" rev-parse refs/heads/parity-harness)
parity_sync
check "the fixed harness passes" "" "${SYNC_FAILED}"
contains "nothing to merge, the seams fit" "${SYNC_LOG}" "nothing to push; parity-harness fits upstream ${u14:0:12}"
contains "the seams issue is closed" "${CALLS}" "issue close 201"
harness_after=$(git -C "${origin}" rev-parse refs/heads/parity-harness)
check "nothing is pushed" "${harness_before}" "${harness_after}"

# A person changes AGENTS.md on main, which the harness changed too: the sync conflicts, pushes
# nothing and opens the conflict issue.
DAY=parity-conflict
human_main_commit AGENTS.md 'fork agents v2' "Update the agent notes"
harness_before=$(git -C "${origin}" rev-parse refs/heads/parity-harness)
parity_sync
check "a conflicting parity sync still succeeds" "" "${SYNC_FAILED}"
harness_after=$(git -C "${origin}" rev-parse refs/heads/parity-harness)
check "the conflict pushes nothing" "${harness_before}" "${harness_after}"
contains "the conflict issue" "${CALLS}" "issue create --repo test/fork --title parity-harness: merging main conflicts"
issue=$(cat "${shim}/body-1.md")
contains "the issue names the path" "${issue}" "AGENTS.md"

# No upstream commit, tree or blob ever entered the repository, except the license text, which the
# port ships as an allowed copy.
DAY=end
upstream_objects=$(git -C "${up}" rev-list --objects --all)
license_blob=$(git -C "${up}" rev-parse "${u1}:LICENSE")
ids=$(cut -d ' ' -f 1 <<< "${upstream_objects}" | grep -v -x -F -e "${license_blob}")
present=$(git -C "${origin}" cat-file --batch-check <<< "${ids}" | grep -c -v ' missing$' || true)
check "no upstream object is in the repository" 0 "${present}"

t_done
