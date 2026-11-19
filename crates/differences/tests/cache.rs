//! Pins of `moka` against `coocood/freecache` as the stored-request and floors caches use it:
//! which entries a 1 MiB cache keeps (`testdata/golden/cache.json`).

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use moka::sync::Cache;
    use pbs_differences::{TestResult, check_table, golden, list, number, text};

    /// freecache refuses an entry whose key and value together are longer than 1/1024 of the
    /// cache less its 24-byte entry header (`ErrLargeEntry`, whose text says only 1/1024): a
    /// 1 MiB cache keeps a 993-byte document under a 7-byte key, 1000 bytes in all, and refuses
    /// a 994-byte one, so such a stored request is fetched again every time. `moka`, bounded by
    /// the same byte budget through a weigher, keeps every entry that fits the budget.
    // Go: stored_requests/caches/memory/cache.go:29 (freecache.NewCache for a size-bounded cache)
    // and stored_requests/caches/memory/maps.go:60-64 (Set, whose error is logged and dropped)
    #[test]
    fn go_difference_large_entries_admitted() -> TestResult {
        let doc = golden("cache.json")?;
        let cache: Cache<String, Arc<[u8]>> = Cache::builder()
            .max_capacity(number(&doc, "/cache_size")?)
            .weigher(|key: &String, value: &Arc<[u8]>| {
                u32::try_from(key.len().saturating_add(value.len())).unwrap_or(u32::MAX)
            })
            .build();
        let entries = list(&doc, "/entries")?;
        for entry in entries {
            let size = usize::try_from(number(entry, "/value_len")?)?;
            cache.insert(text(entry, "/key")?.to_owned(), Arc::from(vec![b'x'; size]));
        }
        cache.run_pending_tasks();
        let mut rows = Vec::new();
        for entry in entries {
            let key = text(entry, "/key")?;
            let kept = |err: &str| if err.is_empty() { "kept" } else { "refused" };
            rows.push(vec![
                key.to_owned(),
                kept(text(entry, "/set_err")?).to_owned(),
                text(entry, "/set_err")?.to_owned(),
                cache.get(key).map_or("refused", |_| "kept").to_owned(),
            ]);
        }
        check_table(
            &rows,
            &[
                &["doc-512", "kept", "", "kept"],
                &["doc-993", "kept", "", "kept"],
                &[
                    "doc-994",
                    "refused",
                    "The entry size is larger than 1/1024 of cache size",
                    "kept",
                ],
                &[
                    "doc-1000",
                    "refused",
                    "The entry size is larger than 1/1024 of cache size",
                    "kept",
                ],
                &[
                    "doc-1024",
                    "refused",
                    "The entry size is larger than 1/1024 of cache size",
                    "kept",
                ],
                &[
                    "doc-65536",
                    "refused",
                    "The entry size is larger than 1/1024 of cache size",
                    "kept",
                ],
            ],
            "key, freecache, freecache's error, moka",
        )
    }
}
