# prebid-server-rust

A Rust port of [Prebid Server](https://github.com/prebid/prebid-server), the Go server-side header
bidding service. The goal is exact parity: the Rust server should be able to take the Go server's
production traffic without anyone noticing. HTTP status codes, response bodies, outgoing bidder
requests, configuration keys and defaults, environment variables and metric names must all match
Go. Where Go is awkward, inconsistent or buggy, the port reproduces it.

This repository holds only the Rust port and its test tooling. The upstream Go server is the
reference implementation and is never committed here. Builds and tests check it out at a pinned
commit.

## Status

Early. The foundations are ported, but the server itself is not, and nothing serves traffic yet.

| Area | Crates |
| --- | --- |
| Go behaviour the server depends on | `pbs-gojson` (+ derive), `pbs-gocompat`, `pbs-goyaml` (+ derive), `pbs-goviper`, `pbs-gogzip`, `pbs-gohttp-client`, `pbs-gohttp-server`, `pbs-freecache` |
| Prebid Server packages | `pbs-errortypes`, `pbs-util`, `pbs-logger`, `pbs-metrics`, `pbs-currency`, `pbs-gpp`, `pbs-tcf2`, `pbs-openrtb`, `pbs-openrtb-ext`, `pbs-bidder-params` |
| Tooling | `pbs-devtools` (ground truth from Go, type generation), `pbs-parity` (parity-harness seams) |
| Not ported yet | the endpoints, the exchange, the bidder adapters, modules, stored requests, privacy enforcement and the server binary |

## Getting started

You need git, bash 4.4 or newer and a Rust toolchain installed with rustup (`rust-toolchain.toml`
pins the version, and rustup installs it on first use). The scripts in `scripts/` need that bash:
on macOS, `/bin/bash` is 3.2 and they stop at once, so put a newer bash first on `PATH`.
Regenerating or checking goldens also needs Go.

```sh
git clone https://github.com/aryehlev/prebid-server-rust.git
cd prebid-server-rust
scripts/fetch-upstream.sh    # check out the pinned upstream Go server into .upstream/
cargo test --workspace
```

`UPSTREAM_SYNC` names the upstream commit the port covers. `scripts/fetch-upstream.sh` makes a
detached checkout of exactly that commit in `.upstream/`, which git ignores. It does nothing when
the checkout is already there and clean, and it refuses to touch a checkout with local changes.

- `--dir DIR` puts the checkout somewhere else. Point the tests at it with `PBS_GO_ROOT=DIR`.
- `--sha SHA` checks out another commit.
- `PBS_UPSTREAM_REFERENCE=/path/to/clone` fetches the commit from a local clone, with no network,
  when that clone has it. `PBS_UPSTREAM_URL` changes the remote.

Tests and tools read upstream sources, corpora and static files from the *Go root*: `$PBS_GO_ROOT`
when it is set, else `.upstream/`. They never search for it. If it is missing, they fail and say to
run the script.

A change must pass these checks, all at full strictness:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo +nightly-2026-11-13 fmt --all -- --check     # rustfmt.toml uses unstable options
cargo deny --locked check --deny warnings
cargo machete
taplo fmt --check
cargo run -p pbs-devtools -- all --check           # the testdata/ ground truth and generated types
scripts/check-no-lint-suppression.sh               # nothing suppresses a lint
scripts/tests/run.sh                               # the self-tests of that scanner
scripts/lint-shell.sh                              # shellcheck at its strictest on every script
```

The workflows under `.github/` do not run them yet: they come from the original fork, which kept
the port in `rust/` beside the Go server, and have not been moved to this layout.

No lint is ever suppressed: there is no `#[allow]` or `#[expect]` anywhere. `PORTING.md` has the
whole policy.

## How parity is tested

The Go code at the pinned commit is the oracle. The Rust tests compare against what it actually
does, not against a description of it.

- **Differential goldens.** A crate that needs them has a Go program in
  `crates/<crate>/testdata/go/`. It runs the real Go code (Prebid Server's packages, the standard
  library, the exact versions of jsoniter, viper, yaml.v3, go-gdpr and the other dependencies)
  over hand-picked, adversarial and seeded random inputs and writes the results as goldens. The
  Rust tests replay them byte for byte. Each program is its own Go module. Its `go.mod` requires
  the upstream module at the pinned commit and replaces it with `.upstream/`, so a plain `go run`
  builds against that checkout even when `PBS_GO_ROOT` names another one (`pbs-devtools` runs its
  own programs against the Go root). Its doc comment gives the command that regenerates its
  goldens. `pbs-parity`'s goldens are the exception: its program needs the Go parity harness,
  which only the `parity-harness` branch holds, so they are regenerated on that branch
  (`crates/parity/testdata/go/run.sh` there).
- **Ground truth from Go.** `pbs-devtools` writes `testdata/`: the adapter test manifest, reflected
  struct metadata, viper defaults, bidder infos and bidder names. It also generates Rust types from
  Go packages. `cargo run -p pbs-devtools -- all --check` fails when any of it is stale.
- **Upstream's own corpora.** The adapter JSON fixtures, sample requests and other test data are
  read from the Go root and replayed with the comparison rules of the Go harnesses.
  [`conformance/`](conformance/README.md) holds the ratchet of adapter fixtures that may still fail.
  That list only shrinks.

[`PORTING.md`](PORTING.md) is the porting contract: parity rules, layout, testing against Go, the
lint policy and performance rules.

## The daily upstream port

The plan is a scheduled workflow that keeps the port in step with upstream. It takes the upstream
commits after `UPSTREAM_SYNC` and checks them out with `scripts/fetch-upstream.sh`. Agents port
the behaviour changes and write differential tests for them, and the workflow advances the pin.
It then opens a pull request, which must pass the same checks. `upstream-port-log.jsonl` is where
it records every upstream commit it handled and the Go behaviour it left unported. Upstream
commits are never copied into this repository.

That workflow does not exist yet. `.github/workflows/upstream-port.yml` and the scripts in
`.github/port/` are the original fork's, which merged upstream into the fork; `.github/README.md`
describes them.

## License

Apache License 2.0. See [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).

This project is a port of Prebid Server, which Prebid.org and its contributors develop at
[prebid/prebid-server](https://github.com/prebid/prebid-server) under the same license. It is an
independent project, not affiliated with Prebid.org.
