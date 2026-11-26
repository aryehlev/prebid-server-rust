//! Validation: fragments as written become the ledger, or the list of everything wrong with them.

use std::collections::BTreeMap;

use super::{
    fragment::{Fragment, RawEntry, RawPattern, RawRestoration},
    pattern::PathPattern,
    pins::{Found, Pin, Sources},
    vocab::{
        self, CATEGORIES, Category, INPUTS, Input, KINDS, Kind, REFUSED_CLASSES, REFUSED_KINDS,
        RuleB, Status, WHO,
    },
};

/// The validated ledger.
#[derive(Debug)]
pub(super) struct Ledger {
    /// The fragments read, relative to the workspace root, in path order.
    pub(super) fragments: Vec<String>,
    /// Every entry, in id order.
    pub(super) entries: Vec<Entry>,
}

/// One difference.
#[derive(Debug)]
pub(super) struct Entry {
    /// The dotted id.
    pub(super) id: String,
    /// The category.
    pub(super) category: &'static Category,
    /// The status.
    pub(super) status: Status,
    /// One line on the difference.
    pub(super) title: String,
    /// What Go does.
    pub(super) go: String,
    /// What Rust does.
    pub(super) rust: String,
    /// Who could notice.
    pub(super) who: Vec<&'static str>,
    /// Where it shows.
    pub(super) visible_in: String,
    /// How the harness recognises it.
    pub(super) harness: Vec<Pattern>,
    /// The pin.
    pub(super) pin: String,
    /// The restore option, or what keeps Go's behaviour.
    pub(super) restore: Option<String>,
    /// What applies until the user decides.
    pub(super) default: Option<String>,
    /// The decision it depends on.
    pub(super) decision: Option<String>,
    /// The Rule B case.
    pub(super) rule_b: Option<RuleB>,
    /// The units whose merge brings the restore pin.
    pub(super) restore_by: Vec<String>,
    /// The restore pins, in pin order.
    pub(super) restorations: Vec<Restoration>,
    /// Anything else a reader needs.
    pub(super) note: Option<String>,
    /// The fragment that records it.
    pub(super) fragment: String,
}

impl Entry {
    /// Whether the entry keeps Go's behaviour but no restore pin has landed yet.
    pub(super) const fn restore_pending(&self) -> bool {
        matches!(self.status, Status::Restored) && self.restorations.is_empty()
    }
}

/// A harness pattern.
#[derive(Debug)]
pub(super) struct Pattern {
    /// The mismatch kind.
    pub(super) kind: &'static Kind,
    /// The lexical or wire class.
    pub(super) class: Option<&'static str>,
    /// The path pattern.
    pub(super) path: PathPattern,
    /// The input predicate.
    pub(super) input: Option<&'static Input>,
}

impl Pattern {
    /// The pattern in words, as messages and the document give it.
    pub(super) fn describe(&self) -> String {
        let class = self.class.map_or_default(|class| format!(" {class}"));
        let input = self
            .input
            .map_or_default(|input| format!(" when the case has {}", input.name));
        format!("{}{class} at {}{input}", self.kind.name, self.path)
    }

    /// Whether one mismatch could match both patterns.
    fn overlaps(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.class == other.class
            && (self.input == other.input || self.input.is_none() || other.input.is_none())
            && self.path.overlaps(&other.path)
    }
}

/// A restore pin.
#[derive(Debug)]
pub(super) struct Restoration {
    /// The test that compares the kept behaviour with a Go golden.
    pub(super) pin: String,
    /// Anything else a reader needs.
    pub(super) note: Option<String>,
    /// The fragment that brings it.
    pub(super) fragment: String,
}

/// The problems of one entry or restoration, each prefixed with where it is.
struct Problems<'p> {
    /// Every problem found so far.
    list: &'p mut Vec<String>,
    /// The fragment and the entry.
    at: String,
}

impl Problems<'_> {
    fn add(&mut self, message: &str) {
        self.list.push(format!("{}: {message}", self.at));
    }

    /// Checks a text field: one line of Markdown, trimmed, with its code spans closed.
    fn prose(&mut self, field: &str, text: &str) {
        if text.trim().is_empty() {
            self.add(&format!("{field} is empty"));
            return;
        }
        if text.trim() != text {
            self.add(&format!("{field} has leading or trailing whitespace"));
        }
        if text.chars().any(char::is_control) {
            self.add(&format!(
                "{field} holds a line break or another control character (wrap a long text \
                 with a TOML line-ending backslash)"
            ));
        }
        if !text.matches('`').count().is_multiple_of(2) {
            self.add(&format!("{field} has an odd number of backticks"));
        }
    }

    /// Checks an optional text field.
    fn optional_prose(&mut self, field: &str, text: Option<&String>) {
        if let Some(text) = text {
            self.prose(field, text);
        }
    }

    /// Checks a pin and returns it as written.
    fn pin(&mut self, field: &str, text: &str, sources: &mut Sources) -> String {
        match Pin::parse(text).and_then(|pin| sources.find(&pin)) {
            Ok(Found::Test) => {}
            Ok(Found::Ignored) => self.add(&format!(
                "{field} {text}: the test is marked #[ignore], so cargo test never runs it"
            )),
            Ok(Found::NoTest) => self.add(&format!(
                "{field} {text}: the file has no test function of that name"
            )),
            Ok(Found::NoFile) => self.add(&format!("{field} {text}: no such file")),
            Err(e) => self.add(&format!("{field}: {e}")),
        }
        text.to_owned()
    }
}

