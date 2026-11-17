#!/usr/bin/env bash
# plan.sh: decide what this run of the upstream-port workflow does. It never changes the
# repository; it reads git state, the open port pull request and CI results, then writes
# ${OUT_DIR}/plan.json and the step outputs the later jobs use.
#
# Actions
#   advance   merge and port the next chunk of upstream first-parent commits
#   repair    fix the failing CI of the open port pull request without porting new commits
#   escalate  label the open port pull request for a human and stop
#   none      nothing to do (up to date, waiting for CI or for a human)
#
# Env (all required unless marked optional)
#   UPSTREAM_BRANCH BASE_BRANCH PORT_BRANCH   branch names (upstream/<branch>, origin/<branch>)
#   RUN_MODE        auto | advance | repair | merge-only | dry-run
#   MAX_COMMITS MAX_PR_COMMITS MAX_REPAIRS MAX_GO_LINES   chunking limits (positive integers)
#   TARGET_INPUT    optional upstream first-parent commit to stop at
#   PORT_GIT_EMAIL  the bot's commit email (commits by anyone else count as human)
#   OUT_DIR GITHUB_OUTPUT GITHUB_REPOSITORY GH_TOKEN
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
out_dir=${OUT_DIR:?}
repo=${GITHUB_REPOSITORY:?}
port_branch=${PORT_BRANCH:?}
base_branch=${BASE_BRANCH:?}
: "${UPSTREAM_BRANCH:?}" "${GITHUB_OUTPUT:?}"
run_mode=${RUN_MODE:-auto}
bot_email=${PORT_GIT_EMAIL:?}
for limit_name in MAX_COMMITS MAX_PR_COMMITS MAX_REPAIRS MAX_GO_LINES; do
  limit_value=${!limit_name:-}
  if [[ ! "${limit_value}" =~ ^[1-9][0-9]*$ ]]; then
    port_die "${limit_name} must be a positive integer, got '${limit_value}'"
  fi
done
max_commits=${MAX_COMMITS:?}
max_pr_commits=${MAX_PR_COMMITS:?}
max_repairs=${MAX_REPAIRS:?}
max_go_lines=${MAX_GO_LINES:?}
case "${run_mode}" in
  auto | advance | repair | merge-only | dry-run) ;;
  *) port_die "RUN_MODE must be auto, advance, repair, merge-only or dry-run, got '${run_mode}'" ;;
esac
case "${port_branch}" in
  main | master | "${base_branch}") port_die "PORT_BRANCH must not be the default branch" ;;
  *) ;;
esac

up=$(port_upstream_ref)
main=$(port_main_ref)
branch_ref="refs/remotes/origin/${port_branch}"
mkdir -p "${out_dir}"
commits_file="${out_dir}/commits.jsonl"
: > "${commits_file}"

action=none
reason=""
restart=false
pr=""
base=""
target=""
port_branch_sha=""

# add_commit SHA: classify SHA against its first parent and append it to the chunk.
add_commit() {
  local sha=${1:?} subject entry skipped
  # Subjects reach commit messages, the port log and pull requests, so attribution is removed here.
  subject=$(git log -1 --format=%s "${sha}" | port_sanitize)
  entry=$("${PORT_SCRIPTS}/classify.sh" "${sha}^1" "${sha}")
  skipped=$(port_list "${PORT_DIR}/skip-commits.txt" | awk -v sha="${sha}" '$1 == sha { print "yes"; exit }')
  if [[ "${skipped}" == yes ]]; then
    entry=$(jq -c '.classes = ["skipped"] | .needs_claude = false | .heavy = false' <<< "${entry}")
  fi
  jq -c --arg sha "${sha}" --arg subject "${subject}" '. + {sha: $sha, subject: $subject}' <<< "${entry}" >> "${commits_file}"
}

