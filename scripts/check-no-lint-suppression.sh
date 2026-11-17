#!/usr/bin/env bash
# check-no-lint-suppression.sh: fail when code the fork authors suppresses, weakens or hides a lint,
# formatter or analyzer finding. The policy is to fix the finding; nothing may silence it.
#
# Usage:
#   rust/scripts/check-no-lint-suppression.sh             scan the git work tree (tracked files and
#                                                         untracked files that are not ignored)
#   rust/scripts/check-no-lint-suppression.sh --root DIR  scan every file under DIR, laid out like the
#                                                         repository (used by rust/scripts/tests/run.sh)
#
# Scope: rust/, .github/, .planning/, AGENTS.md, CLAUDE.md and parity/. The upstream Go server is
# never scanned. rust/legacy/ is skipped: it is not compiled, and it is deleted once every
# replacement crate has landed (rust/README.md).
#
# Rules (the bracketed id is printed with every finding):
#   Rust sources, with comments and string literals removed first
#     rust-lint-attr        #[allow(..)], #![allow(..)], #[expect(..)], #![expect(..)], including the
#                           raw-identifier forms #[r#allow(..)] / #[r#expect(..)]
#     rust-cfg-attr-lint    cfg_attr(.., allow(..)) or cfg_attr(.., expect(..)), at any nesting
#     rustfmt-skip          rustfmt::skip in any form (raw identifiers too), and the old rustfmt_skip
#     rust-cfg-tool         cfg(..)/cfg_attr(..)/cfg!(..) predicates on clippy or rustfmt
#     rust-include-legacy   #[path] or include!() that pulls rust/legacy/ code into a crate
#     rust-macro-attr       #[$x] / #![$x] in a macro body applying a caller-supplied attribute name
#                           (not one captured whole as #[$x:meta]): launders a lint suppression that
#                           neither the literal scan above nor clippy's allow_attributes lint sees
#   Cargo.toml (parsed with taplo, so every TOML spelling is covered)
#     lint-level            any [lints] or [workspace.lints] level other than warn, deny or forbid
#     crate-lints           a package whose [lints] is not exactly `workspace = true`
#     machete-ignore        package.metadata/workspace.metadata cargo-machete `ignored` lists
#   other configuration
#     toml-parse            a TOML file the checks above cannot parse
#     clippy-config         allow-* / allowed-* options in clippy.toml or .clippy.toml
#     rustfmt-config        rustfmt configs other than rust/rustfmt.toml, and options that skip files
#     deny-config           cargo-deny configs other than rust/deny.toml, and ignore/skip/exception lists
#     cargo-config          rustflags, rustc / rustc-wrapper / rustc-workspace-wrapper / runner
#                           overrides, lint-related [env] entries (including RUSTC_WRAPPER), build.
#                           warnings and lint-command aliases in .cargo/config and .cargo/config.toml
#   Go code we author
#     go-nolint             nolint, lint:ignore, lint:file-ignore, nosec and revive:disable comments
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
      sed -n '2,52p' "${BASH_SOURCE[0]}"
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

# 1. Collect the candidate files as repository-relative paths.
if [[ "${mode}" == git ]]; then
  git -C "${root}" ls-files -z --cached --others --exclude-standard -- \
    rust .github .planning AGENTS.md CLAUDE.md parity > "${work}/all"
else
  (cd "${root}" && find . -type d \( -name .git -o -name target \) -prune -o -type f -print0) \
    | perl -0 -pe 's{\A\./}{}' > "${work}/all"
fi

# 2. Classify them. Each output line is "<kind>\t<path>".
perl -0 -ne '
  chomp;
  my $p = $_;
  next if $p =~ m{\Arust/legacy/};
  next if $p =~ m{(?:\A|/)target/};
  next unless $p =~ m{\A(?:rust/|\.github/|\.planning/|parity/|AGENTS\.md\z|CLAUDE\.md\z)};
  my ($base) = $p =~ m{([^/]+)\z};
  my $kind;
  if    ($p =~ /\.rs\z/)                                      { $kind = "rust" }
  elsif ($base eq "Cargo.toml")                               { $kind = "cargo" }
  elsif ($base eq "clippy.toml" || $base eq ".clippy.toml")   { $kind = "clippy" }
  elsif ($base eq "rustfmt.toml" || $base eq ".rustfmt.toml") { $kind = "rustfmt" }
  elsif ($base eq "deny.toml")                                { $kind = "deny" }
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
# kept, so offsets and line numbers are unchanged.
sub rust_code_only {
  my ($s) = @_;
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
      $out .= blank(substr($s, $start, $end - $start));
      pos($s) = $end;
      next;
    }
    if ($s =~ m{\G"(?:[^"\\]|\\.)*"?}gcs) { $out .= blank($&); next; }
    if ($s =~ m{\G\x27(?:\\(?:x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f]{1,6}\}|.)|[^\\\x27\n])\x27}gcs) {
      $out .= blank($&);
      next;
    }
    if ($s =~ m{\G(?:[A-Za-z0-9_]+|\s+|.)}gcs) { $out .= $&; next; }
  }
  return $out;
}