/// Validates the fragments read from the workspace, given with their paths.
///
/// # Errors
///
/// Every problem found, each naming its fragment and entry.
pub(super) fn validate(
    fragments: Vec<(String, Fragment)>,
    sources: &mut Sources,
) -> Result<Ledger, Vec<String>> {
    let mut problems = Vec::new();
    let mut paths = Vec::new();
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    let mut entries = Vec::new();
    let mut restorations = Vec::new();
    for (path, fragment) in fragments {
        if fragment.difference.is_empty() && fragment.restoration.is_empty() {
            problems.push(format!(
                "{path}: holds no [[difference]] or [[restoration]] table"
            ));
        }
        for raw in fragment.difference {
            if let Some(first) = ids.get(&raw.id) {
                problems.push(format!(
                    "{path}: {}: the id is already used in {first}",
                    raw.id
                ));
                continue;
            }
            let _previous = ids.insert(raw.id.clone(), path.clone());
            if let Some(entry) = entry(raw, &path, sources, &mut problems) {
                entries.push(entry);
            }
        }
        restorations.extend(
            fragment
                .restoration
                .into_iter()
                .map(|raw| (path.clone(), raw)),
        );
        paths.push(path);
    }
    for (path, raw) in restorations {
        restoration(raw, &path, &ids, &mut entries, sources, &mut problems);
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    for entry in &mut entries {
        entry.restorations.sort_by(|a, b| a.pin.cmp(&b.pin));
        if entry.restore_pending() && entry.restore_by.is_empty() {
            problems.push(format!(
                "{}: {}: a restored entry needs a restore pin: a [[restoration]] table, in the \
                 fragment of the crate that keeps Go's behaviour, or restore_by naming the units \
                 whose merge brings it",
                entry.fragment, entry.id
            ));
        }
    }
    overlaps(&entries, &mut problems);
    paths.sort();
    if problems.is_empty() {
        Ok(Ledger {
            fragments: paths,
            entries,
        })
    } else {
        Err(problems)
    }
}

/// Validates one `[[difference]]` table.
fn entry(
    raw: RawEntry,
    fragment: &str,
    sources: &mut Sources,
    list: &mut Vec<String>,
) -> Option<Entry> {
    let before = list.len();
    let mut problems = Problems {
        list,
        at: format!("{fragment}: {}", raw.id),
    };
    let category = check_id(&raw, &mut problems);
    let status = Status::parse(&raw.status);
    if status.is_none() {
        problems.add(&format!(
            "unknown status {:?}: ledgered, restored, decision-pending or extension",
            raw.status
        ));
    }
    problems.prose("title", &raw.title);
    problems.prose("go", &raw.go);
    problems.prose("rust", &raw.rust);
    problems.prose("visible_in", &raw.visible_in);
    problems.optional_prose("restore", raw.restore.as_ref());
    problems.optional_prose("default", raw.default.as_ref());
    problems.optional_prose("note", raw.note.as_ref());
    let who = check_who(&raw.who, &mut problems);
    let pin = problems.pin("pin", &raw.pin, sources);
    let harness: Vec<Pattern> = raw
        .harness
        .iter()
        .enumerate()
        .filter_map(|(n, raw_pattern)| {
            pattern(raw_pattern)
                .map_err(|e| problems.add(&format!("harness pattern {}: {e}", n + 1)))
                .ok()
        })
        .collect();
    let rule_b = check_references(&raw, &mut problems);
    if let Some(status) = status {
        by_status(status, &raw, &mut problems);
    }
    if problems.list.len() > before {
        return None;
    }
    Some(Entry {
        category: category?,
        status: status?,
        id: raw.id,
        title: raw.title,
        go: raw.go,
        rust: raw.rust,
        who,
        visible_in: raw.visible_in,
        harness,
        pin,
        restore: raw.restore,
        default: raw.default,
        decision: raw.decision,
        rule_b,
        restore_by: raw.restore_by,
        restorations: Vec::new(),
        note: raw.note,
        fragment: fragment.to_owned(),
    })
}

/// Checks the id and the category, and returns the category.
fn check_id(raw: &RawEntry, problems: &mut Problems<'_>) -> Option<&'static Category> {
    if !vocab::is_entry_id(&raw.id) {
        problems.add(
            "the id is not dotted lower-case words, its first segment the category \
             (json.html-escape)",
        );
    }
    let category = vocab::category(&raw.category);
    match category {
        None => problems.add(&format!(
            "unknown category {:?}; the categories are {}",
            raw.category,
            CATEGORIES
                .iter()
                .map(|known| known.id)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Some(known) if raw.id.split('.').next() != Some(known.id) => problems.add(&format!(
            "the id does not start with its category, {}.",
            known.id
        )),
        Some(_) => {}
    }
    category
}

/// Checks the decision, the Rule B case and the units of `restore_by`, and returns the case.
fn check_references(raw: &RawEntry, problems: &mut Problems<'_>) -> Option<RuleB> {
    if let Some(decision) = &raw.decision
        && !vocab::is_decision(decision)
    {
        problems.add(&format!(
            "decision {decision:?} is not a DECISIONS.md id such as D-UA"
        ));
    }
    for (n, unit) in raw.restore_by.iter().enumerate() {
        if !vocab::is_unit(unit) {
            problems.add(&format!(
                "restore_by {unit:?} is not a plan unit such as CONFIG-LOADER"
            ));
        }
        if raw.restore_by.iter().take(n).any(|earlier| earlier == unit) {
            problems.add(&format!("restore_by names {unit} twice"));
        }
    }
    raw.rule_b.as_ref().and_then(|name| {
        let case = RuleB::parse(name);
        if case.is_none() {
            problems.add(&format!(
                "rule_b {name:?} is none of a, b, c, d and prebid-logic"
            ));
        }
        case
    })
}

/// Checks the fields whose presence the status decides.
fn by_status(status: Status, raw: &RawEntry, problems: &mut Problems<'_>) {
    match status {
        Status::Ledgered => {
            if raw.rule_b.is_some() {
                problems.add("a ledgered entry names no Rule B case (rule_b)");
            }
            if raw.default.is_some() {
                problems.add("only a decision-pending entry has a default");
            }
            if !raw.restore_by.is_empty() {
                problems.add("only a restored entry names restore_by");
            }
        }
        Status::Restored => {
            if !raw.harness.is_empty() {
                problems.add(
                    "a restored entry has no harness pattern: a mismatch where Go's behaviour is \
                     kept is unexpected",
                );
            }
            if raw.restore.is_none() {
                problems.add("a restored entry says what keeps Go's behaviour (restore)");
            }
            if raw.rule_b.is_none() && raw.decision.is_none() {
                problems.add(
                    "a restored entry names its Rule B case, or prebid-logic (rule_b), or the \
                     decided decision that keeps Go's behaviour (decision)",
                );
            }
            if raw.default.is_some() {
                problems.add("only a decision-pending entry has a default");
            }
        }
        Status::Extension => {
            if !raw.harness.is_empty() {
                problems.add(
                    "an extension entry has no harness pattern: parity runs keep the extension \
                     off, and off is Go",
                );
            }
            if raw.restore.is_some() {
                problems.add(
                    "an extension entry has no restore: off is Go, which its pin shows, and rust \
                     says what happens with it on",
                );
            }
            if raw.rule_b.is_some() {
                problems.add("an extension entry names no Rule B case (rule_b)");
            }
            if raw.default.is_some() {
                problems.add("only a decision-pending entry has a default");
            }
            if !raw.restore_by.is_empty() {
                problems.add("only a restored entry names restore_by");
            }
        }
        Status::DecisionPending => {
            if raw.decision.is_none() {
                problems.add("a decision-pending entry names its decision");
            }
            if raw.default.is_none() {
                problems.add(
                    "a decision-pending entry says what applies until the user decides \
                     (default)",
                );
            }
            if raw.restore.is_none() {
                problems.add(
                    "a decision-pending entry gives the restore option the decision weighs \
                     (restore)",
                );
            }
            if raw.rule_b.is_some() {
                problems.add(
                    "a decision-pending entry names no Rule B case (rule_b): until the decision \
                     restores it, the default applies; say what a restore would rest on in \
                     restore",
                );
            }
            if !raw.restore_by.is_empty() {
                problems.add("only a restored entry names restore_by");
            }
        }
    }
}

/// Checks the who list.
fn check_who(raw: &[String], problems: &mut Problems<'_>) -> Vec<&'static str> {
    if raw.is_empty() {
        problems.add(&format!(
            "who is empty; say who could notice: {}",
            WHO.join(", ")
        ));
    }
    let mut who = Vec::new();
    for name in raw {
        match WHO.iter().copied().find(|known| *known == name.as_str()) {
            Some(known) if who.contains(&known) => {
                problems.add(&format!("who names {name} twice"));
            }
            Some(known) => who.push(known),
            None => problems.add(&format!("who {name:?} is none of {}", WHO.join(", "))),
        }
    }
    if who.contains(&"none") && who.len() > 1 {
        problems.add("who: none stands alone");
    }
    who
}

/// Validates one harness pattern.
fn pattern(raw: &RawPattern) -> Result<Pattern, String> {
    if let Some((name, why)) = REFUSED_KINDS.iter().find(|(name, _)| *name == raw.kind) {
        return Err(format!("no entry names kind {name}: {why}"));
    }
    let kind = vocab::kind(&raw.kind).ok_or_else(|| {
        format!(
            "unknown kind {:?}; the kinds are {}",
            raw.kind,
            KINDS
                .iter()
                .map(|kind| kind.name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let class = match (raw.lexical_class.as_deref(), kind.classes.is_empty()) {
        (None, true) => None,
        (Some(class), true) => {
            return Err(format!(
                "kind {} takes no lexical_class ({class:?})",
                kind.name
            ));
        }
        (None, false) => {
            return Err(format!(
                "kind {} names its lexical_class: {}",
                kind.name,
                kind.classes.join(", ")
            ));
        }
        (Some(class), false) => {
            if let Some((_, why)) = REFUSED_CLASSES.iter().find(|(name, _)| *name == class) {
                return Err(format!("no entry names class {class}: {why}"));
            }
            Some(
                *kind
                    .classes
                    .iter()
                    .find(|known| **known == class)
                    .ok_or_else(|| {
                        format!(
                            "kind {} has no class {class:?}; its classes are {}",
                            kind.name,
                            kind.classes.join(", ")
                        )
                    })?,
            )
        }
    };
    let path = PathPattern::parse(&raw.path).map_err(|e| format!("path {:?}: {e}", raw.path))?;
    if kind.concrete && !path.is_concrete() {
        return Err(format!(
            "a {} pattern names a concrete path, without .* or .**, not {}",
            kind.name, raw.path
        ));
    }
    let input = raw
        .input
        .as_deref()
        .map(|name| {
            vocab::input(name).ok_or_else(|| {
                format!(
                    "unknown input predicate {name:?}; the predicates are {}",
                    INPUTS
                        .iter()
                        .map(|input| input.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        })
        .transpose()?;
    if kind.needs_input && input.is_none() {
        return Err(format!(
            "a {} pattern names an input predicate, so a {} mismatch on any other case stays \
             unexpected",
            kind.name, kind.name
        ));
    }
    Ok(Pattern {
        kind,
        class,
        path,
        input,
    })
}

/// Validates one `[[restoration]]` table and attaches it to its entry.
fn restoration(
    raw: RawRestoration,
    fragment: &str,
    ids: &BTreeMap<String, String>,
    entries: &mut [Entry],
    sources: &mut Sources,
    list: &mut Vec<String>,
) {
    let mut problems = Problems {
        list,
        at: format!("{fragment}: restoration of {}", raw.id),
    };
    let pin = problems.pin("pin", &raw.pin, sources);
    problems.optional_prose("note", raw.note.as_ref());
    if !ids.contains_key(&raw.id) {
        problems.add("no entry has that id");
        return;
    }
    // An entry that failed validation is not in `entries`; its own problems are reported.
    let Some(entry) = entries.iter_mut().find(|entry| entry.id == raw.id) else {
        return;
    };
    if entry.status != Status::Restored {
        problems.add(&format!(
            "the entry is {}, not restored",
            entry.status.name()
        ));
    } else if entry.restorations.iter().any(|known| known.pin == pin) {
        problems.add(&format!("the entry already has restore pin {pin}"));
    } else {
        entry.restorations.push(Restoration {
            pin,
            note: raw.note,
            fragment: fragment.to_owned(),
        });
    }
}

/// Reports every pair of patterns one mismatch could match: a mismatch belongs to one entry.
fn overlaps(entries: &[Entry], problems: &mut Vec<String>) {
    let patterns: Vec<(&Entry, usize, &Pattern)> = entries
        .iter()
        .flat_map(|entry| {
            entry
                .harness
                .iter()
                .enumerate()
                .map(move |(n, pattern)| (entry, n + 1, pattern))
        })
        .collect();
    for (at, (entry, n, pattern)) in patterns.iter().enumerate() {
        for (other, m, other_pattern) in patterns.iter().skip(at + 1) {
            if pattern.overlaps(other_pattern) {
                problems.push(format!(
                    "{}: {}: harness pattern {n} ({}) overlaps pattern {m} of {} ({}): one \
                     mismatch would match both",
                    entry.fragment,
                    entry.id,
                    pattern.describe(),
                    other.id,
                    other_pattern.describe()
                ));
            }
        }
    }
}
