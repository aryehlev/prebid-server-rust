//! Glue (Rule B (c)): adapter label values, and the adapter names in the go-metrics engine's
//! metric names, are bidder names lowercased with Go's `strings.ToLower`, which maps each character
//! by its simple lower-case mapping, while std's `str::to_lowercase` applies the full mapping
//! (U+0130 `İ` becomes `i` and U+0307) and the final sigma rule (a word-final `Σ` becomes `ς`).
//! Metric labels are the operator interface, so the values are Go's (ledger: `metrics.label-case`).
//! The mappings come from std's tables, which are a later Unicode version than go1.26.2's 15.0.0
//! (ledger: `metrics.label-unicode`).

/// Go: `strings.ToLower`: each character's simple lower-case mapping. std's mapping of a single
/// character is one character, except for U+0130, whose simple mapping is the first of its two.
pub(super) fn to_lower(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

#[cfg(test)]
#[path = "lower_test.rs"]
mod tests;
