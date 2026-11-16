# Go-vs-Rust live differential parity harness

This directory holds the parity harness between the Go Prebid Server (the
source of truth) and its Rust port under `rust/`. Both servers run side by side
in parity mode. A runner sends the same case to both and compares everything
they observed: the HTTP response, every internal stage, every outbound call,
every random decision, the analytics calls and the Prometheus deltas. Any
difference that the normalization allowlist does not explicitly cover is a
mismatch.

This README is the contract the Rust side implements. The JSON Schemas under
[`schema/`](schema/) are normative; the Go types in package `parity` mirror
them, and `go test ./parity/schema/...` keeps the two in sync.

## Contents

| Path | What it is |
|---|---|
| `contracts.go`, `ids.go`, `envelope.go`, `trace.go`, `body.go`, `adapterio.go`, `errorcodes.go` | Go mirror of the contract (package `parity`, standard library only, safe to import from product code). |
| `schema/*.schema.json` | JSON Schemas (draft-07), `$id` prefix `https://prebid.org/pbs-parity/schema/v1/`. |
| `schema/{stages,decisions,normalization,fakes,error_codes}.json` | Registries: stage IDs, decision points, normalization allowlist, deterministic fakes, error codes. |
| `schema/examples/` | Hand-written example of every document; all of them validate. |
| `schema/` (Go) | Embeds the files above; `schema.Validate(doc)` validates any parity document. Harness-only. |
| `trace/` | Go trace session: the process-wide case lock, nil-safe recorders, decision points, error records, metric deltas. |
| `transport/` | Parity `http.RoundTripper`: oracle-proxy and inline modes, `X-Parity-Meta`. |
| `fakes/` | Deterministic clock, random and UUID sources for the 8 nondeterministic adapters. |
| `wiring/` | Bootstrap glue used by `main.go` and `router/router.go`; everything is a no-op unless `parity.enabled`. |
| `diff/` | The differ: normalization by the allowlist, semantic, lexical and wire comparison, attribution and clustering. Harness-only. |
| `testdata/diff/` | Golden differ cases: a Go trace, a Rust trace and the exact mismatches expected. |

## Versioning

* Every document carries `"schema": "<name>/<major>"` and
  `"schema_version": "<major>.<minor>.<patch>"`. The current version is
  **1.0.0** (`parity.SchemaVersion`).
* A minor bump only adds optional members or enum values that old readers can
  ignore. A patch bump changes only documentation.
* A major bump changes every `schema` name (`pbs-parity-trace/2` and so on).
  Readers reject documents with a major version they do not know.
* A contract change updates the schema file, the Go mirror, the registries, the
  examples and this README in one commit, and bumps `SchemaVersion`.

## Documents

| `schema` member | Schema file | Produced by | Endpoint |
|---|---|---|---|
| `pbs-parity-envelope/1` | `envelope.schema.json` | runner | body of `POST /__parity/exec` |
| `pbs-parity-trace/1` | `trace.schema.json` | server | response of `POST /__parity/exec` |
| `pbs-parity-adapter-requests-input/1` / `-output/1` | `adapter_io.schema.json` | runner / server | `POST /__parity/adapter/{bidder}/requests` (MakeRequests) |
| `pbs-parity-adapter-bids-input/1` / `-output/1` | `adapter_io.schema.json` | runner / server | `POST /__parity/adapter/{bidder}/bids` (MakeBids) |
| `pbs-parity/1` | `introspection.schema.json#/definitions/Health` | server | `GET /__parity/health` |
| `pbs-parity-config/1` | `introspection.schema.json#/definitions/ConfigDump` | server | `GET /__parity/config` |
| `pbs-parity-bidders/1` | `introspection.schema.json#/definitions/BiddersDump` | server | `GET /__parity/bidders` |
| `pbs-parity-reset/1` | `introspection.schema.json#/definitions/Reset` | server | `POST /__parity/reset` |
| `{"error": ...}` | `introspection.schema.json#/definitions/Error` | server | any `/__parity/*` failure |
| `pbs-parity-diff/1` | `mismatch.schema.json` | runner | `parity/out/<run>/cases/<n>/diff.json` |
| `pbs-parity-index/1` | `index.schema.json` | runner | `parity/out/<run>/index.json` |
| registries (`pbs-parity-stages/1` and others) | `registries.schema.json` | checked in | `parity/schema/*.json` |

