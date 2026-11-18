#!/usr/bin/env bash
# sync-go-modules.sh: bring every Go tooling module under crates/ in line with the go.mod of the
# upstream commit the work tree pins, the rules pbs-devtools' tooling_modules_follow_upstream test
# checks: the upstream go line, no toolchain line, and every module a tooling module requires at
# the version upstream requires it at. Then go mod tidy in each module adds what the new upstream
# code needs, drops what it no longer needs and writes the go.sum lines, so that the modules build
# again with GOFLAGS=-mod=readonly. Running it again changes nothing.
#
# Usage: sync-go-modules.sh [--out DIR]
#   --out DIR  write DIR/go-sync.txt, a summary of what the sync changed for the commit message
#              (empty when nothing changed), and DIR/go-sync-files.tsv, "<path>\t<blob>" for every
#              go.mod and go.sum this run changed, so commit.sh can tell those changes from changes
#              the agents made
#
# The port job runs it after advance-pin.sh and before the agent sessions, as a trusted step with
# network access (go mod tidy downloads what the new pin requires); the agents have no network and
# no go mod, so they could not do it. The Go root is the new pin's checkout: PBS_GO_ROOT when it is
# set, else .upstream, where the modules' replace directives point. advance-pin.sh has already set
# the version the modules require the upstream module at; the sync never changes that version or
# the replace directive.
#
# crates/parity/testdata/go imports the parity packages, which only the parity-harness branch has,
# so go mod tidy cannot load it here: it gets the go line and the upstream versions, and loses the
# requirements upstream no longer has (the parity packages build against upstream's go.mod), with a
# notice. It builds in workspace mode against the assembled parity tree (ci-parity.sh, its run.sh),
# whose go.sum stands in for its own.
#
# It fails, naming every such module, when go mod tidy fails in a module, or when a module still
# breaks the rules after tidying: it requires a module upstream does not, or one at another
# version, or it has a toolchain line.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

out=""
while [[ "$#" -gt 0 ]]; do
  case "${1}" in
    --out)
      if [[ "$#" -lt 2 || -z "${2}" ]]; then
        port_die "sync-go-modules: --out needs a directory"
      fi
      out=${2}
      shift 2
      ;;
    *)
      printf 'usage: sync-go-modules.sh [--out DIR]\n' >&2
      exit 2
      ;;
  esac
done

port_cd_top
top=$(pwd)
go_root=${PBS_GO_ROOT:-${top}/.upstream}
if [[ ! -f "${go_root}/go.mod" ]]; then
  port_die "sync-go-modules: ${go_root} is not an upstream checkout; run scripts/fetch-upstream.sh first"
fi
module=$(port_go_module)
parity_module=crates/parity/testdata/go
upstream=$(go mod edit -json "${go_root}/go.mod")
upstream_go=$(jq -r '.Go // ""' <<< "${upstream}")
if [[ -z "${upstream_go}" ]]; then
  port_die "sync-go-modules: ${go_root}/go.mod has no go line"
fi
# The versions upstream requires, "path version" per line.
upstream_requires=$(jq -r '.Require[]? | "\(.Path) \(.Version)"' <<< "${upstream}")
declare -A wanted=()
while read -r path version; do
  if [[ -n "${path}" ]]; then
    wanted[${path}]=${version}
  fi
done <<< "${upstream_requires}"

work=$(mktemp -d)
trap 'rm -rf -- "${work}"' EXIT
: > "${work}/changes"
: > "${work}/files"

# requires DIR: "path version" per line, sorted, of what DIR's go.mod requires, the upstream module
# left out.
requires() {
  local json
  json=$(cd "${1:?}" && go mod edit -json)
  jq -r --arg module "${module}" '.Require[]? | select(.Path != $module) | "\(.Path) \(.Version)"' <<< "${json}" | sort
}

# field DIR KEY: the go.mod field KEY (Go, Toolchain) of DIR's go.mod, empty when it has none.
field() {
  local json
  json=$(cd "${1:?}" && go mod edit -json)
  jq -r --arg key "${2:?}" '.[$key] // ""' <<< "${json}"
}

# blob FILE: the git blob of FILE, or - when it does not exist.
blob() {
  if [[ -f "${1:?}" ]]; then
    git hash-object -- "${1}"
  else
    printf '%s\n' -
  fi
}

