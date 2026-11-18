#!/usr/bin/env bash
# guard.sh: check the repository invariants of REF. It only reads git objects, of this repository
# and of the upstream mirror, and never builds or runs code from REF, so it is safe to run on
# untrusted commits (the publish job runs it on the port job's bundle before pushing anything).
#
# Usage: guard.sh --ref REF --base BASE [--upstream GITDIR] [--bot] [--allow-human]
#                 [--parity-harness] [--strict-dirs a,b] [--pr-text FILE]
#   --ref             the commit to check
#   --base            what REF is compared with: the pull request base, the commit before a push,
#                     or the commit a port run started from. The rules compare REF with
#                     merge-base(BASE, REF), which is what the change itself did.
#   --upstream        the bare upstream mirror (upstream-mirror.sh; default $UPSTREAM_GIT)
#   --bot             also apply the rules for what the port bot publishes
#   --allow-human     with --bot: commits by other authors are allowed (pull request CI of the port
#                     branch, where people may push fixes); without it every non-merge commit must
#                     be the bot's (the publish job)
#   --parity-harness  the parity-harness branch: parity/ may exist and hold Go code
#   --strict-dirs     adapter directories the change touches
#   --pr-text         a file with the pull request title and body
#
# Rules
#   upstream-files  the repository holds no upstream Go server file: only the top-level paths of
#                   lib.sh port_top_level_paths (and parity/ on the parity-harness branch); Go code
#                   only in the Go tooling modules (and parity/); no symbolic link, submodule,
#                   .gitmodules, go.work or vendor/ directory of a Go module; and no file
#                   byte-identical to a file of the upstream tree pinned at REF or at the
#                   merge-base, except an empty file or a lib.sh port_upstream_copies entry
#   local-state     no tracked agent configuration or local tool state (.claude/, .omx/, ...)
#   marker          UPSTREAM_SYNC pins one upstream commit, on the upstream branch, and the pin
#                   did not move backwards from the merge-base's
#   known-list      conformance/known_failures.txt is sorted, has no duplicates and lists only
#                   files of the pinned upstream tree
#   known-strict    no known failures are added for the --strict-dirs adapters
#   attribution     no AI attribution in the messages or identities of the first-parent commits
#                   of BASE..REF, nor in --pr-text
# Bot rules (--bot), measured against merge-base(REF, default branch) so that merges of the
# default branch into the port branch do not count
#   bot-merge      merge commits only merge the default branch
#   bot-author     non-merge commits are the bot's (unless --allow-human), and the bot's touch
#                  only the paths of lib.sh port_writable_paths and port_record_paths
#   bot-protected  no change to the protected paths (lib.sh port_protected_paths)
#   bot-deps       no new external crate in Cargo.lock
#   bot-known      no known failures are added
#   bot-log        upstream-port-log.jsonl only grows: what that merge-base has is unchanged
# Env: UPSTREAM_BRANCH BASE_BRANCH, UPSTREAM_GIT, PORT_GIT_EMAIL (the bot's commit email)
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

usage() {
  printf 'usage: guard.sh --ref REF --base BASE [--upstream GITDIR] [--bot] [--allow-human] [--parity-harness] [--strict-dirs a,b] [--pr-text FILE]\n' >&2
  exit 2
}

ref_arg=""
base_arg=""
upstream_arg=${UPSTREAM_GIT:-}
bot=false
allow_human=false
parity=false
strict_csv=""
pr_text=""
while [[ "$#" -gt 0 ]]; do
  case "${1}" in
    --bot | --allow-human | --parity-harness)
      case "${1}" in
        --bot) bot=true ;;
        --allow-human) allow_human=true ;;
        *) parity=true ;;
      esac
      shift
      ;;
    --ref | --base | --upstream | --strict-dirs | --pr-text)
      if [[ "$#" -lt 2 ]]; then
        usage
      fi
      case "${1}" in
        --ref) ref_arg=${2} ;;
        --base) base_arg=${2} ;;
        --upstream) upstream_arg=${2} ;;
        --strict-dirs) strict_csv=${2} ;;
        *) pr_text=${2} ;;
      esac
      shift 2
      ;;
    *) usage ;;
  esac
