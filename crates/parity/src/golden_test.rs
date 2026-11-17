//! The comparisons with the goldens the Go program under `testdata/go` writes from the real
//! `parity` packages, and the documents whose Go schema verdicts it records.
//!
//! `PBS_PARITY_REGEN=1` makes [`documents_validate_with_go`] rewrite `testdata/documents`
//! instead of checking it (`testdata/go/run.sh` does that between two runs of the Go program:
//! the first writes the scripted sessions the trace documents come from, the second records the
//! Go schema verdicts on the rewritten documents).

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::Path,
};

use pbs_errortypes::GoError;
use pbs_gocompat::base64::STD_ENCODING;
use pbs_gojson::{GoEncode, StdDecode, stdjson};
use pbs_util::{
    randomutil::RandomGenerator as _, timeutil::Time as _, uuidutil::UuidGenerator as _,
};
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
    golden::{Json, TestResult, check, check_eq, crate_path, golden_json, read_file},
    ids::{self, DecisionId, OutboundKind, StageId, StageStatus},
    trace::{error_record_of, error_records_of, nil_error_record},
    tracedoc::{self, BuildInfo, ErrorRecord, OutboundMeta, Trace},
};

/// The `golden` member of `testdata/golden/<name>`.
fn golden(name: &str) -> Result<Json, String> {
    golden_json(name)?.get("golden")
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `json.Marshal(v)` as text.
fn marshal<T: GoEncode + ?Sized>(v: &T) -> Result<String, String> {
    stdjson::marshal(v)
        .map(|raw| lossy(&raw))
        .map_err(|e| e.to_string())
}

/// `json.Unmarshal(raw, &v)` into a zero value.
fn decode<T: StdDecode + Default>(raw: &[u8]) -> Result<T, String> {
    let mut v = T::default();
    stdjson::unmarshal(raw, &mut v).map_err(|e| e.to_string())?;
    Ok(v)
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

fn decode_b64(text: &str) -> Result<Vec<u8>, String> {
    STD_ENCODING.decode_string(text).map_err(|e| e.to_string())
}

/// The object members of `json`.
fn members(json: &Json) -> Result<Vec<(String, Json)>, String> {
    json.0
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), Json(v.clone())))
                .collect()
        })
        .ok_or_else(|| format!("not an object: {}", json.0))
}

