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
| `server/` | The Go server's `/__parity/*` routes: exec and its aliases, the adapter endpoints, config, bidders, reset and health. Linked into the binary by a blank import in `main.go`. |
| `endpointtrace/` | One-line endpoint-stage hooks for `/openrtb2/auction`, `/openrtb2/amp` and `/openrtb2/video`; its package doc lists each endpoint stage payload. |
| `shadow/` | Independent deep copies of a `RequestWrapper` for request snapshots, so hooks never rebuild the live wrapper. |
| `stagedata/` | Payload types of the exchange stages (`hooks.processed_auction` to `exchange.response`). |
| `corpus/` | Case loaders: adapter fixtures, sample requests, exchangetest, bidder params, stored data, seeds, and the `go/ast` builder-config extractor. Harness-only. |
| `oracle/` | Mock oracle: loopback forward proxy, per-case plans, fixture replay, templates, content-addressed cache UUIDs, static currency and GVL. Harness-only. |
| `fixtures/` | Static oracle data: `currency/latest.json` and the `gvl/v2`, `gvl/v3` vendor lists. |
| `diff/` | The differ: normalization by the allowlist, semantic, lexical and wire comparison, attribution and clustering. Harness-only. |
| `testdata/diff/` | Golden differ cases: a Go trace, a Rust trace and the exact mismatches expected. |
| `runner/` | The differential runner: config profiles, server pairs, sharding, case execution, oracle capture merging, the A/A self-test and the adapter fixture self-test, and live minimization. Harness-only. |
| `report/` | Run artifacts: per-case summaries, `index.json`, `run.json`, `clusters.json`, `summary.md`, `junit.xml`, and the CI ratchet baseline. Harness-only. |
| `matrix/` | The domain coverage matrix (`domains.json` locks the 9 domains) and its checked-in snapshot `status.json` and `README.md`. Harness-only. |
| `minimize/` | ddmin minimizer over envelope bodies, headers, query and mock rules. Harness-only. |
| `cmd/parity/` | The CLI: `corpus`, `run`, `selftest`, `minimize`, `report`, `matrix`. |
| `fixtures/profiles/` | Base host configurations (`default`, `gdpr-enforced`, `floors`, `account-required`, `events-cache`). |
| `fixtures/stored/` | Stored requests, imps, responses and accounts copied into every run's stored-data tree. |

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
`warnings` lists, `[]` when there are none. The `exchange.response` lists hold
the error HoldAuction returned; the endpoint that called it attaches them, so
the error appears once. A `bidder#call` key counts the
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
* `http_out` order with `seq` dropped, and the order of a Prebid Cache call's
  `puts` (with its `responses` permuted alongside);
* the `responsetimemillis` key set;
* the shape of the auction timestamp and of event `ts`;
* UUID placeholders;
* hook execution times and timing histograms, compared by count only;
* the Go runtime collector's `go_*` series in `metrics_delta`;
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
| `main.go` | blank import of `parity/server`; `wiring.Bootstrap(cfg)`; currency client `wiring.WrapClient`; `wiring.Mount` around `NoCache{SupportCORS(r)}` when enabled |
| `router/router.go` | `wiring.WrapClient` on the general, cache and floors clients; `wiring.WrapAnalytics`; `Router.ParityDeps` |
| `server/server.go` | exported `WrapCompression` |
| `exchange/parity_export.go` | `BuildParityBidder`, `ParityAdapterConfig`, `ParityServerConfig`, `ParityMockBidderResponse`, `ConvertParityBidderResponse`, `ParityError` |
| `adapters/{adnuntius,appnexus,flipp}` Builders | inject `fakes.Active()` into the existing time, random and UUID seams |
| `adapters/{cadent_aperture_mx,huaweiads,tappx,yieldlab}` | `time.Now()` replaced by `fakes.Now()` |
| `adapters/eplanning` | `rand.Int()` replaced by `fakes.Int()` |
| `endpoints/openrtb2/{auction,amp_auction,video_auction}.go` | `endpointtrace.*` one-liners at the endpoint stages: entrypoint, stored fetch, account, raw auction, merge, normalize, stored responses, validate, GDPR, the HoldAuction and seat-non-bid errors, auction response, AMP targeting, exitpoint, hook outcomes, video pods and cache |
| `exchange/exchange.go` | `HoldAuction` wraps `holdAuction` (`exchange.response`); stage hooks after the processed-auction hook stage, floors enrichment, setup, FPD, `cleanOpenRTBRequests`, `floors.Enforce`, category mapping, events, the all-processed-bid-responses hook stage, winners, targeting, `setSeatNonBid` and each bidder goroutine; `category.dedupe` in `randomDeduplicateBidBooleanGenerator.Generate` |
| `exchange/bidder.go` | `requestBid` tracer `pt` (`hooks.bidder_request`, `adapter.requests`, `adapter.requests_final`, `adapter.bids` through `pt.makeBids`, `hooks.raw_bidder_response`, `bids.adjusted`); bidder context for the transport; `doRequest` short-circuit for `mock.level = bidder_response`; throttle note |
| `exchange/utils.go` | `privacy.bidder` in the `cleanOpenRTBRequests` loop |
| `exchange/auction.go` | `cache.result` at the end of `doCache` |
| `exchange/price_granularity.go` | `pricegranularity.timesplit` |
| `exchange/parity_stages.go` | the exchange recorders (harness code living in package `exchange`) |
| `floors/{floors,rule,enforce}.go`, `floors/parity.go` | `floors.enrich` after `updateFloorsInRequest` rebuilds the request; the four `floors.*` decisions |
| `usersync/shuffler.go` | `usersync.shuffle` |

