//! Go: `github.com/rs/cors/internal.SortedSet`.

use std::collections::BTreeMap;

/// Go: `SortedSet`: names with their rank in sorted order.
#[derive(Debug, Clone, Default)]
pub(super) struct SortedSet {
    m: BTreeMap<String, usize>,
    max_len: usize,
}

impl SortedSet {
    /// Go: `NewSortedSet(elems...)`.
    pub(super) fn new(mut elems: Vec<String>) -> Self {
        elems.sort_unstable();
        let mut m = BTreeMap::new();
        let mut max_len = 0;
        for s in elems {
            if m.contains_key(&s) {
                continue;
            }
            max_len = max_len.max(s.len());
            let rank = m.len();
            let _previous = m.insert(s, rank);
        }
        Self { m, max_len }
    }

    /// Go: `set.Size()`.
    #[cfg(test)]
    pub(super) fn size(&self) -> usize {
        self.m.len()
    }

    /// Go: `set.String()`: the names in order, comma-separated.
    #[cfg(test)]
    pub(super) fn to_csv(&self) -> String {
        let mut elems: Vec<(&usize, &String)> = self.m.iter().map(|(k, v)| (v, k)).collect();
        elems.sort_unstable();
        elems
            .into_iter()
            .map(|(_, k)| k.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Go: `set.Subsumes(csv)`: whether `csv`, a comma-separated list of names without spaces in
    /// sorted order, contains only names of the set.
    pub(super) fn subsumes(&self, csv: &str) -> bool {
        if csv.is_empty() {
            return true;
        }
        let mut csv = csv;
        let mut last_seen: Option<usize> = None;
        let chunk_size = self.max_len + 1;
        loop {
            let end = csv.len().min(chunk_size);
            let comma = csv
                .as_bytes()
                .get(..end)
                .and_then(|chunk| chunk.iter().position(|&b| b == b','));
            let name = comma.and_then(|i| csv.get(..i)).unwrap_or(csv);
            let Some(&pos) = self.m.get(name) else {
                return false;
            };
            if last_seen.is_some_and(|last| pos <= last) {
                return false;
            }
            last_seen = Some(pos);
            match comma {
                None => return true,
                Some(i) => csv = csv.get(i + 1..).unwrap_or_default(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SortedSet;

    fn set(elems: &[&str]) -> SortedSet {
        SortedSet::new(elems.iter().map(|&s| s.to_owned()).collect())
    }

    #[test]
    fn sorted_set_like_go() -> Result<(), String> {
        // Go: internal/sortedset_test.go.
        let cases: [(&[&str], usize, &str); 5] = [
            (&[], 0, ""),
            (&["foo"], 1, "foo"),
            (&["foo", "foo"], 1, "foo"),
            (&["foo", "bar", "baz"], 3, "bar,baz,foo"),
            (&["bar", "baz", "foo", "foo", "baz"], 3, "bar,baz,foo"),
        ];
        for (elems, size, csv) in cases {
            let s = set(elems);
            if s.size() != size || s.to_csv() != csv {
                return Err(format!("{elems:?}: size {} csv {:?}", s.size(), s.to_csv()));
            }
        }
        let s = set(&["accept", "content-type", "x-requested-with", "origin"]);
        for (csv, want) in [
            ("", true),
            ("accept", true),
            ("accept,content-type", true),
            ("content-type,accept", false),
            ("accept,accept", false),
            ("accept, content-type", false),
            ("Accept", false),
            ("accept,", false),
            (",accept", false),
            ("accept,origin,x-requested-with", true),
            ("x-requested-with-and-much-longer", false),
            ("bogus", false),
        ] {
            if s.subsumes(csv) != want {
                return Err(format!("Subsumes({csv:?}) != {want}"));
            }
        }
        Ok(())
    }
}
