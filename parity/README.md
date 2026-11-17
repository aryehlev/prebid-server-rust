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
5. Before taking the after-snapshot of Prometheus, the server waits up to 5s
   for the case's bidder goroutines (`trace.Session.Track`/`Settle`). Their
   deferred metrics, such as `RecordAdapterRequest`, can run after the bidder
   sent its result, and therefore after the response was written. A case that
   does not settle in time gets a note.
6. The server returns the trace:
   * `http_in` and `http_response` hold the exact status, headers and body bytes.
   * `stage_status` covers every registered stage.
   * `metrics_delta` lists the changed series.
   * `analytics`, `decisions` and `notes` complete the document.
7. In oracle mode the runner merges the oracle's wire captures into `http_out`
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

**Generated UUIDs.** Every other v4 UUID the server generates during a case
comes from a deterministic stream. These are the request ID for `{{UUID}}`,
`source.tid`, `imp.ext.tid`, the generated bid IDs and the AMP and video
request IDs (the `go_sites` of `generated_uuids` in `schema/fakes.json`). The
n-th UUID of a case (n = 1, 2, ...) is `sha256("<fakes.uuid>:<n>")`, truncated
to 16 bytes with the version 4 and RFC 4122 variant bits set. The counter
restarts each time the fakes are set: at the start and end of every case and
adapter call, and on `/__parity/reset`.
`fakes.Install` wraps `uuid.DefaultGenerator` (gofrs/uuid), so `uuid.NewV4()`
takes the stream while a parity case runs and stays random otherwise. The
differ still numbers these values as `<uuid#n>`. The stream exists because a
random tid changes the gzip-compressed size of a bidder request
(`endpoint_compression`, `exchange/bidder.go:867-885`), which the never
normalized `content_length` wire class reports. The first values are listed in
`fakes.json` for a Rust implementation.

**Deterministic orders.** Some Go outcomes, not just their order, depend on
response completion order or map iteration. In parity mode the server
imposes the orders in the `orders` member of `schema/fakes.json`:

* The bidders of one auction build their requests one at a time, in bidder
  name order (`exchange/parity_turns.go`). A bidder's bidder-request hook
  stage and `MakeRequests` start after `MakeRequests` of every bidder before
  it has returned. HTTP calls and `MakeBids` stay concurrent. The order
  matters because per-bidder requests share objects: gumgum copies `Site` but
  writes the publisher ID through the shared `Publisher`
  (`adapters/gumgum/gumgum.go:45-46`). Whether another bidder sends the
  original ID or gumgum's then depends on scheduling. Name order is one of
  the schedules Go can take.
* First party data is resolved for the bidders in name order
  (`firstpartydata/first_party_data.go:142`). Go appends the errors per
  bidder in map order, keeps a bidder only while no error has occurred, and
  joins the messages into one error.
* A bidder's responses reach `MakeBids` in call order: the `MakeRequests`
  output order, then the stored bid responses by imp ID
  (`parityBidderTrace.inCallOrder`, `exchange/bidder.go:287`). The order
  matters because `requestBid` sets an empty request `cur` to USD after the
  first `MakeBids` (`exchange/bidder.go:327-329`), which adgeneration reads.
  It also matters because flipp keeps imp params in a package variable
  (`adapters/flipp/flipp.go:37`).
* adtarget, adtelligent, bidmatic, datablocks, trafficgate and
  zeroclickfraud range over a map of imp groups, and each group writes the
  shared `request.Imp`, which `MakeBids` reads back. `fakes.MapKeys` returns
  their groups sorted. adtarget, adtelligent, bidmatic and datablocks sort by
  `sourceId`, trafficgate by `(placementId, host)` and zeroclickfraud by
  `(sourceId, host)`.
* Imp ext validation checks the imp's bidders in name order
  (`ortb/request_validator.go:127`). Go returns the first bidder whose params
  fail validation, so with two failing bidders the message names whichever
  one map iteration reached first. The other request validations that stop
  at the first failing map entry get the same treatment: the stored bid
  response bidders (`ortb/request_validator.go:184`), the aliases and the
  alias GVL IDs (`endpoints/openrtb2/auction.go:1086`, `:1110`) and the bid
  adjustment factors (`endpoints/openrtb2/auction.go:1000`).
* The auction picks each imp's winner visiting the seats in bidder name
  order (`exchange/auction.go:116`). A bid replaces the winner only at a
  strictly higher price, so of two bids at the same price the first bidder
  visited wins. Category mapping visits the seats in the same order
  (`exchange/exchange.go:1092`), so the earlier bid the `category.dedupe`
  decision keeps or drops is the one from the smaller bidder name.

Outside parity mode `parityTurns` returns the context unchanged,
`inCallOrder` returns the channel unchanged and `MapKeys` returns the keys in
map order. `parity/SELFTEST.md` records how the A/A
self-test found these sites.

## Normalization and what is never normalized

`schema/normalization.json` is the complete allowlist. Every rule cites the Go
source of the nondeterminism it removes:

* seatbid, warning, httpcalls and seatnonbid order, including a bidder's
  `bidder.result` http calls and non-bids, and the http calls the hook stages
  and analytics record. The rules also apply to the response ext the AMP
  endpoint returns as `ortb2.ext`;
* the order of one seat's bids, and of a bidder's `bidder.result` errors, in
  the exchange stages, which Go fills in response arrival order;
* the order of a bidder's `MakeRequests` output, and of each request's imp
  IDs (also where `adapter.bids` and `hooks.raw_bidder_response` echo the
  request), which adapters that group imps in a map return in map order (Go's
  adapter tests ignore both);
* the `bidder#call` index of `adapter.bids` entries, re-derived from their
  requests, together with the `bids.adjusted` call members that name them;
