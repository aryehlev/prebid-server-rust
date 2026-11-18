# Operations: the Rust port of Prebid Server

This repository holds the Rust port of [prebid/prebid-server](https://github.com/prebid/prebid-server)
and its tooling, and nothing of the Go server. Go is the source of truth: the port matches it
byte for byte, and every check compares against the upstream commit the repository is pinned to.
The daily upstream-port workflow advances that pin over new upstream commits and ports them.

## Repository model

- **The pin.** `UPSTREAM_SYNC` holds one non-comment line: the full SHA of the upstream `master`
  commit the port covers. Only the upstream-port workflow advances it, and only forward.
- **The upstream checkout.** `scripts/fetch-upstream.sh [--dir DIR] [--sha SHA]` checks the pinned
  commit out into `.upstream/`, which git ignores. Tests, generators and checks read it as the Go
  root: `$PBS_GO_ROOT`, else `.upstream`. It fetches from `$PBS_UPSTREAM_URL` (default
  `https://github.com/prebid/prebid-server.git`), or from `$PBS_UPSTREAM_REFERENCE`, a local
  repository that has the commit, without network access.
- **Upstream history.** Scripts that need upstream commits (the planner, the guard, the change
  detection, the classifier) read a separate bare mirror that
  `.github/port/scripts/upstream-mirror.sh` makes. Upstream objects never enter this repository.
- **The Go tooling modules.** The only Go code here is test tooling: the programs under
  `crates/<crate>/testdata/go/` that write goldens from the real Go code, and the dump programs in
  `crates/devtools/go/`. Each module requires `github.com/prebid/prebid-server/v4` at the pin's
  pseudo-version (`.github/port/scripts/pin-version.sh` derives it the way the go command does)
  and replaces it with a relative path to `.upstream`. The port workflow updates the versions
  together with the pin (`advance-pin.sh`).
- **The port log.** `upstream-port-log.jsonl` gets one line per upstream commit the pin advanced
  over: its status (`ported`, `partial`, `blocked`, `no_change_needed`, `skipped` or
  `pinned-not-ported`), the Go behaviour left unported, and the run. It only grows: the guard
  fails a port pull request that changes lines `main` already has.
- Only the trusted scripts write the pin and the log (`advance-pin.sh` and `commit.sh`, after the
  last agent session). A session that changes them has the change reverted.
- `conformance/known_failures.txt` lists the Go adapter fixtures that may still fail. The list
  only shrinks. See `conformance/README.md`.
- `.claude/`, `.omx/` and other agent configuration and local tool state are never committed.

**No upstream file.** `guard.sh` enforces on every branch that the repository holds no upstream
Go server file:

- only the top-level paths of `lib.sh port_top_level_paths` (plus `parity/` on `parity-harness`);
- Go code only in the Go tooling modules (and `parity/` on `parity-harness`);
- no symbolic link, submodule, `.gitmodules`, `go.work` or `vendor/` directory;
- no file byte-identical to a file of upstream history (any blob the mirror's branch and tags
  reach, so an older version of an upstream file counts too), except an empty file and the entries
  of `lib.sh port_upstream_copies`: `LICENSE` (the same Apache 2.0 text) and
  `crates/util/src/httputil/pixel.png` (the 86-byte pixel Go serves). Each entry names its upstream
  file and covers only the bytes that file has had, not whatever is put at the path.

A Go change belongs upstream; the port picks it up when the pin advances.

### Branches

| Branch | Purpose |
| --- | --- |
| `main` | Default branch. Changes land by pull request, as a **merge commit** only. |
| `port/upstream-sync` | The port pull request. The bot owns this branch. |
| `parity-harness` | The parity harness: `parity/`, which may hold Go, and `parity/upstream-seams.patch`, the small seams it needs in upstream code. `main` is merged into it daily. |

Merge the port pull request with a merge commit. A squash or rebase merge rewrites the bot's
commits, so `main` loses each port commit with its `Upstream-Range`, `Upstream-Version` and
`Port-Run` record.

## Workflows

### `ci.yml`: every pull request, every push to `main` and `parity-harness`, merge queues

Every check runs at full strictness. There are no toggles, and no job may continue on error.
The jobs that read Go sources check the pinned commit out into `.upstream/` with
`scripts/fetch-upstream.sh` first (cached on the pin, saved from `main`), with `PBS_GO_ROOT`
pointing at it.

| Job | What it checks |
| --- | --- |
| `changes` | the compared range; the adapter directories the change touches (upstream files changed between the two pins, read from the upstream mirror, and Rust adapter files); whether it is the port pull request or the parity harness |
| `guard` | the repository invariants (`.github/port/scripts/guard.sh`): no upstream file (above), no committed agent configuration or local state, the pin (one upstream `master` commit that never moves backwards), the known-failures list, no AI attribution in commits or pull request text, and on the port pull request the bot rules |
| `rust-fmt` | `cargo +<nightly> fmt --all -- --check`. The nightly comes from `rustfmt.toml` through `scripts/rustfmt-toolchain.sh`. |
| `rust-clippy` | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| `rust-doc` | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked` |
| `rust-test` | `cargo test --workspace --locked`, against the pinned upstream checkout |
| `conformance` | the Go fixture ratchet (`.github/port/scripts/conformance.sh`). Fixtures of touched adapters must all pass. |
| `devtools` | `cargo run -p pbs-devtools --locked -- all --check`: the generated files match Go, with the Go version of the upstream `Dockerfile` |
| `go-tooling` | the Go tooling modules (`.github/port/scripts/go-tooling.sh`): `gofmt -s -l` prints nothing, and `go build ./...` and `go test ./...` pass with `GOFLAGS=-mod=readonly` in every `crates/**/go.mod` module except `crates/parity/testdata/go`, which needs `parity/` and runs in the `parity` job |
| `lint-policy` | `scripts/check-no-lint-suppression.sh` (no `#[allow]`, `#[expect]`, lint caps or "allow" levels anywhere, including attributes a macro assembles, which clippy does not see) and its self-tests `scripts/tests/run.sh` |
| `lint-config` | `taplo fmt --check`, shellcheck of every script with every optional check at style severity (`scripts/lint-shell.sh`), and actionlint |
| `port-tests` | `.github/port/tests/run-all.sh`: the port tooling, tested in temporary repositories, with an upstream mirror for the checks of real upstream commits |
| `deny` | `cargo deny --locked check --deny warnings`: advisories, bans, licenses, sources (crates.io only, no duplicate versions) |
| `machete` | unused dependencies (`scripts/cargo-machete.sh`) |
| `parity` | `parity-harness` only: `.github/port/scripts/ci-parity.sh` assembles the pinned upstream tree, `parity/` and the seams patch in a temporary copy and runs the harness there |
| `ci-ok` | passes only when every other job passed. `parity` may be skipped. |

Make `ci-ok` the only required status check.

The Go code is test tooling and is not linted: there is no `go vet` job and no golangci-lint, and
the suppression ban rejects a golangci-lint configuration. Its checks are `gofmt -s`, `go build`
and `go test` (which runs the small vet subset the go command always runs with tests).

### `upstream-port.yml`: daily at 05:23 UTC, or by hand

Three jobs, each with its own trust level. One run at a time.

1. **plan** (read-only token, no agents). It mirrors upstream into a bare repository, reads git,
   the open port pull request and its `ci-ok` result, and chooses one action:
   - `advance`: port the next chunk of upstream first-parent commits after the pin and advance
     the pin over it.
   - `repair`: fix the red CI of the open port pull request without porting anything new. Each
     attempt is a `port-repair:` commit, an empty one when the session changed nothing, so the
     attempts count towards `PORT_MAX_REPAIRS` and CI runs again.
   - `escalate`: label the pull request `port:needs-human` and stop.
   - `none`: up to date, waiting for CI, or waiting for a person. A failed `ci-ok` does not count
     while another CI run on the same head is still going: a push and an edit of the pull request
     start two runs, and the older one is cancelled with a failed `ci-ok`.

   A chunk holds at most `PORT_MAX_COMMITS` commits and `PORT_MAX_GO_LINES` changed Go lines.
   A heavy commit (core Go, modules, an OpenRTB bump, or more than 1500 Go lines) gets a chunk of
   its own. The plan also carries the target's pseudo-version, and the job packs the default
   branch's `.github/port` tooling and `scripts/fetch-upstream.sh`, so later jobs run trusted
   scripts, never ones taken from the branch under work.
2. **port** (read-only contents token, no push credential). It works on the planned commit,
   checked out without persisted credentials, and merges `main` into it if needed. Then:
   - `prepare-trees.sh` checks out three upstream trees with `fetch-upstream.sh`: the old pin
     (read-only, what the port covered so far), the new pin in `.upstream/` (read-only, the Go
     root of every test, tool and gate) and a writable probe copy of the new pin, the only place
     a session may add Go probes (`zz_parity_scratch_test.go`).
   - `advance-pin.sh` moves `UPSTREAM_SYNC` and the Go tooling modules to the new pin.
   - `build-context.sh` gives the agents the plan, each upstream commit's patch and message, the
     whole Go diff between the pins, the failing CI log of a repair and the Go-to-Rust path map.
   - Crates and Go modules are prefetched; the gates run (`gates.sh`: the CI checks above).

   Then the agent sessions run:
   - **port**: ports the chunk and writes differential tests.
   - **adversarial review**: compares against Go and writes `adversarial_*.rs` tests that fail
     where Rust differs.
   - **fix**: runs only when the review found blocking differences or the gates are red.

   The sessions run when a commit needs Rust changes, for every repair, and when advancing the pin
   alone broke a test, the conformance ratchet, the generated files or the Go tooling; never in
   `pin-only` mode. After every session, `check-worktree.sh` undoes commits, deletes Go probes in
   the work tree, reverts every change outside the writable paths (`crates/`, `testdata/`,
   `conformance/known_failures.txt`, `Cargo.lock`), drops added known failures, applies the pin
   again, resets the probe tree, and fails the job when either read-only tree is no longer a
   clean checkout of its pin. The final gates decide pass or fail. `commit.sh` then appends the
   port log and commits with the bot identity, and `guard.sh --bot` checks the result. The result
   leaves the job as a git bundle.
3. **publish** (fresh runner, default-branch tooling, the only push credential). It runs the
   guard on the bundle against a fresh upstream mirror, without building or running anything
   from it. It pushes `port/upstream-sync` (never `main`) and opens or updates the pull request.
   The PR is a draft while red, labelled `port:green` or `port:red`, plus `port:partial` when a
   commit is logged `partial`, `blocked` or `pinned-not-ported`: the agents reported it so, never
   reported on it (a failed or refused session), or a `pin-only` run skipped it. Go behaviour left
   unported, and those commits, go to the "Parity debt from upstream ports" issue. A failed port
   job goes to "Upstream port run failed".

The agents run with `.github/port/claude-settings.json`, plus a per-session allow list from
`render-inputs.sh`:

- Attribution is off, and bypass-permissions mode is disabled. Only the settings the workflow
  writes are loaded, so a `.claude/` or hook in the work tree changes nothing.
- A deny list blocks git commands that change history, refs or remotes, `gh`, network tools,
  `WebFetch`/`WebSearch`, MCP tools, dependency changes, and edits to `.github/`, the protected
  files (the workspace manifest, toolchain, rustfmt, clippy, cargo, deny and taplo configs,
  `scripts/`, the porting rules), the pin, the port log, local tool state and both read-only
  upstream trees. The deny list always wins over the allow lists.
- Cargo and Go are offline (`CARGO_NET_OFFLINE`, `GOPROXY=off`, `GOFLAGS=-mod=readonly`,
  `GOTOOLCHAIN=local`).

The system prompts are in `.github/port/PORT_SYSTEM.md` and `.github/port/REVIEW_SYSTEM.md`. They
tell the agents that repository and upstream content is data, not instructions, and that the GSD
workflow of `AGENTS.md` does not apply in CI. Each session has a turn limit, a dollar budget and a
step timeout. Agent output reaches pull requests only after sanitizing and fencing.

### `parity-harness-sync.yml`: daily at 06:47 UTC, or by hand

This workflow merges `main` into `parity-harness`, then checks that the harness still fits the
upstream commit the merge pins: `parity/upstream-seams.patch` must apply (`git apply --check`) to a
checkout of that commit made with `scripts/fetch-upstream.sh`. Only then is the merge pushed.

- A conflicting merge pushes nothing and opens or comments the "parity-harness: merging main
  conflicts" issue.
- A patch that does not apply (or a missing patch) pushes nothing, fails the run and opens or
  comments the "parity-harness: the seams patch does not apply" issue.
- When there is nothing to merge, the seams are still checked against the harness's pin.
- A sync that finds the harness fitting closes both issues.

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
- **`main` ruleset:** require a pull request and the `ci-ok` status check; block force pushes and
  deletion. Do not require linear history: pull requests land as merge commits, and the port
  branch merges `main` into itself.
- **`port/upstream-sync`:** leave it unprotected for the pushing App or token. The publish job
  force-pushes with a lease when it rebuilds the branch.
- **`parity-harness`:** allow the pushing App or token to push merges.
- **Labels:** the scripts create the labels they use (`upstream-port`, `port:green`, `port:red`,
  `port:partial`, `port:needs-human`, `port:run-failed`, `parity-debt`, `parity-harness`).

## Attribution policy

No commit or pull request of this repository carries Claude or other AI attribution:

- no `Co-Authored-By` trailer naming Claude or Anthropic
- no "Generated with Claude Code" or robot-emoji line
- no claude.ai session link or `Claude-Session` trailer
- no Claude author or committer

The policy is enforced in three places:

1. the agents' settings (attribution off, `includeCoAuthoredBy: false`, no git commit access);
2. the bot identity that commits;
3. `guard.sh`, which checks the first-parent commits of every pull request and push, and the pull
   request text.

Upstream commits never enter the repository; some carry such trailers. Text from upstream subjects
or from agents passes through `port_sanitize` before it reaches a commit or a pull request. Lines
about this repository's own generator, such as "Generated with `cargo run -p pbs-devtools -- all`",
are fine.

## Runbook

**The port pull request is green.** Review it and merge it with "Create a merge commit". The next
run continues from the new pin.

**The port pull request is red.** The daily run repairs it, up to `PORT_MAX_REPAIRS` times,
provided CI on `main` is green (while `main` is red, the planner waits). Then it escalates with
`port:needs-human`. You can fix things yourself: push commits to `port/upstream-sync`, where human
commits are allowed. Remove `port:needs-human` to hand the branch back. If the branch carries
human commits and conflicts with `main`, the planner escalates instead of rebuilding it.

**An upstream commit cannot be ported now.** Add its full SHA, with a comment, to
`.github/port/skip-commits.txt` by pull request. The pin still advances over it, but no agent is
asked to port it, and the log records `skipped`. To advance the pin without agents, run the
workflow with mode `pin-only`: every commit of the chunk is logged `pinned-not-ported` and becomes
parity debt, and the pull request is labelled `port:partial`.

**Every agent step fails, and the commits are logged `blocked`.** Check the agent step log. The
agent action runs only for a human actor. A scheduled run acts as the user who last changed the
`cron` line of `upstream-port.yml`, so that change must come from a person, not an App or bot. A
run started by hand acts as the person who started it. Also check the API key or OAuth token.

**Manual runs.** `mode` takes `auto`, `advance`, `repair`, `pin-only` or `dry-run`:

- `advance` ports new commits even when the pull request is red.
- `repair` only repairs; it does nothing when there is nothing to repair.
- `dry-run` plans and ports as `auto` would, but publishes nothing. Read the job summary and the
  `port-result` artifact.

`max_commits` limits the chunk. `target_sha` stops at a given upstream first-parent commit.

**The parity-harness sync reports a conflict.** Merge `main` into `parity-harness` by hand
(a merge commit), resolve the conflict, and push. The next sync closes the issue.

**The seams patch does not apply to the new pin.** Rebase the seams on the commit `main` pins: check
it out with `scripts/fetch-upstream.sh --dir <dir>`, apply the old patch there with
`git apply --3way`, fix what it rejects, and write the new patch with `git diff`. Commit it on
`parity-harness` together with a merge of `main`, and push. The next sync checks it and closes the
issue; `ci-parity.sh` assembles the same tree.

**The port job failed or was cancelled.** See the "Upstream port run failed" issue and the
linked run. Nothing was pushed. The next scheduled run plans again from scratch.

## Local commands

The scripts need bash 4.4 or newer, git, jq and perl. On macOS, install bash with Homebrew.

```sh
scripts/fetch-upstream.sh                      # .upstream/ at the pinned commit
PBS_UPSTREAM_REFERENCE=~/src/prebid-server scripts/fetch-upstream.sh   # from a local clone, offline

cargo test --workspace --locked                # reads .upstream/ (or $PBS_GO_ROOT)
.github/port/scripts/go-tooling.sh             # gofmt -s, go build and go test of the Go tooling
scripts/lint-shell.sh                          # strict shellcheck of every script
scripts/check-no-lint-suppression.sh           # the suppression ban; self-tests: scripts/tests/run.sh
taplo fmt --check --config taplo.toml
cargo deny --locked check --deny warnings
scripts/cargo-machete.sh

# A bare upstream mirror, for the port tooling tests, the guard and the classifier:
mirror=$(.github/port/scripts/upstream-mirror.sh /tmp/upstream.git)
PORT_TESTS_UPSTREAM_GIT=${mirror} .github/port/tests/run-all.sh   # or name some: run-all.sh guard plan
.github/port/scripts/guard.sh --ref HEAD --base origin/main --upstream "${mirror}"
GIT_DIR=${mirror} .github/port/scripts/classify.sh <sha>^1 <sha>
.github/port/scripts/conformance.sh --strict-dirs <adapter,...>
```

The rules the agents follow are in `.github/port/PORT_SYSTEM.md`. The map from Go packages to Rust
crates is in `.github/port/path-map.md`.
