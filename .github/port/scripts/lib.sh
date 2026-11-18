#!/usr/bin/env bash
# lib.sh: helpers shared by the upstream-port and CI scripts. Sourcing it sets the strict shell
# options every caller uses (errexit, nounset, pipefail, inherit_errexit) and defines names.
#
# Values are returned on stdout (one item per line) so callers can capture them with $(...) and
# `mapfile`, never through globals or exit codes of functions used in conditions.
set -euo pipefail
shopt -s inherit_errexit
# Byte-order sorting and byte-wise matching everywhere: the known-failures list is sorted in
# LC_ALL=C order, and patterns must not depend on the runner's locale. It is exported once here
# rather than prefixed to single commands because bash 5.3 can crash on a temporary LC_ALL=...
# assignment inside a command substitution.
export LC_ALL=C

PORT_SCRIPTS=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PORT_DIR=$(cd "${PORT_SCRIPTS}/.." && pwd)

# port_protected_paths: the paths under rust/, one per line, that the port agents and the bot may
# never change. Upstream ports change crate sources, tests and generated data; these files set the
# rules those changes are checked against (the workspace and its lint policy, the toolchains, the
# formatter, clippy, cargo, dependency and suppression checks, the conformance contract and the
# porting rules). Tool configuration files that do not exist yet are listed too, so that an agent
# cannot add one: a rust/clippy.toml raising thresholds, a rust/.cargo/config.toml changing flags
# or sources, or a legacy rust/rust-toolchain file, which rustup prefers to rust-toolchain.toml.
port_protected_paths() {
  printf '%s\n' \
    rust/Cargo.toml \
    rust/rust-toolchain.toml \
    rust/rust-toolchain \
    rust/rustfmt.toml \
    rust/.rustfmt.toml \
    rust/clippy.toml \
    rust/.clippy.toml \
    rust/.cargo \
    rust/deny.toml \
    rust/taplo.toml \
    rust/PORTING.md \
    rust/README.md \
    rust/scripts \
    rust/conformance/README.md \
    rust/legacy
}

# port_record_paths: the port records under rust/, one per line: the upstream marker and the port
# log. Only commit.sh writes them, after the last agent session, so check-worktree.sh reverts any
# change a session makes to them, and guard.sh's bot-log rule checks that the log only grows.
port_record_paths() {
  printf '%s\n' \
    rust/UPSTREAM_SYNC \
    rust/upstream-port-log.jsonl
}

# port_agent_config_paths: repository-root paths that configure, or can run code in, a coding agent
# (Claude Code settings, hooks and MCP servers) or a tool it drives (ripgrep). The fork neither owns
# these nor accepts them from upstream, so upstream content can never drop settings, a hook or an MCP
# server that a port agent session would load and run with the job's credentials. They are excluded
# from port_nonfork_spec (so dropping them is not a go-tree difference from upstream),
# merge-upstream.sh never imports them, and guard.sh's local-state rule rejects any that are
# committed. .omx is local tool state and is listed here too.
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

# port_upstream_ref / port_main_ref: remote-tracking refs of the upstream branch and of the fork's
# default branch.
port_upstream_ref() {
  printf 'refs/remotes/upstream/%s\n' "${UPSTREAM_BRANCH:-master}"
}

port_main_ref() {
  printf 'refs/remotes/origin/%s\n' "${BASE_BRANCH:-main}"
}

# port_list [FILE]: the entries of a list file (or stdin), without comments, blank lines or
# surrounding space.
port_list() {
  sed -E -e 's/[[:space:]]*#.*$//' -e 's/^[[:space:]]+//' -e 's/[[:space:]]+$//' -e '/^$/d' "$@"
}

# port_fork_paths: the fork-owned paths, one per line.
port_fork_paths() {
  port_list "${PORT_DIR}/fork-paths.txt"
}

# port_nonfork_spec: git pathspecs, one per line, that select every path the fork does not own and
# does not treat as untrusted agent configuration. Both the fork-owned paths and the agent-config
# paths are excluded: the latter so that dropping an upstream-supplied .claude/.mcp.json/hook (see
# merge-upstream.sh) is not reported as a go-tree difference from upstream.
port_nonfork_spec() {
  local fork_listing agent_listing listing path
  fork_listing=$(port_fork_paths)
  agent_listing=$(port_agent_config_paths)
  listing=$(printf '%s\n%s\n' "${fork_listing}" "${agent_listing}")
  printf '%s\n' ':(top)'
  while IFS= read -r path; do
    if [[ -n "${path}" ]]; then
      printf ':(top,exclude)%s\n' "${path}"
    fi
  done <<< "${listing}"
}

# port_outside_rust_spec: git pathspecs, one per line, that select everything outside rust/.
port_outside_rust_spec() {
  printf '%s\n' ':(top)' ':(top,exclude)rust'
}

# port_marker REV: the upstream commit recorded in rust/UPSTREAM_SYNC at REV (the last 40-hex
# line), or nothing when the file is missing or holds no commit.
port_marker() {
  local text
  text=$(git show "${1:?}:rust/UPSTREAM_SYNC" 2> /dev/null) || text=""
  sed -n -E '/^[0-9a-f]{40}$/p' <<< "${text}" | tail -n 1
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

# port_sanitize: copy stdin to stdout with every line that carries AI attribution replaced, so
# text that ends up in commits or pull requests never carries it.
port_sanitize() {
  local re
  re=$(port_attribution_re)
  PORT_RE=${re} perl -ne 'BEGIN { $re = qr/$ENV{PORT_RE}/i } print(/$re/ ? "[removed: AI attribution]\n" : $_)'
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
