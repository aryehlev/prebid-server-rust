#!/usr/bin/env bash
# lib.sh: helpers shared by the upstream-port and CI scripts. Sourcing it sets the strict shell
# options every caller uses (errexit, nounset, pipefail, inherit_errexit) and defines names.
#
# Values are returned on stdout (one item per line) so callers can capture them with $(...) and
# `mapfile`, never through globals or exit codes of functions used in conditions.
#
# The repository holds only the Rust port and its tooling. The upstream Go server is never in it:
# scripts/fetch-upstream.sh checks the pinned commit out into .upstream/ (git ignores it), and the
# scripts that need upstream history read a separate bare mirror (upstream-mirror.sh) through
# port_up. Upstream objects never enter this repository's object store.
set -euo pipefail
shopt -s inherit_errexit
# Byte-order sorting and byte-wise matching everywhere: the known-failures list is sorted in
# LC_ALL=C order, and patterns must not depend on the runner's locale. It is exported once here
# rather than prefixed to single commands because bash 5.3 can crash on a temporary LC_ALL=...
# assignment inside a command substitution.
export LC_ALL=C

PORT_SCRIPTS=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PORT_DIR=$(cd "${PORT_SCRIPTS}/.." && pwd)
# The top of the tree these scripts came from: the repository, or the trusted tooling the plan
# job packs (.github/port and scripts/fetch-upstream.sh of the default branch, in the same layout).
PORT_TOP=$(cd "${PORT_DIR}/../.." && pwd)

# port_fetch_upstream ARG...: run scripts/fetch-upstream.sh of that tree.
port_fetch_upstream() {
  "${PORT_TOP}/scripts/fetch-upstream.sh" "$@"
}

# port_go_module: the module path of the upstream Go server, which the Go tooling modules require.
port_go_module() {
  printf '%s\n' github.com/prebid/prebid-server/v4
}

# port_top_level_paths: the only top-level paths a branch may hold (guard.sh's upstream-files
# rule). The parity-harness branch also holds parity/. Anything new at the top needs this list
# changed by a person.
port_top_level_paths() {
  printf '%s\n' \
    .github \
    .gitignore \
    .planning \
    AGENTS.md \
    Cargo.lock \
    Cargo.toml \
    LICENSE \
    LICENSES \
    NOTICE \
    PORTING.md \
    README.md \
    UPSTREAM_SYNC \
    clippy.toml \
    conformance \
    crates \
    deny.toml \
    patches \
    rust-toolchain.toml \
    rustfmt.toml \
    scripts \
    taplo.toml \
    testdata \
    upstream-port-log.jsonl
}

# port_writable_paths: what the port agent sessions and the bot may change, one per line: crate
# sources, tests, Go tooling modules and generated data, the goldens under testdata/, the
# known-failures list (entries may only be removed) and Cargo.lock (a crate may start using a
# crate the workspace already locks). check-worktree.sh reverts everything else after a session.
port_writable_paths() {
  printf '%s\n' \
    crates \
    testdata \
    conformance/known_failures.txt \
    Cargo.lock
}

# port_record_paths: the port records, one per line: the upstream pin and the port log. Only the
# trusted scripts write them (advance-pin.sh and commit.sh); check-worktree.sh reverts any change
# a session makes to them, and guard.sh's bot-log rule checks that the log only grows.
port_record_paths() {
  printf '%s\n' \
    UPSTREAM_SYNC \
    upstream-port-log.jsonl
}

# port_protected_paths: the paths, one per line, that the port agents and the bot may never
# change, and that no commit on the port branch may change (guard.sh's bot-protected rule). They
# set the rules a port is checked against: the workspace and its lint policy, the toolchains, the
# formatter, clippy, cargo, dependency and suppression checks, the patched crates, what git
# ignores (the upstream checkout), the scripts, the conformance contract, the porting rules, and
# the license with the third-party license texts (LICENSES/). Tool configuration files that do not
# exist yet are listed too, so that an agent cannot add one: a clippy.toml raising thresholds, a
# .cargo/config.toml changing flags or sources, or a legacy rust-toolchain file, which rustup
# prefers to rust-toolchain.toml.
port_protected_paths() {
  printf '%s\n' \
    Cargo.toml \
    rust-toolchain.toml \
    rust-toolchain \
    rustfmt.toml \
    .rustfmt.toml \
    clippy.toml \
    .clippy.toml \
    .cargo \
    deny.toml \
    taplo.toml \
    .gitignore \
    patches \
    scripts \
    PORTING.md \
    README.md \
    LICENSE \
    LICENSES \
    NOTICE \
    conformance/README.md
}

