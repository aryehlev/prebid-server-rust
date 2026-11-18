#!/usr/bin/env bash
# render-inputs.sh: when the agents run, their claude_args (models, budgets, turns, allow lists,
# the upstream trees, system prompts, JSON schemas), their settings (deny list with the read-only
# trees, attribution off, environment) and their prompts, which must be filled in one pass so
# commit subjects cannot inject placeholders.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

ctx="${T_ROOT}/ctx"
mkdir -p "${ctx}"
s1=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
s2=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
start=cccccccccccccccccccccccccccccccccccccccc
version=v4.0.1-0.20260101000000-bbbbbbbbbbbb
old_root="${T_ROOT}/old"
new_root="${T_ROOT}/new"
probe_root="${T_ROOT}/probe"
# A subject that tries to inject placeholders and code spans: foo: use ${CTX} and ${RUN_URL} | `code`
injected=$(printf 'foo: use %s{CTX} and %s{RUN_URL} | %scode%s' "\$" "\$" "\`" "\`")
verbatim=${injected%% |*}
code_span=${injected##*| }
jq -n --arg s1 "${s1}" --arg s2 "${s2}" --arg subject "${injected}" '{commits: [
  {sha: $s1, subject: $subject, classes: ["adapter-change"], adapter_dirs: ["foo"], needs_claude: true},
  {sha: $s2, subject: "docs", classes: ["docs-infra"], adapter_dirs: [], needs_claude: false}]}' > "${ctx}/plan.json"

# render NAME [VAR=VALUE...]: run render-inputs.sh from the top of this repository (it only reads
# it); outputs go to ${T_ROOT}/NAME/output.
render() {
  local name=${1:?} out
  shift
  out="${T_ROOT}/${name}"
  mkdir -p "${out}/runner"
  : > "${out}/output"
  t_run_in "${T_TOP}" env CTX="${ctx}" ACTION=advance RUN_MODE=auto NEEDS_CLAUDE=true BUDGET_CLASS=adapter \
    START_SHA="${start}" BASE_SHA="${s1}" TARGET_SHA="${s2}" PIN_VERSION="${version}" ADAPTER_DIRS=foo,bar \
    OLD_GO_ROOT="${old_root}" PBS_GO_ROOT="${new_root}" PROBE_ROOT="${probe_root}" \
    PR_NUMBER="" PORT_BRANCH=port/upstream-sync RUN_URL=https://example.invalid/runs/9 PORT_MODEL=claude-opus-5-5 \
    REVIEW_MODEL=claude-opus-5-5 PORT_FALLBACK_MODEL=claude-sonnet-5-5 PORT_BUDGET_USD=30 PORT_CORE_BUDGET_USD=60 \
    REVIEW_BUDGET_USD=10 FIX_BUDGET_USD=15 RUNNER_TEMP="${out}/runner" GITHUB_WORKSPACE="${T_TOP}" \
    GITHUB_OUTPUT="${out}/output" "$@" "${script}"
  if [[ "${T_RC}" -ne 0 ]]; then
    printf '%s\n' "${T_ERR}"
  fi
}

# get NAME OUTPUT: a step output of render NAME.
get() {
  t_output "${2:?}" "${T_ROOT}/${1:?}/output"
}

script="${T_SCRIPTS}/render-inputs.sh"
render port
t_eq "port: exit status" 0 "${T_RC}"
run_claude=$(get port run_claude)
t_eq "port: the agents run" true "${run_claude}"

args=$(get port port_args)
t_has "port args: model" "${args}" "--model claude-opus-5-5"
t_has "port args: fallback model" "${args}" "--fallback-model claude-sonnet-5-5"
t_has "port args: permission mode" "${args}" "--permission-mode default"
t_has "port args: only trusted settings load" "${args}" "--setting-sources user"
t_has "port args: turns" "${args}" "--max-turns 150"
t_has "port args: budget" "${args}" "--max-budget-usd 30"
t_has "port args: context directory" "${args}" "--add-dir ${ctx}"
t_has "port args: the old pin's tree" "${args}" "--add-dir ${old_root}"
t_has "port args: the new pin's tree" "${args}" "--add-dir ${new_root}"
t_has "port args: the probe tree" "${args}" "--add-dir ${probe_root}"
t_has "port args: system prompt" "${args}" "--append-system-prompt-file ${T_PORT}/PORT_SYSTEM.md"
t_has "port args: allow list" "${args}" '--allowedTools "Read,Glob,Grep,Agent,TodoWrite,Edit(./crates/**),Edit(./testdata/**),Edit(./conformance/known_failures.txt),'
t_has "port args: probes only in the probe tree" "${args}" "Edit(/${probe_root}/**/zz_parity_scratch_test.go)"
t_lacks "port args: no probes in the work tree" "${args}" "Edit(./**/zz_parity_scratch_test.go)"
t_has "port args: the pinned rustfmt" "${args}" "Bash(rustfmt +nightly-"
t_has "port args: the Go tooling check" "${args}" "Bash(${T_SCRIPTS}/go-tooling.sh)"
t_lacks "port args: no go vet" "${args}" "go vet"
t_lacks "port args: never bypass permissions" "${args}" "dangerously"
t_lacks "port args: no network tools" "${args}" "WebFetch"
t_lacks "port args: no edits of the upstream checkout" "${args}" "Edit(./.upstream"
schema=$(sed -n "s/^--json-schema '\(.*\)'\$/\1/p" <<< "${args}")
expected_schema=$(jq -c . "${T_PORT}/schemas/port-result.schema.json")
t_eq "port args: the port result schema" "${expected_schema}" "${schema}"
stray=$(grep -c -v -e '^--[A-Za-z-]* ' <<< "${args}" || true)
t_eq "port args: one option per line" 0 "${stray}"

