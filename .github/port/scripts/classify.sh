#!/usr/bin/env bash
# classify.sh FROM TO: print one JSON object describing the Go-side change FROM..TO (normally an
# upstream commit against its first parent):
#
#   {"classes": [...], "adapter_dirs": [...], "bidders": [...], "go_lines": N,
#    "needs_claude": true|false, "heavy": true|false}
#
# classes
#   new-adapter        Go code of an adapter directory that did not exist in FROM
#   adapter-change     Go code of an existing adapter, or its openrtb_ext/imp_<bidder>.go params
#   adapter-fixture    JSON fixtures or other files under adapters/<dir>/<dir>test/
#   adapter-other      any other file in an adapter directory
#   adapter-framework  adapters/*.go or adapters/adapterstest/ (also counts as core)
#   bidder-alias       a new static/bidder-info/<name>.yaml with aliasOf
#   bidder-info        another static/bidder-info/*.yaml change
#   bidder-params      static/bidder-params/*.json
#   registration       openrtb_ext/bidders.go or exchange/adapter_builders.go
#   core               other non-test Go code
#   module             modules/
#   openrtb-bump       a go.mod change of github.com/prebid/openrtb (also counts as core)
#   deps               other go.mod / go.sum changes
#   go-test-only       Go tests and Go test data outside adapters
#   data               other files under static/
#   docs-infra         documentation, build and repository tooling
#   other              anything else
# adapter_dirs  adapter directories whose fixtures the change can affect (aliases resolve to their
#               parent adapter)
# bidders       bidder names whose bidder-info or bidder-params changed
# go_lines      changed lines of non-test Go code
# needs_claude  a class that needs Rust changes is present
# heavy         core, module or openrtb-bump, or more than HEAVY_GO_LINES (default 1500) go_lines;
#               a heavy commit is ported in a chunk of its own
set -euo pipefail
shopt -s inherit_errexit

if [[ "$#" -ne 2 ]]; then
  printf 'usage: classify.sh FROM TO\n' >&2
  exit 2
fi
from=$(git rev-parse --verify --end-of-options "${1}^{commit}")
to=$(git rev-parse --verify --end-of-options "${2}^{commit}")
heavy_lines=${HEAVY_GO_LINES:-1500}
if [[ ! "${heavy_lines}" =~ ^[0-9]+$ ]]; then
  printf 'classify: HEAVY_GO_LINES must be a number\n' >&2
  exit 2
fi

declare -A classes=() dirs=() bidders=()

exists() {
  local rev=${1:?} path=${2:?}
  if git cat-file -e "${rev}:${path}" 2> /dev/null; then
    printf 'yes\n'
  else
    printf 'no\n'
  fi
}

# is_bidder_dir NAME: "yes" when adapters/NAME is a bidder adapter (it has a <name>test fixture
# directory or a bidder-info file) in FROM or TO.
is_bidder_dir() {
  local name=${1:?} rev
  if [[ "${name}" == adapterstest ]]; then
    printf 'no\n'
    return
  fi
  for rev in "${to}" "${from}"; do
    if git cat-file -e "${rev}:adapters/${name}/${name}test" 2> /dev/null \
      || git cat-file -e "${rev}:static/bidder-info/${name}.yaml" 2> /dev/null; then
      printf 'yes\n'
      return
    fi
  done
  printf 'no\n'
}

# alias_of PATH: the aliasOf value of the bidder-info file PATH in TO (empty if none).
alias_of() {
  local text
  text=$(git show "${to}:${1:?}" 2> /dev/null) || text=""
  sed -n -E 's/^aliasOf:[[:space:]]*"?([A-Za-z0-9_]+)"?[[:space:]]*(#.*)?$/\1/p' <<< "${text}" | head -n 1
}

core() {
  classes[core]=1
  classes[${1:?}]=1
}

