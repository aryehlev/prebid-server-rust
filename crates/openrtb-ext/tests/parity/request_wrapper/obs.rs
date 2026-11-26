//! The observations of the wrapper goldens, Go's and the port's, and their comparison.
//!
//! An observation is a sequence of parts (Go: `obs` in `testdata/go/request_wrapper/steps.go`),
//! each its kind, its decimal length, `:` and its bytes. They compare part by part: error texts
//! exactly where Prebid wrote them and as `json.error-texts` where the library did (inside
//! Prebid's `req.ext is invalid: ...`), JSON by its tokens (a string by its text and a number by
//! its value, the spelling a lexical class), and ext maps member by member.

use std::{collections::BTreeSet, str::from_utf8};

use pbs_jsonutil::replace_lone_surrogates;

use crate::shapes::{ERROR_TEXTS, NIL_RAW_WRITTEN, Node, compare_encoded, nil_raw_as_null};

/// `json.lone-surrogates-raw`: a raw member Go keeps with a lone surrogate escape, which the
/// port holds as the escape of U+FFFD.
pub(super) const LONE_SURROGATES_RAW: &str = "json.lone-surrogates-raw";

/// `json.non-finite`: NaN and infinities, which Go fails to write and the port writes as null.
pub(super) const NON_FINITE: &str = "json.non-finite";

/// `json.ext-null-member`: an ext map with a null member, which Go's rebuild writes as nothing
/// (the member as nothing, then the ext, which is no longer JSON, as nothing) and the port
/// writes with the member null.
pub(super) const EXT_NULL_MEMBER: &str = "json.ext-null-member";

/// A part of an observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Part {
    /// `e`: an error text, empty for none.
    Err(String),
    /// `j`: JSON that `jsonutil.Marshal` (the port: `pbs_jsonutil::marshal`) wrote.
    Json(String),
    /// `x`: the error text of a marshal that failed.
    MarshalErr(String),
    /// `t`: a flag (`t` or `f`) or a number.
    Text(String),
    /// `m`: an ext map, its members in key order: each key as JSON and its value (`None` for
    /// nil).
    Map(Vec<(String, Option<String>)>),
}

impl Part {
    /// The error text of an `e` part, empty for any other part.
    pub(super) fn error(&self) -> &str {
        match self {
            Self::Err(text) => text,
            Self::Json(_) | Self::MarshalErr(_) | Self::Text(_) | Self::Map(_) => "",
        }
    }
}

/// The parts of `obs` as Go wrote them, each part's bytes as text (an invalid UTF-8 byte as
/// U+FFFD).
pub(super) fn parse(obs: &[u8]) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    let mut rest = obs;
    while let Some((&kind, tail)) = rest.split_first() {
        let bad = || format!("bad observation part at {}", obs.len() - rest.len());
        let colon = tail.iter().position(|&b| b == b':').ok_or_else(bad)?;
        let len: usize = tail
            .get(..colon)
            .and_then(|digits| from_utf8(digits).ok())
            .and_then(|digits| digits.parse().ok())
            .ok_or_else(bad)?;
        let end = colon + 1 + len;
        let bytes = tail.get(colon + 1..end).ok_or_else(bad)?;
        rest = tail.get(end..).ok_or_else(bad)?;
        let text = || String::from_utf8_lossy(bytes).into_owned();
        parts.push(match kind {
            b'e' => Part::Err(text()),
            b'j' => Part::Json(text()),
            b'x' => Part::MarshalErr(text()),
            b't' => Part::Text(text()),
            b'm' => Part::Map(members(bytes)?),
            _ => return Err(bad()),
        });
    }
    Ok(parts)
}