* `http_out` order with `seq` dropped, and the order of a Prebid Cache call's
  `puts` (with its `responses` permuted alongside);
* the `responsetimemillis` key set (a value is masked only where it is a
  non-negative integer);
* the shape of the auction timestamp and of event `ts`, wherever an event URL
  is recorded (responses, hook stages, analytics);
* UUID placeholders;
* hook execution times, masked only where the member is a non-negative
  integer (its presence is still compared);
* timing histograms, compared by count only;
* the Go runtime collector's `go_*` series in `metrics_delta`, and the main
  listener's `connections_*` series. Exec dispatches in process, so those
  series only count the runner's own connections;
* the `Date` header;
* semantic JSON member order;
* the stage entry `seq`;
* the order of the gojsonschema items in a bidder-params validation message.
  The items are split by the library's own description templates, and a list
  that does not split exactly one way is left as recorded;
* the imp ID of a stored auction response media type error. It is masked only
  when the side's own `stored.responses` shows two or more imps whose stored
  bids Go rejects, and the reported imp is one of them;
* the response `cur`, which Go takes from the last seat with bids its map
  iteration visits. It is masked only when the side's own `bidder.result`
  shows seats with bids in two or more currencies, and the `cur` is one of
  them;
* the AMP targeting keys that cached bids from different seats set to
  different values. The AMP endpoint merges the bids' targeting in seatbid
  order, and a later seat overwrites an earlier one. A key is masked only
  when the side's own `exchange.response` bids disagree on it, and some merge
  order of those bids gives exactly the recorded targeting.

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
  vs `1.0`), `trailing_newline`, `whitespace`, `member_order`, `unrecorded`
  and `other`. `unrecorded` means one side recorded only the semantic `json`
  of a value the other side recorded as exact bytes (`raw`, `raw_b64`, `text`
  or `b64`). None of the other classes can then be checked, so the gap is
  reported rather than skipped.
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
| per-call transports built by product code (`/optout` recaptcha check) | `general`, through `transport.WrapProcess` |

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
| `endpoints/openrtb2/{auction,amp_auction,video_auction}.go` | `endpointtrace.*` one-liners at the endpoint stages: entrypoint, stored fetch, account, raw auction, merge, normalize, stored responses, validate, GDPR, the HoldAuction and seat-non-bid errors, auction response, AMP targeting, exitpoint, hook outcomes, video pods and cache; the alias, alias GVL ID and bid adjustment factor validation loops range over `fakes.MapKeys(...)` |
| `exchange/exchange.go` | `HoldAuction` wraps `holdAuction` (`exchange.response`); stage hooks after the processed-auction hook stage, floors enrichment, setup, FPD, `cleanOpenRTBRequests`, `floors.Enforce`, category mapping, events, the all-processed-bid-responses hook stage, winners, targeting, `setSeatNonBid` and each bidder goroutine; `defer s.Track()()` first in each bidder goroutine, so exec waits for its deferred metrics; `ctx = parityTurns(...)` before the bidder loop and `defer parityTakeTurn(...)()` in each bidder goroutine; `category.dedupe` in `randomDeduplicateBidBooleanGenerator.Generate`; the `applyCategoryMapping` seat loop ranges over `fakes.MapKeys(...)` |
| `exchange/bidder.go` | `requestBid` tracer `pt` (`hooks.bidder_request`, `adapter.requests`, `adapter.requests_final`, `adapter.bids` through `pt.makeBids`, `hooks.raw_bidder_response`, `bids.adjusted`); `pt.inCallOrder` before the response loop; bidder context for the transport; `doRequest` short-circuit for `mock.level = bidder_response`; throttle note |
| `adapters/{adtarget,adtelligent,bidmatic,datablocks,trafficgate,zeroclickfraud}` | the imp-group map loop ranges over `fakes.MapKeys(...)` |
| `ortb/request_validator.go` | the imp ext bidder loop and the stored bid response bidder loop range over `fakes.MapKeys(...)` |
| `exchange/utils.go` | `privacy.bidder` in the `cleanOpenRTBRequests` loop |
| `exchange/auction.go` | `cache.result` at the end of `doCache`; the `newAuction` seat loop ranges over `fakes.MapKeys(...)` |
| `exchange/price_granularity.go` | `pricegranularity.timesplit` |
| `exchange/parity_stages.go`, `exchange/parity_turns.go` | the exchange recorders and the bidder turns (harness code living in package `exchange`) |
| `firstpartydata/first_party_data.go` | the `ResolveFPD` bidder loop ranges over `fakes.MapKeys(...)` |
| `floors/{floors,rule,enforce}.go`, `floors/parity.go` | `floors.enrich` after `updateFloorsInRequest` rebuilds the request; the four `floors.*` decisions (the rate-driven ones honour the envelope only where the rate leaves the outcome to chance) |
| `usersync/shuffler.go` | `usersync.shuffle` |
| `pbs/usersync.go` | `transport.WrapProcess` on the transport `VerifyRecaptcha` builds per call |

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
go run ./parity/cmd/parity run --corpus all --gen 5000 --seed 42 --shards 4
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
   * `--gen N --seed S` first adds N cases drawn by `parity/gen` (corpus
     `gen`). Their IDs regenerate them exactly. A `--corpus` list that names
     neither `all` nor `gen` gets `gen` added.
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
  cases in the harness. `parity/SELFTEST.md` records the runs at scale and
  what each mismatch class turned out to be.
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
   `fakes.json` values into the 8 adapters. Generate case UUIDs from the
   `generated_uuids` stream. Impose the `orders`.
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
  every case not-yet-instrumented, and all reports are still written;