finish() {
  local plan="${out_dir}/plan.json" budget_class dirs needs
  jq -s \
    --arg action "${action}" --arg reason "${reason}" --arg mode "${run_mode}" \
    --arg base "${base}" --arg target "${target}" --arg pr "${pr}" \
    --arg work_sha "${work_sha}" --arg port_branch_sha "${port_branch_sha}" \
    --argjson restart "${restart}" \
    '{action: $action, reason: $reason, run_mode: $mode, restart: $restart,
      base_sha: $base, target_sha: $target, pr_number: $pr,
      work_sha: $work_sha, port_branch_sha: $port_branch_sha, commits: .,
      adapter_dirs: ([.[].adapter_dirs[]] | unique), bidders: ([.[].bidders[]] | unique),
      needs_claude: any(.[]; .needs_claude), heavy: any(.[]; .heavy)}' \
    "${commits_file}" > "${plan}"
  budget_class=adapter
  heavy=$(jq -r '.heavy' "${plan}")
  if [[ "${action}" == repair ]]; then
    budget_class=repair
  elif [[ "${heavy}" == true ]]; then
    budget_class=core
  fi
  dirs=$(jq -r '.adapter_dirs | join(",")' "${plan}")
  needs=$(jq -r '.needs_claude' "${plan}")
  port_output action "${action}"
  port_output reason "${reason}"
  port_output restart "${restart}"
  port_output pr_number "${pr}"
  port_output work_sha "${work_sha}"
  port_output port_branch_sha "${port_branch_sha}"
  port_output base_sha "${base}"
  port_output target_sha "${target}"
  port_output adapter_dirs "${dirs}"
  port_output needs_claude "${needs}"
  port_output budget_class "${budget_class}"
  printf 'plan: %s (%s)\n' "${action}" "${reason}"
  exit 0
}

main_sync=$(port_marker "${main}")
if [[ -z "${main_sync}" ]]; then
  port_die "rust/UPSTREAM_SYNC on ${base_branch} holds no upstream commit"
fi
if ! git merge-base --is-ancestor "${main_sync}" "${main}"; then
  port_die "rust/UPSTREAM_SYNC ${main_sync:0:12} is not merged into ${base_branch}; was a port pull request squash- or rebase-merged? See .github/README.md"
fi
if ! git merge-base --is-ancestor "${main_sync}" "${up}"; then
  port_die "upstream ${UPSTREAM_BRANCH} does not contain rust/UPSTREAM_SYNC ${main_sync:0:12}"
fi
limit=$(git rev-parse "${up}")
if [[ -n "${TARGET_INPUT:-}" ]]; then
  if [[ ! "${TARGET_INPUT}" =~ ^[0-9a-f]{7,40}$ ]]; then
    port_die "target_sha must be a hex commit id"
  fi
  limit=$(git rev-parse --verify "${TARGET_INPUT}^{commit}")
  if ! git merge-base --is-ancestor "${limit}" "${up}"; then
    port_die "target ${limit:0:12} is not on upstream ${UPSTREAM_BRANCH}"
  fi
fi

action=advance
work_sha=$(git rev-parse "${main}")
base=${main_sync}
if git rev-parse -q --verify "${branch_ref}" > /dev/null; then
  port_branch_sha=$(git rev-parse "${branch_ref}")
fi
pr=$(gh pr list --repo "${repo}" --head "${port_branch}" --base "${base_branch}" --state open \
  --json number --jq '.[0].number // empty')

