//! The comparisons with the goldens the Go program under `testdata/go` writes from the real
//! `parity` packages, and the documents whose Go schema verdicts it records.
//!
//! The documents are serde types that `serde_json` reads and writes, where Go's mirror uses
//! `encoding/json`, so the two compare as JSON values ([`Doc`]), not bytes. Where they read an
//! input differently a ledger entry says why (`json.parity-documents`,
//! `base64.parity-std-encoding`, `gzip.parity-bodies` and the `json.*` entries they name): the
//! hand-written cases list each such input, and the fuzz tests count the generated ones by that
//! reason ([`Reading`], [`sessions::Divergence`]) and pin the counts, so an input whose reading
//! changes fails them.
//!
//! `PBS_PARITY_REGEN=1` makes [`documents_validate_with_go`] rewrite `testdata/documents`
//! instead of checking it (`testdata/go/run.sh` does that between two runs of the Go program:
//! the first writes the scripted sessions the trace documents come from, the second records the
//! Go schema verdicts on the rewritten documents).

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Read as _,
    path::Path,
    str,
};

use flate2::read::MultiGzDecoder;
use pbs_errortypes::GoError;
use pbs_util::{
    randomutil::RandomGenerator as _, timeutil::Time as _, uuidutil::UuidGenerator as _,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::{
    adapterio::{
        AdapterBidsInput, AdapterBidsOutput, AdapterRequestsInput, AdapterRequestsOutput,
        BiddersDump, ConfigDump, Health, ResetResult, WRAP_INFO_AWARE, WRAP_RAW,
    },
    body::{Body, BodyError},
    contracts::{
        self, Domain, IMPL_GO, IMPL_RUST, META_HEADER, PATH_PREFIX, SCHEMA_VERSION, TOKEN_HEADER,
        encode_document, error_document,
    },
    envelope::{Envelope, FakeValues},
    errorcodes::error_codes,
    fakes::{Source, defaults, sequence_uuid},
    golden::{
        Doc, Json, TestResult, check, check_eq, check_same_json, crate_path, decode_b64,
        golden_json, lossy, read_file,
    },
    ids::{self, DecisionId, OutboundKind, StageId, StageStatus},
    trace::{error_record_of, error_records_of, nil_error_record},
    tracedoc::{self, BuildInfo, MetaError, OutboundMeta, Trace},
};

/// The `golden` member of `testdata/golden/<name>`.
fn golden(name: &str) -> Result<Json, String> {
    golden_json(name)?.get("golden")
}

/// `v` as `serde_json` writes it.
fn to_json<T: Serialize + ?Sized>(v: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(v).map_err(|e| e.to_string())
}

/// `raw` read as a `T` by `serde_json`.
fn from_json<T: DeserializeOwned>(raw: &[u8]) -> Result<T, String> {
    serde_json::from_slice(raw).map_err(|e| e.to_string())
}

/// `bytes` as JSON, naming `what` and the bytes when they are not.
fn parse_doc(bytes: &[u8], what: &str) -> Result<Doc, String> {
    Doc::parse(bytes).map_err(|e| format!("{what}: {e}: {}", lossy(bytes)))
}

/// A golden member holding a JSON document: as text when the golden program wrote it as a
/// string, otherwise its value.
fn document_bytes(member: &Json) -> Result<Vec<u8>, String> {
    if member.0.is_string() {
        member.text().map(String::into_bytes)
    } else {
        Ok(member.to_bytes())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .flat_map(|b| [b >> 4, b & 0x0F])
        .filter_map(|nibble| char::from_digit(u32::from(nibble), 16))
        .collect()
}

/// Whether `name` is a `.json` file name.
fn is_json(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|ext| ext == "json")
}

fn names<T: Copy>(all: &[T], name: fn(T) -> &'static str) -> Vec<String> {
    all.iter().map(|v| name(*v).to_owned()).collect()
}

fn strings(json: &Json) -> Result<Vec<String>, String> {
    json.elements()?.iter().map(Json::text).collect()
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

/// How the port reads an input beside Go: the same, or the ledgered reason it differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Reading {
    /// Both read it, to the same value.
    Same,
    /// Both refuse it, each with its library's text (`json.error-texts`,
    /// `base64.parity-std-encoding`, `gzip.parity-bodies`).
    BothRefuse,
    /// Go reads it, `serde_json` refuses its invalid UTF-8 (`json.invalid-utf8`).
    InvalidUtf8,
    /// Go reads the last of a repeated member, the serde derives refuse the document
    /// (`json.duplicate-keys`).
    RepeatedMember,
    /// Go reads a `null` as the zero value, serde refuses it (`json.null-into-value`,
    /// `json.null-elements`).
    Null,
    /// Go pads or cuts a header pair to two strings, serde refuses another length
    /// (`json.parity-documents`).
    HeaderPair,
    /// Go's `base64.StdEncoding` skips a line break or a last digit's set unused bits, the base64
    /// crate refuses them (`base64.parity-std-encoding`).
    Base64,
    /// Go fails with a type error where serde reads the input: a member matched ignoring case, or
    /// repeated, holds a value of another kind, or an array stands for a struct (`json.key-case`,
    /// `json.duplicate-keys`, `json.struct-from-array`).
    GoRefuses,
    /// Both read it, to other values: Go fills members from keys that match them ignoring case,
    /// or keeps `null` in a raw member (`json.key-case`, `json.null-raw`).
    ValueDiffers,
    /// Go keeps a `null` raw member as the bytes `null`, serde reads it as absent and leaves the
    /// member out (`json.null-raw`).
    NullRaw,
    /// Both compress the same content, to other gzip bytes (`gzip.payload-bytes`).
    GzipBytes,
    /// flate2 and `compress/gzip` read the content otherwise (`gzip.parity-bodies`).
    GzipRead,
    /// A `Content-Encoding` holding U+0130, which Go lower-cases to `i` and Rust to `i` and
    /// U+0307 (`gzip.parity-bodies`).
    DottedCapitalI,
}

/// How many inputs each reading covers.
fn tally<I: IntoIterator<Item = Reading>>(readings: I) -> BTreeMap<Reading, usize> {
    let mut counts = BTreeMap::new();
    for reading in readings {
        *counts.entry(reading).or_default() += 1;
    }
    counts
}

/// The reason the serde derives refuse a document that Go reads, from `serde_json`'s message.
fn serde_refusal(message: &str) -> Option<Reading> {
    [
        ("invalid unicode code point", Reading::InvalidUtf8),
        ("duplicate field", Reading::RepeatedMember),
        ("invalid type: null", Reading::Null),
        ("expected a tuple of size 2", Reading::HeaderPair),
        ("trailing characters", Reading::HeaderPair),
    ]
    .into_iter()
    .find_map(|(text, reading)| message.contains(text).then_some(reading))
}

/// `doc` without its object members that are `null`.
fn without_nulls(doc: Doc) -> Doc {
    match doc {
        Doc::Object(members) => Doc::Object(
            members
                .into_iter()
                .filter(|(_, v)| !matches!(v, Doc::Null))
                .map(|(k, v)| (k, without_nulls(v)))
                .collect(),
        ),
        Doc::Array(items) => Doc::Array(items.into_iter().map(without_nulls).collect()),
        scalar @ (Doc::Null | Doc::Bool(_) | Doc::Number(_) | Doc::String(_) | Doc::Escaped(_)) => {
            scalar
        }
    }
}

/// How the port's reading of a document compares with Go's: each side's document written back,
/// or its refusal.
fn document_reading(
    ours: &Result<Vec<u8>, String>,
    theirs: &Result<Vec<u8>, String>,
    what: &str,
) -> Result<Reading, String> {
    match (ours, theirs) {
        (Ok(mine), Ok(go)) => {
            let (mine, go) = (parse_doc(mine, what)?, parse_doc(go, what)?);
            Ok(if mine == go {
                Reading::Same
            } else if without_nulls(mine) == without_nulls(go) {
                Reading::NullRaw
            } else {
                Reading::ValueDiffers
            })
        }
        (Err(_), Err(_)) => Ok(Reading::BothRefuse),
        (Err(err), Ok(_)) => serde_refusal(err)
            .ok_or_else(|| format!("{what}: serde_json refuses it ({err}), Go reads it")),
        (Ok(_), Err(err)) => {
            if err.starts_with("json: cannot unmarshal ") {
                Ok(Reading::GoRefuses)
            } else {
                Err(format!(
                    "{what}: Go refuses it ({err}), serde_json reads it"
                ))
            }
        }
    }
}

// The vendored contract is byte-identical to the schema files of the parity-harness commit the
// goldens were built from (the Go program refuses to write goldens otherwise), and nothing was
// added to or changed in it since.
#[test]
fn contract_is_the_go_contract() -> TestResult {
    let files = golden("contract.json")?.get("files")?;
    let want: BTreeMap<String, String> = files
        .members()?
        .into_iter()
        .map(|(name, sha)| Ok((name, sha.text()?)))
        .collect::<Result<_, String>>()?;
    let mut have = BTreeMap::new();
    for dir in ["", "examples"] {
        let path = crate_path(&format!("contract/{dir}"));
        let entries = fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_json(&name) {
                continue;
            }
            let rel = if dir.is_empty() {
                name
            } else {
                format!("{dir}/{name}")
            };
            let bytes = read_file(&format!("contract/{rel}"))?;
            let _previous = have.insert(rel, sha256_hex(&bytes));
        }
    }
    check_eq(&have, &want, "vendored contract files")
}

