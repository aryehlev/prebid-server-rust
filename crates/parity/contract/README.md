# Vendored parity contract

Byte-for-byte copies of the JSON files under `parity/schema/` (and `parity/schema/examples/`) on
the `parity-harness` branch, which is the source of truth for the cross-language contract. They
were taken from commit `abad587d` (`schema_version` 1.0.0).

`pbs-parity`'s tests check the Rust registries against them (stage and decision IDs with their
order, domains and keys, outbound kinds, error codes, fake defaults, order sites), and the Go
golden program (`../testdata/go`) fails when a vendored file differs from the copy embedded in the
`parity/schema` package it is built against.

To refresh after the contract changes, from the repository root:

```sh
for f in $(git ls-tree --name-only parity-harness parity/schema/ parity/schema/examples/ | grep '\.json$'); do
  git show "parity-harness:$f" > "crates/parity/contract/${f#parity/schema/}"
done
```

then update the Rust registries until `cargo test -p pbs-parity` passes, and regenerate the
goldens with `crates/parity/testdata/go/run.sh` on the `parity-harness` branch, the only branch
that holds the parity packages the golden program is built against.
