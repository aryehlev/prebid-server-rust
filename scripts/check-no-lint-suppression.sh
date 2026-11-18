#!/usr/bin/env bash
# check-no-lint-suppression.sh: fail when code the fork authors suppresses, weakens or hides a lint,
# formatter or analyzer finding. The policy is to fix the finding; nothing may silence it.
#
# Usage:
#   scripts/check-no-lint-suppression.sh             scan the git work tree (tracked files and
#                                                    untracked files that are not ignored)
#   scripts/check-no-lint-suppression.sh --root DIR  scan every file under DIR, laid out like the
#                                                    repository (used by scripts/tests/run.sh)
#
# Scope: every file of the repository, which holds only the Rust port and its tooling. Only build
# output (target/) is skipped. The upstream Go server is never scanned: it is the .upstream/
# checkout that scripts/fetch-upstream.sh makes, which git ignores and a --root scan skips.
#
# Rules (the bracketed id is printed with every finding):
#   Rust sources, with comments and string literals removed first
#     rust-lint-attr        #[allow(..)], #![allow(..)], #[expect(..)], #![expect(..)], including the
#                           raw-identifier forms #[r#allow(..)] / #[r#expect(..)]
#     rust-cfg-attr-lint    cfg_attr(.., allow(..)) or cfg_attr(.., expect(..)), at any nesting
#     rustfmt-skip          rustfmt::skip in any form (raw identifiers too), and the old rustfmt_skip
#     rust-cfg-tool         cfg(..)/cfg_attr(..)/cfg!(..) predicates on clippy or rustfmt
#     rust-include-legacy   #[path] or include!() that pulls legacy/ code (the Rust code that preceded
#                           the port) into a crate
#     rust-include          include!() or #[path] naming anything but a .rs file by a plain string
#                           literal (generated code in OUT_DIR, other extensions): code no scan reads
#     rust-macro-attr       an attribute a macro assembles from its input, which launders a lint
#                           suppression past the literal scan above and clippy's allow_attributes
#                           lint (neither sees inside macro expansions): #[$x] / #![$x] with a
#                           metavariable the macro arm does not capture whole as #[$x:meta],
#                           #[$(..)*], a metavariable as an attribute of cfg_attr(pred, $x), and
#                           quote! interpolation #[#x] / cfg_attr(pred, #x) in proc macros
#     rust-macro-string     a string literal in a proc-macro crate that names or spells a
#                           suppressing attribute (Ident::new("expect", ..), parse_str("#[allow(..)]"))
#     rust-clippy-msrv      #[clippy::msrv]: a lower Rust version turns off the lints that suggest
#                           newer features
#     rust-auto-derived     #[automatically_derived], which makes rustc and clippy skip lints
#   Cargo.toml (parsed with taplo, so every TOML spelling is covered)
#     lint-level            any [lints] or [workspace.lints] level other than warn, deny or forbid
#     crate-lints           a package whose [lints] is not exactly `workspace = true`
#     machete-ignore        package.metadata/workspace.metadata cargo-machete `ignored` lists
#     rust-version          a package rust-version that is not `workspace = true`, or a workspace
#                           rust-version other than the rust-toolchain.toml channel (clippy takes
#                           it as the MSRV and turns off the lints for newer features)
#   other configuration (the one allowed file of each kind is at the repository root)
#     toml-parse            a TOML file the checks above cannot parse
#     clippy-config         clippy configs other than clippy.toml (a nested one replaces it for the
#                           crates below it), and allow-* / *-allowed-* and msrv options
#     rust-toolchain        toolchain files other than rust-toolchain.toml (a legacy rust-toolchain
#                           wins over it and can pick a toolchain without lints)
#     rustfmt-config        rustfmt configs other than rustfmt.toml, and options that skip files
#     deny-config           cargo-deny configs other than deny.toml, and ignore/skip/exception lists
#     cargo-config          rustflags, rustc / rustc-wrapper / rustc-workspace-wrapper / runner
#                           overrides, lint-related [env] entries (including RUSTC_WRAPPER), build.
#                           warnings and lint-command aliases in .cargo/config and .cargo/config.toml
#   Go code we author
#     go-nolint             nolint, lint:ignore, lint:file-ignore, nosec and revive:disable comments
#     go-build-tag          //go:build and // +build constraints, which keep a file out of the
#                           golangci-lint and go vet runs
#     golangci-config       golangci-lint configs other than .github/linters/golangci.yml, and
#                           exclusion settings, test-file skipping or a non-`all` default in it
#   shell scripts, workflow files and other .github YAML
#     sh-directive          directives for the shell linter that disable checks or extended analysis
#     sh-rc                 .shellcheckrc entries that disable checks or extended analysis
#     sh-options            shell linter options that exclude checks, include only some, or
#                           lower the severity
#     rustflags-env         compiler flag variables that can lower lint levels
#     rustdocflags-env      the rustdoc flags variable set to anything but "-D warnings"
#     cargo-warnings-env    cargo's build-warnings variable set to anything but deny
#     lint-flag             allow, warn, force-warn or cap-lints flags on a cargo or rustc command
#     actionlint-ignore     actionlint's ignore flag, and ignore entries in an actionlint config
#     ci-continue-on-error  continue-on-error in .github/workflows/ci.yml
#
# Requires bash 4+, perl 5, jq and taplo.
set -euo pipefail
shopt -s inherit_errexit

