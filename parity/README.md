# Go-vs-Rust live differential parity harness

This directory holds the parity harness between the Go Prebid Server (the
source of truth) and its Rust port, the Cargo workspace of this repository.
Both servers run side by side in parity mode. A runner sends the same case to
both and compares everything they observed: the HTTP response, every internal
stage, every outbound call, every random decision, the analytics calls and the
Prometheus deltas. Any difference that the normalization allowlist does not
explicitly cover is a mismatch.

The harness is Go code of the upstream module, and it needs hooks in upstream
files. This repository never holds an upstream Go file, so the hooks are a
patch, `upstream-seams.patch`, and the harness compiles only in a tree that
`scripts/build-go.sh` assembles from the pinned upstream checkout, the patch
and this directory (see "The assembled tree"). Every Go command below runs in
that tree, or uses the binaries built from it.

This README is the contract the Rust side implements. The JSON Schemas under
[`schema/`](schema/) are normative; the Go types in package `parity` mirror
them, and `go test ./parity/schema/...` keeps the two in sync.

## Contents

| Path | What it is |
|---|---|
| `contracts.go`, `ids.go`, `envelope.go`, `trace.go`, `body.go`, `adapterio.go`, `errorcodes.go` | Go mirror of the contract (package `parity`, standard library only, safe to import from product code). |
| `upstream-seams.patch` | The seams: every change the harness makes to upstream files (the hook lines of "Go hook lines" and the harness files that live in upstream packages), as a patch on the commit `UPSTREAM_SYNC` pins (see "The upstream seams"). |
| `SELFTEST.md` | The A/A self-test runs at scale, and what each mismatch class they found turned out to be. |
| `schema/*.schema.json` | JSON Schemas (draft-07), `$id` prefix `https://prebid.org/pbs-parity/schema/v1/`. |
| `schema/{stages,decisions,normalization,fakes,error_codes}.json` | Registries: stage IDs, decision points, normalization allowlist, deterministic fakes, error codes. |
| `schema/examples/` | Hand-written example of every document; all of them validate. |
| `schema/` (Go) | Embeds the files above; `schema.Validate(doc)` validates any parity document. Harness-only. |
| `trace/` | Go trace session: the process-wide case lock, nil-safe recorders, decision points, error records, metric deltas. |
| `transport/` | Parity `http.RoundTripper`: oracle-proxy and inline modes, `X-Parity-Meta`. |
| `fakes/` | Deterministic clock, random and UUID sources for the 8 nondeterministic adapters and the user sync clock (`server_sites`), and `MapKeys`, which sorts map keys in parity mode only (`orders`). |
| `wiring/` | Bootstrap glue used by `main.go` and `router/router.go`; everything is a no-op unless `parity.enabled`. |
| `server/` | The Go server's `/__parity/*` routes: exec and its aliases, the adapter endpoints, config, bidders, reset and health. Linked into the binary by a blank import in `main.go`. |
| `endpointtrace/` | One-line endpoint-stage hooks for `/openrtb2/auction`, `/openrtb2/amp` and `/openrtb2/video`; its package doc lists each endpoint stage payload. |
| `shadow/` | Independent deep copies of a `RequestWrapper` for request snapshots, so hooks never rebuild the live wrapper. |
| `stagedata/` | Payload types of the exchange stages (`hooks.processed_auction` to `exchange.response`). |
| `corpus/` | Case loaders: adapter fixtures, sample requests, exchangetest, bidder params, stored data, seeds, the ReplayHarness requests, and the `go/ast` builder-config extractor. Harness-only. |
| `gen/` | The case generator (corpus `gen`, `--gen N --seed S`): schema-aware auction, AMP and video requests, and structured mutations of every corpus case, with case IDs that regenerate each case exactly. Harness-only. |
| `oracle/` | Mock oracle: loopback forward proxy, per-case plans, fixture replay, templates, content-addressed cache UUIDs, static currency and GVL. Harness-only. |
| `fixtures/currency/`, `fixtures/gvl/` | Static oracle data: `currency/latest.json` and the `gvl/v2`, `gvl/v3` vendor lists. |
| `fixtures/pools/` | The value pools `gen/` draws from: OpenRTB values, privacy signal strings, and the boundary, unicode and malformed-input catalogs. Harness-only. |
| `diff/` | The differ: normalization by the allowlist, semantic, lexical and wire comparison, attribution and clustering. Harness-only. |
| `testdata/diff/` | Golden differ cases: a Go trace, a Rust trace and the exact mismatches expected. |
| `runner/` | The differential runner: config profiles, server pairs, sharding, case execution, oracle capture merging, the A/A self-test and the adapter fixture self-test, and live minimization. Harness-only. |
| `report/` | Run artifacts: per-case summaries, `index.json`, `run.json`, `clusters.json`, `summary.md`, `junit.xml`, and the CI ratchet baseline. Harness-only. |
| `matrix/` | The domain coverage matrix (`domains.json` locks the 9 domains) and its checked-in snapshot `status.json` and `README.md`. Harness-only. |
| `minimize/` | ddmin minimizer over envelope bodies, headers, query and mock rules. Harness-only. |
| `cmd/parity/` | The CLI: `corpus`, `run`, `selftest`, `minimize`, `report`, `matrix`. |
| `fixtures/profiles/` | Base host configurations (`default`, `gdpr-enforced`, `floors`, `account-required`, `events-cache`). |
| `fixtures/stored/` | Stored requests, imps, responses and accounts copied into every run's stored-data tree. |
| `replayharness/api.yaml` | The ReplayHarness API map of the Go server (see "ReplayHarness"). |
| `replayharness/replays/` | Replays in the ReplayHarness format that pin what Go answers; each one's `why` cites the Go code. |
| `replayharness/profiles/` | Case profiles: per directory, a `profile.json` of host settings no base profile has and the replays that state what Go answers under it (see "Case profiles"). |
| `replayharness/stored/` | The stored data the replays use (IDs `rh-*`), in the `fixtures/stored` layout and loaded next to it. |
| `replayharness/traffic.jsonl` | The probes, replays and mutated requests ReplayHarness sent Go, with Go's answers, compacted by `rhtool traffic`. |
| `replayharness/saved.jsonl` | Go's answers to the replay steps that save a value, such as the `uids` cookie, for the steps that send it. |
| `replayharness/rhreplay/` | Reads replay files and recordings and rebuilds the requests ReplayHarness sends for them, byte for byte, at the pinned commit. Harness-only. |
| `replayharness/traffic/` | Compacts ReplayHarness recordings into `traffic.jsonl`. Harness-only. |
| `replayharness/rhgate/` | Judges a `replayharness run --baseline` report of side B against side A, the Go server. Harness-only. |
| `replayharness/rhtool/` | The command the scripts drive: `serve` boots the Go server as the runner does, `traffic` and `saved` keep what ReplayHarness recorded, `cases` lists what the corpus compares and leaves out, and `gate` judges a baseline run. Harness-only. |
| `scripts/` | `build-go.sh` (assembles the tree and builds the Go server, the runner and `rhtool` from it), `seams.sh` (prints, writes and checks `upstream-seams.patch`), `replayharness.sh` (the parity, A/A, baseline and canary runs on the ReplayHarness requests), `replayharness-replay.sh` (the probes and replays against Go, and the recording), `replayharness-context.sh` (checks the API map against the Go source), and the scripts' tests, a Go module of their own that stays out of the tree. |

## The assembled tree

The harness's packages are `github.com/prebid/prebid-server/v4/parity/...`,
inside the upstream module, and the server it instruments is the upstream
server with the seams applied. `scripts/build-go.sh` puts the three together
in one tree:

1. the pinned upstream checkout, the Go root: `$PBS_GO_ROOT` when it is set
   (a relative path is taken from the repository root), else `.upstream`,
   where `scripts/fetch-upstream.sh` checks out the commit `UPSTREAM_SYNC`
   pins. The script never writes to it: the tree is a git repository that
   borrows every object of the Go root (`objects/info/alternates`), checked
   out at the pinned commit, whatever the Go root has checked out. It is not
   a clone, because a clone of a shallow checkout, which `fetch-upstream.sh`
   makes, copies only what the checkout's `HEAD` reaches, and a refresh
   needs the blobs of the previous pin (see "When the pin moves");
2. the seams, `upstream-seams.patch`, applied with `git apply --index`. They
   must apply cleanly;
3. a copy of this directory, its tracked files and its untracked files that
   git does not ignore, without `scripts/`, `out/` and the patch.

From the repository root:

```sh
scripts/fetch-upstream.sh     # once: the pinned upstream checkout, in .upstream/
parity/scripts/build-go.sh    # the tree in parity/out/tree, the builds in parity/out/bin
```

The builds are `pbs-go`, the instrumented server, `parity`, the runner
(`cmd/parity`), and `rhtool` (`replayharness/rhtool`), built in the tree with
`GOWORK=off` and `-buildvcs=false`. `--no-build` assembles the tree only.
`--tree DIR` and `--bin DIR` put the tree and the builds elsewhere. A tree
inside this repository must be under `parity/out/`, which git ignores, so the
upstream code never reaches the lint scans. The script deletes and assembles
the tree on every run, and the last line it prints is the tree's path.

