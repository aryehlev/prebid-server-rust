# Porting contract

Every change to the Rust port follows these rules. They exist so that many people (and agents)
can port in parallel without diverging from Go or from each other.

## 1. Parity is the objective

- The Go code at the repository root is the source of truth. Port **behaviour**, not intent: when
  Go is awkward, inconsistent, or buggy, reproduce it exactly. Externally visible behaviour includes
  HTTP status codes, header names/values, JSON field presence and spelling, error and warning
  text, outgoing bidder HTTP requests (method, URI, headers, body), config keys/defaults/env
  bindings, and metric names/labels.
- Read the Go code itself. Do not trust names, comments, or the old Rust code in `legacy/` — it
  was an approximation and is only a reading aid.
- Cite the Go origin of non-obvious behaviour in a comment, e.g. `// Go: exchange/utils.go:412`.
- Do not add behaviour Go does not have (no extra fields, routes, config keys, or "improvements").
  Do not port Go code that is dead in production (only reachable from Go tests).

## 2. Layout and ownership

- The orchestrator owns `rust/Cargo.toml`, `rust/Cargo.lock` (merges regenerate it),
  `rust-toolchain.toml`, `rustfmt.toml`, this file, and every crate's `lib.rs` module list unless a
  work unit is told it owns the whole crate.
- A work unit edits **only the files it owns**. If it needs a change in a shared file (a new
  workspace member, a new pub item in another crate, a fix in another unit's code), it must not make
  it; it reports the exact change in its result and works around it locally if it can.
- A unit that owns a crate may add dependencies to that crate's `Cargo.toml` with explicit
  versions. Prefer the standard library; add a dependency only when it is mature and reproducing it
  would be unreasonable.
- Mirror the Go package structure: one Rust module per Go file where practical, named after it.

## 3. Go-compatibility building blocks

Use these crates instead of re-implementing Go semantics locally:

| Crate | Purpose |
|---|---|
| `pbs-gojson` (+ `pbs-gojson-derive`) | Go JSON: the three encoder flavours (`jsonutil.Marshal`, `encoding/json.Marshal`, `Encoder` with HTML escaping off), Go float formatting, `RawJson` (exact bytes, absent vs `null`), and jsoniter/`encoding/json` decoding semantics with byte-identical error text. |
| `pbs-errortypes` | Go `errortypes` codes, severities, and error kinds. |
| `pbs-gocompat` | Go `net/http` header semantics, `net/url` escaping and `Values.Encode`, `strconv` formatting/parsing, `fmt` verbs, the `text/template` subset Prebid uses, `time` Format/Parse/JSON and `Duration`/`ParseDuration` (Local treated as UTC), `strings` case/split/`Replacer`, `encoding/base64`, `unicode` tables, and `math` conversions. |

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
  through Rust harnesses with the same comparison semantics the Go harness uses.
- Differential tests are encouraged: a small Go program under `<crate>/testdata/go/` may generate
  golden outputs from the real Go code, which Rust tests then compare against. Go code you author
  must be `gofmt`-clean and pass `go vet`.
- A unit is done only when its tests pass and it can state which Go corpus/test cases it reproduces.

## 5. Lint and format policy

- Every crate opts into the workspace lint policy with `[lints] workspace = true`.
- **Never suppress a lint**: no `#[allow(..)]`, `#![allow(..)]`, `#[expect(..)]`,
  `cfg_attr(.., allow(..))`, or `"allow"` levels in any `[lints]` table. Fix the code instead.
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
