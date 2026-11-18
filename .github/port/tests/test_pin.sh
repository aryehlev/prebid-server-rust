#!/usr/bin/env bash
# The pin scripts: upstream-mirror.sh (the bare mirror of the upstream branch and tags),
# pin-version.sh (the Go pseudo-version of an upstream commit, with the go command's tag rules),
# upstream-pin.sh (the pinned commit, read the way scripts/fetch-upstream.sh reads it) and
# advance-pin.sh (UPSTREAM_SYNC and the Go tooling modules' upstream requirement).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

up="${T_ROOT}/upstream"
t_repo "${up}"

# at DATE MESSAGE: commit everything in the upstream repository with committer date DATE.
at() {
  t_put "${up}" "notes/${2:?}.txt" "${2}"
  git -C "${up}" add -A
  env GIT_COMMITTER_DATE="${1:?}" git -C "${up}" commit -q -m "${2}"
  git -C "${up}" rev-parse HEAD
}

# version SHA: pin-version.sh's answer for SHA, or the failure status.
version() {
  t_run_in "${T_ROOT}" "${T_SCRIPTS}/pin-version.sh" "${1:?}"
  if [[ "${T_RC}" -ne 0 ]]; then
    printf 'exit %s: %s\n' "${T_RC}" "${T_ERR}"
  else
    printf '%s\n' "${T_OUT}"
  fi
}

# upstream-mirror.sh
c1=$(at "2026-01-02T03:04:05+02:00" c1)
export PBS_UPSTREAM_URL="${up}"
mirror="${T_ROOT}/mirror.git"
t_run_in "${T_ROOT}" "${T_SCRIPTS}/upstream-mirror.sh" "${mirror}"
t_eq "mirror: exit status" 0 "${T_RC}"
t_eq "mirror: prints its absolute path" "${mirror}" "${T_OUT}"
bare=$(git --git-dir="${mirror}" rev-parse --is-bare-repository)
t_eq "mirror: a bare repository" true "${bare}"
head=$(git --git-dir="${mirror}" rev-parse refs/heads/master)
t_eq "mirror: the upstream branch" "${c1}" "${head}"
t_run_in "${T_ROOT}" "${T_SCRIPTS}/upstream-mirror.sh" "${T_ROOT}/upstream"
t_eq "mirror: a work tree is refused" 1 "${T_RC}"
t_run_in "${T_ROOT}" env UPSTREAM_BRANCH='master;x' "${T_SCRIPTS}/upstream-mirror.sh" "${mirror}"
t_eq "mirror: an odd branch name is refused" 1 "${T_RC}"
t_run_in "${T_ROOT}" env UPSTREAM_BRANCH=nope "${T_SCRIPTS}/upstream-mirror.sh" "${T_ROOT}/other.git"
t_fails "mirror: a missing branch fails"
t_run_in "${T_ROOT}" "${T_SCRIPTS}/upstream-mirror.sh"
t_eq "mirror: the directory is required" 2 "${T_RC}"
export UPSTREAM_GIT="${mirror}"

# pin-version.sh
stamp=20260102010405
value=$(version "${c1}")
t_eq "version: no tag, the commit time in UTC" "v4.0.0-${stamp}-${c1:0:12}" "${value}"
git -C "${up}" tag v3.9.0 "${c1}"
git -C "${up}" tag v4.1 "${c1}"
git -C "${up}" tag v4.01.0 "${c1}"
"${T_SCRIPTS}/upstream-mirror.sh" "${mirror}" > /dev/null 2>&1
value=$(version "${c1}")
t_eq "version: other majors, shorthand and invalid tags are ignored" "v4.0.0-${stamp}-${c1:0:12}" "${value}"
git -C "${up}" tag v4.0.0 "${c1}"
c2=$(at "2026-02-01T00:00:00Z" c2)
"${T_SCRIPTS}/upstream-mirror.sh" "${mirror}" > /dev/null 2>&1
value=$(version "${c2}")
t_eq "version: after a release" "v4.0.1-0.20260201000000-${c2:0:12}" "${value}"
git -C "${up}" tag v4.1.0 "${c2}"
git -C "${up}" tag v4.0.5 "${c2}"
c3=$(at "2026-03-01T00:00:00Z" c3)
"${T_SCRIPTS}/upstream-mirror.sh" "${mirror}" > /dev/null 2>&1
value=$(version "${c3}")
t_eq "version: the highest release" "v4.1.1-0.20260301000000-${c3:0:12}" "${value}"
git -C "${up}" tag v4.2.0-rc.2 "${c3}"
git -C "${up}" tag v4.2.0-rc.10 "${c3}"
git -C "${up}" tag v4.2.0-beta "${c3}"
c4=$(at "2026-04-01T00:00:00Z" c4)
"${T_SCRIPTS}/upstream-mirror.sh" "${mirror}" > /dev/null 2>&1
value=$(version "${c4}")
t_eq "version: after a prerelease (numeric identifiers compare as numbers)" \
  "v4.2.0-rc.10.0.20260401000000-${c4:0:12}" "${value}"
