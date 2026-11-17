//! Go `regexp` (RE2 syntax with Perl flags, as `regexp.Compile` uses) on top of the `regex`
//! crate.
//!
//! A port of Go's `regexp/syntax` parser translates the pattern instead of handing it to Rust,
//! because the two engines differ in syntax and in their Unicode data:
//!
//! - every set of characters (`[...]`, `\d`, `[:alpha:]`, `\pL`, and a literal under `(?i)`) is
//!   computed here the way Go computes it, from the Unicode 15.0.0 tables of Go's `unicode` package
//!   (`go_unicode`) and Go's simple case folding, and is emitted as explicit ranges. Rust's `i`
//!   flag and Unicode tables are never used;
//! - Go accepts patterns that Rust rejects: repeated or digit-leading capture names, a repetition
//!   right after a flag group (`a*(?i)*`), repeated flags (`(?ii)`), surrogate code points (which
//!   never match), `\Q...\E`, octal escapes, a literal `{` that does not start a repetition, and
//!   `[`, `&&`, `--` or `~~` inside a class;
//! - Go rejects patterns that Rust accepts, so Go's checks run here: bad escapes, bad repetitions,
//!   and class names that Go's `unicode` package does not know.
//!
//! Only `MatchString` is ported, so groups do not capture, and greediness only has to stay valid.
//!
//! Known differences, all about pattern size: Go's `maxHeight` (1000) and `maxSize` checks are
//! not ported; Rust instead rejects patterns nested more than 250 levels deep (its default nest
//! limit, kept because its compiler recurses) and patterns whose compiled form exceeds 256 MiB.

use std::fmt::Write as _;

use regex::{Regex, RegexBuilder};

use super::go_unicode::{CASE_ORBIT, CASE_RANGES, CATEGORIES, CATEGORY_ALIASES, SCRIPTS, Table};

/// A compiled Go regular expression that remembers its source (Go: `(*Regexp).String`).
#[derive(Debug, Clone)]
pub struct GoRegexp {
    source: String,
    regex: Regex,
}

impl GoRegexp {
    /// Go: `regexp.Compile(expr)`.
    ///
    /// # Errors
    ///
    /// Returns a description of the problem when Go would reject the pattern (or the
    /// translated pattern does not compile).
    pub fn compile(expr: &str) -> Result<Self, String> {
        let translated = translate(expr)?;
        // Rust's default nest limit (250) stays: the `regex` compiler recurses, and a limit
        // near Go's `maxHeight` (1000) overflows a 2 MiB thread stack.
        let regex = RegexBuilder::new(&translated)
            .size_limit(1 << 28)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            source: expr.to_owned(),
            regex,
        })
    }

    /// Go: `(*Regexp).MatchString` (an unanchored search).
    #[must_use]
    pub fn match_string(&self, s: &str) -> bool {
        self.regex.is_match(s)
    }

    /// Go: `(*Regexp).String`: the source text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }
}

/// Go: `unicode.MaxRune`.
const MAX_RUNE: u32 = 0x0010_FFFF;

/// Go: regexp/syntax `minFold` and `maxFold`, the range of runes that case folding can change.
const MIN_FOLD: u32 = 0x0041;
const MAX_FOLD: u32 = 0x0001_E943;

/// More than the longest `unicode.SimpleFold` orbit; it only bounds the loops.
const MAX_ORBIT: usize = 8;

/// A Rust class that matches nothing.
const NOTHING: &str = r"[^\x{0}-\x{10FFFF}]";

/// Go: regexp/syntax rejects repeat counts above 1000.
const MAX_REPEAT: u32 = 1000;
/// Go: `perlGroup` (`\d`, `\s`, `\w`).
const DIGIT: &[(u32, u32)] = &[(0x30, 0x39)];
const SPACE: &[(u32, u32)] = &[(0x9, 0xA), (0xC, 0xD), (0x20, 0x20)];
const WORD: &[(u32, u32)] = &[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)];

/// Go: `perlGroup`: `(positive, ranges)`.
const fn perl_group(c: char) -> Option<(bool, &'static [(u32, u32)])> {
    match c {
        'd' => Some((true, DIGIT)),
        'D' => Some((false, DIGIT)),
        's' => Some((true, SPACE)),
        'S' => Some((false, SPACE)),
        'w' => Some((true, WORD)),
        'W' => Some((false, WORD)),
        _ => None,
    }
}

/// Go: `posixGroup`, without the `^` of the negated names.
fn posix_group(name: &str) -> Option<&'static [(u32, u32)]> {
    Some(match name {
        "alnum" => &[(0x30, 0x39), (0x41, 0x5A), (0x61, 0x7A)],
        "alpha" => &[(0x41, 0x5A), (0x61, 0x7A)],
        "ascii" => &[(0x0, 0x7F)],
        "blank" => &[(0x9, 0x9), (0x20, 0x20)],
        "cntrl" => &[(0x0, 0x1F), (0x7F, 0x7F)],
        "digit" => DIGIT,
        "graph" => &[(0x21, 0x7E)],
        "lower" => &[(0x61, 0x7A)],
        "print" => &[(0x20, 0x7E)],
        "punct" => &[(0x21, 0x2F), (0x3A, 0x40), (0x5B, 0x60), (0x7B, 0x7E)],
        "space" => &[(0x9, 0xD), (0x20, 0x20)],
        "upper" => &[(0x41, 0x5A)],
        "word" => WORD,
        "xdigit" => &[(0x30, 0x39), (0x41, 0x46), (0x61, 0x66)],
        _ => return None,
    })
}