`.parity-tree`, at the top of the tree, names this repository, the upstream
commit and the seams the tree was assembled with. The harness reads the
corpora, `static/` and its own fixtures from the tree, and resolves its
outputs against the repository it names, the harness root
(`corpus.Repo.HarnessRoot`): runs go to `parity/out/<run-id>`, the matrix
snapshot to `parity/matrix`, the Go build to `parity/out/bin/pbs-go`, and the
Rust server is `target/release/pbs-server`. `corpus.FindRepo` finds the tree
from any directory inside it or inside this repository, so the runner works
from the repository root, and relative paths given to it (`--baseline
parity/baseline.json`) name this repository's files.

Edit the harness here, never in the tree: the tree's copy of this directory
is replaced on every run, so run `build-go.sh` again after an edit. The tree
also holds no `scripts/`: the scripts' tests are a Go module of their own
(`scripts/go.mod`) and run from that directory (see "Tests").

### The upstream seams

`upstream-seams.patch` holds every change the harness makes to upstream
files, in one form: `git diff` against the pinned commit with every option
that git config could change set explicitly, full blob IDs, no renames and
three lines of context, so the same changes always give the same bytes.
`scripts/seams.sh` keeps it in step with the tree:

```sh
parity/scripts/seams.sh diff  [--tree DIR]     # print the tree's seams in that form
parity/scripts/seams.sh write [--tree DIR]     # write them to parity/upstream-seams.patch
parity/scripts/seams.sh check [--patch FILE]   # the patch applies cleanly to the pin, in that form
```

To change a seam, edit the upstream file in the tree (`parity/out/tree`),
run `seams.sh write` and commit the patch. A new file in an upstream package
is a seam too: `seams.sh` stages every change but `parity/`, which the tree's
`.git/info/exclude` lists. `build-go.sh` refuses to replace a tree that holds
seam edits the patch lacks, so a run never discards them.
`scripts/seams_test.go` runs `seams.sh check`, checks that it refuses a patch
that does not apply and one that applies in another form, that
`build-go.sh` refuses the trees it must not replace, and that a tree
assembled from a shallow checkout sees an object of it that no ref reaches.

**When the pin moves.** A new `UPSTREAM_SYNC` can leave the patch unable to
apply, and `build-go.sh` then stops and points here. A patch that applies
may still have changed form: other line numbers, or other upstream blob IDs
in its `index` lines. `seams.sh check` reports that, and
`build-go.sh --no-build` followed by `seams.sh write` records the new form.
A patch that does not apply is refreshed with a three-way merge, from the
repository root:

1. Check that the Go root holds the old pin (`git -C .upstream cat-file -e
   OLD_PIN`): `--3way` reads the blobs the patch was made against from it,
   through the tree. `scripts/fetch-upstream.sh` keeps them when it moves a
   checkout to a new pin; in a fresh checkout, run
   `scripts/fetch-upstream.sh --sha OLD_PIN` first.
2. Update `UPSTREAM_SYNC` and run `scripts/fetch-upstream.sh`.
3. Run `parity/scripts/build-go.sh --3way`. It applies the patch with
   `git apply --3way`, lists the files whose seams conflict with the new
   upstream code, and builds nothing.
4. Resolve the conflict markers in those files in `parity/out/tree`, keeping
   what each seam does ("Go hook lines" says what that is), and adapt the
   harness in this directory to upstream changes the same way.
5. Run `parity/scripts/seams.sh write` and `parity/scripts/seams.sh check`.
6. Run `parity/scripts/build-go.sh`, the tests and the self-tests (see
   "Tests" and `SELFTEST.md`), and commit the patch with `UPSTREAM_SYNC`.

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
runs the original random draw unchanged, except at a site whose registry entry
has `outside_case`: in parity mode that site takes the `outside_case` value
when no case is in flight. `usersync.shuffle` has `outside_case: "identity"`
(`usersync/shuffler.go:30`), so a Go server in parity mode keeps the
chooser's order of the syncers on its main port too, and two of them answer
`POST /cookie_sync` alike. Outside parity mode it shuffles.

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

**Server sites.** The server itself reads `now_unix_ms` at the sites in the
`server_sites` member of `schema/fakes.json`: the `Expires` of the `uids`
Set-Cookie (`usersync/cookie.go:100`, and the size check at `:72`), the
expiry of each uid a sync writes (`:129`), whether a uid of the request's
cookie is live (`:174`, `:205`), and the `Expires` of the
`receive-cookie-deprecation` cookie that `/cookie_sync` sets
(`endpoints/cookie_sync.go:584`). Parity mode installs the sources for the
whole process, so these sites read the parity clock on the main port as
well as in a case: in the default profile a `uids` Set-Cookie always ends
`Expires=Thu, 18 Nov 2021 15:28:50 GMT` (90 days), and a uid synced in a
case or on the main port expires `2021-09-03T15:28:50.823Z`.

