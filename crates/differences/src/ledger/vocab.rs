//! The closed vocabularies of the ledger: categories, who can notice, statuses, Rule B's cases,
//! the harness's mismatch kinds and classes, and the input predicates.

/// A category: the first segment of an entry's id, and the section of `docs/DIFFERENCES.md` the
/// entry is listed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Category {
    /// The id segment.
    pub(super) id: &'static str,
    /// The section title.
    pub(super) title: &'static str,
}

/// Every category, in the order of the document's sections: the id prefixes the plan's ledger
/// tables use, and those of the libraries DECISIONS.md weighs.
pub(super) const CATEGORIES: &[Category] = &[
    Category {
        id: "adscert",
        title: "ads.cert signing",
    },
    Category {
        id: "analytics",
        title: "Analytics modules",
    },
    Category {
        id: "base64",
        title: "Base64",
    },
    Category {
        id: "cache",
        title: "In-memory caches",
    },
    Category {
        id: "config",
        title: "Configuration loading",
    },
    Category {
        id: "cookie",
        title: "Cookies",
    },
    Category {
        id: "currency",
        title: "Currency rates",
    },
    Category {
        id: "db",
        title: "Databases",
    },
    Category {
        id: "dns",
        title: "DNS lookups",
    },
    Category {
        id: "grpc",
        title: "gRPC",
    },
    Category {
        id: "gvl",
        title: "Global vendor list",
    },
    Category {
        id: "gzip",
        title: "Gzip",
    },
    Category {
        id: "http-client",
        title: "HTTP client",
    },
    Category {
        id: "influx",
        title: "InfluxDB metrics",
    },
    Category {
        id: "ip",
        title: "IP addresses",
    },
    Category {
        id: "json",
        title: "JSON",
    },
    Category {
        id: "jsonschema",
        title: "JSON Schema",
    },
    Category {
        id: "logging",
        title: "Logging",
    },
    Category {
        id: "metrics",
        title: "Prometheus metrics",
    },
    Category {
        id: "mod51d",
        title: "51Degrees device detection",
    },
    Category {
        id: "psl",
        title: "Public suffix list",
    },
    Category {
        id: "rand",
        title: "Random numbers",
    },
    Category {
        id: "regex",
        title: "Regular expressions",
    },
    Category {
        id: "server",
        title: "HTTP server",
    },
    Category {
        id: "size",
        title: "Byte sizes",
    },
    Category {
        id: "sort",
        title: "Sorting",
    },
    Category {
        id: "task",
        title: "Background tasks",
    },
    Category {
        id: "template",
        title: "Templates",
    },
    Category {
        id: "time",
        title: "Time formatting",
    },
    Category {
        id: "tls",
        title: "TLS",
    },
    Category {
        id: "url",
        title: "URLs",
    },
    Category {
        id: "usersync",
        title: "User syncs",
    },
    Category {
        id: "uuid",
        title: "UUIDs",
    },
    Category {
        id: "xml",
        title: "XML",
    },
    Category {
        id: "yaml",
        title: "YAML",
    },
];

/// The category with id `id`.
pub(super) fn category(id: &str) -> Option<&'static Category> {
    CATEGORIES.iter().find(|category| category.id == id)
}

/// Who could notice a difference. `none` stands alone.
pub(super) const WHO: &[&str] = &["publisher", "bidder", "operator", "analytics", "none"];

/// The status of an entry, in the order the document lists them within a category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Status {
    /// The default applies until the user decides.
    DecisionPending,
    /// The crate's behaviour is accepted (Rule A).
    Ledgered,
    /// Go's behaviour is kept.
    Restored,
}

impl Status {
    /// Every status, in document order.
    pub(super) const ALL: [Self; 3] = [Self::DecisionPending, Self::Ledgered, Self::Restored];

    /// The status a fragment spells `name`.
    pub(super) fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.name() == name)
    }

    /// The fragment spelling.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::DecisionPending => "decision-pending",
            Self::Ledgered => "ledgered",
            Self::Restored => "restored",
        }
    }

    /// The heading the document lists the status under.
    pub(super) const fn heading(self) -> &'static str {
        match self {
            Self::DecisionPending => "Decision pending",
            Self::Ledgered => "Ledgered",
            Self::Restored => "Go behaviour kept",
        }
    }
}