/// The members of Go's `m` part: each a `j` part (the key) and an `r` part (the value) or an `n`
/// part (nil).
fn members(bytes: &[u8]) -> Result<Vec<(String, Option<String>)>, String> {
    let mut members = Vec::new();
    let mut rest = bytes;
    let mut key = None;
    while let Some((&kind, tail)) = rest.split_first() {
        let bad = || format!("bad map part at {}", bytes.len() - rest.len());
        let colon = tail.iter().position(|&b| b == b':').ok_or_else(bad)?;
        let len: usize = tail
            .get(..colon)
            .and_then(|digits| from_utf8(digits).ok())
            .and_then(|digits| digits.parse().ok())
            .ok_or_else(bad)?;
        let end = colon + 1 + len;
        let part = String::from_utf8_lossy(tail.get(colon + 1..end).ok_or_else(bad)?).into_owned();
        rest = tail.get(end..).ok_or_else(bad)?;
        match (kind, key.take()) {
            (b'j', None) => key = Some(part),
            (b'r', Some(key)) => members.push((key, Some(part))),
            (b'n', Some(key)) => members.push((key, None)),
            _ => return Err(bad()),
        }
    }
    if key.is_some() {
        return Err("a map key without a value".to_owned());
    }
    Ok(members)
}

/// The Prebid texts the wrapper and the conversions write (Go: `openrtb_ext/convert_up.go` and
/// `request_wrapper.go`); the port must write them as they are.
const PREBID_TEXTS: &[&str] = &[
    "gdpr must be an integer",
    "request.site.ext.amp must be either 1, 0, or undefined",
    "Requestwrapper RebuildRequest called on a nil BidRequest",
    "ImpWrapper RebuildImp called on a nil Imp",
];

/// The start of the messages of Prebid's `UnmarshalJSON` methods, which the port must write as
/// Go does (`json.prebid-message-context`, restored).
const PREBID_MESSAGE: &str = "request.";

/// The Prebid prefix of an error text that `ConvertUpTo26` puts before an ext's decoding error
/// (`req.ext is invalid: `, `imp[0].imp.ext is invalid: `, ...), and the rest.
fn convert_prefix(text: &str) -> Option<(&str, &str)> {
    let (prefix, rest) = text.split_once(" is invalid: ")?;
    let known = ["req.ext", "req.source.ext", "req.regs.ext", "req.user.ext"].contains(&prefix)
        || prefix
            .strip_prefix("imp[")
            .and_then(|index| index.strip_suffix("].imp.ext"))
            .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()));
    known.then_some((prefix, rest))
}

/// The ledger entries two error texts differ in, or what is unexplained: Prebid's texts must be
/// the same, the library's (`json.error-texts`) may differ; an error on one side alone is
/// unexplained.
fn error_texts(port: &str, go: &str) -> Result<Vec<&'static str>, String> {
    if port == go {
        return Ok(Vec::new());
    }
    let unexplained = || Err(format!("error: port {port:?}, Go {go:?}"));
    if port.is_empty() || go.is_empty() {
        return unexplained();
    }
    if let Some((go_prefix, go_rest)) = convert_prefix(go) {
        return match convert_prefix(port) {
            Some((port_prefix, port_rest)) if port_prefix == go_prefix => {
                error_texts(port_rest, go_rest)
            }
            _ => unexplained(),
        };
    }
    if PREBID_TEXTS.contains(&go) || go.starts_with(PREBID_MESSAGE) {
        return unexplained();
    }
    Ok(vec![ERROR_TEXTS])
}

/// Whether `text` holds a lone surrogate escape, which `replace_lone_surrogates` rewrites.
fn has_lone_surrogate(text: &str) -> bool {
    replace_lone_surrogates(text.as_bytes()).as_ref() != text.as_bytes()
}

/// What stands for a raw value Go wrote as nothing in [`with_holes`]: a string no golden holds.
const HOLE: &str = r#""\u0000nil raw""#;

/// Go's bytes with each raw value it wrote as nothing (a nil `json.RawMessage`, or one that is
/// not JSON, such as an ext map with a nil member) written as [`HOLE`].
fn with_holes(go: &str) -> String {
    go.replace("\":,", &format!("\":{HOLE},"))
        .replace("\":}", &format!("\":{HOLE}}}"))
        .replace("\":]", &format!("\":{HOLE}]"))
}

