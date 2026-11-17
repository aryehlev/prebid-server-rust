#!/usr/bin/env bash
# render-inputs.sh: decide whether the agent sessions run, and render everything they get.
#
# Step outputs
#   run_claude                       true | false
#   port_prompt review_prompt fix_prompt      prompts (multi-line)
#   port_args review_args fix_args            claude_args for claude-code-action (multi-line)
#   port_settings review_settings fix_settings  settings files: .github/port/claude-settings.json
#                                    plus the session's environment and its permissions.allow list
#
# The sessions run in permission mode "default", so a tool call that no allow rule covers is
# denied; the deny list in claude-settings.json always wins over the allow lists built here.
#
# Env
#   ACTION RUN_MODE NEEDS_CLAUDE BUDGET_CLASS   from the plan
#   CTX MERGED_SHA BASE_SHA TARGET_SHA ADAPTER_DIRS PR_NUMBER PORT_BRANCH RUN_URL
#   PORT_MODEL REVIEW_MODEL PORT_FALLBACK_MODEL
#   PORT_BUDGET_USD PORT_CORE_BUDGET_USD REVIEW_BUDGET_USD FIX_BUDGET_USD
#   RUNNER_TEMP GITHUB_WORKSPACE GITHUB_OUTPUT
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
ctx=${CTX:?}
action=${ACTION:?}
run_mode=${RUN_MODE:-auto}
temp=${RUNNER_TEMP:?}
workspace=${GITHUB_WORKSPACE:?}
: "${GITHUB_OUTPUT:?}"

check_value() {
  local name=${1:?} value=${2-} pattern=${3:?}
  if [[ ! "${value}" =~ ${pattern} ]]; then
    port_die "render-inputs: ${name} has an unexpected value '${value}'"
  fi
}
check_value PORT_MODEL "${PORT_MODEL:-}" '^[A-Za-z0-9._-]+$'
check_value REVIEW_MODEL "${REVIEW_MODEL:-}" '^[A-Za-z0-9._-]+$'
check_value PORT_FALLBACK_MODEL "${PORT_FALLBACK_MODEL:-}" '^[A-Za-z0-9._-]+$'
for budget in PORT_BUDGET_USD PORT_CORE_BUDGET_USD REVIEW_BUDGET_USD FIX_BUDGET_USD; do
  check_value "${budget}" "${!budget:-}" '^[0-9]+([.][0-9]+)?$'
done
check_value MERGED_SHA "${MERGED_SHA:-}" '^[0-9a-f]{40}$'
check_value BASE_SHA "${BASE_SHA:-}" '^[0-9a-f]{40}$'
check_value TARGET_SHA "${TARGET_SHA:-}" '^[0-9a-f]{40}$'
check_value PR_NUMBER "${PR_NUMBER:-}" '^[0-9]*$'
check_value PORT_BRANCH "${PORT_BRANCH:-}" '^[A-Za-z0-9._/-]+$'
for path in "${ctx}" "${temp}" "${workspace}"; do
  check_value path "${path}" '^/[A-Za-z0-9._/-]+$'
done
adapter_lines=$(port_csv_lines "${ADAPTER_DIRS:-}")
adapter_dirs=$(paste -s -d, - <<< "${adapter_lines}")

# Whether the agents run: never in merge-only mode; otherwise for repairs, for commits whose class
# needs Rust changes, and when the merge alone broke a check new upstream data can break.
before_failed=""
if [[ -f "${ctx}/gates-before.json" ]]; then
  before_failed=$(jq -r '[.failed[]? | select(. == "test" or . == "conformance" or . == "devtools")] | join(",")' \
    "${ctx}/gates-before.json")
fi
run_claude=false
if [[ "${run_mode}" != merge-only ]]; then
  if [[ "${action}" == repair || "${NEEDS_CLAUDE:-false}" == true || -n "${before_failed}" ]]; then
    run_claude=true
  fi
fi
port_output run_claude "${run_claude}"
printf 'render-inputs: run_claude=%s (action %s, needs_claude %s, failing before: %s)\n' \
  "${run_claude}" "${action}" "${NEEDS_CLAUDE:-false}" "${before_failed:-none}"

