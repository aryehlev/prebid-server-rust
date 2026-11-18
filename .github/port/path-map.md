# Go package to Rust crate map

Where the Rust counterpart of each part of the Go server lives. Crate directories are under
`crates/<dir>/`, and the package is named `pbs-<dir>`. Each crate's `lib.rs` documentation lists
the Go files its modules mirror; within a crate, modules follow the Go files (PORTING.md
section 2). Go paths below are relative to the Go root (`.upstream/`, the pinned upstream
checkout).

## Crates in the workspace

| Go source | Rust crate | Directory |
|---|---|---|
| `github.com/prebid/openrtb/v20` (`openrtb2`, `openrtb3`, `adcom1`, `native1`) | `pbs-openrtb` | `openrtb` |
| `openrtb_ext/` (extension types, bidder names, `RequestWrapper`) | `pbs-openrtb-ext` | `openrtb-ext` |
| `errortypes/` | `pbs-errortypes` | `errortypes` |
| `util/` packages (except `util/jsonutil`) | `pbs-util` | `util` |
| `util/jsonutil`, `encoding/json`, json-iterator | `pbs-gojson`, `pbs-gojson-derive` | `gojson`, `gojson-derive` |
| `gopkg.in/yaml.v3`, `mapstructure` decoding | `pbs-goyaml`, `pbs-goyaml-derive` | `goyaml`, `goyaml-derive` |
| `spf13/viper` v1.12 semantics behind `config/` | `pbs-goviper` | `goviper` |
| Go standard library behaviour (`net/http` headers, `net/url`, `strconv`, `fmt`, `text/template`, `time`, `strings`, `encoding/base64`, `unicode`, `math`) | `pbs-gocompat` | `gocompat` |
| Go `net/http` client and transport wire behaviour | `pbs-gohttp-client` | `gohttp-client` |
| Go `net/http` server, `julienschmidt/httprouter` and `rs/cors` behaviour | `pbs-gohttp-server` | `gohttp-server` |
| Go `compress/flate` and `compress/gzip`, byte for byte | `pbs-gogzip` | `gogzip` |
| `github.com/coocood/freecache` (stored requests and price floors caches) | `pbs-freecache` | `freecache` |
| `logger/` | `pbs-logger` | `logger` |
| `metrics/` | `pbs-metrics` | `metrics` |
| `currency/` | `pbs-currency` | `currency` |
| `github.com/prebid/go-gdpr` (TCF v2 consent strings, global vendor lists) | `pbs-tcf2` | `tcf2` |
| `github.com/prebid/go-gpp` | `pbs-gpp` | `gpp` |
| `static/bidder-params/` validation (`openrtb_ext.NewBidderParamsValidator`, gojsonschema) | `pbs-bidder-params` | `bidder-params` |
| The parity harness seams the Rust server exposes (stage traces, decisions, fakes; feature `trace`) | `pbs-parity` | `parity` |
| Generated ground truth from Go: bidder names and infos, configuration defaults, struct metadata, the adapter fixture manifest `testdata/go_fixture_manifest.json` (`cargo run -p pbs-devtools -- all`) | `pbs-devtools` | `devtools` |

## Planned crates (not in the workspace yet)

A port that needs one of these reports the Go change as not ported (status `partial` or
`blocked`); adding a workspace crate is a human decision, because the root `Cargo.toml` is
protected.

| Go source | Planned crate |
|---|---|
| `config/` (`config.Configuration`, account defaults, bidder-info loading) | `pbs-config` |
| `adapters/` (one module per bidder) and the `adapters/adapterstest` fixture harness (`go_fixtures` test, conformance/README.md) | `pbs-adapters` |
| `exchange/` | `pbs-exchange` |
| `endpoints/` | `pbs-endpoints` |
| `main.go`, `router/`, `server/` | `pbs-server` |
| `privacy/` | `pbs-privacy` |
| `gdpr/` (enforcement on top of `pbs-tcf2`) | `pbs-gdpr` |
| `usersync/` | `pbs-usersync` |
| `stored_requests/`, `stored_responses/` | `pbs-stored` |
| `hooks/` | `pbs-hooks` |
| `modules/` | `pbs-modules` |
| `analytics/` | `pbs-analytics` |
| `floors/` | `pbs-floors` |
| `prebid_cache_client/` | `pbs-cache-client` |

## Go code in this repository

The repository holds no upstream Go code. Its only Go code is test tooling: the Go tooling
modules under `crates/<crate>/testdata/go/` and `crates/devtools/go/`, which run the real Go code
of the Go root to write goldens. Each one requires the upstream module at the pin's
pseudo-version and replaces it with the `.upstream/` checkout.

## Data read at run time

`static/bidder-info/*.yaml`, `static/bidder-params/*.json` and `static/category-mapping/` are read
from `static/` of the Go root (`PBS_GO_ROOT`), never copied into the repository. The adapter JSON
fixtures under `adapters/<dir>/<dir>test/` are read in place by the conformance harness.