root=""
mode=git
while [[ $# -gt 0 ]]; do
  case "$1" in
    --root)
      root=${2:?--root needs a directory}
      mode=tree
      shift 2
      ;;
    -h | --help)
      sed -n '2,/^# Requires/p' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *)
      printf 'check-no-lint-suppression: unknown argument %s\n' "$1" >&2
      exit 2
      ;;
  esac
done

for tool in perl jq taplo; do
  if ! command -v "${tool}" > /dev/null; then
    printf 'check-no-lint-suppression: %s is required but not installed\n' "${tool}" >&2
    exit 2
  fi
done

if [[ "${mode}" == git ]]; then
  here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  root=$(git -C "${here}" rev-parse --show-toplevel)
fi
root=$(cd "${root}" && pwd)

work=$(mktemp -d)
trap 'rm -rf -- "${work}"' EXIT

# 1. Collect the candidate files as repository-relative paths: every file but build output and, in
#    a --root scan, the fetched upstream checkout (.gitignore keeps it out of the git listing).
if [[ "${mode}" == git ]]; then
  git -C "${root}" ls-files -z --cached --others --exclude-standard > "${work}/all"
else
  (cd "${root}" && find . -type d \( -name .git -o -name target -o -path ./.upstream \) -prune \
    -o -type f -print0) | perl -0 -pe 's{\A\./}{}' > "${work}/all"
fi

# 2. Classify them. Each output line is "<kind>\t<path>".
perl -0 -ne '
  chomp;
  my $p = $_;
  next if $p =~ m{(?:\A|/)target/};
  my ($base) = $p =~ m{([^/]+)\z};
  my $kind;
  if    ($p =~ /\.rs\z/)                                      { $kind = "rust" }
  elsif ($base eq "Cargo.toml")                               { $kind = "cargo" }
  elsif ($base eq "clippy.toml" || $base eq ".clippy.toml")   { $kind = "clippy" }
  elsif ($base eq "rustfmt.toml" || $base eq ".rustfmt.toml") { $kind = "rustfmt" }
  elsif ($base eq "deny.toml")                                { $kind = "deny" }
  elsif ($base eq "rust-toolchain" || $base eq "rust-toolchain.toml") { $kind = "toolchain" }
  elsif ($p =~ m{(?:\A|/)\.cargo/config(?:\.toml)?\z})        { $kind = "cargoconfig" }
  elsif ($p =~ /\.go\z/)                                      { $kind = "go" }
  elsif ($base =~ /\A\.?golangci\.(?:ya?ml|toml|json)\z/)     { $kind = "golangci" }
  elsif ($p =~ /\.(?:sh|bash)\z/)                             { $kind = "shell" }
  elsif ($base eq ".shellcheckrc")                            { $kind = "shellcheckrc" }
  elsif ($p =~ m{\A\.github/actionlint\.ya?ml\z})             { $kind = "actionlint" }
  elsif ($p =~ m{\A\.github/.*\.ya?ml\z})                     { $kind = "yaml" }
  else                                                        { next }
  print "$kind\t$p\n";
' "${work}/all" > "${work}/classified"

: > "${work}/findings"

# 3. Text rules (Rust, Go, shell, YAML). One perl process reads every file.
perl -e '
use strict;
use warnings;

my ($root, $out) = @ARGV;
open(my $fh_out, ">>", $out) or die "cannot write $out: $!";

sub finding {
  my ($path, $line, $rule, $msg) = @_;
  print {$fh_out} "$path:$line: [$rule] $msg\n";
}