/// Why a restored entry keeps Go's behaviour: one of Rule B's four cases, or Prebid's own logic
/// ported onto the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuleB {
    /// Input real production traffic sends that the crate would reject or misread.
    Input,
    /// State one server writes and the other reads while both run side by side.
    SharedState,
    /// The operator interface.
    Operator,
    /// Prebid-authored texts and bytes.
    PrebidTexts,
    /// Prebid's own logic ported onto the crate's API, which is not glue.
    PrebidLogic,
}

impl RuleB {
    /// Every case.
    const ALL: [Self; 5] = [
        Self::Input,
        Self::SharedState,
        Self::Operator,
        Self::PrebidTexts,
        Self::PrebidLogic,
    ];

    /// The case a fragment spells `name`.
    pub(super) fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|case| case.name() == name)
    }

    /// The fragment spelling.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Input => "a",
            Self::SharedState => "b",
            Self::Operator => "c",
            Self::PrebidTexts => "d",
            Self::PrebidLogic => "prebid-logic",
        }
    }

    /// What the case covers, as the document says it.
    pub(super) const fn describe(self) -> &'static str {
        match self {
            Self::Input => {
                "Rule B (a): input real production traffic sends that the crate would reject or \
                 misread"
            }
            Self::SharedState => {
                "Rule B (b): state one server writes and the other reads while Go and Rust run \
                 side by side"
            }
            Self::Operator => {
                "Rule B (c): the operator interface (config keys, defaults, `PBS_` variables, CLI \
                 flags, metric names, labels and types)"
            }
            Self::PrebidTexts => "Rule B (d): Prebid-authored texts and bytes",
            Self::PrebidLogic => "Prebid's own logic ported onto the crate, not glue",
        }
    }
}

/// A harness mismatch kind a pattern may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Kind {
    /// The kind's name in the harness contract.
    pub(super) name: &'static str,
    /// The classes the kind takes; empty when it takes none.
    pub(super) classes: &'static [&'static str],
    /// Whether the pattern's path must be concrete.
    pub(super) concrete: bool,
    /// Whether the pattern must name an input predicate.
    pub(super) needs_input: bool,
}

/// The lexical classes a pattern may name.
const LEXICAL_CLASSES: &[&str] = &[
    "html_escape",
    "unicode_escape",
    "number_format",
    "trailing_newline",
    "whitespace",
    "member_order",
];

/// The wire classes a pattern may name: the harness contract's, and the three the plan adds
/// (contract 1.1.0 lands them with PH-SEMANTIC).
const WIRE_CLASSES: &[&str] = &[
    "header_case",
    "header_order",
    "default_header",
    "http_version",
    "content_length",
    "transfer_encoding",
    "framing",
    "gzip_bytes",
    "cookie_attribute_order",
];

/// A kind that takes no class.
const fn plain(name: &'static str, concrete: bool) -> Kind {
    Kind {
        name,
        classes: &[],
        concrete,
        needs_input: false,
    }
}

/// The kinds a pattern may name. Value, type, missing, extra and status patterns name a concrete
/// path, and a status pattern names an input predicate, so a status mismatch on any other case
/// stays unexpected.
pub(super) const KINDS: &[Kind] = &[
    plain("value", true),
    plain("type", true),
    plain("missing_in_rust", true),
    plain("extra_in_rust", true),
    plain("order", false),
    Kind {
        name: "lexical",
        classes: LEXICAL_CLASSES,
        concrete: false,
        needs_input: false,
    },
    Kind {
        name: "status",
        classes: &[],
        concrete: true,
        needs_input: true,
    },
    plain("header", false),
    Kind {
        name: "wire",
        classes: WIRE_CLASSES,
        concrete: false,
        needs_input: false,
    },
    plain("error_code", false),
    plain("error_text", false),
    plain("metric", false),
    plain("timing", false),
    plain("decision", false),
];