/// Whether a node holds null: is null, or has a member or element that holds null.
fn holds_null(node: &Node) -> bool {
    match node {
        Node::Scalar(text) => text == "null",
        Node::Object(members) => members.iter().any(|(_, value)| holds_null(value)),
        Node::Array(elements) => elements.iter().any(holds_null),
    }
}

/// Whether the port's node matches Go's, where Go wrote a raw value as nothing (a [`HOLE`]) and
/// the port wrote null (`json.nil-raw-written`) or a value that holds null (the nil member,
/// written null, of an ext map Go could then not write: `json.ext-null-member`), adding the
/// ledger entries and the lexical classes of the scalars to `classes`.
fn fills(port: &Node, go: &Node, classes: &mut Vec<&'static str>) -> bool {
    match (port, go) {
        (Node::Scalar(p), Node::Scalar(g)) if g == HOLE && p == "null" => {
            classes.push(NIL_RAW_WRITTEN);
            true
        }
        (_, Node::Scalar(g)) if g == HOLE => {
            classes.push(EXT_NULL_MEMBER);
            holds_null(port)
        }
        (Node::Object(p), Node::Object(g)) => {
            p.len() == g.len()
                && p.iter()
                    .zip(g)
                    .all(|((pk, pv), (gk, gv))| pk == gk && fills(pv, gv, classes))
        }
        (Node::Array(p), Node::Array(g)) => {
            p.len() == g.len() && p.iter().zip(g).all(|(p, g)| fills(p, g, classes))
        }
        (Node::Scalar(p), Node::Scalar(g)) => compare_encoded(p, g).is_ok_and(|found| {
            classes.extend(found);
            true
        }),
        _ => false,
    }
}

/// The ledger entries two pieces of JSON differ in, or what is unexplained: the same tokens,
/// each other spelling of a string or a number a lexical class, and whitespace the same, as
/// `jsonutil.Marshal` compacts every raw value it writes (`json.raw-not-compacted` is restored).
/// Where Go wrote a raw value as nothing, the port's value must hold the null that made Go's
/// raw value one it could not write (`json.nil-raw-written`).
fn json(port: &str, go: &str) -> Result<Vec<&'static str>, String> {
    let classes = match compare_encoded(port, go) {
        Ok(classes) => classes,
        Err(e) if nil_raw_as_null(go) != go => {
            let mut classes = Vec::new();
            match (Node::parse(port), Node::parse(&with_holes(go))) {
                (Some(p), Some(g)) if fills(&p, &g, &mut classes) => classes,
                _ => return Err(e),
            }
        }
        Err(e) if has_lone_surrogate(go) => {
            let fixed =
                String::from_utf8_lossy(&replace_lone_surrogates(go.as_bytes())).into_owned();
            let mut classes = compare_encoded(port, &fixed).map_err(|_fixed| e)?;
            classes.push(LONE_SURROGATES_RAW);
            classes
        }
        Err(e) => return Err(e),
    };
    if classes.is_empty() && port != go {
        return Err(format!("whitespace: port {port}, Go {go}"));
    }
    Ok(classes)
}

/// JSON whitespace.
const WHITESPACE: [char; 4] = [' ', '\t', '\n', '\r'];

/// The ledger entries two ext members differ in, or what is unexplained: as [`json`], but for
/// whitespace around Go's bytes, which a raw member `SetExt` is given keeps in Go and not in the
/// port, and which nothing writes (every writer compacts the member).
fn member(port: &str, go: &str) -> Result<Vec<&'static str>, String> {
    json(port, go.trim_matches(WHITESPACE))
}