### The `/__parity/*` routes (PH-07)

`parity/server` implements the routes and registers them from `init`
(`wiring.RegisterRoutes(New)`). `main.go` links them with a blank import:

```go
import _ "github.com/prebid/prebid-server/v4/parity/server"
```

`wiring.Mount` serves them only when `parity.enabled` is true, after the
loopback and token checks. A binary built without the import answers `501` to
every authorized `/__parity/*` request. `New(deps, dispatch, app)` receives
`dispatch`, the exact handler the main listener serves (compression included),
and `deps`, which carries the resolved config, the Prometheus gatherer and the
bidder infos.

| Route | Method | In | Out |
|---|---|---|---|
| `/__parity/health` | GET | | `pbs-parity/1` |
| `/__parity/exec` | POST | `pbs-parity-envelope/1` | `pbs-parity-trace/1` |
| `/__parity/auction`, `/__parity/video` | POST | envelope with `request.method` and `request.path` optional | `pbs-parity-trace/1` |
| `/__parity/amp` | POST | same as above, fixed to `GET /openrtb2/amp` | `pbs-parity-trace/1` |
| `/__parity/amp?<query>` | GET | the call's query, `Host` and headers (without `X-Parity-Token`) | `pbs-parity-trace/1` |
| `/__parity/adapter/{bidder}/requests` | POST | `pbs-parity-adapter-requests-input/1` | `pbs-parity-adapter-requests-output/1` |
| `/__parity/adapter/{bidder}/bids` | POST | `pbs-parity-adapter-bids-input/1` | `pbs-parity-adapter-bids-output/1` |
| `/__parity/config` | GET | | `pbs-parity-config/1` |
| `/__parity/bidders` | GET | | `pbs-parity-bidders/1` |
| `/__parity/reset` | POST | | `pbs-parity-reset/1` |

The routes answer with these statuses:

* `200` for every document the route produced. A case whose handler failed is
  still `200`: the failure is in the trace.
* `400` for an input document that is not valid. That covers unknown members,
  a wrong `schema`, an unknown major version, an unknown stage, decision point,
  mock mode, level or outbound kind, a header name that is not a token, a header
  value with CR, LF or NUL, and a `Content-Length` that differs from the body.
  It also covers a route bidder that is not `build.bidder`, and a bidder that
  has no bidder info.
* `404` for an unknown route.
* `405` with `Allow` for a wrong method.
* `413` for a document over 64 MiB.
* `500` for a bidder panic on the adapter routes, and for a config dump
  without a resolved configuration.
* `503` when the caller gives up while waiting for the exec lock.

The routes behave as follows:

* **Inbound request.** Exec rebuilds the request with `RemoteAddr`
  `127.0.0.1:0` and host `127.0.0.1` unless `remote_addr`, `host` or a `Host`
  header says otherwise. A chunked `Transfer-Encoding` moves to
  `Request.TransferEncoding`, as `net/http` does. Any other
  `Transfer-Encoding` is rejected.
* **`http_in`.** This stage echoes the envelope request and its wire bytes.
* **`http_response`.** This stage is the handler chain's response as the
  `net/http` server would send it, minus the connection framing (`Date`,
  `Content-Length`, `Transfer-Encoding`). The header is snapshotted at
  `WriteHeader`. `Content-Type` is sniffed like `net/http`. No body is
  recorded for 1xx, 204 and 304 responses or for `HEAD`. A handler panic or an
  invalid status code records status 500, no headers and no body, plus a
  `panic: ...` note.
