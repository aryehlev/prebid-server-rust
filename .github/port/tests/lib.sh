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
# The upstream trees the tests check out are made read-only, so they are made writable before the
# directory is removed.
trap 'chmod -R u+w -- "${T_ROOT}" 2> /dev/null || true; rm -rf -- "${T_ROOT}"' EXIT
# A test runs from the temporary directory, which is no git work tree (git does not look above it),
# so a script a test starts without naming a repository fails instead of changing this one.
cd "${T_ROOT}"
GIT_CEILING_DIRECTORIES=$(dirname "${T_ROOT}")
export GIT_CEILING_DIRECTORIES

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
  CTX PLAN_DIR OUT_DIR START_SHA BASE_SHA TARGET_SHA WORK_SHA ACTION RUN_MODE ADAPTER_DIRS PR_NUMBER \
  NEEDS_CLAUDE BUDGET_CLASS RUN_URL PORT_JSON FIX_JSON REVIEW_JSON AFTER_STATUS FINAL_STATUS \
  CLAUDE_RAN CONFORMANCE_REQUIRED PBS_GO_ROOT TARGET_INPUT WORK_BRANCH DRY_RUN REASON PORT_RESULT \
  EVENT REF REPOSITORY PR_BASE PR_HEAD PR_HEAD_REF PR_HEAD_REPO BASE_REF PUSH_BEFORE MG_BASE \
  MG_HEAD HEAVY_GO_LINES MAX_COMMITS MAX_PR_COMMITS MAX_REPAIRS MAX_GO_LINES GH_SHIM_DIR \
  UPSTREAM_GIT PBS_UPSTREAM_URL PBS_UPSTREAM_REFERENCE PIN_VERSION OLD_GO_ROOT PROBE_ROOT \
  CARGO_TARGET_DIR GOFLAGS GOWORK
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

# t_real_upstream: the git directory holding the real upstream history that test_classify and
# test_guard read: PORT_TESTS_UPSTREAM_GIT, which CI sets to the upstream mirror (locally: a mirror
# made with .github/port/scripts/upstream-mirror.sh, or any clone of prebid/prebid-server). Prints
# nothing when it is unset or not a git directory.
t_real_upstream() {
  local dir=${PORT_TESTS_UPSTREAM_GIT:-}
  if [[ -n "${dir}" ]] && git --git-dir="${dir}" rev-parse --git-dir > /dev/null 2>&1; then
    printf '%s\n' "${dir}"
  fi
}

# t_go_mod DIR VERSION: the go.mod of a Go tooling module at DIR (relative to the repository top)
# that requires the upstream module at VERSION and replaces it with .upstream at the top.
t_go_mod() {
  local dir=${1:?} version=${2:?} up="" depth
  depth=$(tr -cd '/' <<< "${dir}" | wc -c)
  depth=$((depth + 1))
  while [[ "${depth}" -gt 0 ]]; do
    up+="../"
    depth=$((depth - 1))
  done
  printf 'module github.com/aryehlev/prebid-server-rust/%s\n\ngo 1.25.0\n\nrequire github.com/prebid/prebid-server/v4 %s\n\nreplace github.com/prebid/prebid-server/v4 => %s.upstream\n' \
    "${dir}" "${version}" "${up}"
}

# t_mirror DIR: make or refresh DIR/upstream.git, the bare mirror of DIR/upstream that
# upstream-mirror.sh keeps, and export UPSTREAM_GIT and PBS_UPSTREAM_URL for it.
t_mirror() {
  local dir=${1:?}
  export PBS_UPSTREAM_URL="${dir}/upstream"
  "${T_SCRIPTS}/upstream-mirror.sh" "${dir}/upstream.git" > /dev/null 2>&1
  export UPSTREAM_GIT="${dir}/upstream.git"
}