/// Go: `unicode.UpperCase` and `unicode.LowerCase`, indexes into a `CaseRange` delta.
const UPPER_CASE: usize = 0;
const LOWER_CASE: usize = 1;

/// Go: `unicode.SimpleFold`.
fn simple_fold(r: u32) -> u32 {
    if r > MAX_RUNE {
        return r;
    }
    if let Ok(i) = CASE_ORBIT.binary_search_by_key(&r, |&(from, _)| from)
        && let Some(&(_, to)) = CASE_ORBIT.get(i)
    {
        return to;
    }
    let i = CASE_RANGES.partition_point(|&(_, hi, _)| hi < r);
    let Some(&(lo, _, deltas)) = CASE_RANGES.get(i).filter(|&&(lo, ..)| lo <= r) else {
        return r;
    };
    let lower = convert_case(LOWER_CASE, r, lo, deltas);
    if lower != r {
        return lower;
    }
    convert_case(UPPER_CASE, r, lo, deltas)
}

/// Go: `unicode.convertCase`.
fn convert_case(case: usize, r: u32, lo: u32, deltas: [i32; 3]) -> u32 {
    let delta = i64::from(deltas.get(case).copied().unwrap_or(0));
    if delta > i64::from(MAX_RUNE) {
        // Go: `UpperLower`: upper case at even offsets from `lo`, lower case at odd ones.
        return lo + (((r - lo) & !1) | u32::from(case & 1 == 1));
    }
    u32::try_from(i64::from(r) + delta).unwrap_or(r)
}

/// A set of runes being built (Go: the `[]rune` pairs of a character class).
#[derive(Debug, Clone, Default)]
struct Class {
    ranges: Vec<(u32, u32)>,
}

impl Class {
    fn of(ranges: &[(u32, u32)]) -> Self {
        Self {
            ranges: ranges.to_vec(),
        }
    }

    /// Go: `appendRange`.
    fn add(&mut self, lo: u32, hi: u32) {
        self.ranges.push((lo, hi));
    }

    /// Go: `appendFoldedRange`: the range and every rune that case-folds to one in it.
    fn add_folded(&mut self, lo: u32, hi: u32) {
        if (lo <= MIN_FOLD && hi >= MAX_FOLD) || hi < MIN_FOLD || lo > MAX_FOLD {
            self.add(lo, hi);
            return;
        }
        let first = lo.max(MIN_FOLD);
        let last = hi.min(MAX_FOLD);
        self.add(lo, hi);
        for c in first..=last {
            let mut f = simple_fold(c);
            for _ in 0..MAX_ORBIT {
                if f == c {
                    break;
                }
                self.add(f, f);
                f = simple_fold(f);
            }
        }
    }

    /// Go: `appendTable`.
    fn add_table(&mut self, table: Table) {
        for &[lo, hi, stride] in table.as_chunks::<3>().0 {
            match usize::try_from(stride) {
                Ok(step) if step > 1 => {
                    for c in (lo..=hi).step_by(step) {
                        self.add(c, c);
                    }
                }
                Ok(_) | Err(_) => self.add(lo, hi),
            }
        }
    }

    fn add_class(&mut self, other: &Self) {
        self.ranges.extend_from_slice(&other.ranges);
    }

    /// Go: `cleanClass`: sorted, merged ranges.
    fn clean(&mut self) {
        self.ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(self.ranges.len());
        for &(lo, hi) in &self.ranges {
            if let Some(last) = merged.last_mut()
                && lo <= last.1.saturating_add(1)
            {
                last.1 = last.1.max(hi);
                continue;
            }
            merged.push((lo, hi));
        }
        self.ranges = merged;
    }

    /// Go: `negateClass` (over `0..=MaxRune`).
    fn negate(&mut self) {
        self.clean();
        let mut out = Vec::with_capacity(self.ranges.len() + 1);
        let mut next = 0u32;
        for &(lo, hi) in &self.ranges {
            if lo > next {
                out.push((next, lo - 1));
            }
            next = hi.saturating_add(1);
        }
        if next <= MAX_RUNE {
            out.push((next, MAX_RUNE));
        }
        self.ranges = out;
    }

    /// Appends the class as a Rust class. Go matches decoded runes, which are never
    /// surrogates, so surrogates are left out (a class of only surrogates never matches).
    fn emit(mut self, out: &mut String) {
        self.clean();
        let mut items = String::new();
        for &(lo, hi) in &self.ranges {
            for (a, b) in [(lo, hi.min(0xD7FF)), (lo.max(0xE000), hi)] {
                if a <= b {
                    push_code(&mut items, a);
                    if b > a {
                        items.push('-');
                        push_code(&mut items, b);
                    }
                }
            }
        }
        if items.is_empty() {
            out.push_str(NOTHING);
        } else {
            let _ok = write!(out, "[{items}]");
        }
    }
}

/// Appends `r` as a `\x{..}` escape, valid both inside and outside Rust classes.
fn push_code(out: &mut String, r: u32) {
    let _ok = write!(out, r"\x{{{r:X}}}");
}

const fn is_surrogate(r: u32) -> bool {
    matches!(r, 0xD800..=0xDFFF)
}

/// Go: regexp/syntax `canonicalName`: `_`, `-` and spaces dropped, the first letter upper case
/// and the rest lower case.
fn canonical_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut first = true;
    for c in name.chars() {
        match c {
            '_' | '-' | ' ' => {}
            _ if first => {
                first = false;
                out.push(c.to_ascii_uppercase());
            }
            _ => out.push(c.to_ascii_lowercase()),
        }
    }
    out
}

