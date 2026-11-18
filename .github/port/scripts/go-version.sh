#!/usr/bin/env bash
# go-version.sh [GO_ROOT]: print the Go version the upstream Dockerfile builds the production
# server with, and set it as the step output "version" when GITHUB_OUTPUT is set. CI and the port
# job install that version, so Go-side ground truth comes from the toolchain production uses.
#
# The Dockerfile is read from the upstream checkout: GO_ROOT, else PBS_GO_ROOT, else .upstream at
# the top of the work tree (run scripts/fetch-upstream.sh first).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
top=$(pwd)
go_root=${1:-${PBS_GO_ROOT:-${top}/.upstream}}
dockerfile="${go_root}/Dockerfile"
if [[ ! -f "${dockerfile}" ]]; then
  port_die "go-version: ${dockerfile} does not exist; run scripts/fetch-upstream.sh first"
fi
downloads=$(sed -n -E 's#.*dl\.google\.com/go/go([0-9]+\.[0-9]+(\.[0-9]+)?)\.linux-amd64\.tar\.gz.*#\1#p' "${dockerfile}")
version=$(head -n 1 <<< "${downloads}")
if [[ -z "${version}" ]]; then
  port_die "go-version: ${dockerfile} has no go<version>.linux-amd64.tar.gz download"
fi
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  port_output version "${version}"
fi
printf '%s\n' "${version}"
