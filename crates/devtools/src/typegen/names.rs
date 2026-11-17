//! Go identifiers and JSON keys to Rust identifiers.

use core::mem;

/// Words that cannot be plain Rust identifiers (strict and reserved keywords, 2024 edition).
const KEYWORDS: &[&str] = &[
    "Self", "abstract", "as", "async", "await", "become", "box", "break", "const", "continue",
    "crate", "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if",
    "impl", "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub",
    "ref", "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// Keywords that cannot be raw identifiers either.
const NOT_RAW: &[&str] = &["Self", "crate", "self", "super"];

/// Splits a Go identifier into words.
///
/// `BidFloorCur` → `Bid Floor Cur`, `IABContent10` → `IAB Content10`, `MIMEs` → `MIMEs` (a
/// lower-case `s` after an initialism is its plural), `Connection2G` → `Connection2G` and
/// `IP2Location` → `IP2 Location` (digits stay with what precedes them), `us_privacy` → `us
/// privacy`.
#[must_use]
pub fn words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut out = Vec::new();
    let mut word = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || !c.is_alphanumeric() {
            if !word.is_empty() {
                out.push(mem::take(&mut word));
            }
            continue;
        }
        let prev = i.checked_sub(1).and_then(|j| chars.get(j)).copied();
        let next = chars.get(i + 1).copied();
        let after = chars.get(i + 2).copied();
        let boundary = c.is_uppercase()
            && match prev {
                Some(p) if p.is_lowercase() => true,
                Some(p) if p.is_ascii_digit() => next.is_some_and(char::is_lowercase),
                Some(p) if p.is_uppercase() => {
                    // The last capital of an initialism starts the next word (`IABContent`),
                    // unless what follows is a plural `s` (`MIMEs`, `EIDsFoo`).
                    next.is_some_and(char::is_lowercase)
                        && (next != Some('s') || after.is_some_and(char::is_lowercase))
                }
                _ => false,
            };
        if boundary && !word.is_empty() {
            out.push(mem::take(&mut word));
        }
        word.push(c);
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// `snake_case` from a Go identifier.
#[must_use]
pub fn snake(name: &str) -> String {
    words(name)
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// `SCREAMING_SNAKE_CASE` from a Go identifier.
#[must_use]
pub fn screaming(name: &str) -> String {
    words(name)
        .iter()
        .map(|w| w.to_uppercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// A field name from a JSON key: kept when it is already `snake_case`, otherwise split into
/// words like a Go identifier (`placementId` → `placement_id`).
#[must_use]
pub fn snake_from_json(key: &str) -> String {
    let simple = key
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if simple { key.to_owned() } else { snake(key) }
}

/// `UpperCamelCase` for a Go type name that is not one already (unexported or underscored).
#[must_use]
pub fn type_name(go: &str) -> String {
    let exported = go.chars().next().is_some_and(char::is_uppercase);
    if exported && !go.contains('_') {
        return go.to_owned();
    }
    words(go)
        .iter()
        .map(|w| {
            let mut chars = w.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect::<String>()
            })
        })
        .collect()
}

/// Whether `name` is a usable identifier as written (ASCII, not starting with a digit).
#[must_use]
pub fn is_ident(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name != "_"
}

/// The identifier as Rust source spells it: keywords become raw identifiers (`r#type`), and the
/// few that cannot be raw get a trailing underscore.
#[must_use]
pub fn escape(name: &str) -> String {
    if NOT_RAW.contains(&name) {
        format!("{name}_")
    } else if KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_owned()
    }
}

/// The JSON key a field's tag gives it, as `encoding/json` reads it (`None` for no usable name).
#[must_use]
pub fn json_key(tag: Option<&str>) -> Option<&str> {
    let tag = tag?;
    if tag == "-" {
        return None;
    }
    let name = tag.split(',').next().unwrap_or_default();
    valid_tag_name(name).then_some(name)
}

/// encoding/json's `isValidTag`.
fn valid_tag_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| {
            "!#$%&()*+-./:;<=>?@[]^_{|}~ ".contains(c) || c.is_alphabetic() || c.is_numeric()
        })
}

#[cfg(test)]
mod tests {
    use super::{escape, json_key, screaming, snake, snake_from_json, type_name, words};
    use crate::{error::Result, testutil::check_eq};

    #[test]
    fn splits_go_identifiers() -> Result<()> {
        for (go, want) in [
            ("BidFloorCur", "bid_floor_cur"),
            ("ID", "id"),
            ("ImpID", "imp_id"),
            ("MIMEs", "mimes"),
            ("EIDs", "eids"),
            ("APIs", "apis"),
            ("URLsFoo", "urls_foo"),
            ("IABContent10", "iab_content10"),
            ("QAGMediaRating", "qag_media_rating"),
            ("DIDSHA1", "didsha1"),
            ("us_privacy", "us_privacy"),
            ("placementId", "placement_id"),
            ("Html5Video", "html5_video"),
            ("Connection2G", "connection2g"),
            ("IP2Location", "ip2_location"),
            ("OS3DS", "os3ds"),
            ("DistributionChannel", "distribution_channel"),
            ("x", "x"),
        ] {
            check_eq(snake(go).as_str(), want, go)?;
        }
        check_eq(
            screaming("CatTaxIABContent10").as_str(),
            "CAT_TAX_IAB_CONTENT10",
            "screaming",
        )?;
        check_eq(
            screaming("APIVPAID10").as_str(),
            "APIVPAID10",
            "initialism and digits",
        )?;
        check_eq(
            &words("a__b"),
            &vec!["a".to_owned(), "b".to_owned()],
            "underscores",
        )
    }

    #[test]
    fn names_fields_types_and_keywords() -> Result<()> {
        check_eq(snake_from_json("gpp_sid").as_str(), "gpp_sid", "snake key")?;
        check_eq(snake_from_json("siteID").as_str(), "site_id", "camel key")?;
        check_eq(escape("type").as_str(), "r#type", "keyword")?;
        check_eq(escape("self").as_str(), "self_", "not raw")?;
        check_eq(escape("id").as_str(), "id", "plain")?;
        check_eq(type_name("PMP").as_str(), "PMP", "exported")?;
        check_eq(type_name("adapter").as_str(), "Adapter", "unexported")?;
        check_eq(type_name("Ext_Imp").as_str(), "ExtImp", "underscore")?;
        check_eq(&json_key(Some("id,omitempty")), &Some("id"), "tag")?;
        check_eq(&json_key(Some(",omitempty")), &None, "no name")?;
        check_eq(&json_key(Some("-")), &None, "dash")?;
        check_eq(&json_key(Some("-,")), &Some("-"), "dash name")?;
        check_eq(&json_key(Some("a\"b")), &None, "invalid")?;
        check_eq(&json_key(None), &None, "no tag")
    }
}