review=$(get port review_args)
t_has "review args: turns" "${review}" "--max-turns 60"
t_has "review args: budget" "${review}" "--max-budget-usd 10"
t_has "review args: system prompt" "${review}" "${T_PORT}/REVIEW_SYSTEM.md"
t_has "review args: adversarial tests only" "${review}" "Edit(./crates/*/tests/adversarial_*.rs)"
t_lacks "review args: no production edits" "${review}" "Edit(./crates/**)"
t_lacks "review args: no subagents" "${review}" ",Agent,"
review_schema=$(sed -n "s/^--json-schema '\(.*\)'\$/\1/p" <<< "${review}")
expected_review=$(jq -c . "${T_PORT}/schemas/review-result.schema.json")
t_eq "review args: the review schema" "${expected_review}" "${review_schema}"
fix=$(get port fix_args)
t_has "fix args: turns" "${fix}" "--max-turns 100"
t_has "fix args: budget" "${fix}" "--max-budget-usd 15"

settings=$(get port port_settings)
t_eq "port settings: in the runner temp" "${T_ROOT}/port/runner/claude-settings-port.json" "${settings}"
checks=$(jq -c '[.attribution.commit, .attribution.pr, .includeCoAuthoredBy, .includeGitInstructions,
  .permissions.defaultMode, .permissions.disableBypassPermissionsMode, .env.PBS_GO_ROOT, .env.CARGO_NET_OFFLINE,
  .env.GOPROXY, .env.GOFLAGS, .env.GOTOOLCHAIN]' "${settings}")
t_eq "port settings: attribution off, offline, no bypass, the new pin's tree" \
  "[\"\",\"\",false,false,\"default\",\"disable\",\"${new_root}\",\"true\",\"off\",\"-mod=readonly\",\"local\"]" "${checks}"
toolchain=$(jq -r .env.RUSTUP_TOOLCHAIN "${settings}")
channel=$(sed -n -E 's/^channel[[:space:]]*=[[:space:]]*"([^"]+)".*$/\1/p' "${T_TOP}/rust-toolchain.toml")
t_eq "port settings: the pinned toolchain" "${channel}" "${toolchain}"
denied=$(jq -r '.permissions.deny | join(" ")' "${settings}")
for rule in "Bash(git commit:*)" "Bash(git push:*)" "Bash(git merge:*)" "Bash(git rebase:*)" "Bash(git reset:*)" \
  "Bash(git -c:*)" "Bash(curl:*)" "Bash(wget:*)" "Bash(gh:*)" "WebFetch" "WebSearch" "mcp__*" \
  "Edit(./.github/**)" "Edit(./Cargo.toml)" "Edit(./scripts/**)" "Edit(./UPSTREAM_SYNC)" "Edit(./.upstream/**)" \
  "Edit(/${old_root}/**)" "Edit(/${new_root}/**)"; do
  t_has "port settings: deny ${rule}" " ${denied} " " ${rule} "
done
repeated=$(jq -r '.permissions.deny | length - (unique | length)' "${settings}")
t_eq "port settings: no deny rule twice" 0 "${repeated}"
allowed=$(jq -r '.permissions.allow | join(" ")' "${settings}")
t_has "port settings: allow list matches the arguments" "${allowed}" "Edit(./crates/**)"
review_settings=$(get port review_settings)
review_allowed=$(jq -r '.permissions.allow | join(" ")' "${review_settings}")
t_lacks "review settings: no production edits" "${review_allowed}" "Edit(./crates/**)"

