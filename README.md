# Prebid Server — Rust port

A Rust implementation of [Prebid Server](https://github.com/prebid/prebid-server) that is meant to
replace the Go server for production traffic **without changing externally visible behaviour**:
same HTTP status codes, response bodies, outgoing bidder requests, configuration keys and defaults,
and metric names.

The Go code at the repository root is the reference implementation and the source of truth. It
tracks upstream `prebid/prebid-server`; the Rust workspace lives in this `rust/` directory.

- `crates/` — the Rust workspace members.
- `legacy/` — an earlier approximate port kept only as reading material while crates are rewritten
  against the Go sources. It is not compiled and is deleted once every replacement has landed.
- `PORTING.md` — the porting contract: parity rules, crate layout, testing against Go's own JSON
  corpora, lint policy, and commit rules.

## Toolchain

`rust-toolchain.toml` pins the compiler. Formatting uses a pinned nightly rustfmt so the stricter
unstable options in `rustfmt.toml` apply:

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo +nightly-2026-11-13 fmt --all -- --check
```
