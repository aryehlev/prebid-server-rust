#!/usr/bin/env bash
# classify.sh: real upstream commits from this repository's history, and synthetic changes for the
# classes those commits do not cover.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

classify="${T_SCRIPTS}/classify.sh"

# real SHA EXPECTED_JSON: classify the real upstream commit SHA against its first parent.
real() {
  local sha=${1:?} expected=${2:?} actual
  if ! git -C "${T_TOP}" cat-file -e "${sha}^{commit}" 2> /dev/null; then
    t_fail "upstream commit ${sha} is missing; run the tests in a clone with full history"
    return
  fi
  t_run_in "${T_TOP}" "${classify}" "${sha}^1" "${sha}"
  actual=${T_OUT}
  t_eq "upstream ${sha}" "${expected}" "${actual}"
}

real 4e296450 '{"classes":["adapter-change","adapter-fixture","bidder-info","bidder-params","go-test-only","new-adapter","registration"],"adapter_dirs":["panxo"],"bidders":["panxo"],"go_lines":417,"needs_claude":true,"heavy":false}'
real 361cd6c9 '{"classes":["bidder-info"],"adapter_dirs":["superedge"],"bidders":["superedge"],"go_lines":0,"needs_claude":false,"heavy":false}'
real 585d74d5 '{"classes":["deps"],"adapter_dirs":[],"bidders":[],"go_lines":0,"needs_claude":false,"heavy":false}'
real f1e9d0d2 '{"classes":["core","deps","docs-infra","go-test-only"],"adapter_dirs":[],"bidders":[],"go_lines":1513,"needs_claude":true,"heavy":true}'
real 0b91679f '{"classes":["adapter-change","adapter-fixture","bidder-info","bidder-params","go-test-only"],"adapter_dirs":["missena"],"bidders":["missena"],"go_lines":219,"needs_claude":true,"heavy":false}'
real 066bbe70 '{"classes":["go-test-only","module"],"adapter_dirs":[],"bidders":[],"go_lines":741,"needs_claude":true,"heavy":true}'

repo="${T_ROOT}/repo"
t_repo "${repo}"
t_put "${repo}" adapters/foo/foo.go 'package foo'
t_put "${repo}" adapters/foo/footest/exemplary/a.json '{}'
t_put "${repo}" adapters/bidder.go 'package adapters'
t_put "${repo}" static/bidder-info/foo.yaml 'endpoint: "http://foo"'
t_put "${repo}" static/bidder-info/gone.yaml 'aliasOf: foo'
t_put "${repo}" go.mod "$(printf 'module x\n\nrequire github.com/prebid/openrtb/v20 v20.3.0\nrequire github.com/other/thing v1.0.0')"
t_put "${repo}" modules/m/m.go 'package m'
base=$(t_commit "${repo}" "base")

# change NAME EXPECTED_JSON: classify the work-tree changes against base, then reset.
change() {
  local name=${1:?} expected=${2:?} head
  head=$(t_commit "${repo}" "${name}")
  t_run_in "${repo}" "${classify}" "${base}" "${head}"
  t_eq "${name}" "${expected}" "${T_OUT}"
  git -C "${repo}" reset -q --hard "${base}"
}

t_put "${repo}" adapters/adapterstest/test_json.go 'package adapterstest // changed'
t_put "${repo}" adapters/bidder.go 'package adapters // changed'
change "the adapter framework is core" \
  '{"classes":["adapter-framework","core"],"adapter_dirs":[],"bidders":[],"go_lines":3,"needs_claude":true,"heavy":true}'

t_put "${repo}" static/bidder-info/fooalias.yaml 'aliasOf: "foo" # a comment'
change "a new alias resolves to its adapter" \
  '{"classes":["bidder-alias"],"adapter_dirs":["foo"],"bidders":["fooalias"],"go_lines":0,"needs_claude":true,"heavy":false}'

git -C "${repo}" rm -q static/bidder-info/gone.yaml
change "a deleted alias" \
  '{"classes":["bidder-info"],"adapter_dirs":[],"bidders":["gone"],"go_lines":0,"needs_claude":false,"heavy":false}'

t_put "${repo}" go.mod "$(printf 'module x\n\nrequire github.com/prebid/openrtb/v20 v20.4.0\nrequire github.com/other/thing v1.0.0')"
change "an openrtb bump is core" \
  '{"classes":["core","openrtb-bump"],"adapter_dirs":[],"bidders":[],"go_lines":0,"needs_claude":true,"heavy":true}'

t_put "${repo}" go.mod "$(printf 'module x\n\nrequire (\n\tgithub.com/prebid/openrtb/v20 v20.3.0\n\tgithub.com/other/thing v1.0.0\n)')"
base_block=$(t_commit "${repo}" "require block")
t_put "${repo}" go.mod "$(printf 'module x\n\nrequire (\n\tgithub.com/prebid/openrtb/v20 v20.4.0\n\tgithub.com/other/thing v1.0.0\n)')"
head=$(t_commit "${repo}" "openrtb bump in a require block")
t_run_in "${repo}" "${classify}" "${base_block}" "${head}"
classes=$(jq -c '.classes' <<< "${T_OUT}")
t_eq "an openrtb bump in a require block is core" '["core","openrtb-bump"]' "${classes}"
git -C "${repo}" reset -q --hard "${base}"

t_put "${repo}" go.mod "$(printf 'module x\n\nrequire github.com/prebid/openrtb/v20 v20.3.0\nrequire github.com/other/thing v1.1.0')"
change "another dependency bump" \
  '{"classes":["deps"],"adapter_dirs":[],"bidders":[],"go_lines":0,"needs_claude":false,"heavy":false}'

t_put "${repo}" modules/m/m.go 'package m // changed'
change "a module change" \
  '{"classes":["module"],"adapter_dirs":[],"bidders":[],"go_lines":2,"needs_claude":true,"heavy":true}'

t_put "${repo}" adapters/foo/README.md 'notes'
t_put "${repo}" adapters/foo/foo_test.go 'package foo'
change "other adapter files" \
  '{"classes":["adapter-other","go-test-only"],"adapter_dirs":["foo"],"bidders":[],"go_lines":0,"needs_claude":true,"heavy":false}'

t_put "${repo}" static/category-mapping/x.json '{}'
t_put "${repo}" docs/x.md 'doc'
t_put "${repo}" weird.bin 'x'
change "data, docs and other files" \
  '{"classes":["data","docs-infra","other"],"adapter_dirs":[],"bidders":[],"go_lines":0,"needs_claude":true,"heavy":false}'

t_put "${repo}" adapters/foo/foo.go "$(printf 'package foo\n// a\n// b\n// c')"
head=$(t_commit "${repo}" "a big adapter change")
t_run_in "${repo}" env HEAVY_GO_LINES=2 "${classify}" "${base}" "${head}"
heavy=$(jq -r '.heavy' <<< "${T_OUT}")
t_eq "HEAVY_GO_LINES makes a large change heavy" true "${heavy}"
git -C "${repo}" reset -q --hard "${base}"

t_run_in "${repo}" "${classify}" "${base}"
t_eq "a missing argument is a usage error" 2 "${T_RC}"
t_run_in "${repo}" env HEAVY_GO_LINES=lots "${classify}" "${base}" "${base}"
t_eq "a bad HEAVY_GO_LINES is a usage error" 2 "${T_RC}"

t_done