// Go: testdata/go/main.go idsGolden (ids.json).
#[test]
fn schemas_match_go() -> TestResult {
    let g = golden("ids.json")?;
    check_eq(
        &SCHEMA_VERSION.to_owned(),
        &g.string("schema_version")?,
        "schema_version",
    )?;
    let schemas: BTreeMap<String, String> = g
        .get("schemas")?
        .members()?
        .into_iter()
        .map(|(k, v)| Ok((k, v.text()?)))
        .collect::<Result<_, String>>()?;
    let ours: BTreeMap<String, String> = [
        ("envelope", contracts::SCHEMA_ENVELOPE),
        ("trace", contracts::SCHEMA_TRACE),
        (
            "adapter_requests_input",
            contracts::SCHEMA_ADAPTER_REQUESTS_INPUT,
        ),
        (
            "adapter_requests_output",
            contracts::SCHEMA_ADAPTER_REQUESTS_OUTPUT,
        ),
        ("adapter_bids_input", contracts::SCHEMA_ADAPTER_BIDS_INPUT),
        ("adapter_bids_output", contracts::SCHEMA_ADAPTER_BIDS_OUTPUT),
        ("diff", contracts::SCHEMA_DIFF),
        ("index", contracts::SCHEMA_INDEX),
        ("health", contracts::SCHEMA_HEALTH),
        ("config_dump", contracts::SCHEMA_CONFIG_DUMP),
        ("bidders_dump", contracts::SCHEMA_BIDDERS_DUMP),
        ("reset", contracts::SCHEMA_RESET),
        ("stages", contracts::SCHEMA_STAGES),
        ("decisions", contracts::SCHEMA_DECISIONS),
        ("normalization", contracts::SCHEMA_NORMALIZATION),
        ("fakes", contracts::SCHEMA_FAKES),
        ("error_codes", contracts::SCHEMA_ERROR_CODES),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    check_eq(&ours, &schemas, "schemas")?;
    check_eq(
        &i64::try_from(tracedoc::META_VERSION).ok(),
        &Some(g.int("meta_version")?),
        "meta_version",
    )
}

/// The registry identifiers and the constants of the `parity` package, by golden member.
fn id_lists() -> [(&'static str, Vec<String>); 14] {
    [
        ("impls", owned(&[IMPL_GO, IMPL_RUST])),
        ("http", owned(&[PATH_PREFIX, TOKEN_HEADER, META_HEADER])),
        ("domains", names(&Domain::ALL, Domain::as_str)),
        ("stages", names(&StageId::ALL, StageId::as_str)),
        (
            "stage_statuses",
            names(&StageStatus::ALL, StageStatus::as_str),
        ),
        ("decisions", names(&DecisionId::ALL, DecisionId::as_str)),
        (
            "decision_sources",
            owned(&[ids::DECISION_SOURCE_ENVELOPE, ids::DECISION_SOURCE_RNG]),
        ),
        (
            "outbound_kinds",
            names(&OutboundKind::ALL, OutboundKind::as_str),
        ),
        (
            "mock_modes",
            owned(&[ids::MOCK_MODE_ORACLE, ids::MOCK_MODE_INLINE]),
        ),
        (
            "mock_levels",
            owned(&[ids::MOCK_LEVEL_HTTP, ids::MOCK_LEVEL_BIDDER_RESPONSE]),
        ),
        (
            "mock_sources",
            owned(&[
                ids::MOCK_SOURCE_INLINE,
                ids::MOCK_SOURCE_RULE,
                ids::MOCK_SOURCE_FIXTURE,
                ids::MOCK_SOURCE_TEMPLATE,
                ids::MOCK_SOURCE_DEFAULT,
                ids::MOCK_SOURCE_STATIC,
            ]),
        ),
        (
            "outbound_errors",
            owned(&[
                ids::OUTBOUND_ERR_TIMEOUT,
                ids::OUTBOUND_ERR_REFUSED,
                ids::OUTBOUND_ERR_RESET,
                ids::OUTBOUND_ERR_NETWORK_DISABLED,
                ids::OUTBOUND_ERR_OTHER,
            ]),
        ),
        ("unkeyed_stage", owned(&[ids::UNKEYED_STAGE])),
        (
            "usersync_shuffle_identity",
            owned(&[ids::USERSYNC_SHUFFLE_IDENTITY]),
        ),
    ]
}

/// The document constants of the `parity` packages, by golden member.
fn document_lists() -> [(&'static str, Vec<String>); 6] {
    [
        ("wraps", owned(&[WRAP_RAW, WRAP_INFO_AWARE])),
        (
            "scopes",
            owned(&[tracedoc::SCOPE_GENERAL, tracedoc::SCOPE_DEBUG]),
        ),
        (
            "severities",
            owned(&[
                tracedoc::SEVERITY_FATAL,
                tracedoc::SEVERITY_WARNING,
                tracedoc::SEVERITY_UNKNOWN,
            ]),
        ),
        (
            "metric_types",
            owned(&[
                tracedoc::METRIC_COUNTER,
                tracedoc::METRIC_GAUGE,
                tracedoc::METRIC_HISTOGRAM,
                tracedoc::METRIC_SUMMARY,
                tracedoc::METRIC_UNTYPED,
            ]),
        ),
        (
            "analytics_kinds",
            owned(&[
                tracedoc::ANALYTICS_AUCTION,
                tracedoc::ANALYTICS_AMP,
                tracedoc::ANALYTICS_VIDEO,
                tracedoc::ANALYTICS_COOKIE_SYNC,
                tracedoc::ANALYTICS_SETUID,
                tracedoc::ANALYTICS_NOTIFICATION,
            ]),
        ),
        ("error_kind_unknown", owned(&[tracedoc::ERROR_KIND_UNKNOWN])),
    ]
}

// Go: testdata/go/main.go idsGolden (ids.json).
#[test]
fn identifiers_match_go() -> TestResult {
    let g = golden("ids.json")?;
    for (name, ours) in id_lists().iter().chain(&document_lists()) {
        let member = g.get(name)?;
        let theirs = if member.0.is_string() {
            vec![member.text()?]
        } else {
            strings(&member)?
        };
        check_eq(ours, &theirs, name)?;
    }
    Ok(())
}

// Go: testdata/go/main.go idsGolden (ids.json).
#[test]
fn error_codes_match_go() -> TestResult {
    let codes: Vec<(i64, String)> = golden("ids.json")?
        .array("error_codes")?
        .iter()
        .map(|c| Ok((c.int("code")?, c.string("kind")?)))
        .collect::<Result<_, String>>()?;
    let ours: Vec<(i64, String)> = error_codes()
        .iter()
        .map(|(code, kind)| (*code, (*kind).to_owned()))
        .collect();
    check_eq(&ours, &codes, "error codes")
}

// Go: testdata/go/main.go fakesGolden (fakes.json).
#[test]
fn fakes_match_go() -> TestResult {
    let g = golden("fakes.json")?;
    let want_defaults: FakeValues = from_json(&g.get("defaults")?.to_bytes())?;
    check_eq(&defaults(), &want_defaults, "defaults")?;
    let derived = g.get("derived")?;
    let fixed = Source::new().now();
    check_eq(
        &fixed.format("2006-01-02T15:04:05.000Z07:00"),
        &derived.string("now_rfc3339")?,
        "rfc3339",
    )?;
    check_eq(&fixed.unix(), &derived.int("now_unix_seconds")?, "unix")?;
    check_eq(&fixed.weekday(), &derived.int("now_weekday")?, "weekday")?;
    check_eq(&fixed.year_day(), &derived.int("now_year_day")?, "year day")?;
    for case in g.array("sequence")? {
        let n = case.get("n")?.0.as_u64().ok_or("sequence n is not a u64")?;
        check_eq(
            &sequence_uuid(&case.string("seed")?, n),
            &case.string("uuid")?,
            "sequence uuid",
        )?;
    }
    for case in g.array("sources")? {
        let set: Option<FakeValues> = from_json(&case.get("set")?.to_bytes())?;
        let what = case.get("set")?.0.to_string();
        let src = Source::new();
        src.set(set.as_ref());
        let values = serde_json::to_value(src.values()).map_err(|e| e.to_string())?;
        check_eq(&values, &case.get("values")?.0, &what)?;
        check_eq(&src.now().unix_milli(), &case.int("now_unix_ms")?, &what)?;
        check_eq(
            &src.now().format("2006-01-02T15:04:05.000Z07:00"),
            &case.string("now_formatted")?,
            &what,
        )?;
        check_eq(&src.generate_int63(), &case.int("int63")?, &what)?;
        check_eq(
            &i64::try_from(src.int()).ok(),
            &Some(case.int("int")?),
            &what,
        )?;
        for (n, want) in case.get("intn")?.members()? {
            let n: i64 = n.parse().map_err(|e| format!("intn key: {e}"))?;
            check_eq(&src.intn(n), &want.0.as_i64(), &what)?;
        }
        check_eq(
            &src.generate().map_err(|e| e.to_string()),
            &Ok(case.string("generate")?),
            &what,
        )?;
        let draws: Vec<String> = (0..3).map(|_| src.new_v4()).collect();
        check_eq(&draws, &strings(&case.get("new_v4")?)?, &what)?;
        src.set(set.as_ref());
        check_eq(&src.new_v4(), &case.string("restarted")?, &what)?;
    }
    Ok(())
}

/// A Go error the golden program names.
fn go_error(spec: &Json) -> Result<GoError, String> {
    let message = spec.string("message")?;
    let code = spec.opt("code").and_then(|c| c.0.as_i64()).unwrap_or(0);
    Ok(match spec.string("type")?.as_str() {
        "timeout" => GoError::timeout(message),
        "tmax_timeout" => GoError::tmax_timeout(message),
        "bad_input" => GoError::bad_input(message),
        "blocked_app" => GoError::blocked_app(message),
        "account_disabled" => GoError::account_disabled(message),
        "acct_required" => GoError::acct_required(message),
        "bad_server_response" => GoError::bad_server_response(message),
        "failed_to_request_bids" => GoError::failed_to_request_bids(message),
        "bidder_temporarily_disabled" => GoError::bidder_temporarily_disabled(message),
        "bidder_throttled" => GoError::bidder_throttled(message),
        "malformed_acct" => GoError::malformed_acct(message),
        "warning" => GoError::warning(code, message),
        "failed_to_unmarshal" => GoError::failed_to_unmarshal(message),
        "failed_to_marshal" => GoError::failed_to_marshal(message),
        "debug_warning" => GoError::debug_warning(code, message),
        "invalid_imp_first_party_data" => GoError::invalid_imp_first_party_data(message),
        "generic" => GoError::generic(message),
        other => return Err(format!("unknown error type {other}")),
    })
}

// Go: testdata/go/main.go errorsGolden (errors.json).
#[test]
fn error_records_match_go() -> TestResult {
    let g = golden("errors.json")?;
    let mut all = Vec::new();
    for case in g.array("cases")? {
        let err = go_error(&case.get("spec")?)?;
        check_same_json(
            &to_json(&error_record_of(&err))?,
            case.string("record")?.as_bytes(),
            err.message(),
        )?;
        all.push(err);
    }
    check_same_json(
        &to_json(&nil_error_record())?,
        g.string("nil")?.as_bytes(),
        "nil error",
    )?;
    let split = |errs: &[GoError]| {
        let (fatal, warnings) = error_records_of(errs);
        to_json(&BTreeMap::from([("errors", fatal), ("warnings", warnings)]))
    };
    check_same_json(&split(&all)?, g.string("split")?.as_bytes(), "split")?;
    check_same_json(
        &split(&[])?,
        g.string("split_empty")?.as_bytes(),
        "split empty",
    )
}

/// The step an `X-Parity-Meta` decode error names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MetaStep {
    /// The base64 of the header.
    Base64,
    /// The JSON inside it.
    Json,
    /// The meta's version.
    Version,
}

/// The step of a decode error's text, the port's or Go's.
fn meta_step(message: &str) -> Option<MetaStep> {
    [
        ("parity: decoding X-Parity-Meta base64: ", MetaStep::Base64),
        ("parity: decoding X-Parity-Meta JSON: ", MetaStep::Json),
        (
            "parity: unsupported X-Parity-Meta version ",
            MetaStep::Version,
        ),
    ]
    .into_iter()
    .find_map(|(prefix, step)| message.starts_with(prefix).then_some(step))
}

/// How the port's reading of an `X-Parity-Meta` value compares with Go's (`theirs` is the meta
/// Go read, as JSON, or Go's error).
fn meta_reading(
    ours: &Result<OutboundMeta, MetaError>,
    theirs: &Result<Vec<u8>, String>,
    what: &str,
) -> Result<Reading, String> {
    let go_step = theirs.as_ref().err().and_then(|err| meta_step(err));
    let reading = match ours {
        Ok(meta) => match theirs {
            Ok(go) => Some(
                if parse_doc(&to_json(meta)?, what)? == parse_doc(go, what)? {
                    Reading::Same
                } else {
                    Reading::ValueDiffers
                },
            ),
            // Go read the version from a member whose key matches `v` only ignoring case.
            Err(_) if go_step == Some(MetaStep::Version) => Some(Reading::ValueDiffers),
            Err(err) => err
                .contains("json: cannot unmarshal ")
                .then_some(Reading::GoRefuses),
        },
        Err(err) => {
            let text = err.to_string();
            match meta_step(&text) {
                Some(MetaStep::Base64) => Some(if go_step == Some(MetaStep::Base64) {
                    Reading::BothRefuse
                } else {
                    Reading::Base64
                }),
                Some(MetaStep::Json) => match go_step {
                    Some(MetaStep::Json) => Some(Reading::BothRefuse),
                    Some(MetaStep::Version) | None => serde_refusal(&text),
                    Some(MetaStep::Base64) => None,
                },
                Some(MetaStep::Version) => match go_step {
                    Some(MetaStep::Version) if theirs.as_ref().err() == Some(&text) => {
                        Some(Reading::BothRefuse)
                    }
                    Some(MetaStep::Version) | None => Some(Reading::ValueDiffers),
                    Some(MetaStep::Json) => Some(Reading::GoRefuses),
                    Some(MetaStep::Base64) => None,
                },
                None => None,
            }
        }
    };
    reading.ok_or_else(|| format!("{what}: the port reads {ours:?}, Go {theirs:?}"))
}

// Go: testdata/go/main.go metaGolden (meta.json).
#[test]
fn outbound_meta_matches_go() -> TestResult {
    let g = golden("meta.json")?;
    for case in g.array("encoded")? {
        let meta: OutboundMeta = from_json(&case.get("meta")?.to_bytes())?;
        let header = meta.encode_header().map_err(|e| e.to_string())?;
        // The two encode their own JSON spelling of the meta.
        check_same_json(
            &decode_b64(&header)?,
            &decode_b64(&case.string("header")?)?,
            &header,
        )?;
    }
    let mut readings = Vec::new();
    for case in g.array("decoded")? {
        let header = case.string("header")?;
        let theirs = match case.opt("error") {
            Some(err) => Err(err.text()?),
            None => Ok(document_bytes(&case.get("meta")?)?),
        };
        readings.push(meta_reading(
            &OutboundMeta::decode_header(&header),
            &theirs,
            &header,
        )?);
    }
    check_eq(
        &readings,
        &vec![
            Reading::Same,
            Reading::BothRefuse,
            Reading::BothRefuse,
            Reading::BothRefuse,
            Reading::BothRefuse,
            Reading::Same,
            Reading::BothRefuse,
        ],
        "readings",
    )
}

// Generated X-Parity-Meta values (`bodyfuzz.go`): damaged base64 of rewritten meta JSON, decoded,
// and each decoded meta encoded again.
// Go: testdata/go/bodyfuzz.go fuzzMetaGolden (meta_fuzz.json).
#[test]
fn fuzzed_outbound_meta_matches_go() -> TestResult {
    let cases = golden("meta_fuzz.json")?.elements()?;
    check_eq(&cases.len(), &300, "cases")?;
    let mut readings = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let header = String::from_utf8(decode_b64(&case.string("header_b64")?)?)
            .map_err(|e| format!("header {i}: {e}"))?;
        let what = format!("header {i} ({header:?})");
        let theirs = match case.opt("error") {
            Some(err) => Err(err.text()?),
            None => Ok(case.string("meta")?.into_bytes()),
        };
        let ours = OutboundMeta::decode_header(&header);
        let reading = meta_reading(&ours, &theirs, &what)?;
        if reading == Reading::Same
            && let Ok(meta) = &ours
        {
            let again = meta.encode_header().map_err(|e| e.to_string())?;
            check_same_json(
                &decode_b64(&again)?,
                &decode_b64(&case.string("header_again")?)?,
                &what,
            )?;
        }
        readings.push(reading);
    }
    check_eq(
        &tally(readings),
        &BTreeMap::from([
            (Reading::Same, 79),
            (Reading::BothRefuse, 156),
            (Reading::InvalidUtf8, 20),
            (Reading::Null, 14),
            (Reading::Base64, 18),
            (Reading::GoRefuses, 6),
            (Reading::ValueDiffers, 7),
        ]),
        "readings of the generated headers",
    )
}

/// Whether the port's stage filter of `env` is the golden program's `stage_filter`: the same
/// stage IDs, or the same error (the library's message inside it may differ,
/// `json.error-texts`).
fn same_filter(env: &Envelope, theirs: &Json) -> Result<bool, String> {
    Ok(match env.capture.stage_filter() {
        Ok(None) => theirs.0.is_null(),
        Ok(Some(ids)) => {
            theirs.0.is_array() && strings(theirs)? == ids.into_iter().collect::<Vec<_>>()
        }
        Err(err) => {
            let go = theirs
                .opt("error")
                .map(|e| e.text())
                .transpose()?
                .unwrap_or_default();
            let mine = err.to_string();
            // Both libraries refuse the value, each with its own message.
            let library_errors =
                go.starts_with("capture.stages: json: ") && !mine.contains("string value must be");
            mine == go || library_errors
        }
    })
}

/// The aspects of the port's reading of an envelope that differ from the golden program's.
fn envelope_differences(env: &Envelope, case: &Json) -> Result<Vec<String>, String> {
    let name = case.string("name")?;
    let mut differ = Vec::new();
    if parse_doc(&to_json(env)?, &name)? != parse_doc(case.string("reencoded")?.as_bytes(), &name)?
    {
        differ.push("reencoded".to_owned());
    }
    if !same_filter(env, &case.get("stage_filter")?)? {
        differ.push("stage_filter".to_owned());
    }
    for (member, ours) in [
        ("wants_metrics", Value::Bool(env.capture.wants_metrics())),
        (
            "wants_analytics",
            Value::Bool(env.capture.wants_analytics()),
        ),
        ("effective_mode", Value::from(env.mock.effective_mode())),
        ("effective_level", Value::from(env.mock.effective_level())),
    ] {
        if case.get(member)?.0 != ours {
            differ.push(member.to_owned());
        }
    }
    let ints = case.get("decision_int")?;
    let strs = case.get("decision_string")?;
    for point in DecisionId::ALL {
        let id = point.as_str();
        if ints.get(id)?.0 != env.decision_int(point).map_or(Value::Null, Value::from) {
            differ.push(format!("decision_int {id}"));
        }
        if strs.get(id)?.0 != env.decision_string(point).map_or(Value::Null, Value::from) {
            differ.push(format!("decision_string {id}"));
        }
    }
    for (header, want) in case.get("header_values")?.members()? {
        if strings(&want)? != env.request.headers.values(&header) {
            differ.push(format!("header_values {header}"));
        }
    }
    Ok(differ)
}

/// The envelopes of envelopes.json the port reads otherwise than Go (`json.parity-documents`):
/// the aspects that differ, or serde's refusal.
const ENVELOPE_READINGS: [(&str, &str); 15] = [
    // Go keeps the null of capture.stages, serde reads it as absent (json.null-raw); both read
    // it as no filter.
    ("capture_null", "reencoded"),
    // Go reads the null as the stage "", serde refuses the list (json.null-elements).
    ("capture_null_element", "stage_filter"),
    // Go fills case_id and profile from CASE_ID and Profile, serde ignores them (json.key-case).
    ("case_insensitive_keys", "reencoded"),
    // Go reads -0 as the integer 0, serde_json refuses it (json.negative-zero-int).
    ("decisions", "decision_int pricegranularity.timesplit"),
    // Go keeps the second case_id, the serde derives refuse the document (json.duplicate-keys).
    (
        "duplicate_keys",
        "duplicate field `case_id` at line 1 column 82",
    ),
    // Go pads ["only"] and cuts ["a","b","c"] to two strings, serde refuses them.
    (
        "headers",
        "invalid length 1, expected a tuple of size 2 at line 1 column 193",
    ),
    ("headers_object", "refused by both"),
    ("headers_pair_number", "refused by both"),
    ("headers_pair_object", "refused by both"),
    ("headers_pair_string", "refused by both"),
    ("nested_type_error", "refused by both"),
    ("not_object", "refused by both"),
    ("syntax_error", "refused by both"),
    ("trailing_garbage", "refused by both"),
    ("type_error", "refused by both"),
];

// Go: testdata/go/main.go envelopesGolden (envelopes.json).
#[test]
fn envelopes_match_go() -> TestResult {
    let mut otherwise = BTreeMap::new();
    for case in golden("envelopes.json")?.elements()? {
        let name = case.string("name")?;
        let go_error = case.opt("decode_error").map(|e| e.text()).transpose()?;
        let reading = match (
            serde_json::from_str::<Envelope>(&case.string("input")?),
            go_error,
        ) {
            (Ok(env), None) => envelope_differences(&env, &case)?.join(", "),
            (Err(err), None) => err.to_string(),
            (Err(_), Some(_)) => "refused by both".to_owned(),
            (Ok(_), Some(err)) => format!("refused by Go: {err}"),
        };
        if !reading.is_empty() {
            let _previous = otherwise.insert(name, reading);
        }
    }
    let want = ENVELOPE_READINGS
        .iter()
        .map(|(name, reading)| ((*name).to_owned(), (*reading).to_owned()))
        .collect();
    check_eq(&otherwise, &want, "envelopes the port reads otherwise")
}

/// The content of gzip bytes, when they are gzip.
fn inflated(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut content = Vec::new();
    MultiGzDecoder::new(bytes)
        .read_to_end(&mut content)
        .ok()
        .map(|_| content)
}

/// How the port's body of recorded bytes compares with Go's.
fn recorded_reading(
    ours: &[u8],
    theirs: &[u8],
    encoding: &str,
    what: &str,
) -> Result<Reading, String> {
    if str::from_utf8(theirs).is_err() {
        // Go's json member holds the recorded bytes as they came, invalid UTF-8 included.
        return Ok(Reading::InvalidUtf8);
    }
    if parse_doc(ours, what)? == parse_doc(theirs, what)? {
        Ok(Reading::Same)
    } else if encoding.contains('\u{130}') {
        Ok(Reading::DottedCapitalI)
    } else {
        check_same_json(ours, theirs, what).map(|()| Reading::Same)
    }
}

/// How the port's wire or decoded bytes of an input body compare with Go's.
fn bytes_reading(
    ours: &Result<Vec<u8>, BodyError>,
    theirs: &Result<Vec<u8>, String>,
    what: &str,
) -> Result<Reading, String> {
    let reading = match (ours, theirs) {
        (Ok(mine), Ok(go)) if mine == go => Some(Reading::Same),
        (Ok(mine), Ok(go))
            if mine.is_empty() && (go == b"null" || inflated(go).is_some_and(|c| c == b"null")) =>
        {
            Some(Reading::NullRaw)
        }
        (Ok(mine), Ok(go)) => inflated(mine)
            .filter(|content| inflated(go).as_ref() == Some(content))
            .map(|_| Reading::GzipBytes),
        (Err(BodyError::Base64(_)), Ok(_)) => Some(Reading::Base64),
        (Err(BodyError::Base64(_)), Err(go)) => Some(if go.starts_with("body.b64: ") {
            Reading::BothRefuse
        } else {
            Reading::Base64
        }),
        (Err(BodyError::UnsupportedEncoding(encoding)), _) if encoding.contains('\u{307}') => {
            Some(Reading::DottedCapitalI)
        }
        (Err(BodyError::UnsupportedEncoding(_)), Ok(_)) => None,
        (Err(BodyError::UnsupportedEncoding(_)), Err(go)) => go
            .starts_with("unsupported content encoding ")
            .then_some(Reading::BothRefuse),
        (Err(BodyError::Gzip { encode, .. }), Err(go)) => go
            .starts_with(if *encode {
                "gzip encode: "
            } else {
                "gzip decode: "
            })
            .then_some(Reading::BothRefuse),
        (Err(BodyError::Gzip { .. }), Ok(_)) => Some(Reading::GzipRead),
        (Ok(_), Err(go)) => go.starts_with("gzip decode: ").then_some(Reading::GzipRead),
    };
    reading.ok_or_else(|| format!("{what}: the port gives {ours:?}, Go {theirs:?}"))
}

/// The readings of the bodies of a bodies golden, by case: `<name>` for a recorded body,
/// `<name> wire` and `<name> decoded` for the bytes of an input body.
fn body_readings(g: &Json) -> Result<BTreeMap<String, Reading>, String> {
    let mut readings = BTreeMap::new();
    for case in g.array("recorded")? {
        let name = case.string("name")?;
        let raw = decode_b64(&case.string("raw_b64")?)?;
        let encoding = case.string("content_encoding")?;
        let ours = to_json(&Body::new(&raw, &encoding))?;
        // The exact bytes when the golden has them: a json member passes invalid UTF-8 through.
        let theirs = match case.opt("body_b64") {
            Some(b64) => decode_b64(&b64.text()?)?,
            None => case.string("body")?.into_bytes(),
        };
        let reading = recorded_reading(&ours, &theirs, &encoding, &name)?;
        let _previous = readings.insert(name, reading);
    }
    for case in g.array("inputs")? {
        let name = case.string("name")?;
        let body: Body = from_json(case.string("body")?.as_bytes())?;
        for (what, ours) in [
            ("wire", body.wire_bytes()),
            ("decoded", body.decoded_bytes()),
        ] {
            let theirs = match case.opt(&format!("{what}_error")) {
                Some(err) => Err(err.text()?),
                None => Ok(decode_b64(&case.string(&format!("{what}_b64"))?)?),
            };
            let label = format!("{name} {what}");
            let reading = bytes_reading(&ours, &theirs, &label)?;
            let _previous = readings.insert(label, reading);
        }
    }
    Ok(readings)
}

/// The bodies of bodies.json the port reads otherwise than Go.
const BODY_READINGS: [(&str, Reading); 12] = [
    ("b64_invalid decoded", Reading::BothRefuse),
    ("b64_invalid wire", Reading::BothRefuse),
    // Go skips the line break, the base64 crate refuses it (base64.parity-std-encoding).
    ("b64_newlines decoded", Reading::Base64),
    ("b64_newlines wire", Reading::Base64),
    ("b64_unpadded decoded", Reading::BothRefuse),
    ("b64_unpadded wire", Reading::BothRefuse),
    ("b64_unsupported decoded", Reading::BothRefuse),
    // Go's json member holds the recorded `"\xff"` as it came (json.invalid-utf8).
    ("invalid_utf8_json", Reading::InvalidUtf8),
    // flate2 compresses `{"a":1}` to other bytes than compress/gzip (gzip.payload-bytes).
    ("json_gzip wire", Reading::GzipBytes),
    // Go's json member keeps the `null`, which is the content (json.null-raw).
    ("json_null decoded", Reading::NullRaw),
    ("json_null wire", Reading::NullRaw),
    ("text_unsupported wire", Reading::BothRefuse),
];

// Go: testdata/go/main.go bodiesGolden (bodies.json).
#[test]
fn bodies_match_go() -> TestResult {
    let g = golden("bodies.json")?;
    let otherwise: BTreeMap<String, Reading> = body_readings(&g)?
        .into_iter()
        .filter(|(_, reading)| *reading != Reading::Same)
        .collect();
    let want = BODY_READINGS
        .iter()
        .map(|(name, reading)| ((*name).to_owned(), *reading))
        .collect();
    check_eq(&otherwise, &want, "bodies the port reads otherwise")?;
    check_same_json(
        &to_json(&Body::text_body("a<b"))?,
        g.string("text_body")?.as_bytes(),
        "text body",
    )
}

// Generated bodies (`bodyfuzz.go`): random wire bytes under Content-Encoding spellings (case,
// Unicode spaces, the dotted capital I) and random input bodies with damaged base64.
// Go: testdata/go/bodyfuzz.go fuzzBodiesGolden (bodies_fuzz.json).
#[test]
fn fuzzed_bodies_match_go() -> TestResult {
    let g = golden("bodies_fuzz.json")?;
    check_eq(
        &(g.array("recorded")?.len(), g.array("inputs")?.len()),
        &(300, 300),
        "cases",
    )?;
    check_eq(
        &tally(body_readings(&g)?.into_values()),
        &BTreeMap::from([
            (Reading::Same, 685),
            (Reading::BothRefuse, 115),
            (Reading::InvalidUtf8, 17),
            (Reading::Base64, 28),
            (Reading::GzipBytes, 14),
            (Reading::DottedCapitalI, 41),
        ]),
        "readings of the generated bodies",
    )
}

/// The example `name` read with its Rust type and written as the Go server writes documents.
fn round_trip(name: &str) -> Result<Vec<u8>, String> {
    fn with<T: Serialize + DeserializeOwned>(raw: &[u8]) -> Result<Vec<u8>, String> {
        encode_document(&from_json::<T>(raw)?).map_err(|e| e.to_string())
    }
    let raw = read_file(&format!("contract/examples/{name}"))?;
    match name {
        "envelope.oracle.json" | "envelope.inline.json" | "envelope.bidder_response.json" => {
            with::<Envelope>(&raw)
        }
        "trace.json" => with::<Trace>(&raw),
        "adapter_requests_input.json" => with::<AdapterRequestsInput>(&raw),
        "adapter_requests_output.json" => with::<AdapterRequestsOutput>(&raw),
        "adapter_bids_input.json" => with::<AdapterBidsInput>(&raw),
        "adapter_bids_output.json" => with::<AdapterBidsOutput>(&raw),
        "health.json" => with::<Health>(&raw),
        "config.json" => with::<ConfigDump>(&raw),
        "bidders.json" => with::<BiddersDump>(&raw),
        "reset.json" => with::<ResetResult>(&raw),
        "outbound_meta.json" => with::<OutboundMeta>(&raw),
        other => Err(format!("no Rust type for {other}")),
    }
}

/// The examples this crate has types for (`diff.json` and `index.json` are runner documents).
const EXAMPLES: [&str; 13] = [
    "adapter_bids_input.json",
    "adapter_bids_output.json",
    "adapter_requests_input.json",
    "adapter_requests_output.json",
    "bidders.json",
    "config.json",
    "envelope.bidder_response.json",
    "envelope.inline.json",
    "envelope.oracle.json",
    "health.json",
    "outbound_meta.json",
    "reset.json",
    "trace.json",
];

/// The token error of the parity routes (Go: `parity/wiring` `WriteError`).
const TOKEN_ERROR: &str = "parity: missing or invalid X-Parity-Token <&>";

// Every contract example reads into its serde type and writes back the document Go's mirror
// writes back, as a JSON value.
#[test]
fn documents_match_go() -> TestResult {
    let g = golden("documents.json")?;
    let examples = g.array("examples")?;
    let names: Vec<String> = examples
        .iter()
        .map(|e| e.string("example"))
        .collect::<Result<_, _>>()?;
    check_eq(&names, &owned(&EXAMPLES), "examples")?;
    for case in examples {
        let name = case.string("example")?;
        check_same_json(
            &round_trip(&name)?,
            case.string("encoded")?.as_bytes(),
            &name,
        )?;
    }
    check_same_json(
        &error_document(TOKEN_ERROR),
        g.string("error_document")?.as_bytes(),
        "error document",
    )
}

/// `raw` read as a `T` and written back as the Go server writes documents, or `serde_json`'s
/// refusal; `Err` only when writing fails.
fn reencode<T: Serialize + DeserializeOwned>(
    raw: &[u8],
) -> Result<Result<Vec<u8>, String>, String> {
    serde_json::from_slice::<T>(raw).map_or_else(
        |err| Ok(Err(err.to_string())),
        |v| encode_document(&v).map(Ok).map_err(|e| e.to_string()),
    )
}

/// [`reencode`] with the contract type the golden program names `kind`.
fn reencode_kind(kind: &str, raw: &[u8]) -> Result<Result<Vec<u8>, String>, String> {
    match kind {
        "envelope" => reencode::<Envelope>(raw),
        "trace" => reencode::<Trace>(raw),
        "adapter_requests_input" => reencode::<AdapterRequestsInput>(raw),
        "adapter_requests_output" => reencode::<AdapterRequestsOutput>(raw),
        "adapter_bids_input" => reencode::<AdapterBidsInput>(raw),
        "adapter_bids_output" => reencode::<AdapterBidsOutput>(raw),
        "health" => reencode::<Health>(raw),
        "config" => reencode::<ConfigDump>(raw),
        "bidders" => reencode::<BiddersDump>(raw),
        "reset" => reencode::<ResetResult>(raw),
        "outbound_meta" => reencode::<OutboundMeta>(raw),
        other => Err(format!("no Rust type for {other}")),
    }
}

// Generated documents of every type (`docfuzz.go`): random values (nil, empty and filled lists,
// maps and pointers), some written back by json.Marshal, the others rewritten the ways a runner
// may send them (member order, whitespace, escapes, key case, unknown, missing, repeated and null
// members, number forms, short and long header pairs, raw invalid UTF-8, wrong kinds, broken
// syntax). Both kinds are counted by how the port reads them: a document json.Marshal wrote reads
// as Go reads it but for a null raw member.
#[test]
fn fuzzed_documents_match_go() -> TestResult {
    let cases = golden("documents_fuzz.json")?.elements()?;
    check_eq(&cases.len(), &(11 * 120), "documents")?;
    let mut marshaled = Vec::new();
    let mut rewritten = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let kind = case.string("type")?;
        let input = decode_b64(&case.string("input_b64")?)?;
        let what = format!("document {i} ({kind}): {}", lossy(&input));
        let ours = reencode_kind(&kind, &input)?;
        let theirs = match case.opt("error") {
            Some(err) => Err(err.text()?),
            None => Ok(decode_b64(&case.string("encoded_b64")?)?),
        };
        let reading = document_reading(&ours, &theirs, &what)?;
        if case.boolean("rewritten")? {
            rewritten.push(reading);
        } else {
            marshaled.push(reading);
        }
    }
    check_eq(
        &tally(marshaled),
        &BTreeMap::from([(Reading::Same, 321), (Reading::NullRaw, 4)]),
        "readings of the documents json.Marshal wrote",
    )?;
    // Every document Go reads otherwise (`ValueDiffers`) or refuses with a type error that serde
    // reads (`GoRefuses`) has a key that matches a member only ignoring case (json.key-case).
    check_eq(
        &tally(rewritten),
        &BTreeMap::from([
            (Reading::Same, 199),
            (Reading::BothRefuse, 330),
            (Reading::InvalidUtf8, 219),
            (Reading::RepeatedMember, 1),
            (Reading::Null, 118),
            (Reading::HeaderPair, 7),
            (Reading::GoRefuses, 42),
            (Reading::ValueDiffers, 77),
            (Reading::NullRaw, 2),
        ]),
        "readings of the rewritten documents",
    )
}