rustfmt_toolchain=$(rust/scripts/rustfmt-toolchain.sh)
stable_toolchain=$(sed -n -E 's/^channel[[:space:]]*=[[:space:]]*"([^"]+)".*$/\1/p' rust/rust-toolchain.toml)
check_value "rust-toolchain.toml channel" "${stable_toolchain}" '^[A-Za-z0-9._-]+$'
conformance="${PORT_SCRIPTS}/conformance.sh"
check_value "conformance.sh path" "${conformance}" '^/[A-Za-z0-9._/-]+$'
gomodcache=""
if command -v go > /dev/null; then
  gomodcache=$(go env GOMODCACHE)
  check_value GOMODCACHE "${gomodcache}" '^/[A-Za-z0-9._/-]+$'
fi

read_tools=(Read Glob Grep)
inspect_tools=(
  "Bash(cd *)"
  "Bash(ls *)"
  "Bash(wc *)"
  "Bash(diff *)"
  "Bash(jq *)"
  "Bash(git diff *)"
  "Bash(git log *)"
  "Bash(git show *)"
  "Bash(git status *)"
  "Bash(git ls-files *)"
  "Bash(git grep *)"
  "Bash(git blame *)"
  "Bash(git rev-parse *)"
  "Bash(git cat-file *)"
  "Bash(git merge-base *)"
  "Bash(cargo build *)"
  "Bash(cargo check *)"
  "Bash(cargo test *)"
  "Bash(cargo clippy *)"
  "Bash(cargo doc *)"
  "Bash(cargo metadata *)"
  "Bash(cargo tree *)"
  "Bash(go test *)"
  "Bash(go vet *)"
  "Bash(go build *)"
  "Bash(go list *)"
  "Bash(go doc *)"
  "Bash(go env *)"
  "Bash(${conformance})"
  "Bash(${conformance} *)"
  "Bash(rust/scripts/check-no-lint-suppression.sh)"
  "Bash(scripts/check-no-lint-suppression.sh)"
)
port_tools=(
  "${read_tools[@]}"
  Agent
  TodoWrite
  "Edit(./rust/**)"
  "Edit(./**/zz_parity_scratch_test.go)"
  "${inspect_tools[@]}"
  "Bash(cargo +${rustfmt_toolchain} fmt *)"
  "Bash(rustfmt +${rustfmt_toolchain} *)"
  "Bash(cargo run -p pbs-devtools *)"
  "Bash(taplo fmt *)"
  "Bash(scripts/cargo-machete.sh)"
  "Bash(rust/scripts/cargo-machete.sh)"
)
review_tools=(
  "${read_tools[@]}"
  TodoWrite
  "Edit(./rust/crates/*/tests/adversarial_*.rs)"
  "Edit(./**/zz_parity_scratch_test.go)"
  "${inspect_tools[@]}"
)

# settings PASS TOOLS...: write the settings file of one session and print its path.
settings() {
  local file="${temp}/claude-settings-${1:?}.json" allow
  shift
  allow=$(printf '%s\n' "$@" | jq -R -s -c 'split("\n") | map(select(length > 0))')
  jq --argjson allow "${allow}" --arg go_root "${workspace}" --arg toolchain "${stable_toolchain}" \
    '.env.PBS_GO_ROOT = $go_root | .env.RUSTUP_TOOLCHAIN = $toolchain | .permissions.allow = $allow' \
    "${PORT_DIR}/claude-settings.json" > "${file}"
  printf '%s\n' "${file}"
}

schema() {
  local compact
  compact=$(jq -c '.' "${PORT_DIR}/schemas/${1:?}.schema.json")
  if [[ "${compact}" == *"'"* ]]; then
    port_die "render-inputs: schema ${1} must not contain a single quote"
  fi
  printf '%s\n' "${compact}"
}

