# Porting contract

Every change to the Rust port follows these rules. They exist so that many people (and agents)
can port in parallel without diverging from Go or from each other.

## 1. Parity is the objective

- The upstream Go code at the commit `UPSTREAM_SYNC` pins (the Go root, see §2) is the source of
  truth. Port **behaviour**, not intent: when Go is awkward, inconsistent, or buggy, reproduce it
  exactly. Externally visible behaviour includes HTTP status codes, header names/values, JSON
  field presence and spelling, error and warning text, outgoing bidder HTTP requests (method, URI,
  headers, body), config keys/defaults/env bindings, and metric names/labels.
- Read the Go code itself. Do not trust names, comments, or the old Rust code that preceded this
  port (`rust/legacy/` on the original fork's branches) — it was an approximation and is only a
  reading aid.
- Cite the Go origin of non-obvious behaviour in a comment, e.g. `// Go: exchange/utils.go:412`.
- Do not add behaviour Go does not have (no extra fields, routes, config keys, or "improvements").
  Do not port Go code that is dead in production (only reachable from Go tests).

## 2. Layout and ownership

- This repository holds only the Rust port: the Cargo workspace at the root, its crates under
  `crates/`, the shared goldens in `testdata/`, the conformance ratchet in `conformance/` and the
  scripts in `scripts/`. Upstream Go is never committed, and upstream commits are never copied or
  rewritten.
- `UPSTREAM_SYNC` pins the upstream commit the port covers. `scripts/fetch-upstream.sh` checks it
  out into `.upstream/`, which git ignores. That checkout is the *Go root*: tests and tools read
  Go sources, corpora and `static/` from `$PBS_GO_ROOT` when it is set, else from `.upstream/`,
  and never search for it (`pbs_devtools::repo`). Go paths in these rules are relative to it.
- The orchestrator owns `Cargo.toml`, `Cargo.lock` (merges regenerate it), `rust-toolchain.toml`,
  `rustfmt.toml`, this file, and every crate's `lib.rs` module list unless a work unit is told it
  owns the whole crate.
- A work unit edits **only the files it owns**. If it needs a change in a shared file (a new
  workspace member, a new pub item in another crate, a fix in another unit's code), it must not make
  it; it reports the exact change in its result and works around it locally if it can.
- A unit that owns a crate may add dependencies to that crate's `Cargo.toml` with explicit
  versions. Prefer the standard library; add a dependency only when it is mature and reproducing it
  would be unreasonable.
- Mirror the Go package structure: one Rust module per Go file where practical, named after it.
- `patches/ring` replaces ring through `[patch.crates-io]` in the workspace `Cargo.toml`. Nothing
  builds ring (rustls uses aws-lc-rs), but rustls-webpki's weak `ring?/alloc` feature keeps it in
  `Cargo.lock`, where clippy's `multiple_crate_versions` would count its windows-sys 0.52 beside
  tokio's 0.61. The stand-in has no dependencies and its `lib.rs` is a `compile_error!`, so a crate
  that enables ring stops the build. Delete both once rustls 0.24 and rustls-webpki 0.104, which
  have no ring feature, are adopted; the `pbs_gohttp_client::tls` docs give the details.

## 3. Go-compatibility building blocks

Use these crates instead of re-implementing Go semantics locally:

| Crate | Purpose |
|---|---|
| `pbs-gojson` (+ `pbs-gojson-derive`) | Go JSON: the three encoder flavours (`jsonutil.Marshal`, `encoding/json.Marshal`, `Encoder` with HTML escaping off), Go float formatting, `RawJson` (exact bytes, absent vs `null`), and jsoniter/`encoding/json` decoding semantics with byte-identical error text. |
| `pbs-errortypes` | Go `errortypes` codes, severities, and error kinds. |
| `pbs-gocompat` | Go `net/http` header semantics, `net/url` escaping and `Values.Encode`, `strconv` formatting/parsing, `fmt` verbs, the `text/template` subset Prebid uses, `time` Format/Parse/JSON and `Duration`/`ParseDuration` (Local treated as UTC), `strings` case/split/`Replacer`, `encoding/base64`, `unicode` tables, and `math` conversions. |
| `pbs-goyaml` (+ `pbs-goyaml-derive`) | `gopkg.in/yaml.v3` decoding (as viper 1.12 uses it) and `mapstructure` v1.5.0 decoding into Rust structs, with Go's error text; JSON encoding of decoded values. |
| `pbs-goviper` | `spf13/viper` v1.12.0 and `spf13/cast` v1.5.0: defaults, env binding (`SetEnvPrefix`, `SetEnvKeyReplacer`, `AutomaticEnv`), config-file search and merge, typed getters, `Unmarshal`. Known gaps are listed in its crate docs: config formats Prebid does not use, `AllSettings` aliasing, and keys that prefix other keys (Go's output there is nondeterministic). |

Each of these crates documents its API in its `lib.rs` crate docs. Read them before porting.

- Never enable `serde_json`'s `preserve_order` or `arbitrary_precision` features anywhere in the
  workspace: `pbs-gojson` relies on `serde_json::Map` being sorted (Go's map-key order) and on
  `Value` numbers behaving as Go `float64`/ints, and Cargo feature unification would silently change
  encoded bytes.
- A few Tier 0 crates still keep private copies of Go helpers that now live in the shared crates
  (`pbs-metrics` float formatting, `pbs-tcf2` `encoding/json` scanner and base64, `pbs-currency`
  rates JSON). Do not copy those; use the shared crates.

## 4. Testing against Go

- Port the Go unit tests for the code you port, and replay Go's JSON corpora (adapter fixtures under
  `adapters/*/*test/`, `exchange/exchangetest/`, `endpoints/openrtb2/sample-requests/`, and others)
  through Rust harnesses with the same comparison semantics the Go harness uses. Read the corpora
  from the Go root; never copy them into this repository.
- Differential tests are encouraged: a small Go program under `crates/<crate>/testdata/go/` may
  generate golden outputs from the real Go code, which Rust tests then compare against. That
  directory is a Go module of its own: it requires `github.com/prebid/prebid-server/v4` at the
  pinned commit's pseudo-version, replaced by `.upstream`, with upstream's `go` version and
  dependency versions and a tidy `go.sum`, and it runs from its own directory. Go code you author
  is test tooling: it must be `gofmt -s` clean, build, and pass its tests. It is not linted (no
  `go vet` step, no golangci-lint).
- A unit is done only when its tests pass and it can state which Go corpus/test cases it reproduces.

## 5. Lint and format policy

- Every crate opts into the workspace lint policy with `[lints] workspace = true`.
- **Never suppress a lint**: no `#[allow(..)]`, `#![allow(..)]`, `#[expect(..)]`,
  `cfg_attr(.., allow(..))`, or `"allow"` levels in any `[lints]` table. Fix the code instead.
  This covers every spelling: raw identifiers (`#[r#allow(..)]`, `rustfmt::r#skip`), attributes a
  macro assembles from its input (`#[$x]`, `quote!`'s `#[#x]`), proc-macro strings that spell a
  suppressing attribute, `#[clippy::msrv]`, `#[automatically_derived]`, and compiler overrides in
  `.cargo/config*` or the environment (`rustflags`, `rustc-wrapper`, `rustc-workspace-wrapper`,
  `runner`, `RUSTC*`, `RUSTFLAGS`). Go code we author may not use `nolint`/`nosec`/build tags.
  `scripts/check-no-lint-suppression.sh` enforces all of this in CI; its header lists the rules.
- No `unsafe`. No `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`/indexing that can panic —
  including in tests (tests return `Result` and use `?`).
- Before finishing, run `cargo clippy -p <crate> --all-targets` and fix every warning in files you
  own. Format only the files you own: `rustfmt +nightly-2026-11-13 --edition 2024 <files>`.
  Never run `cargo fmt` on the whole workspace from a work unit (it rewrites files you do not own).

## 6. Commits

- Work on the branch you were given (`wu/<unit-id>`), commit with the configured git user, and write
  messages that explain what changed and why.
- Do **not** add `Co-Authored-By` trailers, "Generated with" lines, or links to AI sessions.
- Never push to any remote.

## 7. Performance

Speed never buys a difference from Go. Bytes, ordering, error text and side effects stay as §1
requires. Optimise only what a benchmark or profile shows, and report the numbers with the change.

### Data layout

- A Go pointer to a struct is `Option<Arc<T>>`. Typegen emits it, and hand-written types follow.
  Other pointers stay `Option<T>`, slices stay `Option<Vec<T>>` (`BidRequest.imp` included) and
  maps stay `Option<BTreeMap<K, V>>`. `RawJson` already holds shared bytes: clone it, and never
  re-encode it to copy it.
- No `Arc<Vec<T>>` or `Arc<String>` (`clippy::rc_buffer`). An `Arc<[T]>` or `Arc<str>` field needs
  a benchmark and its own `pbs-gojson` impls first.
- Write through a pointer field only with `Arc::make_mut` (or `pbs_util::ptrutil::make_mut` /
  `make_mut_or_default`), where Go writes and after Go's condition. `make_mut` on a shared value
  copies the whole struct, so never call it to read, speculatively, or to store a value already there.
  Clone with `Arc::clone(&x)` (`clippy::clone_on_ref_ptr`) and move out with `Arc::unwrap_or_clone`.
- Port a Go copy (`c := *p`, `ptrutil.Clone`, `ortb.Clone*`) as an `Arc` clone at the same point.
  Port the writes Go then makes to the copy as `make_mut` writes. The values then match Go's.
- Decode into the existing value wherever Go does (`jsonutil.Unmarshal(data, &x)`). Merge with
  `jsonutil::merge_clone`, which copies only the paths it decodes, as Go's `MergeClone` does.

### Ownership

- The endpoint owns the `RequestWrapper` and lends it as `&mut` through `HoldAuction`. Nothing holds
  it in an `Arc` while it can change. A bidder's request is an owned `BidRequest` whose pointer
  fields are `Arc` clones of the auction request's.
- Adapters, hooks and module mutations get `&mut` to the value Go passes by pointer. The caller keeps
  using that same value: `MakeBids` reads what `MakeRequests` left, and the exchange reads `cur`, the
  imps and the headers after the adapter.
- A response's bids live in an arena with typed indices. Go's pointer-keyed maps and pointer
  comparisons become index maps and index comparisons.
- Analytics, debug and hook objects read their owner where Go reads it, not a copy taken earlier.
- Goroutines become Tokio tasks that own their inputs (`Arc` snapshots, owned requests). Never start
  an OS thread per request. Blocking I/O moves to `spawn_blocking` without changing when it
  completes relative to the response.

### Aliasing

- Copy-on-write gives value semantics. Go has reference semantics wherever two holders share a
  pointee, a slice's backing array or a map. Some deterministic Go behaviour depends on that sharing:
  a write one holder makes that another reads, or a pointer comparison. Port each such behaviour as a
  named seam:
  - an item named after the Go behaviour;
  - a `// Go:` citation;
  - a test with Go's output;
  - an entry under `# Go aliasing` in the crate docs.

  Never count on `Arc` sharing to reproduce such behaviour implicitly.
- `Arc::ptr_eq` stands for Go pointer identity only for objects whose Go copy points Rust mirrors one
  for one: `make_mut` where Go writes a copy, the paths `merge_clone` decodes, and `Arc::new` of a
  copy where Go copies without writing. Identity does not survive an `ortb.Clone*` port, because Go
  copies deeply where Rust shares. Slices and maps have no identity at all, so use an explicit
  marker.
- A Go struct value copy shares its slices and maps with the original. An element write through
  either one (`c.Imp[i].TagID = ..`) is seen through both, so port it so that every reader Go has
  sees it.
- Go also writes through objects that bidder requests share with the auction request. These writes
  go through the exchange's shared-write ledger, which replays them where Go's readers see them:
  - on the auction request once the bidders have run;
  - in parity mode, also on the bidders after the writer in `bidder_request_phase` order.
- Never emulate Go nondeterminism (map iteration, completion order, data races). Take one order Go
  can take: the one `pbs-parity`'s `contract/fakes.json` `orders` names, or else ascending keys. Cite
  it at the site. In parity mode, impose exactly the orders Go's parity mode imposes.

### Shared state

- State built once at startup is an `Arc` (or `&'static`). State that a background task refreshes is
  read with one atomic load of an `Arc`: `arc_swap::ArcSwap`, or a `RwLock<Arc<T>>` cloned and
  released at once. Never hold a guard across `.await`, and never copy the value per request.
- State that Go writes process-wide at runtime (package variables, config fields set per request)
  stays process-wide and shared, so later requests see it as Go's do.
- Counters and flags are atomics. Resolve metric label sets once rather than formatting them per
  request, and keep the exposition byte-identical.

### Hot path

- Encode with `pbs-gojson` straight into the output buffer. Never `format!` a number or field, never
  build an intermediate `String`, and never decode and re-encode what Go passes through as bytes.
- Keep Go's laziness: parse where Go parses (the wrapper's `Get*Ext`), because parsing earlier moves
  the errors.
- A cache must not change what a request observes. Cache only pure functions of immutable inputs,
  or exactly what Go caches, with Go's key, lifetime and invalidation.

### Benchmarks

- Benchmarks live in the crate's `benches/` and run over the Go corpora and the fixtures in
  `benches/data/`. Use criterion with `harness = false`, and write `main` by hand if criterion's
  macros trip a lint.
- Count allocations with `dhat`; its allocator is a safe `#[global_allocator]` static. Never write
  an allocator, because `unsafe` is forbidden.
- Benchmark dependencies follow §2 and must not add a second version of any crate
  (`clippy::multiple_crate_versions`).
- A claim against Go needs a paired Go benchmark under the crate's `testdata/go/`. Report both,
  before and after.
- §5 applies to benchmark code: no suppression, no `unwrap`/`expect`, no panicking indexing.