/// A Unicode class as Go's `unicodeTable` returns it.
struct UnicodeTable {
    table: Class,
    /// `None` when Go has no fold table: the class is then not case folded.
    fold: Option<Class>,
    positive: bool,
}

impl UnicodeTable {
    fn named(table: Table, fold: Table, positive: bool) -> Self {
        let mut class = Class::default();
        class.add_table(table);
        let fold = (!fold.is_empty()).then(|| {
            let mut fold_class = Class::default();
            fold_class.add_table(fold);
            fold_class
        });
        Self {
            table: class,
            fold,
            positive,
        }
    }
}

fn lookup(list: &[(&str, Table, Table)], name: &str) -> Option<(Table, Table)> {
    list.iter()
        .find(|&&(key, ..)| key == name)
        .map(|&(_, table, fold)| (table, fold))
}

/// Go: regexp/syntax `unicodeTable`.
fn unicode_table(name: &str) -> Option<UnicodeTable> {
    let name = canonical_name(name);
    match name.as_str() {
        "Any" => {
            let any = Class::of(&[(0, MAX_RUNE)]);
            return Some(UnicodeTable {
                fold: Some(any.clone()),
                table: any,
                positive: true,
            });
        }
        "Assigned" => {
            let (cn, _) = lookup(CATEGORIES, "Cn")?;
            return Some(UnicodeTable::named(cn, cn, false));
        }
        "Ascii" => {
            return Some(UnicodeTable {
                table: Class::of(&[(0, 0x7F)]),
                fold: Some(Class::of(&[(0, 0x7F), (0x017F, 0x017F), (0x212A, 0x212A)])),
                positive: true,
            });
        }
        "Lc" => {
            let (table, fold) = lookup(CATEGORIES, "LC")?;
            return Some(UnicodeTable::named(table, fold, true));
        }
        _ => {}
    }
    if let Some((table, fold)) = lookup(CATEGORIES, &name).or_else(|| lookup(SCRIPTS, &name)) {
        return Some(UnicodeTable::named(table, fold, true));
    }
    let &(_, actual) = CATEGORY_ALIASES
        .iter()
        .find(|&&(alias, _)| canonical_name(alias) == name)?;
    let (table, fold) = lookup(CATEGORIES, actual)?;
    Some(UnicodeTable::named(table, fold, true))
}

/// Translates a Go pattern into equivalent Rust `regex` syntax.
fn translate(expr: &str) -> Result<String, String> {
    let mut t = Translator {
        chars: expr.chars().collect(),
        pos: 0,
        out: String::new(),
        flags: Flags::default(),
        emitted: 0,
        groups: Vec::new(),
        operand_start: 0,
        last_copies: 1,
        copies: 1,
    };
    t.run()?;
    Ok(t.out)
}

/// Go's parse flags that `(?flags)` changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Flags(u8);

impl Flags {
    /// `s` (Go: `DotNL`).
    const DOT_NL: u8 = 4;
    /// `i` (Go: `FoldCase`), applied by the translator itself.
    const FOLD: u8 = 1;
    /// `m` (Go: not `OneLine`).
    const MULTI_LINE: u8 = 2;
    /// `U` (Go: `NonGreedy`).
    const NON_GREEDY: u8 = 8;

    const fn fold(self) -> bool {
        self.0 & Self::FOLD != 0
    }

    /// The flags Rust applies (all but `i`).
    const fn rust(self) -> u8 {
        self.0 & !Self::FOLD
    }
}

/// A Rust flag group that sets exactly `flags` (from [`Flags::rust`]).
fn rust_flag_group(flags: u8) -> String {
    let mut on = String::new();
    let mut off = String::new();
    for (bit, c) in [
        (Flags::MULTI_LINE, 'm'),
        (Flags::DOT_NL, 's'),
        (Flags::NON_GREEDY, 'U'),
    ] {
        if flags & bit == 0 {
            off.push(c);
        } else {
            on.push(c);
        }
    }
    if off.is_empty() {
        format!("(?{on})")
    } else {
        format!("(?{on}-{off})")
    }
}

/// What the last item was, for Go's repetition rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Last {
    /// Start of the pattern, after `(` or `|`: a repetition has no argument.
    Nothing,
    /// An operand that can be repeated.
    Operand,
    /// A repetition operator: Go rejects stacking another one.
    Repeat,
    /// A repetition operator followed by a flag group or an empty `\Q\E`: Go resets its
    /// "last repetition" there, so another repetition applies to the repeated operand.
    RepeatThenFlags,
}

impl Last {
    /// Go: what `lastRepeat` and the operand stack look like after a bare flag group.
    const fn after_flags(self) -> Self {
        match self {
            Self::Repeat | Self::RepeatThenFlags => Self::RepeatThenFlags,
            Self::Nothing => Self::Nothing,
            Self::Operand => Self::Operand,
        }
    }
}

/// The upper bound of a counted repetition.
#[derive(Debug, Clone, Copy)]
enum RepeatMax {
    /// `{n}`.
    Exact,
    /// `{n,}`.
    Unbounded,
    /// `{n,m}`; `None` when `m` overflows Go's `parseInt` limit.
    Bounded(Option<u32>),
}

/// A parsed `{..}` repetition; `min` is `None` when it overflows Go's `parseInt` limit.
#[derive(Debug, Clone, Copy)]
struct RepeatSpec {
    min: Option<u32>,
    max: RepeatMax,
    len: usize,
}

