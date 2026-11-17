//! The schema names, HTTP surface and matrix domains shared by every parity document
//! (Go: `parity/contracts.go`).
//!
//! The strings are a cross-language contract: they change only together with the schema files
//! and a `schema_version` bump on the Go side.

use pbs_gojson::{GoEncode, GoJson, JsonError, stdjson};

/// The semantic version of every parity document (Go: `parity.SchemaVersion`).
pub const SCHEMA_VERSION: &str = "1.0.0";

/// `schema` of the exec envelope (Go: `parity.SchemaEnvelope`).
pub const SCHEMA_ENVELOPE: &str = "pbs-parity-envelope/1";
/// `schema` of a trace (Go: `parity.SchemaTrace`).
pub const SCHEMA_TRACE: &str = "pbs-parity-trace/1";
/// `schema` of the adapter requests endpoint input (Go: `parity.SchemaAdapterRequestsInput`).
pub const SCHEMA_ADAPTER_REQUESTS_INPUT: &str = "pbs-parity-adapter-requests-input/1";
/// `schema` of the adapter requests endpoint output (Go: `parity.SchemaAdapterRequestsOutput`).
pub const SCHEMA_ADAPTER_REQUESTS_OUTPUT: &str = "pbs-parity-adapter-requests-output/1";
/// `schema` of the adapter bids endpoint input (Go: `parity.SchemaAdapterBidsInput`).
pub const SCHEMA_ADAPTER_BIDS_INPUT: &str = "pbs-parity-adapter-bids-input/1";
/// `schema` of the adapter bids endpoint output (Go: `parity.SchemaAdapterBidsOutput`).
pub const SCHEMA_ADAPTER_BIDS_OUTPUT: &str = "pbs-parity-adapter-bids-output/1";
/// `schema` of a runner diff (Go: `parity.SchemaDiff`).
pub const SCHEMA_DIFF: &str = "pbs-parity-diff/1";
/// `schema` of a runner index (Go: `parity.SchemaIndex`).
pub const SCHEMA_INDEX: &str = "pbs-parity-index/1";
/// `schema` of `GET /__parity/health` (Go: `parity.SchemaHealth`).
pub const SCHEMA_HEALTH: &str = "pbs-parity/1";
/// `schema` of `GET /__parity/config` (Go: `parity.SchemaConfigDump`).
pub const SCHEMA_CONFIG_DUMP: &str = "pbs-parity-config/1";
/// `schema` of `GET /__parity/bidders` (Go: `parity.SchemaBiddersDump`).
pub const SCHEMA_BIDDERS_DUMP: &str = "pbs-parity-bidders/1";
/// `schema` of `POST /__parity/reset` (Go: `parity.SchemaReset`).
pub const SCHEMA_RESET: &str = "pbs-parity-reset/1";
/// `schema` of the stage registry `stages.json` (Go: `parity.SchemaStages`).
pub const SCHEMA_STAGES: &str = "pbs-parity-stages/1";
/// `schema` of the decision registry `decisions.json` (Go: `parity.SchemaDecisions`).
pub const SCHEMA_DECISIONS: &str = "pbs-parity-decisions/1";
/// `schema` of the normalization registry (Go: `parity.SchemaNormalization`).
pub const SCHEMA_NORMALIZATION: &str = "pbs-parity-normalization/1";
/// `schema` of the fakes registry `fakes.json` (Go: `parity.SchemaFakes`).
pub const SCHEMA_FAKES: &str = "pbs-parity-fakes/1";
/// `schema` of the error code registry `error_codes.json` (Go: `parity.SchemaErrorCodes`).
pub const SCHEMA_ERROR_CODES: &str = "pbs-parity-error-codes/1";

/// `impl` of documents the Go server writes (Go: `parity.ImplGo`).
pub const IMPL_GO: &str = "go";
/// `impl` of documents the Rust server writes (Go: `parity.ImplRust`).
pub const IMPL_RUST: &str = "rust";

/// The prefix of every parity introspection route (Go: `parity.PathPrefix`).
pub const PATH_PREFIX: &str = "/__parity/";
/// The header carrying the shared `parity.token` secret (Go: `parity.TokenHeader`).
pub const TOKEN_HEADER: &str = "X-Parity-Token";
/// The header added to every outbound request sent to the oracle (Go: `parity.MetaHeader`).
pub const META_HEADER: &str = "X-Parity-Meta";