classify_path() {
  local status=${1:?} path=${2:?} dir base parent bidder
  case "${path}" in
    adapters/*/*)
      dir=${path#adapters/}
      dir=${dir%%/*}
      bidder=$(is_bidder_dir "${dir}")
      if [[ "${bidder}" != yes ]]; then
        core adapter-framework
        return
      fi
      if [[ "${dir}" =~ ^[A-Za-z0-9_]+$ ]]; then
        dirs[${dir}]=1
      fi
      case "${path}" in
        adapters/"${dir}"/"${dir}"test/* | *.json) classes[adapter-fixture]=1 ;;
        *_test.go) classes[go-test-only]=1 ;;
        *.go)
          parent=$(exists "${from}" "adapters/${dir}")
          if [[ "${parent}" == yes ]]; then
            classes[adapter-change]=1
          else
            classes[new-adapter]=1
          fi
          ;;
        *) classes[adapter-other]=1 ;;
      esac
      ;;
    adapters/*.go) core adapter-framework ;;
    static/bidder-info/*.yaml)
      base=${path##*/}
      base=${base%.yaml}
      if [[ "${base}" =~ ^[A-Za-z0-9_]+$ ]]; then
        bidders[${base}]=1
      fi
      parent=""
      if [[ "${status}" != D ]]; then
        parent=$(alias_of "${path}")
      fi
      if [[ -n "${parent}" ]]; then
        dirs[${parent}]=1
        if [[ "${status}" == A ]]; then
          classes[bidder-alias]=1
        else
          classes[bidder-info]=1
        fi
      else
        classes[bidder-info]=1
        bidder=$(exists "${to}" "adapters/${base}")
        if [[ "${bidder}" == yes && "${base}" =~ ^[A-Za-z0-9_]+$ ]]; then
          dirs[${base}]=1
        fi
      fi
      ;;
    static/bidder-params/*.json)
      base=${path##*/}
      base=${base%.json}
      if [[ "${base}" =~ ^[A-Za-z0-9_]+$ ]]; then
        bidders[${base}]=1
      fi
      classes[bidder-params]=1
      ;;
    openrtb_ext/imp_*.go) classes[adapter-change]=1 ;;
    openrtb_ext/bidders.go | exchange/adapter_builders.go) classes[registration]=1 ;;
    *_test.go | */testdata/* | endpoints/openrtb2/sample-requests/* | exchange/exchangetest/*)
      classes[go-test-only]=1
      ;;
    go.mod)
      gomod=$(git diff "${from}" "${to}" -- go.mod)
      # Any changed line naming the module: a require block entry, a one-line require or a replace.
      if grep -Eq '^[+-].*github\.com/prebid/openrtb/' <<< "${gomod}"; then
        core openrtb-bump
      else
        classes[deps]=1
      fi
      ;;
    go.sum) classes[deps]=1 ;;
    modules/*) classes[module]=1 ;;
    docs/* | *.md | .github/* | .devcontainer/* | sample/* | scripts/* | Makefile | Dockerfile \
      | LICENSE | NOTICE | validate.sh | .gitignore | .dockerignore | .gitattributes | .editorconfig)
      classes[docs-infra]=1
      ;;
    static/*) classes[data]=1 ;;
    *.go) classes[core]=1 ;;
    *) classes[other]=1 ;;
  esac
}

tmp=$(mktemp -d)
trap 'rm -rf -- "${tmp}"' EXIT
git diff -z --name-status --no-renames "${from}" "${to}" > "${tmp}/changes"
while IFS= read -r -d '' status && IFS= read -r -d '' path; do
  classify_path "${status:0:1}" "${path}"
done < "${tmp}/changes"

numstat=$(git diff --numstat --no-renames "${from}" "${to}" -- '*.go' ':(exclude)*_test.go')
go_lines=$(awk '$1 ~ /^[0-9]+$/ { total += $1 + $2 } END { print total + 0 }' <<< "${numstat}")

needs=false
for class in new-adapter adapter-change adapter-other adapter-framework bidder-alias registration core module other; do
  if [[ -n "${classes[${class}]:-}" ]]; then
    needs=true
  fi
done
heavy=false
for class in core module openrtb-bump; do
  if [[ -n "${classes[${class}]:-}" ]]; then
    heavy=true
  fi
done
if [[ "${go_lines}" -gt "${heavy_lines}" ]]; then
  heavy=true
fi

class_lines=$(printf '%s\n' "${!classes[@]}")
dir_lines=$(printf '%s\n' "${!dirs[@]}")
bidder_lines=$(printf '%s\n' "${!bidders[@]}")
jq -n -c \
  --arg classes "${class_lines}" --arg dirs "${dir_lines}" --arg bidders "${bidder_lines}" \
  --argjson go_lines "${go_lines}" --argjson needs "${needs}" --argjson heavy "${heavy}" \
  'def names: split("\n") | map(select(length > 0)) | unique;
   {classes: ($classes | names), adapter_dirs: ($dirs | names), bidders: ($bidders | names),
    go_lines: $go_lines, needs_claude: $needs, heavy: $heavy}'
