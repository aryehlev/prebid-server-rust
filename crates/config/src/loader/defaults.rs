//! Glue (Rule B (c)): the defaults, environment bindings and bidder keys `config.SetupViper`
//! registers (config/config.go:936-1415), the operator's interface: which keys exist, their
//! defaults and the Go types Viper casts their values to, and which environment variables reach
//! them (ledger: `config.env-names`, `config.weak-typing`).
//!
//! The table is `testdata/go_config_defaults.json` (written by `pbs-devtools go-dump
//! config-defaults` from the Go code) as Rust data; `tests/loader.rs::defaults_match_viper` pins
//! it against that file and against Viper's own `AllSettings` in `testdata/golden/loads.json`.

use super::value::GoValue;

/// A default `config.SetupViper` registers with `v.SetDefault`, by its Go type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum GoDefault {
    /// A `bool`.
    Bool(bool),
    /// An `int`.
    Int(i64),
    /// A `float64`.
    Float(f64),
    /// A `string`.
    Str(&'static str),
    /// A `[]string`.
    Strings(&'static [&'static str]),
    /// `[][]string{}`.
    StringLists,
    /// An empty slice of the named Go type, which Viper does not cast to.
    EmptySlice(&'static str),
    /// `nil` (`v.SetDefault("host_schain_node", nil)`): a key Viper lists with no value.
    Nil,
}

impl GoDefault {
    /// The default as Viper holds it.
    pub(super) fn value(self) -> GoValue {
        match self {
            Self::Bool(b) => GoValue::Bool(b),
            Self::Int(i) => GoValue::Int(i),
            Self::Float(f) => GoValue::Float64(f),
            Self::Str(s) => GoValue::String(s.to_owned()),
            Self::Strings(items) => {
                GoValue::Strings(Some(items.iter().map(|s| (*s).to_owned()).collect()))
            }
            Self::StringLists => GoValue::StringLists(Vec::new()),
            Self::EmptySlice(ty) => GoValue::EmptySlice(ty),
            Self::Nil => GoValue::Nil,
        }
    }
}

/// The defaults of the keys this port reads beyond `config.SetupViper`'s (ruling 10): opt-in
/// extensions, each off by default, which keeps Go's behaviour. They are registered as Go's
/// defaults are, so a value set through `PBS_` or `pbs.yaml` is cast as Viper casts any key with
/// a bool default (`cast.ToBool`, ruling 8). Go's `Configuration` has no field for them, so the
/// decode does not see them.
pub(super) const EXTENSION_DEFAULTS: &[(&str, GoDefault)] = &[
    // HTTP/2 to bidders, with a pool of h2 connections per origin (ruling 10 (a), ledger:
    // `http-client.http2-opt-in`): off, the bidder client speaks HTTP/1.1 only, as Go's does.
    ("http_client.http2.enabled", GoDefault::Bool(false)),
];

/// Keys `config.SetupViper` binds with `v.BindEnv(key)` before `v.SetEnvPrefix("PBS")`
/// (config/config.go:1167-1259), which have no default: Viper reads `PBS_<KEY>` and then the
/// un-prefixed `<KEY>` (with `.` replaced by `_`), e.g. `GDPR_DEFAULT_VALUE`.
pub(super) const UNPREFIXED_BINDINGS: &[&str] = &[
    "user_sync.external_url",
    "user_sync.coop_sync.default",
    "gdpr.default_value",
    "account_defaults.privacy.dsa.default",
    "account_defaults.privacy.dsa.gdpr_only",
];

/// The keys `setBidderDefaults` binds with `v.BindEnv` under `adapters.<bidder>` for every bidder
/// (config/config.go:1383-1415), as Go spells them; `BindEnv` lower-cases them. They have no
/// default, so Viper reads their `PBS_ADAPTERS_<BIDDER>_<KEY>` values uncast.
pub(super) const BIDDER_KEYS: &[&str] = &[
    "disabled",
    "endpoint",
    "extra_info",
    "modifyingVastXmlAllowed",
    "debug.allow",
    "gvlVendorID",
    "usersync_url",
    "experiment.adsCert.enabled",
    "platform_id",
    "app_secret",
    "xapi.username",
    "xapi.password",
    "xapi.tracker",
    "endpointCompression",
    "openrtb.version",
    "openrtb.gpp-supported",
    "usersync.enabled",
    "usersync.key",
    "usersync.default",
    "usersync.iframe.url",
    "usersync.iframe.redirect_url",
    "usersync.iframe.external_url",
    "usersync.iframe.user_macro",
    "usersync.redirect.url",
    "usersync.redirect.redirect_url",
    "usersync.redirect.external_url",
    "usersync.redirect.user_macro",
    "usersync.external_url",
    "usersync.support_cors",
];

