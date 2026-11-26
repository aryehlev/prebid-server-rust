# prebid-server-rust

A Rust port of [Prebid Server](https://github.com/prebid/prebid-server), the Go server-side header
bidding service. The goal is exact parity: the Rust server should be able to take the Go server's
production traffic without anyone noticing. HTTP status codes, response bodies, outgoing bidder
requests, configuration keys and defaults, environment variables and metric names must all match
Go. Where Go is awkward, inconsistent or buggy, the port reproduces it.

This repository holds only the Rust port and its test tooling. The upstream Go server is the
reference implementation and is never committed here. Builds and tests check it out at a pinned
commit.

## Status and roadmap

Early. The foundations are ported and checked against Go, but the server itself is not. **The
server binary does not exist yet**: the workspace has no `pbs-server` crate, so nothing serves
traffic, and nothing can be compared with the Go server end to end yet.

Ported today, each checked against the real Go code at the pinned commit (the goldens and corpora
described under "How parity is tested"):

| Area | Crates |
| --- | --- |
| Go behaviour the server depends on | `pbs-gojson` (+ derive: `encoding/json` and json-iterator), `pbs-gocompat` (`net/http`, `net/url`, `strconv`, `fmt`, `text/template`, `time` and more), `pbs-goyaml` (+ derive: yaml.v3 and mapstructure), `pbs-goviper` (viper 1.12), `pbs-gogzip` (`compress/flate` and `compress/gzip`, byte for byte), `pbs-gohttp-client` (the `net/http` client and transport, with Go's certificate verification), `pbs-gohttp-server` (the `net/http` server, httprouter and rs/cors), `pbs-freecache` |
| Prebid Server packages | `pbs-errortypes`, `pbs-util`, `pbs-logger` (glog output), `pbs-metrics` (metric names and labels), `pbs-currency`, `pbs-gpp` (go-gpp), `pbs-tcf2` (go-gdpr), `pbs-openrtb` (prebid/openrtb), `pbs-openrtb-ext`, `pbs-bidder-params` (gojsonschema error text) |
| Tooling | `pbs-devtools` (ground truth from Go, type generation), `pbs-parity` (the Rust side of the parity harness's seams) |

Planned:

- **The server.** The endpoints (`/openrtb2/auction`, `/openrtb2/amp`, `/openrtb2/video`,
  `/cookie_sync`, `/setuid`, `/event`, `/vtrack`, `/info/*`), the exchange, the bidder adapters,
  the modules, stored requests, privacy enforcement, analytics, and the `pbs-server` binary. Once
  the `pbs-adapters` crate exists, the conformance job replays every Go adapter fixture against
  the Rust adapters. Once the binary exists, the parity harness runs it against the instrumented
  Go server on every push to `parity-harness`.
- **51Degrees device detection, with the full engine.** The module will call the same
  EUPL-licensed C engine that the Go module uses, not a reimplementation of it. Its binding crate
  will be the only crate allowed `unsafe` code; everywhere else `unsafe` stays forbidden.
- **Automatic HTTP/2 and h2c to bidders, with no configuration.** HTTPS bidders negotiate `h2`
  through ALPN. For cleartext bidders, a background probe that sends only the HTTP/2 connection
  preface marks the bidders that accept h2c; every other bidder stays on HTTP/1.1. Only the
  framing changes: the content of bidder requests and the auction results never do.

## HTTP/2 and h2c to bidders

HTTP/2 to bidders is an extension beyond Go, and it is off by default. Go's bidder client speaks
HTTP/1.1 only (router.go:143-158), and so does this one while `http_client.http2.enabled` is
false: its TLS handshakes offer only `http/1.1`, it sends no h2c probe, and its requests,
connections and metrics are Go's (`http-client.http2-opt-in`). Parity runs keep it off. To turn it
on, set the key to true in pbs.yaml or with `PBS_HTTP_CLIENT_HTTP2_ENABLED=true`; the value is read
as Viper reads any bool key with a default, so `1`, `t` and `TRUE` are true too, and anything else,
`yes` and `on` included, is false.

With the key on, the bidder client speaks HTTP/2 to the bidders that take it, by itself: there is no
per-bidder setting and no other knob. Every other client stays on HTTP/1.1 whatever the key says, as
router.go's clients do, and so do the currency and analytics clients, which Go's
`http.DefaultTransport` would take to h2 (`http-client.default-transport-h2`). Only the framing
changes: a bid request carries the same method, path, fields and body over HTTP/2 as over HTTP/1.1,
but for the fields the last item below names, and auction results differ from Go's only in the
fallback and redirect cases below.

- **HTTPS bidders.** The TLS handshake offers ALPN `h2` and `http/1.1`, and requests go in
  whichever the bidder picks. A bidder that picks `http/1.1` gets its requests over HTTP/1.1 for
  ten minutes; the connection that learned it is closed unused, so such a bidder sees one more TLS
  handshake every ten minutes.
- **Cleartext bidders.** The first bid request to an origin goes over HTTP/1.1 at once and starts
  a probe beside it. The probe is one connection that sends the HTTP/2 connection preface and an
  empty SETTINGS frame, never a bid request. A SETTINGS frame in answer marks the origin h2c, and
  the probe then says GOAWAY and closes. Anything else (an HTTP/1.1 answer, a close, or no answer
  within five seconds) keeps the origin on HTTP/1.1. The verdict holds for ten minutes, and the
  origin's first request after that starts a new probe. There is one probe per origin, run only
  for an origin a bid request is about to use, never at startup. An HTTP/1.1 bidder reads the
  probe as a `PRI * HTTP/2.0` request: Go's server answers it with 400 behind a ServeMux, and runs
  a bare handler for it.
- **The pool.** Each bidder origin on h2 or h2c has a pool of HTTP/2 connections. A request takes
  the connection with the most free streams: the bidder's `SETTINGS_MAX_CONCURRENT_STREAMS` less
  the streams open on it. Requests that arrive while a connection is opening wait for its SETTINGS
  (at most a second) and then choose. When every connection is full, another one opens, up to
  `max_connections_per_host` (0 means no cap); each connection holds one of the host's slots for
  as long as it is open, as Go's `MaxConnsPerHost` counts connections. At the cap, a request waits
  for a free stream or a slot, as it waits for a slot over HTTP/1.1. A connection that gets GOAWAY
  or fails leaves the pool, and the streams already on it finish there. A connection with no
  stream for `idle_connection_timeout_seconds` closes. Each pooled connection counts once in the
  adapter connection metrics, and every other request on it counts as a reused connection.
- **Fallback.** When a request over h2c fails, the origin goes back to HTTP/1.1 for ten minutes.
  The request is sent again over HTTP/1.1, once, only when the origin read none of it: it
  answered the preface with HTTP/1.x, or the connection failed before it was ready for the
  request. A request whose pooled connection turned out gone before its stream opened tries
  another pooled connection first, at most twice. When it failed any other way (an HTTP/2 error, a
  reset or a close once the connection had the request, after which the origin may have read it),
  it is not sent again, and it is a bidder error with no bid.
- **Redirects.** Over h2c a redirect is followed when it stays on the origin. One to another
  origin comes back unfollowed, and the adapter reads the 3xx as the bidder's answer: that request
  brings no bid, where Go's client would follow a 307 or 308 and could still get one. The first
  such answer moves the origin back to HTTP/1.1 for ten minutes, where its requests follow
  redirects as Go's do; requests already on their way over h2c then come back unfollowed too when
  the bidder redirects them the same way. A later probe can mark the origin h2c again, so a bidder
  that keeps redirecting to another origin loses those requests once in every ten minutes or so.
- **Proxies.** A request that goes through a proxy never takes h2c, starts no probe and does not
  use the pool. An https request through a proxy negotiates h2 with the bidder inside the proxy's
  tunnel, on one connection.
- **The general client.** Go's bidders share one client with the modules, the stored-request
  fetchers and the vendor list. Here that client is two halves over one transport
  (`Builder::build_general`). With the key off, the halves share every connection, as Go's callers
  do. With it on, only the bidders' half negotiates HTTP/2; both halves still wait for the same
  `max_connections_per_host` slots and share their cleartext HTTP/1.1 connections, but the bidders'
  TLS connections are their own, so a bid request does not take a TLS connection that a module or
  a fetcher left idle to the same host.
- **What differs on the wire.** HTTP/2's framing, header compression and SETTINGS are those of the
  `h2` crate, not Go's, and field names are in lower case, as they are over HTTP/1.1 here.
  `User-Agent` stays `Go-http-client/1.1`, where Go's own HTTP/2 sends `Go-http-client/2.0`. The
  host goes as the `:authority` pseudo-header field. A POST without a body carries
  `content-length: 0`, as Go's does, where this client's HTTP/1.1 sends no length. The
  connection-specific fields HTTP/2 forbids are left out, as Go's own HTTP/2 leaves them out:
  gamma's adapter adds `Connection: keep-alive`, which Go's bidder client sends and a gamma that
  takes h2 no longer gets. A request whose stream the bidder refuses unread (`REFUSED_STREAM`), or
  leaves out of a graceful `GOAWAY`, goes again over HTTP/2, at most twice more. A pooled
  connection carries many of a bidder's requests at once, so the adapter connection metrics count
  fewer new connections than Go's, and the probe's connection is in none of them. Response header
  fields may take up to 10 MiB, as Go's Transport allows.

## Getting started

You need git, bash 4.4 or newer and a Rust toolchain installed with rustup (`rust-toolchain.toml`
pins the version, and rustup installs it on first use). The scripts in `scripts/` and
`.github/port/` need that bash: on macOS, `/bin/bash` is 3.2 and they stop at once, so put a newer
bash first on `PATH`. Regenerating or checking goldens, the Go tooling checks and the port tooling
tests also need Go.

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

## Checks

`.github/workflows/ci.yml` runs every check on every pull request, on every push to `main` and
`parity-harness`, in merge queues and by hand, all at full strictness. There are no toggles, and no
job may continue on error. `ci-ok` passes only when every other job passed.

| CI job | What it checks | Locally |
| --- | --- | --- |
| `changes` | the compared range, the adapter directories the change touches, whether it is the port pull request or on `parity-harness` | (CI only) |
| `guard` | the repository invariants: no upstream file, no committed agent configuration, the pin only moves forward, no AI attribution in any commit or pull request text, and the bot rules on the port pull request | `.github/port/scripts/guard.sh --ref HEAD --base origin/main --upstream <mirror>` |
| `rust-fmt` | rustfmt (`rustfmt.toml` uses unstable options, hence the nightly) | `cargo +nightly-2026-11-13 fmt --all -- --check` |
| `rust-clippy` | clippy with warnings denied, with the default features and with `--all-features` | `cargo clippy --workspace --all-targets --locked [--all-features] -- -D warnings` |
| `rust-doc` | rustdoc with warnings denied, both ways | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked [--all-features]` |
| `rust-test` | the tests against the pinned upstream checkout, both ways | `cargo test --workspace --locked [--all-features]` |
| `conformance` | the ratchet of Go adapter fixtures replayed against Rust. No adapter crate exists yet, so it replays nothing and passes with a warning. | `.github/port/scripts/conformance.sh` |
| `devtools` | the generated files match Go | `cargo run -p pbs-devtools --locked -- all --check` |
| `go-tooling` | the Go tooling modules: `gofmt -s`, `go build` and `go test` with `-mod=readonly` | `.github/port/scripts/go-tooling.sh` |
| `lint-policy` | nothing suppresses a lint, and the scanner's self-tests | `scripts/check-no-lint-suppression.sh` and `scripts/tests/run.sh` |
| `lint-config` | TOML formatting, shellcheck at its strictest on every script, and actionlint on the workflows | `taplo fmt --check --config taplo.toml`, `scripts/lint-shell.sh`, `actionlint` |
| `port-tests` | the port and CI tooling, tested in temporary repositories | `PORT_TESTS_UPSTREAM_GIT=<mirror> .github/port/tests/run-all.sh` |
| `deny` | advisories, bans, licenses and sources of the Rust dependencies | `cargo deny --locked check --deny warnings` |
| `machete` | unused dependencies | `scripts/cargo-machete.sh` |
| `parity` | `parity-harness` only: the parity harness (below) | `.github/port/scripts/ci-parity.sh` |

`<mirror>` is a bare mirror of upstream: `.github/port/scripts/upstream-mirror.sh DIR` makes one.

No lint is ever suppressed: there is no `#[allow]` or `#[expect]` anywhere. `PORTING.md` has the
whole policy. The Go code in this repository is test tooling: it must be `gofmt -s` clean, build
and pass its tests, and it is not linted.

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
- **The parity harness.** The `parity-harness` branch adds `parity/`: a Go differential harness
  that runs the instrumented Go server and the Rust server on the same requests and compares their
  responses, their outgoing bidder requests and their traces. The instrumentation is a patch
  (`parity/upstream-seams.patch`) that `parity/scripts/build-go.sh` applies to a copy of the
  pinned upstream checkout; this repository never holds the patched files. The harness's rules:
  its `/__parity/*` endpoints are disabled by default, need a token and serve loopback only, with
  no opt-out, and no request leaves loopback in a parity run. CI checks the Go side on every push
  to that branch; the run against Rust is skipped, with a notice, until the server binary exists.

[`PORTING.md`](PORTING.md) is the porting contract: parity rules, layout, testing against Go, the
lint policy and performance rules.

## The daily upstream port

`.github/workflows/upstream-port.yml` keeps the port in step with upstream. Every day it plans the
next chunk of upstream first-parent commits after `UPSTREAM_SYNC`, checks the old and the new pin
out next to the work tree with `scripts/fetch-upstream.sh`, and advances the pin and the Go
tooling modules with it: the upstream version they require, and the `go` line and dependency
versions of the new pin's `go.mod`. Agent sessions then port the behaviour changes and write
differential tests (a port session, an adversarial review against Go, and a fix session when
needed), checked by the same gates as CI. The bot commits the result with
`upstream-port-log.jsonl`, which records every upstream commit the pin advanced over and the Go
behaviour left unported, and a separate job with the only push credential opens or updates the
port pull request, which must pass CI like any other. A red port pull request is repaired a few
times, then handed to a person. `.github/workflows/parity-harness-sync.yml` merges `main` into
`parity-harness` every day and checks that the seams patch still applies to the new pin.

Upstream commits are never copied into this repository. The workflows need their secrets (agent
credentials, and a GitHub App or token to push); [`.github/README.md`](.github/README.md) has the
design, the setup and the runbook.

## License

Apache License 2.0. See [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE), which also credits the
third-party works the port derives from; their licence texts are in
[`LICENSES/`](LICENSES/README.md).

This project is a port of Prebid Server, which Prebid.org and its contributors develop at
[prebid/prebid-server](https://github.com/prebid/prebid-server) under the same license. It is an
independent project, not affiliated with Prebid.org.