* a trace that does not validate against its schema makes the case a harness
  error, never a passing case. Decoding drops members the contract does not
  know, so such a trace could otherwise hide differences. The same applies to
  adapter outputs and introspection dumps.

It also loads every profile's generated `pbs.yaml` through `config.New`, and
encodes a `case.json` for every case of the repository corpus.

`parity/schema` checks the following:

* every schema compiles;
* every example validates and round-trips through its Go type;
* stages, decisions, outbound kinds, mismatch kinds and classes match the Go
  constants;
* every normalization rule cites a Go source;
* `error_codes.json` matches `errortypes`;
* `fakes.json` matches `fakes.Defaults()`, and each of its `orders` names a
  Go file that holds the seam;
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
* the mutation tests seed 41 known differences into the Rust copy of a
  recorded auction trace (`testdata/diff/mutation`). Each one must be detected
  with the right primary stage, kind and class. A propagating bug must also
  leave consequential records downstream. Eight noise mutations must compare
  equal;
* the time-valued rules mask only values of Go's shape (13-digit millisecond
  timestamps, non-negative integer durations), so a value with the wrong unit,
  sign or presence is still reported;
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
* the adapter routes replay appnexus, rubicon, ix, openx, pubmatic, adagio and
  mediasquare fixtures and match each fixture's own expectations with
  `adapterstest` semantics. The route decodes the request with
  `encoding/json`, as `adapterstest` does, so `"ext": null` reaches the
  adapter as `null`;
* the config dump contains `tmax_default`, `gdpr.default_value` and
  `cache.scheme`, and contains neither `adapters`, `parity` nor the token;
* the bidders dump, health and reset documents are correct;
* the token and loopback guards apply to the routes registered from `init`;
* every response validates against its schema.

## ReplayHarness

ReplayHarness is a separate tool (CLI `replayharness`) that maps an HTTP API
from its source code and then generates requests from that map: built-in
probes, replays its agents write, and recorded or mutated traffic. For parity,
its value is the requests it generates. Its own `--baseline` comparison only
checks the status code and the body's shape, so it adds to the byte-exact
Go-vs-Rust runner and does not replace it. The repository has the API map
and a check for it, replays that pin what Go answers, and a recording of
that traffic. Nothing yet turns ReplayHarness requests into parity cases.

### The API map

`parity/replayharness/api.yaml` maps the Go server in the format of the
ReplayHarness discovery agent (`agents/replayharness-discovery.md` in the
ReplayHarness repository). It has 28 endpoints:

* every route on the main port: `router/router.go`, plus the stored-request
  event API, which exists only with `stored_requests.cache_events.enabled`;
* the admin port (`router/admin.go`) and the Prometheus port
  (`server/prometheus.go`). The map has no port field, so these endpoints say
  so in their first rule.

For each endpoint the map records:

* the handler's `file:line`;
* the path, query and header parameters, and an example body;
* the status codes;
* the rules the handler enforces, each with its `file:line`: size limits,
  required parameters, account checks, privacy and consent, cookie behaviour,
  stored-data lookups, and error statuses and texts.

The notes cover what many endpoints share:

* the handler chain: NoCache, CORS and httprouter, with httprouter's 404, 405,
  redirect, HEAD and OPTIONS behaviour;
* account resolution.

The notes also give the parity facts a replay needs:

* the stored request, imp, response and account IDs in `fixtures/stored`;
* the base profiles;
* the configured and disabled bidders, and the syncer keys;
* the endpoints the profiles switch off (video, the stored-request event API);
* how to send a `uids` cookie, with ready-made values.

Two parity hazards were found while mapping:

* the `uids` Set-Cookie depends on the wall clock;
* `/cookie_sync` without `bidders` returns the syncers in map order. Outside
  a parity case, which is how ReplayHarness requests arrive, Go shuffles them
  at random, so with a limit two runs can sync different bidders.

`/__parity/*` is left out on purpose: it is the harness, not the product.

Send ReplayHarness requests only to a server that the parity runner started.
Such a server runs in parity mode on 127.0.0.1, and the mock oracle answers
every outbound call. Never pass `--allow-remote` to ReplayHarness.

### Checking the map

```sh
REPLAYHARNESS=/path/to/replayharness parity/scripts/replayharness-context.sh [report.md]
```

The script reads the repository and sends no requests:

1. It builds a scan root in a temporary directory outside the repository. The
   root holds every non-test Go file that could register a route, without
   `rust/` and `parity/`. Full-line `//` comments are blanked, with line
   numbers kept, so documentation examples such as
   `stored_requests/events/api/api.go:22-23` are not taken for routes.
2. It copies the map into that root as `.replayharness/api.yaml`.
3. It runs `replayharness context` on the root. The script fails when the
   report has a "Spec drift" section, that is, when the code registers a route
   the map misses or the map lists a route with no handler.
4. It checks that every `file:line` the map cites is inside an existing
   repository file.

It exits 0 when there is no drift and every citation is valid, and 1
otherwise. The optional argument keeps the full context report. After a route
or handler changes, update `api.yaml` and run the script again.

### Running ReplayHarness against Go

```sh
REPLAYHARNESS=/path/to/replayharness parity/scripts/replayharness-replay.sh \
    [--baseline] [--record] [--serve] [--profile NAME] [--out DIR]
```

The script starts the Go server with `rhtool serve`
(`parity/replayharness/rhtool`), which uses the runner's own steps:

* `runner.LoadProfiles` and `BuildConfig` with the base profile (`default`
  unless `--profile` names another) and the oracle's URL;
* `runner.PrepareWorkdir` with `fixtures/stored` plus
  `parity/replayharness/stored`;
* `runner.StartServer` on ports from the free-port helper, in parity mode on
  127.0.0.1;
* the mock oracle, started with the adapter corpus's templates, so every
  bidder, prebid cache, currency, GVL and stored-data call is answered on
  loopback.