# port_agent_config_paths: repository-root paths that configure, or can run code in, a coding agent
# (Claude Code settings, hooks and MCP servers) or a tool it drives (ripgrep). They are never
# committed (guard.sh's local-state rule), and the agent sessions load only the settings the
# workflow writes (render-inputs.sh), so a copy in a work tree changes nothing. .omx is local tool
# state and is listed here too.
port_agent_config_paths() {
  printf '%s\n' \
    .claude \
    .claude.json \
    .claude.local.json \
    .mcp.json \
    .ripgreprc \
    .husky \
    .omx
}

# port_upstream_copies: the files that may be byte-identical to an upstream file, one
# "PATH<TAB>UPSTREAM_PATH<TAB>REASON" line each. PATH may hold only bytes that UPSTREAM_PATH held
# at some upstream commit: the exception covers that one file, not whatever is put at PATH. Every
# other tracked file that equals a blob of upstream history fails guard.sh's upstream-files rule.
# An empty file is always allowed: it holds no content.
port_upstream_copies() {
  printf '%s\t%s\t%s\n' \
    LICENSE LICENSE "the Apache License 2.0 text, which the port ships under the same license as upstream" \
    crates/util/src/httputil/pixel.png util/httputil/pixel.png "the 86-byte transparent pixel Go serves from util/httputil; the port serves the same bytes"
}

# port_go_dir_re [parity]: an extended regular expression for the paths Go code may live at: the
# Go tooling modules (crates/<crate>/testdata/go and crates/devtools/go), plus parity/ on the
# parity-harness branch.
port_go_dir_re() {
  local re='^(crates/[^/]+/testdata/go|crates/devtools/go)/'
  if [[ "${1:-}" == parity ]]; then
    re='^(crates/[^/]+/testdata/go|crates/devtools/go|parity)/'
  fi
  printf '%s\n' "${re}"
}

port_error() {
  printf '::error::%s\n' "$*" >&2
}

port_warning() {
  printf '::warning::%s\n' "$*" >&2
}

port_die() {
  port_error "$@"
  exit 1
}

# port_cd_top: change to the top of the current git work tree.
port_cd_top() {
  local top
  top=$(git rev-parse --show-toplevel)
  cd "${top}" || exit 1
}

# port_up ARG...: git in the bare upstream mirror ${UPSTREAM_GIT} (upstream-mirror.sh makes it).
port_up() {
  git --git-dir="${UPSTREAM_GIT:?UPSTREAM_GIT must name the upstream mirror (upstream-mirror.sh)}" "$@"
}

# port_upstream_ref / port_main_ref: the upstream branch in the mirror, and the remote-tracking
# ref of the default branch in this repository.
port_upstream_ref() {
  printf 'refs/heads/%s\n' "${UPSTREAM_BRANCH:-master}"
}

port_main_ref() {
  printf 'refs/remotes/origin/%s\n' "${BASE_BRANCH:-main}"
}

# port_list [FILE]: the entries of a list file (or stdin), without comments, blank lines or
# surrounding space.
port_list() {
  sed -E -e 's/[[:space:]]*#.*$//' -e 's/^[[:space:]]+//' -e 's/[[:space:]]+$//' -e '/^$/d' "$@"
}

# port_pathspecs PATH...: a git pathspec for each top-relative PATH.
port_pathspecs() {
  local path
  for path in "$@"; do
    printf ':(top)%s\n' "${path}"
  done
}

# port_outside_writable_spec: git pathspecs, one per line, that select everything outside
# port_writable_paths.
port_outside_writable_spec() {
  local listing path
  listing=$(port_writable_paths)
  printf '%s\n' ':(top)'
  while IFS= read -r path; do
    printf ':(top,exclude)%s\n' "${path}"
  done <<< "${listing}"
}