# t_fork_fixture DIR: a fake upstream repository DIR/upstream, its mirror DIR/upstream.git and a
# repository DIR/fork in this repository's layout, which holds no upstream file.
#   upstream (branch master; tags u1 and v4.0.0 on its first commit): a go.mod of
#     github.com/prebid/prebid-server/v4, a Dockerfile, Go code, an adapter with fixtures, bidder
#     info, docs, upstream workflows, an AGENTS.md and a LICENSE
#   upstream.git: the mirror (t_mirror; UPSTREAM_GIT and PBS_UPSTREAM_URL are exported)
#   fork (branch main, tag f0): .github/port, UPSTREAM_SYNC pinning u1, Cargo.toml, Cargo.lock, a
#     crate with a Go tooling module that requires the upstream module at u1's pseudo-version,
#     testdata/, the known-failures list, scripts/ (with a copy of scripts/fetch-upstream.sh),
#     patches/, the port log, its own AGENTS.md and upstream's LICENSE (an allowed copy).
#     refs/remotes/origin/main is main.
# Sets T_U1 and T_U1_VERSION (u1's pseudo-version).
t_fork_fixture() {
  local dir=${1:?} up fork
  up="${dir}/upstream"
  fork="${dir}/fork"
  t_repo "${up}"
  printf 'module github.com/prebid/prebid-server/v4\n\ngo 1.25.0\n' > "${up}/go.mod"
  t_put "${up}" Dockerfile 'RUN wget https://dl.google.com/go/go1.26.2.linux-amd64.tar.gz'
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
  t_put "${up}" LICENSE 'Apache License, Version 2.0 (test copy)'
  T_U1=$(t_commit "${up}" "Initial upstream")
  git -C "${up}" tag u1
  git -C "${up}" tag v4.0.0
  t_mirror "${dir}"
  T_U1_VERSION=$("${T_SCRIPTS}/pin-version.sh" "${T_U1}")
  export T_U1 T_U1_VERSION

  t_repo "${fork}"
  git -C "${fork}" switch -q -c main
  t_put "${fork}" .github/port/README.md 'fork tooling'
  printf '# The upstream commit the port is pinned to.\n# Only the upstream-port workflow advances it.\n%s\n' "${T_U1}" \
    > "${fork}/UPSTREAM_SYNC"
  printf '/target/\n/.upstream/\n/.upstream\n' > "${fork}/.gitignore"
  t_put "${fork}" Cargo.toml '[workspace]'
  printf 'version = 4\n\n[[package]]\nname = "pbs-x"\nversion = "0.1.0"\n' > "${fork}/Cargo.lock"
  t_put "${fork}" crates/x/src/lib.rs '//! x'
  mkdir -p "${fork}/crates/x/testdata/go"
  t_go_mod crates/x/testdata/go "${T_U1_VERSION}" > "${fork}/crates/x/testdata/go/go.mod"
  t_put "${fork}" crates/x/testdata/go/main.go 'package main'
  t_put "${fork}" testdata/README.md 'generated data'
  t_put "${fork}" conformance/known_failures.txt '# known failures'
  t_put "${fork}" conformance/README.md 'the conformance contract'
  t_put "${fork}" scripts/a.sh 'echo a'
  cp "${T_TOP}/scripts/fetch-upstream.sh" "${fork}/scripts/fetch-upstream.sh"
  t_put "${fork}" patches/README.md 'patched crates'
  : > "${fork}/upstream-port-log.jsonl"
  t_put "${fork}" AGENTS.md 'fork agents'
  cp "${up}/LICENSE" "${fork}/LICENSE"
  t_commit "${fork}" "Fork setup" > /dev/null
  git -C "${fork}" tag f0
  git -C "${fork}" update-ref refs/remotes/origin/main refs/heads/main
}

# t_advance REPO SHA: pin REPO to upstream commit SHA with advance-pin.sh (UPSTREAM_SYNC and the
# Go tooling modules), at the version pin-version.sh derives from the mirror.
t_advance() {
  local repo=${1:?} sha=${2:?} version
  version=$("${T_SCRIPTS}/pin-version.sh" "${sha}")
  (cd "${repo}" && "${T_SCRIPTS}/advance-pin.sh" "${sha}" "${version}") > /dev/null
}

# t_sync_upstream DIR: bring DIR/upstream.git up to date with DIR/upstream (its master and tags).
t_sync_upstream() {
  t_mirror "${1:?}"
}