/// An open group: Go's flags before it, where it starts in the output, and the
/// [`Translator::copies`] of the operands before it in the enclosing group.
#[derive(Debug, Clone, Copy)]
struct OpenGroup {
    flags: Flags,
    start: usize,
    copies: u64,
}

struct Translator {
    chars: Vec<char>,
    pos: usize,
    out: String,
    /// Go's flags at this point of the pattern.
    flags: Flags,
    /// The Rust flags in effect at the end of `out`. Flag changes are emitted lazily, right
    /// before the next operand, because Rust rejects a repetition right after a flag group.
    emitted: u8,
    groups: Vec<OpenGroup>,
    /// Where the last operand starts in `out`.
    operand_start: usize,
    /// Go's `repeatIsValid` measure of the last operand: the largest product of the counts of
    /// the counted repetitions nested along one path (1 without any; `x{0}` resets it).
    last_copies: u64,
    /// The largest `last_copies` of the earlier operands of the current group.
    copies: u64,
}

impl Translator {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    /// Emits the pending flag change.
    fn flush(&mut self) {
        let wanted = self.flags.rust();
        if wanted != self.emitted {
            self.out.push_str(&rust_flag_group(wanted));
            self.emitted = wanted;
        }
    }

    /// Starts an operand in `out`.
    fn operand(&mut self) {
        self.flush();
        self.operand_start = self.out.len();
        self.end_operand();
        self.last_copies = 1;
    }

    /// Folds the last operand into the group's [`Translator::copies`].
    fn end_operand(&mut self) {
        self.copies = self.copies.max(self.last_copies);
    }

    fn emit_class(&mut self, class: Class) {
        self.operand();
        class.emit(&mut self.out);
    }

    /// Go: `p.literal(r)`; under `(?i)` a literal matches its case-folding orbit.
    fn literal(&mut self, r: u32) {
        if self.flags.fold() && simple_fold(r) != r {
            let mut class = Class::default();
            class.add_folded(r, r);
            self.emit_class(class);
            return;
        }
        self.operand();
        if is_surrogate(r) {
            self.out.push_str(NOTHING);
        } else {
            push_code(&mut self.out, r);
        }
    }

    fn run(&mut self) -> Result<(), String> {
        let mut last = Last::Nothing;
        while let Some(c) = self.peek() {
            last = match c {
                '(' => {
                    if self.open_group()? {
                        Last::Nothing
                    } else {
                        last.after_flags()
                    }
                }
                '|' => {
                    self.pos += 1;
                    self.out.push('|');
                    self.end_operand();
                    self.last_copies = 1;
                    Last::Nothing
                }
                ')' => {
                    let group = self.groups.pop().ok_or("unexpected )")?;
                    self.pos += 1;
                    self.out.push(')');
                    self.flags = group.flags;
                    self.emitted = group.flags.rust();
                    self.operand_start = group.start;
                    self.end_operand();
                    self.last_copies = self.copies;
                    self.copies = group.copies;
                    Last::Operand
                }
                '^' | '$' | '.' => {
                    self.pos += 1;
                    self.operand();
                    self.out.push(c);
                    Last::Operand
                }
                '[' => {
                    let class = self.class()?;
                    self.emit_class(class);
                    Last::Operand
                }
                '*' | '+' | '?' => {
                    self.pos += 1;
                    self.repeat(last, &c.to_string(), None)?
                }
                '{' => self.brace(last)?,
                '\\' => self.escape(last)?,
                _ => {
                    self.pos += 1;
                    self.literal(u32::from(c));
                    Last::Operand
                }
            };
        }
        if !self.groups.is_empty() {
            return Err("missing closing )".to_owned());
        }
        Ok(())
    }

    /// Go: `p.repeat`: checks the operand, the nesting of counted repetitions (`count` is the
    /// `(min, max)` of `{min,max}`, `max` being `None` for `{min,}`) and the non-greedy suffix.
    fn repeat(
        &mut self,
        last: Last,
        op: &str,
        count: Option<(u32, Option<u32>)>,
    ) -> Result<Last, String> {
        match last {
            Last::Nothing => return Err(format!("missing argument to repetition operator: {op}")),
            Last::Repeat => return Err(format!("invalid nested repetition operator: {op}")),
            Last::RepeatThenFlags | Last::Operand => {}
        }
        if let Some((min, max)) = count {
            // Go: `repeatIsValid(re, 1000)` when min or max is at least 2: along any path, the
            // counts of nested counted repetitions multiply to at most 1000; `x{0}` stops the
            // walk and `x{0,}` counts as 1.
            self.last_copies = if max == Some(0) {
                1
            } else {
                let m = u64::from(max.unwrap_or(min).max(1));
                m.saturating_mul(self.last_copies).max(m)
            };
            if (min >= 2 || max.is_some_and(|m| m >= 2)) && self.last_copies > u64::from(MAX_REPEAT)
            {
                return Err(format!("invalid repeat count: {op}"));
            }
        }
        if last == Last::RepeatThenFlags {
            // Go repeats the repetition (`a*(?i)*` is `(a*)*`); Rust needs the group.
            self.out.insert_str(self.operand_start, "(?:");
            self.out.push(')');
        }
        self.out.push_str(op);
        if self.peek() == Some('?') {
            self.pos += 1;
            self.out.push('?');
        }
        Ok(Last::Repeat)
    }

