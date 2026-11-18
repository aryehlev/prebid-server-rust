#!/usr/bin/env bash
# The security policy of .github/workflows (checked line by line, since the workflows follow one
# layout), self-tested on a workflow that breaks every rule, and the invariants of the agents'
# Claude settings.
#
# Workflow rules
#   - every action is pinned to a full commit sha with a "# v<version>" comment
#   - no ${{ }} expression inside a run script (values reach scripts through env)
#   - no pull_request_target, workflow_run or issue_comment trigger; no bypassing permissions
#   - top-level permissions, a timeout on every job, and no write permission for GITHUB_TOKEN
#   - checkout with persist-credentials: false, except in the jobs that push
#   - the push credentials only in the jobs that push, the Anthropic credentials only in the
#     agent job, and no other secrets
#   - agent steps have a timeout and run only in the agent job; continue-on-error only on agent
#     steps, and never in ci.yml
set -euo pipefail
shopt -s inherit_errexit
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

policy="${T_ROOT}/policy.pl"
cat > "${policy}" << 'EOF'
use strict;
use warnings;

my %push_jobs = map { $_ => 1 } split /,/, ($ENV{PUSH_JOBS} // '');
my %agent_jobs = map { $_ => 1 } split /,/, ($ENV{AGENT_JOBS} // '');
my %strict_files = map { $_ => 1 } split /,/, ($ENV{NO_CONTINUE_ON_ERROR} // '');
my @out;

sub report {
    my ($file, $n, $message) = @_;
    push @out, "$file:$n: $message";
}

for my $path (@ARGV) {
    open my $fh, '<', $path or die "$path: $!\n";
    my @lines = <$fh>;
    close $fh;
    (my $file = $path) =~ s{.*/}{};
    my ($top_permissions, $section, $job) = (0, '', '');
    my ($run_indent, $perm_indent, $steps_indent, $step);
    my (%jobs, %timeouts);
    my $finish_step = sub {
        return unless $step;
        my $text = join "\n", @{ $step->{lines} };
        my $key = "$file:$step->{job}";
        if ($text =~ m{^\s*(?:- )?uses:\s*actions/checkout@}m
            && $text !~ m{^\s+persist-credentials:\s*false\s*$}m && !$push_jobs{$key}) {
            report($file, $step->{line}, 'checkout without persist-credentials: false outside the push jobs');
        }
        my $agent = $text =~ m{^\s*(?:- )?uses:\s*anthropics/claude-code-action@}m;
        if ($agent) {
            report($file, $step->{line}, 'agent step without timeout-minutes')
                unless $text =~ m{^\s+timeout-minutes:\s*\d+}m;
            report($file, $step->{line}, 'agent step outside the agent jobs') unless $agent_jobs{$key};
        }
        if ($text =~ m{^\s+continue-on-error:}m && (!$agent || $strict_files{$file})) {
            report($file, $step->{line}, 'continue-on-error is not allowed here');
        }
        undef $step;
    };
    for my $i (0 .. $#lines) {
        my $n = $i + 1;
        my $line = $lines[$i];
        $line =~ s/\r?\n\z//;
        my $indent = length(($line =~ /^( *)/)[0]);
        my $blank = $line =~ /^\s*(?:#.*)?$/;
        if (defined $run_indent) {
            if ($line =~ /^\s*$/ || $indent > $run_indent) {
                report($file, $n, 'expression in a run script; pass it through env') if $line =~ /\$\{\{/;
                push @{ $step->{lines} }, $line if $step;
                next;
            }
            undef $run_indent;
        }
        (my $code = $line) =~ s/(?:^|\s)#.*$//;
        if ($code =~ /\b(pull_request_target|workflow_run|issue_comment|dangerously[\w-]*|bypassPermissions)\b/) {
            report($file, $n, "forbidden trigger or option: $1");
        }
        if ($indent == 0 && $code =~ /^([A-Za-z_-]+):/) {
            $finish_step->();
            ($section, $job) = ($1, '');
            undef $steps_indent;
            $top_permissions = 1 if $section eq 'permissions';
        }
        if ($section eq 'jobs' && $code =~ /^  ([A-Za-z0-9_-]+):\s*$/) {
            $finish_step->();
            $job = $1;
            $jobs{$job} = $n;
            undef $steps_indent;
        }
        if ($job ne '' && $indent == 4 && $code =~ /^\s+timeout-minutes:\s*\d+/) {
            $timeouts{$job} = 1;
        }
        if ($job ne '' && $indent == 4 && $code =~ /^\s+continue-on-error:/) {
            report($file, $n, 'job-level continue-on-error');
        }
        if (defined $perm_indent) {
            if ($blank || $indent > $perm_indent) {
                report($file, $n, 'write permission for GITHUB_TOKEN') if $code =~ /:\s*write\s*$/;
            } else {
                undef $perm_indent;
            }
        }
        if ($code =~ /^(\s*)permissions:\s*(\S.*?)?\s*$/) {
            if (defined $2) {
                report($file, $n, "permissions: $2") if $2 =~ /write/;
            } else {
                $perm_indent = length $1;
            }
        }
        if ($job ne '' && $code =~ /^(\s+)steps:\s*$/) {
            $finish_step->();
            $steps_indent = length $1;
        } elsif (defined $steps_indent && $code =~ /^(\s*)- /
            && (length($1) == $steps_indent || length($1) == $steps_indent + 2)) {
            $finish_step->();
            $step = { line => $n, job => $job, lines => [] };
        } elsif (defined $steps_indent && !$blank && $indent <= $steps_indent) {
            $finish_step->();
            undef $steps_indent;
        }
        push @{ $step->{lines} }, $line if $step;
        if ($code =~ /^\s*(?:- )?uses:\s*(\S+)/) {
            my $ref = $1;
            report($file, $n, "action not pinned to a full commit sha with a version comment: $ref")
                unless $ref =~ m{^[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}$} && $line =~ /\s#\s*v\d+(?:\.\d+)*\s*$/;
        }
        while ($code =~ /\bsecrets\.([A-Za-z0-9_]+)/g) {
            my $name = $1;
            my $key = "$file:$job";
            if ($name eq 'PORT_APP_PRIVATE_KEY' || $name eq 'PORT_PUSH_TOKEN') {
                report($file, $n, "push credential $name outside the push jobs") unless $push_jobs{$key};
            } elsif ($name eq 'ANTHROPIC_API_KEY' || $name eq 'CLAUDE_CODE_OAUTH_TOKEN') {
                report($file, $n, "agent credential $name outside the agent jobs") unless $agent_jobs{$key};
            } else {
                report($file, $n, "unexpected secret $name");
            }
        }
        if ($code =~ /^(\s*)(?:- )?run:\s*(.*)$/) {
            my ($key_indent, $rest) = (length $1, $2);
            if ($rest =~ /^[|>][-+]?\d*\s*$/) {
                $run_indent = $key_indent;
            } elsif ($rest =~ /\$\{\{/) {
                report($file, $n, 'expression in a run script; pass it through env');
            }
        }
    }
    $finish_step->();
    report($file, 1, 'no top-level permissions') unless $top_permissions;
    for my $name (sort keys %jobs) {
        report($file, $jobs{$name}, "job $name has no timeout-minutes") unless $timeouts{$name};
    }
}
print "$_\n" for @out;
exit(@out ? 1 : 0);
EOF

# scan FILE...: run the policy with the fork's job lists.
scan() {
  t_run_in "${T_TOP}" env PUSH_JOBS=upstream-port.yml:publish,parity-harness-sync.yml:sync \
    AGENT_JOBS=upstream-port.yml:port NO_CONTINUE_ON_ERROR=ci.yml perl "${policy}" "$@"
}

shopt -s nullglob
workflows=("${T_TOP}"/.github/workflows/*.yml "${T_TOP}"/.github/workflows/*.yaml)
shopt -u nullglob
t_eq "the fork has its three workflows" 3 "${#workflows[@]}"
scan "${workflows[@]}"
t_eq "the workflows follow the policy" "" "${T_OUT}"
t_eq "the workflows follow the policy: exit status" 0 "${T_RC}"

bad="${T_ROOT}/bad.yml"
cat > "${bad}" << 'EOF'
name: Bad
on:
  pull_request_target:
jobs:
  build:
    runs-on: ubuntu-24.04
    permissions:
      contents: write
    continue-on-error: true
    steps:
      - uses: actions/checkout@v4
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
        with:
          persist-credentials: false
      - name: Agent
        uses: anthropics/claude-code-action@cc98b3ae241b0fcfb6a42a2690751afffecb4452 # v1.0.278
        with:
          anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
          claude_args: --dangerously-skip-permissions
      - name: Echo
        continue-on-error: true
        env:
          TOKEN: ${{ secrets.PORT_PUSH_TOKEN }}
        run: |
          echo "${{ github.event.pull_request.title }}"
      - run: echo ${{ secrets.OTHER }}
EOF
scan "${bad}"
t_eq "a broken workflow fails" 1 "${T_RC}"
for expected in \
  "bad.yml:3: forbidden trigger or option: pull_request_target" \
  "bad.yml:8: write permission for GITHUB_TOKEN" \
  "bad.yml:9: job-level continue-on-error" \
  "bad.yml:11: action not pinned to a full commit sha with a version comment: actions/checkout@v4" \
  "bad.yml:11: checkout without persist-credentials: false outside the push jobs" \
  "bad.yml:12: action not pinned to a full commit sha with a version comment: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1" \
  "bad.yml:15: agent step without timeout-minutes" \
  "bad.yml:15: agent step outside the agent jobs" \
  "bad.yml:18: agent credential ANTHROPIC_API_KEY outside the agent jobs" \
  "bad.yml:19: forbidden trigger or option: dangerously-skip-permissions" \
  "bad.yml:20: continue-on-error is not allowed here" \
  "bad.yml:23: push credential PORT_PUSH_TOKEN outside the push jobs" \
  "bad.yml:25: expression in a run script; pass it through env" \
  "bad.yml:26: unexpected secret OTHER" \
  "bad.yml:26: expression in a run script; pass it through env" \
  "bad.yml:1: no top-level permissions" \
  "bad.yml:5: job build has no timeout-minutes"; do
  t_has "the policy reports: ${expected#bad.yml:}" "${T_OUT}" "${expected}"
done
reported=$(wc -l <<< "${T_OUT}")
t_eq "the policy reports nothing else" 17 "${reported// /}"

strict="${T_ROOT}/ci.yml"
cp "${T_TOP}/.github/workflows/upstream-port.yml" "${strict}"
scan "${strict}"
t_has "continue-on-error is refused in ci.yml even on agent steps" "${T_OUT}" "continue-on-error is not allowed here"

# A pull request title or body edited after ci-ok is green must re-trigger CI, or the guard would
# never re-check the edited text for AI attribution. The only pull_request types: line is ci.yml's.
ci_types=$(grep -E '^[[:space:]]+types:' "${T_TOP}/.github/workflows/ci.yml")
t_has "ci.yml re-runs on pull request edits so the guard re-checks the title and body" "${ci_types}" "edited"

# The agents' settings.
settings="${T_PORT}/claude-settings.json"
checks=$(jq -c '[.attribution.commit, .attribution.pr, .includeCoAuthoredBy, .includeGitInstructions,
  .enableAllProjectMcpServers, .permissions.defaultMode, .permissions.disableBypassPermissionsMode,
  .env.CARGO_NET_OFFLINE, .env.GOPROXY, .env.GOFLAGS, .env.GOTOOLCHAIN, .env.GIT_TERMINAL_PROMPT]' "${settings}")
t_eq "settings: no attribution, no MCP servers, no bypass, offline" \
  '["","",false,false,false,"default","disable","true","off","-mod=readonly","local","0"]' "${checks}"
denied=$(jq -r '.permissions.deny | join(" ")' "${settings}")
for rule in WebFetch WebSearch 'mcp__*' 'Bash(git commit:*)' 'Bash(git push:*)' 'Bash(git merge:*)' \
  'Bash(git rebase:*)' 'Bash(git reset:*)' 'Bash(git -c:*)' 'Bash(git -C:*)' 'Bash(git config:*)' \
  'Bash(git remote:*)' 'Bash(git fetch:*)' 'Bash(gh:*)' 'Bash(curl:*)' 'Bash(wget:*)' 'Bash(ssh:*)' \
  'Bash(cargo add:*)' 'Bash(cargo install:*)' 'Bash(cargo update:*)' 'Bash(go get:*)' 'Bash(go mod:*)' \
  'Edit(./.github/**)' 'Edit(./.claude/**)' 'Edit(./.planning/**)' 'Edit(./AGENTS.md)' 'Edit(./.upstream/**)' \
  'Edit(./Cargo.toml)' 'Edit(./.cargo/**)' 'Edit(./deny.toml)' 'Edit(./scripts/**)' 'Edit(./UPSTREAM_SYNC)' \
  'Edit(./upstream-port-log.jsonl)' 'Edit(./**/rust-toolchain)' 'Edit(./**/clippy.toml)' \
  'Edit(./**/.clippy.toml)' 'Edit(./**/.rustfmt.toml)' 'Edit(./**/go.work)' 'Edit(./**/vendor/**)'; do
  t_has "settings: deny ${rule}" " ${denied} " " ${rule} "
done
# Every protected path and port record that check-worktree.sh reverts is also denied to Edit, so a
# session learns at once instead of losing the change afterwards.
protected=$(
  # shellcheck source=../scripts/lib.sh
  source "${T_SCRIPTS}/lib.sh"
  port_protected_paths
  port_record_paths
)
undenied=""
while IFS= read -r path; do
  if [[ " ${denied} " != *" Edit(./${path})"* && " ${denied} " != *" Edit(./${path}/**)"* ]]; then
    undenied+="${path} "
  fi
done <<< "${protected}"
t_eq "settings: every protected path is denied to Edit" "" "${undenied}"
allowed=$(jq -r '(.permissions.allow // []) | length' "${settings}")
t_eq "settings: the allow lists come from render-inputs.sh" 0 "${allowed}"

# The repository's Go code is test tooling: it must build, pass its tests and be gofmt -s clean,
# and it is not linted (no golangci-lint, no separate go vet), as .github/README.md explains.
linted=$(grep -n -E '^[^#]*(golangci|go vet)' "${T_TOP}/.github/workflows/"*.yml "${T_SCRIPTS}/"*.sh || true)
t_eq "no workflow or port script lints the Go tooling" "" "${linted}"
# (The suppression scanner's fixtures hold golangci configurations on purpose: it flags them.)
golangci=$(find "${T_TOP}/.github" "${T_TOP}/scripts" -path "${T_TOP}/scripts/tests/fixtures" -prune \
  -o \( -name '.golangci*' -o -name 'golangci*' \) -print | head -n 5)
t_eq "no golangci-lint configuration" "" "${golangci}"
fetched=$(grep -c 'scripts/fetch-upstream.sh' "${T_TOP}/.github/workflows/ci.yml" || true)
if [[ "${fetched}" -ge 4 ]]; then
  t_ok "ci.yml fetches the pinned upstream for the jobs that read Go source"
else
  t_fail "ci.yml fetches the pinned upstream in ${fetched} place(s)"
fi

t_done