Shared definitions (headers, bodies, error records, outbound calls,
`X-Parity-Meta`) live in `common.schema.json`.

### Shared value rules

* **Headers** are `HeaderList`: an ordered list of `[name, value]` pairs. Names
  keep their case and repeated names keep their relative order. Where the
  producer only has a map (`http.Header` in Go), it emits names sorted
  canonically and says so (`wire: false` on outbound calls).
* **Bodies** (`Body`): `b64` is always the exact wire bytes. `text` is the exact
  wire bytes when `content_encoding` is empty. `json` is the semantic JSON of the
  decoded content. Recorders emit `text` for valid UTF-8 identity bodies and
  `b64` otherwise (always `b64` for content-encoded bodies), plus `json` when the
  decoded content parses. Only on input documents may `text` or `json` be
  combined with `content_encoding`; the receiver then encodes the content itself.
  `null` means no body.
* **Errors** are `ErrorRecord {code, kind, message, scope, severity, fatal}`.
  `code` uses Go's `errortypes` numbers (`error_codes.json`). An error without a
  code is `999 UnknownError` and fatal, exactly as `errortypes.FatalOnly`
  treats it. Severity follows the Go type, not the code range: for example
  `BidderTemporarilyDisabled` (6) is a warning.
* **Empty and absent are different.** Trace members are always present: use
  empty arrays and maps, never omit them. In a stage entry, `errors: null` means
  the stage records no error list, and `errors: []` means it recorded an empty
  one.

## Exec flow

1. The runner posts an envelope to `POST /__parity/exec` on both servers.
2. The server acquires the process-wide exec lock, so only one case runs per
   process. It then publishes the trace session, sets the deterministic fakes
   from `envelope.fakes`, and snapshots Prometheus.
3. The server rebuilds the inbound request from `envelope.request`: method,
   path, raw query, headers in order, `host`, `remote_addr`, and the body from
   its wire bytes. It dispatches the request through its **full outer handler
   chain**, exactly what the main listener serves. In Go that chain is
   compression, then `NoCache`, then CORS, then the router.
4. Product code records stages through one-line hooks (see below). Outbound
   calls go through the parity transport.
5. The server returns the trace:
   * `http_in` and `http_response` hold the exact status, headers and body bytes.
   * `stage_status` covers every registered stage.
   * `metrics_delta` lists the changed series.
   * `analytics`, `decisions` and `notes` complete the document.
6. In oracle mode the runner merges the oracle's wire captures into `http_out`
   with `wire: true`.

`capture.stages` limits recording to the listed stages; `"*"` or no value means
all stages. `capture.metrics` and `capture.analytics` default to `true`.

## Stages

`schema/stages.json` is the ordered registry: 45 stages, from `config.resolved`
to `http.response`. Each stage has the following fields:

* `order`: the lowest-ordered stage with a mismatch is the case's *first
  diverging stage*. Mismatches there are `primary`; later ones are marked
  `consequential_of` that primary signature.
* `domain`: exactly one of the nine locked matrix domains. Each domain has at
  least one stage, and `domains[].requirements` assigns every requirement ID to
  exactly one domain.
* `requirements`: the requirement IDs the stage evidences.
* `keyed_by`: how stage entries are keyed. `_` means unkeyed; `bidder` and
  `seat` key by name; `bidder#call` keys by bidder and zero-based request index
  in `MakeRequests` order.
* `trace_field`: where the stage lives in the trace. It is `stages` unless the
  stage is a top-level member (`http_in`, `http_out`, `analytics`,
  `metrics_delta`, `http_response`) or is compared through introspection
  (`config.resolved`, `bidders.resolved`).
* `go_capture`: the Go site of the capture point. This is the reference the Rust
  side copies.

A stage entry is `{seq, writes, json, raw, raw_b64?, errors, warnings, notes?}`:

* `json` is the semantic value.
* `raw` is the exact bytes the implementation marshaled, so lexical differences
  survive. Go uses `jsonutil.Marshal`, with HTML escaping on. `raw_b64` replaces
  `raw` when the bytes are not UTF-8.
* `writes` counts how many times the key was recorded; the last write wins.

`stage_status` holds a value for every registered stage:

