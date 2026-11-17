#!/usr/bin/env bash
# gh-shim.sh: a stand-in for the GitHub CLI in the port tooling tests (lib.sh t_gh_shim links it
# as gh). It answers from JSON files in GH_SHIM_DIR, applies --jq with the real jq so the scripts'
# filters are exercised, and appends every call to GH_SHIM_DIR/calls.log. A --body-file is copied
# to GH_SHIM_DIR/body-N.md (N counts from 1).
#
# State files (all optional)
#   prs.json               gh pr list               (default [])
#   pr_view.json           gh pr view               (default {"labels":[],"isDraft":false}); gh pr
#                          create, gh pr edit --add-label/--remove-label and gh pr ready keep its
#                          labels and draft state up to date, like GitHub would
#   pr_title.txt pr_body.md  the pull request title and body last set by gh pr create or gh pr edit
#   checks.json checks_rc  gh pr checks and its exit status (default [] and 0)
#   runs_<branch>.json     gh run list --branch B   ("/" in B becomes "_"; default [])
#   run_log.txt            gh run view --log-failed
#   issues_<label>.json    gh issue list --label L  (default [])
#   create_number          number of a created pull request (default 101); gh pr create writes
#                          prs.json, gh issue create writes issues_<label>.json (number 201)
#   fail_<cmd>_<sub>       the next "gh <cmd> <sub>" call fails (exit 1) and removes the file, for
#                          example fail_pr_create
set -euo pipefail
shopt -s inherit_errexit

dir=${GH_SHIM_DIR:?}
args=("$@")
printf '%s\n' "${args[*]}" >> "${dir}/calls.log"

jq_expr=""
body_file=""
branch=""
label=""
title=""
add_labels=""
remove_labels=""
draft=false
undo=false
index=0
while [[ "${index}" -lt "${#args[@]}" ]]; do
  case "${args[${index}]}" in
    --jq | --body-file | --branch | --label | --title | --add-label | --remove-label)
      option=${args[${index}]}
      index=$((index + 1))
      value=${args[${index}]-}
      case "${option}" in
        --jq) jq_expr=${value} ;;
        --body-file) body_file=${value} ;;
        --branch) branch=${value} ;;
        --label) label=${value} ;;
        --add-label) add_labels=${value} ;;
        --remove-label) remove_labels=${value} ;;
        *) title=${value} ;;
      esac
      ;;
    --draft) draft=true ;;
    --undo) undo=true ;;
    *) ;;
  esac
  index=$((index + 1))
done

if [[ -n "${body_file}" ]]; then
  bodies=$(find "${dir}" -maxdepth 1 -name 'body-*.md' | wc -l)
  bodies=${bodies//[[:space:]]/}
  cp "${body_file}" "${dir}/body-$((bodies + 1)).md"
fi

# state FILE DEFAULT: the contents of a state file, or DEFAULT.
state() {
  if [[ -f "${dir}/${1:?}" ]]; then
    cat "${dir}/${1}"
  else
    printf '%s\n' "${2:?}"
  fi
}

# view: the pull request state that gh pr view reports.
view() {
  state pr_view.json '{"labels":[],"isDraft":false}'
}

# save_view: standard input becomes the pull request state.
save_view() {
  cat > "${dir}/pr_view.json.tmp"
  mv "${dir}/pr_view.json.tmp" "${dir}/pr_view.json"
}

# remember_text: keep the title and body that gh pr create / gh pr edit set.
remember_text() {
  if [[ -n "${title}" ]]; then
    printf '%s\n' "${title}" > "${dir}/pr_title.txt"
  fi
  if [[ -n "${body_file}" ]]; then
    cp "${body_file}" "${dir}/pr_body.md"
  fi
}

fail_file="${dir}/fail_${1:-}_${2:-}"
if [[ -f "${fail_file}" ]]; then
  rm -f -- "${fail_file}"
  printf 'gh-shim: %s %s failed (injected)\n' "${1:-}" "${2:-}" >&2
  exit 1
fi

json=""
rc=0
command="${1:-} ${2:-}"
case "${command}" in
  "pr list") json=$(state prs.json '[]') ;;
  "pr view") json=$(view) ;;
  "pr checks")
    json=$(state checks.json '[]')
    rc=$(state checks_rc 0)
    ;;
  "pr create")
    number=$(state create_number 101)
    printf '[{"number":%s}]\n' "${number}" > "${dir}/prs.json"
    jq -n -c --argjson draft "${draft}" '{labels: [], isDraft: $draft}' > "${dir}/pr_view.json"
    remember_text
    ;;
  "pr edit")
    remember_text
    if [[ -n "${add_labels}${remove_labels}" ]]; then
      current=$(view)
      jq -c --arg add "${add_labels}" --arg remove "${remove_labels}" '.labels = ([.labels[].name]
        - ($remove | split(",")) + ($add | split(",")) | map(select(length > 0)) | unique | map({name: .}))' \
        <<< "${current}" | save_view
    fi
    ;;
  "pr ready")
    current=$(view)
    jq -c --argjson undo "${undo}" '.isDraft = $undo' <<< "${current}" | save_view
    ;;
  "run list") json=$(state "runs_${branch//\//_}.json" '[]') ;;
  "run view") state run_log.txt 'no log' ;;
  "issue list") json=$(state "issues_${label}.json" '[]') ;;
  "issue create")
    jq -n -c --arg title "${title}" '[{number: 201, title: $title}]' > "${dir}/issues_${label}.json"
    ;;
  "issue close") rm -f -- "${dir}/issues_parity-harness.json" ;;
  "label create" | "pr comment" | "issue comment") ;;
  *)
    printf 'gh-shim: unsupported command: %s\n' "${args[*]}" >&2
    exit 2
    ;;
esac
if [[ -n "${json}" ]]; then
  if [[ -n "${jq_expr}" ]]; then
    jq -r "${jq_expr}" <<< "${json}"
  else
    printf '%s\n' "${json}"
  fi
fi
exit "${rc}"