    /// Go: `case '{'`: a repetition when `parseRepeat` accepts it, a literal `{` otherwise.
    fn brace(&mut self, last: Last) -> Result<Last, String> {
        let Some(spec) = self.parse_repeat() else {
            self.pos += 1;
            self.literal(u32::from('{'));
            return Ok(Last::Operand);
        };
        let invalid = || "invalid repeat count".to_owned();
        let min = spec.min.filter(|&m| m <= MAX_REPEAT).ok_or_else(invalid)?;
        let (op, max) = match spec.max {
            RepeatMax::Exact => (format!("{{{min}}}"), Some(min)),
            RepeatMax::Unbounded => (format!("{{{min},}}"), None),
            RepeatMax::Bounded(max) => {
                let max = max
                    .filter(|&m| m <= MAX_REPEAT && m >= min)
                    .ok_or_else(invalid)?;
                (format!("{{{min},{max}}}"), Some(max))
            }
        };
        self.pos += spec.len;
        self.repeat(last, &op, Some((min, max)))
    }

    /// Go: `parseRepeat`.
    fn parse_repeat(&self) -> Option<RepeatSpec> {
        let mut i = self.pos + 1;
        let (min, next) = self.parse_int(i)?;
        i = next;
        let max = match self.chars.get(i) {
            Some(',') => {
                i += 1;
                if self.chars.get(i) == Some(&'}') {
                    RepeatMax::Unbounded
                } else {
                    let (hi, after) = self.parse_int(i)?;
                    i = after;
                    RepeatMax::Bounded(hi)
                }
            }
            Some(_) => RepeatMax::Exact,
            None => return None,
        };
        if self.chars.get(i) != Some(&'}') {
            return None;
        }
        Some(RepeatSpec {
            min,
            max,
            len: i + 1 - self.pos,
        })
    }

    /// Go: `parseInt`: digits without a leading zero; values of 1e8 and more become `None`.
    fn parse_int(&self, start: usize) -> Option<(Option<u32>, usize)> {
        let first = self.chars.get(start)?;
        if !first.is_ascii_digit() {
            return None;
        }
        if *first == '0' && self.chars.get(start + 1).is_some_and(char::is_ascii_digit) {
            return None;
        }
        let mut end = start;
        let mut value: Option<u32> = Some(0);
        while let Some(d) = self.chars.get(end).and_then(|c| c.to_digit(10)) {
            value = value.and_then(|v| {
                if v >= 100_000_000 {
                    None
                } else {
                    Some(v * 10 + d)
                }
            });
            end += 1;
        }
        Some((value, end))
    }

    /// Opens a group in `out` (Go: `p.op(opLeftParen)`, which remembers the flags).
    fn push_group(&mut self) {
        self.flush();
        self.end_operand();
        self.groups.push(OpenGroup {
            flags: self.flags,
            start: self.out.len(),
            copies: self.copies,
        });
        self.out.push_str("(?:");
        self.copies = 1;
        self.last_copies = 1;
    }

