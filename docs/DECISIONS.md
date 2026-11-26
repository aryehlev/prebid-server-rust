# Decisions

This file records every decision in the plan's decision table, the evidence behind each one, and
the crates units may adopt. It revises the version of 2026-11-18 (wu/SPIKE-DEPS, `a8f38cb9`) with
the orchestrator's rulings of 2026-11-19 and with the second dependency spike (unit DECISIONS,
2026-11-19). That spike tried every candidate crate of the plan under principle 9's gates. Where a
crate passed, the default moved to it (crates first). Where none passed, the plan's fallback
applies, and the entry says why.

Go paths are relative to the Go root at upstream `4e296450de2fcc62741340b2e5f7d1df6c1744a5`
(`git describe --tags`: `v4.11.1-1-g4e296450`), the commit `UPSTREAM_SYNC` pins, and every
`file:line` is at that commit. A Go module path names the version the pinned `go.mod` requires.

Each decision has one of two statuses:

- **decided**: the user or the orchestrator decided it, and units follow it;
- **default applies**: the plan's default, revised here where the spike changed what passes. A
  unit takes the default until the user decides otherwise.

No unit adopts a candidate crate before this file records it. [The accepted
list](#the-accepted-list) gives the exact `[workspace.dependencies]` line of every accepted crate.
Only the orchestrator adds a line (principle 9). The line waits in the commented block at the end
of `[workspace.dependencies]` until the first unit that uses it moves it up unchanged (ruling 6).

Rule A applies to every crate: each difference from Go that a unit observes gets a ledger entry
with a pin test. Go-matching glue exists only under Rule B. Each glue module starts with `//! Glue
(Rule B): <case and reason>` and comes with a restored ledger entry and a Go golden pin. The ledger
ids this file names are proposals for the adopting unit, which writes the entries.

## Summary

| Decision | Status | Decision or default | Units it gates |
|---|---|---|---|
| D-STACK | decided (user, 2026-11-18) | Crates for library work; no viper port; tracing for logs; OpenRTB types as plain serde structs | every unit |
| RULE-B | decided (orchestrator, 2026-11-19) | Go-matching glue only in Rule B's four cases; everything else is ledgered | every unit |
| D-DECODE | decided (orchestrator, 2026-11-19) | Plain serde decoding; Go's null handling and lone surrogates restored | JSONUTIL, ORTB-SERDE |
| D-HTMLESC | decided (orchestrator, 2026-11-19) | serde_json's escaping everywhere (json.html-escape ledgered) | JSONUTIL |
| D-UA | decided (orchestrator, 2026-11-19) | `Go-http-client/1.1` when the caller sets no User-Agent | HTTP-CLIENT, HTTP-H2 |
| D-HDRCASE | decided (orchestrator, 2026-11-19) | hyper's lower-case header names on the wire; canonical names inside Prebid's output | HTTP-CLIENT, SERVER-AXUM |
| D-ROUTER | decided (orchestrator, 2026-11-19) | axum's router, with httprouter's edges ledgered | SERVER-AXUM, ROUTER-MAIN |
| D-TLS | decided (orchestrator, 2026-11-19) | rustls-platform-verifier plus `certificates_file` roots (tls.acceptance ledgered) | HTTP-CLIENT |
| D-VIPERFMT | decided (orchestrator, 2026-11-19; formats settled by this spike) | JSON, YAML and TOML; INI and viper's other formats fail startup with a clear error | CONFIG-LOADER |
| HTTP/2 | decided (user, 2026-11-18; demotion rule by the orchestrator, 2026-11-19) | Automatic for bidder requests, with no configuration; one HTTP/1.1 resend, only when the origin read no request | HTTP-H2, EXCH-BIDDER, ROUTER-MAIN, PH-PROFILES, PH-WIRE-COVER |
| D-51D | decided (user, 2026-11-18) | The full 51Degrees engine through FFI in `pbs-fiftyone-dd` | GOC-51D-DD, GOC-51D-ONPREMISE, MOD-51D |
| D-WURFL | decided | The module over a `WurflApi` trait, with no binding | MOD-WURFL |
| D-FASTGZIP | decided (user, 2026-11-25, ruling 10 (b)) | fastgzip writes the server's gzip: pbs-server's own response layer keeps gziphandler's semantics in place of tower-http's `compression-gzip` | FASTGZIP, EXCH-BIDDER |
| D-COW | decided (keep) | `Option<Arc<T>>` on serde's rc feature, re-measured by COW | COW |
| D-TEMPLATE | default applies | `pbs-gocompat`'s template module: gtmpl fails the gates and the error goldens | ENT-MACROS, USERSYNC-PBS, CLEANUP |
| D-XML | default applies | quick-xml 0.42.0 for tokens, with Go's encoder output and attribute escaping as glue | INJECTOR |
| D-DB | default applies (fallback) | Minimal clients: sqlx, tokio-postgres and mysql_async all fail the gates | STORED-DB |
| D-GRPC | default applies (fallback) | A minimal unary client on h2 0.4.20, with prost 0.14.6 without derive: tonic fails | ADSCERT |
| D-DNS | default applies | hickory-resolver 0.26.3 (tokio, no system-config), configured from `/etc/resolv.conf` by resolv-conf 0.7.6 | ADSCERT |
| D-INFLUX | default applies (kept by the orchestrator, 2026-11-19; its crate option now passes) | The reporter's points and writes go through influxdb 0.8.0; pings and go-metrics' semantics are written by hand | METRICS-INFLUX |
| D-JSONSCHEMA | default applies (kept by the orchestrator, 2026-11-19; not retried) | The hand-written draft-04 subset | BIDDER-PARAMS |
| D-LOGFILE | default applies | tracing-appender 0.2.5; glog's per-severity files ledgered (logging.files) | LOGGING, ANALYTICS-MODULES |
| D-SIZE | default applies (fallback) | A port of go-units' `FromHumanSize`: parse-size and bytesize pass the gates but not the goldens | ANALYTICS-MODULES |
| D-PSL | default applies | psl, with psl.list-version ledgered; publicsuffix 2.3.0 passes and is the restore option | GOC-PSL, OPENRTB2-AUCTION |
| D-DEAD | default applies | Production-dead Go is ported with its tests | INJECTOR, PORTED-GAPS |
| D-GOCOLL | default applies | No `go_*` families (metrics.go-families, decision-pending) | METRICS-PROM, ROUTER-MAIN |
| D-PPROF | default applies | `/debug/pprof` is not served (ledgered) | ROUTER-MAIN |
| D-VERSION | default applies | `PBS_VERSION` and `PBS_REVISION` identical on both sides in every comparison | FPD-SCHAIN-VERSION, OPS-IMAGE, ROUTER-MAIN |
| D-ARCH | default applies | amd64 | Go goldens, the Go parity server, OPS-IMAGE |
| D-SYNC | default applies | Ask the user at FINAL-INT | SYNC-CATCHUP |

[Crates without a decision of their own](#crates-without-a-decision-of-their-own) covers lru,
getopts, multer, json-patch, arc-swap, thiserror, similar, tower-http's `fs` feature, criterion,
dhat, notify and aws-lc-rs as a normal dependency. [The second dependency
spike](#the-second-dependency-spike) gives the method, a row per candidate, the union run and the
Go goldens. [Changes outside this file](#changes-outside-this-file) lists what other files need.

## The accepted list

Each crate below passed every gate alone, and all of them passed together (see [the union
run](#the-union-run)). The orchestrator adds the lines, in name order, to the commented block at
the end of `[workspace.dependencies]`, each line behind one more `# `, and the unit named in the
table below moves a line up when it first uses the crate.

```toml
# Shared state a background task replaces while requests read it (currency rates, vendor lists,
# stored data).
arc-swap = "1.9.5"
# Dev-only: COW's benches. Cargo.lock must keep half at 2.6.0: half 2.7.1 depends on zerocopy,
# whose zerocopy-derive is on syn 2.
criterion = { version = "0.8.2", default-features = false }
# glog's nine flags in Go's flag syntax (LOGGING). The default unicode feature resolves to the
# unicode-width 0.2.3 that serde-saphyr's annotate-snippets already locks.
getopts = "0.2.24"
# The ads.cert remote signer's unary gRPC client (D-GRPC: tonic fails the gates). hyper and
# reqwest already lock it.
h2 = "0.4.20"
# ads.cert's TXT discovery (D-DNS). No system-config: it pulls core-foundation 0.9 beside 0.10.
hickory-resolver = { version = "0.26.3", default-features = false, features = ["tokio"] }
# The InfluxDB reporter's line protocol and writes (D-INFLUX), over pbs-http-client's reqwest.
influxdb = { version = "0.8.0", default-features = false, features = ["rustls"] }
# RFC 7386 merge only (JSONUTIL). The diff feature pulls treediff; 0.3.0 through 4.1.0 need
# thiserror 1, on syn 2.
json-patch = { version = "0.2.7", default-features = false }
# golang-lru's simplelru (FETCHER). No default features: they pull foldhash, licensed Zlib.
lru = { version = "0.19.0", default-features = false }
# Multipart bodies for FormValue at /optout (USERSYNC-PBS).
multer = { version = "3.1.0", default-features = false }
# The ads.cert messages, implemented by hand: prost-derive is on syn 2.
prost = { version = "0.14.6", default-features = false, features = ["std"] }
# The VAST tokenizer (D-XML). The injector writes Go's encoder output itself (Rule B (d)).
quick-xml = { version = "0.42.0", default-features = false }
# Reads /etc/resolv.conf, the file Go's resolver reads, for hickory-resolver (D-DNS).
resolv-conf = "0.7.6"
# adapterstest's diff text (ADP-HARNESS).
similar = { version = "3.2.1", default-features = false, features = ["text"] }
# Error types (pbs-errortypes and each crate's own); prometheus and quinn already lock it.
thiserror = "2.0.21"
# One log file (D-LOGFILE); glog's per-severity files are ledgered.
tracing-appender = { version = "0.2.5", default-features = false }
```

Three existing entries change. Each change also passed the gates:

- `config` gains the `toml` feature (D-VIPERFMT): `features = ["json", "toml"]`. The feature needs
  `toml` ^1.0.6 with `parse` and `std`, which the waiting `toml` 1.2.1 entry satisfies, so the
  lock gains no package.
- `tower-http` gains the `fs` feature for `/static` (SERVER-AXUM; ROUTER-MAIN mounts the service):
  `features = ["compression-gzip", "cors", "fs", "limit", "timeout"]`. The lock gains
  http-range-header 0.4.2, mime_guess 2.0.5 and unicase 2.9.0.
- The waiting `aws-lc-rs` entry becomes a normal dependency of ADSCERT (X25519, HMAC-SHA256) and
  STORED-DB (SHA-1, RSA-OAEP, PBKDF2), with the same version and features. rustls already links
  aws-lc-rs into every build, so the lock gains nothing. Its "Dev-only" comment changes when the
  first of those units moves it up.

These crates passed the gates but are not adopted. A unit takes one only if its decision changes
here first:

```toml
# D-PSL's restore option: the list behind x/net v0.55.0, loaded at startup.
publicsuffix = { version = "2.3.0", default-features = false, features = ["std"] }
# D-SIZE: neither matches go-units' FromHumanSize.
parse-size = "1.1.0"
bytesize = { version = "2.7.0", default-features = false }
```

json-patch 5.0.0 (default features off) passes as well and adds jsonptr 0.8.2. It merges exactly
as 0.2.7 does on the goldens. 0.2.7, the version ruling 9 named for the trial, is chosen because it
adds one package fewer.

### What each accepted crate brings to the ledger

| Crate | Adopting unit | Rule B glue (restored entries) | Ledgered (Rule A) |
|---|---|---|---|
| arc-swap 1.9.5 | CURRENCY-CRATES, GDPR, STORED-CORE | none | none |
| criterion 0.8.2 (dev) | COW | none | none: benches only |
| getopts 0.2.24 | LOGGING | `logging.flags` (c): glog's nine flags with Go's syntax, values, repeats (the last value wins) and exit codes | `logging.flag-texts`: the usage and error texts Go's flag package prints |
| h2 0.4.20 | ADSCERT | none | `grpc.client-frames`: HTTP/2 framing (SETTINGS, window updates, HPACK, header order) against grpc-go 1.83.1 |
| hickory-resolver 0.26.3, resolv-conf 0.7.6 | ADSCERT | none | `dns.txt-resolver`: queries, retries, EDNS0 and TCP fallback against Go's pure-Go resolver |
| influxdb 0.8.0 | METRICS-INFLUX | metric names, tags and field types stay go-metrics-influxdb's (c) | `influx.write-request`: the query parameters and their order, no Content-Type header, no final newline, non-finite floats |
| json-patch 0.2.7 | JSONUTIL | none by default; the restore option of the two decision-pending entries is a check in front of the merge | `json.merge-patch-quirks`: nulls inside arrays; `json.merge-patch-null-document` and `json.merge-patch-scalar-patch` (decision-pending): Go's `ErrBadJSONDoc` and `ErrBadJSONPatch`, which Prebid turns into its own 400 |
| lru 0.19.0 | FETCHER | none | none: exact on the golden |
| multer 3.1.0 | USERSYNC-PBS | none | `url.multipart-edges`: quoted-printable parts, limits and malformed bodies against Go's `mime/multipart` |
| prost 0.14.6 | ADSCERT | none | none: the messages are hand-written and pinned to adscert's Go encoding |
| quick-xml 0.42.0 | INJECTOR | `xml.escape` (d): Go's `Encoder.EncodeToken` output, attribute values escaped as `EscapeString` escapes them | `xml.error-texts`; `xml.token-acceptance` |
| similar 3.2.1 | ADP-HARNESS | none | none: test output only |
| thiserror 2.0.21 | CLEANUP | none | none: the texts are written to match Prebid's |
| tracing-appender 0.2.5 | LOGGING, ANALYTICS-MODULES | `analytics.line-prefix` (b) | `logging.files`; `analytics.file-rotation` |
| config's `toml` feature | CONFIG-LOADER | viper's file search order and weak typing (c), as for JSON and YAML | `config.formats`; `config.toml-reader` |
| tower-http's `fs` feature | SERVER-AXUM | none | `server.static-files` |
| aws-lc-rs 1.19.0 (normal) | ADSCERT, STORED-DB | none | none |

## Rule B and the rulings of 2026-11-19

### RULE-B: where Go-matching glue is allowed

**Status.** Decided by the orchestrator on 2026-11-19 (ruling 1).

**Decision.** Go-matching glue is written only for:

- (a) input that real production traffic sends and that the crate would reject or misread. The
  known case is JSON null where Go leaves the zero value of a non-pointer field: serde's default
  plus a null-tolerant deserializer that typegen emits;
- (b) state one server writes and the other reads while Go and Rust run side by side during the
  migration: the uids cookie value format, cache and stored-data formats, and IDs and hashes
  another system stores or compares;
- (c) the operator interface: config keys, nesting, defaults, `PBS_` variable names and
  precedence, CLI flags, and metric names, labels and types;
- (d) Prebid-authored texts and bytes: the error and warning messages Prebid writes, macros,
  targeting keys and values, consent strings, VAST injection and `DropElement`.

Everything else a crate does is accepted and ledgered (Rule A) with a pin test. Ruling 2 names what
is ledgered rather than restored: JSON number spelling, HTML escaping, json.non-finite,
case-insensitive and duplicate JSON keys, invalid UTF-8, `interface{}` numbers, query and form
parsing, routing edges, header case and order, library error wording, log line format and files,
exposition formatting, TLS chain acceptance and viper's formats.

### D-DECODE: JSON decoding

**Status.** Decided by the orchestrator on 2026-11-19 (rulings 1, 2, 3 and 7).

**Decision.** Plain serde decoding, with serde_json and serde derives used directly everywhere
(ruling 3; no mandatory writer, no review ban on `serde_json::to_*`, axum's `Json` or reqwest's
`.json()`). Restored under Rule B (a):

- null on a non-pointer field keeps the zero value (serde's default plus typegen's `null_default`);
- null on a `json.RawMessage` stays null (`raw_keep_null`);
- an unpaired `\uD800`-`\uDFFF` escape becomes `\uFFFD` before serde_json reads external bytes
  (json.lone-surrogates, ruling 7). Prebid.js sends `JSON.stringify` output, which can hold a
  truncated emoji; Go decodes it to U+FFFD where serde_json would reject the whole document. The
  scan runs only when the bytes contain such an escape, and tracks string and escape state.

Ledgered: duplicate keys, invalid UTF-8, case-insensitive keys, `interface{}` numbers, error texts
and unvalidated RawMessage content.

**Correction (review, 2026-11-19).** Go's handling of null on a `json.RawMessage`, which Rule B (a)
restores, depends on the decoder, and the ruling's premise that Go keeps the bytes holds only for
`encoding/json`. `jsonutil.Unmarshal` and `UnmarshalValid` run json-iterator v1.1.12, whose
`jsonRawMessageCodec.Decode` sets the field to nil on null (`reflect_json_raw_message.go:35-41`;
Prebid's `RawMessageExtension` changes only encoding). More than 300 non-test files decode through
them, the auction request among them (`endpoints/openrtb2/auction.go:468`, `:516`, `:539`). Only
13 production files use `encoding/json`, which keeps the four bytes `null`: 8 adapters,
`floors/fetcher.go`, `gdpr/gvl_vendor_ids.go`, `pbs/usersync.go`, `analytics/pubstack/config.go`
and `privacysandbox/topics.go`. A Go program built against the pinned `go.mod` (go1.26.2, amd64)
decodes `{"id":"u","ext":null}` into `openrtb2.User` with `len(Ext) == 0` and `Ext == nil` through
both jsonutil functions, re-marshals it as `{"id":"u"}`, and gets `null` through `json.Unmarshal`.
So `raw_keep_null` reproduces Go only at `encoding/json` sites. At jsonutil sites serde's `None` is
already Go's nil, and `Some(null)` would write `"ext":null` into bidder requests where Go omits the
field and would send Prebid into its `len(ext) > 0` branches, such as the first-party-data merges
(`firstpartydata/first_party_data.go:253`, `util/jsonutil/merge.go:159`). [Changes outside this
file](#changes-outside-this-file) asks the orchestrator to correct the ruling's text; JSONUTIL and
ORTB-SERDE pin both decoders against Go goldens either way.

### D-HTMLESC: HTML escaping in JSON output

**Status.** Decided by the orchestrator on 2026-11-19 (rulings 2 and 5).

**Decision.** serde_json's escaping everywhere (json.html-escape, ledgered), with no Go-escaping
formatter. Only a site where marshalled JSON becomes an ID or hash another system stores or
compares (Rule B (b)) would get Go's escapes, and none is known: both servers decode the uids
cookie's JSON either way.

### D-UA: the outbound User-Agent

**Status.** Decided by the orchestrator on 2026-11-19 (ruling 5).

**Decision.** `User-Agent: Go-http-client/1.1` when the caller set none: one line of client
configuration, also over HTTP/2. A caller that sets its own value keeps it (the InfluxDB reporter
sends `InfluxDBClient`, as influxdb1-client does).

### D-HDRCASE: header-name case

**Status.** Decided by the orchestrator on 2026-11-19 (ruling 2).

**Decision.** Header names go on the wire as hyper writes them (lower case), on the client and the
server. Go's canonical spelling on the wire is not reproduced (http-client.header-case and
server.header-case, ledgered). Header names inside Prebid's own output (`RequestData`,
`ext.debug.httpcalls`) keep `http.Header`'s canonical form (Rule B (d)).

### D-ROUTER: routing

**Status.** Decided by the orchestrator on 2026-11-19 (ruling 2).

**Decision.** axum's router, with no httprouter emulation. httprouter's 404 body, redirects, 405
and `Allow`, automatic OPTIONS, HEAD handling and decoded-path matching are ledgered
(server.not-found, server.redirects, server.method-not-allowed, server.head-method and
server.decoded-path). Ruling 5 also accepts and ledgers server.cors-preflight,
http-client.request-target and config.gc-threshold.

### D-TLS: certificate verification

**Status.** Decided by the orchestrator on 2026-11-19 (ruling 2).

**Decision.** rustls-platform-verifier plus the roots in `certificates_file`. Chains that
crypto/x509 accepts and webpki refuses are ledgered (tls.acceptance) and pinned by wu/t2b-tls's
x509 goldens. That branch's Go-order verifier is not used.

### D-VIPERFMT: config files other than JSON and YAML

**Status.** Decided by the orchestrator on 2026-11-19 (ruling 2): JSON and YAML, plus TOML and INI
only if config's features pass this spike's gates. The `toml` feature passes and the `ini` feature
fails, so the formats are JSON, YAML and TOML.

**Go behaviour at stake.** `config.SetupViper` (`config/config.go:936-940`) names the config file
`pbs` (`main.go:59`) and adds the search paths `.` and `/etc/config`. viper v1.12.0 (`go.mod:37`)
tries every path in turn, and in each one every extension of `SupportedExts` (`viper.go:419`; the
loops are `viper_go1_15.go:19` and `:30`, the file built without the `finder` tag, which Prebid
does not set): json, toml, yaml, yml, properties, props, prop, hcl, tfvars, dotenv, env, ini.
So `./pbs.toml` is read when `./pbs.json` is absent, and it wins over `./pbs.yaml`.
`ReadInConfig`'s error is ignored (`config/config.go:1322`). The readers come from
`pelletier/go-toml/v2` v2.0.1, `hashicorp/hcl` v1.0.0, `magiconair/properties` v1.8.6,
`subosito/gotenv` v1.3.0 and `gopkg.in/ini.v1` v1.66.4.

**Decision.** CONFIG-LOADER reads JSON and YAML (the stack) and TOML through config's `toml`
feature, which reads with the `toml` crate. It keeps viper's search order over every extension
(Rule B (c)), so a `pbs.properties`, `pbs.hcl`, `pbs.tfvars`, `pbs.env`, `pbs.dotenv` or `pbs.ini`
that viper would pick makes startup fail with an error naming the file (config.formats, ledgered).
TOML values then get the same weak typing as JSON and YAML values (ruling 8 for bools). The `toml`
crate's parse results and error texts are not go-toml v2.0.1's (config.toml-reader, ledgered;
CONFIG-LOADER pins it against a Go golden of `ReadInConfig` and `Unmarshal`).

**Evidence.** config's `toml` feature added nothing to the lock (row
[config `toml`](#results)). The `ini` feature pulls rust-ini 0.21.3, which brings getrandom 0.2.17
beside the stack's 0.4.3, hashbrown 0.14.5 beside 0.17.1, and tiny-keccak 2.0.2, whose licence,
CC0-1.0, is not in `deny.toml`'s allow list.

### The other rulings

Ruling 4 deletes `pbs-gocompat` except for a module justified under ruling 1; only `text/template`
remained a candidate for that, and [D-TEMPLATE](#d-template-gos-texttemplate) now keeps it. Ruling
4 also makes floats that Prebid prints into its own texts use `format!` with the precision Prebid's
Go code uses (Prebid logic, ported in place). Ruling 5 sends `/optout`'s multipart bodies through
multer, which passed (see [multer](#multer-formvalue-at-optout)). It also leaves out a clippy
`disallowed-methods` rule for JSON and shrinks JSONUTIL to size M. Ruling 6 sets R1 check 10 (one
version per package in `Cargo.lock` and in `cargo tree`) and the waiting block. Ruling 8 converts
config bools as viper does (Rule B (c)). Ruling 9 turns STACK's 30 library pins into ledger
entries, and it asked this spike to try json-patch 0.2.7.

## D-STACK: crates for library work

**Status.** Decided by the user on 2026-11-18.

**Decision.** Library work uses crates: the plan's crate stack, which STACK put into
`[workspace.dependencies]`, plus this file's accepted list. There is no port of viper: config
0.15.27 with a serde-saphyr `Format` and the `pbs-config` loader replace it, and the loader holds
the operator interface (Rule B (c)). Logs go through tracing. OpenRTB and openrtb_ext types are
plain serde structs, with no Go-named derive (ruling 3).

**Evidence.** STACK's spike and this one. The crates both spikes rejected, and why, are in the
plan's rejected table and in [the results](#results) below.

## D-COW: shared request data

**Status.** Decided (keep).

**Decision.** `Option<Arc<T>>` on serde's `rc` feature. COW re-measures, and reverts to `Box` only
below 40% gains or above a 5% regression.

**Evidence.** criterion 0.8.2 passes as a dev dependency with `half` held at 2.6.0, so COW's time
benches can use it. dhat 0.3.3, which PORTING §7 names for allocation counts, fails R1 check 10
(rustc-hash 1.1.0 beside 2.1.3), so allocation counts need another way: a counting `GlobalAlloc`
needs `unsafe`, which only `pbs-fiftyone-dd` may contain, and this spike tried no other
allocation-profiling crate. COW measures time with criterion and, if it needs allocation counts,
asks for a candidate first.

## D-51D: 51Degrees device detection

**Status.** Decided by the user on 2026-11-18: implement the engine fully, with `unsafe` allowed
only for the EUPL code. GOC-51D-DD and GOC-51D-ONPREMISE port it, and MOD-51D has no exemptions.

**Go behaviour at stake.** `modules/fiftyonedegrees/devicedetection` is in Go's default build and
links `github.com/51Degrees/device-detection-go/v4` v4.4.35 (`go.mod:6`) through cgo. The engine
is `dd/device-detection-cxx.c` (21,925 lines) and `dd/device-detection-cxx.h` (9,196 lines),
EUPL-1.2, compiled with cgo's default `-g -O2` plus `dd/cgoflags.go:31-33`: `#cgo CFLAGS:
-fcommon`, `#cgo LDFLAGS: -lm` and `#cgo !darwin LDFLAGS: -latomic`. The `onpremise` package loads
and reloads the data file, pulls updates (it checks `Content-MD5` with `crypto/md5`,
`onpremise/file_pulling.go:199-203`) and watches the file. The watcher polls:
`onpremise/file_watch.go:52` creates it with a one-second interval, and `onpremise/watcher.go:53-73`
compares `os.Stat`'s size and `ModTime` on each tick. The module reads the engine's properties
into `device` and `device.ext`. The Go module ships a test data file, `onpremise/mock_hash.gz`
(30,896,373 bytes).

**Decision.** `pbs-fiftyone-dd` holds the binding and is the only crate with `unsafe`.
`pbs-fiftyone-onpremise` and the module keep `forbid(unsafe_code)`. The C is never committed: the
build script takes the two files from `PBS_51D_SRC`, then `.upstream-deps`, then the Go module
cache at v4.4.35, rejects them unless they match these SHA-256s, and compiles them with `cc`:

| File | SHA-256 |
|---|---|
| `dd/device-detection-cxx.c` | `5f0f4595b5f991eaacdbbc117adeb01f38247e3d5b7212f3635b73d063141be9` |
| `dd/device-detection-cxx.h` | `7209f994bd8315b102e195495e444a2c6bb1773b5e79dac736f67789775660a4` |

The flags are cgo's, plus `-ffp-contract=off`. Under D-ARCH the reference C runs on x86-64, which
has no fused multiply-add in its baseline instruction set. C compilers contract by default (clang
`-ffp-contract=on`, GCC `fast`), so on arm64 they would fuse. Turning contraction off gives the
same results on every host.

GOC-51D-ONPREMISE ports the watcher as the same polling loop on `tokio::time`, so it needs no
crate; notify fails the gates anyway (see [notify](#notify-51degrees-file-watching)). The
`Content-MD5` check uses md-5 0.11.0 from the stack's waiting block, and the pulls use reqwest and
flate2.

**Evidence.** The first spike's `cc` trial passed every gate. Its build script used `cc` 1.11.2
and `sha2` as build dependencies, both already in the lock (`cc` through `aws-lc-sys`). It read the
files from the Go module cache (`$GOMODCACHE`, `$GOPATH/pkg/mod` or `~/go/pkg/mod`, under
`github.com/51!degrees/device-detection-go/v4@v4.4.35`), checked both hashes, and built
`libfiftyone_dd.a` with `opt_level(2)`, `debug(true)`, `-fcommon` and `-ffp-contract=off`. It
linked `-lm`, plus `-latomic` everywhere except macOS. A test confirmed that the archive defines
six entry points the `dd` package calls (`fiftyoneDegreesHashInitManagerFromFile`,
`...HashReloadManagerFromOriginalFile`, `...ResultsHashFromEvidence`,
`...ResultsHashGetValuesString`, `...ResourceManagerFree` and `...EvidenceAddString`). The trial
never called the C, so it needed no `unsafe`, and clippy accepted the build script's `cargo::`
lines, because `print_stdout` does not apply to build scripts. This spike found one obstacle:
`scripts/cargo-machete.sh` reports `cc` as unused when only `build.rs` uses it (see [changes
outside this file](#changes-outside-this-file)).

**What else must change.** The scanner's `unsafe` and `crate-lints` rules (designed in
GOC-51D-DD), PORTING §5, the crate's licence metadata against `deny.toml`, NOTICE, and
`scripts/cargo-machete.sh`; all are listed under [changes outside this
file](#changes-outside-this-file).

## D-WURFL: ScientiaMobile WURFL

**Status.** Decided by the user on 2026-11-18: port the whole module over a `WurflApi` trait. The
libwurfl binding waits on the user.

**Go behaviour at stake.** In Go's default build, `wurfl_engine_fallback.go` (`//go:build
!wurfl`) makes the engine constructor fail with `wurfl module requires the wurfl build tag; build
with: go build -tags wurfl`. With `-tags wurfl`, `wurfl_engine.go` uses
`github.com/WURFL/golang-wurfl` v1.30.3 (`go.mod:10`), which links the closed `libwurfl`
(`wurfl.go:12`, `#cgo LDFLAGS: -lwurfl`). The engine calls `wurfl.Create` with an LRU cache
(`wurfl_engine.go:30-34`), `GetAllCaps` (`:41`), `LookupWithImportantHeaderMap`, `GetDeviceID`,
`GetStaticCaps`, `GetVirtualCaps` and `Destroy` (`:67-81`), and the updater calls
`SetUpdaterDataURL`, `SetUpdaterDataFrequency(daily)` and `UpdaterStart` (`:97-109`).
golang-wurfl's LICENSE begins "WURFL API for GoLang - Commercial Use License". libwurfl is not
installed here; the module's `libwurfl/` directory holds only a `.gitignore`.

**Decision.** MOD-WURFL ports config, enricher, headers, data and hooks. The `wurfl` build tag
becomes a cargo feature whose engine ports `wurfl_engine.go` over `WurflApi`, which has exactly
the golang-wurfl calls listed above. Its tests run against a fake. The default build returns
Go's fallback error, so it behaves like Go's default build. No binding is written. A binding
needs the user's ScientiaMobile licence, libwurfl itself, and a second `unsafe` allowance, which
the user would decide when that time comes.

**Evidence.** No dependency is involved. `module_wurfl_test.go` (`//go:build wurfl`, 59 lines)
needs a real engine in Go. In Rust it runs over the fake, and a Go golden for it waits for the
binding.

## HTTP/2 to bidders

**Status.** Decided by the user on 2026-11-18 ("support http2 and h2c based if the buyers support
it, put it in the Readme", then "No configs I want automatically to figure out"). The orchestrator
added the demotion rule on 2026-11-19 (ruling 5).

**Go behaviour at stake.** Go sends every bidder request over HTTP/1.1. All adapters share one
client (`router/router.go:143-158`, passed to `exchange.BuildAdapters` at `:247`). Its transport
sets `DialContext` and `TLSClientConfig` and sets neither `ForceAttemptHTTP2` nor `Protocols`.
Go 1.26.2's `Transport.protocols` (`net/http/transport.go:477`) therefore leaves HTTP/2 off.
The same client also serves modules, stored-request HTTP fetchers and the GVL fetchers
(`router.go:211-237`). The cache and floors clients (`:160-189`) are HTTP/1.1 for the same reason.

**Decision.** The bidder client negotiates HTTP/2 by itself, with no configuration key and no
per-bidder setting, on reqwest (HTTP-H2):

- on https it offers ALPN `h2,http/1.1` in the real handshake and uses whatever the bidder picks;
- on cleartext, requests go over HTTP/1.1 at once while a background probe connection sends only
  the HTTP/2 preface and an empty SETTINGS frame, never a bid request. A SETTINGS reply marks the
  origin h2c, and anything else keeps it on HTTP/1.1;
- when a prior-knowledge request to an h2c origin fails, the origin is demoted to HTTP/1.1. The
  failed request is resent once over HTTP/1.1 only when the origin never answered the preface (it
  read no request); otherwise it is not resent.

Every other client stays HTTP/1.1, as in Go, so the bidder client becomes separate from the
general client Go shares. Against an HTTP/1.1 bidder the request stays Go's, apart from the
differences the ledger records for every client (D-HDRCASE, http-client.default-accept). Against
an HTTP/2 bidder only the framing changes, never the request content or the auction result.
HTTP-H2 adds a README section that documents this. This is behaviour Go does not have, so PORTING
§1 needs the exception.

**Evidence.** The first spike's `h2` trial covered an h2c round trip on loopback, the preface-only
probe against an h2c server (SETTINGS seen) and against an HTTP/1.1 server (not seen), and a
rustls client config with ALPN `h2` then `http/1.1`. Since STACK, h2 0.4.20 is locked through
hyper 1.12.0 and reqwest 0.13.5 (with its `http2` feature), and this spike's
[h2 row](#results) shows that a direct dependency on it adds nothing.

## D-FASTGZIP: the server's gzip

**Status.** Decided by the user on 2026-11-25 (ruling 10 (b)).

**Decision.** fastgzip 0.2.0, the user's crate, writes every gzip the server writes. For
responses, a layer of pbs-server's own (`crates/server/src/compression`) replaces tower-http's
`compression-gzip`, which the workspace drops (tower-http keeps `cors`, `fs`, `limit` and
`timeout`), and decides as gziphandler v1.1.1 does: the 1,400-byte threshold, every type and
status, gziphandler's Accept-Encoding parser, `Vary: Accept-Encoding` on every answer,
Content-Encoding and Content-Length as gziphandler reads them, and a range compressed whole. The
layer buffers a body and compresses it in one call; a streamed body past 1 MiB goes out
uncompressed (`server.gzip-streams`). Each worker thread keeps one `fastgzip::Gzip` per kind of
payload (JSON, other text, the rest), used inside the request's task and never on a blocking
pool, at level 0, fastgzip's default: the pins compare decompressed bytes and the gzip header's
fixed fields, and no level writes compress/gzip's bytes. Decompression stays flate2 and reqwest's
gzip. `server.vary`, `server.gzip-negotiation` and `server.gzip-ranges` are restored;
`server.gzip-bytes`, `server.framing` and `gzip.payload-bytes` name fastgzip.

**Evidence.** Per response on an Apple M1, release build, against flate2 1.1.11 (miniz_oxide) at
its default level, which tower-http's layer used: 6.1 µs against 71.1 µs on the 67 adapter-fixture
bidder responses of 1,400 bytes or more (median 3,253 bytes), 6.4 µs against 54.8 µs on 102
auction-shaped bodies of four of them (median 2,363 bytes), and 6.8 µs against 75.3 µs on 84 of
sixteen (median 4,889 bytes), for output 7 to 13% larger. A compressor's first payload costs
about twice its later ones.

## D-TEMPLATE: Go's text/template

**Status.** Default applies (Rule B (d)).

**Go behaviour at stake.** Prebid parses and executes Go templates in two places: every bidder's
endpoint, parsed at startup (`config/bidderinfo.go:499`, `template.New("endpointTemplate")`) and
executed per request through the macros, and the user-sync iframe and redirect URLs that
`usersync/syncer.go:114-136` builds and parses. A parse error stops startup with Prebid's message
around the template's error text.

**Options.** (a) gtmpl, if it equals Go's goldens over every bidder-info template, the builtins and
their errors. (b) `pbs-gocompat` keeps only its template module, a port of the parse and execute
subset these templates use, with Go's error texts.

**Default.** (b). gtmpl fails twice:

- the gates: gtmpl 0.7.1 depends on thiserror 1.0.69, whose thiserror-impl 1.0.69 is a proc macro
  on syn 2.0.119, so syn, thiserror and thiserror-impl each appear at two versions, and it brings
  anyhow 1.0.104;
- the goldens, run outside the gates in a separate scratch project: all 646 shipped templates
  (365 bidder-info endpoints and 116 iframe and 165 redirect user-sync URLs, built as
  `buildTemplate` builds them), each executed with full and with empty data (1,292 rows), give
  Go's output, but every one of the 2,080 parse and execution errors has a different text, and
  the 4 builtin cases that print the whole data value (`{{.}}`) print it differently. Prebid
  writes those error texts into its startup errors (Rule B (d)).

`pbs-gocompat` therefore survives ruling 4 with this one module (ENT-MACROS, USERSYNC-PBS; CLEANUP
removes the rest). The module opens with `//! Glue (Rule B (d))` and has a restored ledger entry
(template.go-texts) pinned to ENT-MACROS' goldens of every shipped template and Go's error texts.

## D-XML: encoding/xml for VAST injection

**Status.** Default applies.

**Go behaviour at stake.** `injector/injector.go:116-165` reads the VAST with `xml.NewDecoder` and
`RawToken`, writes start and end elements and every other token back through `xml.Encoder`'s
`EncodeToken`, and writes trimmed character data as `<![CDATA[...]]>` itself. Errors come back as
`XML processing error: %w` around the decoder's or encoder's text. The encoder escapes attribute
values with `EscapeString` (go1.26.2's `encoding/xml/marshal.go:763` and `xml.go:1964`): `&#34;`,
`&#39;`, `&amp;`, `&lt;`, `&gt;`, `&#x9;`, `&#xA;`, `&#xD;`, and U+FFFD for a character XML does
not allow. It checks that end tags match. It would escape character data as `EscapeText` does
but without `&#xA;` (`marshal.go:225`), yet the injector never hands it any: it writes character
data itself as CDATA. No production path reaches the injector (D-DEAD).

**Default.** quick-xml 0.42.0 (default features off) for the tokens. INJECTOR writes the tokens
back the way Go's encoder does, with `EscapeString`'s escaping of attribute values and its end-tag
checks, as glue under Rule B (d) (VAST injection): quick-xml 0.42.0's `escape` writes `&quot;`,
`&apos;` and `&#13;` and leaves tab and newline alone (xml.escape, restored, pinned to a Go
golden). The decoder's error texts are ledgered (xml.error-texts). Documents one tokenizer accepts
and the other rejects are ledgered too (xml.token-acceptance): the injector is production-dead, so
no production input reaches it and Rule B (a) does not apply.

**Evidence.** quick-xml passed every gate and added only itself. Its trial counts the tags of a VAST
wrapper and pins its escaping: `<a href="x">'&'</a>` plus a tab becomes
`&lt;a href=&quot;x&quot;&gt;&apos;&amp;&apos;&lt;/a&gt;` plus the tab.

## D-DB: database-backed stored requests

**Status.** Default applies: the plan's third choice, because neither crate stack passes.

**Go behaviour at stake.** `stored_requests/backends/db_provider`, `db_fetcher` and
`events/database` go through `database/sql` with `go-sql-driver/mysql` v1.6.0 (`go.mod:18`) and
`lib/pq` v1.10.4 (`go.mod:25`). The behaviour includes DSN building (TLS parameters, timeouts),
the `$IDS` and `$LAST_UPDATED` templates, scan and NULL semantics, `sql.ErrNoRows`, and the event
poller's timeline. The two drivers speak these protocols:

- lib/pq authenticates with trust, cleartext, MD5 and SCRAM-SHA-256 (`conn.go:1192-1300`), rejects
  GSSAPI unless a provider is imported, and follows `sslmode`;
- go-sql-driver/mysql authenticates with `mysql_native_password` (SHA-1),
  `caching_sha2_password` (SHA-256, plus RSA-OAEP with SHA-1 of the password off TLS,
  `auth.go:225-226`), `sha256_password`, `mysql_clear_password` and `mysql_old_password`
  (`auth.go:239-273`), and runs queries with arguments as server-side prepared statements unless
  `interpolateParams=true` (`connection.go:198`, `:307`, `:368`).

**Options.** (a) sqlx without macros (postgres, mysql, runtime-tokio, rustls). (b) tokio-postgres
plus mysql_async. (c) Minimal clients.

**Default.** (c). STORED-DB writes a `database/sql` subset and two minimal async clients over tokio
and tokio-rustls. They take SHA-1, RSA-OAEP and PBKDF2 from aws-lc-rs (now an accepted normal
dependency), HMAC-SHA256 from hmac and sha2, and MD5 from md-5 0.11.0, all in the stack. They are
tested against a go-sqlmock-equivalent fake and Go goldens. DSN building is Prebid logic and is
ported. Runs against MySQL 8 and Postgres containers happen only when the images are already local:
STORED-DB asks before starting a container runtime and never pulls.

**Evidence.** Every crate fails the gates ([results](#results)):

- sqlx 0.9.1 adds 40 packages, with base64 0.22.1, hashbrown 0.16.1 and syn 2.0.119 as second
  versions; sqlx-macros 0.9.1 and tracing-attributes 0.1.31 are proc macros on syn 2; foldhash
  0.2.0 is licensed Zlib and webpki-roots 1.0.12 CDLA-Permissive-2.0, neither in the allow list;
- tokio-postgres 0.7.18 adds 22, with base64 0.22.1 and wasi 0.14.7 as second versions;
- mysql_async 0.37.1 adds 25, with second versions of base64, block-buffer, cpufeatures,
  crypto-common, digest, getrandom, r-efi and sha2 (the RustCrypto 0.10 line beside the stack's
  0.11), and foldhash's Zlib licence.

tokio-postgres is the nearest: relaxing `multiple_crate_versions` for base64 and wasi would admit
it, which is the user's call, not a unit's.

## D-GRPC: the ads.cert remote signer

**Status.** Default applies: the plan's fallback, because tonic fails.

**Go behaviour at stake.** `experiment/adscert/remotesigner.go:31-42` dials the signatory with
`grpc.Dial(url, grpc.WithTransportCredentials(insecure.NewCredentials()))` from grpc-go v1.83.1.
That is plaintext HTTP/2 with prior knowledge. Each `Sign` makes a unary
`SignAuthenticatedConnection` call under the configured signing timeout. grpc-go sends
`content-type: application/grpc`, `te: trailers`, `user-agent: grpc-go/1.83.1` and `grpc-timeout`
(`internal/transport/http2_client.go:583-613`). The in-process signer, adscert v0.34.0, uses
X25519 (`curve25519.X25519` and `ScalarBaseMult`, `pkg/adscert/discovery/internal_base_key.go:66`,
`:89`), HMAC-SHA256 (`pkg/adscert/signatory/signatory_local_impl.go:206`), SHA-256
(`signatory_helpers.go:21-24`) and DNS TXT lookups (D-DNS).

**Options.** (a) tonic with prost without derive. (b) A minimal client over h2.

**Default.** (b). ADSCERT writes a unary gRPC client on h2 0.4.20: prior-knowledge h2c,
length-prefixed messages, the `grpc-status` and `grpc-message` trailers, and `grpc-timeout`, with
grpc-go's header values. The messages are prost 0.14.6 `Message` implementations written by hand.
The framing below the headers (SETTINGS, window updates, HPACK, header order) is ledgered
(grpc.client-frames). X25519 and HMAC come from aws-lc-rs as a normal dependency.

**Evidence.** tonic 0.14.9 with default features off adds 7 packages, among them base64 0.22.1
beside 0.23.1 and syn 2.0.119 through pin-project-internal 1.1.13 and tracing-attributes 0.1.31.
Its default features add hyper-timeout and async-trait, and tonic with prost-derive adds anyhow,
itertools 0.15.0 and prost-derive, also on syn 2. prost 0.14.6 without derive passed and added only
prost; its trial round-trips a hand-written message. h2 as a direct dependency passed and added
nothing; its trial frames a gRPC message (a flag byte and a four-byte length, then the message) and
builds the prior-knowledge client handshake. aws-lc-rs passed RFC 7748 §6.1's X25519 vectors and
RFC 4231's first HMAC-SHA256 case and added nothing.

## D-DNS: ads.cert TXT discovery

**Status.** Default applies.

**Go behaviour at stake.** adscert's discovery calls `net.LookupTXT`, which ignores the context
(`pkg/adscert/discovery/resolver_impl.go:15`). On Unix, go1.26.2 always resolves TXT with its
pure-Go resolver (`net/lookup_unix.go:109-110`), configured from `/etc/resolv.conf`, with EDNS0
and a 1232-byte buffer (`net/dnsclient_unix.go:39`). `goLookupTXT` (`net/lookup.go:853`) joins
the character-strings of each record into one string.

**Options.** (a) hickory-resolver. (b) A minimal TXT resolver over tokio sockets.

**Default.** (a): hickory-resolver 0.26.3 with only its `tokio` feature, configured from
`/etc/resolv.conf` as parsed by resolv-conf 0.7.6 (name servers, search domains and options), the
file Go reads. ADSCERT joins each record's character-strings as Go does; that is the port, not
glue. How hickory queries, retries, sizes EDNS0 and falls back to TCP is ledgered
(dns.txt-resolver).

**Evidence.** With `default-features = false, features = ["tokio"]`, hickory-resolver passed every
gate and added 10 packages, with async-trait 0.1.92 on syn 3.0.10. The jni stand-in and sha2 0.11's
cpufeatures 0.3 removed the first spike's failures. Its `system-config` feature still fails: it
brings core-foundation 0.9.4 (through system-configuration 0.7.0) beside rustls-platform-verifier's
0.10.1. The trial builds a resolver from a resolv.conf text with two name servers and rejects a
malformed one.

## D-INFLUX: the InfluxDB reporter

**Status.** Default applies (kept by the orchestrator on 2026-11-19, ruling 5): a small
line-protocol writer, with influxdb or influxdb2 without derive as the option "if DECISIONS shows
them passing the gates". METRICS-INFLUX's plan routes the reporter's writes and pings through the
crate once this file admits one. influxdb 0.8.0 passes and is admitted, so the writes go through
it. The pings cannot (below), and the hand-written writer salvaged from wu/METRICS-GOMETRICS
(`b3c323a0`) is the fallback.

**Go behaviour at stake.** When an InfluxDB host is configured, `metrics/config/metrics.go:29`
starts `influxdb.InfluxDB(...)` from `vrischmann/go-metrics-influxdb` v0.1.1 over the
`rcrowley/go-metrics` registry. The reporter sends at every interval, pings every 5 s and recreates
the client when a ping fails (`influxdb.go:70-91`), and it logs through Go's standard `log`
package. The client is `influxdata/influxdb1-client` (`8bf82d3c094d`). `Client.Write`
(`influxdb.go:294-367`) posts line protocol to `/write` with `db`, `rp`, `precision` and
`consistency` set even when empty, an empty `Content-Type`, `User-Agent: InfluxDBClient` and basic
auth. Every point has the configured measurement (`metrics.influxdb.measurement`), and `InfluxDB`
passes no tags of its own. A counter or gauge is one point with the field `<name>.count` or
`<name>.gauge`; a meter, histogram or timer is one point per statistic, tagged
`bucket=<statistic>`, with the one float field `<name>.meter`, `<name>.histogram` or `<name>.timer`
(`influxdb.go:93-222` of go-metrics-influxdb). Points have their fields and tags sorted by key,
floats spelled by `strconv.AppendFloat(v, 'f', -1, 64)` and integers with an `i` suffix
(`models/points.go:2313`, `:2331-2335`). Every line ends with a newline (influxdb1-client's
`influxdb.go:321`), and a point with a NaN or infinite field becomes a `# ERROR: ...` comment line
(`MarshalString`, `influxdb.go:642-646`). `Ping` is `GET /ping` with the same User-Agent and
basic auth.

**Default.** METRICS-INFLUX builds each point with influxdb 0.8.0's `WriteQuery` (escaping, field
types, timestamps) and writes it through the crate's client built `with_http_client` on
pbs-http-client's reqwest client, whose default headers carry basic auth and `User-Agent:
InfluxDBClient`. It does not use `with_auth`, which sends the credentials as `u` and `p` query
parameters. It adds fields and tags in key order, so the lines match Go's. It sends `/ping` itself:
the crate's `ping` unwraps the `X-Influxdb-Build` and `X-Influxdb-Version` headers and so panics
when a server omits one, where Go reads an empty version. go-metrics' meter, timer, counter, gauge
and reservoir semantics are hand-ported, because no crate implements them. Metric names, tags and
field types stay go-metrics-influxdb's (Rule B (c)). What still differs on the wire is ledgered
(influx.write-request): the crate sends only `db` and `precision=ns`, in its hash map's order,
where Go sends `consistency`, `db`, `precision` and `rp` sorted, with `precision` empty
(`influxdb.go:342-345`); it sends no Content-Type header where Go sends an empty one; it joins the
lines without Go's final newline; and it writes a non-finite float as Rust spells it where Go
writes the `# ERROR:` line. If METRICS-INFLUX's Go goldens show a name, tag or field type the
crate cannot produce, the unit falls back to the hand-written writer and says so here.

**Evidence.** influxdb 0.8.0 with `default-features = false, features = ["rustls"]` passed every
gate. It added influxdb, lazy-regex 3.6.2 and lazy-regex-proc_macros 3.6.2 (on syn 3.0.10), and it
uses the stack's reqwest 0.13.5, adding only reqwest's `query` feature (serde_urlencoded, already
locked). Its trial pins the crate's spelling of one line, `prebidserver.requests,host=pbs-1
count=5i,m1=0.25 1700000000`; that line's shape is the trial's own, not go-metrics-influxdb's
(above). influxdb2 0.5.2 fails: it adds 68 packages and leaves 23 packages at two or three
versions (reqwest 0.11.27, hyper 0.14.32, h2 0.3.27, http 0.2.12, base64 0.13.1 and 0.21.7, and
more), proc macros on syn 1 (influxdb2-derive 0.1.1, snafu-derive 0.6.10) and syn 2
(windows-implement 0.60.2, windows-interface 0.59.3), and three advisories: RUSTSEC-2026-0543
(chrono 0.4.45, unmaintained), RUSTSEC-2026-0258 (h2 0.3.27) and RUSTSEC-2024-0384 (instant 0.1.13,
unmaintained). The crates the plan already rejected were not retried: influxdb-line-protocol is on
syn 1, and metrics-exporter-influx brings ring, a second reqwest, chrono and 8 duplicates.

## D-JSONSCHEMA: bidder-params validation

**Status.** Default applies (kept by the orchestrator on 2026-11-19, ruling 5). This spike did not
retry a schema crate, as the plan says.

**Decision.** A small hand-written draft-04 subset validator for the keywords and formats the
schemas use. It is justified twice: no schema crate passes the gates (jsonschema 0.61.2 needs
getrandom 0.3 beside the stack's 0.4; boon fails on base64 0.22, syn 2 and MIT-0), and
gojsonschema's error texts are joined into Prebid's 400 response (Rule B (d)). The user may
override it by relaxing `multiple_crate_versions`: the jsonschema crate beside a second getrandom,
with glue mapping its error kinds to gojsonschema's texts.

## D-LOGFILE: log files

**Status.** Default applies.

**Go behaviour at stake.** glog v1.2.5 writes one file per severity (INFO, WARNING, ERROR, FATAL),
named after the program, host, user, time and process, with a header and a symlink per severity.
`analytics/filesystem` writes its own file through `github.com/chasex/glog`, rotated daily.

**Default.** tracing-appender 0.2.5 (default features off): one file per appender, written by its
non-blocking worker. LOGGING builds the worker with `lossy(false)`, so a full buffer blocks the
writer as glog's synchronous writes do instead of dropping lines. glog's per-severity files, names,
headers and symlinks are ledgered (logging.files), and so are chasex/glog's rotated file names
(analytics.file-rotation). The analytics line prefix stays Go's (Rule B (b): downstream systems read
Go's and Rust's files side by side; analytics.line-prefix, restored).

**Evidence.** tracing-appender passed every gate and added itself and symlink 0.1.0. Its trial
writes a line through the non-blocking writer and reads it back after the guard drops.

## D-SIZE: go-units size strings

**Status.** Default applies: the plan's fallback, because neither crate matches Go.

**Go behaviour at stake.** `analytics/agma/agma_module.go:44` and `analytics/pubstack/config.go:46`
parse buffer sizes from config with go-units v0.4.0's `FromHumanSize`, so the parse is part of the
operator interface (Rule B (c)). `FromHumanSize` matches
`^(\d+(\.\d+)*) ?([kKmMgGtTpP])?[iI]?[bB]?$`, treats every prefix as decimal (`1KiB` is 1000),
and converts the product with `int64(float64)`, which truncates fractions and, on amd64 (D-ARCH),
turns every overflow into -9223372036854775808.

**Options.** (a) parse-size or bytesize, if it matches Go's goldens. (b) A small port.

**Default.** (b): ANALYTICS-MODULES ports `FromHumanSize` (the regular expression through
`pbs_util::goregex`, a float parse and one multiplication) as glue under Rule B (c), with a restored
ledger entry (analytics.size-strings) pinned to a Go golden like the one below.

**Evidence.** Both crates passed every gate and added only themselves, and both fail the golden of
50,105 strings ([go-units sizes](#go-goldens)). Neither can be configured into Go's meaning, because
both read a trailing `i` as binary (parse-size: "if it ends with an 'i' we always use binary
prefix"):

- parse-size 1.1.0 differs on 1,677: 235 values (`1KiB` is 1024, `1.5` rounds to 2, and
  `9223372036854775807` stays exact where Go's float conversion overflows), 38 strings only Go
  accepts (sizes past `i64::MAX`, which Go turns into the overflow value and the crate rejects or
  returns past `i64`), and 1,404 only the crate accepts (spaces, exponents, underscores, `1EB`,
  `7.`);
- bytesize 2.7.0 differs on 1,408: 141 values, 897 strings only Go accepts (`1i`, `1ib`, `1.5`,
  overflow) and 370 only the crate accepts (`+1`, `.5k`, `1EB`).

## D-PSL: the publisher domain

**Status.** Default applies.

**Go behaviour at stake.** `endpoints/openrtb2/auction.go:1609` sets `site.publisher.domain` from
golang.org/x/net v0.55.0's `publicsuffix.EffectiveTLDPlusOne` of the page URL's host and leaves it
unset when that fails. x/net compiles the list at git revision
`d6c92f1bbb7433e5db7b8405c25d4035fb8ff376` into a table, and `EffectiveTLDPlusOne` fails on an
empty label (a leading or trailing dot, two dots in a row) and when the host is itself a suffix,
such as a bare IP address.

**Options.** (a) The psl crate (in the stack), with its list differences ledgered
(psl.list-version). (b) publicsuffix 2.3.0, loading the list behind x/net v0.55.0 at startup.

**Default.** (a), with the differences GOC-PSL's goldens find on corpus domains ledgered
(psl.list-version). Neither option changes the empty-label and IP answers, which STACK's psl pin
already ledgers: both crates answer where Go fails.

**Evidence.** Over 50,450 names (every rule of x/net's list with `x.` and `y.x.` prefixes, plus
edge cases):

- psl 2.1.257 differs from Go on 252: 169 different domains, 7 domains only Go finds (such as
  `x.aivencloud.com`) and 76 only psl finds (such as `trentinosudtirol.it`). Its list is another
  snapshot than x/net's, and the 4 empty-label and IP cases below are among the 76;
- publicsuffix 2.3.0, loading x/net's list rebuilt from `publicsuffix/table_test.go`, differs on
  4: `example.com.`, `.example.com`, `a..b.com` and `127.0.0.1`, where Go fails and the crate
  answers.

publicsuffix passed every gate and added only itself (and psl-types, already locked), so option (b)
needs only this file to change and a list file to ship. The IP-address answer (psl gives `0.1` for
`127.0.0.1`) sends bidders a publisher domain Go leaves unset; the orchestrator may want to rule
whether that is Rule B (a) input.

## D-DEAD: production-dead Go

**Status.** Default applies: port.

**Go behaviour at stake.** No production path reaches:

- `injector/` (856 lines), which no package imports;
- the macros replacer and provider, which `router/router.go:262` builds
  (`macros.NewStringIndexBasedReplacer()`) and `exchange/exchange.go:83` stores in a field nothing
  reads;
- the util helpers iterutil `Firsts`, `Seconds` and `WalkGjsonLeaves`, and sliceutil
  `ContainsStringIgnoreCase`, `IndexPointerFunc` and `DeletePointerFunc`.

**Options.** (a) Port them with their tests, so that every Go test has a Rust counterpart. (b)
Leave them unported and record each one in `dead_code.tsv`, which is what PORTING §1 says today
("Do not port Go code that is dead in production").

**Default.** (a). INJECTOR uses quick-xml (D-XML) where the first version planned a `pbs-goxml`
crate. Under (a), PORTING §1 needs an exception that names this list.

## D-GOCOLL: Go runtime metrics and the GC ballast

**Status.** Default applies.

**Go behaviour at stake.** `metrics/prometheus/prometheus.go:576-585` registers client_golang
v1.12.1's `collectors.NewGoCollector()` through
`WrapRegistererWithPrefix("<namespace>_<subsystem>_")`, so `/metrics` (`server/prometheus.go:22`)
carries the Go runtime under Prebid's prefix. Measured with go1.26.2 and client_golang v1.12.1,
with the prefix `pbs_`, this is 140 families: 76 counters, 55 gauges, 8 histograms and 1 summary.
They include `pbs_go_goroutines`, `pbs_go_threads`, `pbs_go_info{version="go1.26.2"}`,
`pbs_go_gc_duration_seconds`, 24 `pbs_go_memstats_*` families, and one family per runtime/metrics
entry, 46 of them `pbs_go_godebug_non_default_behavior_*`. The set depends on the Go toolchain,
not on Prebid. Separately, `main.go:46-51` allocates `garbage_collector_threshold` bytes
(`config/config.go:36`, default 0 at `:952`) and keeps them alive to pace Go's GC.

**Options.** (a) Produce no `go_*` family; the parity harness drops `<prefix>go_*` from Go's scrape
before comparing, and dashboards built on these families need a new source. (b) Emit a
process-metrics subset under Prebid's prefix.

**Default.** (a), ledgered as metrics.go-families with status decision-pending, so the user sees
it at R1's exit. `garbage_collector_threshold` keeps Go's type, default and environment binding,
and the Rust server allocates nothing for it (config.gc-threshold, ledgered by ruling 5).

**Evidence.** The family list came from a Go program built offline against the pinned `go.mod`
(GOARCH=amd64, GOPROXY=off) that registers the collector exactly as `prometheus.go:584-585` does.

## D-PPROF: /debug/pprof

**Status.** Default applies.

**Go behaviour at stake.** `router/admin.go:18-22` registers `net/http/pprof`'s `Index`,
`Cmdline`, `Profile`, `Symbol` and `Trace` on the admin mux (`admin_port`, default 6060). Their
bodies are Go runtime data: goroutine, heap, allocs, block, mutex and threadcreate profiles, CPU
profiles and execution traces. Two other kinds of output carry Go runtime text: `debug.Stack()` in
the recovered-panic logs (`exchange/exchange.go:883`, `hooks/hookexecution/execution.go:112`) and
glog's Go `file:line` headers in module logs.

**Options.** (a) Do not serve `/debug/pprof`; the admin mux answers as for any unknown path, and the
difference is ledgered. (b) The pprof crate, if it passes the gates: CPU profiles only, in another
format.

**Default.** (a). The logs keep Go's message text, with a Rust backtrace where Go prints
`debug.Stack()`; the harness treats stack text and Go `file:line` as unreproducible and normalises
them (plan Risk 9). The pprof crate was not a candidate of this spike, so option (b) needs a trial
first.

## D-VERSION: version and revision

**Status.** Default applies.

**Go behaviour at stake.** `version.Ver` and `version.Rev` are set only through `-ldflags`
(`version/version.go`). The Dockerfile sets `Ver` to `git describe --tags` without the leading `v`
and `Rev` to `git rev-parse HEAD` (`Dockerfile:30`). At the pin those are `4.11.1-1-g4e296450` and
`4e296450de2fcc62741340b2e5f7d1df6c1744a5`. `Ver` reaches the `X-Prebid` header (`pbs-go/<Ver>`)
on auction, AMP and video responses (`endpoints/openrtb2/auction.go:191`, `amp_auction.go:155`,
`video_auction.go:173`), bidder requests (`exchange/bidder.go:215`), the agma analytics sender
(`analytics/agma/sender.go:67`) and nine adapters (nextmillennium, sharethrough, nexx360,
showheroes, mediasquare, conversant, ix, missena and rubicon). `/version`, on both the main and the
admin routers (`router/router.go:294`, `router/admin.go:25`), returns
`{"revision":...,"version":...}`, with `not-set` for an empty value (`endpoints/version.go:11`). An
empty `Ver` gives `pbs-go/unknown` in `X-Prebid` (`version/xprebidheader.go:40-45`).

**Default.** `PBS_VERSION` and `PBS_REVISION` are identical on both sides in every comparison, so
the version never shows up as a difference. A Rust build without them behaves as Go does with the
ldflags unset (FPD-SCHAIN-VERSION).

## D-ARCH: reference architecture

**Status.** Default applies: amd64.

**Go behaviour at stake.** Go's compiler fuses `x*y + z` into one FMA instruction on arm64 but not
on amd64, and Rust never contracts. For `exchange/price_granularity.go:76`
(`incrementsRounded*increment + bucketMin`), 7,151 of 200,000 sampled results differ between
GOARCH=arm64 and amd64 (measured while planning, plan §3.7). Go's pure-Go `math.Pow` (for example
`modules/scope3/rtd/masking.go:166`) gives the same results on both architectures, so it is ported,
not taken from libm.

**Options.** (a) amd64: the goldens and the Go parity server are built with GOARCH=amd64 (on an
arm64 machine under Rosetta, with `CC="clang -arch x86_64"`), and OPS-IMAGE builds amd64. Rust on
either architecture then matches. (b) arm64: Rust would have to fuse exactly where Go's SSA rules
fuse, site by site, and there is no general way to do that. (c) Both: amd64 stays the truth, and
`pbs-devtools arch-check` lists where Go on arm64 differs.

**Default.** (a). This spike's Go goldens were built that way.

**Evidence.** The measurement above. The D-51D build compiles the C with `-ffp-contract=off`, so
the 51Degrees results match the amd64 reference on any host. go-units' overflow (D-SIZE) is another
amd64-specific result.

## D-SYNC: catching up with upstream after the pin

**Status.** Default applies: ask the user at FINAL-INT.

**Go behaviour at stake.** The port covers upstream `4e296450` (2026-11-12, "New Adapter: Panxo
(#4723)"). The local `upstream/master` still points at the pin, so how far upstream has moved
since cannot be known offline. Before the pin, upstream merged 54 first-parent commits in three
months and 184 in a year. Of those 184, 62 are "New Adapter" commits, 116 touch
`static/bidder-info` and 9 touch `go.mod`. The daily port workflows are paused (plan Risk 10).

**Options.** (a) Catch up before the cut-over (SYNC-CATCHUP). (b) Cut over at the pin and catch up
afterwards. (c) Stay at the pin.

**Default.** Ask at FINAL-INT, after the final gate holds at the pin.

**What catching up involves.**

1. Fetch upstream, which needs the network and the user's consent, and list the first-parent
   commits after `4e296450`. The orchestrator splits them into four chunks by the crates
   `port_map.json` maps their files to.
2. Each chunk is an agent unit with a review: read the Go diff, regenerate the Go goldens at the
   chunk's last commit (GOARCH per D-ARCH, TZ=UTC, the toolchain the new `go.mod` needs, a module
   cache filled while online), port, and run UNIT-CHECKS.
3. Moving the pin moves everything pinned to it:
   - 16 of the 22 Go modules on wu/r1-int require `github.com/prebid/prebid-server/v4
     v4.11.2-0.20261112141353-4e296450de2f`, and each one needs the new pseudo-version;
   - `upstream-seams.patch` (56 files, on rs/parity-harness) is re-applied and re-checked;
   - `port_map.json`, `map_ranges.tsv`, `float_sites.tsv` and the subtest ledger are regenerated;
   - every dependency `go.mod` bumps is a behaviour change to port, cited by version. For example,
     a new device-detection-go changes the D-51D SHA-256 pins.
4. The final gate runs again at the new pin. `UPSTREAM_SYNC` moves, and so does the D-VERSION
   `Ver` string. The port system (PORT-SYS-CRATES) is re-enabled if the user agrees, and the
   orchestrator checks the remote heads before it pushes.

At the past rate of about four commits a week, mostly adapters and bidder-info, a catch-up after
*n* weeks covers about 4*n* commits.

## Crates without a decision of their own

### lru: golang-lru's simplelru

`fetcher/cache/lru.go` uses `hashicorp/golang-lru/v2` v2.0.7's `New`, `Get`, `Add` and `Remove`.
lru 0.19.0 with default features off (the defaults pull foldhash, licensed Zlib) passed every gate,
added only itself, and matched Go on all 24,000 steps of the golden (8 capacities, 3,000 random
operations each, comparing each result and the key order). Go's `New` fails on a size below 1
("must provide a positive size"), and `NewLRUCache` returns that error (`fetcher/cache/lru.go:35`);
FETCHER returns it where `NonZeroUsize` rejects the capacity. FETCHER uses lru, not moka, and
ledgers nothing for it.

### getopts: glog's flags

Prebid's binary registers exactly glog v1.2.5's nine flags (`alsologtostderr`, `log_backtrace_at`,
`log_dir`, `log_link`, `logbuflevel`, `logtostderr`, `stderrthreshold`, `v`, `vmodule`), which
`main.go:28` parses with `flag.Parse`; a golden program importing what `main.go` imports listed
them. getopts 0.2.24 with `long_only(true)`, `ParsingStyle::StopAtFirstFree`, every flag as
`Occur::Multi`, bools as `HasArg::Maybe`, the rest as `HasArg::Yes` and the one-letter `v`
registered as a short name tokenises as Go's `flag` does. The golden isolates tokenisation: it
registers the nine flags with glog's bool-ness and values that record the raw string and never
fail. Over 20,018 argument lists getopts agreed with Go on the outcome (parsed, help or error) of
every list, and for all 12,023 parsed lists on the values in order and the arguments left. Of
the 7,995 lists that end in help or an error in both, 3,125 differ only in the values set before
it, because getopts reports a failure without the matches before it. `-h` and `-help` stay
unregistered, so getopts stops at them in order, as Go does.

LOGGING's glue (Rule B (c), logging.flags) turns the matches into Go's results: the last of
repeated values wins (`opt_strs_pos`), bools parse as `strconv.ParseBool`, and each value is
checked in argument order, as Go's `Set` runs while it parses. A bad value before a help or an
undefined flag therefore decides the outcome (Go exits 2 on `-logtostderr=x -h`, not 0), so the
glue re-parses the arguments before a failing one. The usage and error texts are ledgered
(logging.flag-texts). getopts' `unicode` feature resolves to the locked unicode-width 0.2.3, as the
plan required.

### multer: FormValue at /optout

`pbs/usersync.go:70-71` reads `optout` and `g-recaptcha-response` with `FormValue`, which parses
the query and an urlencoded or multipart body (`ParseMultipartForm` with 32 MiB in memory) and
ignores parse errors. multer 3.1.0 with default features off passed every gate and added itself
and spin 0.9.9; its trial reads both fields from a multipart body. USERSYNC-PBS uses it for
multipart bodies and form_urlencoded for the rest (ruling 5), so url.multipart-form is not needed.
Where multer and Go's `mime/multipart` disagree it is ledgered (url.multipart-edges): for example,
Go decodes a quoted-printable part transparently and multer does not, and their part, header and
size limits differ.

### json-patch: MergePatch

`util/jsonutil/merge.go:165`, `endpoints/openrtb2/auction.go:1711-1722` and ten other files call
evanphx/json-patch v5.9.0's `MergePatch`. json-patch 0.2.7 with default features off (merge only,
ruling 9) passed every gate and added only itself, so JSONUTIL uses it instead of the hand-written
RFC 7386 merge. Over 50,033 document and patch pairs it matched Go on 45,403 and differed on
4,630, in three ways, each with its own entry and pin as the plan asks of JSONUTIL:

- 730 patches with nulls inside arrays (json.merge-patch-quirks, ledgered): Go deletes null
  members of objects inside an array it adds or replaces (`{"a":[{"b":null}]}` gives
  `{"a":[{}]}`), json-patch keeps them;
- 981 null documents (json.merge-patch-null-document, decision-pending): Go fails with
  `ErrBadJSONDoc`, `Invalid JSON Document` (`merge.go:127-129` of json-patch.v5), json-patch
  merges;
- 2,919 patches that are null, a string, a number or a boolean (json.merge-patch-scalar-patch,
  decision-pending): Go fails with `ErrBadJSONPatch`, `Invalid JSON Patch` (`merge.go:131-133`,
  `:143-149`), json-patch returns the patch.

The two failures are more than library wording, because Prebid branches on them. A string, number
or boolean at `ext.prebid.bidderconfig[].config.ortb2.user.ext` (or `site.ext`, `app.ext`), merged
into a non-empty ext of the request, fails `jsonutil.MergeClone` (`util/jsonutil/merge.go:165-172`,
`firstpartydata/first_party_data.go:267`), `formatMergeCloneError` turns the failure into
`invalid first party data ext` (`first_party_data.go:20`, `:348-353`), and the exchange answers
400 with it (`exchange/exchange.go:302-310`). A Go program built against the pinned `go.mod` shows
it through `firstpartydata.ResolveFPD`: with the request's `user.ext` `{"consent":"x"}`, the
bidder configs `{"ext":"abc"}`, `{"ext":5}` and `{"ext":true}` each give `[invalid first party
data ext]`, while `{"ext":null}` keeps `{"consent":"x"}`, `{"ext":{"b":1}}` merges and
`{"ext":[1]}` replaces. json-patch would set `user.ext` to the string, so the Rust server would
not answer with Prebid's 400 there. Both entries are therefore decision-pending, so the user sees
them at R1's exit: json-patch's results apply until the user decides, and the restore option is a
check in front of the merge that returns Go's errors in Go's order (document syntax, patch syntax,
null document, then a null or non-container patch). Go also fails on bytes that are not JSON, such
as the empty RawMessage of a missing ext (`merge.go:119-125`); that is no json-patch difference,
because json-patch never sees unparsed bytes.

json-patch 5.0.0 gave the same results on every pair.

### arc-swap, thiserror and similar

arc-swap 1.9.5 (default features) added only itself; it holds state a background task replaces
while requests read it (currency rates, vendor lists, stored data), as PORTING §7 suggests.
thiserror 2.0.21 added nothing (prometheus and quinn lock it) and serves the error types of
pbs-errortypes and the crates; the texts are written to match Prebid's. similar 3.2.1 with only its
`text` feature added only itself (its defaults add bstr); ADP-HARNESS prints adapterstest's
mismatch diffs with it where Go uses `gojsondiff` (`adapters/adapterstest/test_json.go:417`) and
compares the JSON as `serde_json::Value`s. None of them changes what the server does.

### tower-http's fs feature: /static

`router/router.go:295` serves `/static/*filepath` with httprouter's `ServeFiles` over
`http.FileServer`. tower-http's `fs` feature (`ServeDir`) passed every gate and added
http-range-header 0.4.2, mime_guess 2.0.5 and unicase 2.9.0. Its trial pins the answers that
differ from Go's: a directory without `index.html` gets 404 where `FileServer` lists it, and a
missing file gets 404, with the empty body of `ServeDir`'s `not_found`, where Go writes `404 page
not found`. Content types come from mime_guess's table: `optout.html` is served as `text/html`,
where Go's table gives `text/html; charset=utf-8`, and Go sniffs content whose extension it does
not know. SERVER-AXUM ledgers these and the redirect and conditional-request edges it finds
(server.static-files, `crates/server/tests/static.rs`), and ROUTER-MAIN mounts the service at
`/static`.

### criterion and dhat: benches

criterion 0.8.2 with default features off passed as a dev dependency with `half` held at 2.6.0 and
added 24 packages. Resolved fresh it picks half 2.7.1, which needs zerocopy, and zerocopy-derive
0.8.63 is on syn 2. dhat 0.3.3 fails R1 check 10: it locks rustc-hash 1.1.0 beside 2.1.3. See
[D-COW](#d-cow-shared-request-data).

### notify: 51Degrees file watching

notify 8.2.2 with default features off passed the lock gates (9 packages, no second version) but
fails cargo deny: its own licence is CC0-1.0, outside the allow list. It is not needed: Go's
watcher polls, and GOC-51D-ONPREMISE ports the polling loop (see
[D-51D](#d-51d-51degrees-device-detection)).

### aws-lc-rs as a normal dependency

aws-lc-rs 1.19.0 (`alloc`, `aws-lc-sys`) as a normal dependency added nothing: rustls links it into
every build already. It covers X25519 and HMAC-SHA256 for ADSCERT and SHA-1, RSA-OAEP and PBKDF2 for
STORED-DB, as the first spike found.

### Go libraries that need no crate

- `github.com/alitto/pond` v1.8.3 (`floors/fetcher.go:102`): FLOORS uses a tokio `JoinSet` with a
  `Semaphore`.
- `github.com/benbjohnson/clock` v1.3.0 (analytics and the in-process ads.cert signer): a `Clock`
  trait where Go injects `clock.Clock`, and tokio's paused clock in tests (UTIL-CRATES).
- `github.com/yudai/gojsondiff` v1.0.0: similar, above.

## The second dependency spike

### Method

The scratch workspace was a `git archive` of wu/r1-int at `df5ab5b4`: STACK's merge (`acbe1855`)
and the commit that holds the unused stack entries in a commented block. It kept the real
`Cargo.toml` lint table, `clippy.toml`, `deny.toml`, `rustfmt.toml`, `taplo.toml`, `Cargo.lock` and
`patches/` (the jni 0.22.4, webpki-root-certs 1.0.12 and ring 0.17.14 stand-ins). Nothing from it is
committed.

One scratch crate, `pbs-decisions-spike` with `[lints] workspace = true`, named every entry of the
waiting block but `cc` (base64, futures-util, hmac, hyper-util, ipnet, md-5, rand, semver, toml and
uuid as normal dependencies, aws-lc-rs and rcgen as dev dependencies), so that each trial resolved
against the workspace as it will be once those crates are in use. That base added 15 packages to
STACK's lock. `cc` stayed out because of the machete finding. Each candidate added one module
that calls the candidate from non-test code, which makes it a normal dependency (clippy's
`multiple_crate_versions` follows only normal edges), and tests, some of which replay Go goldens.

Each trial ran these gates, all offline (`--offline --locked`, proxies at `127.0.0.1:9`):

1. `Cargo.lock` against STACK's: additions only;
2. R1 check 10: one version per package in `Cargo.lock` and in `cargo tree --workspace
   --all-features -e normal,build,dev --target all`;
3. every proc-macro crate in the graph depends on syn 3.0.10 and no other syn;
4. `cargo deny --offline --locked check --deny warnings` (advisories, bans, licences, sources),
   with no exception;
5. `cargo build` and `cargo clippy -- -D warnings` (the real lint table: pedantic, nursery and
   cargo, `multiple_crate_versions` included) with `--all-targets --all-features`, `cargo test
   --all-features`, and `cargo doc --no-deps --all-features` with `RUSTDOCFLAGS=-D warnings`;
6. `cargo +nightly-2026-11-13 fmt --check`, `taplo fmt --check` and `cargo machete
   --with-metadata`;
7. `scripts/check-no-lint-suppression.sh --root <scratch workspace>`;
8. cargo itself prints no warning (R1 check 17).

A trial that failed a lock gate (1 to 4) skipped the compile gates. The union ran at workspace
scope: the same gates plus the default-feature build and clippy, plain `cargo test`, `cargo run -p
pbs-devtools -- all --check`, `scripts/cargo-machete.sh`, `scripts/tests/run.sh` and
`scripts/lint-shell.sh`. The tools were rustc and cargo 1.100.0, rustfmt from nightly-2026-11-13,
cargo-deny 0.20.2, cargo-machete 0.9.2, taplo 0.10.0 and shellcheck 0.11.0, on an
aarch64-apple-darwin host.

### The fetch

The spike's only network use was one crates.io session, from 2026-11-19T04:03:24Z to 04:08:32Z: 42
manifest variants (the candidates, several with both default and minimal features) resolved online,
which filled the local registry cache. Seven more variants (config `toml` and `ini`, criterion,
dhat, hickory-resolver with `tokio` only, influxdb with `rustls` only, similar with `text` only)
then resolved offline from that cache. Everything else ran offline.

### Results

"Added" counts packages added to the base lock. "Lock gates" are gates 1 to 4; a candidate that
passes them and every compile gate "passes".

| Candidate | Version and features | Added | Result | For | Ledger entries |
|---|---|---|---|---|---|
| gtmpl | 0.7.1, default features off; gtmpl_value 0.5.1 | 6: anyhow 1.0.104, gtmpl 0.7.1, gtmpl_value 0.5.1, syn 2.0.119, thiserror 1.0.69, thiserror-impl 1.0.69 | fails: syn, thiserror and thiserror-impl at two versions; thiserror-impl 1.0.69 on syn 2 | D-TEMPLATE | rejected; its goldens: outputs equal, all 2,080 error texts differ |
| quick-xml | 0.42.0, default features off | 1: quick-xml 0.42.0 | passes | D-XML | xml.escape (restored, d); xml.error-texts; xml.token-acceptance |
| sqlx | 0.9.1, default features off; mysql, postgres, runtime-tokio, tls-rustls-aws-lc-rs | 40 | fails: base64 0.22.1, hashbrown 0.16.1 and syn 2.0.119 as second versions; sqlx-macros and tracing-attributes on syn 2; licences Zlib (foldhash 0.2.0) and CDLA-Permissive-2.0 (webpki-roots 1.0.12) | D-DB | rejected |
| tokio-postgres | 0.7.18, default features off; runtime | 22 | fails: base64 0.22.1 and wasi 0.14.7 as second versions | D-DB | rejected |
| mysql_async | 0.37.1, default features off | 25 | fails: base64, block-buffer, cpufeatures, crypto-common, digest, getrandom, r-efi and sha2 as second versions; licence Zlib (foldhash 0.2.0) | D-DB | rejected |
| tonic | 0.14.9, default features off | 7: base64 0.22.1, pin-project 1.1.13, pin-project-internal 1.1.13, syn 2.0.119, tokio-stream 0.1.19, tonic 0.14.9, tracing-attributes 0.1.31 | fails: base64 and syn as second versions; pin-project-internal and tracing-attributes on syn 2 | D-GRPC | rejected |
| prost | 0.14.6, default features off; std | 1: prost 0.14.6 | passes | D-GRPC | none |
| h2 (direct) | 0.4.20 | 0 | passes | D-GRPC's client | grpc.client-frames |
| hickory-resolver | 0.26.3, default features off; tokio; with resolv-conf 0.7.6 | 10: async-trait 0.1.92, critical-section 1.2.0, data-encoding 2.11.1, either 1.19.0, futures-io 0.3.34, hickory-net 0.26.3, hickory-proto 0.26.3, hickory-resolver 0.26.3, prefix-trie 0.8.4, resolv-conf 0.7.6 | passes | D-DNS | dns.txt-resolver |
| hickory-resolver | 0.26.3, default features off; system-config, tokio | 19 | fails: core-foundation 0.9.4 beside 0.10.1 | D-DNS | rejected |
| aws-lc-rs (normal) | 1.19.0, default features off; alloc, aws-lc-sys | 0 | passes (RFC 7748 §6.1, RFC 4231 case 1) | ADSCERT, STORED-DB | none |
| tracing-appender | 0.2.5, default features off | 2: symlink 0.1.0, tracing-appender 0.2.5 | passes | D-LOGFILE | logging.files; analytics.file-rotation |
| notify | 8.2.2, default features off | 9: inotify 0.11.5, inotify-sys 0.1.8, kqueue 1.2.1, kqueue-sys 1.1.2, notify 8.2.2, notify-types 2.3.1, same-file 1.0.6, walkdir 2.5.0, winapi-util 0.1.11 | fails cargo deny: notify's licence is CC0-1.0 | 51Degrees file watching | rejected (Go polls) |
| arc-swap | 1.9.5 | 1: arc-swap 1.9.5 | passes | refreshed shared state | none |
| criterion (dev) | 0.8.2, default features off; half held at 2.6.0 | 24: alloca 0.4.0, anes 0.1.6, cast 0.3.0, ciborium 0.2.2, ciborium-io 0.2.2, ciborium-ll 0.2.2, clap 4.6.8, clap_builder 4.6.7, clap_lex 1.1.2, criterion 0.8.2, criterion-plot 0.8.2, crunchy 0.2.4, either 1.19.0, half 2.6.0, itertools 0.13.0, oorandom 11.1.5, page_size 0.6.0, same-file 1.0.6, tinytemplate 1.2.1, walkdir 2.5.0, winapi 0.3.9, winapi-i686-pc-windows-gnu 0.4.0, winapi-util 0.1.11, winapi-x86_64-pc-windows-gnu 0.4.0 | passes | COW's benches | none |
| criterion (dev), resolved fresh | 0.8.2, default features off | 27, with half 2.7.1 and syn 2.0.119 | fails: zerocopy-derive 0.8.63 on syn 2 | COW's benches | needs the half pin |
| dhat (dev) | 0.3.3 | 9: addr2line 0.27.1, backtrace 0.3.77, dhat 0.3.3, gimli 0.34.0, mintex 0.1.4, object 0.40.0, rustc-demangle 0.1.28, rustc-hash 1.1.0, thousands 0.2.0 | fails check 10: rustc-hash 1.1.0 beside 2.1.3 | COW's allocation counts | rejected |
| tower-http `fs` | 0.6.11 with fs | 3: http-range-header 0.4.2, mime_guess 2.0.5, unicase 2.9.0 | passes | `/static` | server.static-files |
| similar | 3.2.1, default features off; text | 1: similar 3.2.1 | passes | adapterstest diffs | none |
| parse-size | 1.1.0 | 1: parse-size 1.1.0 | passes the gates; differs from go-units on 1,677 of 50,105 | D-SIZE | not adopted |
| bytesize | 2.7.0, default features off | 1: bytesize 2.7.0 | passes the gates; differs from go-units on 1,408 of 50,105 | D-SIZE | not adopted |
| thiserror | 2.0.21 | 0 | passes | error types | none |
| config `toml` | 0.15.27 with json, toml | 0 | passes | D-VIPERFMT | config.formats; config.toml-reader |
| config `ini` | 0.15.27 with json, ini | 9: const-random 0.1.18, const-random-macro 0.1.16, crunchy 0.2.4, dlv-list 0.5.2, getrandom 0.2.17, hashbrown 0.14.5, ordered-multimap 0.7.3, rust-ini 0.21.3, tiny-keccak 2.0.2 | fails: getrandom and hashbrown as second versions; licence CC0-1.0 (tiny-keccak 2.0.2) | D-VIPERFMT | rejected (INI fails startup, config.formats) |
| lru | 0.19.0, default features off | 1: lru 0.19.0 | passes; exact on the golden | FETCHER | none |
| publicsuffix | 2.3.0, default features off; std | 1: publicsuffix 2.3.0 | passes | D-PSL's option | psl.list-version restored, if adopted |
| getopts | 0.2.24 (default unicode feature) | 1: getopts 0.2.24 | passes | LOGGING | logging.flags (restored, c); logging.flag-texts |
| influxdb | 0.8.0, default features off; rustls | 3: influxdb 0.8.0, lazy-regex 3.6.2, lazy-regex-proc_macros 3.6.2 | passes | D-INFLUX | influx.write-request |
| influxdb2 | 0.5.2, default features off | 68 | fails: 23 packages at second or third versions; syn 1 and syn 2 proc macros; advisories RUSTSEC-2026-0543, RUSTSEC-2026-0258, RUSTSEC-2024-0384 | D-INFLUX | rejected |
| multer | 3.1.0, default features off | 2: multer 3.1.0, spin 0.9.9 | passes | `/optout` | url.multipart-edges |
| json-patch | 0.2.7, default features off | 1: json-patch 0.2.7 | passes | JSONUTIL (ruling 9) | json.merge-patch-quirks; json.merge-patch-null-document and json.merge-patch-scalar-patch (decision-pending) |
| json-patch | 5.0.0, default features off | 2: json-patch 5.0.0, jsonptr 0.8.2 | passes; merges as 0.2.7 does | JSONUTIL | not adopted |
| jsonschema | not retried | | | D-JSONSCHEMA | |

### The union run

Every gate-passing candidate together, at workspace scope: quick-xml, prost, aws-lc-rs, arc-swap,
thiserror, similar, parse-size, bytesize, lru, getopts, json-patch 0.2.7, publicsuffix, config
`toml`, tracing-appender, multer, tower-http `fs`, hickory-resolver with resolv-conf, influxdb,
criterion and h2 (json-patch 5.0.0 would be a second version of json-patch). It added 53 packages
to the base lock, 68 to STACK's. The `Cargo.lock` diff against STACK's has 692 added lines (624 of
them not blank) and none removed. Every gate passed. The tests ran in 102 test targets (78 test
binaries and 24 doc-test runs): 1,154 passed with `--all-features` and 1,131 with plain `cargo
test`, and none failed. (The driver's `result.json` keeps only the last 60 result lines, which hold
967 of them.) The scratch copy is not a git repository, so `scripts/lint-shell.sh` ran on a
throwaway `git init` index of it (55 scripts, shellcheck 0.11.0).

The 53 packages: alloca 0.4.0, anes 0.1.6, arc-swap 1.9.5, async-trait 0.1.92, bytesize 2.7.0,
cast 0.3.0, ciborium 0.2.2, ciborium-io 0.2.2, ciborium-ll 0.2.2, clap 4.6.8, clap_builder 4.6.7,
clap_lex 1.1.2, criterion 0.8.2, criterion-plot 0.8.2, critical-section 1.2.0, crunchy 0.2.4,
data-encoding 2.11.1, either 1.19.0, futures-io 0.3.34, getopts 0.2.24, half 2.6.0, hickory-net
0.26.3, hickory-proto 0.26.3, hickory-resolver 0.26.3, http-range-header 0.4.2, influxdb 0.8.0,
itertools 0.13.0, json-patch 0.2.7, lazy-regex 3.6.2, lazy-regex-proc_macros 3.6.2, lru 0.19.0,
mime_guess 2.0.5, multer 3.1.0, oorandom 11.1.5, page_size 0.6.0, parse-size 1.1.0, prefix-trie
0.8.4, prost 0.14.6, publicsuffix 2.3.0, quick-xml 0.42.0, resolv-conf 0.7.6, same-file 1.0.6,
similar 3.2.1, spin 0.9.9, symlink 0.1.0, tinytemplate 1.2.1, tracing-appender 0.2.5, unicase
2.9.0, walkdir 2.5.0, winapi 0.3.9, winapi-i686-pc-windows-gnu 0.4.0, winapi-util 0.1.11 and
winapi-x86_64-pc-windows-gnu 0.4.0. The proc macros in the final graph (async-trait,
lazy-regex-proc_macros and the stack's) all depend on syn 3.0.10.

### Go goldens

Each golden is a Go program in a scratch module that requires the pinned upstream module through a
`replace` and reuses its `go.sum`, built offline with go1.26.2, GOARCH=amd64 (D-ARCH), GOPROXY=off
and TZ=UTC. The Rust side replayed the recorded JSON lines in the candidate's trial.

| Golden | Go | Corpus | Result |
|---|---|---|---|
| go-units sizes | go-units v0.4.0 `FromHumanSize` | 50,105 strings | parse-size: 18,248 equal, 30,180 rejected by both, 1,677 differ; bytesize: 17,483 equal, 31,214 rejected by both, 1,408 differ |
| LRU | golang-lru/v2 v2.0.7 `New`, `Get`, `Add`, `Remove` | 8 capacities, 24,000 steps | lru: no difference |
| merge patch | evanphx/json-patch v5.9.0 `MergePatch` | 50,033 pairs | json-patch 0.2.7 and 5.0.0: 45,403 equal, 4,630 differ (730 values, 981 null documents, 2,919 null or scalar patches; Go fails on the last two) |
| publicsuffix | x/net v0.55.0 `EffectiveTLDPlusOne` | 50,450 names | psl 2.1.257: 252 differ; publicsuffix 2.3.0 on x/net's list: 4 differ |
| flags | `flag` with glog's nine flags | 20,018 argument lists | getopts: the outcome equal on all; values and arguments equal on all 12,023 parsed lists |
| templates | `text/template` as `config/bidderinfo.go` and `usersync/syncer.go` use it | 3,386 rows: 646 shipped templates executed with full and empty data (1,292), 2,055 mutants, 39 builtin cases | gtmpl 0.7.1: every output equal except 4 `{{.}}` cases; every one of the 2,080 error texts differs |

## Changes outside this file

These files are not owned by this unit. Each change follows from a decision above.

- `Cargo.toml` (`[workspace.dependencies]`, orchestrator): the accepted lines join the waiting
  block; `config` gains `toml` and `tower-http` gains `fs` when CONFIG-LOADER and SERVER-AXUM take
  them; `aws-lc-rs`'s comment loses "Dev-only" when ADSCERT or STORED-DB moves it up.
- `Cargo.lock` (orchestrator): when criterion joins, `cargo update -p half --precise 2.6.0`, and
  every later regeneration keeps `half` at 2.6.0. A fresh resolution picks 2.7.1 and fails.
- `scripts/cargo-machete.sh` (orchestrator, before GOC-51D-DD): `cargo machete --with-metadata`
  reports a build dependency that only `build.rs` uses as unused. Reproduced in the scratch
  workspace with `cc = { workspace = true }` under `[build-dependencies]` and a `build.rs` calling
  `cc::Build::new()`: "cargo-machete found the following unused dependencies ... cc". Plain `cargo
  machete` sees `build.rs` but misreads renamed packages (it flags md-5, whose library is `md5`).
  The script needs to check build dependencies against `build.rs` itself; a
  `[package.metadata.cargo-machete]` ignore list would be a suppression.
- `PORTING.md`: §5 ("No `unsafe`") gains the D-51D exception for `crates/fiftyone-dd/`; §1 ("Do
  not add behaviour Go does not have") gains the HTTP/2 exception; §1 ("Do not port Go code that is
  dead in production") gains the D-DEAD list, as long as D-DEAD stays "port"; §7 ("Count
  allocations with `dhat`") cannot stand while dhat fails R1 check 10 (see D-COW).
- `scripts/check-no-lint-suppression.sh`: a `rust-unsafe` rule flags `unsafe` in any Rust file
  outside `crates/fiftyone-dd/`. The `crate-lints` rule accepts that crate's own `[lints]` table
  only when it equals the workspace table without `unsafe_code`, plus
  `undocumented_unsafe_blocks`, `multiple_unsafe_ops_per_block`, `missing_safety_doc` and
  `unsafe_op_in_unsafe_fn` at forbid. Fixture tests prove both rules. GOC-51D-DD specifies this
  work. Without it, the crate cannot exist: `unsafe_code = "forbid"` is in the workspace table, and
  `crate-lints` accepts nothing but `workspace = true`.
- `deny.toml` and `NOTICE`: `[licenses.private] ignore = false` makes cargo deny check workspace
  crates' licences. If `pbs-fiftyone-dd` declares the code it links (`license = "Apache-2.0 AND
  EUPL-1.2"`), `EUPL-1.2` joins `[licenses] allow` in the same commit and no earlier, because
  `unused-allowed-license = "deny"` rejects an allowance nothing uses. Either way, NOTICE carries
  the EUPL Article 5 attribution that the 51Degrees sources ask of network applications.
- The plan's library map follows this file: lru for golang-lru, a port for go-units, influxdb for
  the InfluxDB writes, h2 and prost for gRPC, minimal clients for the databases, `pbs-gocompat`'s
  template module for `text/template`, and no crate for the 51Degrees watcher.
- The plan (orchestrator), D-DECODE and JSONUTIL: "a json.RawMessage given null stays Some(null)
  through raw_keep_null, as Go keeps the bytes" holds only where Prebid decodes with
  `encoding/json`; jsonutil's decoders leave the field nil (the correction under
  [D-DECODE](#d-decode-json-decoding)). JSONUTIL's merge-patch ledger row gains
  json.merge-patch-null-document and json.merge-patch-scalar-patch as decision-pending, unless the
  orchestrator rules that Rule B (d) restores them, since Go answers them with Prebid's own 400
  ([json-patch](#json-patch-mergepatch)).
- wu/SPIKE-DEPS adds `docs/DECISIONS.md` too. Merging it after this branch conflicts on that file
  (both add it); the resolution keeps this version.