// A nil inner map (Go: `map[string]StageEntry(nil)`, `map[string]float64(nil)`) is written back
// as null and an empty one as {}, as Go writes them.
#[test]
fn nil_inner_maps_stay_null() -> TestResult {
    let trace = lossy(&reencode::<Trace>(
        br#"{"stages":{"http.in":null,"bidder.request":{}}}"#,
    )??);
    check(
        trace.contains(r#""stages":{"bidder.request":{},"http.in":null}"#),
        &trace,
    )?;
    let input = lossy(&reencode::<AdapterRequestsInput>(
        br#"{"extra_info":{"currency_rates":{"conversions":{"USD":null,"EUR":{}}}}}"#,
    )??);
    check(
        input.contains(r#""conversions":{"EUR":{},"USD":null}"#),
        &input,
    )
}

/// A health document built from this crate's registries.
fn rust_health() -> Health {
    Health {
        schema: contracts::SCHEMA_HEALTH.to_owned(),
        schema_version: SCHEMA_VERSION.to_owned(),
        implementation: IMPL_RUST.to_owned(),
        build: BuildInfo {
            git_sha: "0000000".to_owned(),
            version: "parity".to_owned(),
        },
        stages_supported: Some(StageId::ALL.to_vec()),
        decisions_supported: Some(DecisionId::ALL.to_vec()),
        endpoints: Some(owned(&["/openrtb2/auction", "/status"])),
        mock_modes: Some(owned(&[ids::MOCK_MODE_ORACLE, ids::MOCK_MODE_INLINE])),
        mock_levels: Some(owned(&[
            ids::MOCK_LEVEL_HTTP,
            ids::MOCK_LEVEL_BIDDER_RESPONSE,
        ])),
        fakes: true,
    }
}

/// Every document this crate writes for the Go schema validator, by file name.
fn documents() -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut docs: BTreeMap<String, Vec<u8>> = EXAMPLES
        .iter()
        .map(|name| Ok(((*name).to_owned(), round_trip(name)?)))
        .collect::<Result<_, String>>()?;
    docs.extend([
        ("error.json".to_owned(), error_document(TOKEN_ERROR)),
        (
            "health.rust.json".to_owned(),
            encode_document(&rust_health()).map_err(|e| e.to_string())?,
        ),
    ]);
    #[cfg(feature = "trace")]
    if crate_path("testdata/golden/sessions.json").exists() {
        for case in golden("sessions.json")?.elements()? {
            let trace = sessions::run(&case)?.trace;
            docs.extend([(
                format!("trace.session.{}.json", case.string("name")?),
                trace,
            )]);
        }
    }
    Ok(docs)
}

/// The JSON files of `dir`.
fn json_files(dir: &Path) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(names);
    };
    for entry in entries {
        let name = entry
            .map_err(|e| e.to_string())?
            .file_name()
            .to_string_lossy()
            .into_owned();
        if is_json(&name) {
            let _new = names.insert(name);
        }
    }
    Ok(names)
}