done
if [[ -z "${ref_arg}" || -z "${base_arg}" ]]; then
  usage
fi
if [[ -z "${upstream_arg}" ]]; then
  port_die "guard: name the upstream mirror with --upstream or UPSTREAM_GIT (upstream-mirror.sh makes it)"
fi
UPSTREAM_GIT=$(cd "${upstream_arg}" && pwd)
export UPSTREAM_GIT

port_cd_top
ref=$(git rev-parse --verify --end-of-options "${ref_arg}^{commit}")
base=$(git rev-parse --verify --end-of-options "${base_arg}^{commit}")
up=$(port_upstream_ref)
main=$(port_main_ref)
if ! git --git-dir="${UPSTREAM_GIT}" rev-parse -q --verify "${up}^{commit}" > /dev/null; then
  port_die "guard: the upstream mirror ${UPSTREAM_GIT} has no ${up}; run upstream-mirror.sh first"
fi
strict_lines=$(port_csv_lines "${strict_csv}")
bot_email=${PORT_GIT_EMAIL:-41898282+github-actions[bot]@users.noreply.github.com}
known_path=conformance/known_failures.txt
log_path=upstream-port-log.jsonl
change_base=$(git merge-base "${base}" "${ref}" || printf '%s' "${base}")

tmp=$(mktemp -d)
trap 'rm -rf -- "${tmp}"' EXIT
violations=0

violation() {
  local rule=${1:?}
  shift
  port_error "[${rule}] $*"
  violations=$((violations + 1))
}

# known_entries REV: the sorted entries of the known-failures list at REV.
known_entries() {
  if git cat-file -e "${1:?}:${known_path}" 2> /dev/null; then
    git show "${1}:${known_path}" > "${tmp}/known-show"
    port_list "${tmp}/known-show" | sort -u
  fi
}

# known_added FROM: entries of REF's known-failures list that FROM's list does not have.
known_added() {
  known_entries "${1:?}" > "${tmp}/known-from"
  known_entries "${ref}" > "${tmp}/known-ref"
  comm -13 "${tmp}/known-from" "${tmp}/known-ref"
}

# joined TEXT: TEXT's first 10 lines joined with ", ".
joined() {
  local text
  text=$(head -n 10 <<< "${1-}")
  printf '%s\n' "${text//$'\n'/, }"
}

protected_listing=$(port_protected_paths)
mapfile -t protected <<< "${protected_listing}"
writable_listing=$(port_writable_paths)
record_listing=$(port_record_paths)
bot_paths_listing=$(printf '%s\n%s\n' "${writable_listing}" "${record_listing}")
mapfile -t bot_paths <<< "${bot_paths_listing}"
outside_bot=(':(top)')
for path in "${bot_paths[@]}"; do
  outside_bot+=(":(top,exclude)${path}")
done

# marker: the pins of REF and of the merge-base. The upstream-files and known-list rules read the
# pinned trees, so this rule runs first.
marker=$(port_marker "${ref}")
base_marker=$(port_marker "${change_base}")
marker_ok=false
if [[ -z "${marker}" ]]; then
  violation marker "UPSTREAM_SYNC at ${ref:0:12} must hold exactly one line that is not a # comment: a full upstream commit SHA"
elif ! git --git-dir="${UPSTREAM_GIT}" cat-file -e "${marker}^{commit}" 2> /dev/null; then
  violation marker "UPSTREAM_SYNC pins ${marker:0:12}, which is not in the upstream mirror"
elif ! git --git-dir="${UPSTREAM_GIT}" merge-base --is-ancestor "${marker}" "${up}"; then
  violation marker "UPSTREAM_SYNC pins ${marker:0:12}, which is not on upstream ${UPSTREAM_BRANCH:-master}"
