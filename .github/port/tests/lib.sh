#!/usr/bin/env bash
# lib.sh: helpers for the port tooling tests (.github/port/tests/test_*.sh, run by run-all.sh).
#
# Sourcing it creates a temporary directory (removed on exit), isolates git from the user's and
# the system's configuration, and resets every environment variable the port scripts read, so a
# test sees the same environment locally and in CI. Assertions count instead of exiting; t_done
# prints the totals and fails the test when any assertion failed.
set -euo pipefail
shopt -s inherit_errexit
export LC_ALL=C

T_TESTS=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
T_PORT=$(cd "${T_TESTS}/.." && pwd)
T_SCRIPTS="${T_PORT}/scripts"
T_TOP=$(cd "${T_PORT}/../.." && pwd)
T_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/port-tests.XXXXXX")
T_ROOT=$(cd "${T_ROOT}" && pwd -P)
export T_TESTS T_PORT T_SCRIPTS T_TOP T_ROOT
export T_RC=0 T_OUT="" T_ERR=""
T_PASSED=0
T_FAILED=0
T_NAME=$(basename "${0}" .sh)
trap 'rm -rf -- "${T_ROOT}"' EXIT

# The scripts run with the bash running the tests (bash 4.4 or newer), whatever bash PATH has.
bash_dir=$(dirname "${BASH}")
mkdir -p "${T_ROOT}/bin"
export PATH="${T_ROOT}/bin:${bash_dir}:${PATH}"

# Without a global configuration git still reads ${XDG_CONFIG_HOME:-~/.config}/git/ignore and
# attributes, which can hide files such as .claude/settings.local.json from `git add`.
mkdir -p "${T_ROOT}/xdg"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0 XDG_CONFIG_HOME="${T_ROOT}/xdg"
export GIT_AUTHOR_NAME="Test Person" GIT_AUTHOR_EMAIL="person@example.com"
export GIT_COMMITTER_NAME="Test Person" GIT_COMMITTER_EMAIL="person@example.com"
unset GITHUB_OUTPUT GITHUB_ENV GITHUB_STEP_SUMMARY GITHUB_WORKSPACE RUNNER_TEMP GH_TOKEN \
  CTX PLAN_DIR OUT_DIR MERGED_SHA BASE_SHA TARGET_SHA WORK_SHA ACTION RUN_MODE ADAPTER_DIRS PR_NUMBER \
  NEEDS_CLAUDE BUDGET_CLASS RUN_URL PORT_JSON FIX_JSON REVIEW_JSON AFTER_STATUS FINAL_STATUS \
  CLAUDE_RAN CONFORMANCE_REQUIRED PBS_GO_ROOT TARGET_INPUT WORK_BRANCH DRY_RUN REASON PORT_RESULT \
  EVENT REF REPOSITORY PR_BASE PR_HEAD PR_HEAD_REF PR_HEAD_REPO BASE_REF PUSH_BEFORE MG_BASE \
  MG_HEAD HEAVY_GO_LINES MAX_COMMITS MAX_PR_COMMITS MAX_REPAIRS MAX_GO_LINES GH_SHIM_DIR
export UPSTREAM_BRANCH=master BASE_BRANCH=main PORT_BRANCH=port/upstream-sync HARNESS_BRANCH=parity-harness
export GITHUB_REPOSITORY=test/fork
export T_BOT_NAME="github-actions[bot]" T_BOT_EMAIL="41898282+github-actions[bot]@users.noreply.github.com"
export PORT_GIT_EMAIL="${T_BOT_EMAIL}"

t_ok() {
  T_PASSED=$((T_PASSED + 1))
  printf 'ok   %s\n' "$*"
}

t_fail() {
  T_FAILED=$((T_FAILED + 1))
  printf 'FAIL %s\n' "$*"
}

# t_eq NAME EXPECTED ACTUAL
t_eq() {
  if [[ "${2}" == "${3}" ]]; then
    t_ok "${1}"
  else
    t_fail "${1}: expected [${2}], got [${3}]"
  fi
}