/// The ledger entries two ext maps differ in, or what is unexplained.
fn map(
    port: &[(String, Option<String>)],
    go: &[(String, Option<String>)],
) -> Result<Vec<&'static str>, String> {
    if port.len() != go.len() {
        return Err(format!("map: port {port:?}, Go {go:?}"));
    }
    let mut classes = Vec::new();
    for ((port_key, port_value), (go_key, go_value)) in port.iter().zip(go) {
        classes.extend(json(port_key, go_key)?);
        match (port_value, go_value) {
            (Some(p), Some(g)) => classes.extend(member(p, g)?),
            (None, None) => {}
            _ => {
                return Err(format!(
                    "member {go_key}: port {port_value:?}, Go {go_value:?}"
                ));
            }
        }
    }
    Ok(classes)
}

/// The ledger entries two parts differ in, or what is unexplained.
fn part(port: &Part, go: &Part) -> Result<Vec<&'static str>, String> {
    match (port, go) {
        _ if port == go => Ok(Vec::new()),
        (Part::Err(p), Part::Err(g)) | (Part::MarshalErr(p), Part::MarshalErr(g)) => {
            error_texts(p, g)
        }
        (Part::Json(p), Part::Json(g)) => json(p, g),
        (Part::Map(p), Part::Map(g)) => map(p, g),
        // jsonutil.Marshal fails on NaN and infinities, which serde_json writes as null.
        (Part::Json(_), Part::MarshalErr(g)) if g.contains("unsupported value") => {
            Ok(vec![NON_FINITE])
        }
        _ => Err(format!("port {port:?}, Go {go:?}")),
    }
}

/// The ledger entries two observations differ in, each once, or what is unexplained.
pub(super) fn compare(port: &[Part], go: &[Part]) -> Result<BTreeSet<&'static str>, String> {
    if port.len() != go.len() {
        return Err(format!("parts: port {port:?}, Go {go:?}"));
    }
    let mut classes = BTreeSet::new();
    for (p, g) in port.iter().zip(go) {
        classes.extend(part(p, g)?);
    }
    Ok(classes)
}

/// Whether `port` holds no more than `go`: null for any value, an object's members a subset of
/// Go's (each holding no more), an array's elements each holding no more, a scalar the same.
fn no_more(port: &Node, go: &Node) -> bool {
    match (port, go) {
        (Node::Scalar(p), _) if p == "null" => true,
        (Node::Object(p), Node::Object(g)) => p.iter().all(|(key, value)| {
            g.iter()
                .find(|(k, _)| k == key)
                .is_some_and(|(_, v)| no_more(value, v))
        }),
        (Node::Array(p), Node::Array(g)) => {
            p.len() <= g.len() && p.iter().zip(g).all(|(p, g)| no_more(p, g))
        }
        (Node::Scalar(p), Node::Scalar(g)) => compare_encoded(p, g).is_ok(),
        _ => false,
    }
}

/// [`no_more`] on two pieces of JSON, Go's nil raw values read as null.
fn no_more_json(port: &str, go: &str) -> bool {
    match (Node::parse(port), Node::parse(&nil_raw_as_null(go))) {
        (Some(p), Some(g)) => no_more(&p, &g),
        _ => false,
    }
}

/// Whether the port's observation holds no more than Go's: the same parts, each of the port's
/// holding what Go's does or less (an error where Go's has one, an object's members a subset,
/// null for a value, an ext map's members a subset). It is what Go's decoders leave in a value
/// that failed to decode and the port's do not (`json.partial-results`).
pub(super) fn partial(port: &[Part], go: &[Part]) -> bool {
    port.len() == go.len()
        && port.iter().zip(go).all(|(p, g)| match (p, g) {
            _ if p == g => true,
            (Part::Err(p), Part::Err(g)) => p.is_empty() == g.is_empty(),
            (Part::Json(p), Part::Json(g)) => no_more_json(p, g),
            (Part::Map(p), Part::Map(g)) => p.iter().all(|(key, value)| {
                g.iter().any(|(k, v)| {
                    json(key, k).is_ok()
                        && match (value, v) {
                            (Some(a), Some(b)) => {
                                member(a, b).is_ok() || no_more_json(a, b.trim_matches(WHITESPACE))
                            }
                            (None, None) => true,
                            _ => false,
                        }
                })
            }),
            _ => false,
        })
}