    /// Go: `parsePerlFlags` and plain capture groups. Returns whether a group was opened
    /// (`false` for a bare flag change such as `(?i)`).
    fn open_group(&mut self) -> Result<bool, String> {
        if self.peek_at(1) != Some('?') {
            self.pos += 1;
            self.push_group();
            return Ok(true);
        }
        let named_p = self.peek_at(2) == Some('P') && self.peek_at(3) == Some('<');
        let named = self.peek_at(2) == Some('<');
        if named_p || named {
            let start = self.pos + if named_p { 4 } else { 3 };
            let rest = self.chars.get(start..).unwrap_or_default();
            let end = rest
                .iter()
                .position(|&c| c == '>')
                .ok_or("invalid named capture")?;
            let name: String = rest.get(..end).unwrap_or_default().iter().collect();
            // Go: `isValidCaptureName`. Go allows repeated names and leading digits, which
            // Rust does not, so the group is emitted without its name.
            if name.is_empty() || !name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric()) {
                return Err(format!("invalid named capture: {name}"));
            }
            self.pos = start + end + 1;
            self.push_group();
            return Ok(true);
        }
        self.flags()
    }

    /// Go: the flag loop of `parsePerlFlags`: `(?flags)` or `(?flags:`.
    fn flags(&mut self) -> Result<bool, String> {
        let mut i = self.pos + 2;
        let mut flags = self.flags.0;
        let mut negated = false;
        let mut saw_flag = false;
        while let Some(&c) = self.chars.get(i) {
            i += 1;
            let bit = match c {
                'i' => Flags::FOLD,
                'm' => Flags::MULTI_LINE,
                's' => Flags::DOT_NL,
                'U' => Flags::NON_GREEDY,
                '-' if !negated => {
                    negated = true;
                    saw_flag = false;
                    continue;
                }
                ':' | ')' if !negated || saw_flag => {
                    self.pos = i;
                    let group = c == ':';
                    if group {
                        self.push_group();
                    }
                    self.flags = Flags(flags);
                    return Ok(group);
                }
                _ => break,
            };
            saw_flag = true;
            if negated {
                flags &= !bit;
            } else {
                flags |= bit;
            }
        }
        Err("invalid or unsupported Perl syntax".to_owned())
    }

    /// Escapes outside a class.
    fn escape(&mut self, last: Last) -> Result<Last, String> {
        let Some(c) = self.peek_at(1) else {
            return Err("trailing backslash at end of expression".to_owned());
        };
        match c {
            'A' | 'z' => {
                self.pos += 2;
                self.operand();
                self.out.push('\\');
                self.out.push(c);
            }
            'b' | 'B' => {
                self.pos += 2;
                self.operand();
                let _ok = write!(self.out, r"(?-u:\{c})");
            }
            'C' => return Err(r"invalid escape sequence: \C".to_owned()),
            'Q' => {
                self.pos += 2;
                let mut any = false;
                while let Some(ch) = self.peek() {
                    if ch == '\\' && self.peek_at(1) == Some('E') {
                        self.pos += 2;
                        break;
                    }
                    self.pos += 1;
                    self.literal(u32::from(ch));
                    any = true;
                }
                if !any {
                    return Ok(last.after_flags());
                }
            }
            'p' | 'P' => {
                let class = self.unicode_class()?;
                self.emit_class(class);
            }
            _ => {
                if let Some((positive, ranges)) = perl_group(c) {
                    self.pos += 2;
                    let class = self.group_class(positive, ranges);
                    self.emit_class(class);
                } else {
                    let r = self.parse_escape()?;
                    self.literal(r);
                }
            }
        }
        Ok(Last::Operand)
    }

    /// Go: `appendGroup`: a Perl or POSIX class, case folded under `(?i)`.
    fn group_class(&self, positive: bool, ranges: &[(u32, u32)]) -> Class {
        let mut class = Class::default();
        for &(lo, hi) in ranges {
            if self.flags.fold() {
                class.add_folded(lo, hi);
            } else {
                class.add(lo, hi);
            }
        }
        if !positive {
            class.negate();
        }
        class
    }

    /// Go: `parseUnicodeClass` for `\pX`, `\p{Name}`, `\p{^Name}` and the `\P` forms.
    fn unicode_class(&mut self) -> Result<Class, String> {
        let mut positive = self.peek_at(1) == Some('p');
        let mut name: String = match self.peek_at(2) {
            Some('{') => {
                let rest = self.chars.get(self.pos + 3..).unwrap_or_default();
                let end = rest
                    .iter()
                    .position(|&c| c == '}')
                    .ok_or("invalid character class range")?;
                let braced = rest.get(..end).unwrap_or_default().iter().collect();
                self.pos += 3 + end + 1;
                braced
            }
            Some(first) => {
                self.pos += 3;
                first.to_string()
            }
            None => String::new(),
        };
        if let Some(rest) = name.strip_prefix('^') {
            positive = !positive;
            name = rest.to_owned();
        }
        let table =
            unicode_table(&name).ok_or_else(|| format!("invalid character class range: {name}"))?;
        if !table.positive {
            positive = !positive;
        }
        let mut class = table.table;
        if self.flags.fold()
            && let Some(fold) = &table.fold
        {
            class.add_class(fold);
        }
        if !positive {
            class.negate();
        }
        Ok(class)
    }

    /// Go: `parseEscape` for single-character escapes.
    fn parse_escape(&mut self) -> Result<u32, String> {
        let Some(c) = self.peek_at(1) else {
            return Err("trailing backslash at end of expression".to_owned());
        };
        self.pos += 2;
        let invalid = || format!(r"invalid escape sequence: \{c}");
        match c {
            '1'..='7' => {
                if !self.peek().is_some_and(|d| ('0'..='7').contains(&d)) {
                    return Err(invalid());
                }
                Ok(self.octal(c))
            }
            '0' => Ok(self.octal(c)),
            'x' => self.hex_escape().ok_or_else(invalid),
            'a' => Ok(0x7),
            'f' => Ok(0xC),
            'n' => Ok(0xA),
            'r' => Ok(0xD),
            't' => Ok(0x9),
            'v' => Ok(0xB),
            _ if c.is_ascii() && !c.is_ascii_alphanumeric() => Ok(u32::from(c)),
            _ => Err(invalid()),
        }
    }

    /// Up to two more octal digits after `first`.
    fn octal(&mut self, first: char) -> u32 {
        let mut r = first.to_digit(8).unwrap_or(0);
        for _ in 1..3 {
            match self.peek().and_then(|d| d.to_digit(8)) {
                Some(d) => {
                    r = r * 8 + d;
                    self.pos += 1;
                }
                None => break,
            }
        }
        r
    }

    /// `\xFF` or `\x{10FFFF}` (the `\x` is already consumed). Surrogates are kept: Go parses
    /// them, and they never match.
    fn hex_escape(&mut self) -> Option<u32> {
        let c = self.peek()?;
        self.pos += 1;
        if c == '{' {
            let mut r: u32 = 0;
            let mut count = 0;
            loop {
                let d = self.peek()?;
                self.pos += 1;
                if d == '}' {
                    break;
                }
                r = r * 16 + d.to_digit(16)?;
                if r > MAX_RUNE {
                    return None;
                }
                count += 1;
            }
            if count == 0 {
                return None;
            }
            return Some(r);
        }
        let hi = c.to_digit(16)?;
        let lo = self.peek()?.to_digit(16)?;
        self.pos += 1;
        Some(hi * 16 + lo)
    }

    /// Go: `parseClass`.
    fn class(&mut self) -> Result<Class, String> {
        self.pos += 1;
        let negated = self.peek() == Some('^');
        if negated {
            self.pos += 1;
        }
        let mut class = Class::default();
        let mut first = true;
        while self.peek() != Some(']') || first {
            first = false;
            self.class_item(&mut class)?;
        }
        self.pos += 1; // ]
        if negated {
            class.negate();
        }
        Ok(class)
    }

    /// One item of a class: a POSIX class, a Unicode class, a Perl class, a character or a range.
    fn class_item(&mut self, class: &mut Class) -> Result<(), String> {
        if let Some(posix) = self.posix_class()? {
            class.add_class(&posix);
            return Ok(());
        }
        if self.peek() == Some('\\') {
            if matches!(self.peek_at(1), Some('p' | 'P')) {
                class.add_class(&self.unicode_class()?);
                return Ok(());
            }
            if let Some((positive, ranges)) = self.peek_at(1).and_then(perl_group) {
                self.pos += 2;
                class.add_class(&self.group_class(positive, ranges));
                return Ok(());
            }
        }
        let lo = self.class_char()?;
        let mut hi = lo;
        // Go: [a-] means (a|-), so a range needs something other than ] after the dash.
        if self.peek() == Some('-') && self.peek_at(1).is_some_and(|c| c != ']') {
            self.pos += 1;
            hi = self.class_char()?;
            if hi < lo {
                return Err("invalid character class range".to_owned());
            }
        }
        if self.flags.fold() {
            class.add_folded(lo, hi);
        } else {
            class.add(lo, hi);
        }
        Ok(())
    }

    /// Go: `parseNamedClass` for `[:alpha:]` and `[:^alpha:]`.
    fn posix_class(&mut self) -> Result<Option<Class>, String> {
        if self.peek() != Some('[') || self.peek_at(1) != Some(':') {
            return Ok(None);
        }
        let rest = self.chars.get(self.pos + 2..).unwrap_or_default();
        let Some(end) = rest.windows(2).position(|w| w == [':', ']']) else {
            return Ok(None);
        };
        let name: String = rest.get(..end).unwrap_or_default().iter().collect();
        let (positive, bare) = name
            .strip_prefix('^')
            .map_or((true, name.as_str()), |bare| (false, bare));
        let ranges = posix_group(bare)
            .ok_or_else(|| format!("invalid character class range: [:{name}:]"))?;
        self.pos += 2 + end + 2;
        Ok(Some(self.group_class(positive, ranges)))
    }

    /// Go: `parseClassChar`.
    fn class_char(&mut self) -> Result<u32, String> {
        match self.peek() {
            None => Err("missing closing ]".to_owned()),
            Some('\\') => self.parse_escape(),
            Some(c) => {
                self.pos += 1;
                Ok(u32::from(c))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{super::go_unicode, GoRegexp, MAX_RUNE, simple_fold};

    fn matches(pattern: &str, input: &str) -> Result<bool, String> {
        Ok(GoRegexp::compile(pattern)?.match_string(input))
    }

    fn check(cases: &[(&str, &str, bool)]) -> Result<(), String> {
        for &(pattern, input, want) in cases {
            if matches(pattern, input)? != want {
                return Err(format!("{pattern} on {input:?}: want {want}"));
            }
        }
        Ok(())
    }

    #[test]
    fn translation_keeps_go_semantics() -> Result<(), String> {
        check(&[
            (r"^\d+$", "123", true),
            (r"^\d+$", "\u{0663}", false),
            (r"^\w+$", "\u{e9}", false),
            (r"^\s$", "\u{a0}", false),
            (r"^\S$", "\u{a0}", true),
            (r"\bfoo\b", "\u{e9}foo", true),
            (
                r"^[a-fA-F\d\-]{32,36}$",
                "0123456789abcdef0123456789abcdef",
                true,
            ),
            (
                r"^[a-fA-F\d\-]{32,36}$",
                "0123456789abcdef0123456789abcde",
                false,
            ),
            (r"(\S+-)?\d+-\d+", "x-1-2", true),
            ("[0-9|a-z|A-Z]{16}", "abcdefgh|1234567", true),
            ("a{", "a{", true),
            ("a{,2}", "a{,2}", true),
            (r"\Q.*\E", ".*", true),
            (r"\Q.*\E", "ab", false),
            ("[[a]", "[", true),
            ("[a&&b]", "&", true),
            ("[]a]", "]", true),
            ("[^]a]", "b", true),
            (r"\x41\x{42}\101", "ABA", true),
            ("(?i)k", "\u{212a}", true),
            ("[[:alpha:]]+", "abc", true),
            (r"\pL", "\u{e9}", true),
        ])
    }

    /// Patterns Go accepts and a direct translation to Rust rejected (found by the
    /// `go_adversarial` differential tests; expected values are Go 1.26.2's).
    #[test]
    fn go_only_syntax_compiles_like_go() -> Result<(), String> {
        check(&[
            // Repeated and digit-leading capture names.
            ("(?P<n>a)(?P<n>b)", "ab", true),
            ("(?P<1a>a)", "a", true),
            ("(?<n>a)|(?<n>b)", "b", true),
            // A repetition after a flag group repeats the operand before it.
            ("a*(?i)*", "aaa", true),
            ("a(?i)*", "aaa", true),
            ("a(?i){2}", "aa", true),
            ("a(?i){2}", "a", false),
            (r"(0)\pS(?i)*", "0+", true),
            (r"a*\Q\E*", "b", true),
            // Repeated and cancelled flags.
            ("(?ii)a", "A", true),
            ("(?i-i)a", "A", false),
            ("(?m-m)^b", "a\nb", false),
            ("(?ss).", "\n", true),
            // Flags carry across alternation until the group ends.
            ("x(?m)y|^z$", "a\nz", true),
            ("(?:(?m)y)|^z$", "a\nz", false),
            ("^*a", "a", true),
        ])
    }

    #[test]
    fn surrogates_never_match() -> Result<(), String> {
        check(&[
            (r"\x{D800}", "\u{FFFD}", false),
            (r"a|\x{DFFF}", "a", true),
            (r"[\x{D7FF}-\x{E000}]", "\u{D7FF}", true),
            (r"[\x{D7FF}-\x{E000}]", "\u{E000}", true),
            (r"[\x{D7FF}-\x{E000}]", "\u{FFFD}", false),
            (r"[\x{DFFF}-\x{E001}]", "\u{E001}", true),
            (r"[^\x{D800}]", "a", true),
            (r"[\x{D800}-\x{DFFF}]", "\u{FFFD}", false),
            (r"(?i)\x{D800}", "\u{FFFD}x", false),
        ])
    }

    #[test]
    fn unicode_classes_use_go_names_and_tables() -> Result<(), String> {
        check(&[
            // Go's canonicalName.
            (r"\p{ Lu }", "A", true),
            (r"\p{l-u}", "A", true),
            (r"\p{uppercase letter}", "A", true),
            (r"\p{Uppercase_Letter}", "a", false),
            (r"\pl", "a", true),
            (r"\p{^l}", "1", true),
            (r"\p{Lc}", "a", true),
            (r"\p{GREEK}", "\u{3b1}", true),
            (r"\p{any}", "\u{10FFFF}", true),
            (r"\p{ascii}", "\u{80}", false),
            (r"\P{Assigned}", "\u{378}", true),
            // Script, not Script_Extensions: U+0342 is Inherited.
            (r"\p{Greek}", "\u{342}", false),
            (r"\p{Inherited}", "\u{342}", true),
            // C includes Cn; Cs never matches.
            (r"\pC", "\u{378}", true),
            (r"\p{Cs}", "a", false),
            (r"\P{Cs}", "a", true),
            (r"[^\p{Cs}]", "a", true),
            // Unicode 15.0.0: U+1C89 was assigned (Lu) in Unicode 16.
            (r"\p{Lu}", "\u{1C89}", false),
            (r"\p{Cn}", "\u{1C89}", true),
            (r"\p{Cyrillic}", "\u{1C89}", false),
            // Go's case folding: U+0264 gained an upper case (U+A7CB) only in Unicode 16.
            (r"(?i)\x{264}", "\u{A7CB}", false),
            (r"(?i)[\x{264}]", "\u{A7CB}", false),
            // Go folds L (adding U+0345) but has no fold table for LC.
            (r"(?i)\pL", "\u{345}", true),
            (r"(?i)\p{Lc}", "\u{345}", false),
            (r"(?i)\P{Lc}", "\u{345}", true),
            (r"(?i)\p{Lu}", "a", true),
            (r"(?i)\P{Lu}", "a", false),
            (r"(?i)[^\p{Lu}]", "a", false),
            (r"(?i)\W", "\u{212A}", false),
            ("(?i)[[:upper:]]", "\u{17F}", true),
        ])
    }

    #[test]
    fn go_rejects() -> Result<(), String> {
        for bad in [
            "(",
            ")",
            "[a",
            "a**",
            "a{2}{3}",
            "*a",
            "(?i)*",
            "a|*",
            r"\C",
            r"\1",
            r"\8",
            "a{1001}",
            r"\y",
            "(?x)a",
            "(?-)a",
            "(?P<>a)",
            "(?P<a-b>a)",
            "(?P=a)",
            "\\",
            r"\p{Old_Italic}",
            r"\p{OldItalic}",
            r"\p{Garay}",
            r"\p{Emoji}",
            r"\p{Alphabetic}",
            r"\p{White_Space}",
            r"\p{L&}",
            r"\p{IsGreek}",
            r"\p{}",
            r"\p",
            "[[:foo:]]",
        ] {
            if GoRegexp::compile(bad).is_ok() {
                return Err(format!("{bad} should be rejected"));
            }
        }
        Ok(())
    }

    /// Go's `repeatIsValid`: nested counted repetitions may make at most 1000 copies (Go
    /// 1.26.2 outcomes; Rust compiles all of these).
    #[test]
    fn nested_repeats_are_limited_like_go() -> Result<(), String> {
        let cases = [
            ("(a{1000}){1000}", false),
            ("((a{100}){100}){100}", false),
            ("(x{2,1000}){1000}", false),
            ("((a{1000}){1}){2}", false),
            ("((a{1000}){0,}){2}", false),
            ("((a{501}){0,1}){2}", false),
            ("(a{10}|b{201}){5}", false),
            ("a{10}(?i){101}", false),
            ("((a{1000})*){2}", false),
            ("(a{2}b{501}){2}", false),
            ("(a{1000}){2,}", false),
            ("(?:a{3}){334}", false),
            ("(a{1000}){1}", true),
            ("((a{1000}){0}){2}", true),
            ("((a{500}){0,1}){2}", true),
            ("(a{10}|b{200}){5}", true),
            ("a{10}(?i){100}", true),
            ("(a{2}b{500}){2}", true),
            ("((a){2}){2,}", true),
            ("(a{1000}){1,}", true),
            ("(?:a{3}){333}", true),
            ("(?:(?:a{10}){10}){10}", true),
            ("((a{1000})+){1}", true),
            ("(a{1000}b){1}c{2}", true),
        ];
        for (pattern, valid) in cases {
            if GoRegexp::compile(pattern).is_ok() != valid {
                return Err(format!("{pattern}: Go valid={valid}"));
            }
        }
        Ok(())
    }

    #[test]
    fn simple_fold_matches_go() -> Result<(), String> {
        let mut hash: u64 = 14_695_981_039_346_656_037;
        for r in 0..=MAX_RUNE {
            hash ^= u64::from(simple_fold(r));
            hash = hash.wrapping_mul(1_099_511_628_211);
        }
        if hash == go_unicode::SIMPLE_FOLD_HASH && go_unicode::VERSION == "15.0.0" {
            Ok(())
        } else {
            Err(format!("SimpleFold hash {hash}"))
        }
    }
}