# t_has NAME TEXT NEEDLE: TEXT contains NEEDLE.
t_has() {
  if [[ "${2}" == *"${3}"* ]]; then
    t_ok "${1}"
  else
    t_fail "${1}: [${3}] not found in: ${2:0:3000}"
  fi
}

# t_lacks NAME TEXT NEEDLE: TEXT does not contain NEEDLE.
t_lacks() {
  if [[ "${2}" != *"${3}"* ]]; then
    t_ok "${1}"
  else
    t_fail "${1}: [${3}] unexpectedly found in: ${2:0:3000}"
  fi
}

# t_fails NAME: the last t_run_in command failed (any non-zero status).
t_fails() {
  if [[ "${T_RC}" -ne 0 ]]; then
    t_ok "${1}"
  else
    t_fail "${1}: expected a failure, got exit status 0"
  fi
}

# t_done: print the totals; exit 1 when an assertion failed.
t_done() {
  printf '%s: %s passed, %s failed\n' "${T_NAME}" "${T_PASSED}" "${T_FAILED}"
  if [[ "${T_FAILED}" -gt 0 || "${T_PASSED}" -eq 0 ]]; then
    exit 1
  fi
}

# t_run_in DIR COMMAND...: run COMMAND in DIR; its status, stdout and stderr land in T_RC, T_OUT
# and T_ERR.
t_run_in() {
  local dir=${1:?}
  shift
  T_RC=0
  (cd "${dir}" && "$@") > "${T_ROOT}/stdout" 2> "${T_ROOT}/stderr" || T_RC=$?
  T_OUT=$(cat "${T_ROOT}/stdout")
  T_ERR=$(cat "${T_ROOT}/stderr")
}

# t_put REPO PATH [CONTENT]: write PATH (one line of CONTENT) in REPO, creating directories.
t_put() {
  local repo=${1:?} path=${2:?} content=${3-} dir
  dir=$(dirname "${repo}/${path}")
  mkdir -p "${dir}"
  printf '%s\n' "${content}" > "${repo}/${path}"
}

# t_repo DIR: a new repository on branch master.
t_repo() {
  git init -q -b master "${1:?}"
}

# t_commit REPO MESSAGE: commit everything as the test person; print the commit.
t_commit() {
  local repo=${1:?} message=${2:?}
  git -C "${repo}" add -A
  git -C "${repo}" commit -q --allow-empty -m "${message}"
  git -C "${repo}" rev-parse HEAD
}

# t_commit_as REPO NAME EMAIL MESSAGE: commit everything as NAME <EMAIL> (author and committer).
t_commit_as() {
  local repo=${1:?} name=${2:?} email=${3:?} message=${4:?}
  git -C "${repo}" add -A
  env GIT_AUTHOR_NAME="${name}" GIT_AUTHOR_EMAIL="${email}" GIT_COMMITTER_NAME="${name}" \
    GIT_COMMITTER_EMAIL="${email}" git -C "${repo}" commit -q --allow-empty -m "${message}"
  git -C "${repo}" rev-parse HEAD
}