git -C "${up}" tag v4.2.0 "${c4}"
git -C "${up}" tag v4.2.5+meta "${c4}"
git -C "${up}" tag v4.5.0-0.20200101000000-abcdefabcdef "${c4}"
git -C "${up}" tag v4.6.0-20200101000000-abc "${c4}"
c5=$(at "2026-05-01T00:00:00Z" c5)
git -C "${up}" switch -q -c side "${c4}"
side=$(at "2026-05-02T00:00:00Z" side)
git -C "${up}" tag v4.9.0 "${side}"
git -C "${up}" switch -q master
"${T_SCRIPTS}/upstream-mirror.sh" "${mirror}" > /dev/null 2>&1
value=$(version "${c5}")
t_eq "version: build metadata dropped, pseudo-version tags and unmerged tags ignored" \
  "v4.6.0-20200101000000-abc.0.20260501000000-${c5:0:12}" "${value}"
git -C "${up}" tag -d v4.6.0-20200101000000-abc > /dev/null
git --git-dir="${mirror}" tag -d v4.6.0-20200101000000-abc > /dev/null
value=$(version "${c5}")
t_eq "version: without the prerelease tag" "v4.2.6-0.20260501000000-${c5:0:12}" "${value}"
: > "${T_ROOT}/output"
t_run_in "${T_ROOT}" env GITHUB_OUTPUT="${T_ROOT}/output" "${T_SCRIPTS}/pin-version.sh" "${c5}"
output=$(t_output version "${T_ROOT}/output")
t_eq "version: the step output" "v4.2.6-0.20260501000000-${c5:0:12}" "${output}"
value=$(version "${c5:0:12}")
t_has "version: a short commit is refused" "${value}" "exit 2"
value=$(version 0000000000000000000000000000000000000000)
t_has "version: a commit the mirror lacks is refused" "${value}" "is not in the upstream mirror"

# upstream-pin.sh
repo="${T_ROOT}/repo"
t_repo "${repo}"
printf '# The pin.\n\n  %s  \n' "${c5}" > "${repo}/UPSTREAM_SYNC"
: > "${T_ROOT}/pin-output"
t_run_in "${repo}" env GITHUB_OUTPUT="${T_ROOT}/pin-output" "${T_SCRIPTS}/upstream-pin.sh"
t_eq "pin: the pinned commit" "${c5}" "${T_OUT}"
output=$(t_output sha "${T_ROOT}/pin-output")
t_eq "pin: the step output" "${c5}" "${output}"
printf '%s\n%s\n' "${c4}" "${c5}" > "${repo}/UPSTREAM_SYNC"
t_run_in "${repo}" "${T_SCRIPTS}/upstream-pin.sh"
t_eq "pin: two pins are refused" 1 "${T_RC}"
printf '%s\n' "${c5:0:12}" > "${repo}/UPSTREAM_SYNC"
t_run_in "${repo}" "${T_SCRIPTS}/upstream-pin.sh"
t_eq "pin: a short commit is refused" 1 "${T_RC}"