# port_pin_parse: read UPSTREAM_SYNC text on stdin and print the pinned commit, the way
# scripts/fetch-upstream.sh reads it: exactly one line that is neither blank nor a # comment,
# holding a full 40-character commit. Prints nothing for any other text.
port_pin_parse() {
  local pins
  pins=$(sed -E -e 's/[[:space:]]+$//' -e '/^[[:space:]]*(#|$)/d')
  if [[ -z "${pins}" || "${pins}" == *$'\n'* ]]; then
    return 0
  fi
  pins=${pins#"${pins%%[![:space:]]*}"}
  if [[ "${pins}" =~ ^[0-9a-f]{40}$ ]]; then
    printf '%s\n' "${pins}"
  fi
}

# port_sync_text SHA: read UPSTREAM_SYNC text on stdin and print it pinned to SHA: its # comment
# lines, then SHA. advance-pin.sh writes this; check-worktree.sh expects it.
port_sync_text() {
  local header
  header=$(sed -n '/^[[:space:]]*#/p')
  printf '%s\n%s\n' "${header}" "${1:?}" | sed '/^$/d'
}

# port_marker REV: the upstream commit UPSTREAM_SYNC pins at REV, or nothing when the file is
# missing or malformed.
port_marker() {
  local text
  text=$(git show "${1:?}:UPSTREAM_SYNC" 2> /dev/null) || text=""
  port_pin_parse <<< "${text}"
}

# port_attribution_re: an extended regular expression (match it case-insensitively) for AI
# attribution in commit messages and pull request text: Co-Authored-By trailers naming
# Claude or Anthropic, Anthropic's noreply address, "Generated with [Claude Code]" footers and the
# robot emoji they carry, Claude session links and Claude-Session trailers. Plain mentions of a
# generator ("Generated with `cargo run -p pbs-devtools -- all`") do not match.
port_attribution_re() {
  local robot=$'\xf0\x9f\xa4\x96'
  printf '%s\n' "co-authored-by:.*(claude|anthropic)|noreply@anthropic\\.com|generated (with|by) \\[?claude|${robot}|claude\\.ai/(code|chat|share)|claude\\.com/claude-code|^[[:space:]]*claude-session:"
}

# port_ai_name_re / port_ai_email_re: commit identities that belong to an AI agent.
port_ai_name_re() {
  printf '%s\n' '^claude( code)?(\[bot\])?$'
}

port_ai_email_re() {
  printf '%s\n' 'anthropic\.com|claude\[bot\]'
}

# port_ci_skip_re: an extended regular expression (matched case-insensitively) for the markers that
# make GitHub run no workflow for a push or a pull request whose head commit message carries one:
# [skip ci], [ci skip], [no ci], [skip actions], [actions skip] and a skip-checks trailer.
port_ci_skip_re() {
  printf '%s\n' '\[(skip ci|ci skip|no ci|skip actions|actions skip)\]|^[[:space:]]*skip-checks[[:space:]]*:'
}

# port_sanitize: copy stdin to stdout with every line that carries AI attribution replaced, so
# text that ends up in commits or pull requests never carries it, and with CI skip markers
# defused: an upstream subject such as "Fix a typo [skip ci]" in a port commit message would stop
# GitHub from running CI on the port branch. "[skip ci]" becomes "(skip ci)"; a skip-checks
# trailer line is replaced.
port_sanitize() {
  local re
  re=$(port_attribution_re)
  PORT_RE=${re} perl -ne '
    BEGIN { $re = qr/$ENV{PORT_RE}/i }
    if (/$re/) { print "[removed: AI attribution]\n"; next }
    if (/\A\s*skip-checks\s*:/i) { print "[removed: CI skip marker]\n"; next }
    s/\[(\s*(?:skip\s*ci|ci\s*skip|no\s*ci|skip\s*actions|actions\s*skip)\s*)\]/($1)/gi;
    print;
  '
}

# port_output NAME VALUE: set a single-line step output.
port_output() {
  local name=${1:?} value=${2-}
  if [[ "${value}" == *$'\n'* ]]; then
    port_die "step output ${name} must be a single line"
  fi
  printf '%s=%s\n' "${name}" "${value}" >> "${GITHUB_OUTPUT:?}"
}

# port_output_file NAME FILE: set a step output to the contents of FILE (may be multi-line).
port_output_file() {
  local name=${1:?} file=${2:?} delim
  delim=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
  delim="PORT_EOF_${delim}"
  if grep -qxF -e "${delim}" "${file}"; then
    port_die "output delimiter collision for ${name}"
  fi
  {
    printf '%s<<%s\n' "${name}" "${delim}"
    cat "${file}"
    printf '\n%s\n' "${delim}"
  } >> "${GITHUB_OUTPUT:?}"
}

# port_csv_lines CSV: the comma-separated names in CSV, one per line, each checked against
# ^[A-Za-z0-9_]+$ so they can be used in paths and patterns.
port_csv_lines() {
  local csv=${1-} item
  local -a items=()
  IFS=',' read -r -a items <<< "${csv}"
  for item in "${items[@]}"; do
    item=${item//[[:space:]]/}
    if [[ -z "${item}" ]]; then
      continue
    fi
    if [[ ! "${item}" =~ ^[A-Za-z0-9_]+$ ]]; then
      port_die "invalid name '${item}' in list '${csv}'"
    fi
    printf '%s\n' "${item}"
  done
}