# t_output NAME FILE: the value of step output NAME in the GITHUB_OUTPUT file FILE (the last one
# wins, like GitHub's; a multi-line value loses the newline before its delimiter line).
t_output() {
  perl -e '
    my ($name, $file) = @ARGV;
    open(my $fh, "<", $file) or die "$file: $!";
    my ($value, $current, $delim, $buffer);
    while (my $line = <$fh>) {
      if (defined $delim) {
        if ($line eq "$delim\n") {
          if ($current eq $name) { $buffer =~ s/\n\z//; $value = $buffer; }
          undef $delim;
        } else {
          $buffer .= $line;
        }
        next;
      }
      if ($line =~ /^([^=<]+)<<(.+)\n\z/) { ($current, $delim, $buffer) = ($1, $2, ""); next; }
      if ($line =~ /^([^=]+)=(.*)\n?\z/ && $1 eq $name) { $value = $2; }
    }
    print defined $value ? $value : "";
  ' "${1:?}" "${2:?}"
}

# t_gh_shim DIR: use gh-shim.sh as gh, with its state and call log in DIR.
t_gh_shim() {
  local dir=${1:?}
  mkdir -p "${dir}"
  : > "${dir}/calls.log"
  ln -sf "${T_TESTS}/gh-shim.sh" "${T_ROOT}/bin/gh"
  export GH_SHIM_DIR="${dir}" GH_TOKEN=test-token
}

# t_fork_fixture DIR: a fake upstream repository DIR/upstream and a fork DIR/fork of it.
#   upstream (branch master, tag u1): Go code, an adapter with fixtures, bidder info, docs,
#     upstream workflows and an AGENTS.md
#   fork (branch main, tag f0): u1 without upstream's .github/workflows, plus the fork-owned
#     paths: .github/port, rust/ (UPSTREAM_SYNC = u1, Cargo.toml, Cargo.lock, a crate, the
#     known-failures list, scripts) and its own AGENTS.md. refs/remotes/upstream/master is u1 and
#     refs/remotes/origin/main is main.
t_fork_fixture() {
  local dir=${1:?} up fork u1
  up="${dir}/upstream"
  fork="${dir}/fork"
  t_repo "${up}"
  t_put "${up}" main.go 'package main // v1'
  t_put "${up}" exchange/exchange.go 'package exchange // v1'
  t_put "${up}" adapters/foo/foo.go 'package foo // v1'
  t_put "${up}" adapters/foo/footest/exemplary/a.json '{"a": 1}'
  t_put "${up}" adapters/foo/footest/exemplary/b.json '{"b": 1}'
  t_put "${up}" static/bidder-info/foo.yaml 'endpoint: "http://foo.example"'
  t_put "${up}" "docs/a file.md" 'docs v1'
  t_put "${up}" .github/workflows/upstream.yml 'name: upstream'
  t_put "${up}" AGENTS.md 'upstream agents v1'
  t_put "${up}" README.md 'readme v1'
  u1=$(t_commit "${up}" "Initial upstream")
  git -C "${up}" tag u1

  git clone -q "${up}" "${fork}"
  git -C "${fork}" switch -q -c main
  git -C "${fork}" branch -q -D master
  git -C "${fork}" remote remove origin
  git -C "${fork}" rm -q -r .github/workflows
  t_put "${fork}" .github/port/README.md 'fork tooling'
  mkdir -p "${fork}/rust"
  printf '# Last upstream commit ported to Rust.\n# One commit per line; the last one counts.\n%s\n' "${u1}" \
    > "${fork}/rust/UPSTREAM_SYNC"
  t_put "${fork}" rust/Cargo.toml '[workspace]'
  printf 'version = 4\n\n[[package]]\nname = "pbs-x"\nversion = "0.1.0"\n' > "${fork}/rust/Cargo.lock"
  t_put "${fork}" rust/crates/x/src/lib.rs '//! x'
  t_put "${fork}" rust/conformance/known_failures.txt '# known failures'
  t_put "${fork}" rust/scripts/a.sh 'echo a'
  : > "${fork}/rust/upstream-port-log.jsonl"
  t_put "${fork}" AGENTS.md 'fork agents'
  t_commit "${fork}" "Fork setup" > /dev/null
  git -C "${fork}" tag f0
  git -C "${fork}" fetch -q "${up}" "+refs/heads/master:refs/remotes/upstream/master"
  git -C "${fork}" update-ref refs/remotes/origin/main refs/heads/main
}

# t_sync_upstream DIR: fetch DIR/upstream's master and tags into DIR/fork.
t_sync_upstream() {
  local dir=${1:?}
  git -C "${dir}/fork" fetch -q "${dir}/upstream" "+refs/heads/master:refs/remotes/upstream/master" \
    "+refs/tags/*:refs/tags/*"
}