The parity token is random, written to a 0600 file in the state directory,
and never given to ReplayHarness: `/__parity/*` stays as the harness has it,
token-gated and loopback-only. `HTTP_PROXY` and `HTTPS_PROXY` point at the
oracle in the servers, and at `127.0.0.1:9` in every other process the
script starts. Every second the oracle's captures are appended to
`outbound.jsonl` in the state directory, and on exit `rhtool serve` prints
the count by kind. A watchdog runs `lsof` on every server and on the
recording proxy each second, and kills them all if one has a socket off
loopback. ReplayHarness only ever targets 127.0.0.1; never pass it
`--allow-remote`.

The steps, in order:

1. **Probes.** `replayharness run parity/replayharness/api.yaml` against Go.
   Go is the reference, so findings are only reported.
2. **Replays.** `replayharness replay parity/replayharness/replays`. Every
   replay must pass.
3. **`--baseline`.** The probes against two Go servers. Any divergence fails
   the run, except a shape divergence on `POST /cookie_sync`: Go shuffles the
   syncers it evaluates (`usersync/chooser.go`, `usersync/shuffler.go`), so
   with a limit or without a bidder list two runs pick different bidders and
   `debug` comes and goes, which changes the shape of the body. That
   divergence is printed but does not fail. A status divergence there fails
   the run, as it does on any other endpoint.
4. **`--record`.** `replayharness record` on a spare loopback port, the probes
   and the replays through it, then `replayharness replay <recording>
   --mutate` through the same proxy. `rhtool traffic` then writes
   `parity/replayharness/traffic.jsonl` (see below).

`--serve` only starts the stack, prints the URLs and waits for Ctrl-C. The
reports and logs stay in the state directory; the binaries are deleted.

Requests sent straight to the main port carry no parity case. The exchange
therefore labels bidder calls as general calls, and the oracle answers them
404 as unexpected. The only bids are stored bid responses and stored auction
responses. The oracle derives each cache id from the cached payload, so ids
are stable unless the payload carries a timestamp.

### Stored data for the replays

`parity/replayharness/stored` uses the `fixtures/stored` layout and is
loaded next to it. All IDs start with `rh-`.

| Kind | IDs |
| --- | --- |
| stored requests | `rh-amp-banner`, `rh-amp-stored-bid`, `rh-amp-two-imps`, `rh-amp-app`, `rh-events-request` |
| stored imps | `rh-imp-stored-bid`, `rh-imp-video-stored-bid` |
| stored responses | `rh-appnexus-banner` and `rh-appnexus-video` (appnexus wire bids for stored bid responses), `rh-seatbid-banner` (a seatbid for stored auction responses) |
| accounts | `rh-events-account` (events on), `rh-no-debug-account` (`debug_allow` false), `rh-ccpa-account` (CCPA enforced), `rh-cookie-sync-account` (limit 1), `rh-sandbox-account` (cookie deprecation on), `rh-malformed-account` (`disabled: "yes"`, which does not parse) |

### The replays

`parity/replayharness/replays` holds 174 replays in the ReplayHarness format
(`agents/replayharness-replay.md` in the ReplayHarness repository). Each one
states what Go does, and its `why` cites the Go code.

| File | Replays | Covers |
| --- | --- | --- |
| `auction.yaml` | 25 | validation errors, debug and test, aliases, disabled bidders, accounts, currency, Content-Encoding |
| `stored.yaml` | 15 | stored requests, imps, bid responses and auction responses, targeting, currency conversion, bid adjustment |
| `amp.yaml` | 15 | tag_id, query overrides, consent types, origin headers, the targeting response |
| `video.yaml` | 4 | the deprecated endpoint, video validation, ad pod keys |
| `usersync.yaml` | 18 | setuid to getuids to cookie_sync with the saved cookie, cookie attributes, opt-out, filters, limits, privacy |
| `events.yaml` | 13 | `/event` parameters and accounts, `/vtrack` validation and cache puts |
| `privacy.yaml` | 10 | GDPR, CCPA and GPP in the auction |
| `headers.yaml` | 11 | Referer, User-Agent, X-Forwarded-For, X-Real-IP, DNT, Sec-GPC, Sec-Browsing-Topics, Origin |
| `json.yaml` | 25 | malformed and borderline JSON bodies |
| `limits.yaml` | 6 | bodies at, one byte over and two bytes over `max_request_size` |
| `info.yaml` | 12 | `/info/bidders`, `/bidders/params`, `/status`, `/version`, `/` and `/static` |
| `router.yaml` | 8 | 404, 405, redirects, OPTIONS, the CORS preflight, admin-only routes |
| `fields.yaml` | 12 | inputs nothing else sent: the `uids` cookie in the auction and AMP, Sec-GPC on the bidder request, True-Client-IP, X-Real-IP and Referer in AMP, Sec-Cookie-Deprecation, DOOH, the OpenRTB fields Go passes through, `passthrough`, `is_rewarded_inventory`, `adunitcode`, a client `ext.prebid.server` |

Things to know when editing them:

* ReplayHarness reads `{{name}}` as a variable. A body that must contain
  `{{UUID}}` uses JSON escapes, and a check splits the text.
* `contains` compares raw bytes. Go escapes `&` in some JSON as `&`; in a
  double-quoted YAML string, write the backslash as `\x5C`.
* In a YAML flow mapping, a key with `[` must be quoted:
  `{"$.debug[0].error": ...}`.
* ReplayHarness treats an empty response header as missing.
* The `uids` cookie expires by the wall clock. A flow saves `header:Set-Cookie`
  and sends it back as `Cookie`; Go reads `uids=` and skips the attributes.

### The recorded traffic