else
  marker_ok=true
  if [[ -n "${base_marker}" && "${base_marker}" != "${marker}" ]] \
    && ! git --git-dir="${UPSTREAM_GIT}" merge-base --is-ancestor "${base_marker}" "${marker}" 2> /dev/null; then
    violation marker "UPSTREAM_SYNC moved backwards from ${base_marker:0:12} to ${marker:0:12}"
  fi
fi

# upstream-files
git ls-tree -r --full-tree "${ref}" > "${tmp}/tree"
top_listing=$(port_top_level_paths)
if [[ "${parity}" == true ]]; then
  top_listing+=$'\nparity'
fi
go_re=$(port_go_dir_re)
if [[ "${parity}" == true ]]; then
  go_re=$(port_go_dir_re parity)
fi
found=$(PORT_TOP_PATHS=${top_listing} perl -ne '
  BEGIN { %allowed = map { $_ => 1 } split /\n/, $ENV{PORT_TOP_PATHS}; }
  next unless /\A\d+ \w+ [0-9a-f]+\t(.*)\z/s;
  my $path = $1;
  chomp $path;
  my ($top) = split m{/}, $path, 2;
  print "$top\n" unless $allowed{$top};
' "${tmp}/tree" | sort -u)
if [[ -n "${found}" ]]; then
  list=$(joined "${found}")
  violation upstream-files "top-level paths outside the allowed set (lib.sh port_top_level_paths): ${list}"
fi
found=$(sed -n -E 's/^[0-9]+ blob [0-9a-f]+\t(.*\.go)$/\1/p' "${tmp}/tree" | grep -v -E -e "${go_re}" || true)
if [[ -n "${found}" ]]; then
  list=$(joined "${found}")
  violation upstream-files "Go code outside the Go tooling modules (crates/<crate>/testdata/go, crates/devtools/go): ${list}"
fi
found=$(sed -n -E 's/^(120000|160000) [a-z]+ [0-9a-f]+\t(.*)$/\2 (\1)/p' "${tmp}/tree")
if [[ -n "${found}" ]]; then
  list=$(joined "${found}")
  violation upstream-files "symbolic links and submodules are not allowed: ${list}"
fi
found=$(sed -n -E 's/^[0-9]+ [a-z]+ [0-9a-f]+\t(.*)$/\1/p' "${tmp}/tree" \
  | grep -E -e '(^|/)\.gitmodules$' -e '(^|/)go\.work(\.sum)?$' -e '(^crates/[^/]+/testdata/go|^crates/devtools/go|^parity)/(.*/)?vendor/' || true)
if [[ -n "${found}" ]]; then
  list=$(joined "${found}")
  violation upstream-files "a .gitmodules, go.work or vendor/ directory brings in code from elsewhere: ${list}"
fi
if [[ "${marker_ok}" == true ]]; then
  : > "${tmp}/upstream-blobs"
  for pin in "${marker}" "${base_marker}"; do
    if [[ -z "${pin}" ]] || ! git --git-dir="${UPSTREAM_GIT}" cat-file -e "${pin}^{commit}" 2> /dev/null; then
      continue
    fi
    port_up ls-tree -r --full-tree "${pin}" | sed -n -E 's/^[0-9]+ blob ([0-9a-f]+)\t.*$/\1/p' >> "${tmp}/upstream-blobs"
  done
  sort -u "${tmp}/upstream-blobs" -o "${tmp}/upstream-blobs"
  copies=$(port_upstream_copies | cut -f 1)
  empty_blob=$(git hash-object -t blob /dev/null)
  found=$(PORT_COPIES=${copies} PORT_EMPTY=${empty_blob} perl -e '
    my ($blobs, $tree) = @ARGV;
    my %allowed = map { $_ => 1 } split /\n/, $ENV{PORT_COPIES};
    open(my $b, "<", $blobs) or die "$blobs: $!";
    my %upstream = map { chomp; $_ => 1 } <$b>;
    open(my $t, "<", $tree) or die "$tree: $!";
    while (my $line = <$t>) {
      chomp $line;
      next unless $line =~ /\A\d+ blob ([0-9a-f]+)\t(.*)\z/;
      my ($blob, $path) = ($1, $2);
      next if $blob eq $ENV{PORT_EMPTY} || $allowed{$path} || !$upstream{$blob};
      print "$path\n";
    }
  ' "${tmp}/upstream-blobs" "${tmp}/tree")
  if [[ -n "${found}" ]]; then
    list=$(joined "${found}")
    violation upstream-files "files byte-identical to the pinned upstream tree (upstream code is read from the Go root, never copied; lib.sh port_upstream_copies lists the exceptions): ${list}"
  fi
fi

# local-state: no committed agent configuration (Claude settings/hooks/MCP, ripgrep config) or
# local tool state.
agent_cfg_listing=$(port_agent_config_paths)
mapfile -t agent_cfg <<< "${agent_cfg_listing}"
tracked=$(git ls-tree -r --name-only "${ref}" -- "${agent_cfg[@]}" | head -n 10)
if [[ -n "${tracked}" ]]; then
  violation local-state "agent configuration or local tool state is committed: ${tracked//$'\n'/, }"
fi

# known-list
if git cat-file -e "${ref}:${known_path}" 2> /dev/null; then
  git show "${ref}:${known_path}" > "${tmp}/known-show"
  port_list "${tmp}/known-show" > "${tmp}/known-raw"
  sort -u "${tmp}/known-raw" > "${tmp}/known-sorted"
  if ! cmp -s "${tmp}/known-raw" "${tmp}/known-sorted"; then
    violation known-list "${known_path} is not sorted (LC_ALL=C) or has duplicate lines"
  fi
  if [[ "${marker_ok}" == true && -s "${tmp}/known-sorted" ]]; then
    sed "s|^|${marker}:|" "${tmp}/known-sorted" | port_up cat-file --batch-check > "${tmp}/known-check"
    missing=$(sed -n -E 's/^[0-9a-f]{40}:(.*) missing$/\1/p' "${tmp}/known-check" | head -n 20)
    if [[ -n "${missing}" ]]; then
      violation known-list "${known_path} lists files the pinned upstream tree does not have: ${missing//$'\n'/, }"
    fi
  fi
fi

# known-strict
if [[ -n "${strict_lines}" ]]; then
  added=$(known_added "${change_base}")
  while IFS= read -r dir; do
    hits=$(grep -F -e "adapters/${dir}/" <<< "${added}" | head -n 10 || true)
    if [[ -n "${hits}" ]]; then
      violation known-strict "known failures were added for adapters/${dir}, which this change touches: ${hits//$'\n'/, }"
    fi
  done <<< "${strict_lines}"
fi

# attribution
commits=$(git rev-list --first-parent "${ref}" "^${base}")
attribution_re=$(port_attribution_re)
name_re=$(port_ai_name_re)
email_re=$(port_ai_email_re)
checked=0
while IFS= read -r commit; do
  if [[ -z "${commit}" ]]; then
    continue
  fi
  checked=$((checked + 1))
  body=$(git log -1 --format=%B "${commit}")
  hit=$(grep -Ei -m 1 -e "${attribution_re}" <<< "${body}" || true)
  if [[ -n "${hit}" ]]; then
    violation attribution "commit ${commit:0:12} carries AI attribution: ${hit}"
  fi
  names=$(git log -1 --format='%an%n%cn' "${commit}")
  hit=$(grep -Ei -m 1 -e "${name_re}" <<< "${names}" || true)
  if [[ -n "${hit}" ]]; then
    violation attribution "commit ${commit:0:12} is authored or committed as '${hit}'"
  fi
  emails=$(git log -1 --format='%ae%n%ce' "${commit}")
  hit=$(grep -Ei -m 1 -e "${email_re}" <<< "${emails}" || true)
  if [[ -n "${hit}" ]]; then
    violation attribution "commit ${commit:0:12} is authored or committed by '${hit}'"
  fi
done <<< "${commits}"
if [[ -n "${pr_text}" ]]; then
  hit=$(grep -Ei -m 1 -e "${attribution_re}" "${pr_text}" || true)
  if [[ -n "${hit}" ]]; then
    violation attribution "the pull request title or body carries AI attribution: ${hit}"
  fi
fi

# bot rules
if [[ "${bot}" == true ]]; then
  if ! git rev-parse -q --verify "${main}" > /dev/null; then
    port_die "guard: ${main} is missing; fetch the default branch first"
  fi
  main_base=$(git merge-base "${ref}" "${main}" || true)
  if [[ -z "${main_base}" ]]; then
    violation bot-merge "${ref:0:12} shares no history with ${main}"
    main_base=${base}
  fi
  while IFS= read -r commit; do
    if [[ -z "${commit}" ]]; then
      continue
    fi
    parent_line=$(git rev-list --parents -n 1 "${commit}")
    read -r -a fields <<< "${parent_line}"
    if [[ "${#fields[@]}" -gt 2 ]]; then
      for parent in "${fields[@]:2}"; do
        if ! git merge-base --is-ancestor "${parent}" "${main}"; then
          violation bot-merge "merge ${commit:0:12} merges ${parent:0:12}, which is not on ${main#refs/remotes/origin/}"
        fi
      done
      continue
    fi
    author=$(git log -1 --format=%ae "${commit}")
    if [[ "${author}" == "${bot_email}" ]]; then
      touched=$(git diff-tree --no-commit-id --name-only -r "${commit}" -- "${outside_bot[@]}" | head -n 10)
      if [[ -n "${touched}" ]]; then
        violation bot-author "bot commit ${commit:0:12} touches paths the bot may not change: ${touched//$'\n'/, }"
      fi
    elif [[ "${allow_human}" == false ]]; then
      violation bot-author "commit ${commit:0:12} by ${author} is not the bot's (${bot_email})"
    fi
  done <<< "${commits}"
  protected_specs=$(port_pathspecs "${protected[@]}")
  mapfile -t protected_spec <<< "${protected_specs}"
  changed=$(git diff --name-only "${main_base}" "${ref}" -- "${protected_spec[@]}" | head -n 20)
  if [[ -n "${changed}" ]]; then
    violation bot-protected "protected paths changed: ${changed//$'\n'/, }"
  fi
  lock_diff=$(git diff "${main_base}" "${ref}" -- ':(top)Cargo.lock')
  sources=$(grep -E '^\+source = ' <<< "${lock_diff}" | sort -u | head -n 10 || true)
  if [[ -n "${sources}" ]]; then
    violation bot-deps "Cargo.lock gained external packages: ${sources//$'\n'/, }"
  fi
  added=$(known_added "${main_base}")
  if [[ -n "${added}" ]]; then
    violation bot-known "known failures were added: ${added//$'\n'/, }"
  fi
  # An append adds lines and deletes none; an edited, removed or reordered line counts as deleted.
  log_stat=$(git diff --numstat --no-renames "${main_base}" "${ref}" -- ":(top)${log_path}")
  log_deleted=0
  if [[ -n "${log_stat}" ]]; then
    read -r _ log_deleted _ <<< "${log_stat}"
  fi
  if [[ "${log_deleted}" != 0 ]]; then
    violation bot-log "${log_path} changed lines that ${main#refs/remotes/origin/} already has; the port only appends to it"
  fi
fi

if [[ "${violations}" -gt 0 ]]; then
  printf 'guard: %s violation(s) in %s (%s first-parent commit(s) checked)\n' "${violations}" "${ref:0:12}" "${checked}" >&2
  exit 1
fi
mode=main
if [[ "${bot}" == true ]]; then
  mode=bot
fi
printf 'guard: ok for %s (%s rules, %s first-parent commit(s) checked)\n' "${ref:0:12}" "${mode}" "${checked}"