/// The kinds no entry may name, with the reason.
pub(super) const REFUSED_KINDS: &[(&str, &str)] = &[
    (
        "stage_missing",
        "a stage captured on Go only is unported code, never a library difference",
    ),
    (
        "stage_extra",
        "a stage captured on Rust only is never a library difference",
    ),
    (
        "unexpected_outbound",
        "an outbound call that reached no mock is a harness failure",
    ),
];

/// The classes no entry may name, with the reason.
pub(super) const REFUSED_CLASSES: &[(&str, &str)] = &[
    ("other", "it names no difference, so it would ledger any"),
    (
        "unrecorded",
        "it says one side's bytes were not recorded, not how they differ",
    ),
];

/// The kind named `name`.
pub(super) fn kind(name: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|kind| kind.name == name)
}

/// An input predicate: a feature of a case's request or bidder responses that an acceptance
/// pattern requires, so the pattern matches only cases that have it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Input {
    /// The predicate's name.
    pub(super) name: &'static str,
    /// When a case has it.
    pub(super) description: &'static str,
}

/// Every input predicate, in name order. PH-SEMANTIC implements each.
pub(super) const INPUTS: &[Input] = &[
    Input {
        name: "case_folded_key",
        description: "A JSON key in the request body or in a bidder response matches a field of \
                      the Go type only when case is ignored (`ID` for `id`).",
    },
    Input {
        name: "deep_nesting",
        description: "A JSON document in the request body or in a bidder response nests 128 \
                      levels or more, its outermost value included.",
    },
    Input {
        name: "duplicate_keys",
        description: "A JSON object in the request body or in a bidder response has the same \
                      key twice.",
    },
    Input {
        name: "invalid_utf8",
        description: "A JSON string in the request body or in a bidder response holds bytes \
                      that are not UTF-8.",
    },
    Input {
        name: "options_request",
        description: "The request's method is OPTIONS: a CORS preflight, or any other OPTIONS, \
                      which tower-http's CorsLayer answers as one.",
    },
];

/// The input predicate named `name`.
pub(super) fn input(name: &str) -> Option<&'static Input> {
    INPUTS.iter().find(|input| input.name == name)
}

/// Whether `text` is one or more `[A-Z0-9]+` words joined by single hyphens.
fn upper_words(text: &str) -> bool {
    !text.is_empty()
        && text.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        })
}

/// Whether `text` names a decision of docs/DECISIONS.md: `D-` and upper-case words, as `D-UA`.
pub(super) fn is_decision(text: &str) -> bool {
    text.strip_prefix("D-").is_some_and(upper_words)
}

/// Whether `text` names a plan unit, as `JSONUTIL` or `CONFIG-LOADER`.
pub(super) fn is_unit(text: &str) -> bool {
    upper_words(text)
}

