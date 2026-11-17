# Go fixture conformance

Each bidder adapter in the Go server ships JSON test fixtures under
`adapters/<dir>/<dir>test/{exemplary,supplemental,amp,video,videosupplemental}/*.json`.
`adapters/adapterstest/test_json.go` runs them against the Go adapter. The Rust port has to pass
the same fixtures, read from the same files, with the same semantics. This directory holds the
contract between that Rust harness, CI and the daily upstream-port workflow.

The fixtures are not in this repository. They are read from the *Go root*: the upstream checkout
at the commit `UPSTREAM_SYNC` pins, which `scripts/fetch-upstream.sh` puts in `.upstream/`. Every
fixture and adapter path below is relative to the Go root.

## Harness

| Item | Contract |
| --- | --- |
| Command | `cargo test -p pbs-adapters --test go_fixtures --locked`, run from the workspace root |
| Fixture set | Every run that `testdata/go_fixture_manifest.json` lists (one per `adapterstest.RunJSONBidderTest` call), with the builder configuration the manifest records |
| Semantics | `adapters/adapterstest/test_json.go`: `mockBidRequest`, `httpCalls[].expectedRequest` (`uri`, `body`, `headers`, `impIDs`), `mockResponse`, `expectedBidResponses`, `expectedMakeRequestsErrors`, `expectedMakeBidsErrors` (literal or regex), and the rule that `exemplary` and `video` fixtures expect no errors |
| `PBS_GO_ROOT` | The Go root, where `adapters/` and `static/` are read from. If unset, `.upstream/` in the workspace. The harness never searches for it: when it is missing, the harness fails and says to run `scripts/fetch-upstream.sh` (`pbs_devtools::repo::go_root`). |
| `PBS_FIXTURE_DIRS` | Optional comma-separated adapter directories (`adapters/<dir>`). Only those runs execute. Empty or unset means every run. |
| `PBS_FIXTURE_RESULTS` | Optional path. See "Report mode". |

### Report mode (`PBS_FIXTURE_RESULTS` set)

The harness writes one JSON object per line to that path, one line for every fixture it runs:

```json
{"run": "<bidder>/<dir>", "fixture": "<relative path>", "status": "pass|fail", "detail": "..."}
```

* `run` names the manifest run: the bidder name, a slash, and the adapter directory.
* `fixture` is the fixture's path relative to the Go root, for example
  `adapters/panxo/panxotest/exemplary/banner.json`.
* `status` is `pass` or `fail`. No other value is allowed.
* `detail` is empty on a pass. On a failure it is the first difference found, in plain text.

In report mode the harness only reports. Its exit status is 0 even when fixtures fail, and it is
non-zero only when the harness itself breaks: a fixture cannot be read, the manifest is
inconsistent, or the results file cannot be written.

### Enforcing mode (`PBS_FIXTURE_RESULTS` unset)

The harness applies the ratchet below itself and fails the test if it does not hold. That way a
plain `cargo test --workspace` also enforces conformance.

## Ratchet: `known_failures.txt`

`known_failures.txt` lists the fixtures that are allowed to fail today: one fixture path per line,
relative to the Go root, sorted (byte order, `LC_ALL=C`), with no duplicates. Blank lines and lines
starting with `#` are ignored. Every listed path must exist in the Go root.

A fixture **fails** when any run that uses it fails, and **passes** when every run that uses it
passes. Given the full report (`.github/port/scripts/conformance.sh` evaluates it), CI fails on
any of these:

1. **New failure.** A failing fixture is not listed.
2. **Stale entry.** A listed fixture passes. Delete the line in the same change that fixes it.
3. **Missing entry.** A listed fixture was not run at all, for example because upstream deleted it.
4. **Incomplete run.** On a full run, the number of results, or of distinct fixtures, differs
   from the manifest totals (`totals.fixture_runs_total` and `totals.distinct_fixture_files`).
5. **Strict adapter.** A fixture of an adapter the change touches fails, even when it is listed.
   A change to an adapter has to make all of that adapter's fixtures pass.
6. **Bad report.** A result line has no `fixture`, or a `status` other than `pass` or `fail`.

The list may only shrink. The upstream-port bot may never add entries to it. A human change that
adds entries needs review, and adding entries for a touched adapter is rejected outright.

## While the harness does not exist

Until `pbs-adapters` has a `go_fixtures` test target, `conformance.sh` reports `harness: absent`
and passes. Setting the repository variable `CONFORMANCE_REQUIRED` to `true` makes an absent
harness fail CI and the port gates instead.