/// Every `v.SetDefault` of `config.SetupViper` as Viper's `AllKeys` lists it, sorted.
pub(super) const DEFAULTS: &[(&str, GoDefault)] = &[
    ("account_defaults.debug_allow", GoDefault::Bool(true)),
    ("account_defaults.disabled", GoDefault::Bool(false)),
    (
        "account_defaults.events.default_url",
        GoDefault::Str(
            "https://PBS_HOST/event?t=##PBS-EVENTTYPE##&vtype=##PBS-VASTEVENT##&b=##P\
            BS-BIDID##&f=i&a=##PBS-ACCOUNTID##&ts=##PBS-TIMESTAMP##&bidder=##PBS-BID\
            DER##&int=##PBS-INTEGRATION##&mt=##PBS-MEDIATYPE##&ch=##PBS-CHANNEL##&ai\
            d=##PBS-AUCTIONID##&l=##PBS-LINEID##",
        ),
    ),
    ("account_defaults.events.enabled", GoDefault::Bool(false)),
    ("account_defaults.events_enabled", GoDefault::Bool(false)),
    (
        "account_defaults.price_floors.adjust_for_bid_adjustment",
        GoDefault::Bool(true),
    ),
    (
        "account_defaults.price_floors.enabled",
        GoDefault::Bool(false),
    ),
    (
        "account_defaults.price_floors.enforce_deal_floors",
        GoDefault::Bool(false),
    ),
    (
        "account_defaults.price_floors.enforce_floors_rate",
        GoDefault::Int(100),
    ),
    (
        "account_defaults.price_floors.fetch.enabled",
        GoDefault::Bool(false),
    ),
    (
        "account_defaults.price_floors.fetch.max_age_sec",
        GoDefault::Int(86400),
    ),
    (
        "account_defaults.price_floors.fetch.max_file_size_kb",
        GoDefault::Int(100),
    ),
    (
        "account_defaults.price_floors.fetch.max_rules",
        GoDefault::Int(1000),
    ),
    (
        "account_defaults.price_floors.fetch.max_schema_dims",
        GoDefault::Int(0),
    ),
    (
        "account_defaults.price_floors.fetch.period_sec",
        GoDefault::Int(3600),
    ),
    (
        "account_defaults.price_floors.fetch.timeout_ms",
        GoDefault::Int(3000),
    ),
    (
        "account_defaults.price_floors.fetch.url",
        GoDefault::Str(""),
    ),
    (
        "account_defaults.price_floors.max_rules",
        GoDefault::Int(100),
    ),
    (
        "account_defaults.price_floors.max_schema_dims",
        GoDefault::Int(3),
    ),
    (
        "account_defaults.price_floors.use_dynamic_data",
        GoDefault::Bool(false),
    ),
    (
        "account_defaults.privacy.ipv4.anon_keep_bits",
        GoDefault::Int(24),
    ),
    (
        "account_defaults.privacy.ipv6.anon_keep_bits",
        GoDefault::Int(56),
    ),
    (
        "account_defaults.privacy.privacysandbox.cookiedeprecation.enabled",
        GoDefault::Bool(false),
    ),
    (
        "account_defaults.privacy.privacysandbox.cookiedeprecation.ttl_sec",
        GoDefault::Int(604_800),
    ),
    (
        "account_defaults.privacy.privacysandbox.topicsdomain",
        GoDefault::Str(""),
    ),
    ("account_required", GoDefault::Bool(false)),
    (
        "accounts.cache.background_refresh_backoff_seconds",
        GoDefault::Int(5),
    ),
    (
        "accounts.cache.background_refresh_timeout_seconds",
        GoDefault::Int(10),
    ),
    ("accounts.cache.coalesce_requests", GoDefault::Bool(false)),
    ("accounts.cache.max_entries", GoDefault::Int(50000)),
    ("accounts.cache.negative.enabled", GoDefault::Bool(false)),
    ("accounts.cache.negative.max_entries", GoDefault::Int(10000)),
    ("accounts.cache.negative.ttl_seconds", GoDefault::Int(60)),
    ("accounts.cache.negative.type", GoDefault::Str("lru")),
    ("accounts.cache.refresh", GoDefault::Str("ttl")),
    ("accounts.cache.serve_stale", GoDefault::Bool(false)),
    ("accounts.cache.ttl_seconds", GoDefault::Int(3600)),
    ("accounts.cache.type", GoDefault::Str("lru")),
    ("accounts.cache_events.enabled", GoDefault::Bool(false)),
    ("accounts.cache_events.endpoint", GoDefault::Str("")),
    (
        "accounts.filesystem.directorypath",
        GoDefault::Str("./stored_requests/data/by_id"),
    ),
    ("accounts.filesystem.enabled", GoDefault::Bool(false)),
    ("accounts.http.endpoint", GoDefault::Str("")),
    (
        "accounts.http.use_rfc3986_compliant_request_builder",
        GoDefault::Bool(true),
    ),
    ("accounts.http_events.endpoint", GoDefault::Str("")),
    (
        "accounts.http_events.refresh_rate_seconds",
        GoDefault::Int(0),
    ),
    ("accounts.http_events.timeout_ms", GoDefault::Int(0)),
    ("accounts.in_memory_cache.size_bytes", GoDefault::Int(0)),
    ("accounts.in_memory_cache.ttl_seconds", GoDefault::Int(0)),
    ("accounts.in_memory_cache.type", GoDefault::Str("none")),
    ("accounts.v2_enabled", GoDefault::Bool(false)),
    ("admin.enabled", GoDefault::Bool(true)),
    ("admin_port", GoDefault::Int(6060)),
    ("amp_timeout_adjustment_ms", GoDefault::Int(0)),
    (
        "analytics.agma.accounts",
        GoDefault::EmptySlice("[]config.AgmaAnalyticsAccount"),
    ),
    ("analytics.agma.buffers.count", GoDefault::Int(100)),
    ("analytics.agma.buffers.size", GoDefault::Str("2MB")),
    ("analytics.agma.buffers.timeout", GoDefault::Str("15m")),
    ("analytics.agma.enabled", GoDefault::Bool(false)),
    ("analytics.agma.endpoint.gzip", GoDefault::Bool(false)),
    ("analytics.agma.endpoint.timeout", GoDefault::Str("2s")),
    (
        "analytics.agma.endpoint.url",
        GoDefault::Str("https://go.pbs.agma-analytics.de/v1/prebid-server"),
    ),
    ("analytics.file.filename", GoDefault::Str("")),
    ("analytics.pubstack.buffers.count", GoDefault::Int(100)),
    ("analytics.pubstack.buffers.size", GoDefault::Str("2MB")),
    ("analytics.pubstack.buffers.timeout", GoDefault::Str("900s")),
    (
        "analytics.pubstack.configuration_refresh_delay",
        GoDefault::Str("2h"),
    ),
    ("analytics.pubstack.enabled", GoDefault::Bool(false)),
    (
        "analytics.pubstack.endpoint",
        GoDefault::Str("https://s2s.pbstck.com/v1"),
    ),
    ("analytics.pubstack.scopeid", GoDefault::Str("change-me")),
    ("auction_timeouts_ms.default", GoDefault::Int(0)),
    ("auction_timeouts_ms.max", GoDefault::Int(0)),
    ("auto_gen_source_tid", GoDefault::Bool(true)),
    ("blocked_apps", GoDefault::Strings(&[""])),
    ("cache.default_ttl_seconds.audio", GoDefault::Int(0)),
    ("cache.default_ttl_seconds.banner", GoDefault::Int(0)),
    ("cache.default_ttl_seconds.native", GoDefault::Int(0)),
    ("cache.default_ttl_seconds.video", GoDefault::Int(0)),
    ("cache.expected_millis", GoDefault::Int(10)),
    ("cache.host", GoDefault::Str("")),
    ("cache.query", GoDefault::Str("")),
    ("cache.scheme", GoDefault::Str("")),
    (
        "category_mapping.filesystem.directorypath",
        GoDefault::Str("./static/category-mapping"),
    ),
    ("category_mapping.filesystem.enabled", GoDefault::Bool(true)),
    ("category_mapping.http.endpoint", GoDefault::Str("")),
    ("ccpa.enforce", GoDefault::Bool(false)),
    ("certificates_file", GoDefault::Str("")),
    ("compression.request.enable_gzip", GoDefault::Bool(false)),
    ("compression.response.enable_gzip", GoDefault::Bool(false)),
    (
        "currency_converter.fetch_interval_seconds",
        GoDefault::Int(1800),
    ),
    ("currency_converter.fetch_timeout_ms", GoDefault::Int(60000)),
    (
        "currency_converter.fetch_url",
        GoDefault::Str("https://cdn.jsdelivr.net/gh/prebid/currency-file@1/latest.json"),
    ),
    ("currency_converter.stale_rates_seconds", GoDefault::Int(0)),
    ("datacenter", GoDefault::Str("")),
    ("debug.override_token", GoDefault::Str("")),
    (
        "debug.timeout_notification.fail_only",
        GoDefault::Bool(false),
    ),
    ("debug.timeout_notification.log", GoDefault::Bool(false)),
    (
        "debug.timeout_notification.sampling_rate",
        GoDefault::Float(0.0),
    ),
    ("default_request.alias_info", GoDefault::Bool(false)),
    ("default_request.file.name", GoDefault::Str("")),
    ("default_request.type", GoDefault::Str("")),
    ("event.timeout_ms", GoDefault::Int(1000)),
    (
        "experiment.adscert.inprocess.domain_check_interval_seconds",
        GoDefault::Int(30),
    ),
    (
        "experiment.adscert.inprocess.domain_renewal_interval_seconds",
        GoDefault::Int(30),
    ),
    ("experiment.adscert.inprocess.key", GoDefault::Str("")),
    ("experiment.adscert.inprocess.origin", GoDefault::Str("")),
    ("experiment.adscert.mode", GoDefault::Str("off")),
    (
        "experiment.adscert.remote.signing_timeout_ms",
        GoDefault::Int(5),
    ),
    ("experiment.adscert.remote.url", GoDefault::Str("")),
    ("external_cache.host", GoDefault::Str("")),
    ("external_cache.path", GoDefault::Str("")),
    ("external_cache.scheme", GoDefault::Str("")),
    ("external_url", GoDefault::Str("http://localhost:8000")),
    ("garbage_collector_threshold", GoDefault::Int(0)),
    ("gdpr.amp_exception", GoDefault::Bool(false)),
    (
        "gdpr.eea_countries",
        GoDefault::Strings(&[
            "ALA", "AUT", "BEL", "BGR", "HRV", "CYP", "CZE", "DNK", "EST", "FIN", "FRA", "GUF",
            "DEU", "GIB", "GRC", "GLP", "GGY", "HUN", "ISL", "IRL", "IMN", "ITA", "JEY", "LVA",
            "LIE", "LTU", "LUX", "MLT", "MTQ", "MYT", "NLD", "NOR", "POL", "PRT", "REU", "ROU",
            "BLM", "MAF", "SPM", "SVK", "SVN", "ESP", "SWE", "GBR",
        ]),
    ),
    ("gdpr.enabled", GoDefault::Bool(true)),
    ("gdpr.host_vendor_id", GoDefault::Int(0)),
    (
        "gdpr.live_gvl_refresh_interval_seconds",
        GoDefault::Int(86400),
    ),
    ("gdpr.non_standard_publishers", GoDefault::Strings(&[""])),
    ("gdpr.tcf2.enabled", GoDefault::Bool(true)),
    ("gdpr.tcf2.purpose1.enforce_algo", GoDefault::Str("full")),
    ("gdpr.tcf2.purpose1.enforce_purpose", GoDefault::Bool(true)),
    ("gdpr.tcf2.purpose1.enforce_vendors", GoDefault::Bool(true)),
    (
        "gdpr.tcf2.purpose1.vendor_exceptions",
        GoDefault::Strings(&[]),
    ),
    ("gdpr.tcf2.purpose10.enforce_algo", GoDefault::Str("full")),
    ("gdpr.tcf2.purpose10.enforce_purpose", GoDefault::Bool(true)),
    ("gdpr.tcf2.purpose10.enforce_vendors", GoDefault::Bool(true)),
    (
        "gdpr.tcf2.purpose10.vendor_exceptions",
        GoDefault::Strings(&[]),
    ),
    ("gdpr.tcf2.purpose2.enforce_algo", GoDefault::Str("full")),
    ("gdpr.tcf2.purpose2.enforce_purpose", GoDefault::Bool(true)),
    ("gdpr.tcf2.purpose2.enforce_vendors", GoDefault::Bool(true)),
    (
        "gdpr.tcf2.purpose2.vendor_exceptions",
        GoDefault::Strings(&[]),
    ),
    ("gdpr.tcf2.purpose3.enforce_algo", GoDefault::Str("full")),
    ("gdpr.tcf2.purpose3.enforce_purpose", GoDefault::Bool(true)),
    ("gdpr.tcf2.purpose3.enforce_vendors", GoDefault::Bool(true)),
    (
        "gdpr.tcf2.purpose3.vendor_exceptions",
        GoDefault::Strings(&[]),
    ),
    ("gdpr.tcf2.purpose4.enforce_algo", GoDefault::Str("full")),
    ("gdpr.tcf2.purpose4.enforce_purpose", GoDefault::Bool(true)),
    ("gdpr.tcf2.purpose4.enforce_vendors", GoDefault::Bool(true)),
    (
        "gdpr.tcf2.purpose4.vendor_exceptions",
        GoDefault::Strings(&[]),
    ),
    ("gdpr.tcf2.purpose5.enforce_algo", GoDefault::Str("full")),
    ("gdpr.tcf2.purpose5.enforce_purpose", GoDefault::Bool(true)),
    ("gdpr.tcf2.purpose5.enforce_vendors", GoDefault::Bool(true)),
    (
        "gdpr.tcf2.purpose5.vendor_exceptions",
        GoDefault::Strings(&[]),
    ),
    ("gdpr.tcf2.purpose6.enforce_algo", GoDefault::Str("full")),
    ("gdpr.tcf2.purpose6.enforce_purpose", GoDefault::Bool(true)),
    ("gdpr.tcf2.purpose6.enforce_vendors", GoDefault::Bool(true)),
    (
        "gdpr.tcf2.purpose6.vendor_exceptions",
        GoDefault::Strings(&[]),
    ),
    ("gdpr.tcf2.purpose7.enforce_algo", GoDefault::Str("full")),
    ("gdpr.tcf2.purpose7.enforce_purpose", GoDefault::Bool(true)),
    ("gdpr.tcf2.purpose7.enforce_vendors", GoDefault::Bool(true)),
    (
        "gdpr.tcf2.purpose7.vendor_exceptions",
        GoDefault::Strings(&[]),
    ),
    ("gdpr.tcf2.purpose8.enforce_algo", GoDefault::Str("full")),
    ("gdpr.tcf2.purpose8.enforce_purpose", GoDefault::Bool(true)),
    ("gdpr.tcf2.purpose8.enforce_vendors", GoDefault::Bool(true)),
    (
        "gdpr.tcf2.purpose8.vendor_exceptions",
        GoDefault::Strings(&[]),
    ),
    ("gdpr.tcf2.purpose9.enforce_algo", GoDefault::Str("full")),
    ("gdpr.tcf2.purpose9.enforce_purpose", GoDefault::Bool(true)),
    ("gdpr.tcf2.purpose9.enforce_vendors", GoDefault::Bool(true)),
    (
        "gdpr.tcf2.purpose9.vendor_exceptions",
        GoDefault::Strings(&[]),
    ),
    (
        "gdpr.tcf2.purpose_one_treatment.access_allowed",
        GoDefault::Bool(true),
    ),
    (
        "gdpr.tcf2.purpose_one_treatment.enabled",
        GoDefault::Bool(true),
    ),
    ("gdpr.tcf2.special_feature1.enforce", GoDefault::Bool(true)),
    (
        "gdpr.tcf2.special_feature1.vendor_exceptions",
        GoDefault::EmptySlice("[]openrtb_ext.BidderName"),
    ),
    (
        "gdpr.timeouts_ms.active_vendorlist_fetch",
        GoDefault::Int(0),
    ),
    (
        "gdpr.timeouts_ms.init_vendorlist_fetches",
        GoDefault::Int(0),
    ),
    ("generate_bid_id", GoDefault::Bool(false)),
    ("generate_request_id", GoDefault::Bool(false)),
    ("hooks.enabled", GoDefault::Bool(false)),
    ("host", GoDefault::Str("")),
    ("host_cookie.cookie_name", GoDefault::Str("")),
    ("host_cookie.domain", GoDefault::Str("")),
    ("host_cookie.family", GoDefault::Str("")),
    ("host_cookie.max_cookie_size_bytes", GoDefault::Int(0)),
    ("host_cookie.opt_in_url", GoDefault::Str("")),
    ("host_cookie.opt_out_url", GoDefault::Str("")),
    ("host_cookie.optout_cookie.name", GoDefault::Str("")),
    ("host_cookie.ttl_days", GoDefault::Int(90)),
    ("host_cookie.value", GoDefault::Str("")),
    ("host_schain_node", GoDefault::Nil),
    ("http_client.dialer.keep_alive_seconds", GoDefault::Int(15)),
    ("http_client.dialer.timeout_seconds", GoDefault::Int(30)),
    (
        "http_client.expect_continue_timeout_seconds",
        GoDefault::Int(1),
    ),
    (
        "http_client.idle_connection_timeout_seconds",
        GoDefault::Int(60),
    ),
    ("http_client.max_connections_per_host", GoDefault::Int(0)),
    ("http_client.max_idle_connections", GoDefault::Int(400)),
    (
        "http_client.max_idle_connections_per_host",
        GoDefault::Int(10),
    ),
    (
        "http_client.throttle.enable_throttling",
        GoDefault::Bool(false),
    ),
    (
        "http_client.throttle.long_queue_wait_threshold_ms",
        GoDefault::Int(50),
    ),
    (
        "http_client.throttle.short_queue_wait_threshold_ms",
        GoDefault::Int(10),
    ),
    (
        "http_client.throttle.simulate_throttling_only",
        GoDefault::Bool(false),
    ),
    ("http_client.throttle.throttle_window", GoDefault::Int(1000)),
    (
        "http_client.tls_handshake_timeout_seconds",
        GoDefault::Int(10),
    ),
    (
        "http_client_cache.dialer.keep_alive_seconds",
        GoDefault::Int(15),
    ),
    (
        "http_client_cache.dialer.timeout_seconds",
        GoDefault::Int(30),
    ),
    (
        "http_client_cache.expect_continue_timeout_seconds",
        GoDefault::Int(1),
    ),
    (
        "http_client_cache.idle_connection_timeout_seconds",
        GoDefault::Int(60),
    ),
    (
        "http_client_cache.max_connections_per_host",
        GoDefault::Int(0),
    ),
    ("http_client_cache.max_idle_connections", GoDefault::Int(10)),
    (
        "http_client_cache.max_idle_connections_per_host",
        GoDefault::Int(2),
    ),
    (
        "http_client_cache.tls_handshake_timeout_seconds",
        GoDefault::Int(10),
    ),
    ("lmt.enforce", GoDefault::Bool(true)),
    ("max_request_size", GoDefault::Int(262_144)),
    (
        "metrics.disabled_metrics.account_adapter_details",
        GoDefault::Bool(false),
    ),
    (
        "metrics.disabled_metrics.account_debug",
        GoDefault::Bool(true),
    ),
    (
        "metrics.disabled_metrics.account_stored_responses",
        GoDefault::Bool(true),
    ),
    (
        "metrics.disabled_metrics.adapter_buyeruid_scrubbed",
        GoDefault::Bool(true),
    ),
    (
        "metrics.disabled_metrics.adapter_connections_dial_metrics",
        GoDefault::Bool(true),
    ),
    (
        "metrics.disabled_metrics.adapter_connections_metrics",
        GoDefault::Bool(true),
    ),
    (
        "metrics.disabled_metrics.adapter_gdpr_request_blocked",
        GoDefault::Bool(false),
    ),
    ("metrics.influxdb.align_timestamps", GoDefault::Bool(false)),
    ("metrics.influxdb.database", GoDefault::Str("")),
    ("metrics.influxdb.host", GoDefault::Str("")),
    ("metrics.influxdb.measurement", GoDefault::Str("")),
    ("metrics.influxdb.metric_send_interval", GoDefault::Int(20)),
    ("metrics.influxdb.password", GoDefault::Str("")),
    ("metrics.influxdb.username", GoDefault::Str("")),
    ("metrics.prometheus.namespace", GoDefault::Str("")),
    ("metrics.prometheus.port", GoDefault::Int(0)),
    ("metrics.prometheus.subsystem", GoDefault::Str("")),
    ("metrics.prometheus.timeout_ms", GoDefault::Int(10000)),
    ("port", GoDefault::Int(8000)),
    ("price_floors.enabled", GoDefault::Bool(false)),
    ("price_floors.fetcher.cache_size_mb", GoDefault::Int(64)),
    ("price_floors.fetcher.capacity", GoDefault::Int(20000)),
    (
        "price_floors.fetcher.http_client.dialer.keep_alive_seconds",
        GoDefault::Int(15),
    ),
    (
        "price_floors.fetcher.http_client.dialer.timeout_seconds",
        GoDefault::Int(30),
    ),
    (
        "price_floors.fetcher.http_client.expect_continue_timeout_seconds",
        GoDefault::Int(1),
    ),
    (
        "price_floors.fetcher.http_client.idle_connection_timeout_seconds",
        GoDefault::Int(60),
    ),
    (
        "price_floors.fetcher.http_client.max_connections_per_host",
        GoDefault::Int(0),
    ),
    (
        "price_floors.fetcher.http_client.max_idle_connections",
        GoDefault::Int(40),
    ),
    (
        "price_floors.fetcher.http_client.max_idle_connections_per_host",
        GoDefault::Int(2),
    ),
    (
        "price_floors.fetcher.http_client.tls_handshake_timeout_seconds",
        GoDefault::Int(10),
    ),
    ("price_floors.fetcher.max_retries", GoDefault::Int(10)),
    ("price_floors.fetcher.worker", GoDefault::Int(20)),
    ("recaptcha_secret", GoDefault::Str("")),
    (
        "request_timeout_headers.request_time_in_queue",
        GoDefault::Str(""),
    ),
    (
        "request_timeout_headers.request_timeout_in_queue",
        GoDefault::Str(""),
    ),
    (
        "request_validation.ipv4_private_networks",
        GoDefault::Strings(&[
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "169.254.0.0/16",
            "127.0.0.0/8",
        ]),
    ),
    (
        "request_validation.ipv6_private_networks",
        GoDefault::Strings(&[
            "::1/128",
            "fc00::/7",
            "fe80::/10",
            "ff00::/8",
            "2001:db8::/32",
        ]),
    ),
    ("status_response", GoDefault::Str("")),
    ("stored_requests.cache_events_api", GoDefault::Bool(false)),
    (
        "stored_requests.database.connection.dbname",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.connection.driver",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.connection.host",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.connection.password",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.connection.port",
        GoDefault::Int(0),
    ),
    (
        "stored_requests.database.connection.query_string",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.connection.tls.client_cert",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.connection.tls.client_key",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.connection.tls.root_cert",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.connection.user",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.fetcher.amp_query",
        GoDefault::Str(""),
    ),
    ("stored_requests.database.fetcher.query", GoDefault::Str("")),
    (
        "stored_requests.database.initialize_caches.amp_query",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.initialize_caches.query",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.initialize_caches.timeout_ms",
        GoDefault::Int(0),
    ),
    (
        "stored_requests.database.poll_for_updates.amp_query",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.poll_for_updates.query",
        GoDefault::Str(""),
    ),
    (
        "stored_requests.database.poll_for_updates.refresh_rate_seconds",
        GoDefault::Int(0),
    ),
    (
        "stored_requests.database.poll_for_updates.timeout_ms",
        GoDefault::Int(0),
    ),
    (
        "stored_requests.directorypath",
        GoDefault::Str("./stored_requests/data/by_id"),
    ),
    (
        "stored_requests.filesystem.directorypath",
        GoDefault::Str("./stored_requests/data/by_id"),
    ),
    ("stored_requests.filesystem.enabled", GoDefault::Bool(false)),
    ("stored_requests.http.amp_endpoint", GoDefault::Str("")),
    ("stored_requests.http.endpoint", GoDefault::Str("")),
    (
        "stored_requests.http.use_rfc3986_compliant_request_builder",
        GoDefault::Bool(true),
    ),
    (
        "stored_requests.http_events.amp_endpoint",
        GoDefault::Str(""),
    ),
    ("stored_requests.http_events.endpoint", GoDefault::Str("")),
    (
        "stored_requests.http_events.refresh_rate_seconds",
        GoDefault::Int(0),
    ),
    ("stored_requests.http_events.timeout_ms", GoDefault::Int(0)),
    (
        "stored_requests.in_memory_cache.imp_cache_size_bytes",
        GoDefault::Int(0),
    ),
    (
        "stored_requests.in_memory_cache.request_cache_size_bytes",
        GoDefault::Int(0),
    ),
    (
        "stored_requests.in_memory_cache.resp_cache_size_bytes",
        GoDefault::Int(0),
    ),
    (
        "stored_requests.in_memory_cache.ttl_seconds",
        GoDefault::Int(0),
    ),
    (
        "stored_requests.in_memory_cache.type",
        GoDefault::Str("none"),
    ),
    ("stored_requests_timeout_ms", GoDefault::Int(50)),
    (
        "stored_responses.cache_events.enabled",
        GoDefault::Bool(false),
    ),
    ("stored_responses.cache_events.endpoint", GoDefault::Str("")),
    (
        "stored_responses.database.connection.dbname",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.connection.driver",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.connection.host",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.connection.password",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.connection.port",
        GoDefault::Int(0),
    ),
    (
        "stored_responses.database.connection.query_string",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.connection.tls.client_cert",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.connection.tls.client_key",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.connection.tls.root_cert",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.connection.user",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.fetcher.amp_query",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.fetcher.query",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.initialize_caches.amp_query",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.initialize_caches.query",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.initialize_caches.timeout_ms",
        GoDefault::Int(0),
    ),
    (
        "stored_responses.database.poll_for_updates.amp_query",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.poll_for_updates.query",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.database.poll_for_updates.refresh_rate_seconds",
        GoDefault::Int(0),
    ),
    (
        "stored_responses.database.poll_for_updates.timeout_ms",
        GoDefault::Int(0),
    ),
    (
        "stored_responses.filesystem.directorypath",
        GoDefault::Str(""),
    ),
    (
        "stored_responses.filesystem.enabled",
        GoDefault::Bool(false),
    ),
    ("stored_responses.http.endpoint", GoDefault::Str("")),
    ("stored_responses.http_events.endpoint", GoDefault::Str("")),
    (
        "stored_responses.http_events.refresh_rate_seconds",
        GoDefault::Int(0),
    ),
    ("stored_responses.http_events.timeout_ms", GoDefault::Int(0)),
    (
        "stored_responses.in_memory_cache.imp_cache_size_bytes",
        GoDefault::Int(0),
    ),
    (
        "stored_responses.in_memory_cache.request_cache_size_bytes",
        GoDefault::Int(0),
    ),
    (
        "stored_responses.in_memory_cache.resp_cache_size_bytes",
        GoDefault::Int(0),
    ),
    (
        "stored_responses.in_memory_cache.ttl_seconds",
        GoDefault::Int(0),
    ),
    (
        "stored_responses.in_memory_cache.type",
        GoDefault::Str("none"),
    ),
    (
        "stored_video_req.cache_events.enabled",
        GoDefault::Bool(false),
    ),
    ("stored_video_req.cache_events.endpoint", GoDefault::Str("")),
    (
        "stored_video_req.database.connection.dbname",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.connection.driver",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.connection.host",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.connection.password",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.connection.port",
        GoDefault::Int(0),
    ),
    (
        "stored_video_req.database.connection.query_string",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.connection.tls.client_cert",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.connection.tls.client_key",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.connection.tls.root_cert",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.connection.user",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.fetcher.amp_query",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.fetcher.query",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.initialize_caches.amp_query",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.initialize_caches.query",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.initialize_caches.timeout_ms",
        GoDefault::Int(0),
    ),
    (
        "stored_video_req.database.poll_for_updates.amp_query",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.poll_for_updates.query",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.database.poll_for_updates.refresh_rate_seconds",
        GoDefault::Int(0),
    ),
    (
        "stored_video_req.database.poll_for_updates.timeout_ms",
        GoDefault::Int(0),
    ),
    (
        "stored_video_req.filesystem.directorypath",
        GoDefault::Str(""),
    ),
    (
        "stored_video_req.filesystem.enabled",
        GoDefault::Bool(false),
    ),
    ("stored_video_req.http.endpoint", GoDefault::Str("")),
    ("stored_video_req.http_events.endpoint", GoDefault::Str("")),
    (
        "stored_video_req.http_events.refresh_rate_seconds",
        GoDefault::Int(0),
    ),
    ("stored_video_req.http_events.timeout_ms", GoDefault::Int(0)),
    (
        "stored_video_req.in_memory_cache.imp_cache_size_bytes",
        GoDefault::Int(0),
    ),
    (
        "stored_video_req.in_memory_cache.request_cache_size_bytes",
        GoDefault::Int(0),
    ),
    (
        "stored_video_req.in_memory_cache.resp_cache_size_bytes",
        GoDefault::Int(0),
    ),
    (
        "stored_video_req.in_memory_cache.ttl_seconds",
        GoDefault::Int(0),
    ),
    (
        "stored_video_req.in_memory_cache.type",
        GoDefault::Str("none"),
    ),
    (
        "tmax_adjustments.bidder_network_latency_buffer_ms",
        GoDefault::Int(0),
    ),
    (
        "tmax_adjustments.bidder_response_duration_min_ms",
        GoDefault::Int(0),
    ),
    ("tmax_adjustments.enabled", GoDefault::Bool(false)),
    (
        "tmax_adjustments.pbs_response_preparation_duration_ms",
        GoDefault::Int(0),
    ),
    ("tmax_default", GoDefault::Int(0)),
    ("unix_socket_enable", GoDefault::Bool(false)),
    ("unix_socket_name", GoDefault::Str("prebid-server.sock")),
    ("user_sync.priority_groups", GoDefault::StringLists),
    (
        "user_sync.redirect_url",
        GoDefault::Str(
            "{{.ExternalURL}}/setuid?bidder={{.SyncerKey}}&gdpr={{.GDPR}}&gdpr_consen\
            t={{.GDPRConsent}}&gpp={{.GPP}}&gpp_sid={{.GPPSID}}&f={{.SyncType}}&uid=\
            {{.UserMacro}}",
        ),
    ),
    (
        "validations.banner_creative_max_size",
        GoDefault::Str("skip"),
    ),
    ("validations.max_creative_size.height", GoDefault::Int(0)),
    ("validations.max_creative_size.width", GoDefault::Int(0)),
    ("validations.secure_markup", GoDefault::Str("skip")),
    ("video.enable_deprecated_endpoint", GoDefault::Bool(false)),
    ("vtrack.allow_unknown_bidder", GoDefault::Bool(true)),
    ("vtrack.enabled", GoDefault::Bool(true)),
    ("vtrack.timeout_ms", GoDefault::Int(2000)),
];