listing=$(find crates \( -name target -o -name '.*' \) -prune -o -type f -name go.mod -print | sort)
failed=()
changed=0
while IFS= read -r gomod; do
  if [[ -z "${gomod}" ]]; then
    continue
  fi
  dir=${gomod%/go.mod}
  before=$(requires "${dir}")
  before_go=$(field "${dir}" Go)
  before_mod=$(blob "${dir}/go.mod")
  before_sum=$(blob "${dir}/go.sum")
  edits=("-go=${upstream_go}" -toolchain=none)
  while read -r path version; do
    if [[ -z "${path}" ]]; then
      continue
    fi
    if [[ -n "${wanted[${path}]:-}" && "${wanted[${path}]}" != "${version}" ]]; then
      edits+=("-require=${path}@${wanted[${path}]}")
    elif [[ -z "${wanted[${path}]:-}" && "${dir}" == "${parity_module}" ]]; then
      edits+=("-droprequire=${path}")
    fi
  done <<< "${before}"
  (cd "${dir}" && go mod edit "${edits[@]}")
  if [[ "${dir}" == "${parity_module}" ]]; then
    printf '::notice::sync-go-modules: %s imports the parity packages, which only the parity-harness branch has, so it gets the upstream go line and versions without go mod tidy; it builds in workspace mode against the assembled parity tree\n' "${dir}"
  elif ! (cd "${dir}" && env GOFLAGS= GOWORK=off go mod tidy) > "${work}/tidy.log" 2>&1; then
    port_error "sync-go-modules: go mod tidy failed in ${dir}:"
    cat "${work}/tidy.log" >&2
    failed+=("${dir}")
    continue
  fi
  after=$(requires "${dir}")
  # What the module must not keep: a module upstream does not require, another version of one, or
  # a toolchain line.
  wrong=""
  while read -r path version; do
    if [[ -z "${path}" ]]; then
      continue
    fi
    if [[ -z "${wanted[${path}]:-}" ]]; then
      wrong+="${path} ${version} (upstream does not require it); "
    elif [[ "${wanted[${path}]}" != "${version}" ]]; then
      wrong+="${path} ${version} (upstream requires ${wanted[${path}]}); "
    fi
  done <<< "${after}"
  toolchain=$(field "${dir}" Toolchain)
  if [[ -n "${toolchain}" ]]; then
    wrong+="toolchain ${toolchain}; "
  fi
  if [[ -n "${wrong}" ]]; then
    port_error "sync-go-modules: ${dir} still does not follow upstream go.mod after the sync: ${wrong%; }"
    failed+=("${dir}")
    continue
  fi
  module_changed=0
  for file in go.mod go.sum; do
    case "${file}" in
      go.mod) previous=${before_mod} ;;
      *) previous=${before_sum} ;;
    esac
    current=$(blob "${dir}/${file}")
    if [[ "${current}" != "${previous}" ]]; then
      printf '%s\t%s\n' "${dir}/${file}" "${current}" >> "${work}/files"
      module_changed=1
    fi
  done
  if [[ "${module_changed}" -eq 0 ]]; then
    continue
  fi
  changed=$((changed + 1))
  if [[ "${before_go}" != "${upstream_go}" ]]; then
    printf 'go %s -> %s\n' "${before_go}" "${upstream_go}" >> "${work}/changes"
  fi
  # Per module path: its old and its new version, - when it was not or is no longer required.
  printf '%s\n' "${before}" | sed '/^$/d' > "${work}/before"
  printf '%s\n' "${after}" | sed '/^$/d' > "${work}/after"
  join -a 1 -a 2 -e - -o 0,1.2,2.2 "${work}/before" "${work}/after" \
    | awk '$2 == "-" { print $1 " " $3 " (added)"; next }
           $3 == "-" { print $1 " " $2 " (dropped)"; next }
           $2 != $3 { print $1 " " $2 " -> " $3 }' >> "${work}/changes"
done <<< "${listing}"

if [[ "${#failed[@]}" -gt 0 ]]; then
  port_die "sync-go-modules: ${#failed[@]} Go tooling module(s) could not be brought in line with the upstream go.mod (go ${upstream_go}): ${failed[*]}"
fi

summary=""
changes=$(sort -u "${work}/changes")
if [[ "${changed}" -gt 0 ]]; then
  summary="Go tooling modules synced with the upstream go.mod (${changed} module(s) changed):"
  if [[ -z "${changes}" ]]; then
    changes="go.sum lines only"
  fi
  while IFS= read -r line; do
    summary+=$'\n'"  ${line}"
  done <<< "${changes}"
fi
if [[ -n "${out}" ]]; then
  mkdir -p "${out}"
  if [[ -n "${summary}" ]]; then
    printf '%s\n' "${summary}" > "${out}/go-sync.txt"
  else
    : > "${out}/go-sync.txt"
  fi
  cp "${work}/files" "${out}/go-sync-files.tsv"
fi
if [[ -n "${summary}" ]]; then
  printf '%s\n' "${summary}"
else
  printf 'sync-go-modules: every Go tooling module already follows the upstream go.mod (go %s)\n' "${upstream_go}"
fi
