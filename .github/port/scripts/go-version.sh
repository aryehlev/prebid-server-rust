#!/usr/bin/env bash
# go-version.sh: print the Go version the upstream Dockerfile builds the production server with,
# and set it as the step output "version" when GITHUB_OUTPUT is set. CI and the port job install
# that version, so Go-side ground truth comes from the toolchain production uses.
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

port_cd_top
downloads=$(sed -n -E 's#.*dl\.google\.com/go/go([0-9]+\.[0-9]+(\.[0-9]+)?)\.linux-amd64\.tar\.gz.*#\1#p' Dockerfile)
version=$(head -n 1 <<< "${downloads}")
if [[ -z "${version}" ]]; then
  port_die "go-version: the Dockerfile has no go<version>.linux-amd64.tar.gz download"
fi
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  port_output version "${version}"
fi
printf '%s\n' "${version}"