* `captured`;
* `not_reached`, when the code path never ran;
* `unimplemented`, which only Rust may send, when it has no equivalent capture
  point yet. The differ reports it as `stage_missing`.

**Observer-effect rule.** Hooks only snapshot values the code has already built.
Never call `RequestWrapper.RebuildRequest`, never marshal a live wrapper to
snapshot it, and never mutate state for tracing. A disabled harness costs one
atomic load per hook:

```go
if s := trace.Current(); s != nil { s.Record(parity.StageBidderRequest, bidder, req) }
```

**Exchange stage payloads.** The `json` member of stages 13 to 36
(`hooks.processed_auction` to `exchange.response`) has the shape of the types in
`parity/stagedata`, which are the shared contract for those stages. Their field
comments give each member's meaning, null and empty rules, and the sort key of
every list Go builds from map iteration. Every entry carries `errors` and
`warnings` lists, `[]` when there are none. A `bidder#call` key counts the
`MakeRequests` output from 0 in order, then the bidder's stored bid responses in
ascending imp ID order. Three stages leave out part of what `stages.json`
lists, because capturing it would break the observer-effect rule:

* `hooks.processed_auction` records only `{rejected}`. The request after that
  hook stage shows up in `floors.enrich` and `bidder.request`, both of which
  snapshot a request the code rebuilt itself.
* `privacy.bidder` takes its activity results from re-running the pure
  `ActivityControl.Allow` on the same inputs.
* `floors.enforce.enforced` is the `floors.enforce` decision value: false when
  the decision was not reached.

## Decision points

`schema/decisions.json` lists every place where Go draws a random number on the
request path:

* `floors.skip`, `floors.data_rate`, `floors.modelgroup` and `floors.enforce`;
* `category.dedupe`;
* `pricegranularity.timesplit`;
* `usersync.shuffle`.

At these exact sites both implementations read `envelope.decisions[id]`:

* `binary` decisions take `0` or `1`, with the meanings documented in the
  registry.
* `index` decisions take a zero-based index.
* `usersync.shuffle` takes the enum value `identity`.

When the envelope has no value, the implementation draws from its own random
source and records `source: "rng"`. An out-of-domain value is ignored and the
decision carries a note. A decision is recorded only when the site is reached.
For example, `floors.skip` is not consulted when every skip rate is 0. In Go,
the helpers are `Session.Decide`, `Session.DecideBool`, `Session.Intn` and
`Session.DecideString` (`trace/decide.go`). With no case in flight, each helper
runs the original random draw unchanged.

`randomizeList`, which sets the bidder fan-out order, is deliberately not a
decision point: any order is Go-compatible, so `seatbid` order is normalized
instead. Bidder throttling is not a decision point either, because parity
profiles disable it. If a case does reach the throttle draw, Go adds a trace
note.

## Deterministic fakes

Eight adapters read the wall clock or a random source internally: adnuntius,
appnexus, cadent_aperture_mx, eplanning, flipp, huaweiads, tappx and yieldlab.
They are **not** normalized. In parity mode the Go server installs deterministic
sources (`fakes.Install`, called from `wiring.Bootstrap`) and the adapters pick
them up through small branch-only seams. The seams are listed in
`schema/fakes.json`. The defaults are:

| Value | Default | Matches |
|---|---|---|
| `now_unix_ms` | `1629473330823` (2021-08-20T15:28:50.823Z, ISO week 33, location UTC) | huaweiads fixture nonce, yieldlab week |
| `int63` | `10` | appnexus `adpod_id` fixtures |
| `uuid` | `30470a14-2949-4110-abce-b62d57304ad5` | flipp fixtures |

`envelope.fakes`, and `fakes` on adapter inputs, override them for one case.
Outside parity mode `fakes.Now()` is `time.Now()`, `fakes.Int()` is
`rand.Int()`, and the Builders keep their production sources. The runner
starts both servers with `TZ=UTC`, because huaweiads formats local time.

## Normalization and what is never normalized

`schema/normalization.json` is the complete allowlist. Every rule cites the Go
source of the nondeterminism it removes:

* seatbid, warning, httpcalls and seatnonbid order;
* `http_out` order with `seq` dropped;
* the `responsetimemillis` key set;
* the shape of the auction timestamp and of event `ts`;
* UUID placeholders;
* hook execution times and timing histograms, compared by count only;
* the `Date` header;
* semantic JSON member order;
* the stage entry `seq`.

