# Decisions

This file records the twelve decisions of plan principle 20 and the HTTP/2 decision, with the
dependency spikes behind them. It was written on 2026-11-18 against upstream
`4e296450de2fcc62741340b2e5f7d1df6c1744a5`, the commit `UPSTREAM_SYNC` pins (`git describe
--tags`: `v4.11.1-1-g4e296450`). Go paths are relative to the Go root (PORTING §2) and every
`file:line` is at that commit. A module path names the version the pinned `go.mod` requires.

Each entry gives the status, the Go behaviour at stake, the options, the decision or default, and
the evidence. Three entries are the user's decisions of 2026-11-18. The others are open, and a
gated unit takes the default until the user decides. Principle 20 gives no default for D-GOCOLL,
D-PPROF or D-VERSION, so the defaults given for them below are proposals and are marked as such.

## Summary

| Decision | Status | Decision or default | Units it gates |
|---|---|---|---|
| D-51D | decided | The full 51Degrees engine through FFI to the pinned EUPL-1.2 C | GOC-51D-DD, GOC-51D-ONPREMISE, MOD-51D |
| D-WURFL | decided | The whole module over a `WurflApi` trait; the libwurfl binding waits on the user | MOD-WURFL |
| HTTP/2 | decided | Automatic for bidder requests, with no configuration | NET-H2, EXCH-BIDDER, ROUTER-MAIN, PH-PROFILES, PH-WIRE-COVER |
| D-DEAD | open | Port production-dead Go with its tests | INJECTOR, PORTED-GAPS |
| D-VIPERFMT | open | Port viper's TOML, HCL, properties, dotenv and INI readers | GOC-VIPER-FMT |
| D-GOCOLL | open | Proposed: no `go_*` families; `garbage_collector_threshold` parsed, nothing allocated | GOC-PROMHTTP, ROUTER-MAIN |
| D-PPROF | open | Proposed: keep the five `/debug/pprof` routes, reproduce what is not Go runtime data, 501 for the rest | ROUTER-MAIN |
| D-INFLUX | open | Port the go-metrics engine and its InfluxDB reporter | METRICS-GOMETRICS |
| D-DB | open | Port both SQL drivers as minimal clients; containers only if the images are local | STORED-DB |
| D-GRPC | open | Port: a minimal unary gRPC client over `h2` | ADSCERT |
| D-VERSION | open | Proposed: Go's `Ver` for the pin everywhere; Go's `Rev` in golden and parity builds, the Rust commit in release images | FPD-SCHAIN-VERSION, OPS-IMAGE, ROUTER-MAIN |
| D-ARCH | open | amd64 | Go goldens, the Go parity server, OPS-IMAGE |
| D-SYNC | open | Ask the user at FINAL-INT | SYNC-CATCHUP |