# advance-pin.sh
old=$(version "${c4}")
new=$(version "${c5}")
printf '# The pin.\n# Only the port advances it.\n%s\n' "${c4}" > "${repo}/UPSTREAM_SYNC"
mkdir -p "${repo}/crates/a/testdata/go" "${repo}/crates/b/testdata/go" "${repo}/crates/c/testdata/go" \
  "${repo}/crates/d/target/go" "${repo}/crates/e/.hidden"
cat > "${repo}/crates/a/testdata/go/go.mod" << EOF
module github.com/aryehlev/prebid-server-rust/crates/a/testdata/go

go 1.25.0

require (
	github.com/prebid/prebid-server/v4 ${old} // the pin
	github.com/prebid/prebid-server/v4x v4.0.0
	github.com/stretchr/testify v1.8.1
)

replace github.com/prebid/prebid-server/v4 => ../../../../.upstream
EOF
t_go_mod crates/b/testdata/go "${old}" > "${repo}/crates/b/testdata/go/go.mod"
printf 'module github.com/aryehlev/prebid-server-rust/crates/c/testdata/go\n\ngo 1.25.0\n' \
  > "${repo}/crates/c/testdata/go/go.mod"
t_go_mod crates/d/target/go "${old}" > "${repo}/crates/d/target/go/go.mod"
t_go_mod crates/e/.hidden "${old}" > "${repo}/crates/e/.hidden/go.mod"
t_commit "${repo}" "tooling" > /dev/null
t_run_in "${repo}" "${T_SCRIPTS}/advance-pin.sh" "${c5}" "${new}"
t_eq "advance: exit status" 0 "${T_RC}"
t_has "advance: the changed modules are counted" "${T_OUT}" "2 tooling go.mod file(s) differ from HEAD"
pin=$(cat "${repo}/UPSTREAM_SYNC")
t_eq "advance: the pin, with its comment lines" $'# The pin.\n# Only the port advances it.\n'"${c5}" "${pin}"
a_mod=$(cat "${repo}/crates/a/testdata/go/go.mod")
t_has "advance: a block requirement, comment kept" "${a_mod}" $'\tgithub.com/prebid/prebid-server/v4 '"${new}"' // the pin'
t_has "advance: another module is left alone" "${a_mod}" $'\tgithub.com/prebid/prebid-server/v4x v4.0.0'
t_has "advance: the replace directive is left alone" "${a_mod}" \
  "replace github.com/prebid/prebid-server/v4 => ../../../../.upstream"
b_mod=$(grep '^require' "${repo}/crates/b/testdata/go/go.mod")
t_eq "advance: a single-line requirement" "require github.com/prebid/prebid-server/v4 ${new}" "${b_mod}"
changed=$(git -C "${repo}" status --porcelain)
t_eq "advance: only the pin and the requiring modules change" \
  $' M UPSTREAM_SYNC\n M crates/a/testdata/go/go.mod\n M crates/b/testdata/go/go.mod' "${changed}"
before=$(git -C "${repo}" diff)
t_run_in "${repo}" "${T_SCRIPTS}/advance-pin.sh" "${c5}" "${new}"
after=$(git -C "${repo}" diff)
t_eq "advance: running it again changes nothing" "${before}" "${after}"
t_run_in "${repo}" "${T_SCRIPTS}/advance-pin.sh" "${c5}" "${old}"
t_eq "advance: a version of another commit is refused" 1 "${T_RC}"
t_run_in "${repo}" "${T_SCRIPTS}/advance-pin.sh" "${c5}" "v4.0.0-${stamp}-${c5:0:12}; echo"
t_eq "advance: a malformed version is refused" 1 "${T_RC}"
t_run_in "${repo}" "${T_SCRIPTS}/advance-pin.sh" "${c5:0:12}" "${new}"
t_eq "advance: a short commit is refused" 2 "${T_RC}"
rm "${repo}/UPSTREAM_SYNC"
t_run_in "${repo}" "${T_SCRIPTS}/advance-pin.sh" "${c5}" "${new}"
t_eq "advance: UPSTREAM_SYNC is required" 1 "${T_RC}"

t_done