/// The documents of [`documents`] the Go schema validator refuses, and its message: the
/// `outbound_analytics` script adds a NaN analytics object, which Go notes as a marshal error and
/// leaves out, and `serde_json` writes as `null` (json.non-finite).
const SCHEMA_REFUSALS: [(&str, &str); 1] = [(
    "trace.session.outbound_analytics.json",
    concat!(
        "parity schema: document does not match ",
        "https://prebid.org/pbs-parity/schema/v1/trace.schema.json: ",
        "analytics.2.object: Invalid type. Expected: object, given: null",
    ),
)];

// Every document this crate writes but those of SCHEMA_REFUSALS passed the Go schema validator
// (`schema.Validate`, or `schema.ValidateDefinition` for the documents without a `schema` member)
// when the goldens were built, and the crate still writes exactly those bytes.
#[test]
fn documents_validate_with_go() -> TestResult {
    let dir = crate_path("testdata/documents");
    let docs = documents()?;
    if env::var_os("PBS_PARITY_REGEN").is_some() {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for (name, bytes) in &docs {
            fs::write(dir.join(name), bytes).map_err(|e| format!("{name}: {e}"))?;
        }
        if cfg!(feature = "trace") {
            for stale in json_files(&dir)?.difference(&docs.keys().cloned().collect()) {
                fs::remove_file(dir.join(stale)).map_err(|e| format!("{stale}: {e}"))?;
            }
        }
        return Ok(());
    }
    let verdicts: BTreeMap<String, (String, Option<String>)> = golden("verdicts.json")?
        .elements()?
        .iter()
        .map(|v| {
            Ok((
                v.string("file")?,
                (
                    v.string("sha256")?,
                    v.opt("error").map(|e| e.text()).transpose()?,
                ),
            ))
        })
        .collect::<Result<_, String>>()?;
    let refused: BTreeMap<String, String> = verdicts
        .iter()
        .filter_map(|(file, (_sha, error))| error.clone().map(|e| (file.clone(), e)))
        .collect();
    check_eq(
        &refused,
        &SCHEMA_REFUSALS
            .iter()
            .map(|(file, error)| ((*file).to_owned(), (*error).to_owned()))
            .collect(),
        "the documents the Go schema refuses",
    )?;
    let on_disk = json_files(&dir)?;
    check_eq(
        &on_disk,
        &verdicts.keys().cloned().collect(),
        "every document has a verdict",
    )?;
    for (name, bytes) in &docs {
        let file = read_file(&format!("testdata/documents/{name}"))?;
        check_eq(&lossy(&file), &lossy(bytes), &format!("{name} is current"))?;
        check_eq(
            &verdicts.get(name).map(|(sha, _)| sha.clone()),
            &Some(sha256_hex(bytes)),
            &format!("{name} validated as written"),
        )?;
    }
    if cfg!(feature = "trace") {
        check_eq(
            &docs.keys().cloned().collect::<BTreeSet<_>>(),
            &on_disk,
            "no stale documents",
        )?;
    }
    check(docs.len() > EXAMPLES.len(), "documents beyond the examples")
}