`parity/replayharness/traffic.jsonl` is a `replayharness record` file. It
holds the probes, the replays and the mutated replays of the recording, as
Go answered them in one `--record` run. `rhtool traffic` keeps each line as
ReplayHarness wrote it, and drops:

* repeated requests;
* exchanges whose request body is over 64 KiB. These are size probes: the
  recorder truncates bodies at 1 MiB, and the replays and probes send them
  again on every run;
* repeats of a response body of 64 KiB or more on the same method, path and
  status, such as `/bidders/params` fetched with an unknown query parameter.

It fails when a line holds the parity token or a credential header, or when
the result is over 5 MB. The recorder already leaves out Cookie,
Set-Cookie and Authorization. The last run read 4704 exchanges and kept 2523
on 278 endpoints, in 4172393 bytes.

A later run recorded the 12 replays of `replays/fields.yaml` with the
pinned recorder, sending the `User-Agent` that the other lines carry, and
`rhtool traffic` merged them in. The 2523 lines are unchanged; the file
holds 2535 exchanges on the same 278 endpoints, in 4201172 bytes.

To send the same requests to any server that the parity runner started:

```sh
replayharness replay parity/replayharness/traffic.jsonl --target http://127.0.0.1:PORT [--mutate]
```

### Go behaviour the replays pin

A port must copy these, even where they look wrong:

* **Errors.**
  * A JSON error at the end of the body names a NUL byte
    (`expect { or n, but found \x00`).
  * A string `tmax` error ends with a raw 0xFF byte.
  * Account errors that are not 400s still start with `Invalid request:`: a
    malformed account is 500, a disabled one is 403 in the auction and 503 in
    `/event` and `/vtrack`.
  * The size error ends with a period in the auction but not in `/vtrack`.
* **Size limit.** An auction body exactly one byte over `max_request_size`
  passes the size check and fails as truncated JSON. net/http returns the
  last byte together with `io.EOF` (`endpoints/openrtb2/auction.go:448-470`).
  Two bytes over gets the size error. `/cookie_sync` has no limit.
* **Responses.**
  * Every request without test or debug gets warning 10002 (`debug turned off
    for account`).
  * Event URLs in bid ext have `&` escaped as `&`.
  * The AMP response is `text/plain`.
  * The `/event` pixel's Content-Type is set after `WriteHeader`; net/http
    sniffs `image/png`.
* **Request enrichment.** The Referer's host fills `site.domain` even when
  `site.page` names another host.
* **`/vtrack`.** Every VAST put it modifies is broken. For a bidder that may
  modify VAST, which includes unknown bidders by default, a VAST with an
  Impression tag is put to the cache as unquoted XML
  (`endpoints/events/vtrack.go:320-333`). The cache answers 400 and
  `/vtrack` answers 500.
* **Request ids.** A `{{UUID}}` request id is replaced only when it comes
  from the stored request (`endpoints/openrtb2/auction.go:1736`). Sent in the
  body, it is kept.
* **AMP consent.** `consent_type` 4 (GPP) is not applied; it only warns.
* **No enforcement.** `imp.bidfloor` is not enforced in the default profile.
  `setuid` accepts an unparsable GDPR consent, because the host vendor ID is
  unset.
* **Static files.** A file name over 255 bytes under `/static` is 500.
* **Routing.** `httprouter` redirects a POST with a trailing slash with 307.
* **Stored data.** `fixtures/stored`'s `parity-stored-response` has no mtype,
  so using it with a bidder fails the auction with 500.

ReplayHarness limits:

* It cannot send a body that is not valid UTF-8 or that is binary, such as
  gzip.
* It cannot send a literal `{{...}}`.
* It cannot tell an empty header from a missing one.
* Its recorder adds `127.0.0.1` to X-Forwarded-For.

### ReplayHarness requests as parity cases

The `replayharness` corpus (`parity/corpus/replayharness.go`) turns what
ReplayHarness sent the Go server into parity cases, so the byte-exact runner
compares both sides on those requests. It is part of `--corpus all`. The
intro above, written before this corpus existed, says that nothing does this;
that is no longer true.

`parity/replayharness` is one of the approved corpus roots
(`corpus.ApprovedRoots`), and the corpus reads it through `Repo.FS`, so every
read goes through `Repo.Resolve`, as every other loader's does. A file whose
symlink leaves the approved roots is refused with `ErrOutsideCorpus`, not
read.

The corpus takes its requests from two sources:

* every exchange in `traffic.jsonl`;
* every step of the replays in `replays/`. Package
  `parity/replayharness/rhreplay` rebuilds each step offline the way the
  pinned ReplayHarness sends it. That covers the query encoding, `json`
  bodies written by `json.Marshal`, `{{name}}` values, and the `User-Agent`
  of the build that recorded `traffic.jsonl`. As a result, a step and its line
  in the recording are the same request.

Each distinct request is one case:

* The method, path, raw query, headers and body bytes are kept as sent.
  Header names are put in canonical form. A UTF-8 body is sent as text; any
  other body is sent as base64.
* Some headers are left out:
  * the headers the recorder leaves out (`rhreplay.HopHeaders`: `Connection`,
    `Keep-Alive`, `Proxy-Connection`, `Transfer-Encoding`, `Upgrade`, `Te`,
    `Trailer`, `Host`, `Content-Length`, `Accept-Encoding`). The transport
    writes these, or they describe one connection;
  * `X-Parity-Token`.

  The case's notes name each header left out.