sub line_at {
  my ($text, $pos) = @_;
  return (substr($text, 0, $pos) =~ tr/\n//) + 1;
}

sub blank {
  my ($t) = @_;
  $t =~ s/[^\n]/ /g;
  return $t;
}

# Rust source with comments, string literals and char literals replaced by spaces. Newlines are
# kept, so offsets and line numbers are unchanged. The string literals are pushed to the optional
# array reference, each as [offset, literal].
sub rust_code_only {
  my ($s, $strings) = @_;
  my $len = length $s;
  my $out = "";
  pos($s) = 0;
  while (pos($s) < $len) {
    if ($s =~ m{\G//[^\n]*}gc) { $out .= blank($&); next; }
    if ($s =~ m{\G/\*}gc) {
      my $start = pos($s) - 2;
      my $depth = 1;
      while ($depth > 0 && $s =~ m{\G(?:[^/*]+|/\*|\*/|[/*])}gc) {
        if ($& eq "/*") { $depth++; } elsif ($& eq "*/") { $depth--; }
      }
      $out .= blank(substr($s, $start, pos($s) - $start));
      next;
    }
    if ($s =~ m{\G[bc]?r(\#*)"}gc) {
      my $start = $-[0];
      my $close = "\"" . $1;
      my $end = index($s, $close, pos($s));
      $end = $end < 0 ? $len : $end + length($close);
      push @{$strings}, [$start, substr($s, $start, $end - $start)] if $strings;
      $out .= blank(substr($s, $start, $end - $start));
      pos($s) = $end;
      next;
    }
    if ($s =~ m{\G"(?:[^"\\]|\\.)*"?}gcs) {
      push @{$strings}, [$-[0], $&] if $strings;
      $out .= blank($&);
      next;
    }
    if ($s =~ m{\G\x27(?:\\(?:x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f]{1,6}\}|.)|[^\\\x27\n])\x27}gcs) {
      $out .= blank($&);
      next;
    }
    if ($s =~ m{\G(?:[A-Za-z0-9_]+|\s+|.)}gcs) { $out .= $&; next; }
  }
  return $out;
}

my $balanced = qr{(\((?:[^()]++|(?-1))*\))};
# A token tree of code without comments and literals: (..), [..] or {..}, nested. Patterns use it
# as (?&tt) and end with $tt_def.
my $tt_def = qr{(?(DEFINE)(?<tt>\((?:[^()\[\]{}]++|(?&tt))*+\)|\[(?:[^()\[\]{}]++|(?&tt))*+\]|\{(?:[^()\[\]{}]++|(?&tt))*+\}))};

# The arms of every macro_rules! definition, nested ones too: the offsets of the matcher and of the
# transcriber, the metavariables the matcher binds, and those it binds as a whole attribute the
# caller writes with #[..]: #[$x:meta], or #[$($x:tt)*].
sub macro_arms {
  my ($code) = @_;
  my @arms;
  while ($code =~ m{\bmacro_rules\s*!\s*(?:r\#)?\w+\s*(?=((?&tt)))$tt_def}g) {
    my ($body, $base) = ($1, $-[1] + 1);
    my $inner = substr($body, 1, -1);
    while ($inner =~ m{\G\s*((?&tt))\s*=>\s*((?&tt))\s*;?$tt_def}gc) {
      my ($matcher, $mfrom, $mto, $from, $to) = ($1, $base + $-[1], $base + $+[1], $base + $-[2], $base + $+[2]);
      my (%bound, %whole);
      $bound{$1} = 1 while $matcher =~ m{\$(\w+)\s*:\s*\w}g;
      $whole{$1} = 1 while $matcher =~ m{\#\s*\[\s*\$(\w+)\s*:}g;
      $whole{$1} = 1 while $matcher =~ m{\#\s*\[\s*\$\s*\(\s*\$(\w+)\s*:}g;
      push @arms, {mfrom => $mfrom, mto => $mto, from => $from, to => $to, bound => \%bound, whole => \%whole};
    }
  }
  return \@arms;
}

# Whether offset $at is inside a macro matcher, where #[$x:meta] captures instead of applying.
sub in_matcher {
  my ($arms, $at) = @_;
  return scalar grep { $_->{mfrom} <= $at && $at < $_->{mto} } @{$arms};
}

# Whether metavariable $name at offset $at stands for a whole attribute the caller wrote with
# #[..] (which the literal scan sees at the call site): the innermost macro arm around $at that
# binds $name binds it as #[$name:meta].
sub whole_attribute {
  my ($arms, $at, $name) = @_;
  my @around = sort { ($a->{to} - $a->{from}) <=> ($b->{to} - $b->{from}) }
    grep { $_->{from} <= $at && $at < $_->{to} } @{$arms};
  for my $arm (@around) {
    return $arm->{whole}{$name} ? 1 : 0 if $arm->{bound}{$name};
  }
  return 0;
}

sub check_rust {
  my ($path, $src, $proc_macro) = @_;
  my @strings;
  my $code = rust_code_only($src, \@strings);
  # The optional r\# catches raw identifiers: #[r#allow(..)] and #[r#expect(..)] suppress a lint
  # exactly like #[allow(..)]/#[expect(..)], and the clippy allow_attributes lint does not flag
  # them, so the text scan is the only backstop.
  while ($code =~ m{\#\s*!?\s*\[\s*(?:r\#)?(allow|expect)\s*\(}g) {
    finding($path, line_at($code, $-[0]), "rust-lint-attr", "#[$1(..)] suppresses a lint; fix the code instead");
  }
  while ($code =~ m{\bcfg_attr\s*$balanced}g) {
    my ($args, $at) = ($1, $-[0]);
    if ($args =~ m{\b(allow|expect)\s*\(}) {
      finding($path, line_at($code, $at), "rust-cfg-attr-lint", "cfg_attr(.., $1(..)) suppresses a lint; fix the code instead");
    }
  }
  while ($code =~ m{\b(?:r\#)?rustfmt\s*::\s*(?:r\#)?skip\b|\b(?:r\#)?rustfmt_skip\b}g) {
    finding($path, line_at($code, $-[0]), "rustfmt-skip", "rustfmt::skip hides code from the formatter");
  }
  while ($code =~ m{\bcfg(_attr)?\s*!?\s*$balanced}g) {
    my ($is_attr, $args, $at) = ($1, $2, $-[0]);
    my $predicate = $args;
    if (defined $is_attr) {
      # cfg_attr(predicate, attributes..): only the first top-level argument is a predicate.
      $predicate = $args =~ m{\A\(((?:[^(),]++|$balanced)*)} ? $1 : $args;
    }
    if ($predicate =~ m{\b(clippy|rustfmt)\b}) {
      finding($path, line_at($code, $at), "rust-cfg-tool", "a cfg predicate on $1 hides code from that tool");
    }
  }
  # The path itself is a string literal, so find the construct in the code and read the path from
  # the same offset in the original source.
  while ($code =~ m{\#\s*!?\s*\[\s*path\s*=|\binclude(?:_str|_bytes)?\s*!\s*\(}g) {
    my $at = $-[0];
    my $eol = index($src, "\n", $at);
    $eol = length($src) if $eol < 0;
    if (substr($src, $at, $eol - $at) =~ m{\blegacy/}) {
      finding($path, line_at($code, $at), "rust-include-legacy", "legacy/ code, which preceded the port, must not be compiled into a crate");
    }
  }
  # include! and #[path] may only name a .rs file with a plain literal: anything else (generated
  # code in OUT_DIR, a file with another extension) is compiled but never scanned.
  while ($code =~ m{\binclude\s*!\s*\(}g) {
    my ($at, $rest) = ($-[0], substr($src, $+[0]));
    if ($rest !~ m{\A\s*(?:r\#*)?"[^"\n]*\.rs"\#*\s*,?\s*\)}) {
      finding($path, line_at($code, $at), "rust-include", "include! of anything but a .rs file named by a string literal compiles code this scan never reads");
    }
  }
  while ($code =~ m{\#\s*!?\s*\[\s*path\s*=}g) {
    my ($at, $rest) = ($-[0], substr($src, $+[0]));
    if ($rest !~ m{\A\s*(?:r\#*)?"[^"\n]*\.rs"\#*\s*\]}) {
      finding($path, line_at($code, $at), "rust-include", "#[path] to anything but a .rs file compiles code this scan never reads");
    }
  }
  while ($code =~ m{\bclippy\s*::\s*msrv\b}g) {
    finding($path, line_at($code, $-[0]), "rust-clippy-msrv", "#[clippy::msrv] lowers the Rust version clippy assumes, which turns off the lints that suggest newer features");
  }
  while ($code =~ m{\bautomatically_derived\b}g) {
    finding($path, line_at($code, $-[0]), "rust-auto-derived", "#[automatically_derived] makes rustc and clippy skip lints on this code");
  }
  # An attribute a macro assembles from its input launders a suppression: the caller passes
  # allow(..)/expect(..) as bare tokens and the macro wraps them in #[..], which neither the literal
  # scan above nor clippy (it does not lint attributes from macro expansions) sees. The legitimate
  # form forwards a whole attribute the caller already wrote with #[..], captured by the same macro
  # arm as #[$x:meta]; anything else is flagged, except $crate (the defining crate, which no caller
  # controls).
  my $arms = macro_arms($code);
  my $why = "so a lint suppression can be laundered through it; capture a whole attribute as #[\$x:meta] or write a fixed attribute";
  while ($code =~ m{\#\s*!?\s*\[\s*\$(\w+)\b}g) {
    my ($name, $at) = ($1, $-[0]);
    next if $name eq "crate" || in_matcher($arms, $at) || whole_attribute($arms, $at, $name);
    finding($path, line_at($code, $at), "rust-macro-attr", "#[\$$name] applies a caller-supplied attribute name, $why");
  }
  while ($code =~ m{\#\s*!?\s*\[\s*\$\s*\(\s*(?:\$(\w+))?}g) {
    my ($name, $at) = ($1, $-[0]);
    next if in_matcher($arms, $at) || (defined $name && whole_attribute($arms, $at, $name));
    finding($path, line_at($code, $at), "rust-macro-attr", "#[\$(..)] assembles an attribute from caller tokens, $why");
  }
  while ($code =~ m{\#\s*!?\s*\[\s*\#}g) {
    finding($path, line_at($code, $-[0]), "rust-macro-attr", "#[#..] interpolates the attribute of generated code, which no scan sees; write a fixed attribute");
  }
  while ($code =~ m{\bcfg_attr\s*(?=$balanced)}g) {
    my ($args, $at) = ($1, $-[0]);
    next if in_matcher($arms, $at);
    my $inner = substr($args, 1, -1);
    my @items;
    while ($inner =~ m{\G\s*((?:[^(),]++|$balanced)*+)(?:,|\z)}gc) {
      push @items, $1;
      last if pos($inner) >= length($inner);
    }
    shift @items;
    for my $item (@items) {
      if ($item =~ m{\A\s*\$(\w+)}) {
        my $name = $1;
        next if $name eq "crate" || whole_attribute($arms, $at, $name);
        finding($path, line_at($code, $at), "rust-macro-attr", "cfg_attr(.., \$$name) applies a caller-supplied attribute, $why");
      } elsif ($item =~ m{\A\s*[\$\#]}) {
        finding($path, line_at($code, $at), "rust-macro-attr", "cfg_attr(.., ..) applies an attribute assembled from macro input, $why");
      }
    }
  }
  # A proc macro can build the attribute from strings instead: Ident::new("expect", ..),
  # format_ident!("allow"), syn::parse_str("#[allow(..)]").
  if ($proc_macro) {
    for my $literal (@strings) {
      my ($at, $text) = @{$literal};
      if ($text =~ m{\A[bc]?r?\#*"\s*(?:allow|expect|automatically_derived|rustfmt|msrv|cfg_attr)\s*"\#*\z}
          || $text =~ m{\b(?:allow|expect)\s*\(|\brustfmt\s*::\s*skip\b|\bclippy\s*::\s*msrv\b|\bautomatically_derived\b}) {
        finding($path, line_at($code, $at), "rust-macro-string", "a string literal that names a suppressing attribute lets a proc macro generate it where no scan sees; write a fixed attribute");
      }
    }
  }
}

sub check_go {
  my ($path, $src) = @_;
  my $n = 0;
  for my $line (split /\n/, $src, -1) {
    $n++;
    if ($line =~ m{//\s*nolint\b|//\s*lint:(?:ignore|file-ignore)\b|//[^\n]*\#nosec\b|//\s*nosec\b|//\s*revive:disable}) {
      finding($path, $n, "go-nolint", "linter suppression directive; fix the finding instead");
    }
    if ($line =~ m{\A\s*//\s*(?:go:build|\+build)\b}) {
      finding($path, $n, "go-build-tag", "a build constraint keeps this file out of the golangci-lint and go vet runs");
    }
  }
}

sub check_golangci {
  my ($path, $src) = @_;
  if ($path ne ".github/linters/golangci.yml") {
    finding($path, 1, "golangci-config", "only .github/linters/golangci.yml may configure golangci-lint");
  }
  my $n = 0;
  for my $line (split /\n/, $src, -1) {
    $n++;
    next if $line =~ m{\A\s*\#};
    if ($line =~ m{\A\s*-?\s*(exclusions|exclude[A-Za-z-]*|skip-dirs[A-Za-z-]*|skip-files|new|new-from-rev|new-from-patch|new-from-merge-base|whole-files|disable-all)\s*:}) {
      finding($path, $n, "golangci-config", "`$1` hides findings");
    }
    if ($line =~ m{\A\s*tests\s*:\s*false\b}) {
      finding($path, $n, "golangci-config", "`tests: false` skips test files");
    }
    if ($line =~ m{\A\s*default\s*:\s*(\S+)} && $1 ne "all") {
      finding($path, $n, "golangci-config", "linters.default must be `all`");
    }
  }
}

sub check_shellcheckrc {
  my ($path, $src) = @_;
  my $n = 0;
  for my $line (split /\n/, $src, -1) {
    $n++;
    if ($line =~ m{\A\s*(?:disable\s*=|extended-analysis\s*=\s*false)}) {
      finding($path, $n, "sh-rc", "a .shellcheckrc entry that turns shellcheck checks off");
    }
  }
}

sub check_script_text {
  my ($path, $src, $kind) = @_;
  my $n = 0;
  for my $line (split /\n/, $src, -1) {
    $n++;
    if ($line =~ m{\#\s*shellcheck\s[^\n]*(?:\bdisable\s*=|\bextended-analysis\s*=\s*false)}) {
      finding($path, $n, "sh-directive", "shellcheck directive that turns checks off; fix the script instead");
    }
    if ($line =~ m{shellcheck|SHELLCHECK_OPT[S]}i
        && $line =~ m{(?:\A|[\s"\x27=])(?:--exclud[e]\b|-e\s*SC\d|--includ[e]\b|-i\s*SC\d|--severity[=\s]+\x27?"?(?:error|warning|info)\b|-S\s*(?:error|warning|info)\b)}) {
      finding($path, $n, "sh-options", "shellcheck option that skips checks or lowers the severity");
    }
    if ($line =~ m{\b(?:CARGO_ENCODED_)?RUST[F]LAGS\b|\bCARGO_BUILD_RUST[F]LAGS\b|\bCARGO_TARGET_[A-Z0-9_]+_RUST[F]LAGS\b|\bCARGO_(?:ENCODED|BUILD)_RUSTDOC[F]LAGS\b|\bCLIPPY_CONF_DI[R]\b|\bCLIPPY_ARG[S]\b}) {
      finding($path, $n, "rustflags-env", "compiler flag variables can lower lint levels; lint policy lives in Cargo.toml");
    }
    if ($line =~ m{\bRUSTDOC[F]LAGS\b}
        && $line !~ m{\bRUSTDOC[F]LAGS\s*[:=]\s*(["\x27]?)(?:-D\s*warnings|--deny[=\s]warnings)\1\s*(?:\z|[;\s\}]|\\)}) {
      finding($path, $n, "rustdocflags-env", "the rustdoc flags variable may only be -D warnings");
    }
    if ($line =~ m{\bCARGO_BUILD_WARNING[S]\s*[:=]\s*["\x27]?(\w*)} && $1 ne "deny") {
      finding($path, $n, "cargo-warnings-env", "CARGO_BUILD_WARNINGS may only be deny");
    }
    if ($line =~ m{\b(?:cargo|rustc|rustdoc|clippy-driver|cargo-clippy)\b.*\s(?:-A|-W)(?:\s|[a-z]|=|\z)}
        || $line =~ m{\b(?:cargo|rustc|rustdoc|clippy-driver|cargo-clippy)\b.*\s(?:--allo[w]|--war[n]|--force-war[n]|--cap-lint[s])(?:\s|=|\z)}) {
      finding($path, $n, "lint-flag", "command-line lint level flags override the workspace lint policy");
    }
    if ($line =~ m{actionlint} && $line =~ m{\s-ignor[e]\b}) {
      finding($path, $n, "actionlint-ignore", "the actionlint ignore flag hides findings");
    }
    if ($kind eq "yaml" && $path eq ".github/workflows/ci.yml" && $line =~ m{\A\s*continue-on-erro[r]\s*:}) {
      finding($path, $n, "ci-continue-on-error", "CI checks are always strict; continue-on-error is not allowed in ci.yml");
    }
  }
}

sub check_actionlint_config {
  my ($path, $src) = @_;
  my $n = 0;
  for my $line (split /\n/, $src, -1) {
    $n++;
    if ($line =~ m{\A\s*ignor[e]\s*:}) {
      finding($path, $n, "actionlint-ignore", "ignore: entries hide actionlint findings");
    }
  }
}

sub read_file {
  my ($file) = @_;
  open(my $fh, "<:raw", $file) or die "cannot read $file: $!";
  local $/;
  my $src = <$fh>;
  close $fh;
  return defined $src ? $src : "";
}

my @entries = map { chomp; [split /\t/, $_, 2] } <STDIN>;
# The directories of proc-macro crates, whose string literals can become generated attributes.
my @proc_macro_dirs;
for my $entry (@entries) {
  my ($kind, $path) = @{$entry};
  next unless $kind eq "cargo" && -f "$root/$path";
  my $manifest = read_file("$root/$path");
  $manifest =~ s/\#[^\n]*//g;
  if ($manifest =~ m{\bproc[-_]macro\s*=\s*true\b}) {
    (my $dir = $path) =~ s{/?Cargo\.toml\z}{};
    push @proc_macro_dirs, $dir;
  }
}

for my $entry (@entries) {
  my ($kind, $path) = @{$entry};
  if ($kind eq "toolchain") {
    if ($path ne "rust-toolchain.toml") {
      finding($path, 1, "rust-toolchain", "only rust-toolchain.toml at the repository root may pick the toolchain");
    }
    next;
  }
  next unless $kind =~ m{\A(?:rust|go|golangci|shell|shellcheckrc|yaml|actionlint)\z};
  my $file = "$root/$path";
  next unless -f $file;
  my $src = read_file($file);
  if ($kind eq "rust") {
    my $proc_macro = grep { $_ eq "" || index($path, "$_/") == 0 } @proc_macro_dirs;
    check_rust($path, $src, $proc_macro);
  }
  elsif ($kind eq "go")           { check_go($path, $src); }
  elsif ($kind eq "golangci")     { check_golangci($path, $src); }
  elsif ($kind eq "shellcheckrc") { check_shellcheckrc($path, $src); }
  elsif ($kind eq "actionlint")   { check_actionlint_config($path, $src); check_script_text($path, $src, $kind); }
  else                            { check_script_text($path, $src, $kind); }
}
close $fh_out or die "cannot write $out: $!";
' "${root}" "${work}/findings" < "${work}/classified"

# 4. TOML rules. taplo turns each file into JSON, so dotted keys, inline tables and table headers
#    are all seen the same way.
toml_rules() {
  local kind=$1 path=$2 json reason
  if ! json=$(taplo get -o json -f "${root}/${path}" 2> "${work}/taplo.err"); then
    reason=$(tr '\n' ' ' < "${work}/taplo.err")
    printf '%s:1: [toml-parse] taplo cannot parse this file: %s\n' "${path}" "${reason}" \
      >> "${work}/findings"
    return 0
  fi
  jq -r --arg kind "${kind}" --arg path "${path}" --arg channel "${channel}" '
    def level: if type == "string" then . elif type == "object" then (.level // "") else "" end;
    def nonempty: . != null and . != [] and . != {} and . != "";
    def minor: ([tostring | capture("^(?<v>[0-9]+[.][0-9]+)") | .v][0]) // "";
    def lint_findings($prefix):
      to_entries[]
      | select(.key != "workspace")
      | .key as $tool
      | (.value | if type == "object" then to_entries[] else empty end)
      | (.value | level) as $level
      | select(["warn", "deny", "forbid"] | index($level) | not)
      | "lint-level\t\($prefix).\($tool).\(.key) = \(if $level == "" then "(no level)" else $level end): only warn, deny and forbid are allowed";
    (if $kind == "cargo" then
      ((.lints // {}) | lint_findings("lints")),
      ((.workspace.lints // {}) | lint_findings("workspace.lints")),
      (if has("package") and .lints != {"workspace": true} then
        "crate-lints\t[lints] must be exactly `workspace = true`, so the crate inherits the workspace lint policy"
      else empty end),
      (if (.package.metadata["cargo-machete"].ignored | nonempty) or (.workspace.metadata["cargo-machete"].ignored | nonempty) then
        "machete-ignore\tcargo-machete `ignored` hides unused dependencies"
      else empty end),
      (if (.package // {} | has("rust-version")) and .package["rust-version"] != {"workspace": true} then
        "rust-version\tpackage.rust-version must be `workspace = true`: clippy takes it as the MSRV and turns off the lints for newer features"
      else empty end),
      (.workspace.package["rust-version"] // null | select(. != null) as $msrv
        | select(($channel | minor) != "" and ($msrv | minor) != ($channel | minor))
        | "rust-version\tworkspace.package.rust-version \($msrv | tostring) is not the rust-toolchain.toml channel \($channel): clippy takes it as the MSRV and turns off the lints for newer features")
    elif $kind == "clippy" then
      (if $path != "clippy.toml" then "clippy-config\tonly clippy.toml at the repository root may configure clippy; a nested config replaces it for the crates below it" else empty end),
      (keys[] | select(test("(^|-)allow")) | "clippy-config\t`\(.)` relaxes clippy lints"),
      (keys[] | select(test("msrv")) | "clippy-config\t`\(.)` lowers the Rust version clippy assumes, which turns off the lints that suggest newer features")
    elif $kind == "rustfmt" then
      (if $path != "rustfmt.toml" then "rustfmt-config\tonly rustfmt.toml at the repository root may configure rustfmt" else empty end),
      (if .disable_all_formatting == true then "rustfmt-config\tdisable_all_formatting turns rustfmt off" else empty end),
      (if (.ignore | nonempty) then "rustfmt-config\t`ignore` skips files" else empty end),
      (if .skip_children == true then "rustfmt-config\tskip_children skips modules" else empty end),
      (if .format_generated_files == false then "rustfmt-config\tformat_generated_files = false skips files" else empty end),
      (if (.skip_macro_invocations | nonempty) then "rustfmt-config\tskip_macro_invocations skips code" else empty end),
      (if .error_on_line_overflow == false then "rustfmt-config\terror_on_line_overflow must stay true" else empty end),
      (if .error_on_unformatted == false then "rustfmt-config\terror_on_unformatted must stay true" else empty end)
    elif $kind == "deny" then
      (if $path != "deny.toml" then "deny-config\tonly deny.toml at the repository root may configure cargo-deny" else empty end),
      (if (.advisories.ignore | nonempty) then "deny-config\tadvisories.ignore hides advisories" else empty end),
      (if (.bans.skip | nonempty) then "deny-config\tbans.skip hides duplicate versions" else empty end),
      (if (.bans["skip-tree"] | nonempty) then "deny-config\tbans.skip-tree hides duplicate versions" else empty end),
      (if (.licenses.exceptions | nonempty) then "deny-config\tlicenses.exceptions allows licenses per crate" else empty end),
      (if (.sources["allow-git"] | nonempty) then "deny-config\tsources.allow-git admits git dependencies" else empty end),
      (if (.sources["allow-org"] | nonempty) then "deny-config\tsources.allow-org admits git dependencies" else empty end)
    elif $kind == "cargoconfig" then
      (paths | select(.[-1] | tostring | test("^rust(doc)?flags$")) | "cargo-config\t`\(map(tostring) | join("."))` sets compiler flags; lint policy lives in Cargo.toml"),
      (paths | select(.[-1] | tostring | test("^(rustc|rustc-wrapper|rustc-workspace-wrapper|runner)$")) | "cargo-config\t`\(map(tostring) | join("."))` overrides the compiler or the test runner, which can strip lint flags or run code"),
      ((.env // {}) | keys[] | select(test("^(CARGO_ENCODED_)?RUST(DOC)?FLAGS$|^CARGO_BUILD_RUST(DOC)?FLAGS$|^CARGO_TARGET_.*_RUST[F]LAGS$|^CLIPPY_|^CARGO_BUILD_WARNINGS$|^RUSTC(_WRAPPER|_WORKSPACE_WRAPPER)?$")) | "cargo-config\t[env] \(.) can lower lint levels or run code"),
      (if (.build.warnings // "deny") != "deny" then "cargo-config\tbuild.warnings must be deny if set" else empty end),
      ((.alias // {}) | to_entries[]
        | select((.key | test("^(build|check|clippy|doc|fmt|test|rustdoc|deny|machete)$"))
          or ((.value | if type == "array" then join(" ") else tostring end)
            | test("(^|\\s)(-A|-W)(\\s|[a-z]|=|$)|(^|\\s)--(allow|warn|force-warn|cap-lints)(\\s|=|$)")))
        | "cargo-config\talias `\(.key)` shadows a lint command or passes lint flags")
    else empty end)
    | split("\t") | "\($path):1: [\(.[0])] \(.[1])"
  ' <<< "${json}" >> "${work}/findings"
}

# The pinned toolchain channel, which the workspace rust-version (the MSRV clippy assumes) must match.
channel=""
if [[ -f "${root}/rust-toolchain.toml" ]]; then
  channel=$(taplo get -f "${root}/rust-toolchain.toml" toolchain.channel 2> /dev/null) || channel=""
fi

while IFS=$'\t' read -r kind path; do
  case "${kind}" in
    cargo | clippy | rustfmt | deny | cargoconfig)
      toml_rules "${kind}" "${path}"
      ;;
    *) ;;
  esac
done < "${work}/classified"

scanned=$(wc -l < "${work}/classified")
scanned=${scanned//[[:space:]]/}
if [[ -s "${work}/findings" ]]; then
  sort -u "${work}/findings"
  count=$(sort -u "${work}/findings" | wc -l)
  count=${count//[[:space:]]/}
  printf '\ncheck-no-lint-suppression: %s finding(s) in %s scanned file(s).\n' "${count}" "${scanned}"
  printf 'Suppressing lints is forbidden in this repository: fix the code instead (PORTING.md).\n'
  exit 1
fi
printf 'check-no-lint-suppression: no suppressions in %s scanned file(s).\n' "${scanned}"