/// The scripted sessions of `sessions.json`, `sessions_fuzz.json` and `sessions_portable.json`,
/// run against [`Session`](crate::trace::Session).
///
/// A script the port runs as Go does has Go's op results and Go's trace, compared as JSON
/// values: each side writes its own spelling of a value, inside the recorded `raw` texts too.
/// A script with a step the two read or write differently by a ledgered difference
/// ([`Divergence`]) is counted by it instead, and its portable form, which Go ran without that
/// step, is compared in full.
#[cfg(feature = "trace")]
mod sessions {
    use std::{
        collections::{BTreeMap, BTreeSet},
        str,
    };

    use pbs_errortypes::GoError;
    use serde::{Serialize, Serializer};
    use serde_json::{Value, value::RawValue};

    use super::{from_json, go_error};
    use crate::{
        contracts::encode_document,
        envelope::Envelope,
        golden::{Doc, Json, Regs, RegsRecord, TestResult, check_eq, decode_b64, lossy},
        ids::{DecisionId, StageId, StageStatus},
        trace::Session,
        tracedoc::OutboundCall,
    };

    /// The Go string of `bytes` as Go's encoders write it: each byte that starts no UTF-8 sequence
    /// becomes U+FFFD, as `encoding/json` and jsoniter write a string holding it.
    fn go_text(bytes: &[u8]) -> String {
        let mut text = String::with_capacity(bytes.len());
        for chunk in bytes.utf8_chunks() {
            text.push_str(chunk.valid());
            text.extend(chunk.invalid().iter().map(|_| char::REPLACEMENT_CHARACTER));
        }
        text
    }