* Two of those headers are put back as Go's HTTP client writes them, because
  the Go server received them and a compression layer reads one of them
  (`clientHeaders`):
  * `Content-Length`, for a body and for an empty POST, PUT or PATCH;
  * `Accept-Encoding: gzip`, on every request but a HEAD or a Range request.
    ReplayHarness's transport asks for gzip itself, and so does the
    recorder's when it forwards a request.

  The case's notes name them too. Without them, no case asked for a
  compressed answer. A server that gzips whatever
  `compression.response.enable_gzip` says showed only in the `Vary` header
  that the gzip handler adds to every answer; with them, 55 cases also
  differ in `Content-Encoding` (see "Review: what the two tools catch").
* Identical requests are one case. Its notes list every source, such as
  `from traffic.jsonl:12` or `from replays/auction.yaml: replay "...", step 1`.
* The case ID is `parity/replayharness#<method>:<path>:<hash>`. The path is
  cut at 40 bytes. The hash is the first 16 hex digits of a SHA-256 of the
  request. The ID depends on nothing but the request, so it stays the same
  across runs and file orders, and changes when the request changes.
* The envelope:
  * uses the `default` profile, plus the exec defaults for the host and the
    remote address (127.0.0.1);
  * pins every decision point, with `usersync.shuffle` set to identity;
  * carries `parity/replayharness/stored` as the case's stored data;
  * names its imp IDs for the oracle, for an auction, AMP or video request.

Some steps send a value an earlier step saved, such as the `uids` cookie
from `/setuid`. Such a step gets the value Go answered, from `saved.jsonl`.
`parity/scripts/replayharness.sh record-saved` writes that file:

1. It starts one Go server.
2. It runs the replays through `replayharness record --keep-secrets`.
3. For each step that saves a value, `rhtool saved` keeps only three things:
   the status, the saved headers, and the body when a JSON path is saved.
   It fails if the parity token would be kept.

The saved `uids` cookie gives its uid an expiry 14 days after the recording
(`2026-12-01` in the checked-in file). After that, both servers treat the uid
as expired. The cases still compare, but they no longer test a live uid, so
run `record-saved` again.

### What the corpus does not compare

Some requests cannot be compared: two correct servers may answer them
differently, or the envelope cannot carry them as sent. Such a request is not
a case. `rhtool cases` lists each one with a tag and a reason, and the
`parity` and `selftest` commands write that list to `not-compared.json`.
The corpus drops nothing recorded or replayed without an entry in that list.

| Tag | Requests | Last run |
| --- | --- | --- |
| `wall_clock` | `GET /setuid` when Go may set the `uids` cookie: its `Expires`, and the per-uid expiry inside it, come from the wall clock (`usersync/cookie.go`). Also `POST /cookie_sync` for an account with cookie deprecation on and no `receive-cookie-deprecation` cookie: Go sets that cookie with an `Expires` from the wall clock (`endpoints/cookie_sync.go`). | 51 |
| `map_order` | `POST /cookie_sync` without a bidder list, or with `coopSync`. Go evaluates the syncers in map order (`usersync/chooser.go`), and that order differs between two processes. | 5 |
| `side_port` | An API map endpoint served only on the admin or Prometheus port. `/__parity/exec` dispatches to the main router only. The requests to these paths are still cases, and they check the main port's answer. | 7 |
| `exec_framing` | An auction body of exactly `max_request_size` + 1 bytes. net/http hands over the last byte with `io.EOF`. `/__parity/exec` reads the body from a `bytes.Reader`, which does not, so the case would test the harness. | 1 |
| `request_line` | A request the envelope cannot carry as sent: an invalid method, target or header. | 0 |
| `truncated_body` | A body that the recorder cut at 1 MiB. | 0 |
| `not_rebuilt` | A replay step that cannot be rebuilt offline. | 0 |

The last run had 2495 cases and 64 requests not compared.

The runner reports 944 of those cases as not-yet-instrumented
(`exec cannot dispatch to <path>`). Its check matches a case's path exactly
against the routes in the health document, and those routes are httprouter
patterns. The 944 break down as follows:

* 293 are concrete paths of `/info/bidders/:bidderName` and
  `/static/*filepath`;
* 651 are paths that no route registers as written: unknown paths, trailing
  slashes, the admin-only paths, and the stored-request event API, which is
  off in the default profile.

They will be compared once the runner matches the patterns and dispatches
the other paths to the router, which answers them 404 or with a redirect.

### The ReplayHarness parity script

```sh
parity/scripts/replayharness.sh baseline|parity|selftest|canary|record-saved|cli \
    [--bin-b FILE] [--impl-b go|rust] [--profile NAME] [--out DIR] [-- PARITY-RUN-FLAGS...]
```

| Command | What it does |
| --- | --- |
| `baseline` | Starts side A (Go) and side B with `rhtool serve -servers 2`, both under one profile and with the mock oracle. Then it runs three steps: the probes on side A alone; `replayharness run parity/replayharness/api.yaml --target B --baseline A --fail-on warning --junit ... --json ...`; and `replayharness replay parity/replayharness/replays --target B`. `rhtool gate` judges the reports (see below). |
| `parity` | Runs `parity run --corpus replayharness`, with side B set by `--impl-b` and `--bin-b`. It fails on any mismatch or error, and it lists what the corpus leaves out. Flags after `--` go to `parity run`; with `-baseline`, the ratchet decides the result. |
| `selftest` | An A/A run: `parity selftest --corpus replayharness`, then `baseline` with Go on both sides. Neither may find a difference. |
| `canary` | Builds a Go side B with two seeded bugs, through a build overlay, so the checkout does not change. The first bug drops the `X-Prebid` header from `/openrtb2/auction`. The second renames `status` to `state` in `/info/bidders/{name}`. Both `parity` and `baseline` must fail. |
| `record-saved` | Writes `parity/replayharness/saved.jsonl` (see above). |
| `cli` | Builds the pinned ReplayHarness CLI and prints its path. |