* **`metrics_delta`.** Without a Prometheus registry
  (`metrics.prometheus.port` 0), this stage stays `not_reached` and the trace
  carries a note.
* **Adapter routes.** These routes build the bidder with
  `exchange.BuildParityBidder`. An omitted `adapter_config` or `server`
  defaults to what `BuildAdapters` derives from the bidder info and host
  config. The routes hold the exec lock, set `fakes` for the call and reset
  them afterwards. `errors` keeps the adapter's order. Header names keep their
  case in both directions.
* **Config dump.** `GET /__parity/config` is `mapstructure.Decode` of the
  resolved configuration, without `adapters` and `parity`. Other secrets stay
  in the dump, because the route sits behind the token and the loopback
  check.
* **Reset.** `POST /__parity/reset` waits for the in-flight case and restores
  the default fakes. It reports router-internal caches from the configuration.
  A cache that the configuration disables is clean. An enabled stored-data
  in-memory cache, the Fetchers 2.0 account cache, the floors fetcher and
  bidder health throttling are reported in `not_reset`, with the config key
  that disables them. The GVL vendor lists and the currency rates are always
  `not_reset`. Parity profiles should disable these caches.

## Runner

```sh
go run ./parity/cmd/parity selftest --corpus sample-requests,adapters,exchangetest
go run ./parity/cmd/parity run --impl-b rust --bin-b rust/target/release/pbs-server --baseline parity/baseline.json
go run ./parity/cmd/parity minimize --run parity/out/<run-id>
go run ./parity/cmd/parity report --run parity/out/<run-id> --baseline parity/baseline.json
go run ./parity/cmd/parity matrix --run parity/out/<run-id>
```

`--impl-a` and `--impl-b` name the sides (`go` or `rust`). `--bin-a` and
`--bin-b` give their binaries. Without `--bin-*`, a Go side is built into
`parity/out/bin/pbs-go`. A Rust side uses `rust/target/release/pbs-server`
when it exists. Otherwise the Rust side is not run, and every case is
not-yet-instrumented.

Exit codes:

| Code | Meaning |
|---|---|
| 0 | Success. |
| 1 | The self-test found a mismatch, a harness error or a failing fixture, or the ratchet regressed. |
| 2 | A usage or harness error. |
| 3 | The self-test was incomplete: nothing could be compared. |

### Batches and processes

1. The runner selects cases by `--corpus`, `--kinds`, `--match` and `--limit`.
   * Every case runs under every `--profiles` base profile. A case ID gets the
     suffix `@<profile>` when the base profile is not `default`.
   * Adapter cases run once, under the first base profile.
   * `contract_gap` cases are skipped.
   * `--corpus introspection` (part of `all`) adds one config and bidders
     comparison per batch.
2. Cases are grouped into batches. A batch shares the base profile, the case
   profile and stored data that does not conflict. Two cases that give
   different bytes to the same stored ID go to separate batches.
3. Each batch gets a working directory, `<run>/work/<batch>/`, containing:
   * `pbs.yaml`, merged from the profile `extends` chain, then the case
     profile, then the keys the runner owns;
   * a `static/` symlink;
   * `stored/{stored_requests,stored_imps,stored_responses,accounts}/<id>.json`;
   * `default_request.json`, when the case profile has one.
4. Ports, the parity token and `TZ=UTC` go through the environment. Both
   servers therefore read byte-identical files. A non-Go server also gets
   `PBS_CONFIG_FILE` and `PBS_STATIC_DIR`.
5. `--shards N` starts N pairs of servers per batch on free loopback ports.
   Each pair runs its share of the cases one at a time, in the same order on
   both sides, and calls `/__parity/reset` before every envelope.
6. When the two sides have different implementation names, they run each
   envelope concurrently. In an A/A self-test both sides are `go`, so they run
   one after the other, and each side's oracle captures are drained before the
   next side starts.

A server can be not parity-capable: it answers `/__parity/health` with 404 or
501, does not start, or reports a different `impl`. A health document can also
show that the server lacks something a case needs: the endpoint, the mock mode
or level, or the fakes for a `needs_fake` adapter case. In each of these cases,
the affected cases are not-yet-instrumented, with the reason, and the run still
writes every report. `--budget` stops starting cases after the given time; the
remaining cases are skipped.

### Run artifacts

`parity/out/<run-id>/` contains the following. The runner writes
`parity/out/.gitignore`.