    /// Why the port's trace of a script cannot be Go's.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub(super) enum Divergence {
        /// A NaN or an infinity recorded: Go notes the marshal error, `serde_json` writes `null`
        /// (`json.non-finite`).
        NonFinite,
        /// A raw value that is not JSON: Go's `jsonutil.Marshal` writes it as it is or as
        /// `null`, and a `RawValue` holds only JSON, so the port has no such value.
        RawNotJson,
        /// Recorded bytes that are JSON but for their invalid UTF-8: Go keeps them as `json`,
        /// `serde_json` refuses them (`json.invalid-utf8`).
        InvalidUtf8Json,
        /// A `-0` decision read as an integer: Go reads 0, `serde_json` refuses it
        /// (`json.negative-zero-int`).
        NegativeZero,
        /// A null in `capture.stages`: Go reads it as the stage `""`, serde refuses the list
        /// (`json.null-elements`).
        NullStage,
        /// Two keys of a recorded map that Go writes alike once it replaces their invalid UTF-8,
        /// in an order Go leaves open.
        KeyCollision,
    }

    /// A value of the golden program's `typedValue`.
    enum Typed {
        Int(isize),
        Float(f64),
        Bool(bool),
        Str(String),
        Map(BTreeMap<String, String>),
        Strings(Option<Vec<String>>),
        Raw(Box<RawValue>),
        Regs(RegsRecord),
    }

    impl Typed {
        /// Whether `serde_json` writes the value as Go's writer does: a NaN or an infinity is
        /// `null` here and a marshal error in Go.
        const fn finite(&self) -> bool {
            match self {
                Self::Float(v) => v.is_finite(),
                Self::Int(_)
                | Self::Bool(_)
                | Self::Str(_)
                | Self::Map(_)
                | Self::Strings(_)
                | Self::Raw(_)
                | Self::Regs(_) => true,
            }
        }
    }

    impl Serialize for Typed {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match self {
                Self::Int(v) => v.serialize(serializer),
                Self::Float(v) => v.serialize(serializer),
                Self::Bool(v) => v.serialize(serializer),
                Self::Str(v) => v.serialize(serializer),
                Self::Map(v) => v.serialize(serializer),
                Self::Strings(v) => v.serialize(serializer),
                Self::Raw(v) => v.serialize(serializer),
                Self::Regs(v) => v.serialize(serializer),
            }
        }
    }

    /// A raw JSON text as a `RawValue`, or [`Divergence::RawNotJson`].
    fn raw(text: String) -> Result<Box<RawValue>, Divergence> {
        RawValue::from_string(text)
            .ok()
            .ok_or(Divergence::RawNotJson)
    }

    /// A `bytes_map` value: Go's `map[string]string` of the decoded pairs (the later of two
    /// pairs with one key wins), each key and value as Go writes them.
    fn bytes_map(pairs: &Json) -> Result<Result<Typed, Divergence>, String> {
        let mut by_key: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        for pair in pairs.elements()? {
            let [k, v] = <[Json; 2]>::try_from(pair.elements()?)
                .map_err(|p| format!("not a pair: {p:?}"))?;
            let _previous = by_key.insert(decode_b64(&k.text()?)?, decode_b64(&v.text()?)?);
        }
        let mut map = BTreeMap::new();
        for (k, v) in &by_key {
            let value = go_text(v);
            if let Some(earlier) = map.insert(go_text(k), value.clone())
                && earlier != value
            {
                return Ok(Err(Divergence::KeyCollision));
            }
        }
        Ok(Ok(Typed::Map(map)))
    }

    /// The `value` list of a `strings` or `bytes_strings` op, each element read by `element`.
    fn string_list<F: Fn(&Json) -> Result<String, String>>(
        op: &Json,
        element: F,
    ) -> Result<Option<Vec<String>>, String> {
        op.opt("value")
            .map(|list| list.elements()?.iter().map(&element).collect())
            .transpose()
    }

    /// A `regs_record` value; an empty or absent `ext` is left out, as Go's `omitempty` does.
    fn regs(v: &Json) -> Result<Result<Typed, Divergence>, String> {
        let gdpr = isize::try_from(v.int("gdpr")?).map_err(|e| e.to_string())?;
        let text = v.opt_string("ext")?;
        let ext = if text.is_empty() {
            None
        } else {
            match raw(text) {
                Ok(ext) => Some(ext),
                Err(why) => return Ok(Err(why)),
            }
        };
        Ok(Ok(Typed::Regs(RegsRecord {
            regs: Some(Regs { gdpr, ext }),
        })))
    }

    /// The value a `record` or `analytics` op records, or why the port has none.
    fn typed(op: &Json) -> Result<Result<Typed, Divergence>, String> {
        let value = || op.get("value");
        Ok(Ok(match op.string("type")?.as_str() {
            "int" => Typed::Int(
                value()?
                    .0
                    .as_i64()
                    .and_then(|n| isize::try_from(n).ok())
                    .ok_or("int")?,
            ),
            "float" => Typed::Float(value()?.0.as_f64().ok_or("float")?),
            "float_bits" => Typed::Float(f64::from_bits(
                op.string("text")?
                    .parse()
                    .map_err(|e| format!("float bits: {e}"))?,
            )),
            "nan" => Typed::Float(f64::NAN),
            "bool" => Typed::Bool(value()?.0.as_bool().ok_or("bool")?),
            "string" => Typed::Str(value()?.text()?),
            "map" => Typed::Map(
                value()?
                    .members()?
                    .into_iter()
                    .map(|(k, v)| Ok((k, v.text()?)))
                    .collect::<Result<_, String>>()?,
            ),
            "strings" => Typed::Strings(string_list(op, Json::text)?),
            "bytes_string" => Typed::Str(go_text(&decode_b64(&op.opt_string("text")?)?)),
            "bytes_strings" => {
                Typed::Strings(string_list(op, |s| Ok(go_text(&decode_b64(&s.text()?)?)))?)
            }
            "bytes_map" => return bytes_map(&value()?),
            // The golden program's op members are omitempty: an empty text is absent.
            "raw" => match raw(op.opt_string("text")?) {
                Ok(raw_value) => Typed::Raw(raw_value),
                Err(why) => return Ok(Err(why)),
            },
            "regs_record" => return regs(&value()?),
            "nil_regs" => Typed::Regs(RegsRecord::default()),
            other => return Err(format!("unknown value type {other}")),
        }))
    }

    fn opt_int(op: &Json, key: &str) -> isize {
        op.opt(key)
            .and_then(|v| v.0.as_i64())
            .and_then(|n| isize::try_from(n).ok())
            .unwrap_or(0)
    }

    fn opt_bool(op: &Json, key: &str) -> bool {
        op.opt(key).and_then(|v| v.0.as_bool()).unwrap_or(false)
    }

    /// Notes why the value of an op that takes effect diverges.
    fn diverges(
        takes_effect: bool,
        value: &Result<Typed, Divergence>,
        diverged: &mut BTreeSet<Divergence>,
    ) {
        let why = match value {
            Ok(v) if v.finite() => None,
            Ok(_) => Some(Divergence::NonFinite),
            Err(why) => Some(*why),
        };
        if takes_effect && let Some(why) = why {
            let _new = diverged.insert(why);
        }
    }

    /// Notes [`Divergence::NegativeZero`] when the envelope sets `point` to `-0`, which Go
    /// reads as the integer 0.
    fn integer_decision(s: &Session, point: DecisionId, diverged: &mut BTreeSet<Divergence>) {
        if s.envelope()
            .decision(point)
            .is_some_and(|v| v.get() == "-0")
        {
            let _new = diverged.insert(Divergence::NegativeZero);
        }
    }

    /// Runs one decision op of the golden program's `runOp`, returning its result, or `None`
    /// when `name` is another op.
    fn decision_op(
        s: &Session,
        op: &Json,
        name: &str,
        diverged: &mut BTreeSet<Divergence>,
    ) -> Result<Option<Value>, String> {
        let point = || {
            let id = op.string("point")?;
            DecisionId::from_name(&id).ok_or_else(|| format!("unregistered point {id}"))
        };
        Ok(Some(match name {
            "decide" => {
                let point = point()?;
                integer_decision(s, point, diverged);
                let def = opt_int(op, "def");
                Value::from(s.decide(point, || def))
            }
            "decide_bool" => {
                let point = point()?;
                integer_decision(s, point, diverged);
                let draw = opt_bool(op, "draw");
                Value::Bool(s.decide_bool(point, || draw))
            }
            "decide_chance" => {
                let point = point()?;
                let uncertain = opt_bool(op, "uncertain");
                if uncertain {
                    integer_decision(s, point, diverged);
                }
                let draw = opt_bool(op, "draw");
                Value::Bool(s.decide_chance(point, uncertain, || draw))
            }
            "decide_string" => {
                let def = op.opt_string("def")?;
                Value::from(s.decide_string(point()?, || def))
            }
            "intn" => {
                let point = point()?;
                integer_decision(s, point, diverged);
                let n = opt_int(op, "n");
                let offset = opt_int(op, "offset");
                Value::from(s.intn(point, n, |n| n + offset))
            }
            "last_decision" => s
                .last_decision(point()?)
                .map_or(Value::Null, |v| Value::from(v.get())),
            _ => return Ok(None),
        }))
    }

    /// The errors of an `errors` op.
    fn errors_of(op: &Json) -> Result<Vec<GoError>, String> {
        op.opt("errors").map_or_else(
            || Ok(Vec::new()),
            |list| list.elements()?.iter().map(go_error).collect(),
        )
    }

    /// Runs one op of the golden program's `runOp`, returning its result.
    fn run_op(
        s: &Session,
        op: &Json,
        diverged: &mut BTreeSet<Divergence>,
    ) -> Result<Value, String> {
        let name = op.string("op")?;
        if let Some(result) = decision_op(s, op, &name, diverged)? {
            return Ok(result);
        }
        let stage = || {
            let id = op.string("stage")?;
            StageId::from_name(&id).ok_or_else(|| format!("unregistered stage {id}"))
        };
        let key = op.opt_string("key")?;
        match name.as_str() {
            "record" => {
                let stage = stage()?;
                let value = typed(op)?;
                diverges(s.wants(stage), &value, diverged);
                if let Ok(v) = &value {
                    s.record(stage, &key, v);
                }
            }
            "record_raw" => {
                let stage = stage()?;
                let bytes = decode_b64(&op.opt_string("raw_b64")?)?;
                if s.wants(stage)
                    && str::from_utf8(&bytes).is_err()
                    && RawValue::from_string(go_text(&bytes)).is_ok()
                {
                    let _new = diverged.insert(Divergence::InvalidUtf8Json);
                }
                s.record_raw(stage, &key, &bytes);
            }
            "errors" => s.errors(stage()?, &key, &errors_of(op)?),
            "mark" => {
                let status = op.string("status")?;
                s.mark(
                    stage()?,
                    StageStatus::from_name(&status).ok_or_else(|| format!("status {status}"))?,
                );
            }
            "note" => s.note(op.opt_string("text")?),
            "note_b64" => s.note(go_text(&decode_b64(&op.opt_string("raw_b64")?)?)),
            "outbound" => s.add_outbound(from_json::<OutboundCall>(op.string("call")?.as_bytes())?),
            "analytics" => {
                let kind = op.opt_string("kind")?;
                let value = typed(op)?;
                let takes_effect =
                    s.wants(StageId::Analytics) && s.envelope().capture.wants_analytics();
                diverges(takes_effect, &value, diverged);
                if let Ok(v) = &value {
                    s.add_analytics(&kind, v);
                }
            }
            "next_seq" => return Ok(Value::from(s.next_seq())),
            "wants" => return Ok(Value::Bool(s.wants(stage()?))),
            other => return Err(format!("unknown op {other}")),
        }
        Ok(Value::Null)
    }

    /// What one scripted session gives.
    pub(super) struct Run {
        /// The result of each op.
        results: Vec<Value>,
        /// The trace as the Go server writes it.
        pub(super) trace: Vec<u8>,
        /// Why the trace cannot be Go's.
        diverged: BTreeSet<Divergence>,
    }

    /// Whether `capture.stages` is a list holding null.
    fn null_stage(env: &Envelope) -> bool {
        env.capture
            .stages
            .as_ref()
            .and_then(|stages| serde_json::from_str::<Vec<Option<String>>>(stages.get()).ok())
            .is_some_and(|ids| ids.contains(&None))
    }

    /// Runs one scripted session (a step the port has no value for is left out).
    pub(super) fn run(case: &Json) -> Result<Run, String> {
        let env: Envelope = from_json(case.string("envelope")?.as_bytes())?;
        let mut diverged = BTreeSet::new();
        if null_stage(&env) {
            let _new = diverged.insert(Divergence::NullStage);
        }
        let s = Session::new(env);
        let results = case
            .array("ops")?
            .iter()
            .map(|op| run_op(&s, op, &mut diverged))
            .collect::<Result<Vec<_>, _>>()?;
        let trace = encode_document(&s.snapshot()).map_err(|e| e.to_string())?;
        Ok(Run {
            results,
            trace,
            diverged,
        })
    }

    /// The note [`Session::new`] writes for an invalid `capture.stages`, up to the library's
    /// message.
    const FILTER_NOTE: &str = "capture.stages ignored: capture.stages: ";

    /// The entries of the `stages` member of a trace.
    fn entries(trace: &mut BTreeMap<String, Doc>) -> Vec<&mut BTreeMap<String, Doc>> {
        let Some(Doc::Object(stages)) = trace.get_mut("stages") else {
            return Vec::new();
        };
        stages
            .values_mut()
            .filter_map(|keyed| match keyed {
                Doc::Object(entries) => Some(entries.values_mut()),
                Doc::Null
                | Doc::Bool(_)
                | Doc::Number(_)
                | Doc::String(_)
                | Doc::Escaped(_)
                | Doc::Array(_) => None,
            })
            .flatten()
            .filter_map(|entry| match entry {
                Doc::Object(fields) => Some(fields),
                Doc::Null
                | Doc::Bool(_)
                | Doc::Number(_)
                | Doc::String(_)
                | Doc::Escaped(_)
                | Doc::Array(_) => None,
            })
            .collect()
    }

    /// A trace as a value, with the text of each recorded `raw` read as JSON, and the library's
    /// message in a stage filter note cut off (`json.error-texts`).
    fn normalized_trace(bytes: &[u8], what: &str) -> Result<Doc, String> {
        let mut doc = super::parse_doc(bytes, what)?;
        if let Doc::Object(trace) = &mut doc {
            for entry in entries(trace) {
                if let Some(raw) = entry.get_mut("raw")
                    && let Doc::String(text) = raw
                    && let Ok(parsed) = Doc::parse(text.as_bytes())
                {
                    *raw = parsed;
                }
            }
            if let Some(Doc::Array(notes)) = trace.get_mut("notes") {
                for note in notes {
                    if let Doc::String(text) = note
                        && text.starts_with(FILTER_NOTE)
                        && !text.contains("string value must be")
                    {
                        text.truncate(FILTER_NOTE.len());
                    }
                }
            }
        }
        Ok(doc)
    }

    /// Compares the op results with Go's: a recorded decision value as JSON (Go quotes a string
    /// with `json.Marshal`, which escapes HTML characters), every other result exactly.
    fn check_results(ours: &[Value], case: &Json, name: &str) -> TestResult {
        let ops = case.array("ops")?;
        let theirs = case.array("results")?;
        check_eq(&ours.len(), &theirs.len(), name)?;
        for ((op, mine), go) in ops.iter().zip(ours).zip(&theirs) {
            let same = if let (Value::String(a), Value::String(b)) = (mine, &go.0)
                && op.string("op")? == "last_decision"
            {
                Doc::parse(a.as_bytes())? == Doc::parse(b.as_bytes())?
            } else {
                *mine == go.0
            };
            if !same {
                return Err(format!("{name}: {} gives {mine}, Go {}", op.0, go.0));
            }
        }
        Ok(())
    }

    /// Replays golden scripts: the number that run as in Go, and the others by why they diverge.
    fn replay(cases: &[Json]) -> Result<(usize, BTreeMap<Divergence, usize>), String> {
        let mut same = 0;
        let mut diverging = BTreeMap::new();
        for case in cases {
            let name = case.string("name")?;
            let ran = run(case)?;
            if !ran.diverged.is_empty() {
                for why in ran.diverged {
                    *diverging.entry(why).or_default() += 1;
                }
                continue;
            }
            check_results(&ran.results, case, &name)?;
            let want = decode_b64(&case.string("trace_b64")?)?;
            if normalized_trace(&ran.trace, &name)? != normalized_trace(&want, &name)? {
                return Err(format!(
                    "{name}: not Go's trace\n  ours: {}\n  go:   {}",
                    lossy(&ran.trace),
                    lossy(&want)
                ));
            }
            same += 1;
        }
        Ok((same, diverging))
    }

    // The traces Go's trace.Session records for the same calls, as JSON values.
    // Go: testdata/go/main.go sessionsGolden (sessions.json).
    #[test]
    fn sessions_match_go() -> TestResult {
        let cases = super::golden("sessions.json")?.elements()?;
        check_eq(&cases.len(), &10, "scripts")?;
        for case in &cases {
            check_eq(
                &go_text(&decode_b64(&case.string("trace_b64")?)?),
                &case.string("trace")?,
                "trace and trace_b64 agree",
            )?;
        }
        check_eq(
            &replay(&cases)?,
            &(
                8,
                BTreeMap::from([(Divergence::NonFinite, 2), (Divergence::RawNotJson, 2)]),
            ),
            "scripts run as in Go, and the others by why they diverge",
        )
    }

    // Generated scripts (`fuzz.go`): random envelopes (stage filters, flags, decisions of every
    // JSON kind), keys and values with the characters encoders treat differently, raw bytes that
    // are not JSON or not UTF-8, every error type, and outbound calls, through every op.
    // Go: testdata/go/fuzz.go fuzzSessionsGolden (sessions_fuzz.json).
    #[test]
    fn fuzzed_sessions_match_go() -> TestResult {
        let cases = super::golden("sessions_fuzz.json")?.elements()?;
        check_eq(&cases.len(), &400, "scripts")?;
        check_eq(
            &replay(&cases)?,
            &(
                207,
                BTreeMap::from([
                    (Divergence::NonFinite, 81),
                    (Divergence::RawNotJson, 86),
                    (Divergence::InvalidUtf8Json, 95),
                    (Divergence::NegativeZero, 5),
                    (Divergence::NullStage, 14),
                ]),
            ),
            "scripts run as in Go, and the others by why they diverge",
        )
    }

    // The scripts above that hold an input the port reads otherwise than Go, each without that
    // input (`portable.go`: the ops of a NaN, an infinity, a raw value that is not JSON or bytes
    // that are JSON but not UTF-8 left out, and the envelope's `-0` decisions and null stage IDs
    // written as the `0` and `""` Go reads them as): Go's results and trace for the rest, which
    // the port's must be in full, so a diverging input hides nothing else its script records.
    // Go: testdata/go/portable.go portableSessionsGolden (sessions_portable.json).
    #[test]
    fn portable_sessions_match_go() -> TestResult {
        let twins = super::golden("sessions_portable.json")?.elements()?;
        let names = twins
            .iter()
            .map(|twin| twin.string("name"))
            .collect::<Result<BTreeSet<_>, _>>()?;
        check_eq(&names.len(), &twins.len(), "one portable script per name")?;
        let mut originals = 0;
        for file in ["sessions.json", "sessions_fuzz.json"] {
            for case in super::golden(file)?.elements()? {
                let name = case.string("name")?;
                if !run(&case)?.diverged.is_empty() {
                    originals += 1;
                    if !names.contains(&name) {
                        return Err(format!(
                            "{file}: {name} diverges and has no portable script"
                        ));
                    }
                }
            }
        }
        check_eq(&originals, &(2 + 193), "diverging scripts")?;
        check_eq(
            &replay(&twins)?,
            &(257, BTreeMap::new()),
            "every portable script runs as in Go",
        )
    }
}