The script builds the ReplayHarness CLI at commit
`f61ab1997f27b0044eeb801caea62810d75e2d76`. That is `rhreplay.PinnedCommit`,
and a corpus test checks that the two agree. The build works like this:

1. It clones `REPLAYHARNESS_SRC` (by default
   `https://github.com/aryehlev/ReplayHarness.git`) into a new temporary
   directory.
2. It checks out the pinned commit and builds it with `go build -trimpath`.
3. It checks the build: the Go build info must record that commit,
   unmodified.

Set `REPLAYHARNESS=/path/to/replayharness` to use a CLI that is already
built. The script warns when that CLI is not the pinned build.

The Go server, `rhtool`, the runner and the canary are built into the state
directory. The script writes nothing in the checkout except `saved.jsonl`,
and only `record-saved` writes that.

The builds run first, and they may fetch modules. After the builds, the
script guards every process:

* `HTTP_PROXY` and `HTTPS_PROXY` point at `127.0.0.1:9`; a server's proxy
  variables point at the oracle.
* A watchdog runs `lsof` every second on every process the script started,
  and on their children. If any of them has a socket off loopback, it kills
  them all.
* Every server and proxy is stopped on exit.

Exit codes:

| Code | Meaning |
| --- | --- |
| 0 | No difference. |
| 1 | A difference, an error or a failed replay. |
| 2 | A usage error. |
| 3 | Incomplete: nothing could be compared. |

### The baseline gate

With `--baseline`, `replayharness run` sends each probe to both sides. It
reports a `divergence` finding when the status or the JSON shape differs.
The shape is the set of members present and their JSON types, not their
values, and headers are not compared.

With `--fail-on warning`, ReplayHarness exits 1 whenever side B draws an
error or a warning. Go draws some on its own. In the last run, Go alone drew
465 findings on the 1199 probes, in 17 error, 21 warning and 20 info groups.
They include 500s on extreme input (`server-error`), statuses the map does
not list (`undocumented-status`), and invalid requests accepted or valid ones
rejected. So that exit status says nothing about parity. The script reports
it and leaves the verdict to `rhtool gate`, which compares side B's report
with side A's probe run:

* A divergence on a compared endpoint fails the run. So does a probe that
  side B does not answer.
* A finding that only one side draws on a probe fails the run. This check
  ignores `divergence`, `slow`, `timeout` and `connection` findings, which
  depend on the run, not on the server.
* On `POST /cookie_sync`, a shape divergence is printed with its reason and
  not counted. Go evaluates the syncers in an order it draws (see
  `map_order`), so two Go servers sync different bidders and `debug` comes
  and goes. That order changes the shape of the body, never the status, so
  everything else on that endpoint counts: a status divergence, a probe side
  B does not answer, and a finding only one side draws. At the pinned commit
  ReplayHarness reports a status divergence at severity `error` and a shape
  divergence at severity `warning` with the message `response shape differs
  from the baseline` (`internal/check/check.go`, func `baseline`). The gate
  leaves a divergence out only when both the severity and the message say
  it is a shape divergence.
* The run is incomplete (exit 3) if any of these holds:
  * the report has no baseline;
  * the run was interrupted or has no results;
  * the ReplayHarness versions differ;
  * a probe is missing from side A's run.

After the gate, every replay must pass on side B.

### Limits of the baseline

* The baseline sees status codes and JSON shapes only. A changed value, a
  missing header or a reordered member passes it. Byte-exactness comes from
  the parity runner, and the replays check the values they state.
* Probes go straight to the main port, outside a parity case. The oracle
  therefore answers bidder calls 404, and the only bids come from stored
  responses. Through `/__parity/exec`, a parity case's bidder calls carry
  the case label, and the oracle answers them from its templates.
* Neither tool compares the 64 requests listed as not compared.
* The parity runner does not yet run the 944 not-yet-instrumented cases. On
  their routes, only the baseline's probes and the replays compare anything.

### Results: A/A run and canary

The A/A run is `parity/scripts/replayharness.sh selftest`, with Go on both
sides. Two runs gave the same results:

* `parity selftest --corpus replayharness`: 2480 cases, of which 1536 passed,
  0 mismatched, 944 were not yet instrumented, and 0 errored.
* `baseline`, Go against Go:
  * 1199 probes: 0 divergences, 0 findings drawn by only one side, and 465
    findings drawn by both;
  * one or two `POST /cookie_sync` shape divergences, printed and not
    counted (the number changes from run to run, as `map_order` explains);
  * 162 of 162 replays passed.

The canary is `parity/scripts/replayharness.sh canary`. Both tools caught it:

* The parity run found 257 mismatching cases in one cluster: the `X-Prebid`
  header of `/openrtb2/auction`.
* The baseline found 4 divergences, on the shape of
  `/info/bidders/{bidderName}`.
* 4 replays failed: two on `X-Prebid` and two on `status`.

The two tools caught different bugs:

* The baseline missed the dropped header, because it does not compare
  headers.
* The parity runner missed the renamed member, because
  `/info/bidders/{bidderName}` is among the not-yet-instrumented cases.

So the final gate needs both.

### What the corpus does not exercise

A review listed what each handler reads (query parameters, headers, cookies
and body members) and checked which of them some case sends.
`replays/fields.yaml` covers the gaps it found first. These inputs are still
never sent:

* `POST /openrtb2/auction`: most of `ext.prebid`: `adservertargeting`,
  `aliasgvlids`, `alternatebiddercodes`, `analytics`, `bidadjustments`,
  `biddercontrols`, `bidderconfig`, `bidderparams`, `currency`, `data`,
  `events`, `experiment`, `floors`, `macros`, `multibid`, `nosale`,
  `schains`, `sdk`, `supportdeals` and `trace`. Also
  `imp[].ext.prebid.options`, `imp[].ext.prebid.floors`,
  `imp[].ext.prebid.imp` and `wlangb`.