| File | Contents |
|---|---|
| `index.json` | `pbs-parity-index/1`: every case with its status, its first divergence and the case directory. |
| `run.json` | Counts, options, and the server records: binary, health, reason and log for each side and shard. |
| `clusters.json` | Mismatch clusters by signature, with their representative case and repro paths. |
| `summary.md` | Counts, reasons cases were not compared, cluster tables by domain, and failing fixtures. |
| `junit.xml` | One suite per corpus plus `fixture-selftest`. Mismatch is a failure, error is an error, and not-yet-instrumented or skipped is skipped. |
| `fixtures.json` | Adapter fixture self-test results, when any adapter case ran: passing, failing, excused (`needs_fake`, `unresolved_build`) or not-run. |
| `matrix.json`, `matrix.md` | The coverage matrix of this run. |
| `cases/<sha1-16>/` | `summary.json` for every case. A mismatch or error also gets `case.json` (everything needed to rerun the case), `envelope.json`, both traces, `diff.json`, and `min.*.json` after minimization. |
| `servers/*.log` | Server stdout and stderr. |

### Self-tests

* **A/A.** `selftest` runs implementation A against a second process of
  itself. Any mismatch means missing normalization or state leaking between
  cases in the harness.
* **Adapter fixtures.** `selftest`, or `run --fixtures`, also checks A's
  adapter endpoints against each fixture's own expectations, with the
  `adapterstest` semantics:
  * request count, URI, headers (when expected), imp IDs and JSON body,
    matched out of order;
  * error lists, compared literally, by regex or with `startswith`;
  * bid responses: currency, bid count, seat, type, bid JSON, video and fledge
    configs.

  A failure on a `needs_fake` or `unresolved_build` case is excused.

### Minimization and the ratchet

`minimize` re-runs the representative case of each primary cluster on a fresh
pair. The `--cluster` and `--case` flags select others. ddmin keeps the case's
primary signatures, and writes `min.envelope.json`, `min.case.json` and
`min.report.json` into the case directory. The run's reports and matrix files
are then rebuilt. Adapter and introspection cases are not minimized.

`report --baseline parity/baseline.json` fails, with exit code 1, when any of
the following holds:

* mismatches, errors or the count of any mismatch kind grew;
* fewer cases were compared;
* a new cluster appeared.

`--update-baseline` writes the current totals.

`matrix --run <dir>` writes the checked-in snapshot `parity/matrix/status.json`
and `parity/matrix/README.md`. It has one row for each of the 9 domains in
`matrix/domains.json`. A row is `passing` or `mismatch` only with evidence
cases; otherwise it is `not-yet-instrumented`. The matrix also has
per-endpoint, per-bidder and per-stage tables.

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
go test ./parity/...                 # schemas, examples, registries, contract conformance, /__parity/* routes
go test -race ./parity/trace/... ./parity/transport/... ./parity/diff/...
go test -race ./parity/runner/... ./parity/minimize/... ./parity/report/... ./parity/matrix/...
go test ./config ./router ./server ./exchange
```

`parity/runner` runs its own test binary as a fake parity server, built on
`parity/trace`, which makes one oracle call per oracle-mode case. With that
server it checks the following:

* an A/A self-test passes, with oracle captures merged and config ports masked;
* a seeded bug on side B is a mismatch, and live minimization keeps only the
  triggering member;
* a server without parity routes, a missing binary, or a wrong `impl` makes
  every case not-yet-instrumented, and all reports are still written.

It also loads every profile's generated `pbs.yaml` through `config.New`, and
encodes a `case.json` for every case of the repository corpus.

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

`parity/server` checks the following:

* exec over a minimal router returns the status, headers and body a real
  `net/http` server sends, and the status and body of a direct `ServeHTTP`.
  The cases cover JSON, gzip, sniffing, late headers, 204, 304, flush, HEAD,
  router 404 and router 405;
* the inbound request is rebuilt exactly: query, header order, host, remote
  address, byte-exact b64 and content-encoded bodies, and chunked framing;
* stages, inline `http_out`, decisions, fakes and `metrics_delta` reach the
  trace, and `capture` limits them;
* invalid envelopes are rejected, and the aliases check their fixed method and
  path;
* the adapter routes replay appnexus, rubicon, ix, openx, pubmatic and adagio
  fixtures and match each fixture's own expectations with `adapterstest`
  semantics;
* the config dump contains `tmax_default`, `gdpr.default_value` and
  `cache.scheme`, and contains neither `adapters`, `parity` nor the token;
* the bidders dump, health and reset documents are correct;
* the token and loopback guards apply to the routes registered from `init`;
* every response validates against its schema.