prompt=$(get port port_prompt)
t_has "port prompt: the injected subject is quoted verbatim" "${prompt}" "${verbatim}"
t_lacks "port prompt: backticks in subjects are neutralised" "${prompt}" "${code_span}"
t_has "port prompt: context directory" "${prompt}" "Context directory (read-only inputs): ${ctx}"
t_has "port prompt: run" "${prompt}" "Run: https://example.invalid/runs/9"
t_has "port prompt: turns" "${prompt}" "at most 150 turns"
t_has "port prompt: adapters" "${prompt}" "must ALL pass when you are done: foo,bar"
t_has "port prompt: conformance script" "${prompt}" "${T_SCRIPTS}/conformance.sh --strict-dirs"
t_has "port prompt: the old tree" "${prompt}" "${old_root}"
t_has "port prompt: the new tree" "${prompt}" "${new_root}"
t_has "port prompt: the probe tree" "${prompt}" "${probe_root}"
t_has "port prompt: the pin's version" "${prompt}" "${version}"
t_has "port prompt: the Go tooling check" "${prompt}" "${T_SCRIPTS}/go-tooling.sh"
# The injected subject is the only text that may still read like a placeholder.
template_text=${prompt//"${verbatim}"/}
for placeholder in CTX RUN_URL BASE_SHA TARGET_SHA START_SHA N_COMMITS COMMIT_TABLE ADAPTER_DIRS MAX_TURNS \
  PR_NUMBER PORT_BRANCH CONFORMANCE RUSTFMT_TOOLCHAIN OLD_GO_ROOT NEW_GO_ROOT PROBE_ROOT PIN_VERSION GO_TOOLING; do
  t_lacks "port prompt: \${${placeholder}} is filled" "${template_text}" "\${${placeholder}}"
done
review_prompt=$(get port review_prompt)
t_has "review prompt: range" "${review_prompt}" "${s1}..${s2}"
t_has "review prompt: the probe tree" "${review_prompt}" "${probe_root}"
fix_prompt=$(get port fix_prompt)
t_has "fix prompt: input file" "${fix_prompt}" "${ctx}/fix-input.md"

render core BUDGET_CLASS=core
core_args=$(get core port_args)
t_has "core: more turns" "${core_args}" "--max-turns 300"
t_has "core: core budget" "${core_args}" "--max-budget-usd 60"

render repair ACTION=repair PR_NUMBER=12 PIN_VERSION=""
t_eq "repair: exit status" 0 "${T_RC}"
repair_args=$(get repair port_args)
t_has "repair: turns" "${repair_args}" "--max-turns 100"
t_has "repair: budget" "${repair_args}" "--max-budget-usd 15"
repair_prompt=$(get repair port_prompt)
t_has "repair: the repair prompt" "${repair_prompt}" "Do NOT port new"
t_has "repair: the pull request" "${repair_prompt}" "#12"

render pinonly RUN_MODE=pin-only
value=$(get pinonly run_claude)
t_eq "pin-only: no agents" false "${value}"
render noneed NEEDS_CLAUDE=false
value=$(get noneed run_claude)
t_eq "no class needs the agents" false "${value}"
printf '{"status": "fail", "failed": ["clippy"], "checks": []}\n' > "${ctx}/gates-before.json"
render clippy NEEDS_CLAUDE=false
value=$(get clippy run_claude)
t_eq "a red check upstream data cannot break does not start the agents" false "${value}"
for failed in conformance test devtools go; do
  printf '{"status": "fail", "failed": ["clippy", "%s"], "checks": []}\n' "${failed}" > "${ctx}/gates-before.json"
  render "broken-${failed}" NEEDS_CLAUDE=false
  value=$(get "broken-${failed}" run_claude)
  t_eq "a ${failed} failure after advancing the pin starts the agents" true "${value}"
done
printf '{"status": "fail", "failed": ["go"], "checks": []}\n' > "${ctx}/gates-before.json"
render pinonlybroken RUN_MODE=pin-only NEEDS_CLAUDE=false
value=$(get pinonlybroken run_claude)
t_eq "pin-only never starts the agents" false "${value}"
rm -f "${ctx}/gates-before.json"

render badmodel PORT_MODEL='claude; rm -rf /'
t_eq "a model name with shell characters is refused" 1 "${T_RC}"
render badbudget FIX_BUDGET_USD=lots
t_eq "a non-numeric budget is refused" 1 "${T_RC}"
render badsha TARGET_SHA=HEAD
t_eq "a symbolic target is refused" 1 "${T_RC}"
render badstart START_SHA=""
t_eq "the start commit is required" 1 "${T_RC}"
render baddirs ADAPTER_DIRS='foo,../etc'
t_eq "an adapter directory with a path is refused" 1 "${T_RC}"
render badtree PROBE_ROOT=relative/probe
t_eq "a relative tree is refused" 1 "${T_RC}"
render notree OLD_GO_ROOT=""
t_eq "the old tree is required" 1 "${T_RC}"
render badversion PIN_VERSION='v4.1.0; echo'
t_eq "a version with shell characters is refused" 1 "${T_RC}"

tooling="${T_ROOT}/tooling"
cp -R "${T_PORT}" "${tooling}"
printf 'Unknown: %s{BOGUS}\n' "\$" >> "${tooling}/prompts/port.md.tmpl"
script="${tooling}/scripts/render-inputs.sh"
render bogus
t_fails "an unknown placeholder is refused"
t_has "an unknown placeholder is named" "${T_ERR}" "unknown placeholder BOGUS"

t_done