/// The fake values, UUID streams and map orders of `cases.json`: what the installed fakes serve
/// inside and between a sequence of cases, through every seam Go routes to them.
#[cfg(feature = "trace")]
mod cases {
    use pbs_util::{
        randomutil::RandomGenerator as _,
        uuidutil::{UuidGenerator as _, UuidRandomGenerator},
    };
    use serde_json::Value;

    use super::{from_json, strings};
    use crate::{
        envelope::{Envelope, FakeValues},
        fakes::{self, ParityUuidGenerator, Source},
        golden::{Json, TestResult, check, check_eq, global_lock},
        trace,
    };

    /// Runs one op of the golden program's `runFakeOp`.
    fn run_op(src: &Source, op: &Json) -> Result<Value, String> {
        let n = op.opt("n").and_then(|n| n.0.as_i64()).unwrap_or(0);
        Ok(match op.string("op")?.as_str() {
            "new_v4" => Value::from(fakes::new_v4().map_err(|e| e.to_string())?),
            "uuidutil" => Value::from(
                ParityUuidGenerator(UuidRandomGenerator)
                    .generate()
                    .map_err(|e| e.to_string())?,
            ),
            "generate" => Value::from(src.generate().map_err(|e| e.to_string())?),
            "now" => {
                let t = fakes::now();
                serde_json::json!({
                    "unix_milli": t.unix_milli(),
                    "unix": t.unix(),
                    "format": t.format("2006-01-02T15:04:05.999999999Z07:00"),
                    "huaweiads": t.format("2006-01-02 15:04:05.000"),
                    "weekday": t.weekday(),
                    "year_day": t.year_day(),
                })
            }
            "int" => Value::from(fakes::int()),
            "int63" => Value::from(src.generate_int63()),
            "intn" => Value::from(src.intn(n).ok_or("intn")?),
            "map_keys" => {
                let keys = op.opt("keys").map(|k| strings(&k)).transpose()?;
                Value::from(fakes::map_keys(keys.unwrap_or_default(), |a, b| a < b))
            }
            "set" => {
                let v: FakeValues = from_json(&op.get("set")?.to_bytes())?;
                src.set(Some(&v));
                Value::Null
            }
            "reset" => {
                src.set(None);
                Value::Null
            }
            "values" => serde_json::to_value(src.values()).map_err(|e| e.to_string())?,
            other => return Err(format!("unknown fake op {other}")),
        })
    }

    async fn replay(src: &Source) -> TestResult {
        let phases = super::golden("cases.json")?.elements()?;
        check(phases.len() > 40, "phases")?;
        for (i, phase) in phases.iter().enumerate() {
            let what = format!("phase {i}");
            let case = match phase.opt("envelope") {
                Some(env) => {
                    Some(trace::begin_case(from_json::<Envelope>(env.text()?.as_bytes())?).await)
                }
                None => None,
            };
            let results = phase
                .array("ops")?
                .iter()
                .map(|op| run_op(src, op))
                .collect::<Result<Vec<_>, _>>();
            drop(case);
            check_eq(
                &results?,
                &phase
                    .get("results")?
                    .elements()?
                    .into_iter()
                    .map(|r| r.0)
                    .collect(),
                &what,
            )?;
        }
        Ok(())
    }

    // Go installs the fakes once and runs the phases: every case applies its envelope's fakes
    // (restarting the UUID stream) and every end restores the defaults (restarting it again).
    #[tokio::test]
    async fn fake_cases_match_go() -> TestResult {
        let _lock = global_lock().await;
        check(fakes::active().is_none(), "the fakes start uninstalled")?;
        let src = fakes::install();
        let result = replay(src).await;
        fakes::uninstall();
        result
    }
}