my $balanced = qr{(\((?:[^()]++|(?-1))*\))};

sub check_rust {
  my ($path, $src) = @_;
  my $code = rust_code_only($src);
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
      finding($path, line_at($code, $at), "rust-include-legacy", "rust/legacy/ is not linted and must not be compiled into a crate");
    }
  }
  # A macro that applies a caller-supplied attribute name, #[$x] or #![$x], launders a suppression:
  # the caller passes allow(..)/expect(..) as a bare :meta fragment and the macro wraps it in #[..],
  # which neither the literal scan above nor the clippy allow_attributes lint sees. The legitimate
  # form forwards a whole attribute the caller already wrote with #[..], captured in the matcher as
  # #[$x:meta]; flag only a metavariable that is never captured inside #[..], and never $crate (the
  # defining crate path, which no caller controls).
  my %wrapped;
  while ($code =~ m{\#\s*\[\s*\$(\w+)\s*:}g) {
    $wrapped{$1} = 1;
  }
  while ($code =~ m{\#\s*!?\s*\[\s*\$(\w+)\b}g) {
    my ($name, $at) = ($1, $-[0]);
    next if $name eq "crate" || $wrapped{$name};
    finding($path, line_at($code, $at), "rust-macro-attr",
      "#[\$$name] applies a caller-supplied attribute name; capture a whole attribute as #[\$$name:meta] or inline a fixed attribute so a lint suppression cannot be laundered through it");
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
      finding($path, $n, "rustflags-env", "compiler flag variables can lower lint levels; lint policy lives in rust/Cargo.toml");
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

while (my $entry = <STDIN>) {
  chomp $entry;
  my ($kind, $path) = split /\t/, $entry, 2;
  next unless $kind =~ m{\A(?:rust|go|golangci|shell|shellcheckrc|yaml|actionlint)\z};
  my $file = "$root/$path";
  next unless -f $file;
  open(my $fh, "<:raw", $file) or die "cannot read $file: $!";
  local $/;
  my $src = <$fh>;
  close $fh;
  $src = "" unless defined $src;
  if    ($kind eq "rust")         { check_rust($path, $src); }
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
  jq -r --arg kind "${kind}" --arg path "${path}" '
    def level: if type == "string" then . elif type == "object" then (.level // "") else "" end;
    def nonempty: . != null and . != [] and . != {} and . != "";
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
      else empty end)
    elif $kind == "clippy" then
      keys[] | select(test("^allow")) | "clippy-config\t`\(.)` relaxes clippy lints"
    elif $kind == "rustfmt" then
      (if $path != "rust/rustfmt.toml" then "rustfmt-config\tonly rust/rustfmt.toml may configure rustfmt" else empty end),
      (if .disable_all_formatting == true then "rustfmt-config\tdisable_all_formatting turns rustfmt off" else empty end),
      (if (.ignore | nonempty) then "rustfmt-config\t`ignore` skips files" else empty end),
      (if .skip_children == true then "rustfmt-config\tskip_children skips modules" else empty end),
      (if .format_generated_files == false then "rustfmt-config\tformat_generated_files = false skips files" else empty end),
      (if (.skip_macro_invocations | nonempty) then "rustfmt-config\tskip_macro_invocations skips code" else empty end),
      (if .error_on_line_overflow == false then "rustfmt-config\terror_on_line_overflow must stay true" else empty end),
      (if .error_on_unformatted == false then "rustfmt-config\terror_on_unformatted must stay true" else empty end)
    elif $kind == "deny" then
      (if $path != "rust/deny.toml" then "deny-config\tonly rust/deny.toml may configure cargo-deny" else empty end),
      (if (.advisories.ignore | nonempty) then "deny-config\tadvisories.ignore hides advisories" else empty end),
      (if (.bans.skip | nonempty) then "deny-config\tbans.skip hides duplicate versions" else empty end),
      (if (.bans["skip-tree"] | nonempty) then "deny-config\tbans.skip-tree hides duplicate versions" else empty end),
      (if (.licenses.exceptions | nonempty) then "deny-config\tlicenses.exceptions allows licenses per crate" else empty end),
      (if (.sources["allow-git"] | nonempty) then "deny-config\tsources.allow-git admits git dependencies" else empty end),
      (if (.sources["allow-org"] | nonempty) then "deny-config\tsources.allow-org admits git dependencies" else empty end)
    elif $kind == "cargoconfig" then
      (paths | select(.[-1] | tostring | test("^rust(doc)?flags$")) | "cargo-config\t`\(map(tostring) | join("."))` sets compiler flags; lint policy lives in rust/Cargo.toml"),
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
  printf 'Suppressing lints is forbidden in this repository: fix the code instead (rust/PORTING.md).\n'
  exit 1
fi
printf 'check-no-lint-suppression: no suppressions in %s scanned file(s).\n' "${scanned}"