The uid's `expires` is Go's `time.Time` JSON: `time.RFC3339Nano` in UTC,
with the fraction of the second cut after its last nonzero digit and left
out for a whole second. A uid is live while the clock is strictly before
its `expires`, so one that expires at the clock's instant is not. Liveness
decides the syncers that `/cookie_sync` skips as already synced
(`usersync/chooser.go:190`), its `ok` or `no_cookie` status
(`endpoints/cookie_sync.go:513`) and the cookie label of the auction
metrics. It does not decide the buyer uids: the exchange gives a bidder the
cookie's uid whether it is live or not (`exchange/utils.go:780`), and the
host cookie sync looks only at whether a uid is there
(`usersync/cookie.go:137`). `usersync.yaml` sends `/cookie_sync` and the
auction uids that expire between the parity clock and the wall clock, at
the clock's instant and a millisecond after it.

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
imposes the orders in the `orders` member of `schema/fakes.json`. Name
order is byte order throughout (Go's `cmp.Less[string]`), so `appStockSSP`
comes before `appnexus`:

* The bidders of one auction build their requests one at a time, in bidder
  name order (`exchange/parity_turns.go`). A bidder's bidder-request hook
  stage and `MakeRequests` start after `MakeRequests` of every bidder before
  it has returned. HTTP calls and `MakeBids` stay concurrent. The order
  matters because per-bidder requests share objects: gumgum copies `Site` but
  writes the publisher ID through the shared `Publisher`
  (`adapters/gumgum/gumgum.go:45-46`). Whether another bidder sends the
  original ID or gumgum's then depends on scheduling. Name order is one of
  the schedules Go can take.
* Cookie sync lists the syncers of the known bidders in bidder name order
  (`usersync/chooser.go:23`), the order `usersync.shuffle` then keeps or
  shuffles. Go ranges over the bidder-to-syncer map there, so without this
  two processes chose different syncers to report under a limit, and a
  different `debug` list.
* The `user.data` entries that `Sec-Browsing-Topics` adds come in ascending
  `segtax`, then `segclass` order, and the segment IDs of each entry, also
  those merged into an entry the request has, in ascending order
  (`privacysandbox/topics.go:140`, `:142`, `:155` and `:220`). Go groups the
  header's topics in maps and ranges over them, so two processes sent the
  bidders the same segments in different orders.
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

These lines are the seams: they live in `upstream-seams.patch`, and the
paths are those of the upstream files the patch changes (see "The upstream
seams").

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
| `usersync/shuffler.go` | `usersync.shuffle`, the identity in parity mode with no case in flight |
| `usersync/chooser.go` | the bidder-to-syncer loop ranges over `fakes.MapKeys(...)` |
| `usersync/cookie.go` | `time.Now()` replaced by `fakes.Now()` in `PrepareCookieForWrite`, `WriteCookie`, `Sync`, `GetUID` and `HasAnyLiveSyncs` |
| `privacysandbox/topics.go` | the segtax, segclass and segment ID loops of `UpdateUserDataWithTopics` and `findNewSegIDs` range over `fakes.MapKeys(...)` |
| `endpoints/cookie_sync.go` | the endpoint's clock is `fakes.Active()` in parity mode, for the `receive-cookie-deprecation` cookie's `Expires` |
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
  `Transfer-Encoding` is rejected. The body returns `io.EOF` together with
  its last bytes and fails a read after `Close` with
  `http.ErrBodyReadAfterClose`, as the `net/http` server's body does
  (`parity/server/request.go`). A handler that reads one byte past a limit,
  as the auction's `max_request_size` check does, therefore sees what it
  sees on the main port.
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

From the repository root, after `parity/scripts/build-go.sh` (see "The
assembled tree"):

```sh
parity/out/bin/parity selftest --corpus sample-requests,adapters,exchangetest
parity/out/bin/parity run --impl-b rust --bin-b target/release/pbs-server --baseline parity/baseline.json
parity/out/bin/parity run --corpus all --gen 5000 --seed 42 --shards 4
parity/out/bin/parity minimize --run parity/out/<run-id>
parity/out/bin/parity report --run parity/out/<run-id> --baseline parity/baseline.json
parity/out/bin/parity matrix --run parity/out/<run-id>
```

`--impl-a` and `--impl-b` name the sides (`go` or `rust`). `--bin-a` and
`--bin-b` give their binaries. Without `--bin-*`, a Go side is built from the
tree into `parity/out/bin/pbs-go`. A Rust side uses `target/release/pbs-server`
when it exists. Otherwise the Rust side is not run, and every case is
not-yet-instrumented. `--repo` names the tree, or a directory inside it or
inside this repository; by default the runner finds it from the working
directory.

Exit codes:

| Code | Meaning |
|---|---|
| 0 | Success. |
| 1 | `selftest`, or `run` without `--baseline`, found a mismatch, a harness error or a failing fixture; or the ratchet regressed. |
| 2 | A usage or harness error. |
| 3 | `selftest`, or `run` without `--baseline`, was incomplete: nothing could be compared. |

`run` without `--baseline` judges its run as `selftest` does, and prints
each reason after `run:`. With `--baseline`, the ratchet alone decides (see
"Minimization and the ratchet"): a run whose mismatches the baseline already
counts exits 0, and one that compares fewer cases exits 1.

### Batches and processes

The runner loads every corpus first and refuses to run, with exit code 2,
when any of them has a load error. The loaders skip a file they cannot read
so that the rest still loads, and one invalid replay file drops every
`replayharness` case. A run that went on would compare fewer cases than the
corpus holds: it could pass, or end with "nothing was compared", without
naming the file. The error lists every load error.

1. The runner selects cases by `--corpus`, `--kinds`, `--match` and `--limit`.
   * `--corpus` takes a comma-separated list of `adapters`,
     `sample-requests`, `exchangetest`, `bidder-params`, `replayharness`,
     `introspection`, `gen` and `all` (the default).
   * `--gen N --seed S` first adds N cases drawn by `parity/gen` (corpus
     `gen`). Their IDs regenerate them exactly. A `--corpus` list that names
     neither `all` nor `gen` gets `gen` added.
   * Every case runs under every `--profiles` base profile. A case ID gets the
     suffix `@<profile>` when the base profile is not `default`.
   * Adapter cases run once, under the first base profile.
   * `contract_gap` cases are skipped.
   * The requests a corpus leaves out of its cases (`corpus.NotCompared`;
     only the `replayharness` corpus has any) are reported under every base
     profile with status `not-compared`, the same `@<profile>` suffix, and
     the reason `<tag>: <reason>`. `--corpus`, `--kinds` and `--match` select
     them as they select cases; `--limit` does not apply, because none of
     them runs.
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
   `PBS_CONFIG_FILE` and `PBS_STATIC_DIR`. `TMPDIR` is the server's own
   directory, its log path with the extension `.tmp`
   (`ServerSpec.TempDir`): it is created empty at the start and deleted
   when the server stops. Go's glog writes a file per log level to the
   temporary directory, every INFO line included. With the runner's own
   `TMPDIR` those files stayed behind after every run, outside the run
   directory, about 0.8 MB per server. `rhtool serve` starts its servers
   the same way, so they now go with the scripts' state directory.
5. `--shards N` starts N pairs of servers per batch on free loopback ports.
   Each pair runs its share of the cases one at a time, in the same order on
   both sides, and calls `/__parity/reset` before every envelope. The cases
   of a batch run in case ID order, and a shard takes every Nth of them.
   `--shuffle SEED` (not 0) runs each batch's cases in an order drawn from
   SEED instead, the same order for the same seed and cases. A case whose
   result depends on the cases run before it on the same servers then shows
   up as a difference between two runs.
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

The health document's `endpoints` member lists every route of the main
router as httprouter patterns; an empty list stands for every route. The Go
server lists the stored-request event API's route first when
`stored_requests` `cache_events` is on (`parity/server/introspect.go`). The
runner matches a case's path against these routes as httprouter v1.3.0 does:
`:name` matches up to the next slash and `*name` the rest
(`parity/runner/routes.go`).

* A path that a route of each side matches is dispatched.
* A path that only one side's routes match is not yet instrumented, and the
  reason names the route.
* A path that no route matches is dispatched when both sides list the same
  routes, because the router itself answers it: 404, 405 with `Allow`, a
  redirect or `OPTIONS`.

### Run artifacts

`parity/out/<run-id>/` contains the following. The runner writes
`parity/out/.gitignore`.

| File | Contents |
|---|---|
| `index.json` | `pbs-parity-index/1`: every case with its status, its first divergence and the case directory. |
| `run.json` | Counts by status (`counts`: `total`, `passing`, `mismatch`, `not_yet_instrumented`, `error`, `skipped`, `not_compared`), options, and the server records: binary, health, reason and log for each side and shard. |
| `clusters.json` | Mismatch clusters by signature, with their representative case and repro paths. |
| `summary.md` | Counts, reasons cases were not compared, every reason of the requests not compared, cluster tables by domain, and failing fixtures. |
| `junit.xml` | One suite per corpus plus `fixture-selftest`. Mismatch is a failure, error is an error, and not-yet-instrumented, skipped or not-compared is skipped, with the message `<status>: <reason>`. |
| `fixtures.json` | Adapter fixture self-test results, when any adapter case ran: passing, failing, excused (`needs_fake`, `unresolved_build`) or not-run. |
| `matrix.json`, `matrix.md` | The coverage matrix of this run. |
| `cases/<sha1-16>/` | `summary.json` for every case and every request not compared. A mismatch or error also gets `case.json` (everything needed to rerun the case), `envelope.json`, both traces, `diff.json`, and `min.*.json` after minimization. |
| `servers/*.log` | Server stdout and stderr. |

`run.json` (`pbs-parity-run/1`) and each `summary.json`
(`pbs-parity-case-summary/1`) are runner documents without a JSON Schema.
A summary's `status` is one of these, and `reason` explains every status
but the first two:

| Status | Meaning | Compared | `index.json` entry |
|---|---|---|---|
| `passing` | Both sides were compared and nothing differed. | yes | yes |
| `mismatch` | Both sides were compared and a mismatch was recorded. | yes | yes |
| `not-yet-instrumented` | A side does not serve what the case needs, such as the endpoint or the mock level. | no | yes |
| `error` | The harness could not complete the case. | no | yes |
| `skipped` | Selected but not run: a `contract_gap` case, or the budget ran out. | no | no |
| `not-compared` | A request the corpus leaves out of its cases; the reason is `<tag>: <reason>` (see "What the corpus does not compare"). It never runs. | no | no |

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
4. Read `envelope.decisions` at the seven decision sites, and take a site's
   `outside_case` value when no case is in flight. Inject the `fakes.json`
   values into the 8 adapters, and read `now_unix_ms` at the `server_sites`
   on the main port as well as in a case. Generate case UUIDs from the
   `generated_uuids` stream. Impose the `orders`.
5. List every route of the main router in the health document's
   `endpoints`, as httprouter patterns.
6. Route every outbound call through an oracle-proxy or inline transport with
   identical kinds, classification and `X-Parity-Meta`.
7. Produce Go's error codes (`error_codes.json`) and Prometheus names, labels
   and deltas.
8. Record the `http_response` header names with the case and in the order
   the main listener writes them. The runner compares them as wire classes
   (`header_case`, `header_order`), and it sees only what the recorder
   reports: no gate reads the main port's raw response bytes. The baseline
   does not compare headers, and the replays read them through Go's HTTP
   client, which canonicalizes names and keeps no order. A server whose
   recorder reports `Content-Type` while its listener writes `content-type`
   (hyper's default for HTTP/1.1) would pass every gate.

## Tests

The harness's tests run in the tree, from its top:

```sh
parity/scripts/build-go.sh --no-build   # assemble parity/out/tree
cd parity/out/tree
go test ./parity/...                 # schemas, examples, registries, contract conformance, /__parity/* routes
go test -race ./parity/trace/... ./parity/transport/... ./parity/diff/...
go test -race ./parity/runner/... ./parity/minimize/... ./parity/report/... ./parity/matrix/...
go test ./config ./router ./server ./exchange
go test ./usersync ./endpoints ./privacysandbox   # the user sync and topics seams in parity mode
```

Set `GOWORK=off` when a `go.work` above the repository could reach the
tree. Tests that run upstream code in process leave the glog files of their
test binaries in `TMPDIR`, so give them a `TMPDIR` of their own.

The scripts' tests run in `parity/scripts`, a Go module of its own. From the
repository root:

```sh
(cd parity/scripts && go test ./...)   # seams.sh check, build-go.sh's guards and tree, the scripts' state directories
```

Package `parity` checks that the Contents table at the top of this file
names every directory of this repository's `parity/` down to two levels and
every file at its top level, and that every path the table names exists. It
reads the `parity/` of the harness root, so `scripts/` and the patch, which
the tree leaves out, count.

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
encodes a `case.json` for every case of the tree's corpus. Its route
matcher must agree with httprouter's own lookup on the Go server's routes and
on every path of the `replayharness` corpus
(`TestRouteMatchIsHTTPRouterLookup`).

`parity/schema` checks the following:

* every schema compiles;
* every example validates and round-trips through its Go type;
* stages, decisions, outbound kinds, mismatch kinds and classes match the Go
  constants;
* every normalization rule cites a Go source;
* `error_codes.json` matches `errortypes`;
* `fakes.json` matches `fakes.Defaults()`, each of its `orders` names a Go
  file that holds the seam, and each of its `server_sites` names Go lines
  that hold it;
* only `usersync.shuffle` has an `outside_case` value, and it is one of its
  values;
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
* a body around a `LimitedReader` limit reaches a handler through exec as it
  does through a real `net/http` server (`TestExecBodyEndsLikeTheWire`);
* the health document lists every route of the main router, with the
  stored-request event routes when `cache_events` is on;
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
that traffic. The `replayharness` corpus turns those requests into parity
cases (see "ReplayHarness requests as parity cases").

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

Two parity hazards were found while mapping. Parity mode now takes both
out, on the main port as well as in a case (see "Deterministic fakes" and
"Decision points"):

* the `uids` Set-Cookie depended on the wall clock; it now reads the parity
  clock;
* `/cookie_sync` without `bidders` returned the syncers in map order, and
  outside a parity case, which is how ReplayHarness requests arrive, Go
  shuffled them at random, so with a limit two runs could sync different
  bidders. The chooser now lists them in bidder name order and the shuffle
  keeps that order.

`/__parity/*` is left out on purpose: it is the harness, not the product.

Send ReplayHarness requests only to a server that the parity runner started.
Such a server runs in parity mode on 127.0.0.1, and the mock oracle answers
every outbound call. Never pass `--allow-remote` to ReplayHarness.

### Checking the map

```sh
REPLAYHARNESS=/path/to/replayharness parity/scripts/replayharness-context.sh [report.md]
```

The script reads the Go source of the server the harness instruments, and
sends no requests:

1. It assembles a tree in a temporary directory outside the repository
   (`build-go.sh --no-build`): the pinned upstream checkout with the seams
   and `parity/`.
2. It builds a scan root next to it. The root holds every non-test Go file of
   the tree that could register a route, without `parity/`. Full-line `//`
   comments are blanked, with line numbers kept, so documentation examples
   such as `stored_requests/events/api/api.go:22-23` are not taken for
   routes.
3. It copies the map into that root as `.replayharness/api.yaml`.
4. It runs `replayharness context` on the root. The script fails when the
   report has a "Spec drift" section, that is, when the code registers a route
   the map misses or the map lists a route with no handler.
5. It checks that every `file:line` the map cites is inside an existing file
   of the tree.

It exits 0 when there is no drift and every citation is valid, and 1
otherwise. The optional argument keeps the full context report. After a route
or handler changes, update `api.yaml` and run the script again.

### Running ReplayHarness against Go

```sh
REPLAYHARNESS=/path/to/replayharness parity/scripts/replayharness-replay.sh \
    [--baseline] [--record] [--serve] [--profile NAME] [--case-profile NAME] \
    [--out DIR] [--keep-state]
```

The script assembles a tree in its state directory and builds the Go server
and `rhtool` from it (`build-go.sh --tree --bin`). It starts the Go server
with `rhtool serve` (`parity/replayharness/rhtool`), which uses the runner's
own steps:

* `runner.LoadProfiles` and `BuildConfig` with the base profile (`default`
  unless `--profile` names another), the case profile of
  `parity/replayharness/profiles/NAME` when `--case-profile` names one
  (`rhtool serve -case-profile`), and the oracle's URL;
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
   the run, on `POST /cookie_sync` as on every other endpoint: in parity mode
   Go lists the syncers in bidder name order and keeps that order on its
   main port, so two Go servers answer it alike.
4. **`--record`.** `replayharness record` on a spare loopback port, the probes
   and the replays through it, then `replayharness replay <recording>
   --mutate` through the same proxy. `rhtool traffic` then writes
   `parity/replayharness/traffic.jsonl` (see below).

`--serve` only starts the stack, prints the URLs and waits for Ctrl-C.

With `--case-profile NAME`, the server also takes
`parity/replayharness/profiles/NAME/profile.json`, and only the replays of
`parity/replayharness/profiles/NAME/replays` run: no probes, and
`--baseline` and `--record` are refused, since the probes run under no case
profile and a case profile's replays are not recorded (see "Case
profiles").

The state directory holds the token file, `rhtool serve`'s `state.env`,
outbound log and server logs, the raw recording, and the builds: the tree and
the binaries built from it. The script deletes it on every exit: on success,
on failure, and on `INT`, `TERM` or `HUP`. With `--keep-state`, it keeps the
directory without the builds and prints its path. The reports go to `--out`,
or to a new temporary directory outside the state directory, which is kept.
The script never prints the token. The probes, the replays and the case
profiles are read from this repository's `parity/replayharness`, and
`--record` writes `traffic.jsonl` there.

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
| stored requests | `rh-amp-banner`, `rh-amp-stored-bid`, `rh-amp-two-imps`, `rh-amp-app`, `rh-amp-topics` (for `rh-topics-account`), `rh-events-request`, `rh-video-pods` (two ad pods, for the `video` case profile), `rh-uuid-request` (id `{{UUID}}`, so it is merged over the body) |
| stored imps | `rh-imp-stored-bid`, `rh-imp-video-stored-bid`, `rh-imp-video-pod` (legacy `imp.ext.appnexus` params and a stored bid response), `rh-imp-video-live` (an appnexus video imp with no stored bid) |
| stored responses | `rh-appnexus-banner`, `rh-appnexus-video` and `rh-appnexus-deal` (appnexus wire bids for stored bid responses, the last with a deal ID), `rh-seatbid-banner` (a seatbid for stored auction responses) |
| accounts | `rh-events-account` (events on), `rh-no-debug-account` (`debug_allow` false), `rh-ccpa-account` (CCPA enforced), `rh-cookie-sync-account` (limit 1), `rh-sandbox-account` (cookie deprecation on), `rh-topics-account` (a topics domain and cookie deprecation), `rh-malformed-account` (`disabled: "yes"`, which does not parse) |

### The replays

`parity/replayharness/replays` holds 204 replays in the ReplayHarness format
(`agents/replayharness-replay.md` in the ReplayHarness repository). Each one
states what Go does, and its `why` cites the Go code.

| File | Replays | Covers |
| --- | --- | --- |
| `auction.yaml` | 25 | validation errors, debug and test, aliases, disabled bidders, accounts, currency, Content-Encoding |
| `stored.yaml` | 16 | stored requests, imps, bid responses and auction responses, targeting, currency conversion, bid adjustment, a `{{UUID}}` stored request merged over the body |
| `amp.yaml` | 16 | tag_id, query overrides, consent types, origin headers, True-Client-IP, Sec-Cookie-Deprecation and Sec-Browsing-Topics, the targeting response |
| `video.yaml` | 4 | the deprecated endpoint, video validation, ad pod keys |
| `usersync.yaml` | 22 | setuid to getuids to cookie_sync with the saved cookie, cookie attributes and the Chrome versions that add `SameSite=None`, opt-out, filters, limits, privacy, `/optout` with a `uids` cookie, uid liveness at and around the parity clock, and the buyer uid of a uid that is no longer live |
| `events.yaml` | 14 | `/event` parameters and accounts, `/vtrack` validation, cache puts and put keys |
| `privacy.yaml` | 10 | GDPR, CCPA and GPP in the auction |
| `headers.yaml` | 11 | Referer, User-Agent, X-Forwarded-For, X-Real-IP, DNT, Sec-GPC, Sec-Browsing-Topics, Origin |
| `json.yaml` | 25 | malformed and borderline JSON bodies |
| `limits.yaml` | 6 | bodies at, one byte over and two bytes over `max_request_size` |
| `info.yaml` | 12 | `/info/bidders`, `/bidders/params`, `/status`, `/version`, `/` and `/static` |
| `router.yaml` | 8 | 404, 405, redirects, OPTIONS, the CORS preflight, admin-only routes |
| `prebid.yaml` | 23 | the `ext.prebid` and `imp[].ext.prebid` options no other replay sent, chosen for what they change in the bidder request or the response: `bidderparams`, `schains`, `bidderconfig`, `data` and `eidpermissions`, `currency`, `floors`, `adservertargeting`, `alternatebiddercodes`, `nosale`, `bidadjustments`, `biddercontrols`, `aliasgvlids`, `events`, `sdk` (with `analytics`, `macros` and `experiment`), `multibid`, `supportdeals`, `trace`, the imp's `options`, `floors` and `imp`, `wlangb`, Sec-Browsing-Topics on the auction |
| `fields.yaml` | 12 | inputs nothing else sent: the `uids` cookie in the auction and AMP, Sec-GPC on the bidder request, True-Client-IP, X-Real-IP and Referer in AMP, Sec-Cookie-Deprecation, DOOH, the OpenRTB fields Go passes through, `passthrough`, `is_rewarded_inventory`, `adunitcode`, a client `ext.prebid.server` |

Things to know when editing them:

* ReplayHarness reads `{{name}}` as a variable. A body that must contain
  `{{UUID}}` uses JSON escapes, and a check splits the text.
* `contains` compares raw bytes. Go escapes `&` in some JSON as `&`; in a
  double-quoted YAML string, write the backslash as `\x5C`.
* In a YAML flow mapping, a key with `[` must be quoted:
  `{"$.debug[0].error": ...}`.
* ReplayHarness treats an empty response header as missing.
* The `uids` cookie's `Expires` and uid expiries come from the parity clock
  (see "Deterministic fakes"), so a replay may state a whole Set-Cookie, as
  the `host-cookie` case profile's do. A flow saves `header:Set-Cookie` and
  sends it back as `Cookie`; Go reads `uids=` and skips the attributes.
* A bidder request shows in the response only as the escaped JSON string
  `ext.debug.httpcalls.<bidder>[0].requestbody`, next to
  `ext.debug.resolvedrequest`, which holds the request before the bidder
  copies. A `not_contains` meant for the bidder request therefore names the
  escaped form, such as `'\"source\":\"rh.test\"'`.

### Case profiles

Some handlers' real paths need host config that no base profile sets. A
directory of `parity/replayharness/profiles` holds a case profile:
`profile.json` is a `corpus.Profile` (`corpus.ReadReplayHarnessProfile`
rejects an unknown member, an empty profile, and `currency_rates` and
`default_request`, which are not config settings), and `replays/` holds
replays that state what Go answers under it.

| Profile | Settings | Replays | Covers |
| --- | --- | --- | --- |
| `host-cookie` | `host_cookie`: cookie `rh_host` for the syncer key `adnxs`, opt-out cookie `rh_optout=1`, domain `parity.test`, 30 day TTL | 5 | the host cookie in `/getuids`, `/setuid` (with the whole Set-Cookie), `/cookie_sync`, the auction and AMP; the host opt-out cookie |
| `status-response` | `status_response: ok` | 1 | `GET /status` answering 200 with the body |
| `video` | `video.enable_deprecated_endpoint` | 4 | `/openrtb2/video` past the 501: stored request merge, ad pods and pod errors, validation and account errors, the refusal of stored bid responses |

They reach the code three ways:

* The corpus builds a case from every step, under the case profile
  (`Case.Profile`), and the runner runs those cases on servers started with
  it, as it does the cases of any other profile. The case ID hashes the
  profile directory's name with the request, so a request sent under two
  profiles is two cases. A step that sends a saved value is not rebuilt:
  `saved.jsonl` holds the default profile's answers.
* `replayharness.sh baseline` runs each profile's replays on a side B
  started with it (`rhtool serve -case-profile`).
* `replayharness-replay.sh --case-profile NAME` runs them against Go.

The profiles' replays are not recorded: their cases come from the steps,
rebuilt as the default replays' are, whose fidelity to what ReplayHarness
sends `TestReplayHarnessRebuildsWhatReplayHarnessSent` checks.
`stored_requests` `cache_events` has no case profile. Without an in-memory
cache nothing reads its event API's updates, so a `POST` never returns
(`stored_requests/config/config.go:77-81`,
`stored_requests/events/api/api.go:52`). With one, an update stays in the
cache for the rest of the process, so one case would change what the cases
after it see, and `/__parity/reset` does not clear it.

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
Set-Cookie and Authorization. The last full run read 4797 exchanges and kept
2569 on 278 endpoints, in 4264664 bytes. It dropped 2194 repeated requests,
17 request bodies over 64 KiB and 17 repeated large responses.

`rhtool traffic` also takes several recordings and keeps the first exchange
of each request, so a recording of new replays alone can be added to the
file. The `{{UUID}}` stored request replay of `stored.yaml` was added that
way: its two steps and their `--mutate` requests (156 exchanges) were
recorded by the clean pinned build through `replayharness record` in front
of a `replayharness-replay.sh --serve` server, as `--record` records them,
and compacted together with the file. The 2569 lines stayed as they were,
first, and 131 new ones follow them: 4594137 bytes in all, after 23
repeated requests and 2 request bodies over 64 KiB were dropped. The
`setuid` Chrome version replay of `usersync.yaml` was added after them the
same way: of its 31 exchanges, 12 repeated a request and 19 follow the 2700
lines, which now come to 2719 lines and 4614652 bytes. `--mutate` sends its
own `User-Agent`, so only the 5 steps send the browser ones.

The recording comes from the unmodified pinned build: every request that
ReplayHarness itself sent carries the `User-Agent`
`ReplayHarness/0.1.1-0.20261116212744-f61ab1997f27` (`rhreplay.PinnedVersion`),
and `TestReplayHarnessRecordingsComeFromThePinnedBuild` fails on any other
ReplayHarness build, `+dirty` included, in `traffic.jsonl` or `saved.jsonl`.
Four requests carry a browser `User-Agent` that their replay step sets.

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
* **Video** (the `video` case profile). Errors are `text/plain` with status
  500, 403 for a disabled account, and every message joined after one
  space each, so the text after `Critical error while running the video
  endpoint:` starts with two spaces. A pod whose `configid` names no stored
  imp is reported in `adPods` with `"targeting":null`, and the request fails
  only when every pod does. A video imp cannot use a stored bid response:
  the endpoint validates the request without them.
* **`/vtrack` put keys.** A put's `key` becomes the cache key and is written
  into the cache request without escaping, so a key with a quote makes the
  cache answer 400 and `/vtrack` 500; an empty key is left out, and the cache
  makes one up.
* **Privacy sandbox.** For an account with a topics domain,
  `Sec-Browsing-Topics` fills `user.data` with one entry per taxonomy and
  model version (`segtax` 599 plus the taxonomy version, `segclass` the
  model version; `privacysandbox/topics.go:90`).
  In parity mode the entries and their segment IDs come in ascending order
  (see "Deterministic fakes"); outside it, in map order. Every request that
  sends the header is answered `Observe-Browsing-Topics: ?1`. A topic set it
  cannot read is warning 10012, shown only with debug. A `Sec-Cookie-Deprecation`
  label over 100 characters is warning 10011 and is dropped. AMP reads both
  headers, and `True-Client-IP`, as the auction does.
* **Host cookie** (the `host-cookie` case profile). A host cookie fills the
  host syncer's uid in `/getuids`, `/setuid` and the auctions' buyer uid
  unless the `uids` cookie already has one. With the host opt-out cookie,
  `/setuid` and `/cookie_sync` answer 401 `User has opted out`, and the
  auctions send no buyer uid.
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
compares both sides on those requests. It is part of `--corpus all`.

`parity/replayharness` is one of the approved corpus roots
(`corpus.ApprovedRoots`), and the corpus reads it through `Repo.FS`, so every
read goes through `Repo.Resolve`, as every other loader's does. A file whose
symlink leaves the approved roots is refused with `ErrOutsideCorpus`, not
read.

The corpus takes its requests from three sources:

* every exchange in `traffic.jsonl`;
* every step of the replays in `replays/`. Package
  `parity/replayharness/rhreplay` rebuilds each step offline the way the
  pinned ReplayHarness sends it. That covers the query encoding, `json`
  bodies written by `json.Marshal`, `{{name}}` values, and the `User-Agent`
  of the build that recorded `traffic.jsonl`. As a result, a step and its line
  in the recording are the same request;
* every step of the replays of each case profile in `profiles/`, rebuilt the
  same way, as cases of that profile (see "Case profiles").

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
  request; for a case profile's step, of the profile directory's name, a
  newline and the request's SHA-256 in hex. The ID depends on nothing but
  the request and its profile, so it stays the same across runs and file
  orders, and changes when the request changes.
* The envelope:
  * uses the `default` profile, or for a case profile's step that profile's
    name (`Profile.Name`, such as `video-3d9ce2ee`; the reports show it
    after the base profile, as `default+video-3d9ce2ee`), plus the exec
    defaults for the host and the remote address (127.0.0.1);
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

The saved `uids` cookie was recorded before the cookie code read the parity
clock, so its uid expires `2026-12-01T12:00:11.992254Z`, 14 days after the
recording. A parity-mode server judges a uid live by the parity clock
(2021-08-20), so the uid stays live in every run. A `record-saved` run now
records the expiry `2021-09-03T15:28:50.823Z`, which is live as well.

### What the corpus does not compare

Some requests cannot be compared: two correct servers may answer them
differently, or the envelope cannot carry them as sent. Such a request is not
a case. `rhtool cases` lists each one with a tag and a reason, and the
`parity` and `selftest` commands write that list to `not-compared.json`.
The corpus drops nothing recorded or replayed without an entry in that list.
Each entry has the ID, corpus, kind, fixture path and endpoint its case
would have. `corpus.LoadAll` keeps the list in `Corpus.NotCompared`, and the
runner reports each entry with status `not-compared` and the reason
`<tag>: <reason>`: `run.json` counts them in `counts.not_compared`,
`summary.md` lists every reason under "Requests not compared", `junit.xml`
has a skipped test case for each, the matrix counts them per endpoint, and
each gets a `summary.json`, but no `index.json` entry.

| Tag | Requests | Last run |
| --- | --- | --- |
| `side_port` | An API map endpoint served only on the admin or Prometheus port. `/__parity/exec` dispatches to the main router only. The requests to these paths are still cases, and they check the main port's answer. | 7 |
| `request_line` | A request the envelope cannot carry as sent: an invalid method, target or header. | 0 |
| `truncated_body` | A body that the recorder cut at 1 MiB. | 0 |
| `not_rebuilt` | A replay step that cannot be rebuilt offline. | 0 |

Three tags are gone, with the reasons for them:

* `wall_clock` (51 requests): `/setuid` and `/cookie_sync` answers whose
  cookie `Expires` or uid expiry came from the wall clock. They read the
  parity clock now (see "Deterministic fakes").
* `map_order` (5): `/cookie_sync` without a bidder list, or with `coopSync`,
  which evaluated the syncers in map order. In parity mode the chooser lists
  them in bidder name order now.
* `exec_framing` (1): the auction body of exactly `max_request_size` + 1
  bytes. net/http hands over the last byte with `io.EOF`, and
  `/__parity/exec` now ends a body the same way
  (`parity/server/request.go`).

The last run had 2767 cases and 7 requests not compared. 2743 cases run
under the `default` profile, and 24 under the case profiles: 11 under
`host-cookie`, 3 under `status-response` and 10 under `video`.

No case is not yet instrumented. The runner used to report 944 of them
(`exec cannot dispatch to <path>`): it matched a case's path exactly against
the routes in the health document, which are httprouter patterns, so it
left out every concrete path of `/info/bidders/:bidderName` and
`/static/*filepath` and every path no route registers. It now matches them
as httprouter does (see "Batches and processes").

### The ReplayHarness parity script

```sh
parity/scripts/replayharness.sh baseline|parity|selftest|canary|record-saved|cli \
    [--bin-b FILE] [--impl-b go|rust] [--profile NAME] [--out DIR] [--keep-state] \
    [-- PARITY-RUN-FLAGS...]
```

| Command | What it does |
| --- | --- |
| `baseline` | Starts side A (Go) and side B with `rhtool serve -servers 2`, both under one profile and with the mock oracle. Then it runs three steps: the probes on side A alone; `replayharness run parity/replayharness/api.yaml --target B --baseline A --fail-on warning --junit ... --json ...`; and `replayharness replay parity/replayharness/replays --target B`. `rhtool gate` judges the reports (see below). Last, for each case profile, it starts both sides again with `rhtool serve -case-profile` and runs that profile's replays on side B (`replays-NAME.txt` and `.junit.xml`). |
| `parity` | Runs `parity run --corpus replayharness`, with side B set by `--impl-b` and `--bin-b`, and lists what the corpus leaves out. The runner's exit code is the result: it fails on any mismatch or error. Flags after `--` go to `parity run`; with `-baseline`, the ratchet decides the result. |
| `selftest` | An A/A run: `parity selftest --corpus replayharness`, then `baseline` with Go on both sides. Neither may find a difference. |
| `canary` | Builds a Go side B with two seeded bugs from the tree, through a build overlay, so the tree does not change. The first bug drops the `X-Prebid` header from `/openrtb2/auction`. The second renames `status` to `state` in `/info/bidders/{name}`. Both `parity` and `baseline` must fail. |
| `record-saved` | Writes `parity/replayharness/saved.jsonl` (see above). |
| `cli` | Builds the pinned ReplayHarness CLI into the reports directory and prints its path. |

The script builds the ReplayHarness CLI at commit
`f61ab1997f27b0044eeb801caea62810d75e2d76`. That is `rhreplay.PinnedCommit`,
and a corpus test checks that the two agree. The build works like this:

1. It clones `REPLAYHARNESS_SRC` (by default
   `https://github.com/aryehlev/ReplayHarness.git`) into the state directory
   (see below).
2. It checks out the pinned commit and builds it with `go build -trimpath`.
3. It checks the build: the Go build info must record that commit,
   unmodified.

Set `REPLAYHARNESS=/path/to/replayharness` to use a CLI that is already
built. The script warns when that CLI is not the pinned build.

The script assembles a tree in the state directory, a new temporary
directory, and builds the Go server, `rhtool` and the runner from it
(`build-go.sh --tree --bin`); the canary and the ReplayHarness CLI are built
there too. The state directory also holds the parity token file and, for
each stack `rhtool serve` starts, the `state.env`, the outbound log and the
server logs. The script deletes it on every exit: on success, on failure,
and on `INT`, `TERM` or `HUP`. With `--keep-state`, it keeps the directory
without the builds and prints its path. The reports go to `--out`, or to a
new temporary directory outside the state directory, which is kept. The
script never prints the token. `parity/scripts/state_test.go` checks this
with stubs for `go` and the CLI: for this script on success (`cli`), on a
failed build, on `TERM` and with `--keep-state`, and for
`replayharness-replay.sh` in the last three cases. The probes, replays and
case profiles are read from this repository's `parity/replayharness`. The
script writes no tracked file except `saved.jsonl`, and only `record-saved`
writes that.

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
* `POST /cookie_sync` counts like every other endpoint. The gate used to
  print its shape divergences without counting them, because two Go servers
  evaluated the syncers in different orders and synced different bidders. A
  parity-mode Go server lists them in bidder name order and keeps that order
  on its main port, so two of them answer alike.
* The run is incomplete (exit 3) if any of these holds:
  * the report has no baseline;
  * the run was interrupted or has no results;
  * the ReplayHarness versions differ;
  * a probe is missing from side A's run.

After the gate, every replay must pass on side B, and every case profile's
replays must pass on a side B started with that profile.

### Limits of the baseline

* The baseline sees status codes and JSON shapes only. A changed value, a
  missing header or a reordered member passes it. Byte-exactness comes from
  the parity runner, and the replays check the values they state.
* Probes go straight to the main port, outside a parity case. The oracle
  therefore answers bidder calls 404, and the only bids come from stored
  responses. Through `/__parity/exec`, a parity case's bidder calls carry
  the case label, and the oracle answers them from its templates.
* Neither tool compares the 7 requests listed as not compared.
* The probes run under no case profile: on the handlers that only a case
  profile turns on, only the parity runner and that profile's replays
  compare anything.

### Results: A/A run and canary

The A/A run is `parity/scripts/replayharness.sh selftest`, with Go on both
sides. The last run, on the current corpus, gave:

* `parity selftest --corpus replayharness`: 2767 cases, all passed: 0
  mismatched, 0 were not yet instrumented, and 0 errored. The 7 requests
  not compared are reported next to them, so `run.json` counts 2774
  entries.
* `baseline`, Go against Go:
  * 1199 probes: 0 divergences, 0 findings drawn by only one side, and 465
    findings drawn by both;
  * 204 of 204 replays passed;
  * every case profile's replays passed on a side B started with it: 5 of
    5 for `host-cookie`, 1 of 1 for `status-response` and 4 of 4 for
    `video`.

The canary is `parity/scripts/replayharness.sh canary`. Both tools caught it:

* The parity run found 468 mismatching cases:
  * 430 auction cases, in one cluster: the `X-Prebid` header of
    `/openrtb2/auction`;
  * 38 cases of `/info/bidders/{bidderName}`, where `$.status` is missing
    and `$.state` is new. The 37 answers for one bidder make two clusters,
    one per member, and the answer of `/info/bidders/ALL` makes two per
    bidder (794), since each bidder's member has its own path.
* The baseline found 4 divergences, on the shape of
  `/info/bidders/{bidderName}`.
* 4 replays failed: two on `X-Prebid` and two on `status`. The case
  profiles' replays passed.

The baseline missed the dropped header, because it does not compare
headers. The parity runner used to miss the renamed member, when
`/info/bidders/{bidderName}` was among its not-yet-instrumented cases; it
now catches both bugs.

### What the corpus does not exercise

A review listed what each handler reads (query parameters, headers, cookies
and body members) and checked which of them some case sends.
`replays/fields.yaml` covered the gaps it found first. The inputs it still
listed are now sent too:

| Input | Sent by |
| --- | --- |
| `POST /openrtb2/auction`: `ext.prebid` `adservertargeting`, `aliasgvlids`, `alternatebiddercodes`, `analytics`, `bidadjustments`, `biddercontrols`, `bidderconfig`, `bidderparams`, `currency`, `data`, `events`, `experiment`, `floors`, `macros`, `multibid`, `nosale`, `schains`, `sdk`, `supportdeals` and `trace`; `imp[].ext.prebid.options`, `.floors` and `.imp`; `wlangb` | `replays/prebid.yaml` |
| `GET /openrtb2/amp`: `True-Client-IP`, `Sec-Cookie-Deprecation` and `Sec-Browsing-Topics` | `amp.yaml`, "AMP reads True-Client-IP, Sec-Cookie-Deprecation and Sec-Browsing-Topics like the auction" |
| `POST /vtrack`: `puts[].key` | `events.yaml`, "a vtrack put key becomes the cache key, written into the put unescaped" |
| `GET /setuid`: `gdpr_consent` | `usersync.yaml`, "setuid accepts GDPR consent strings it cannot parse"; `traffic.jsonl` holds 7 such requests, which the review missed |
| `GET` and `POST /optout`: the `uids` cookie | `usersync.yaml`, "optout reads no cookie before the captcha passes" |
| `POST /openrtb2/video`, past the 501 | the `video` case profile |
| `status_response` and `host_cookie` | the `status-response` and `host-cookie` case profiles |

These paths are still not reached:

* **The `/setuid` host vendor check.** The parity profiles leave
  `gdpr.host_vendor_id` unset, so `/setuid` accepts any consent string
  without asking the TCF permissions whether the host may set a cookie
  (`endpoints/setuid.go:406-427`).
* **`/optout` past the captcha.** Go asks Google's siteverify whether the
  captcha passed, and the oracle answers that call 404, so every `/optout`
  with a `g-recaptcha-response` answers 401 before it reads the cookie
  (`pbs/usersync.go:71-90`). Reaching the cookie code needs an oracle answer
  for siteverify.
* **`stored_requests` `cache_events`.** It has no case profile (see "Case
  profiles").
* **Video bids from a bidder call, and `includebrandcategory`.** On the main
  port the oracle answers bidder calls 404, so the video replays see bids
  from stored data only. With `includebrandcategory`, Go's ad pod targeting
  is not deterministic: `findTargetingByKey` ranges over the targeting map
  and returns the first key whose part from the first `_` on starts the key
  it looks for, and for `hb_pb_cat_dur` that is `_pb_cat_dur_<seat>`, which
  `hb_pb` matches as well as `hb_pb_cat_dur_<seat>` does
  (`endpoints/openrtb2/video_auction.go:594-595`, `:643-652`). Two Go runs
  can answer differently, so no replay or case uses it (see "Known gaps" in
  `SELFTEST.md`).

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
the reports, the server logs or any command line. The scripts used to leave
that directory behind, with the token file in it; they now delete it on
every exit unless `--keep-state` is given.

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

**Determinism.** On the corpus of the time (2613 cases), two A/A runs of
`replayharness.sh selftest`, each with its own `-out`, wrote byte-identical
`clusters.json`, `not-compared.json` and case `diff.json` files. Their
`index.json`, `matrix.json` and 2620 case summaries differ only in times
and in the paths of the output directory. Their baselines drew the same 465
findings on both sides. Before the topics seam, the second of two such runs
had found one case whose `user.data` segments Go ordered by map iteration
(`SELFTEST.md`, S8).

On the earlier recording, three A/A runs of the runner, with `-shards 1`,
`2` and `4`, gave the same status and summary for every case. Only the order
of some cases' notes changed (14 and 15 cases): `parity/diff/view.go` built
them by ranging over a map. It now visits the stages in name order and each
stage's entries in key order (`TestNotesFollowStageAndKeyOrder`). Three new
runs, each with the same `-out` and `-run-id`, wrote byte-identical
`index.json`, `clusters.json` and case directories; in the 2495 case
summaries only `duration_ms` differs. Each run had 1551 cases passing, 944
not yet instrumented, no mismatch and no error. In 24 of those cases the
notes name two or more stage entries of a side, so their order came from the
map. `run.json`, `summary.md`, `matrix.*` and `junit.xml` differ only in
times, the shard count and the list of servers, one pair per shard.

**Detection.** Each bug below was seeded into a Go side B with a build
overlay, as the canary is, on the tree of the time, and run through
`parity` and `baseline` on the corpus of the time (2613 cases). The Replays
column counts the failing
replays of `replays/`; every case profile's replays passed for every bug.

| Bug | Parity runner | Baseline probes | Replays |
| --- | --- | --- | --- |
| `/status` answers 200 instead of 204 | 10 cases (status) | 4 status divergences | 1 failed |
| `/cookie_sync` writes `type` before `url` in `usersync` | 91 cases (member order), one of them under `host-cookie` | nothing | nothing |
| Bidder requests always carry `Content-Type: application/json` | 125 cases: 112 auctions, 12 AMP requests and 1 video request under `video` | nothing | nothing |
| `Sec-GPC` is not forwarded to bidders | 2 cases (an AMP request and an auction) | nothing | 2 failed |
| `/cookie_sync` answers 422 instead of 400 to a bad body | 95 cases (status) | 86 status divergences and 86 findings only side B draws | 2 failed |
| Responses are gzipped whatever `enable_gzip` says | all 2613 cases (`Vary`; 80 also `Content-Encoding`) | nothing | nothing |
| The same, but only for requests that send `Accept-Encoding` | 2609 cases (`Vary`; 80 also `Content-Encoding`) | nothing | nothing |
| `/info/bidders/{name}` renames `status` to `state` | 38 cases (the canary's renamed member) | 4 shape divergences (the canary) | 2 failed (the canary) |

What the table shows:

* The parity runner catches all eight bugs. It missed the renamed member
  while `/info/bidders/{bidderName}` was among its not-yet-instrumented
  cases; it now runs those cases. The baseline's probes catch the three
  bugs that change a status or a response's shape, and miss headers, member
  order and values. The replays catch four. No bug is caught by the
  baseline or the replays alone. The baseline is still the check that runs
  on the server's own listener rather than through `/__parity/exec`, and
  the one a server without `/__parity/*` can pass.
* The cases left out of the second gzip bug are the four that send no
  `Accept-Encoding`: three `HEAD` requests and a `Range` request, which
  Go's HTTP client sends without it.
* The baseline's gate used to count nothing on `POST /cookie_sync`, not even
  a status divergence: it printed 173 differences for the 422 bug and
  counted none, so only the replays caught it. It then left out only the
  shape divergences there; it now counts everything (see "The baseline
  gate"). The same bug fails it with 86 status divergences and 86
  `undocumented-status` findings only side B draws.
* The `Sec-GPC` bug reached one case, the AMP one, before `fields.yaml`
  also checked `Sec-GPC` on an auction's bidder request. It now reaches two
  cases and fails two replays.

**Provenance.** The `User-Agent` of the lines in `traffic.jsonl` named a
modified build of the pinned commit
(`ReplayHarness/0.1.1-0.20261116212744-f61ab1997f27+dirty`), and nothing
checked that its probes and mutated replays were what the pinned build
sends. `saved.jsonl` came from the clean pinned build. The file was not
edited: `replayharness-replay.sh --record` recorded it again with the clean
pinned build, under the same compaction rules. The new recording holds the
same 2535 requests, identical but for that `User-Agent`, in another order.
Go answered every one with the same status. The answers differ only in the
`Date` header, the `Last-Modified` header of static files, timestamps,
random transaction IDs, the `User-Agent` that debug output echoes, and the
syncers that `POST /cookie_sync` lists in map order (`map_order`). The ID
of every case that sends the recorder's `User-Agent` changed with it, since
an ID hashes the whole request. The corpus then still had 2495 cases and 64
requests not compared, with the same tags.

The file was recorded again, with the same clean pinned build and the same
compaction rules, once `prebid.yaml` and the new replays of `amp.yaml`,
`events.yaml` and `usersync.yaml` existed. It holds every request of the
previous recording, unchanged, and 34 more: the steps of `prebid.yaml` (26
auction requests), of the AMP privacy sandbox replay (4) and of the
`/vtrack` put key replay (4). The `/optout` replay's requests differ from
earlier ones only in their cookie, which the recorder leaves out, so they
are lines the file already had. All 200 replays passed on Go during the
recording, and the two Go servers showed no divergence on the probes. The
recorded answers to the topics requests predate the topics seam; the corpus
uses only the requests.

### Review: fifteen more bugs, run order, the seams and isolation

A second review tried to break the claim that zero Go-vs-Go mismatches
make every Go-vs-Rust mismatch a real bug, and every real bug a mismatch.

**Seeded bugs.** Fifteen more Rust-style bugs were each built into a Go
side B with a build overlay, so nothing in the tree changed. httprouter
lives in the module cache, which an overlay cannot replace, so the 308 bug
was built with a copy of `go.mod` that replaces httprouter with a patched
copy. Each side B ran through `parity run --corpus replayharness -shards 3`
on the corpus before this review (2613 cases), and again once the review's
replays were in it (2748 cases). The last replay, the Chrome versions of
`/setuid`, then added 19 cases, all of them requests to `/setuid`, so every
side B also ran the 138 cases whose ID names `setuid` (`--match setuid`).
The After column counts the final corpus (2767 cases): the full run's count
plus the new cases that the second run caught. Each older case had the same
outcome in both runs. The two Chrome bugs, which nothing but `/setuid`
reaches, ran only that way, and their Before column counts the older cases:

| Bug | Before | After |
| --- | --- | --- |
| AMP sets `AMP-Access-Control-Allow-Source-Origin` with that spelling, not `Amp-Access-...` | 195 cases (`header_case`) | 195 cases |
| `/info/bidders/{name}` writes `capabilities` before `maintainer` | 29 cases (`member_order`) | 29 cases |
| The `uids` cookie with `SameSite=None` loses `Secure` | 1 case: the Chrome `User-Agent` `/setuid` | 4 cases |
| `/setuid` adds `SameSite=None` only above Chrome 67, not from 67 on | 0 cases | 2 cases: Chrome 67 and CriOS 67 |
| `/setuid` never looks for `CriOS/` | 0 cases | 1 case: CriOS 67 |
| The router's 404 body is `Not Found\n`, not `404 page not found\n` | 570 cases | 570 cases |
| A trailing-slash redirect of a request other than GET is 308, not 307 | 28 cases | 28 cases |
| No trailing-slash redirect at all | 37 cases | 37 cases |
| An auction body of exactly `max_request_size` bytes is too large | 2 cases | 2 cases |
| The chooser lists the syncers in descending name order | 5 cases (`/cookie_sync`) | 5 cases |
| A synced uid expires after 15 days, not 14 | 15 cases (`/setuid`) | 25 cases |
| appnexus sends `Content-Type: application/json; charset=utf-8`, with a space | 125 cases: 112 auctions, 12 AMP requests, 1 video request | 126 cases |
| With a `{{UUID}}` stored request the body wins over it, and the generated id is set last | **0 cases** | 98 cases |
| The stored request wins over the body | 119 cases | 119 cases |
| A uid's liveness reads the wall clock | 1 case: the saved-cookie `/cookie_sync` | 3 cases |

No server opened a socket off loopback in any of these runs.

What the review changed:

* **The `{{UUID}}` branch.** No stored request had the id `{{UUID}}`, so no
  case reached the branch of `processStoredRequests` that merges the stored
  request, with a generated id, over the body
  (`endpoints/openrtb2/auction.go:1736-1750`). That is the reverse of every
  other stored request, and a side B that merged it the usual way passed
  every case. `rh-uuid-request` and its replay in `stored.yaml` now reach
  it, and the 129 `--mutate` variants of the replay's two steps that were
  recorded with them (see "The recorded traffic") send it more bodies.
* **uid liveness.** Only the saved-cookie flow sent a uid that the two
  clocks judge differently, so one case stood between a server that reads
  the wall clock and a pass. `usersync.yaml` now sends uids that expire in
  2024, at the parity clock's instant and a millisecond after it.
* **The Chrome check.** One request sent a browser `User-Agent`,
  `Chrome/120`. So one case stood between a pass and a server that drops
  `Secure`, and none reached the version boundary or the `CriOS/` branch of
  `siteCookieCheck` (`endpoints/setuid.go:378-404`). A replay in
  `usersync.yaml` now sends Chrome 66 and 67, CriOS 67, a version with no
  dot, and one that is not a number followed by a `CriOS/90`, which Go
  never reads once it has found `Chrome/`.
* **The contract.** `fakes.json` said that uid liveness decides the buyer
  uids of the auction and the host cookie sync. It decides neither: the
  exchange gives a bidder the cookie's uid whether it is live or not, and a
  Rust server that dropped expired uids would have differed from Go. The
  entry now names every reader, says that `time.Before` is strict, and gives
  the exact JSON form of a uid's `expires`; a replay pins the buyer uid of a
  uid that is no longer live. `usersync_bidders` now says byte order, as
  Go's string comparison is: `appStockSSP` comes before `appnexus`.
* **A corpus that does not load.** While these replays were being written,
  one of them did not parse, and the loader dropped every replayharness
  case. The runner ran the rest, which was nothing: `run.json` counted 0
  cases, the exit code was 3, and no message named the file. A run of every
  corpus without `--baseline` would have passed. The runner now refuses a
  corpus with load errors (see "Batches and processes").
* **The script's report.** Under the lost `Secure`, the Chrome version
  replay failed, and its report printed the PNG that `/setuid` answers.
  grep then took the report for a binary file, so the console showed
  `Binary file ... matches` where the failing replays and their count
  belong. The verdict, which comes from the CLI's exit status, was right.
  Both scripts now print the report as text, without its control bytes.

**The baseline.** `replayharness.sh baseline` ran with every side B as
well, on the final corpus. Its 1199 probes caught one bug, the stored
request that wins over the body (25 divergences, and 19 findings only side
B draws). The replays caught eleven. The two Chrome bugs and the lost
`Secure` each fail the Chrome version replay. The 404 body, the 308 and the
missing redirect fail one, one and two router replays, and the size limit
fails two. The uid TTL fails one replay of `host-cookie`. Each stored
request bug fails one stored request replay, and the wall clock fails two
replays and one of `host-cookie`. The AMP header spelling, the member
order, the syncer order and the bidder request's `Content-Type` fail no
probe and no replay. The replays read header names through Go's client,
find members by path, count the syncers and state no header of a bidder
request, so only the parity runner catches those four.

**What no tool reads.** The runner compares `http_response` as the server
reports it through `/__parity/exec`, and no gate reads the main port's raw
response bytes: the baseline does not compare headers, and the replays read
them through Go's HTTP client, which canonicalizes names. The spelling bug
above was caught because Go's recorder reports header names as the handler
set them. A Rust recorder must report what its listener writes, case and
order included (item 8 of "Implementing the contract in Rust").

**Run order.** `--shuffle` (see "Batches and processes") was added for
this. Three A/A runs of `parity selftest --corpus replayharness` on the
2748-case corpus, with `-shards 1` in case ID order, `-shards 3 -shuffle 7`
and `-shards 4 -shuffle 20261118`, each passed all 2748 cases, with no
error. Their 2755 case summaries and diffs are identical once times and the
run directory's path are taken out. On the final corpus, the script's
selftest (see "Results: A/A run and canary"), in case ID order with one
pair per batch, and a run with `-shards 2 -shuffle 4242` each passed all
2767 cases, and their 2774 case summaries and diffs are identical in the
same way. So no case's result depends on the cases run before it on the
same servers, or on how many pairs share a batch.

**The seams outside parity mode.** The default build (`go build .`) was
started twice from one `pbs.yaml`, once without `parity` and once with
`parity.enabled`, and sent the requests these seams decide. Without parity
it wrote the uid's `expires` from the wall clock in the local zone
(`2026-12-02T03:19:55.920915+02:00`) and the `uids` cookie's `Expires` 90
days after the wall clock, synced six different sets of three bidders in six
`/cookie_sync` requests with `limit` 3, synced appnexus again for a uid that
expired in 2024, and listed the topics segments in four different orders in
four auctions. In parity mode it wrote `2021-09-03T15:28:50.823Z` and
`Expires=Thu, 18 Nov 2021 15:28:50 GMT`, synced `152media 1accord 33across`
every time, answered `Already in sync` for the 2024 uid, and gave the same
segment order four times. `fakes.MapKeys` sorts only when the fakes are
installed, `fakes.Now` is `time.Now` and the shuffle shuffles until then, and
`wiring.Bootstrap` installs them only when `parity.enabled` is true.

**Isolation.** Every server and tool ran under the loopback-only sandbox
and the `lsof` check, and no process opened a socket off loopback. The parity
token of each of the four `parity selftest` runs above, read from a server's
environment while it ran, is in no file of its run directory. The servers'
glog files used to land in the runner's `TMPDIR` and stay there: after the
first runs of this review the INFO logs of 169 servers alone held 142 MB,
each with the config dump, in which the token reads `<REDACTED>`. They now
go to each server's own temporary directory, which is deleted when it
stops. After the A/A runs, and after the script's selftest and canary,
which also delete their state directory, the `TMPDIR` they ran with was
empty.

### The Go-vs-Rust gate

Build the Rust server, then run from the repository root:

```sh
parity/scripts/replayharness.sh parity   --impl-b rust --bin-b target/release/pbs-server
parity/scripts/replayharness.sh baseline --impl-b rust --bin-b target/release/pbs-server
```

On this traffic, Rust can replace Go when both commands exit 0 and the
following holds:

* `parity`: no mismatch and no error. Before relying on the result, check
  the not-yet-instrumented count and the not-compared list; a Rust server
  without `/__parity/exec` passes no case and exits 3.
* `baseline`: no divergence, no finding drawn by only one side, and every
  replay passing on Rust, the case profiles' replays included.

`baseline` needs `--bin-b` for Rust. It needs only the main port, so it can
run before the Rust server implements `/__parity/*`. The replays in
`replays/` state what Go answers under the `default` profile, and each case
profile's replays what it answers with that profile on top, so run
`baseline` under the `default` profile; `parity --profile NAME` compares the
cases under another base profile. The case profiles start the Rust server
with `video.enable_deprecated_endpoint`, `status_response` or `host_cookie`
in its `pbs.yaml`, so it must read them as Go does.
