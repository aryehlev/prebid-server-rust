# A/A self-test at scale

The A/A self-test runs the Go server against a second process of itself, so
it should show no differences at all. Every mismatch it reports is one of two
things:

* a harness bug, which is fixed in the harness; or
* genuine Go nondeterminism, which gets either a parity-mode seam that
  imposes one of the orders Go can take, or a narrow normalization rule that
  cites the Go source.

This file records the runs that drove the A/A mismatch count to zero on
branch `ph/selftest`, and what each mismatch class turned out to be.

## Result

The last four runs used the final binaries: four seeds, 54,847 cases in all,
21,000 of them generated. Every one finished with **0 mismatches and 0
errors**. The adapter fixture self-test passed 3,537 fixtures, failed none
and excused 159 in every run.

| Run | Seed | `--gen` | Cases | Passing | Mismatch | Error | Skipped |
|---|---|---|---|---|---|---|---|
| `aa-final-seed7` | 7 | 5000 | 13,467 | 13,456 | 0 | 0 | 11 |
| `aa-final-seed42` | 42 | 3000 | 11,446 | 11,435 | 0 | 0 | 11 |
| `aa-final-seed31337` | 31337 | 8000 | 16,486 | 16,475 | 0 | 0 | 11 |
| `aa-seed2026c` | 2026 | 5000 | 13,448 | 13,437 | 0 | 0 | 11 |

