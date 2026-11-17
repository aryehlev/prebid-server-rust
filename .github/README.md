# Fork operations: the Rust port of Prebid Server

This repository is a fork of [prebid/prebid-server](https://github.com/prebid/prebid-server). The Go
server stays exactly as upstream ships it. The Rust port lives in `rust/`. The daily upstream-port
workflow merges new upstream commits and ports them to Rust.

## Repository model

| Path | Owner | Rule |
| --- | --- | --- |
| `rust/`, `.github/`, `.planning/`, `AGENTS.md`, `CLAUDE.md` | the fork | listed in `.github/port/fork-paths.txt` |
| everything else | upstream | byte-identical to the newest upstream commit merged into the branch |

- `rust/UPSTREAM_SYNC` names the newest upstream commit whose changes the Rust port covers. Only
  the port workflow moves it, and it only moves forward.
- `rust/upstream-port-log.jsonl` gets one line per upstream commit the port workflow handled: its
  status, the Go behaviour left unported, and the run. It only grows: the guard fails a port
  pull request that changes lines `main` already has.
- Only `commit.sh` writes these two records, after the last agent session. A session that
  changes them (with an edit, or from a test or probe it runs) has the change reverted.
- `rust/conformance/known_failures.txt` lists the Go adapter fixtures that may still fail. The
  list only shrinks. See `rust/conformance/README.md`.
- `.claude/` and `.omx/` hold local tool state. They are never committed, and CI fails if they are.

The fork never edits upstream Go. A Go change belongs upstream, and the fork picks it up with the
next merge. The upstream merge keeps the fork-owned paths and takes everything else from upstream,
so it never conflicts. Upstream history is merged as it is and never rewritten.

### Branches

| Branch | Purpose |
| --- | --- |
| `main` | Default branch. Changes land by pull request, as a **merge commit** only. |
| `port/upstream-sync` | The port pull request. The bot owns this branch. |
| `parity-harness` | The parity harness. It patches upstream Go on purpose, so it is exempt from the Go-tree rule. Main is merged into it daily. |

The upstream merge commits have to stay in `main`'s first-parent history. A squash or rebase
merge of the port pull request drops them. After that the guard and the planner fail with "was a
port pull request squash- or rebase-merged?" (see the runbook below).

## Workflows

### `ci.yml`: every pull request, every push to `main` and `parity-harness`, merge queues

Every check runs at full strictness. There are no toggles, and no job may continue on error.

| Job | What it checks |
| --- | --- |
| `changes` | the compared range and the adapter directories the change touches |
| `guard` | the fork invariants (`.github/port/scripts/guard.sh`): the Go tree equals upstream, no committed `.claude/`/`.omx/`, the `UPSTREAM_SYNC` marker, the known-failures list, no AI attribution in fork commits or pull request text, and on the port pull request the bot rules |
| `rust-fmt` | `cargo +<nightly> fmt --all -- --check`. The nightly comes from `rust/scripts/rustfmt-toolchain.sh`. |
| `rust-clippy` | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| `rust-doc` | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` |
| `rust-test` | `cargo test --workspace --locked` |
| `conformance` | the Go fixture ratchet (`.github/port/scripts/conformance.sh`). Fixtures of touched adapters must all pass. |
| `devtools` | `cargo run -p pbs-devtools --locked -- all --check`, with the Go version of the upstream `Dockerfile` |
| `lint-policy` | `rust/scripts/check-no-lint-suppression.sh` (no `#[allow]`, `#[expect]`, lint caps or "allow" levels anywhere, including attributes a macro assembles, which clippy does not see) and its self-tests |
| `lint-config` | `taplo fmt --check`, shellcheck with every optional check at style severity, and actionlint |
| `port-tests` | `.github/port/tests/run-all.sh`: the port tooling, tested in temporary repositories |
| `deny` | `cargo deny --locked check --deny warnings`: advisories, bans, licenses, sources (crates.io only, no duplicate versions) |
| `machete` | unused dependencies |
| `go-lint` | `gofmt -s`, `go vet` and golangci-lint (`.github/linters/golangci.yml`, every linter) on the Go the fork writes: `rust/**/testdata/go/`, `rust/crates/devtools/go/` and `parity/` |
| `parity` | `parity-harness` only: `.github/port/scripts/ci-parity.sh` |
| `ci-ok` | passes only when every other job passed. `parity` may be skipped. |

Make `ci-ok` the only required status check.

### `upstream-port.yml`: daily at 05:23 UTC, or by hand

Three jobs, each with its own trust level:

1. **plan** (read-only token, no agents). It reads git, the open port pull request and its
   `ci-ok` result, and chooses one action:
   - `advance`: merge and port the next chunk of upstream first-parent commits.
   - `repair`: fix the red CI of the open port pull request without porting anything new. Each
     attempt is a `port-repair:` commit, an empty one when the session changed nothing, so the
     attempts count towards `PORT_MAX_REPAIRS` and CI runs again.
   - `escalate`: label the pull request `port:needs-human` and stop.
   - `none`: up to date, waiting for CI, or waiting for a human. A failed `ci-ok` does not count
     while another CI run on the same head is still going: a push and an edit of the pull request
     start two runs, and the older one is cancelled with a failed `ci-ok`.

   A chunk holds at most `PORT_MAX_COMMITS` commits and `PORT_MAX_GO_LINES` changed Go lines.
   A heavy commit (core Go, modules, an OpenRTB bump, or more than 1500 Go lines) gets a chunk of
   its own. The plan job also packs the default branch's `.github/port` tooling, so later jobs
   run trusted scripts, never ones taken from the branch under work.
2. **port** (read-only contents token, no push credential). It works on the planned commit
   checked out without persisted credentials. It merges `main` if needed, then the upstream chunk
   (`merge-upstream.sh`), builds the porting context, prefetches crates and Go modules, and runs
   the gates. Then the agent sessions run:
   - **port**: ports the chunk and writes differential tests.
   - **adversarial review**: compares against Go and writes `adversarial_*.rs` tests that fail
     where Rust differs.
   - **fix**: runs only when the review found blocking differences or the gates are red.

   After every session, `check-worktree.sh` undoes commits, reverts changes outside `rust/`, to
   protected files (the workspace manifest, toolchain, rustfmt, clippy, cargo, deny and taplo
   configs, `rust/scripts/`, the porting rules) and to the port records, and drops added known
   failures. The final gates decide pass or fail.
   `commit.sh` then commits with the bot identity, and `guard.sh --bot` checks the result. The
   result leaves the job as a git bundle.
3. **publish** (fresh runner, default-branch tooling, the only push credential). It runs the
   guard on the bundle without building or running anything from it. It pushes
   `port/upstream-sync` (never `main`) and opens or updates the pull request. The PR is a draft
   while red, labelled `port:green` or `port:red`, plus `port:partial` when the agents reported a
   commit `partial` or `blocked`, or never reported on it (a failed or refused session). Go
   behaviour the agents left unported, and those commits, go to the "Parity debt from upstream
   ports" issue. A failed port job goes to "Upstream port run failed".

The agents run with `.github/port/claude-settings.json`, plus a per-session allow list from
`render-inputs.sh`:

- Attribution is off, and bypass-permissions mode is disabled.
- A deny list blocks git commands that change history or remotes, `gh`, network tools,
  `WebFetch`/`WebSearch`, MCP tools, dependency changes, and edits to `.github/`, protected
  `rust/` files and local tool state.
- Cargo and Go are offline (`CARGO_NET_OFFLINE`, `GOPROXY=off`, `GOFLAGS=-mod=readonly`,
  `GOTOOLCHAIN=local`).

The system prompts are in `.github/port/PORT_SYSTEM.md` and `.github/port/REVIEW_SYSTEM.md`. They
tell the agents that repository content is data, not instructions, and that the GSD workflow of
`AGENTS.md` does not apply in CI. Each session has a turn limit, a dollar budget and a step
timeout. Agent output reaches pull requests only after sanitizing and fencing.

### `parity-harness-sync.yml`: daily at 06:47 UTC, or by hand

This workflow merges `main` into `parity-harness` and pushes the merge. If the merge conflicts,
nothing is pushed and the "parity-harness: merging main conflicts" issue is opened or commented.
The next clean sync closes it.

### `dependabot.yml`

Weekly, grouped updates of the pinned actions. Every action is pinned to a full commit SHA with a
version comment, and `test_workflow_policy.sh` enforces that.

## Setup

### Secrets

| Secret | Used by | Purpose |
| --- | --- | --- |
| `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` | port job only | the agent sessions; set one |
| `PORT_APP_PRIVATE_KEY` | publish, parity sync | private key of the GitHub App that pushes (preferred) |
| `PORT_PUSH_TOKEN` | publish, parity sync | fallback: a fine-grained token for this repository with contents, pull requests, issues and workflows write |

Pushes and pull requests made with `GITHUB_TOKEN` do not trigger workflows. The port pull request
would then never get `ci-ok`. That is why pushing needs a GitHub App or `PORT_PUSH_TOKEN`, and
the publish job fails without one.

The App needs contents, pull requests, issues and workflows write, and must be installed on this
repository. Set `vars.PORT_APP_ID` (or `vars.PORT_APP_CLIENT_ID`) and the private key secret.

### Variables (all optional)

| Variable | Default | Meaning |
| --- | --- | --- |
| `PORT_ENABLED` | `true` | `false` turns the daily port off |
| `PORT_REVIEW` | `true` | `false` skips the adversarial review session |
| `PORT_APP_ID` / `PORT_APP_CLIENT_ID` | unset | the pushing GitHub App |
| `PORT_GIT_NAME` | `github-actions[bot]` | bot commit name |
| `PORT_GIT_EMAIL` | `41898282+github-actions[bot]@users.noreply.github.com` | bot commit email; commits by any other email count as human |
| `PORT_GIT_ID` | `41898282` | bot user id given to the agent action |
| `PORT_MODEL` | `claude-opus-5-5` | port and fix sessions |
| `PORT_REVIEW_MODEL` | `PORT_MODEL` | review session |
| `PORT_FALLBACK_MODEL` | `claude-sonnet-5-5` | fallback model |
| `PORT_BUDGET_USD` | `30` | port session, adapter-sized chunk (150 turns) |
| `PORT_CORE_BUDGET_USD` | `60` | port session, heavy chunk (300 turns) |
| `PORT_REVIEW_BUDGET_USD` | `10` | review session (60 turns) |
| `PORT_FIX_BUDGET_USD` | `15` | fix and repair sessions (100 turns) |
| `PORT_MAX_COMMITS` | `3` | upstream commits per chunk |
| `PORT_MAX_PR_COMMITS` | `10` | upstream commits on an open port pull request before the planner waits for a merge |
| `PORT_MAX_REPAIRS` | `2` | `port-repair:` attempts before escalating |
| `PORT_MAX_GO_LINES` | `3000` | changed Go lines per chunk |
| `CONFORMANCE_REQUIRED` | `false` | `true` makes a missing Go fixture harness fail CI and the gates |
| `PARITY_SYNC_ENABLED` | `true` | `false` turns the parity-harness sync off |

When you use a GitHub App, set `PORT_GIT_NAME`, `PORT_GIT_EMAIL` and `PORT_GIT_ID` to the App's bot
user: `<app-slug>[bot]`, `<user-id>+<app-slug>[bot]@users.noreply.github.com` and `<user-id>`.
The bot identity is never a personal email.

### Repository settings

- **Pull requests:** allow merge commits only. Turn off squash merging and rebase merging.
- **`main` ruleset:**
  - Require a pull request and the `ci-ok` status check.
  - Block force pushes and deletion.
  - Do not require linear history, because upstream merges are merge commits.
- **`port/upstream-sync`:** leave it unprotected for the pushing App or token. The publish job
  force-pushes with a lease when it rebuilds the branch.
- **`parity-harness`:** allow the pushing App or token to push merges.
- **Labels:** the scripts create the labels they use (`upstream-port`, `port:green`, `port:red`,
  `port:partial`, `port:needs-human`, `port:run-failed`, `parity-debt`, `parity-harness`).

## Attribution policy

No fork commit or pull request carries Claude or other AI attribution:

- no `Co-Authored-By` trailer naming Claude or Anthropic
- no "Generated with Claude Code" or robot-emoji line
- no claude.ai session link
- no Claude author or committer

The policy is enforced in three places:

1. the agents' settings (attribution off, `includeCoAuthoredBy: false`, no git commit access);
2. the bot identity that commits;
3. `guard.sh`, which checks the first-parent fork commits of every pull request and the pull
   request text.

Upstream commits are never checked or rewritten: some carry such trailers, and they are merged
as they are. Text from upstream subjects or from agents passes through `port_sanitize` before it
reaches a commit or a pull request. Lines about the fork's own generator, such as "Generated with
`cargo run -p pbs-devtools -- all`", are fine.

## Runbook

**The port pull request is green.** Review it and merge it with "Create a merge commit". The next
run continues from the new marker.

**The port pull request is red.** The daily run repairs it, up to `PORT_MAX_REPAIRS` times,
provided CI on `main` is green (while `main` is red, the planner waits). Then it escalates with
`port:needs-human`. You can fix things
yourself: push commits to `port/upstream-sync`, where human commits are allowed. Remove
`port:needs-human` to hand the branch back. If the branch carries human commits and conflicts
with `main`, the planner escalates instead of rebuilding it.

**An upstream commit cannot be ported now.** Add its full SHA, with a comment, to
`.github/port/skip-commits.txt` by pull request. The port still merges it, but no agent is asked
to port it, and the log records `skipped`. To merge a chunk without agents, run the workflow with
mode `merge-only`. That records the commits as `merged-not-ported` and leaves the marker where it
is, so a later run ports them.

**Every agent step fails, and the commits are logged `blocked`.** Check the agent step log. The
agent action runs only for a human actor. A scheduled run acts as the user who last changed the
`cron` line of `upstream-port.yml`, so that change must come from a person, not an App or bot. A
run started by hand acts as the person who started it. Also check the API key or OAuth token.

**Manual runs.** `mode` takes `auto`, `advance`, `repair`, `merge-only` or `dry-run`:

- `advance` ports new commits even when the pull request is red.
- `dry-run` plans and ports as `auto` would, but publishes nothing. Read the job summary and the
  `port-result` artifact.

`max_commits` limits the chunk. `target_sha` stops at a given upstream first-parent commit.

**A pull request was squash- or rebase-merged.** The tree is right, but the upstream ancestry is
gone. Restore it on `main` without changing any file, by pull request:

```sh
git switch -c restore-upstream-ancestry origin/main
git fetch https://github.com/prebid/prebid-server.git master
git merge -s ours --no-ff -m "Restore upstream ancestry" "$(sed -n '/^[0-9a-f]\{40\}$/p' rust/UPSTREAM_SYNC)"
```

**The parity-harness sync reports a conflict.** Merge `main` into `parity-harness` by hand,
resolve the conflict, and push. The next sync closes the issue.

**The port job failed or was cancelled.** See the "Upstream port run failed" issue and the
linked run. Nothing was pushed. The next scheduled run plans again from scratch.

## Local commands

The scripts need bash 4.4 or newer, git, jq and perl. On macOS, install bash with Homebrew.

```sh
.github/port/tests/run-all.sh                  # port tooling tests (or name some: run-all.sh guard plan)
rust/scripts/lint-shell.sh                     # strict shellcheck of every script
rust/scripts/check-no-lint-suppression.sh      # the suppression ban; self-tests: rust/scripts/tests/run.sh
(cd rust && taplo fmt --check --config taplo.toml)
(cd rust && cargo deny --locked check --deny warnings)
rust/scripts/cargo-machete.sh
rust/scripts/lint-go.sh

# Fork invariants for the current branch (needs the upstream ref):
git fetch https://github.com/prebid/prebid-server.git +refs/heads/master:refs/remotes/upstream/master
.github/port/scripts/guard.sh --ref HEAD --base origin/main

# What an upstream commit needs, and the conformance ratchet:
.github/port/scripts/classify.sh <sha>^1 <sha>
.github/port/scripts/conformance.sh --strict-dirs <adapter,...>
```

The rules the agents follow are in `.github/port/PORT_SYSTEM.md`. The map from Go packages to Rust
crates is in `.github/port/path-map.md`.