/// One of the nine locked parity matrix domains (Go: `parity.Domain`), in matrix row order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Domain {
    /// `endpoints`
    Endpoints,
    /// `config_bootstrap`
    ConfigBootstrap,
    /// `stored_data`
    StoredData,
    /// `auction_lifecycle`
    AuctionLifecycle,
    /// `policy_privacy_identity`
    PolicyPrivacyIdentity,
    /// `bidders_adapters`
    BiddersAdapters,
    /// `hooks_modules`
    HooksModules,
    /// `cache_support_flows`
    CacheSupportFlows,
    /// `cutover_proof`
    CutoverProof,
}

impl Domain {
    /// Every domain in matrix row order (Go: `parity.AllDomains`).
    pub const ALL: [Self; 9] = [
        Self::Endpoints,
        Self::ConfigBootstrap,
        Self::StoredData,
        Self::AuctionLifecycle,
        Self::PolicyPrivacyIdentity,
        Self::BiddersAdapters,
        Self::HooksModules,
        Self::CacheSupportFlows,
        Self::CutoverProof,
    ];

    /// The domain ID.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Endpoints => "endpoints",
            Self::ConfigBootstrap => "config_bootstrap",
            Self::StoredData => "stored_data",
            Self::AuctionLifecycle => "auction_lifecycle",
            Self::PolicyPrivacyIdentity => "policy_privacy_identity",
            Self::BiddersAdapters => "bidders_adapters",
            Self::HooksModules => "hooks_modules",
            Self::CacheSupportFlows => "cache_support_flows",
            Self::CutoverProof => "cutover_proof",
        }
    }

    /// The domain with ID `name`, or `None` when it is not one of the locked domains
    /// (Go: `parity.Domain(name).Valid()`).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_str() == name)
    }
}

/// The bytes the Go server writes for a parity document (Go: `parity/server` `writeJSON`).
///
/// That is `encoding/json`'s `Encoder` with `SetEscapeHTML(false)`, so embedded
/// `json.RawMessage` members keep `<`, `>` and `&`, plus the trailing newline.
///
/// # Errors
///
/// Go's encode error (`json: unsupported value: NaN`, an invalid raw message, ...), which the Go
/// server answers with a 500 `parity: encoding response: <error>`.
pub fn encode_document<T: GoEncode + ?Sized>(doc: &T) -> Result<Vec<u8>, JsonError> {
    stdjson::encode_no_escape_html(doc)
}

/// The body of a `/__parity/*` error answer (Go: `wiring.WriteError`): `{"error": msg}` as
/// `json.Marshal` writes it, without a trailing newline, sent as `application/json`.
#[must_use]
pub fn error_document(msg: &str) -> Vec<u8> {
    stdjson::marshal(&ErrorDocument {
        error: msg.to_owned(),
    })
    .unwrap_or_else(|_| br#"{"error":"parity: internal error"}"#.to_vec())
}

/// Go's `map[string]string{"error": msg}` (one key, so a struct writes the same bytes).
#[derive(GoJson)]
#[gojson(go_type = "map[string]string")]
struct ErrorDocument {
    #[gojson(json = "error")]
    error: String,
}

#[cfg(test)]
mod tests {
    use super::Domain;
    use crate::golden::{TestResult, check, check_eq};

    // Go: parity/envelope_test.go TestDomains.
    #[test]
    fn domains() -> TestResult {
        check_eq(
            &Domain::from_name("endpoints"),
            &Some(Domain::Endpoints),
            "endpoints",
        )?;
        check_eq(
            &Domain::from_name("cutover_proof"),
            &Some(Domain::CutoverProof),
            "cutover_proof",
        )?;
        check_eq(&Domain::from_name("payments"), &None, "payments")?;
        check_eq(&Domain::from_name(""), &None, "empty")?;
        check_eq(&Domain::ALL.len(), &9, "nine domains")?;
        for (i, d) in Domain::ALL.into_iter().enumerate() {
            check(
                Domain::ALL.iter().skip(i + 1).all(|other| other != &d),
                "no duplicate domain",
            )?;
        }
        Ok(())
    }
}