The 11 skipped cases are tagged `contract_gap`: the parity schemas do not yet
accept their faithful encoding. The runner never runs them, in A/A or in
Go-vs-Rust. After `ph/break` was merged there are 14 (see "After merging
`ph/break`" below).

**Disabled mode.** The full upstream suite, `go test ./...`, passes on this
branch: 387 packages are `ok` and none fail. Without `parity.enabled` the
seams below do what the original code did. `fakes.MapKeys` returns the keys
in map order, `parityTurns` returns the context unchanged, and `inCallOrder`
returns the channel unchanged.

## Reproducing

```sh
go build -o parity/out/bin/pbs-go .
go build -o parity/out/bin/parity ./parity/cmd/parity
./parity/out/bin/parity selftest --corpus all --gen 5000 --seed 7 --shards 8 \
  --bin-a parity/out/bin/pbs-go --run-id aa-final-seed7
```

`selftest` exits 0 only when there are no mismatches, no harness errors and
no failing fixtures. The reports go to `parity/out/<run-id>/`:

* `summary.md` and `clusters.json` hold the clusters;
* `cases/<id>/` holds both traces and `diff.json` for each mismatch.

## Run history

The rows are in the order the runs were made. Each run was made after the
fixes for the run before it.

| Run | Cases | Mismatch | What it surfaced |
|---|---|---|---|
| `aa-base1` (no `--gen`) | 8,709 | 620 | 608 gzip `Content-Length` (H1), 8 gojsonschema message orders (H7), 2 mediasquare raw requests (H1), 1 `adapter_requests` metric (H2), 1 stored auction response imp (R3). 1 failing fixture (H3). |
| `aa-uuid1` (lifted and params cases) | 4,305 | 1 | beachfront gojsonschema order with quoted patterns (H7) |
| `aa-scale1` (seed 42, gen 3000) | 11,446 | 37 | event `ts` in the hook stages (R5); map-ordered imp groups in adtelligent and datablocks (S5); flipp and adgeneration response order (S2); `bidder.result` http calls and non-bids (R1); invibes imp IDs (R2); matterfullrtb call keys in `bids.adjusted` (H6); response `cur` (R4); bluesea params order (H7); telaria short tmax (H5) |
| `aa-scale2` | 11,446 | 1 | `connections_closed` (H4) |
| `aa-scale3` | 11,446 | 0 | none |
| `aa-seed7` (seed 7, gen 5000) | 13,467 | 4 | gumgum shared publisher write (S1); first party data error order (S3) |
| `aa-seed7b` | 13,467 | 4 | AMP targeting merge (R6); definemedia first failing bidder (S4); bidmatic imp groups (S5) |
| `aa-seed7c` | 13,467 | 1 | `connections_closed` again (H4) |
| `aa-seed7d` | 13,467 | 1 | first invalid alias (S4) |
| `aa-seed7e` | 13,467 | 0 | none |
| `aa-scale4` | 11,446 | 0 | none |
| `aa-seed2026` (seed 2026, gen 5000) | 13,448 | 1 | invibes imp IDs echoed in `adapter.bids` (R2) |
| `aa-seed2026b` | 13,448 | 2 | ownadx http calls in the hook stages (R1); tied auction winner (S6) |
| `aa-seed2026c` and the final runs | see above | 0 | none |

### After merging `ph/break`

`ph/break` turned schema violations into harness errors and made the
rate-driven floors decisions honour the envelope only where the rate leaves
the outcome to chance. On `parity-harness`, after both merges, two classes
appeared and were fixed there:

| Run | Cases | Mismatch | Error | What it surfaced |
|---|---|---|---|---|
| `aa-h4-seed7` (seed 7, gen 5000) | 13,467 | 0 | 3 | Go's own bids output fails `TypedBid.type` for mabidder `simple-app-banner` (`""`) and richaudience `bidTypeNotAssigned` (`"no bidtype assigned"`, two runs). These adapter cases are now tagged `contract_gap`, like the untyped exchangetest bids. |
| `aa-h4-seed7b` | 13,467 | 1 | 0 | Floor rejection warnings in seat bid map order (`floors/enforce.go:117`) in the AMP response's `ortb2.ext`, now that floors are enforced at enforcerate 100. `warnings_errors_order` and the other response-ext rules now also anchor at `ortb2.ext`. |
| `aa-h4-seed7c` | 13,467 | 0 | 0 | none (14 skipped `contract_gap`; 3,534 fixtures passing, 159 excused) |
| `aa-h4-seed42` (seed 42, gen 3000) | 11,446 | 0 | 0 | none |

## Harness bugs

**H1. Random UUIDs inside gzip bodies.**
*Seen:* 608 cases differed in a bidder request's `Content-Length`.
*Cause:* Go draws `source.tid`, `imp[].ext.tid` and other v4 UUIDs at random.
UUID placeholders can number them in JSON, but they cannot reach inside a
gzip-compressed body, and random input changes the compressed length. Wire
differences gate parity and are never normalized.
*Fix:* in parity mode every `uuid.NewV4` draws from a deterministic stream
(`parity/fakes/uuids.go`, `fakes.json` `generated_uuids`). The stream
restarts at the start and end of every case. The same fix covers the two
mediasquare cases: Go leaves `regs.ext` empty there (`"ext":}`), so the raw
request is not JSON, and its tids could not get placeholders.

**H2. Metrics recorded after the response.**
*Seen:* `adapter_requests` was missing on one side for tappx.
*Cause:* a bidder goroutine sends its result to the auction, and only then
runs its deferred `RecordAdapterRequest` (`exchange/exchange.go:808-810` and
`:865`). The closing metrics snapshot could be taken before that deferred
call ran.
*Fix:* the bidder goroutines register with `trace.Session.Track`. The exec
handler calls `Settle` before it takes the closing snapshot. The wait is
bounded at 5s, and the trace carries a note when the bound is hit
(`parity/trace/settle.go`, `parity/server/exec.go`).

**H3. Adapter fixture decoding.**
*Seen:* one fixture failed its own expectations:
`adapters/mediasquare/mediasquaretest/supplemental/no-valid-response.json`
expected `imp-bidder[ext]: is empty` and got `imp[ext]: is empty`.
*Cause:* the parity adapter endpoint decoded the fixture request with
`jsonutil`, but `adapterstest` decodes with `encoding/json`
(`adapters/adapterstest/test_json.go:112`). The two treat a member spelled
`null` differently.
*Fix:* `decodeBidRequest` uses `encoding/json` (`parity/server/adapter.go`).

**H4. Listener connection metrics.**
*Seen:* `connections_closed` moved by 1 on one side, in about one case in
12,000.
*Cause:* `/__parity/exec` dispatches the case in process, so a case never
opens or closes a main-listener connection. These series count only the
runner's own connections to the main port:

* they are opened on Accept and closed by the idle timeout
  (`server/listener.go:24-52`);
* Go sets `ReadTimeout` and no `IdleTimeout` (`server/server.go:103`).

The runner's keep-alive pool could dial a second connection, which then
idled out while another case was running. That made the miss more likely,
but limiting the pool did not remove it (`aa-seed7c`).

*Fix:* the runner client holds at most one connection per server
(`parity/runner/client.go`). The `listener_connection_metrics` rule ignores
`connections_opened`, `connections_closed` and `connections_error` in
`metrics_delta`. Exec cannot compare these series at all.

**H5. Timing-dependent generated tmax.**
*Seen:* telaria reached `adapter.bids` on one side only.
*Cause:* the `tmax.edge` mutation drew deadlines of a few milliseconds.
Whether the bidder answered in time then depended on scheduling.
*Fix:* the generator pool draws no tmax between 1 and 249 ms
(`parity/gen/pools.go`, `parity/fixtures/pools/values.json`).

**H6. Call keys of `bids.adjusted`.**
*Seen:* matterfullrtb's `bids.adjusted` `call` members named different calls
on the two sides.
*Cause:* `adapter.bids` entries are re-keyed by their request, because the
MakeRequests order is map order. The `call` members of `bids.adjusted` still
named the original index.
*Fix:* `rekeyView` renames the `bids.adjusted` call members with the same
mapping (`adapter_bids_call_key`, `parity/diff/view.go`).

**H7. gojsonschema message order.**
*Seen:* bidder-params validation messages listed the same items in different
orders (bluesea, beachfront).
*Cause:* gojsonschema lists property errors in map order. The splitter did
not recognise items whose description quotes a pattern or a type.
*Fix:* the splitter matches quoted placeholders (`parity/diff/schemaerrors.go`).
A list that does not split exactly one way is still compared as recorded.

## Go nondeterminism: seams

Where Go's nondeterminism changes an outcome, the differ cannot sort it away.
In those cases parity mode makes Go take one of its possible orders, and the
Rust server must impose the same one. `schema/fakes.json` `orders` lists every
seam, and `TestFakesRegistryOrders` checks that each seam is present at its
cited site.

Four seams were added beside an observed sibling, with the same pattern, and
no A/A run had shown them: `stored_bid_responses_validation`,
`alias_gvlid_validation`, `bid_adjustment_factors_validation` and
`category_dedupe`. Every other seam answers a mismatch seen in a run.

**S1. `bidder_request_phase`.**
*The race:* gumgum copies `Site` but writes the publisher ID through the
shared `Publisher` (`adapters/gumgum/gumgum.go:45-46`). Another bidder's
`hooks.bidder_request` then saw either the original ID or gumgum's.
*The seam:* bidders build their requests one at a time, in name order
(`exchange/parity_turns.go`, `exchange/exchange.go:791` and `:797`). HTTP
calls and `MakeBids` stay concurrent.

**S2. `bidder_response_order`.**
*The race:* a bidder's responses reached `MakeBids` in completion order.
Two adapters depend on that order:

* adgeneration reads the request `cur`, which `requestBid` sets to USD after
  the first `MakeBids` (`exchange/bidder.go:327-329`);
* flipp keeps imp params in a package variable (`adapters/flipp/flipp.go:37`).

*The seam:* `parityBidderTrace.inCallOrder` delivers the responses in call
order (`exchange/bidder.go:287`).

**S3. `fpd_bidders`.**
*The race:* first party data is resolved per bidder in map order. The error
list and the set of bidders resolved before the first error follow that
order.
*The seam:* the bidders are visited in name order
(`firstpartydata/first_party_data.go:142`).

**S4. First failing map entry in validation.**
*The race:* these validations report whichever invalid entry map iteration
reaches first (definemedia, and an invalid alias):

* `bidder_params_validation`: `ortb/request_validator.go:127`;
* `stored_bid_responses_validation`: `ortb/request_validator.go:184`;
* `alias_validation`: `endpoints/openrtb2/auction.go:1086`;
* `alias_gvlid_validation`: `endpoints/openrtb2/auction.go:1110`;
* `bid_adjustment_factors_validation`: `endpoints/openrtb2/auction.go:1000`.

*The seam:* the entries are checked in ascending key order.

**S5. Adapters that range over imp groups.**
*The race:* each group writes the shared `request.Imp`, which `MakeBids`
reads back. So which bids survive depended on map order:

* adtarget: `adapters/adtarget/adtarget.go:62`;
* adtelligent: `adapters/adtelligent/adtelligent.go:62`;
* bidmatic: `adapters/bidmatic/bidmatic.go:58`;
* datablocks: `adapters/datablocks/datablocks.go:41`;
* trafficgate: `adapters/trafficgate/trafficgate.go:46`;
* zeroclickfraud: `adapters/zeroclickfraud/zeroclickfraud.go:42`.

adtelligent, datablocks and bidmatic were observed. The other three came
from a scan for the same pattern.

*The seam:* `fakes.MapKeys` returns the groups sorted.

**S6. `auction_winners` and `category_dedupe`.**
*The race:* the auction replaces an imp's winner only at a strictly higher
price, and it visits the seats in map order (`exchange/auction.go:116`). So
of two bids at the same price, either could win. Category mapping visits the
seats the same way (`exchange/exchange.go:1092`). The "earlier" bid of the
`category.dedupe` decision therefore depended on map order too.
*The seam:* both visit the seats in bidder name order.

**S7. `usersync_bidders`.**
*The race:* the user sync chooser listed the syncers in map order, and
`/cookie_sync` without a bidder list, or with `coopSync`, evaluates them in
that order, so under a limit two processes synced different bidders. On the
main port, where no envelope pins `usersync.shuffle`, Go also shuffled them
with its real random source. The `replayharness` corpus therefore kept 5
such requests out of its cases (`map_order`), and its baseline left the
shape divergences of `POST /cookie_sync` uncounted.
*The seam:* in parity mode the chooser lists the bidders in name order
(`usersync/chooser.go:23`), and `usersync.shuffle` keeps that order when no
case is in flight (`outside_case: "identity"` in `schema/decisions.json`).

**S8. `topics_user_data`.**
*The race:* `UpdateUserDataWithTopics` groups the `Sec-Browsing-Topics`
topics by segtax, segclass and segment ID in nested maps and ranges over
them. Once the `replayharness` replays sent the header, the second of two
A/A runs found one case, an AMP request with two topic sets, whose
`user.data[*].segment` came in a different order on the two sides, in 13
stage records. The first run had passed it.
*The seam:* the added entries come in ascending segtax, then segclass
order, and the segment IDs of every entry, new or merged into one the
request has, in ascending order (`privacysandbox/topics.go:140`, `:142`,
`:155` and `:220`).

## Go nondeterminism: normalization rules

These Go orders and values do not change any other outcome. The differ sorts
or masks them, and each rule in `schema/normalization.json` cites the Go
source.

**R1. `httpcalls_order` and `seatnonbid_order`.**
These rules now also cover:

* a bidder's `bidder.result` http calls and non-bids;
* the http calls that `hooks.all_processed_bid_responses`,
  `hooks.auction_response`, `hooks.exitpoint` and analytics record.

With S2, the calls come in call order. But adapters that group imps in a map
make their requests in map order (ownadx, `adapters/ownadx/ownadx.go:102`),
so the call order itself varies.

**R2. `adapter_requests_order`.**
This rule sorts each request's `imp_ids`, including where `adapter.bids` and
`hooks.raw_bidder_response` echo the request as `request_data`. invibes
builds them from a map (`adapters/invibes/invibes.go:342-344`).

**R3. `stored_auction_error_imp` (candidate).**
Go reports the first stored auction response imp whose bid media type
fails, in map order (`exchange/exchange.go:1560`). The imp ID is masked only
when the side's own `stored.responses` shows two or more such imps.

**R4. `response_cur_seat` (candidate).**
The response `cur` is that of the last seat with bids that map iteration
visits (`exchange/exchange.go:1027-1032`). It is masked only when the side's
own `bidder.result` shows two or more currencies, and only when the `cur` is
one of them.

**R5. `events_ts_param`.**
The event `ts` shape is now masked everywhere an event URL is recorded, not
only in the responses. That includes the hook stages and analytics.

**R6. `amp_targeting_merge` (candidate).**
The AMP endpoint merges cached bids' targeting in seatbid order, which Go
shuffles (`endpoints/openrtb2/amp_auction.go:368-388`,
`exchange/exchange.go:1546`). A key is masked only when two or more cached
bids set it to different values. The trace must also show that some merge
order ends with the recorded value.

A candidate rule masks a value only when the side's own trace shows two or
more candidates and the value is one of them. A wrong value is still a
mismatch.

## Known gaps

* **Untested connection metrics.** `connections_opened`, `connections_closed`
  and `connections_error` cannot be compared through exec. A Rust listener
  that counts connections differently would go unnoticed.
* **Unobserved map-order sites.** The scan of map loops that stop at the
  first error, or that write shared state, covered the endpoint, ortb,
  exchange, firstpartydata and adapter code. Sites no generated case
  reaches, or that no scan pattern matches, stay nondeterministic in Go. One
  known example is `exchange/utils.go:693`, which merges imp FPD per bidder
  and returns the first error in map order. Another is the video endpoint's
  `findTargetingByKey` (`endpoints/openrtb2/video_auction.go:643-652`): it
  returns the value of the first targeting key, in map order, whose part
  from the first `_` on starts the key it looks for. For an ad pod's
  `hb_pb_cat_dur` it looks for `_pb_cat_dur_<seat>`, which `hb_pb` matches
  as well as `hb_pb_cat_dur_<seat>`, so with `includebrandcategory` the value
  is sometimes the price alone. No replay or case turns brand category on
  in a video request.
* **MakeBids can still race.** The turns of S1 order only the request phase.
  An adapter whose `MakeBids` writes an object shared with another bidder
  would still race.
* **Turns end at the deadline.** When the case deadline expires, the waiting
  bidders run concurrently.
* **Untracked goroutine.** `doTimeoutNotification` in `exchange/bidder.go`
  is not tracked by `Settle`. Its effects after the response are not
  awaited.
* **flipp state.** flipp's package-level imp params persist across cases. A/A
  hides this because both processes run the same case order.
* **`MapKeys` allocates outside parity mode.** It builds the key slice even
  when parity is off. That is behaviour-neutral, but it is not zero-cost on
  those loops.