The [dependency spikes](#dependency-spikes) section says how each candidate stack was tried and
what it showed; [changes outside this file](#changes-outside-this-file) lists what other files
need because of these decisions.

## D-51D: 51Degrees device detection

**Status.** Decided by the user on 2026-11-18: implement the engine fully, with `unsafe` allowed
only for the EUPL code. The plan's default (an engine trait only) no longer applies.
GOC-51D-DD and GOC-51D-ONPREMISE (T4) replace GOC-51D-ENGINE, and MOD-51D has no exemptions.

**Go behaviour at stake.** `modules/fiftyonedegrees/devicedetection` is in Go's default build and
links `github.com/51Degrees/device-detection-go/v4` v4.4.35 (`go.mod:6`) through cgo. The engine
is `dd/device-detection-cxx.c` (21,925 lines) and `dd/device-detection-cxx.h` (9,196 lines),
EUPL-1.2, compiled with cgo's default `-g -O2` plus `dd/cgoflags.go:31-33`: `#cgo CFLAGS:
-fcommon`, `#cgo LDFLAGS: -lm` and `#cgo !darwin LDFLAGS: -latomic`. The `onpremise` package loads
and reloads the data file, pulls updates (it checks `Content-MD5` with `crypto/md5`,
`onpremise/file_pulling.go:199-203`) and watches the file. The module reads the engine's
properties into `device` and `device.ext`. The Go module ships a test data file,
`onpremise/mock_hash.gz` (30,896,373 bytes).

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

**Evidence.** The `cc` spike passed every gate. Its build script used `cc` 1.11.2 and `sha2`
0.10.9 as build dependencies. Both were already in the lock (`cc` through `aws-lc-sys`, `sha2`
through `pbs-devtools` and `pbs-parity`), so the lock gained only the spike crate. The script read
the files from the Go module cache (`$GOMODCACHE`, `$GOPATH/pkg/mod` or `~/go/pkg/mod`, under
`github.com/51!degrees/device-detection-go/v4@v4.4.35`), checked both hashes, and built
`libfiftyone_dd.a` with `opt_level(2)`, `debug(true)`, `-fcommon` and `-ffp-contract=off`. It
linked `-lm`, plus `-latomic` everywhere except macOS. A test confirmed that the archive defines
six entry points the `dd` package calls (`fiftyoneDegreesHashInitManagerFromFile`,
`...HashReloadManagerFromOriginalFile`, `...ResultsHashFromEvidence`,
`...ResultsHashGetValuesString`, `...ResourceManagerFree` and `...EvidenceAddString`). The spike
never calls the C, so it needs no `unsafe`. Clippy accepted the build script's `cargo::` lines,
because `print_stdout` does not apply to build scripts.

**What else must change.** The changes are listed under [changes outside this
file](#changes-outside-this-file): the scanner's `unsafe` and `crate-lints` rules (designed in
GOC-51D-DD), PORTING §5, the crate's licence metadata against `deny.toml`, and NOTICE. No Rust
crate offers MD5 offline (`md-5` 0.10.6 is in the index but not downloaded), so the `Content-MD5`
check needs a port of Go's `crypto/md5`. lib/pq needs the same port (see D-DB).

## D-WURFL: ScientiaMobile WURFL

**Status.** Decided by the user on 2026-11-18: port the whole module over a `WurflApi` trait.
The libwurfl binding waits on the user. The plan's default (only the default `!wurfl` build) no
longer applies.

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

## HTTP/2 to bidders (decided; not one of the twelve)

**Status.** Decided by the user on 2026-11-18 ("support http2 and h2c based if the buyers
support it, put it in the Readme", then "No configs I want automatically to figure out").

**Go behaviour at stake.** Go sends every bidder request over HTTP/1.1. All adapters share one
client (`router/router.go:143-158`, passed to `exchange.BuildAdapters` at `:247`). Its transport
sets `DialContext` and `TLSClientConfig` and sets neither `ForceAttemptHTTP2` nor `Protocols`.
Go 1.26.2's `Transport.protocols` (`net/http/transport.go:477`) therefore leaves HTTP/2 off.
The same client also serves modules, stored-request HTTP fetchers and the GVL fetchers
(`router.go:211-237`). The cache and floors clients (`:160-189`) are HTTP/1.1 for the same reason.

**Decision.** The bidder transport negotiates HTTP/2 by itself, with no configuration key and no
per-bidder setting. On https it offers ALPN `h2,http/1.1` in the real handshake and uses whatever
the bidder picks. On cleartext, requests go over HTTP/1.1 at once. Meanwhile a background probe
connection sends only the HTTP/2 preface and an empty SETTINGS frame, never a bid request. A
SETTINGS reply marks the origin h2c, and anything else keeps it on HTTP/1.1. Every other client
stays HTTP/1.1, as in Go, so the bidder transport becomes separate from the general client Go
shares. Against an HTTP/1.1 bidder the bytes stay exactly Go's. Against an HTTP/2 bidder only the
framing changes (lower-case header names and Go's HTTP/2 default User-Agent), never the request
content or the auction result. NET-H2 adds a README section that documents this. This is
behaviour Go does not have, so PORTING §1 needs the exception.

**Evidence.** The `h2` spike passed every gate with `h2` 0.4.20 and `http` 1.5.0. It covered an
h2c round trip on loopback (an `h2` client and server with prior knowledge), the preface-only probe
against an h2c server (SETTINGS seen) and against an HTTP/1.1 server (not seen), and a rustls
0.23.37 client config with ALPN `h2` then `http/1.1`, built from the provider and features
`pbs-gohttp-client` already uses. The lock gained 13 packages: atomic-waker 1.1.2, equivalent
1.0.3, fnv 1.0.7, futures-core 0.3.34, futures-sink 0.3.34, h2 0.4.20, hashbrown 0.17.1, http
1.5.0, indexmap 2.14.2, slab 0.4.12, tokio-util 0.7.20, tracing 0.1.44 and tracing-core 0.1.36.
No version was duplicated. No hyper client is needed.

## D-DEAD: production-dead Go

**Status.** Open. Default: port.

**Go behaviour at stake.** No production path reaches:

- `injector/` (856 lines), which no package imports;
- the macros replacer and provider, which `router/router.go:262` builds
  (`macros.NewStringIndexBasedReplacer()`) and `exchange/exchange.go:83` stores in a field nothing
  reads;
- the util helpers iterutil `Firsts`, `Seconds` and `WalkGjsonLeaves`, and sliceutil
  `ContainsStringIgnoreCase`, `IndexPointerFunc` and `DeletePointerFunc`.

**Options.** (a) Port them with their tests, so that every Go test has a Rust counterpart.
INJECTOR adds `pbs-goxml`, the `encoding/xml` subset the injector needs. (b) Leave them unported
and record each one in `dead_code.tsv`, which is what PORTING §1 says today ("Do not port Go code
that is dead in production").

**Default.** (a). Under (a), PORTING §1 needs an exception that names this list.

**Evidence.** No dependency is involved.

## D-VIPERFMT: config files other than JSON and YAML

**Status.** Open. Default: port.

**Go behaviour at stake.** `config.SetupViper` (`config/config.go:936-940`) names the config file
`pbs` (`main.go:59`) and adds the search paths `.` and `/etc/config`. viper v1.12.0 (`go.mod:37`)
tries every path in turn, and in each one every extension of `SupportedExts` (`viper.go:419`, loop
in `fs.go:20-23`): json, toml, yaml, yml, properties, props, prop, hcl, tfvars, dotenv, env, ini.
So `./pbs.toml` is read when `./pbs.json` is absent, and it wins over `./pbs.yaml`.
`ReadInConfig`'s error is ignored (`config/config.go:1322`). The readers come from
`pelletier/go-toml/v2` v2.0.1, `hashicorp/hcl` v1.0.0, `magiconair/properties` v1.8.6,
`subosito/gotenv` v1.3.0 and `gopkg.in/ini.v1` v1.66.4.

**Options.** (a) Port the five readers into `pbs-goviper`, with Go goldens of values and error
texts read through `ReadInConfig` and `Unmarshal`. (b) Waive the readers: `pbs-goviper` keeps its
`UnportedFormat` error, and the waiver is recorded here and in `dead_code.tsv`. A host that
deploys such a file then fails where Go starts.

**Default.** (a).

**Evidence.** No crate can stand in: the parse quirks and error texts are those of the five Go
libraries, so either way the work is a port, with no new dependency.

## D-GOCOLL: Go runtime metrics and the GC ballast

**Status.** Open. Principle 20 puts "(go_* metrics, /debug/pprof)" after D-GOCOLL and D-PPROF.
That names their subjects and gives no default; read as defaults, "keep the go_* metrics" and
"keep /debug/pprof" cannot be met with Go's values. The default below is a proposal.

**Go behaviour at stake.** `metrics/prometheus/prometheus.go:576-585` registers client_golang
v1.12.1's `collectors.NewGoCollector()` through
`WrapRegistererWithPrefix("<namespace>_<subsystem>_")`, so `/metrics`
(`server/prometheus.go:22`) carries the Go runtime under Prebid's prefix. Measured
with go1.26.2 and client_golang v1.12.1, with the prefix `pbs_`, this is 140 families: 76
counters, 55 gauges, 8 histograms and 1 summary. They include `pbs_go_goroutines`,
`pbs_go_threads`, `pbs_go_info{version="go1.26.2"}`, `pbs_go_gc_duration_seconds`, 24
`pbs_go_memstats_*` families, and one family per runtime/metrics entry, 46 of them
`pbs_go_godebug_non_default_behavior_*`. The set depends on the Go toolchain, not on Prebid.
Separately, `main.go:46-51` allocates `garbage_collector_threshold` bytes
(`config/config.go:36`, default 0 at `:952`) and keeps them alive to pace Go's GC.

**Options.** (a) Produce no `go_*` family. The parity harness drops `<prefix>go_*` from Go's
scrape before comparing, and dashboards built on these families need a new source. (b) Produce the
same names with Rust-side values where an analogue exists (threads, tasks) and zero elsewhere. The
scrape keeps its shape, but the values are not true. (c) Add a Rust runtime collector under new
names, which Go does not have.

**Proposed default.** (a), which is what `pbs-metrics` already does
(`crates/metrics/src/prometheus/mod.rs`). `garbage_collector_threshold` keeps Go's type, default
and environment binding, and the Rust server allocates nothing for it, because no GC needs pacing.

**Evidence.** The family list came from a Go program built offline against the pinned `go.mod`
(GOARCH=amd64, GOPROXY=off) that registers the collector exactly as `prometheus.go:584-585`
does.

## D-PPROF: /debug/pprof and other Go runtime text

**Status.** Open. As with D-GOCOLL, principle 20 gives no default; the default below is a
proposal.

**Go behaviour at stake.** `router/admin.go:18-22` registers `net/http/pprof`'s `Index`,
`Cmdline`, `Profile`, `Symbol` and `Trace` on the admin mux (`admin_port`, default 6060). Their
bodies are Go runtime data: goroutine, heap, allocs, block, mutex and threadcreate profiles, CPU
profiles and execution traces. Two other kinds of output carry Go runtime text: `debug.Stack()` in
the recovered-panic logs (`exchange/exchange.go:883`, `hooks/hookexecution/execution.go:112`) and
glog's Go `file:line` headers in module logs.

**Options.** (a) Keep the five routes with Go's status codes, headers and argument errors.
`/debug/pprof/cmdline` (the arguments joined by NUL) and `/debug/pprof/symbol`'s GET answer
(`num_symbols: 1`) come out exactly as Go's. An unknown profile name gets Go's 404 `Unknown
profile`. The runtime-data handlers answer 501 with a fixed text. (b) Drop the routes, so the admin
mux answers 404. (c) Profile with a Rust profiler. No such crate (`pprof`) is in the local cache,
and its output would differ from Go's anyway.

**Proposed default.** (a). The logs keep Go's message text, with a Rust backtrace where Go prints
`debug.Stack()`. The harness treats pprof bodies, stack text and Go `file:line` as unreproducible
and normalises them, as plan Risk 9 says.

**Evidence.** No dependency is involved.

## D-INFLUX: the InfluxDB reporter

**Status.** Open. Default: port.

**Go behaviour at stake.** When an InfluxDB host is configured, `metrics/config/metrics.go:24-29`
starts `influxdb.InfluxDB(...)` from `vrischmann/go-metrics-influxdb` v0.1.1 (`go.mod:41`) over
the `rcrowley/go-metrics` registry (cf1acfcdf475). The reporter sends at every interval, pings
every 5 s and recreates the client when a ping fails (`influxdb.go:70-91`). It logs through Go's
standard `log` package, not glog (`influxdb.go:5`). The client is `influxdata/influxdb1-client`
(8bf82d3c094d). `Client.Write` (`influxdb.go:294-367`) posts line protocol to `/write` with `db`
and `precision`, basic auth and `User-Agent: InfluxDBClient` (`:188`). `Ping` (`:417`) is
`GET /ping`. The client builds its own transport, with no proxy and no timeout (`:160-184`).
Today `pbs-metrics` returns `InfluxNotSupported` when `metrics.influxdb.host` is set.

**Options.** (a) Port the go-metrics engine, the reporter, and the client's Write, Ping and
line-protocol encoding (METRICS-GOMETRICS) over `pbs-gohttp-client`. (b) Drop the reporter: the
config keys are accepted, nothing is sent, and the exemption is recorded. Hosts that use InfluxDB
then lose their metrics.

**Default.** (a).

**Evidence.** No new crate is needed: the reporter uses only an HTTP/1.1 client and timers, which
the workspace already has.

## D-DB: database-backed stored requests

**Status.** Open. Default: port, with containers only if the images are local.

**Go behaviour at stake.** `stored_requests/backends/db_provider`, `db_fetcher` and
`events/database` go through `database/sql` with `go-sql-driver/mysql` v1.6.0 (`go.mod:18`) and
`lib/pq` v1.10.4 (`go.mod:25`). The behaviour includes DSN building (TLS parameters, timeouts),
the `$IDS` and `$LAST_UPDATED` templates, scan and NULL semantics, `sql.ErrNoRows`, and the event
poller's timeline. The two drivers speak these protocols:

- lib/pq authenticates with trust, cleartext, MD5 (`crypto/md5`) and SCRAM-SHA-256
  (`conn.go:1192-1300`), rejects GSSAPI unless a provider is imported, and follows `sslmode`;
- go-sql-driver/mysql authenticates with `mysql_native_password` (SHA-1),
  `caching_sha2_password` (SHA-256, plus RSA-OAEP with SHA-1 of the password off TLS,
  `auth.go:225-226`), `sha256_password`, `mysql_clear_password` and `mysql_old_password`
  (`auth.go:239-273`), and runs queries with arguments as server-side prepared statements unless
  `interpolateParams=true` (`connection.go:198`, `:307`, `:368`).

**Options.** (a) Port: `pbs-godb` holds a `database/sql` subset and two minimal async clients that
reproduce the drivers over tokio and the existing rustls. They take their crypto from `aws-lc-rs`
and MD5 from a port of Go's `crypto/md5`, and they are tested against a go-sqlmock-equivalent fake
and Go goldens. Runs against MySQL 8 and Postgres containers happen only when the images are
already local. (b) Use `tokio-postgres` and `mysql_async`. (c) Drop the DB backends, which leaves
hosts that use them unable to switch.

**Default.** (a).

**Evidence.**

- Option (b) is unavailable offline: neither `tokio-postgres` nor `mysql_async` is in the local
  registry index (`no matching package named ... found`), and `md-5` 0.10.6 is in the index but
  not downloaded. Even online they would not reproduce the Go drivers' DSN parsing, error texts or
  protocol choices.
- The `aws-lc-rs` spike (`aws-lc-rs` 1.19.0, `default-features = false, features =
  ["aws-lc-sys"]`) added nothing to the lock, because rustls already links it. It covers HMAC-SHA256
  (RFC 4231), PBKDF2-HMAC-SHA256 (RFC 7914 §11), SHA-1 (FIPS 180 `abc`) and RSA-OAEP-SHA1 to a DER
  `SubjectPublicKeyInfo` key. With default features it would add `untrusted` 0.7.1, a second
  version.
- Containers: the docker and podman command-line tools are installed, but on 2026-11-18 neither
  the Docker daemon nor a podman machine was running. Whether the images are local cannot be
  checked without starting one. STORED-DB asks before starting a runtime and never pulls.

## D-GRPC: the ads.cert remote signer

**Status.** Open. Default: port.

**Go behaviour at stake.** `experiment/adscert/remotesigner.go:31-42` dials the signatory with
`grpc.Dial(url, grpc.WithTransportCredentials(insecure.NewCredentials()))` from grpc-go v1.83.1
(`go.mod:47`). That is plaintext HTTP/2 with prior knowledge. Each `Sign` makes a unary
`SignAuthenticatedConnection` call under the configured signing timeout. grpc-go sends
`content-type: application/grpc`, `te: trailers`, `user-agent: grpc-go/1.83.1` and `grpc-timeout`
(`internal/transport/http2_client.go:583-613`). The in-process signer, also adscert v0.34.0, uses:

- X25519: `curve25519.X25519` and `ScalarBaseMult`
  (`pkg/adscert/discovery/internal_base_key.go:66`, `:89`);
- HMAC-SHA256 (`pkg/adscert/signatory/signatory_local_impl.go:206`) and SHA-256
  (`signatory_helpers.go:21-24`);
- DNS TXT lookups through `net.LookupTXT`, which ignores the context
  (`pkg/adscert/discovery/resolver_impl.go:15`). On Unix, go1.26.2 always resolves TXT with its
  pure-Go resolver (`net/lookup_unix.go:109-110`), with EDNS0 and a 1232-byte buffer
  (`net/dnsclient_unix.go:39`).

**Options.** (a) Port: a minimal unary gRPC client over `h2` (prior-knowledge h2c,
length-prefixed messages, `grpc-status` and `grpc-message` trailers, `grpc-timeout`). The adscert
messages are written by hand on `prost::encoding`, without the derive. The in-process signer uses
`aws-lc-rs` for X25519, `hmac` with `sha2` for HMAC-SHA256, and a port of Go's TXT path over tokio
sockets (`resolv.conf`, search and ndots, UDP with TCP fallback). (b) Use `tonic` and
`prost-derive`. (c) Drop the remote signer: the config is accepted and signing returns an error.

**Default.** (a).

**Evidence.**

- `tonic` 0.14.5 cannot resolve offline: it needs 43 new packages (hyper 1.12.0, tower 0.5.3,
  axum 0.8.8 and more), and `axum` 0.8.8 is not downloaded.
- `prost-derive` 0.14.4 fails cargo deny because it needs syn 2 (2.0.119) while the workspace
  locks syn 3 (3.0.10).
- `prost` 0.14.4 without the derive (`default-features = false, features = ["std"]`) passed every
  gate and added only `prost`. The spike implemented adscert's `api.RequestInfo` by hand and
  round-tripped its wire bytes.
- `h2` passed (see HTTP/2). The other candidate stacks are covered under
  [dependency spikes](#dependency-spikes): `x25519-dalek` is missing offline, `curve25519-dalek` and
  `hickory-resolver` fail cargo deny, and `aws-lc-rs` passed with RFC 7748 §6.1 vectors.

## D-VERSION: version and revision

**Status.** Open. Principle 20 gives no default; the default below is a proposal.

**Go behaviour at stake.** `version.Ver` and `version.Rev` are set only through `-ldflags`
(`version/version.go`). The Dockerfile sets `Ver` to `git describe --tags` without the leading `v`
and `Rev` to `git rev-parse HEAD` (`Dockerfile:30`). At the pin those are `4.11.1-1-g4e296450`
and `4e296450de2fcc62741340b2e5f7d1df6c1744a5`. `Ver` reaches:

- the `X-Prebid` header (`pbs-go/<Ver>`) on auction, AMP and video responses
  (`endpoints/openrtb2/auction.go:191`, `amp_auction.go:155`, `video_auction.go:173`);
- bidder requests (`exchange/bidder.go:215`);
- the agma analytics sender (`analytics/agma/sender.go:67`);
- nine adapters: nextmillennium, sharethrough, nexx360, showheroes, mediasquare, conversant, ix,
  missena and rubicon.

`/version`, on both the main and the admin routers (`router/router.go:294`, `router/admin.go:25`),
returns `{"revision":...,"version":...}`, with `not-set` for an empty value
(`endpoints/version.go:11`). An empty `Ver` gives `pbs-go/unknown` in `X-Prebid`
(`version/xprebidheader.go:40-45`). `Rev` is read only by `/version`.

**Options.** (a) Rust builds report the `Ver` and `Rev` of a Go build of the pinned commit. (b)
Rust builds report their own identity (the Rust repository's describe and HEAD). Every `X-Prebid`
header, adapter field and `/version` body then differs from Go (plan Risk 13). (c) Leave both
unset, as a Go build without ldflags does.

**Proposed default.** `Ver` is the pin's Go value in every Rust build, stored as a string beside
`UPSTREAM_SYNC`: `git describe` shortens the hash by the clone's object count (eight hex digits
here), so recomputing the value can change it. `Rev` is Go's (the pin's SHA) in golden and parity
builds, and the Rust repository's commit in release images, so `/version` tells operators which
Rust build runs. Release images then differ from Go only in `/version`'s `revision`. The
`X-Prebid` prefix stays `pbs-go`. A build without `PBS_VERSION` or `PBS_REVISION` behaves as Go
does with the ldflags unset (plan principle 4, FPD-SCHAIN-VERSION).

**Evidence.** No dependency is involved.

## D-ARCH: reference architecture

**Status.** Open. Default: amd64.

**Go behaviour at stake.** Go's compiler fuses `x*y + z` into one FMA instruction on arm64 but
not on amd64, and Rust never contracts. For `exchange/price_granularity.go:76`
(`incrementsRounded*increment + bucketMin`), 7,151 of 200,000 sampled results differ between
GOARCH=arm64 and amd64 (measured while planning, plan §3.7). Go's pure-Go `math.Pow` (for example
`modules/scope3/rtd/masking.go:166`) gives the same results on both architectures, so it is ported,
not taken from libm.

**Options.** (a) amd64: the goldens and the Go parity server are built with GOARCH=amd64 (on this
arm64 machine under Rosetta, with `CC="clang -arch x86_64"`), and OPS-IMAGE builds amd64. Rust on
either architecture then matches. (b) arm64: Rust would have to fuse exactly where Go's SSA rules
fuse, site by site, and there is no general way to do that. (c) Both: amd64 stays the truth, and
`pbs-devtools arch-check` lists where Go on arm64 differs.

**Default.** (a).

**Evidence.** The measurement above. The D-51D build compiles the C with `-ffp-contract=off`, so
the 51Degrees results match the amd64 reference on any host.

## D-SYNC: catching up with upstream after the pin

**Status.** Open. Default: ask the user at FINAL-INT.

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
   - 15 of the 21 Go modules in this repository require `github.com/prebid/prebid-server/v4
     v4.11.2-0.20261112141353-4e296450de2f`, and each one needs the new pseudo-version;
   - `upstream-seams.patch` (56 files) is re-applied and re-checked;
   - `port_map.json`, `map_ranges.tsv`, `float_sites.tsv` and the subtest ledger are regenerated;
   - every dependency `go.mod` bumps is a behaviour change to port, cited by version. For example,
     a new device-detection-go changes the D-51D SHA-256 pins.
4. The final gate runs again at the new pin. `UPSTREAM_SYNC` moves, and so does the D-VERSION
   `Ver` string. The upstream-port workflow is re-enabled on the root layout (PORT-SYS-RSLAYOUT) if
   the user agrees, and the orchestrator checks the remote heads before it pushes.

At the past rate of about four commits a week, mostly adapters and bidder-info, a catch-up after
*n* weeks covers about 4*n* commits.

## Dependency spikes

### Method

Each candidate stack went into a scratch crate inside a copy of this branch's workspace (wu/t3-base,
1725ce2f). The copy kept the real `Cargo.toml` lint table, `clippy.toml`, `deny.toml`,
`rustfmt.toml`, `taplo.toml`, `Cargo.lock` and `patches/`, and every crate had `[lints] workspace =
true`. Resolution used only the local registry cache (`cargo metadata --offline`) and kept every
locked version. The network was pointed at `127.0.0.1:9`. The gates, all offline:

1. the lock diff against the starting lock: additions only;
2. `cargo tree --workspace --all-features --duplicates -e normal,build,dev --target all` prints
   nothing;
3. `cargo build`, `cargo clippy -- -D warnings`, `cargo test` and `cargo doc` (with
   `RUSTDOCFLAGS=-D warnings`), each with `--offline --locked --all-features`, and build and clippy
   with `--all-targets`;
4. `cargo +nightly-2026-11-13 fmt --check`, `taplo fmt --check` and `cargo machete`;
5. `cargo deny --offline --locked check --deny warnings`, covering advisories, bans (one version
   per crate across every target), licences and sources (in cargo-deny 0.20.2, `--offline`
   replaces the plan's `--disable-fetch`);
6. `scripts/check-no-lint-suppression.sh --root <workspace>`.

The tools were rustc and cargo 1.100.0, rustfmt from nightly-2026-11-13, cargo-deny 0.20.2,
cargo-machete 0.9.2 and taplo 0.10.0, on an aarch64-apple-darwin host. The spike crates were scratch
and are not committed. The clippy lint `multiple_crate_versions` counts only the versions reachable
from the crate it checks. cargo deny checks the whole workspace on every target, so it catches
duplicates that clippy misses.

### Results

| Stack | Offline | Added to the lock | Result |
|---|---|---|---|
| `h2` 0.4.20 (`http` 1.5.0) | yes | 13 packages (see HTTP/2) | passes every gate |
| `cc` 1.11.2 (build dependency) | yes | nothing (locked through `aws-lc-sys`) | passes; compiles the pinned 51Degrees C |
| `futures-util` 0.3.34, `default-features = false, features = ["std"]` | yes | futures-core, futures-task, futures-util, slab | passes |
| `hmac` 0.12.1 with `sha2` 0.10.9 | yes | hmac (sha2 0.10.9 already locked) | passes (RFC 4231) |
| `aws-lc-rs` 1.19.0, `default-features = false, features = ["aws-lc-sys"]` | yes | nothing (rustls links it) | passes: X25519, HMAC-SHA256, SHA-1, PBKDF2, RSA-OAEP-SHA1 |
| `prost` 0.14.4, `default-features = false, features = ["std"]` | yes | prost | passes |
| `prost` 0.14.4 with `derive` | yes | anyhow, either, itertools 0.14.0, prost, prost-derive, syn 2.0.119 | fails cargo deny: syn 2.0.119 and 3.0.10 |
| `arc-swap` 1.6.0 (the only cached version) | yes | arc-swap | passes |
| `criterion` 0.8.2 (`default-features = false`) and `dhat` 0.3.3 as dev dependencies, starting from wu/T2B-COW's lock | yes, with that lock | 44 packages relative to this branch's lock, all already in T2B-COW's lock | passes (a timing bench with a hand-written `main` and a dhat allocation bench) |
| `criterion` 0.8.2, resolved fresh | no | | `half` 2.7.1 needs `zerocopy-derive` 0.8.63, which is not downloaded; T2B-COW's lock pins `half` 2.6.0 |
| `x25519-dalek` | no | | not in the local index; replaced by `aws-lc-rs` |
| `curve25519-dalek` 5.0.0, `default-features = false, features = ["zeroize"]` | yes | cpufeatures 0.3.1, curve25519-dalek, curve25519-dalek-derive 0.1.1, fiat-crypto 0.3.0, rustc_version 0.4.1, semver 1.0.28, syn 2.0.119 | fails cargo deny: syn 2 and 3, cpufeatures 0.2.17 and 0.3.1 (both from x86_64-only dependencies) |
| `tonic` 0.14.5 | no | | `axum` 0.8.8 not downloaded (43 packages to add) |
| `tokio-postgres` | no | | not in the local index |
| `mysql_async` | no | | not in the local index |
| `md-5` 0.10.6 | no | | in the index, not downloaded |
| `hickory-resolver` 0.26.3, `default-features = false, features = ["system-config", "tokio"]` | yes | 101 packages | fails clippy (`multiple_crate_versions`: syn) and cargo deny: syn 2 through `jni-macros` (Android-only `jni`), cpufeatures 0.3.1 through `rand` 0.10.3 and `chacha20` 0.10.2 (x86) |

`sha2` 0.11.1, which is cached, would share `cpufeatures` 0.3 with `chacha20` and
`curve25519-dalek`. Moving to it would still leave the syn 2 duplicate, so it rescues neither
stack.

Two union workspaces put together every stack that passed alone:

- `h2`, `futures-util`, `hmac` with `sha2`, `aws-lc-rs`, `prost` without the derive, `cc` and
  `arc-swap`, on this branch's lock: 18 registry packages added;
- the same stacks plus `criterion` and `dhat`, on T2B-COW's lock: 62 registry packages added
  relative to this branch's lock.

Both passed every gate with no duplicate version, and their 13 spike tests passed.

### What each stack is for

- `h2`: NET-H2 (bidder HTTP/2 and h2c) and the D-GRPC client. No hyper.
- `cc`: the D-51D build script.
- `futures-util` (`FuturesUnordered`): hook groups collected in completion order, stopping at the
  first reject, as `hooks/hookexecution/execution.go:142-153` does (principle 23).
- `hmac` and `sha2`: wherever Go uses `crypto/hmac` and `crypto/sha256`. In Prebid that is the
  audienceNetwork and huaweiads adapters, the rulesengine and scope3 modules, and adscert's HMAC.
- `aws-lc-rs`: X25519 (adscert), RSA-OAEP-SHA1 and SHA-1 (mysql), and PBKDF2 (SCRAM), none of
  which a cached RustCrypto crate provides.
- `prost` without the derive: protobuf messages written by hand for D-GRPC.
- `arc-swap`: state a background task refreshes (currency rates, the GVL, stored data), as
  PORTING §7 suggests.
- `criterion` and `dhat`: the T2B-COW benches (plan Risk 8). They are dev-only.
- No crate: MD5, which needs a port of Go's `crypto/md5` for lib/pq and 51Degrees `onpremise`, and
  DNS TXT, which needs a port of Go's resolver path.

## Changes outside this file

These files are not owned by this unit. Each change below follows from a decision above.

- `PORTING.md`:
  - §5 ("No `unsafe`") gains the D-51D exception for `crates/fiftyone-dd/`;
  - §1 ("Do not add behaviour Go does not have") gains the HTTP/2 exception;
  - §1 ("Do not port Go code that is dead in production") gains the D-DEAD list, as long as
    D-DEAD stays "port".
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
- `Cargo.lock`: when merges regenerate it, `criterion`'s graph must keep T2B-COW's `half` 2.6.0.
  An offline resolution from scratch picks `half` 2.7.1 and fails.