* `GET /openrtb2/amp`: `True-Client-IP`, `Sec-Cookie-Deprecation` and
  `Sec-Browsing-Topics`. The auction cases send the first two.
* `POST /vtrack`: `puts[].key`.
* `GET /setuid`: `gdpr_consent`.
* `GET` and `POST /optout`: the `uids` cookie.

`POST /openrtb2/video` reads many inputs that no case sends, but Go answers
every video request in the traffic 501 (`The video endpoint is deprecated
and will be removed in 5.0.`).

The recorder leaves out `Cookie`, so the lines that the cookie replays left
in `traffic.jsonl` carry no cookie. The cases built from those replays' steps
do.

### Review: what the two tools catch

A review of the corpus, the script and the two gates checked isolation,
fidelity, determinism, and what a Rust-style bug looks like to each tool.

**Isolation.** Every server ran under a `sandbox-exec` profile that allows
network traffic on loopback only, and `lsof` checked every process four
times a second. One run sent requests meant to reach other hosts: bidders
that build their host from request parameters (`relevantdigital`'s
`pbsHost`, `acuityads`'s `host`), the cache, the vendor list through GDPR
consent, an event, a path traversal under `/static` and a redirect. No
process opened a socket off loopback, the sandbox denied no IP connection,
and the oracle answered every outbound call. The parity token appears only
in its 0600 file in the state directory: not in `traffic.jsonl`, the cases,
the reports, the server logs or any command line.

**Fidelity.** `TestReplayHarnessRoundTrip` sends requests as ReplayHarness
does, records them the way the pinned recorder does, loads the recording as
cases and dispatches them through `/__parity/exec`. The handler receives the
same method, request URI, escaped path, raw query, headers and body bytes
both times; only `Host` differs. The requests cover an odd raw query,
encoded path bytes, a gzip body, a body that is not UTF-8, empty and
non-empty POST, PUT and PATCH bodies, DELETE, OPTIONS, HEAD and a Range
request. Before the loader put back `Content-Length` and `Accept-Encoding`,
10 of the 12 requests reached the handler differently. ReplayHarness never
sends a header twice (it uses `Header.Set`), and the recorder joins repeated
values with `, `, so a case sends one header line. A recording has no header
order; a case sends its headers sorted by name, the same way to both sides.

**Determinism.** Three A/A runs of the runner, with `-shards 1`, `2` and
`4`, gave the same status and summary for every case. Only the order of some
cases' notes changed (14 and 15 cases): `parity/diff/view.go` builds them by
ranging over a map.

**Detection.** Each bug below was seeded into a Go side B with a build
overlay, as the canary is, and run through `parity` and `baseline`:

| Bug | Parity runner | Baseline probes | Replays |
| --- | --- | --- | --- |
| `/status` answers 200 instead of 204 | 10 cases (status) | 4 divergences | 1 failed |
| `/cookie_sync` writes `type` before `url` in `usersync` | 84 cases (member order) | nothing | nothing |
| Bidder requests always carry `Content-Type: application/json` | 99 cases | nothing | nothing |
| `Sec-GPC` is not forwarded to bidders | 1 case (the AMP one) | nothing | 1 failed |
| `/cookie_sync` answers 422 instead of 400 to a bad body | 95 cases (status) | 86 status divergences and 86 findings only side B draws, all counted | 2 failed |
| Responses are gzipped whatever `enable_gzip` says | 1551 cases (`Vary`; 55 also `Content-Encoding`) | nothing | nothing |
| `/info/bidders/{name}` renames `status` to `state` | nothing (not yet instrumented) | 4 divergences (the canary) | 2 failed (the canary) |

What the table shows:

* Neither tool catches every bug. The runner misses what happens on the
  not-yet-instrumented paths; the baseline misses headers, member order and
  values.
* The baseline's gate used to count nothing on `POST /cookie_sync`, not
  even a status divergence, which map order cannot cause: it printed 173
  differences for the 422 bug and counted none, so only the replays caught
  it. It now leaves out only the shape divergences there, and the same bug
  fails the gate with 86 status divergences and 86 `undocumented-status`
  findings only side B draws.
* The `Sec-GPC` bug reached one case only. `fields.yaml` now also checks
  `Sec-GPC` on an auction's bidder request.

**Provenance.** The `User-Agent` of the lines in `traffic.jsonl` names a
modified build of the pinned commit
(`ReplayHarness/0.1.1-0.20261116212744-f61ab1997f27+dirty`); `saved.jsonl`
came from the clean pinned build, which the script requires. The offline
rebuild test shows that the replay steps in the file are what the pinned
build sends; nothing checks the probes and the mutated replays. Record the
file again with the pinned build to remove the doubt.

### The Go-vs-Rust gate

Build the Rust server, then run from the repository root:

```sh
parity/scripts/replayharness.sh parity   --impl-b rust --bin-b rust/target/release/pbs-server
parity/scripts/replayharness.sh baseline --impl-b rust --bin-b rust/target/release/pbs-server
```

On this traffic, Rust can replace Go when both commands exit 0 and the
following holds:

* `parity`: no mismatch and no error. Before relying on the result, check
  the not-yet-instrumented count and the not-compared list; a Rust server
  without `/__parity/exec` passes no case and exits 3.
* `baseline`: no divergence, no finding drawn by only one side, and every
  replay passing on Rust.

`baseline` needs `--bin-b` for Rust. It needs only the main port, so it can
run before the Rust server implements `/__parity/*`. The replays state what
Go answers under the `default` profile, so run `baseline` under that profile;
`parity --profile NAME` compares the cases under another base profile.