// The vendored contract is byte-identical to the schema files of the parity-harness commit the
// goldens were built from (the Go program refuses to write goldens otherwise), and nothing was
// added to or changed in it since.
#[test]
fn contract_is_the_go_contract() -> TestResult {
    let files = golden("contract.json")?.get("files")?;
    let want: BTreeMap<String, String> = members(&files)?
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

#[test]
fn schemas_match_go() -> TestResult {
    let g = golden("ids.json")?;
    check_eq(
        &SCHEMA_VERSION.to_owned(),
        &g.string("schema_version")?,
        "schema_version",
    )?;
    let schemas: BTreeMap<String, String> = members(&g.get("schemas")?)?
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

#[test]
fn fakes_match_go() -> TestResult {
    let g = golden("fakes.json")?;
    let want_defaults: FakeValues = decode(&g.get("defaults")?.to_bytes())?;
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
        let set: Option<FakeValues> = decode(&case.get("set")?.to_bytes())?;
        let what = case.get("set")?.0.to_string();
        let src = Source::new();
        src.set(set.as_ref());
        let values: Value =
            serde_json::from_str(&marshal(&src.values())?).map_err(|e| e.to_string())?;
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
        for (n, want) in members(&case.get("intn")?)? {
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

#[test]
fn error_records_match_go() -> TestResult {
    let g = golden("errors.json")?;
    let mut all = Vec::new();
    for case in g.array("cases")? {
        let err = go_error(&case.get("spec")?)?;
        check_eq(
            &marshal(&error_record_of(&err))?,
            &case.string("record")?,
            err.message(),
        )?;
        all.push(err);
    }
    check_eq(
        &marshal(&nil_error_record())?,
        &g.string("nil")?,
        "nil error",
    )?;
    let split = |errs: &[GoError]| -> Result<String, String> {
        let (fatal, warnings) = error_records_of(errs);
        let doc: BTreeMap<String, Vec<ErrorRecord>> = BTreeMap::from([
            ("errors".to_owned(), fatal),
            ("warnings".to_owned(), warnings),
        ]);
        marshal(&doc)
    };
    check_eq(&split(&all)?, &g.string("split")?, "split")?;
    check_eq(&split(&[])?, &g.string("split_empty")?, "split empty")
}

#[test]
fn outbound_meta_matches_go() -> TestResult {
    let g = golden("meta.json")?;
    for case in g.array("encoded")? {
        let meta: OutboundMeta = decode(&case.get("meta")?.to_bytes())?;
        check_eq(
            &meta.encode_header().map_err(|e| e.to_string()),
            &Ok(case.string("header")?),
            "encode",
        )?;
    }
    for case in g.array("decoded")? {
        let header = case.string("header")?;
        match OutboundMeta::decode_header(&header) {
            Ok(meta) => {
                let got: Value =
                    serde_json::from_str(&marshal(&meta)?).map_err(|e| e.to_string())?;
                check_eq(&Some(got), &case.opt("meta").map(|m| m.0), &header)?;
            }
            Err(err) => check_eq(
                &Some(err.to_string()),
                &case.opt("error").map(|e| e.text()).transpose()?,
                &header,
            )?,
        }
    }
    Ok(())
}

/// The stage filter as the golden program writes it.
fn filter_json(env: &Envelope) -> Value {
    match env.capture.stage_filter() {
        Ok(None) => Value::Null,
        Ok(Some(ids)) => Value::from(ids.into_iter().collect::<Vec<_>>()),
        Err(err) => serde_json::json!({ "error": err.to_string() }),
    }
}

#[test]
fn envelopes_match_go() -> TestResult {
    for case in golden("envelopes.json")?.elements()? {
        let name = case.string("name")?;
        let input = case.string("input")?;
        let mut env = Envelope::default();
        let decoded = stdjson::unmarshal(input.as_bytes(), &mut env);
        let want_error = case.opt("decode_error").map(|e| e.text()).transpose()?;
        check_eq(
            &decoded.as_ref().err().map(ToString::to_string),
            &want_error,
            &name,
        )?;
        if decoded.is_err() {
            continue;
        }
        check_eq(&marshal(&env)?, &case.string("reencoded")?, &name)?;
        check_eq(&filter_json(&env), &case.get("stage_filter")?.0, &name)?;
        check_eq(
            &env.capture.wants_metrics(),
            &case.boolean("wants_metrics")?,
            &name,
        )?;
        check_eq(
            &env.capture.wants_analytics(),
            &case.boolean("wants_analytics")?,
            &name,
        )?;
        check_eq(
            &env.mock.effective_mode().to_owned(),
            &case.string("effective_mode")?,
            &name,
        )?;
        check_eq(
            &env.mock.effective_level().to_owned(),
            &case.string("effective_level")?,
            &name,
        )?;
        let ints = case.get("decision_int")?;
        let strs = case.get("decision_string")?;
        for point in DecisionId::ALL {
            let what = format!("{name}: {}", point.as_str());
            check_eq(
                &env.decision_int(point).map_or(Value::Null, Value::from),
                &ints.get(point.as_str())?.0,
                &what,
            )?;
            check_eq(
                &env.decision_string(point).map_or(Value::Null, Value::from),
                &strs.get(point.as_str())?.0,
                &what,
            )?;
        }
        for (header, want) in members(&case.get("header_values")?)? {
            let got: Vec<String> = env
                .request
                .headers
                .values(&header)
                .into_iter()
                .map(str::to_owned)
                .collect();
            check_eq(&got, &strings(&want)?, &format!("{name}: {header}"))?;
        }
    }
    Ok(())
}

/// `body` with its `json` member removed, as text.
fn without_json(body: &str) -> Result<String, String> {
    let mut b: Option<Body> = decode(body.as_bytes())?;
    if let Some(b) = b.as_mut() {
        b.json = pbs_gojson::RawJson::nil();
    }
    marshal(&b)
}

/// Compares a Rust body result with the golden's `<what>_b64` / `<what>_error` members; gzip
/// cases may instead report that gzip is not wired in yet.
fn check_bytes_result(
    got: Result<Vec<u8>, BodyError>,
    case: &Json,
    what: &str,
    name: &str,
) -> TestResult {
    let gzip = case.boolean("gzip")?;
    match got {
        Err(BodyError::GzipNotPorted { .. }) if gzip => Ok(()),
        Err(err) => check_eq(
            &Some(err.to_string()),
            &case
                .opt(&format!("{what}_error"))
                .map(|e| e.text())
                .transpose()?,
            &format!("{name}: {what}"),
        ),
        Ok(bytes) => check_eq(
            &Some(STD_ENCODING.encode_to_string(&bytes)),
            &case
                .opt(&format!("{what}_b64"))
                .map(|e| e.text())
                .transpose()?,
            &format!("{name}: {what}"),
        ),
    }
}

#[test]
fn bodies_match_go() -> TestResult {
    let g = golden("bodies.json")?;
    for case in g.array("recorded")? {
        let name = case.string("name")?;
        let raw = decode_b64(&case.string("raw_b64")?)?;
        let got = marshal(&Body::new(&raw, &case.string("content_encoding")?))?;
        let want = case.string("body")?;
        if case.boolean("gzip")? {
            check_eq(&got, &without_json(&want)?, &name)?;
        } else {
            check_eq(&got, &want, &name)?;
        }
    }
    for case in g.array("inputs")? {
        let name = case.string("name")?;
        let body: Body = decode(case.string("body")?.as_bytes())?;
        check_bytes_result(body.wire_bytes(), &case, "wire", &name)?;
        check_bytes_result(body.decoded_bytes(), &case, "decoded", &name)?;
    }
    check_eq(
        &marshal(&Body::text_body("a<b"))?,
        &g.string("text_body")?,
        "text body",
    )
}

/// The example `name` decoded with its Rust type and written as the Go server writes documents.
fn round_trip(name: &str) -> Result<Vec<u8>, String> {
    fn with<T: StdDecode + GoEncode + Default>(raw: &[u8]) -> Result<Vec<u8>, String> {
        encode_document(&decode::<T>(raw)?).map_err(|e| e.to_string())
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
        check_eq(&lossy(&round_trip(&name)?), &case.string("encoded")?, &name)?;
    }
    check_eq(
        &lossy(&error_document(TOKEN_ERROR)),
        &g.string("error_document")?,
        "error document",
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
            let (_results, trace) = sessions::run(&case)?;
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

// Every document this crate writes passed the Go schema validator (`schema.Validate`, or
// `schema.ValidateDefinition` for the documents without a `schema` member) when the goldens
// were built, and the crate still writes exactly those bytes.
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
    for (file, (_sha, error)) in &verdicts {
        check_eq(error, &None, &format!("Go schema verdict on {file}"))?;
    }
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

/// The scripted sessions of `sessions.json`, run against [`Session`](crate::trace::Session).
#[cfg(feature = "trace")]
mod sessions {
    use std::collections::BTreeMap;

    use pbs_errortypes::GoError;
    use pbs_gojson::{GoString, RawJson};
    use serde_json::Value;

    use super::{decode, decode_b64, go_error, lossy, members};
    use crate::{
        contracts::encode_document,
        envelope::Envelope,
        golden::{Json, Regs, RegsRecord, TestResult, check_eq},
        ids::{DecisionId, StageId, StageStatus},
        trace::Session,
        tracedoc::OutboundCall,
    };

    /// A value of the golden program's `typedValue`.
    enum Typed {
        Int(isize),
        Float(f64),
        Bool(bool),
        Str(String),
        Map(BTreeMap<String, String>),
        Strings(Option<Vec<String>>),
        Raw(RawJson),
        Regs(RegsRecord),
    }

    macro_rules! with_typed {
        ($typed:expr, $v:ident => $body:expr) => {
            match $typed {
                Typed::Int($v) => $body,
                Typed::Float($v) => $body,
                Typed::Bool($v) => $body,
                Typed::Str($v) => $body,
                Typed::Map($v) => $body,
                Typed::Strings($v) => $body,
                Typed::Raw($v) => $body,
                Typed::Regs($v) => $body,
            }
        };
    }

    fn typed(op: &Json) -> Result<Typed, String> {
        let value = || op.get("value");
        Ok(match op.string("type")?.as_str() {
            "int" => Typed::Int(
                value()?
                    .0
                    .as_i64()
                    .and_then(|n| isize::try_from(n).ok())
                    .ok_or("int")?,
            ),
            "float" => Typed::Float(value()?.0.as_f64().ok_or("float")?),
            "nan" => Typed::Float(f64::NAN),
            "bool" => Typed::Bool(value()?.0.as_bool().ok_or("bool")?),
            "string" => Typed::Str(value()?.text()?),
            "map" => Typed::Map(
                members(&value()?)?
                    .into_iter()
                    .map(|(k, v)| Ok((k, v.text()?)))
                    .collect::<Result<_, String>>()?,
            ),
            "strings" => {
                let v = value()?;
                Typed::Strings(if v.0.is_null() {
                    None
                } else {
                    Some(super::strings(&v)?)
                })
            }
            "raw" => Typed::Raw(RawJson::from_slice(op.string("text")?.as_bytes())),
            "regs_record" => {
                let v = value()?;
                let gdpr = isize::try_from(v.int("gdpr")?).map_err(|e| e.to_string())?;
                let ext = v
                    .opt("ext")
                    .map(|e| e.text())
                    .transpose()?
                    .map_or_else(RawJson::nil, |e| RawJson::from_slice(e.as_bytes()));
                Typed::Regs(RegsRecord {
                    regs: Some(Regs { gdpr, ext }),
                })
            }
            "nil_regs" => Typed::Regs(RegsRecord::default()),
            other => return Err(format!("unknown value type {other}")),
        })
    }

    fn opt_text(op: &Json, key: &str) -> Result<String, String> {
        Ok(op
            .opt(key)
            .map(|v| v.text())
            .transpose()?
            .unwrap_or_default())
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

    /// Runs one op of the golden program's `runOp`, returning its result.
    fn run_op(s: &Session, op: &Json) -> Result<Value, String> {
        let stage = || {
            let name = op.string("stage")?;
            StageId::from_name(&name).ok_or_else(|| format!("unregistered stage {name}"))
        };
        let point = || {
            let name = op.string("point")?;
            DecisionId::from_name(&name).ok_or_else(|| format!("unregistered point {name}"))
        };
        let key = opt_text(op, "key")?;
        match op.string("op")?.as_str() {
            "record" => {
                let stage = stage()?;
                with_typed!(typed(op)?, v => s.record(stage, &key, &v));
            }
            "record_raw" => s.record_raw(stage()?, &key, &decode_b64(&opt_text(op, "raw_b64")?)?),
            "errors" => {
                let errs: Vec<GoError> = match op.opt("errors") {
                    None => Vec::new(),
                    Some(list) => list
                        .elements()?
                        .iter()
                        .map(go_error)
                        .collect::<Result<_, _>>()?,
                };
                s.errors(stage()?, &key, &errs);
            }
            "mark" => {
                let status = op.string("status")?;
                s.mark(
                    stage()?,
                    StageStatus::from_name(&status).ok_or_else(|| format!("status {status}"))?,
                );
            }
            "note" => s.note(op.string("text")?),
            "note_b64" => s.note(GoString::from_bytes(decode_b64(&op.string("raw_b64")?)?)),
            "outbound" => s.add_outbound(decode::<OutboundCall>(op.string("call")?.as_bytes())?),
            "analytics" => {
                let kind = op.string("kind")?;
                with_typed!(typed(op)?, v => s.add_analytics(&kind, &v));
            }
            "next_seq" => return Ok(Value::from(s.next_seq())),
            "wants" => return Ok(Value::Bool(s.wants(stage()?))),
            "decide" => {
                let def = opt_int(op, "def");
                return Ok(Value::from(s.decide(point()?, || def)));
            }
            "decide_bool" => {
                let draw = opt_bool(op, "draw");
                return Ok(Value::Bool(s.decide_bool(point()?, || draw)));
            }
            "decide_chance" => {
                let draw = opt_bool(op, "draw");
                let uncertain = opt_bool(op, "uncertain");
                return Ok(Value::Bool(s.decide_chance(point()?, uncertain, || draw)));
            }
            "decide_string" => {
                let def = opt_text(op, "def")?;
                return Ok(Value::from(s.decide_string(point()?, || def)));
            }
            "intn" => {
                let n = opt_int(op, "n");
                let offset = opt_int(op, "offset");
                return Ok(Value::from(s.intn(point()?, n, |n| n + offset)));
            }
            "last_decision" => {
                return Ok(s
                    .last_decision(point()?)
                    .map_or(Value::Null, |v| Value::from(lossy(v.as_bytes()))));
            }
            other => return Err(format!("unknown op {other}")),
        }
        Ok(Value::Null)
    }

    /// Runs one scripted session: the op results and the trace as the Go server writes it.
    pub(super) fn run(case: &Json) -> Result<(Vec<Value>, Vec<u8>), String> {
        let env: Envelope = decode(case.string("envelope")?.as_bytes())?;
        let s = Session::new(env);
        let results = case
            .array("ops")?
            .iter()
            .map(|op| run_op(&s, op))
            .collect::<Result<Vec<_>, _>>()?;
        let trace = encode_document(&s.snapshot()).map_err(|e| e.to_string())?;
        Ok((results, trace))
    }

    // The traces Go's trace.Session records for the same calls, byte for byte.
    #[test]
    fn sessions_match_go() -> TestResult {
        let cases = super::golden("sessions.json")?.elements()?;
        check_eq(&cases.len(), &10, "scripts")?;
        for case in cases {
            let name = case.string("name")?;
            let (results, trace) = run(&case)?;
            check_eq(
                &results,
                &case
                    .get("results")?
                    .elements()?
                    .into_iter()
                    .map(|r| r.0)
                    .collect(),
                &name,
            )?;
            check_eq(&lossy(&trace), &case.string("trace")?, &name)?;
        }
        Ok(())
    }
}