if [[ -n "${pr}" ]]; then
  if [[ ! "${pr}" =~ ^[0-9]+$ ]]; then
    port_die "unexpected pull request number '${pr}'"
  fi
  if [[ -z "${port_branch_sha}" ]]; then
    port_die "pull request #${pr} is open but ${port_branch} was not fetched"
  fi
  labels=$(gh pr view "${pr}" --repo "${repo}" --json labels --jq '[.labels[].name] | join(",")')
  if [[ ",${labels}," == *,port:needs-human,* ]]; then
    action=none
    reason="pull request #${pr} is labelled port:needs-human"
    finish
  fi
  authors=$(git log --first-parent --no-merges --format=%ae "${main}..${branch_ref}")
  human=$(grep -cvxF -e "${bot_email}" <<< "${authors}" || true)
  if [[ -z "${authors}" ]]; then
    human=0
  fi
  if ! git merge-tree --write-tree "${main}" "${branch_ref}" > /dev/null 2>&1; then
    if [[ "${human}" -eq 0 ]]; then
      restart=true
      reason="${port_branch} conflicts with ${base_branch}; rebuilding it from ${base_branch}"
    else
      action=escalate
      reason="${port_branch} conflicts with ${base_branch} and carries ${human} human commit(s)"
      finish
    fi
  else
    work_sha=${port_branch_sha}
    base=$(port_marker "${branch_ref}")
    if [[ -z "${base}" ]]; then
      port_die "rust/UPSTREAM_SYNC on ${port_branch} holds no upstream commit"
    fi
    ci=$(gh pr checks "${pr}" --repo "${repo}" --json name,bucket \
      --jq '[.[] | select(.name == "ci-ok") | .bucket][0] // "pending"' 2> /dev/null) || true
    ci=${ci:-pending}
    case "${ci}" in
      pass)
        on_pr=$(git rev-list --first-parent --count "${main_sync}..${base}")
        if [[ "${on_pr}" -ge "${max_pr_commits}" ]]; then
          action=none
          reason="pull request #${pr} carries ${on_pr} upstream commits; waiting for it to be merged"
          finish
        fi
        ;;
      fail | cancel)
        main_ci=$(gh run list --repo "${repo}" --workflow ci.yml --branch "${base_branch}" --event push \
          --limit 1 --json conclusion --jq '.[0].conclusion // "unknown"')
        subjects=$(git log --first-parent --format=%s "${main}..${branch_ref}")
        repairs=$(grep -c '^port-repair:' <<< "${subjects}" || true)
        if [[ "${main_ci}" != success ]]; then
          action=none
          reason="CI is red on pull request #${pr}, but CI on ${base_branch} is '${main_ci}'; not repairing"
          finish
        elif [[ "${repairs}" -ge "${max_repairs}" ]]; then
          action=escalate
          reason="CI is still red on pull request #${pr} after ${repairs} repair attempt(s)"
          finish
        fi
        action=repair
        reason="CI is red on pull request #${pr} (repair $((repairs + 1)) of ${max_repairs})"
        ;;
      *)
        action=none
        reason="ci-ok on pull request #${pr} is '${ci}'; waiting"
        finish
        ;;
    esac
  fi
elif [[ -n "${port_branch_sha}" ]]; then
  restart=true
  reason="${port_branch} exists without an open pull request; rebuilding it from ${base_branch}"
fi

case "${run_mode}" in
  advance)
    if [[ "${action}" == repair ]]; then
      action=advance
      reason="advance forced by the run mode"
    fi
    ;;
  repair)
    if [[ "${action}" != repair ]]; then
      action=none
      reason="run mode is repair but there is nothing to repair"
      finish
    fi
    ;;
  *) ;;
esac

if [[ "${action}" == repair ]]; then
  range=$(git rev-list --first-parent --reverse "${main_sync}..${base}")
  while IFS= read -r sha; do
    if [[ -n "${sha}" ]]; then
      add_commit "${sha}"
    fi
  done <<< "${range}"
  target=${base}
  run_id=$(gh run list --repo "${repo}" --workflow ci.yml --branch "${port_branch}" --limit 1 \
    --json databaseId --jq '.[0].databaseId // empty')
  if [[ "${run_id}" =~ ^[0-9]+$ ]]; then
    log=$(gh run view "${run_id}" --repo "${repo}" --log-failed 2> /dev/null) || log=""
    printf '%s\n' "${log}" | tail -c 200000 > "${out_dir}/ci-failures.log"
  fi
  finish
fi

pending_list=$(git rev-list --first-parent --reverse "${base}..${limit}")
if [[ -z "${pending_list}" ]]; then
  action=none
  reason="up to date with upstream at ${base:0:12}"
  finish
fi
mapfile -t pending <<< "${pending_list}"
count=0
lines=0
for sha in "${pending[@]}"; do
  before_lines=$(wc -l < "${commits_file}")
  add_commit "${sha}"
  entry=$(tail -n 1 "${commits_file}")
  heavy=$(jq -r '.heavy' <<< "${entry}")
  commit_lines=$(jq -r '.go_lines' <<< "${entry}")
  if [[ "${count}" -gt 0 ]] \
    && [[ "${heavy}" == true || "${count}" -ge "${max_commits}" || $((lines + commit_lines)) -gt "${max_go_lines}" ]]; then
    head -n "${before_lines}" "${commits_file}" > "${commits_file}.tmp"
    mv "${commits_file}.tmp" "${commits_file}"
    break
  fi
  count=$((count + 1))
  lines=$((lines + commit_lines))
  target=${sha}
  if [[ "${heavy}" == true ]]; then
    break
  fi
done
reason="${reason:+${reason}; }porting ${count} upstream commit(s) ${base:0:12}..${target:0:12}"
finish