# args FILE MODEL TURNS BUDGET SYSTEM_PROMPT SCHEMA TOOLS...: write claude_args for one session.
args() {
  local file=${1:?} model=${2:?} turns=${3:?} budget=${4:?} system=${5:?} schema_name=${6:?} tools schema_json
  shift 6
  tools=$(printf '%s,' "$@")
  tools=${tools%,}
  if [[ "${tools}" == *'"'* || "${tools}" == *'$'* ]]; then
    port_die "render-inputs: tool rules must not contain quotes or dollar signs"
  fi
  schema_json=$(schema "${schema_name}")
  {
    printf -- '--model %s\n' "${model}"
    printf -- '--fallback-model %s\n' "${PORT_FALLBACK_MODEL}"
    printf -- '--permission-mode default\n'
    printf -- '--max-turns %s\n' "${turns}"
    printf -- '--max-budget-usd %s\n' "${budget}"
    printf -- '--add-dir %s\n' "${ctx}"
    if [[ -n "${gomodcache}" ]]; then
      printf -- '--add-dir %s\n' "${gomodcache}"
    fi
    printf -- '--append-system-prompt-file %s\n' "${PORT_DIR}/${system}"
    printf -- '--allowedTools "%s"\n' "${tools}"
    printf -- "--json-schema '%s'\n" "${schema_json}"
  } > "${file}"
}

commit_table=$(jq -r '.commits[] | "| `\(.sha[0:12])` | \(.subject | gsub("[|`\r\n]"; " ")) | \(.classes | join(", ")) | \(.adapter_dirs | join(", ")) |"' \
  "${ctx}/plan.json")
commit_count=$(jq -r '.commits | length' "${ctx}/plan.json")

# render TEMPLATE TURNS OUT: fill ${NAME} placeholders in one pass (inserted text is never
# rescanned, so commit subjects cannot inject placeholders).
render() {
  local template=${1:?} turns=${2:?} rendered=${3:?}
  TPL_RUN_URL=${RUN_URL:-} TPL_BASE_SHA=${BASE_SHA} TPL_TARGET_SHA=${TARGET_SHA} \
    TPL_MERGED_SHA=${MERGED_SHA} TPL_N_COMMITS=${commit_count} TPL_CTX=${ctx} \
    TPL_COMMIT_TABLE=${commit_table} TPL_ADAPTER_DIRS=${adapter_dirs:-none} TPL_MAX_TURNS=${turns} \
    TPL_PR_NUMBER=${PR_NUMBER:-} TPL_PORT_BRANCH=${PORT_BRANCH} TPL_CONFORMANCE=${conformance} \
    TPL_RUSTFMT_TOOLCHAIN=${rustfmt_toolchain} \
    perl -pe 's/\$\{([A-Z_]+)\}/exists $ENV{"TPL_$1"} ? $ENV{"TPL_$1"} : die("unknown placeholder $1\n")/ge' \
    "${PORT_DIR}/prompts/${template}.md.tmpl" > "${rendered}"
}

out="${temp}/claude-inputs"
mkdir -p "${out}"
port_turns=150
port_budget=${PORT_BUDGET_USD:?}
port_template=port
if [[ "${action}" == repair ]]; then
  port_turns=100
  port_budget=${FIX_BUDGET_USD:?}
  port_template=repair
elif [[ "${BUDGET_CLASS:-adapter}" == core ]]; then
  port_turns=300
  port_budget=${PORT_CORE_BUDGET_USD:?}
fi
render "${port_template}" "${port_turns}" "${out}/port-prompt.md"
render review 60 "${out}/review-prompt.md"
render fix 100 "${out}/fix-prompt.md"
args "${out}/port-args.txt" "${PORT_MODEL}" "${port_turns}" "${port_budget}" PORT_SYSTEM.md port-result \
  "${port_tools[@]}"
args "${out}/review-args.txt" "${REVIEW_MODEL}" 60 "${REVIEW_BUDGET_USD:?}" REVIEW_SYSTEM.md review-result \
  "${review_tools[@]}"
args "${out}/fix-args.txt" "${PORT_MODEL}" 100 "${FIX_BUDGET_USD}" PORT_SYSTEM.md port-result \
  "${port_tools[@]}"
port_settings=$(settings port "${port_tools[@]}")
review_settings=$(settings review "${review_tools[@]}")
fix_settings=$(settings fix "${port_tools[@]}")

port_output_file port_prompt "${out}/port-prompt.md"
port_output_file review_prompt "${out}/review-prompt.md"
port_output_file fix_prompt "${out}/fix-prompt.md"
port_output_file port_args "${out}/port-args.txt"
port_output_file review_args "${out}/review-args.txt"
port_output_file fix_args "${out}/fix-args.txt"
port_output port_settings "${port_settings}"
port_output review_settings "${review_settings}"
port_output fix_settings "${fix_settings}"
printf 'render-inputs: rendered prompts, arguments and settings in %s\n' "${out}"
