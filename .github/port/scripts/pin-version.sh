#!/usr/bin/env bash
# pin-version.sh SHA: print the Go pseudo-version of upstream commit SHA, the version every Go
# tooling module requires the upstream module at (pbs-devtools checks it), and set it as the step
# output "version" when GITHUB_OUTPUT is set.
#
# The version is the one the go command derives for a commit without a release tag of its own
# (cmd/go/internal/modfetch, golang.org/x/mod/module.PseudoVersion): the base is the highest tag
# of major version 4 merged into SHA that the go command accepts as a version, compared the way
# golang.org/x/mod/semver compares (a prerelease sorts before its release); shorthand tags such as
# v4.1 and tags that look like pseudo-versions are ignored, and the base's build metadata is
# dropped, as semver.Canonical drops it. The time is SHA's committer time in UTC and the revision
# SHA's first 12 hex digits.
#   no such tag              v4.0.0-<time>-<rev>
#   base vX.Y.Z              vX.Y.(Z+1)-0.<time>-<rev>
#   base vX.Y.Z-<pre>        vX.Y.Z-<pre>.0.<time>-<rev>
# Retractions are not consulted (upstream retracts no version). A replaced requirement accepts any
# valid version, so the version only has to name the pin, which pbs-devtools checks.
#
# Env: UPSTREAM_GIT, the bare upstream mirror (upstream-mirror.sh).
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

sha=${1:-}
if [[ "$#" -ne 1 || ! "${sha}" =~ ^[0-9a-f]{40}$ ]]; then
  printf 'usage: pin-version.sh SHA   (a full 40-character upstream commit)\n' >&2
  exit 2
fi
if ! git --git-dir="${UPSTREAM_GIT:?}" cat-file -e "${sha}^{commit}" 2> /dev/null; then
  port_die "pin-version: ${sha} is not in the upstream mirror ${UPSTREAM_GIT}"
fi
stamp=$(env TZ=UTC git --git-dir="${UPSTREAM_GIT}" show -s --format=%cd --date=format-local:%Y%m%d%H%M%S "${sha}")
if [[ ! "${stamp}" =~ ^[0-9]{14}$ ]]; then
  port_die "pin-version: unexpected commit time '${stamp}' of ${sha}"
fi
tags=$(port_up tag --merged "${sha}" --list 'v4.*')
version=$(PIN_TAGS=${tags} PIN_STAMP=${stamp} PIN_REV=${sha:0:12} perl -e '
use strict;
use warnings;

my $num = qr/(?:0|[1-9][0-9]*)/;
my $ident = qr/(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)/;
my $build_ident = qr/[0-9A-Za-z-]+/;
# golang.org/x/mod/module.IsPseudoVersion, applied to an already valid version.
my $pseudo = qr/\Av[0-9]+\.(?:0\.0-|[0-9]+\.[0-9]+-(?:[^+]*\.)?0\.)[0-9]{14}-[A-Za-z0-9]+(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?\z/;

# parse TAG: [major, minor, patch, [prerelease identifiers]] of a full semantic version of major
# version 4 (build metadata allowed and dropped) that does not look like a pseudo-version, else
# nothing.
sub parse {
  my ($tag) = @_;
  my ($major, $minor, $patch, $pre) = $tag =~
    /\Av($num)\.($num)\.($num)(?:-($ident(?:\.$ident)*))?(?:\+$build_ident(?:\.$build_ident)*)?\z/
    or return;
  return if $major ne "4" || $tag =~ $pseudo;
  return [$major, $minor, $patch, [defined $pre ? split(/\./, $pre) : ()]];
}

sub cmp_ident {
  my ($x, $y) = @_;
  my ($xn, $yn) = ($x =~ /\A[0-9]+\z/, $y =~ /\A[0-9]+\z/);
  return $x <=> $y if $xn && $yn;
  return -1 if $xn;
  return 1 if $yn;
  return $x cmp $y;
}

# semver.Compare.
sub cmp_version {
  my ($x, $y) = @_;
  for my $i (0 .. 2) {
    my $c = $x->[$i] <=> $y->[$i];
    return $c if $c;
  }
  my ($xp, $yp) = ($x->[3], $y->[3]);
  return 0 if !@$xp && !@$yp;
  return 1 if !@$xp;
  return -1 if !@$yp;
  for my $i (0 .. ($#$xp < $#$yp ? $#$xp : $#$yp)) {
    my $c = cmp_ident($xp->[$i], $yp->[$i]);
    return $c if $c;
  }
  return @$xp <=> @$yp;
}

my $best;
for my $tag (split /\n/, $ENV{PIN_TAGS}) {
  my $v = parse($tag) or next;
  $best = $v if !defined $best || cmp_version($v, $best) > 0;
}
my $segment = "$ENV{PIN_STAMP}-$ENV{PIN_REV}";
if (!defined $best) {
  print "v4.0.0-$segment\n";
  exit 0;
}
my ($major, $minor, $patch, $pre) = @$best;
if (@$pre) {
  print "v$major.$minor.$patch-" . join(".", @$pre) . ".0.$segment\n";
} else {
  print "v$major.$minor." . ($patch + 1) . "-0.$segment\n";
}
')
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  port_output version "${version}"
fi
printf '%s\n' "${version}"
