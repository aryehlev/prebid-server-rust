#!/usr/bin/env bash
# sync-go-modules.sh with the real go command and no network: a fixture upstream whose go.mod
# bumps the go line, bumps a dependency and adds one, with the dependencies served by a file://
# module proxy in the test directory. The stale tooling module no longer loads with
# -mod=readonly; after the sync it has the new go line and versions, its go.sum has the new lines
# and it loads offline. The parity tooling module gets the go line and the versions without go mod
# tidy and loses what upstream dropped; a second run changes nothing; and a module that cannot be
# tidied, or that needs a module upstream does not require, fails the run by name.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

if ! command -v go > /dev/null 2>&1; then
  t_fail "this test needs the go command on PATH"
  t_done
fi
proxy="${T_ROOT}/proxy"
export GOTOOLCHAIN=local GOENV=off GOFLAGS="" GOWORK=off GOSUMDB=off GONOSUMDB="" GOPRIVATE="" \
  GONOPROXY="" GOINSECURE="" GOPROXY="file://${proxy}" GOPATH="${T_ROOT}/gopath" \
  GOMODCACHE="${T_ROOT}/gomodcache" GOCACHE="${T_ROOT}/gocache"

# module PATH VERSION: put module PATH at VERSION, one package with func Value, into the proxy.
module() {
  local path=${1:?} version=${2:?} name dir src
  name=${path##*/}
  dir="${proxy}/${path}/@v"
  src="${T_ROOT}/modsrc"
  mkdir -p "${dir}" "${src}/${path}@${version}"
  printf 'module %s\n\ngo 1.21\n' "${path}" > "${src}/${path}@${version}/go.mod"
  printf 'package %s\n\n// Value is the version.\nfunc Value() string { return "%s" }\n' "${name}" "${version}" \
    > "${src}/${path}@${version}/${name}.go"
  cp "${src}/${path}@${version}/go.mod" "${dir}/${version}.mod"
  printf '{"Version":"%s","Time":"2024-01-01T00:00:00Z"}\n' "${version}" > "${dir}/${version}.info"
  printf '%s\n' "${version}" >> "${dir}/list"
  (cd "${src}" && perl -MIO::Compress::Zip=zip,\$ZipError -e '
    my ($out, @files) = @ARGV;
    zip [@files] => $out or die "zip: $ZipError\n";
  ' "${dir}/${version}.zip" "${path}@${version}/go.mod" "${path}@${version}/${name}.go")
}
module example.com/dep v1.0.0
module example.com/dep v1.1.0
module example.com/newdep v1.0.0
module example.com/gone v1.0.0
module example.com/extra v1.0.0

pin_version=v4.0.0-20240101000000-abcdefabcdef
repo="${T_ROOT}/repo"
t_repo "${repo}"
printf '/.upstream/\n' > "${repo}/.gitignore"
up="${repo}/.upstream"
mkdir -p "${up}/util"
printf 'module github.com/prebid/prebid-server/v4\n\ngo 1.22.0\n\nrequire (\n\texample.com/dep v1.0.0\n\texample.com/gone v1.0.0\n)\n' \
  > "${up}/go.mod"
printf 'package util\n\nimport (\n\t"example.com/dep"\n\t"example.com/gone"\n)\n\n// V is what the tools print.\nfunc V() string { return dep.Value() + gone.Value() }\n' \
  > "${up}/util/util.go"

# tooling DIR REQUIRE...: a Go tooling module at DIR that requires the upstream module (replaced
# by .upstream) and the modules REQUIRE ("path version"), with go 1.22.0.
tooling() {
  local dir=${1:?} up_path="" depth require
  shift
  depth=$(tr -cd '/' <<< "${dir}" | wc -c)
  depth=$((depth + 1))
  while [[ "${depth}" -gt 0 ]]; do
    up_path+="../"
    depth=$((depth - 1))
  done
  mkdir -p "${repo}/${dir}"
  {
    printf 'module github.com/aryehlev/prebid-server-rust/%s\n\ngo 1.22.0\n\nrequire github.com/prebid/prebid-server/v4 %s\n' \
      "${dir}" "${pin_version}"
    for require in "$@"; do
      printf '\nrequire %s // indirect\n' "${require}"
    done
    printf '\nreplace github.com/prebid/prebid-server/v4 => %s.upstream\n' "${up_path}"
  } > "${repo}/${dir}/go.mod"
}

x=crates/x/testdata/go
tooling "${x}" "example.com/dep v1.0.0" "example.com/gone v1.0.0"
printf 'package main\n\nimport "github.com/prebid/prebid-server/v4/util"\n\nfunc main() { println(util.V()) }\n' \
  > "${repo}/${x}/main.go"
plain=crates/devtools/go
mkdir -p "${repo}/${plain}"
printf 'module github.com/aryehlev/prebid-server-rust/%s\n\ngo 1.22.0\n' "${plain}" > "${repo}/${plain}/go.mod"
printf 'package main\n\nfunc main() {}\n' > "${repo}/${plain}/main.go"
parity=crates/parity/testdata/go
tooling "${parity}" "example.com/dep v1.0.0" "example.com/gone v1.0.0"
printf 'package main\n\nimport "github.com/prebid/prebid-server/v4/parity/trace"\n\nfunc main() { println(trace.V()) }\n' \
  > "${repo}/${parity}/main.go"
printf 'example.com/old v1.0.0 h1:parity-only\n' > "${repo}/${parity}/go.sum"
# The modules as the pin left them: tidy, with their go.sum.
(cd "${repo}/${x}" && go mod tidy) > /dev/null 2>&1
t_commit "${repo}" "tooling at the old pin" > /dev/null
start=$(git -C "${repo}" rev-parse HEAD)

# A clean module stays clean.
t_run_in "${repo}" "${T_SCRIPTS}/sync-go-modules.sh"
t_eq "in step: exit status" 0 "${T_RC}"
t_has "in step: nothing to do" "${T_OUT}" "every Go tooling module already follows the upstream go.mod (go 1.22.0)"
status=$(git -C "${repo}" status --porcelain --untracked-files=all)
t_eq "in step: nothing changes" "" "${status}"

# The new pin: go 1.23.0, dep bumped, newdep added, gone dropped.
printf 'module github.com/prebid/prebid-server/v4\n\ngo 1.23.0\n\nrequire (\n\texample.com/dep v1.1.0\n\texample.com/newdep v1.0.0\n)\n' \
  > "${up}/go.mod"
printf 'package util\n\nimport (\n\t"example.com/dep"\n\t"example.com/newdep"\n)\n\n// V is what the tools print.\nfunc V() string { return dep.Value() + newdep.Value() }\n' \
  > "${up}/util/util.go"
t_run_in "${repo}/${x}" env GOPROXY=off GOFLAGS=-mod=readonly go list -deps ./...
t_fails "before the sync: the stale module does not load with -mod=readonly"

ctx="${T_ROOT}/ctx"
t_run_in "${repo}" "${T_SCRIPTS}/sync-go-modules.sh" --out "${ctx}"
t_eq "sync: exit status" 0 "${T_RC}"
sync_out=${T_OUT}
x_mod=$(cd "${repo}/${x}" && go mod edit -json)
x_state=$(jq -c '[.Go, .Toolchain, [.Require[] | "\(.Path) \(.Version)\(if .Indirect then " indirect" else "" end)"], [.Replace[] | "\(.Old.Path) => \(.New.Path)"]]' <<< "${x_mod}")
t_eq "sync: the go line and the versions follow upstream; the pin and the replace stay" \
  "[\"1.23.0\",null,[\"github.com/prebid/prebid-server/v4 ${pin_version}\",\"example.com/dep v1.1.0 indirect\",\"example.com/newdep v1.0.0 indirect\"],[\"github.com/prebid/prebid-server/v4 => ../../../../.upstream\"]]" \
  "${x_state}"
x_sum=$(cat "${repo}/${x}/go.sum")
t_has "sync: go.sum has the bumped dependency" "${x_sum}" "example.com/dep v1.1.0 h1:"
t_has "sync: go.sum has the new dependency" "${x_sum}" "example.com/newdep v1.0.0/go.mod h1:"
t_lacks "sync: go.sum drops the dropped dependency" "${x_sum}" "example.com/gone"
t_run_in "${repo}/${x}" env GOPROXY=off GOFLAGS=-mod=readonly go list -deps ./...
t_eq "sync: the module loads offline with -mod=readonly" 0 "${T_RC}"
plain_go=$(cd "${repo}/${plain}" && go mod edit -json | jq -r .Go)
t_eq "sync: a module without requirements gets the go line" 1.23.0 "${plain_go}"
parity_mod=$(cd "${repo}/${parity}" && go mod edit -json)
parity_state=$(jq -c '[.Go, [.Require[] | "\(.Path) \(.Version)"]]' <<< "${parity_mod}")
t_eq "sync: the parity module gets the go line and the versions, and loses what upstream dropped" \
  "[\"1.23.0\",[\"github.com/prebid/prebid-server/v4 ${pin_version}\",\"example.com/dep v1.1.0\"]]" "${parity_state}"
t_has "sync: the parity module is not tidied, with a notice" "${sync_out}" \
  "::notice::sync-go-modules: crates/parity/testdata/go imports the parity packages"
parity_sum=$(cat "${repo}/${parity}/go.sum")
t_eq "sync: the parity module's go.sum is left alone" "example.com/old v1.0.0 h1:parity-only" "${parity_sum}"
summary=$(cat "${ctx}/go-sync.txt")
t_eq "sync: the summary for the commit message" \
  "Go tooling modules synced with the upstream go.mod (3 module(s) changed):
  example.com/dep v1.0.0 -> v1.1.0
  example.com/gone v1.0.0 (dropped)
  example.com/newdep v1.0.0 (added)
  go 1.22.0 -> 1.23.0" "${summary}"
t_has "sync: the summary is printed" "${sync_out}" "${summary}"
expected_files=""
for path in "${plain}/go.mod" "${parity}/go.mod" "${x}/go.mod" "${x}/go.sum"; do
  expected_files+="${path}"$'\t'"$(git -C "${repo}" hash-object -- "${path}")"$'\n'
done
files=$(sort "${ctx}/go-sync-files.tsv")
expected_sorted=$(sort <<< "${expected_files%$'\n'}")
t_eq "sync: every file it changed, with the blob it left" "${expected_sorted}" "${files}"
synced=$(git -C "${repo}" status --porcelain --untracked-files=all)

# Again: nothing changes, and nothing is reported as changed.
t_run_in "${repo}" "${T_SCRIPTS}/sync-go-modules.sh" --out "${ctx}"
t_eq "again: exit status" 0 "${T_RC}"
t_has "again: nothing to do" "${T_OUT}" "already follows the upstream go.mod (go 1.23.0)"
again=$(git -C "${repo}" status --porcelain --untracked-files=all)
t_eq "again: the work tree is as the first run left it" "${synced}" "${again}"
again_summary=$(cat "${ctx}/go-sync.txt")
t_eq "again: an empty summary" "" "${again_summary}"
again_files=$(cat "${ctx}/go-sync-files.tsv")
t_eq "again: no file changed" "" "${again_files}"

# A module that cannot be tidied, and one that needs a module upstream does not require.
git -C "${repo}" reset -q --hard "${start}"
broken=crates/broken/testdata/go
tooling "${broken}"
printf 'package main\n\nimport "example.com/missing"\n\nfunc main() { println(missing.Value()) }\n' \
  > "${repo}/${broken}/main.go"
extra=crates/extra/testdata/go
tooling "${extra}"
printf 'package main\n\nimport "example.com/extra"\n\nfunc main() { println(extra.Value()) }\n' \
  > "${repo}/${extra}/main.go"
t_run_in "${repo}" "${T_SCRIPTS}/sync-go-modules.sh" --out "${ctx}"
t_eq "failure: exit status" 1 "${T_RC}"
t_has "failure: tidy fails loudly in the module" "${T_ERR}" "::error::sync-go-modules: go mod tidy failed in ${broken}:"
t_has "failure: with go's own message" "${T_ERR}" "example.com/missing"
t_has "failure: a requirement upstream does not have" "${T_ERR}" \
  "${extra} still does not follow upstream go.mod after the sync: example.com/extra v1.0.0 (upstream does not require it)"
t_has "failure: the run names every failed module" "${T_ERR}" \
  "2 Go tooling module(s) could not be brought in line with the upstream go.mod (go 1.23.0): ${broken} ${extra}"
x_go=$(cd "${repo}/${x}" && go mod edit -json | jq -r .Go)
t_eq "failure: the other modules are still synced" 1.23.0 "${x_go}"

rm -rf "${up}"
t_run_in "${repo}" "${T_SCRIPTS}/sync-go-modules.sh"
t_eq "no upstream checkout: exit status" 1 "${T_RC}"
t_has "no upstream checkout: explained" "${T_ERR}" "run scripts/fetch-upstream.sh first"
t_run_in "${repo}" "${T_SCRIPTS}/sync-go-modules.sh" --bogus
t_eq "an unknown argument: usage" 2 "${T_RC}"

t_done