/// Whether `text` is one or more `[a-z0-9]+` words joined by single hyphens.
fn lower_words(text: &str) -> bool {
    !text.is_empty()
        && text.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Whether `id` is a dotted entry id: two or more lower-case segments joined by dots, each of
/// hyphen-joined words, as `json.html-escape`.
pub(super) fn is_entry_id(id: &str) -> bool {
    id.contains('.') && id.split('.').all(lower_words)
}

#[cfg(test)]
mod tests {
    use super::{
        CATEGORIES, INPUTS, KINDS, REFUSED_CLASSES, REFUSED_KINDS, RuleB, Status, category, input,
        is_decision, is_entry_id, is_unit, kind,
    };
    use crate::{TestResult, check, check_eq};

    #[test]
    fn categories_are_sorted_and_unique() -> TestResult {
        let ids: Vec<&str> = CATEGORIES.iter().map(|c| c.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        check_eq(&ids, &sorted, "category ids in order, once each")?;
        check(
            CATEGORIES
                .iter()
                .all(|c| is_entry_id(&format!("{}.x", c.id))),
            "every category id is an id segment",
        )?;
        check_eq(&category("json").map(|c| c.title), &Some("JSON"), "lookup")?;
        check(category("jsn").is_none(), "an unknown category")
    }

    /// Every id prefix of the entries the plan's units expect to record: a unit cannot add a
    /// category to this crate, so a missing one would fail its fragment at its merge.
    #[test]
    fn the_plans_categories_are_known() -> TestResult {
        for id in [
            "analytics",
            "base64",
            "cache",
            "config",
            "cookie",
            "currency",
            "gvl",
            "gzip",
            "http-client",
            "influx",
            "ip",
            "json",
            "jsonschema",
            "logging",
            "metrics",
            "mod51d",
            "psl",
            "rand",
            "regex",
            "server",
            "sort",
            "task",
            "time",
            "tls",
            "url",
            "usersync",
            "yaml",
        ] {
            check(category(id).is_some(), id)?;
        }
        Ok(())
    }

    #[test]
    fn inputs_are_sorted_and_unique() -> TestResult {
        let names: Vec<&str> = INPUTS.iter().map(|i| i.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        check_eq(&names, &sorted, "input predicates in order, once each")?;
        check(input("duplicate_keys").is_some(), "lookup")?;
        check(input("duplicate-keys").is_none(), "an unknown predicate")
    }

    #[test]
    fn kinds_cover_the_contract() -> TestResult {
        let mut names: Vec<&str> = KINDS.iter().map(|k| k.name).collect();
        names.extend(REFUSED_KINDS.iter().map(|(name, _)| *name));
        names.sort_unstable();
        let mut contract = [
            "value",
            "type",
            "missing_in_rust",
            "extra_in_rust",
            "order",
            "lexical",
            "status",
            "header",
            "wire",
            "stage_missing",
            "stage_extra",
            "error_code",
            "error_text",
            "metric",
            "timing",
            "decision",
            "unexpected_outbound",
        ];
        contract.sort_unstable();
        check_eq(
            names.as_slice(),
            contract.as_slice(),
            "the 17 mismatch kinds of the contract",
        )?;
        let concrete: Vec<&str> = KINDS
            .iter()
            .filter(|k| k.concrete)
            .map(|k| k.name)
            .collect();
        check_eq(
            concrete.as_slice(),
            [
                "value",
                "type",
                "missing_in_rust",
                "extra_in_rust",
                "status",
            ]
            .as_slice(),
            "the kinds that name a concrete path",
        )?;
        check(
            kind("lexical").is_some_and(|k| k.classes.contains(&"html_escape")),
            "lexical classes",
        )?;
        check(
            kind("wire").is_some_and(|k| k.classes.contains(&"cookie_attribute_order")),
            "wire classes",
        )?;
        check(
            KINDS.iter().all(|k| {
                REFUSED_CLASSES
                    .iter()
                    .all(|(class, _)| !k.classes.contains(class))
            }),
            "no kind takes a refused class",
        )
    }

    #[test]
    fn spellings_round_trip() -> TestResult {
        for status in Status::ALL {
            check_eq(&Status::parse(status.name()), &Some(status), status.name())?;
        }
        for name in ["a", "b", "c", "d", "prebid-logic"] {
            check_eq(&RuleB::parse(name).map(RuleB::name), &Some(name), name)?;
        }
        check(RuleB::parse("e").is_none(), "Rule B has four cases")
    }

    #[test]
    fn ids() -> TestResult {
        check(is_entry_id("json.html-escape"), "a dotted id")?;
        check(
            is_entry_id("http-client.user-agent"),
            "a hyphenated category",
        )?;
        check(!is_entry_id("json"), "one segment")?;
        check(!is_entry_id("json.HTML"), "upper case")?;
        check(!is_entry_id("json..x"), "an empty segment")?;
        check(!is_entry_id("json.-x"), "a leading hyphen")?;
        check(is_decision("D-UA"), "a decision")?;
        check(!is_decision("UA"), "no D- prefix")?;
        check(!is_decision("D-ua"), "lower case")?;
        check(is_unit("CONFIG-LOADER"), "a unit")?;
        check(!is_unit("config-loader"), "lower case")
    }
}