Everything else is a mismatch. The following are **never normalized**, and they
gate parity like any other mismatch:

* **Wire differences** (`kind: wire`). Classes: `header_case`, `header_order`,
  `default_header` (for example `User-Agent: Go-http-client/1.1` or
  `Accept-Encoding: gzip` added by Go's transport), `http_version`,
  `content_length`, `transfer_encoding` and `other`. The semantic header
  comparison is case-insensitive. The wire comparison then reports case and
  order separately.
* **Lexical differences** (`kind: lexical`) between semantically equal JSON.
  Classes: `html_escape`, `unicode_escape`, `number_format` (for example `1`
  vs `1.0`), `trailing_newline`, `whitespace`, `member_order` and `other`.
* **The 8 adapters' outputs.** They are compared exactly, thanks to the fakes.

A new normalization rule must cite a Go nondeterminism source, must reduce a
Go-vs-Go (A/A) self-test diff to zero, and must never hide a Go-vs-Rust
difference.

## Mismatch records

`mismatch.schema.json` defines one record per difference. A record carries the
case, the stage and key, the domain and requirement, the `kind`, the `class`
(required for `lexical` and `wire`), the `json_path`, and the `go` and `rust`
values. It also carries `primary` or `consequential_of`, and a `signature`. The
signature is `sha1(stage|domain|json_path with indices replaced by [*]|kind)` in
lowercase hex, and is used for clustering.

Kinds: `value`, `type`, `missing_in_rust`, `extra_in_rust`, `order`, `lexical`,
`status`, `header`, `wire`, `stage_missing`, `stage_extra`, `error_code`,
`error_text`, `metric`, `timing`, `decision` and `unexpected_outbound`. Every
kind gates parity. CI ratchets the total against `parity/baseline.json`, which
may only go down; the goal is zero.

## Differ

`diff.New()` loads `stages.json` and `normalization.json`, and
`Engine.CompareTraces` turns a Go trace and a Rust trace into a
`pbs-parity-diff/1` document. `CompareAdapterRequests`, `CompareAdapterBids`,
`CompareConfig` and `CompareBidders` do the same for the other documents.

* **Allowlist only.** The differ implements each rule by ID. Loading fails when
  the file lists a rule the differ does not implement, or when a rule's action,
  keys, drop list, stages, shape or target disagree with the implementation. It
  also fails when a rule lacks a Go source or a justification. Removing a rule
  from the file re-enables the difference it hid.
* **UUID placeholders** apply only to v4 UUIDs that appear at a rule's
  positions and not in the case inputs (the envelope and `Case.KnownInputs`).
  Cache UUIDs are the exception and may be any version. They are numbered
  `<uuid#n>` by first appearance in a canonical traversal, so "one tid for
  every bidder" and "one tid per bidder" still differ.
* **Semantic comparison** matches members by name and numbers by exact value.
  Arrays are compared by position after LCS alignment. A pure permutation is
  one `order` record.
* **Lexical comparison** runs only on values that both sides recorded as exact
  bytes: stage `raw` or `raw_b64`, and body `text` or `b64`. Values recorded as
  `json` only are compared semantically, and the result carries a note.
* **Paths** are relative to the compared value. Parts that are not in the value
  use `@` members: `$['@errors'][0].message`, `$['@headers']['Content-Type']`,
  `$['@status']`, `$[0].request.headers['X-Prebid']`.
* **First divergence** is decided per causal plane. Lexical differences cannot
  cause semantic ones, because servers parse what they read. In the semantic
  plane and in the lexical plane, the records of the lowest-ordered diverging
  stage are primary. Later records in the same plane are `consequential_of` the
  first primary signature. Wire records, and `stage_missing` records for stages
  Rust reports as `unimplemented`, are always primary.
* **Attribution** takes the domain from the stage. It is refined by the
  outbound call kind for `http.out` (cache, floors, currency, vendor list,
  stored data), and then by JSON path: `ext.prebid.modules`, `prebid.cache`,
  `hb_cache_id*`, floors, eids, `regs`, consent and device identifiers.
  Config, metrics and analytics stages keep their domain.
* **Clusters** (`BuildClusters`) group records by signature across cases. They
  are ordered by primary count, then case count, then record count.
  `PrimarySignatures` is what a minimizer must keep reproducing.

## Outbound transport

Every outbound client is wrapped in parity mode:

| Client | Kind |
|---|---|
| general client (bidders, stored data, vendor list) | `bidder` when the request context carries a bidder (`trace.WithBidder`). Otherwise `general`, refined by URL prefix to `vendorlist` or `stored`. |
| cache client | `cache` |
| floors fetcher client | `floors` |
| currency client | `currency` |
| `http.DefaultTransport` (DefaultClient users) | `default` |

The general-client classifier rules are `wiring.GeneralClassifierRules`. The
vendor-list prefix comes first, then every configured stored-data HTTP and
HTTP-events endpoint. The Rust side must classify the same way.

* **Oracle mode** (the default). The transport clones the request, rewrites
  `https` to `http`, and adds `X-Parity-Meta`. It then sends the request
  through a clone of the client's own production transport, with
  `Proxy = parity.oracle_url`. The oracle therefore receives an absolute-form
  request carrying the real client defaults. `X-Parity-Meta` is the standard
  base64 (with padding) encoding of
  `{"v":1,"case","impl","kind","bidder","orig_url","seq"}`
  (`common.schema.json#/definitions/OutboundMeta`). The oracle strips it before
  recording. Calls made while no case is in flight, such as the startup GVL
  preload or the first currency fetch, carry an empty `case`.
* **Inline mode** (`mock.mode = inline`). The envelope answers calls in-process,
  and the server records them to `http_out` with `wire: false`. Resolution
  order is:
  1. `rules`, first match wins;
  2. `bidders[bidder]` or `default_bidder`, then `cache`, then `kinds[kind]`;
  3. the built-in defaults: bidder `204`, cache `503`, and anything else the
     transport error `parity: network disabled`.

  `delay_ms` honours the client deadline. `fail` takes one of the values
  `timeout`, `refused`, `reset` or `error`.
* **`mock.level = bidder_response`**. `MakeRequests` still runs and is
  recorded, but no HTTP request is sent. `bidder_responses[bidder]` replaces
  the `MakeBids` output, and everything after `MakeBids` still runs. The Go
  seam is `exchange.ParityMockBidderResponse`.

Without `parity.oracle_url`, oracle-mode calls fail with
`parity: network disabled`. Nothing in parity mode ever reaches the real
network.

## Configuration and startup guards

```yaml
parity:
  enabled: false            # PBS_PARITY_ENABLED
  token: ""                 # PBS_PARITY_TOKEN, >= 16 chars, logged as <REDACTED>
  oracle_url: ""            # PBS_PARITY_ORACLE_URL, absolute http URL
  allow_non_loopback: false # PBS_PARITY_ALLOW_NON_LOOPBACK
```

When parity is enabled, startup fails in these cases:

* the token is shorter than 16 characters;
* `host` is not a loopback address (an empty `host` means all interfaces and
  fails);
* `oracle_url` is not an absolute `http` URL on loopback.

`allow_non_loopback` lifts the loopback restrictions.

`/__parity/*` is served on the main port, ahead of `NoCache` and CORS. A caller
must be on loopback, or on the unix socket when that is the main listener;
otherwise the response is 403. The `X-Parity-Token` header must match, compared
in constant time; otherwise the response is 401. With parity disabled, every hook
is a no-op and `/__parity/*` is ordinary application traffic.

## Go hook lines (branch only)

| File | Hook |
|---|---|
| `config/config.go` | `Parity ParityConfig` field, `SetDefault("parity.*")`, `cfg.Parity.validate` |
| `config/structlog.go` | fields tagged `log:"redact"` are logged as `<REDACTED>` |
| `main.go` | `wiring.Bootstrap(cfg)`; currency client `wiring.WrapClient`; `wiring.Mount` around `NoCache{SupportCORS(r)}` when enabled |
| `router/router.go` | `wiring.WrapClient` on the general, cache and floors clients; `wiring.WrapAnalytics`; `Router.ParityDeps` |
| `server/server.go` | exported `WrapCompression` |
| `exchange/parity_export.go` | `BuildParityBidder`, `ParityAdapterConfig`, `ParityServerConfig`, `ParityMockBidderResponse`, `ConvertParityBidderResponse`, `ParityError` |
| `adapters/{adnuntius,appnexus,flipp}` Builders | inject `fakes.Active()` into the existing time, random and UUID seams |
| `adapters/{cadent_aperture_mx,huaweiads,tappx,yieldlab}` | `time.Now()` replaced by `fakes.Now()` |
| `adapters/eplanning` | `rand.Int()` replaced by `fakes.Int()` |
| `exchange/exchange.go` | `HoldAuction` wraps `holdAuction` (`exchange.response`); stage hooks after the processed-auction hook stage, floors enrichment, setup, FPD, `cleanOpenRTBRequests`, `floors.Enforce`, category mapping, events, the all-processed-bid-responses hook stage, winners, targeting, `setSeatNonBid` and each bidder goroutine; `category.dedupe` in `randomDeduplicateBidBooleanGenerator.Generate` |
| `exchange/bidder.go` | `requestBid` tracer `pt` (`hooks.bidder_request`, `adapter.requests`, `adapter.requests_final`, `adapter.bids` through `pt.makeBids`, `hooks.raw_bidder_response`, `bids.adjusted`); bidder context for the transport; `doRequest` short-circuit for `mock.level = bidder_response`; throttle note |
| `exchange/utils.go` | `privacy.bidder` in the `cleanOpenRTBRequests` loop |
| `exchange/auction.go` | `cache.result` at the end of `doCache` |
| `exchange/price_granularity.go` | `pricegranularity.timesplit` |
| `exchange/parity_stages.go` | the exchange recorders (harness code living in package `exchange`) |
| `floors/{floors,rule,enforce}.go`, `floors/parity.go` | `floors.enrich` after `updateFloorsInRequest` rebuilds the request; the four `floors.*` decisions |
| `usersync/shuffler.go` | `usersync.shuffle` |

### Linking the `/__parity/*` routes (PH-07)

`parity/server` implements the routes and registers them from `init`:

```go
func init() { wiring.RegisterRoutes(New) } // New(deps wiring.Deps, dispatch, app http.Handler) http.Handler
```

`dispatch` is the exact handler the main listener serves, compression
included. `app` is the same chain without compression. `deps` carries the
resolved config, the Prometheus gatherer and the bidder infos. The binary links
the routes with a blank import in `main.go`:

```go
import _ "github.com/prebid/prebid-server/v4/parity/server"
```

Until that import exists, an authorized `/__parity/*` request answers `501`.

## Implementing the contract in Rust

1. Serve the same `/__parity/*` routes behind the same guards and the same
   config keys.
2. Emit the documents above. Validate them in CI, either with the schema files
   or by posting them to the Go runner's validator. `schema.Validate` in Go
   dispatches on the `schema` member.
3. Record every stage in `stages.json` at the equivalent site. Report
   `unimplemented` for stages that have no site yet.
4. Read `envelope.decisions` at the seven decision sites. Inject the
   `fakes.json` values into the 8 adapters.
5. Route every outbound call through an oracle-proxy or inline transport with
   identical kinds, classification and `X-Parity-Meta`.
6. Produce Go's error codes (`error_codes.json`) and Prometheus names, labels
   and deltas.

## Tests

```sh
go test ./parity/...                 # schemas, examples, registries, contract conformance
go test -race ./parity/trace/... ./parity/transport/... ./parity/diff/...
go test ./config ./router ./server ./exchange
```

`parity/schema` checks the following:

* every schema compiles;
* every example validates and round-trips through its Go type;
* stages, decisions, outbound kinds, mismatch kinds and classes match the Go
  constants;
* every normalization rule cites a Go source;
* `error_codes.json` matches `errortypes`;
* `fakes.json` matches `fakes.Defaults()`;
* a trace produced by the real Go recorders validates against
  `trace.schema.json`.

`parity/diff` checks the following:

* the golden cases in `testdata/diff` produce exactly their expected records;
* every allowlist rule is implemented, and every implementation is allowlisted;
* removing each rule re-enables the difference it covers;
* the seeded-bug canary: a Rust side that drops `X-Prebid`, under every kind of
  allowlisted noise, yields exactly one primary cluster, at
  `adapter.requests_final`;
* noise alone (A/A) yields no records;
* every diff document validates against `mismatch.schema.json`.
