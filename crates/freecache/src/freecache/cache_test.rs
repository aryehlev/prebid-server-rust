//! Port of freecache `cache_test.go`.
//!
//! The Go tests sleep on the real clock; these advance a fake clock instead, which makes the
//! timing assertions exact. The tests whose counters Go only logs (`TestFreeCache`,
//! `TestEvacuateCount`, `TestIterator`) are also compared with `scenarios.golden`, recorded from
//! Go on the same fake clock.

use std::{
    error::Error,
    sync::{Arc, Barrier},
    thread,
};

use super::{
    Cache, CacheError, DefaultTimer, ENTRY_HDR_SIZE, Timer,
    golden::{
        FakeCache, FakeTimer, Fnv, SplitMix64, TestResult, bytes_digest, check, check_eq, err_str,
        field, golden_lines, nil_val_str, read_golden,
    },
    xxhash,
};

/// Go: `scenarioClockStart`.
const CLOCK_START: u32 = 1_700_000_000;

/// A cache of `size` bytes on a fake clock at [`CLOCK_START`].
fn fake_cache(size: i64) -> (FakeCache, Arc<FakeTimer>) {
    let clock = FakeTimer::at(CLOCK_START);
    (Cache::with_timer(size, Arc::clone(&clock)), clock)
}

/// The `scenario` line `name` of `scenarios.golden`.
fn scenario(name: &str) -> Result<String, Box<dyn Error>> {
    let golden = read_golden("scenarios.golden")?;
    let fields = golden_lines(&golden, "scenario")
        .into_iter()
        .find(|fields| fields.first() == Some(&name))
        .ok_or_else(|| format!("scenario {name} is not in scenarios.golden"))?;
    Ok(field(&fields, 1)?.to_owned())
}

/// Go: `scenarioStats`.
fn scenario_stats<T: Timer>(cache: &Cache<T>) -> String {
    format!(
        "hr:{:016x} ev:{} e:{} aat:{} ex:{}",
        cache.hit_rate().to_bits(),
        cache.evacuate_count(),
        cache.entry_count(),
        cache.average_access_time(),
        cache.expired_count()
    )
}

/// The value `TestFreeCache` stores under `key`: `strings.Repeat(key, 10)`.
fn repeat10(key: &str) -> String {
    key.repeat(10)
}

/// Joins a worker thread, failing on a panic or an error.
fn join(worker: thread::ScopedJoinHandle<'_, Result<(), CacheError>>) -> TestResult {
    Ok(worker
        .join()
        .map_err(|panic| format!("worker panicked: {panic:?}"))??)
}

/// Go: `TestFreeCache`.
#[test]
fn free_cache() -> TestResult {
    let (cache, clock) = fake_cache(1024);
    check_eq(
        &cache.hit_rate().to_bits(),
        &0,
        "initial hit rate should be zero",
    )?;
    check_eq(
        &cache.average_access_time(),
        &0,
        "initial average access time should be zero",
    )?;
    let mut out = vec![format!("initial:{}", scenario_stats(&cache))];
    let key = b"abcd";
    let val = b"efghijkl";
    let set = cache.set(key, val, 0);
    check_eq(&set, &Ok(()), "err should be nil")?;
    out.push(format!("set:{}", err_str(&set)));
    let found = cache.get(key);
    check_eq(&found, &Ok(val.to_vec()), "value not equal")?;
    out.push(format!("get:{}", nil_val_str(&found)));
    let deleted = cache.del(key);
    check(deleted, "del should return affected true")?;
    out.push(format!("del:{deleted}"));
    let missing = cache.get(key);
    check_eq(
        &missing,
        &Err(CacheError::NotFound),
        "error should be ErrNotFound after being deleted",
    )?;
    out.push(format!("get:{}", nil_val_str(&missing)));
    let deleted_again = cache.del(key);
    check(!deleted_again, "del should not return affected true")?;
    out.push(format!("del:{deleted_again}"));

    cache.clear();
    let n = 5000;
    let mut set_errors = 0;
    for i in 0..n {
        let key_str = format!("key{i}");
        if cache
            .set(key_str.as_bytes(), repeat10(&key_str).as_bytes(), 0)
            .is_err()
        {
            set_errors += 1;
        }
    }
    clock.advance(1);
    for i in (1..n).step_by(2) {
        drop(cache.get(format!("key{i}").as_bytes()));
    }
    for i in (1..n).step_by(8) {
        let _ = cache.del(format!("key{i}").as_bytes());
    }
    for i in (0..n).step_by(2) {
        let key_str = format!("key{i}");
        if cache
            .set(key_str.as_bytes(), repeat10(&key_str).as_bytes(), 0)
            .is_err()
        {
            set_errors += 1;
        }
    }
    let (mut hits, mut mismatches, mut fn_hits) = (0, 0, 0);
    for i in (1..n).step_by(2) {
        let key_str = format!("key{i}");
        let expected = repeat10(&key_str);
        if let Ok(stored) = cache.get(key_str.as_bytes()) {
            hits += 1;
            if stored != expected.as_bytes() {
                mismatches += 1;
            }
        }
        if let Ok(equal) = cache.get_fn(key_str.as_bytes(), |view| view == expected.as_bytes()) {
            fn_hits += 1;
            if !equal {
                mismatches += 1;
            }
        }
    }
    check_eq(&set_errors, &0, "set errors")?;
    check_eq(&mismatches, &0, "values read back")?;
    out.push(format!(
        "seterr:{set_errors} hits:{hits} fnhits:{fn_hits} mismatch:{mismatches}"
    ));
    out.push(format!("logged:{}", scenario_stats(&cache)));
    cache.reset_statistics();
    out.push(format!("reset:{}", scenario_stats(&cache)));
    check_eq(&out.join(" "), &scenario("TestFreeCache")?, "Go counters")
}

/// Go: `TestOverwrite`.
#[test]
fn overwrite() -> TestResult {
    let (cache, _clock) = fake_cache(1024);
    let key = b"abcd";
    let mut val = Vec::new();
    cache.set(key, &val, 0)?;
    val = b"efgh".to_vec();
    cache.set(key, &val, 0)?;
    val.push(b'i');
    cache.set(key, &val, 0)?;
    check_eq(&cache.overwrite_count(), &0, "overwrite count")?;
    check_eq(&cache.get(key)?, &val, "value after growing")?;
    val.push(b'j');
    cache.set(key, &val, 0)?;
    check_eq(&cache.get(key)?, &val, "value after growing in place")?;
    val.push(b'k');
    cache.set(key, &val, 0)?;
    check_eq(&cache.get(key)?, b"efghijk", "value")?;
    val.push(b'l');
    cache.set(key, &val, 0)?;
    check_eq(&cache.get(key)?, b"efghijkl", "value")?;
    val.push(b'm');
    cache.set(key, &val, 0)?;
    check_eq(&cache.overwrite_count(), &3, "overwrite count")
}

/// Go: `TestGetOrSet`.
#[test]
fn get_or_set() -> TestResult {
    let (cache, _clock) = fake_cache(1024);
    let key = b"abcd";
    check_eq(
        &cache.get_or_set(key, b"efgh", 10),
        &Ok(None),
        "Expected to have nils",
    )?;
    // check entry
    check_eq(&cache.get(key)?, b"efgh", "entry")?;
    // call twice for the same key
    check_eq(
        &cache.get_or_set(key, b"xxxx", 10),
        &Ok(Some(b"efgh".to_vec())),
        "Expected to get old record",
    )?;
    check_eq(
        &cache.get_fn(key, <[u8]>::to_vec),
        &Ok(b"efgh".to_vec()),
        "getfn: Expected to get old record",
    )
}

/// Go: `TestGetWithExpiration`.
#[test]
fn get_with_expiration() -> TestResult {
    let (cache, clock) = fake_cache(1024);
    let key = b"abcd";
    let val = b"efgh";
    cache.set(key, val, 2)?;

    let (res, expiry) = cache.get_with_expiration(key)?;
    let mut elapsed = 0;
    while cache.get_with_expiration(key).is_ok() && clock.now() <= expiry + 1 {
        clock.advance(1);
        elapsed += 1;
    }
    check(
        (1..=3).contains(&elapsed),
        "Cache should expire within a second of the expire time",
    )?;
    check_eq(&res, val, "value")
}

/// Go: `TestExpire`.
#[test]
fn expire() -> TestResult {
    let (cache, clock) = fake_cache(1024);
    let key = b"abcd";
    cache.set(key, b"efgh", 1)?;
    clock.advance(1);
    check_eq(
        &cache.get(key),
        &Err(CacheError::NotFound),
        "key should be expired",
    )?;

    cache.reset_statistics();
    check_eq(&cache.expired_count(), &0, "expired count should be zero.")
}

/// Go: `TestTTL`.
#[test]
fn ttl() -> TestResult {
    let (cache, clock) = fake_cache(1024);
    let key = b"abcd";
    cache.set(key, b"efgh", 2)?;
    clock.advance(1);
    check_eq(&cache.ttl(key)?, &1, "ttl should be 1")
}

/// Go: `TestTouch`.
#[test]
fn touch() -> TestResult {
    let (cache, clock) = fake_cache(1024);
    let key1 = b"abcd";
    let key2 = b"ijkl";
    cache.set(key1, b"efgh", 1)?;
    cache.set(key2, b"mnop", 1)?;
    check_eq(&cache.touched_count(), &0, "touched count")?;
    cache.touch(key1, 2)?;
    clock.advance(1);
    check_eq(&cache.ttl(key1)?, &1, "ttl should be 1")?;
    check_eq(&cache.touched_count(), &1, "touched count")?;
    check_eq(
        &cache.touch(key2, 2),
        &Err(CacheError::NotFound),
        "error should be ErrNotFound after expiring",
    )?;
    check_eq(&cache.touched_count(), &1, "touched count")
}

/// Go: `TestAverageAccessTimeWhenUpdateInplace`.
#[test]
fn average_access_time_when_update_inplace() -> TestResult {
    let (cache, clock) = fake_cache(1024);
    let key = b"test-key";
    cache.set(key, b"very-long-de-value", 0)?;
    check(
        i64::from(clock.now()) - cache.average_access_time() <= 1,
        "track average access time error",
    )?;

    clock.advance(4);
    cache.set(key, b"short", 0)?;
    check(
        i64::from(clock.now()) - cache.average_access_time() <= 1,
        "track average access time error",
    )
}

/// Go: `TestAverageAccessTimeWhenUpdateWithNewSpace`.
#[test]
fn average_access_time_when_update_with_new_space() -> TestResult {
    let (cache, clock) = fake_cache(1024);
    let key = b"test-key";
    cache.set(key, b"short", 0)?;
    check(
        i64::from(clock.now()) - cache.average_access_time() <= 1,
        "track average access time error",
    )?;

    clock.advance(4);
    cache.set(key, b"very-long-de-value", 0)?;
    // The replaced entry still counts until it is evacuated: (now-4 + now) / 2.
    check_eq(
        &(i64::from(clock.now()) - cache.average_access_time()),
        &2,
        "average access time",
    )
}

/// Go: `TestLargeEntry`.
#[test]
fn large_entry() -> TestResult {
    let cache_size = 512 * 1024;
    let (cache, _clock) = fake_cache(cache_size);
    let large_key = vec![0; 65536];
    check_eq(
        &cache.set(&large_key, b"efgh", 0),
        &Err(CacheError::LargeKey),
        "large key should return ErrLargeKey",
    )?;
    check_eq(
        &cache.get(&large_key),
        &Err(CacheError::NotFound),
        "value should be nil when get a big key",
    )?;
    let key = b"abcd";
    let max_val_len = 512 * 1024 / 1024 - ENTRY_HDR_SIZE - key.len();
    let mut val = vec![0; max_val_len + 1];
    check_eq(
        &cache.set(key, &val, 0),
        &Err(CacheError::LargeEntry),
        "err should be ErrLargeEntry",
    )?;
    val = vec![0; max_val_len - 2];
    cache.set(key, &val, 0)?;
    val.push(0);
    cache.set(key, &val, 0)?;
    val.push(0);
    cache.set(key, &val, 0)?;
    check_eq(
        &cache.overwrite_count(),
        &1,
        "over write count should be one",
    )?;
    val.push(0);
    check_eq(
        &cache.set(key, &val, 0),
        &Err(CacheError::LargeEntry),
        "err should be ErrLargeEntry",
    )?;

    cache.reset_statistics();
    check_eq(
        &cache.overwrite_count(),
        &0,
        "over write count should be zero.",
    )
}

/// Go: `TestInt64Key`.
#[test]
fn int64_key() -> TestResult {
    let (cache, clock) = fake_cache(1024);
    cache.set_int(1, b"abc", 3)?;
    cache.set_int(2, b"cde", 3)?;
    check_eq(&cache.get_int(1)?, b"abc", "value not equal")?;
    clock.advance(2);
    let (val, expiry) = cache.get_int_with_expiration(1)?;
    check_eq(&val, b"abc", "value not equal")?;
    check_eq(
        &expiry,
        &(clock.now() + 1),
        "Expiry should one second in the future",
    )?;

    check(cache.del_int(1), "del should return affected true")?;
    check_eq(
        &cache.get_int(1),
        &Err(CacheError::NotFound),
        "error should be ErrNotFound after being deleted",
    )
}

/// Go: `TestIterator`.
#[test]
fn iterator() -> TestResult {
    let (cache, clock) = fake_cache(1024);
    let count = 10000;
    for i in 0..count {
        cache.set(i.to_string().as_bytes(), format!("val{i}").as_bytes(), 0)?;
    }
    // Set some value that expires to make sure expired entry is not returned.
    cache.set(b"abc", b"def", 1)?;
    clock.advance(2);
    let mut order = Fnv::new();
    let mut n = 0;
    let mut it = cache.new_iterator();
    for _ in 0..count {
        let entry = it.next().ok_or_else(|| format!("entry is nil for {n}"))?;
        let mut want = b"val".to_vec();
        want.extend_from_slice(&entry.key);
        check_eq(&entry.value, &want, "entry key value not match")?;
        order.i64(i64::try_from(entry.key.len())?);
        order.bytes(&entry.key);
        n += 1;
    }
    check(it.next().is_none(), "no entry after the last")?;
    check_eq(
        &format!("n:{n} mismatch:0 order:{}", order.hex()),
        &scenario("TestIterator")?,
        "Go iteration order",
    )
}

/// Go: `TestSetLargerEntryDeletesWrongEntry`.
#[test]
fn set_larger_entry_deletes_wrong_entry() -> TestResult {
    let (cache, _clock) = fake_cache(512 * 1024);
    let key1 = b"key1";
    cache.set(key1, b"aaa", 0)?;

    let entries: Vec<_> = cache.new_iterator().map(|e| (e.key, e.value)).collect();
    check_eq(&entries, &[(key1.to_vec(), b"aaa".to_vec())], "entries")?;

    cache.set(key1, b"aaaXXXXXX", 0)?;
    let value = b"aaaXXXXYYYYYYY";
    cache.set(key1, value, 0)?;
    let grown: Vec<_> = cache.new_iterator().map(|e| (e.key, e.value)).collect();
    check_eq(
        &grown,
        &[(key1.to_vec(), value.to_vec())],
        "entries after growing",
    )
}

/// Go: `TestRace`.
#[test]
fn race() -> TestResult {
    let cache = Cache::with_timer(512 * 1024, FakeTimer::at(CLOCK_START));
    let in_use = 8;
    let iters = 1000;
    let start = Barrier::new(6);
    thread::scope(|scope| -> TestResult {
        let workers = [
            scope.spawn(|| {
                let _ = start.wait();
                let mut rng = SplitMix64::new(1);
                for _ in 0..iters {
                    cache.set_int(rng.below(in_use).cast_signed(), b"abc", 1)?;
                }
                Ok(())
            }),
            scope.spawn(|| {
                let _ = start.wait();
                let mut rng = SplitMix64::new(2);
                for _ in 0..iters {
                    // it will likely error with the deletes running too
                    drop(cache.get_int(rng.below(in_use).cast_signed()));
                }
                Ok(())
            }),
            scope.spawn(|| {
                let _ = start.wait();
                let mut rng = SplitMix64::new(3);
                for _ in 0..iters {
                    let _ = cache.del_int(rng.below(in_use).cast_signed());
                }
                Ok(())
            }),
            scope.spawn(|| {
                let _ = start.wait();
                for _ in 0..iters {
                    let _ = (
                        cache.evacuate_count(),
                        cache.expired_count(),
                        cache.entry_count(),
                        cache.average_access_time(),
                        cache.hit_count(),
                        cache.lookup_count(),
                        cache.hit_rate(),
                        cache.overwrite_count(),
                    );
                }
                Ok(())
            }),
            scope.spawn(|| {
                let _ = start.wait();
                for _ in 0..iters {
                    cache.reset_statistics();
                }
                Ok(())
            }),
            scope.spawn(|| {
                let _ = start.wait();
                for _ in 0..iters {
                    cache.clear();
                }
                Ok(())
            }),
        ];
        for worker in workers {
            join(worker)?;
        }
        Ok(())
    })
}

/// Go: `TestConcurrentSet`.
#[test]
fn concurrent_set() -> TestResult {
    let cache = Cache::with_timer(256 * 1024 * 1024, FakeTimer::at(CLOCK_START));
    let n: u64 = 4000;
    let routines: u64 = 50;
    thread::scope(|scope| -> TestResult {
        let workers: Vec<_> = (0..routines)
            .map(|fact| {
                let cache = &cache;
                scope.spawn(move || {
                    for i in n * fact..(fact + 1) * n {
                        cache.set(&i.to_le_bytes(), &(i * 2).to_le_bytes(), 0)?;
                    }
                    Ok(())
                })
            })
            .collect();
        for worker in workers {
            join(worker)?;
        }
        Ok(())
    })?;
    let mut value = Vec::with_capacity(8);
    for i in 0..routines * n {
        cache.get_with_buf(&i.to_le_bytes(), &mut value)?;
        let num = u64::from_le_bytes(value.as_slice().try_into()?);
        check_eq(&num, &(i * 2), "stored value")?;
    }
    Ok(())
}

/// Go: `TestEvacuateCount`.
#[test]
fn evacuate_count() -> TestResult {
    let (cache, _clock) = fake_cache(1024 * 1024);
    let n = 100_000;
    for i in 0..n {
        cache.set(i.to_string().as_bytes(), b"A", 0)?;
    }
    let mut missing_items = 0;
    for i in 0..n {
        match cache.get(i.to_string().as_bytes()) {
            Err(CacheError::NotFound) => missing_items += 1,
            Ok(res) if res != b"A" => missing_items += 1,
            Ok(_) => {}
            Err(err) => return Err(err.into()),
        }
    }
    check_eq(
        &(cache.entry_count() + cache.evacuate_count()),
        &n,
        "entries plus evacuations",
    )?;
    check_eq(
        &format!(
            "missing:{missing_items} entries:{} evacuates:{}",
            cache.entry_count(),
            cache.evacuate_count()
        ),
        &scenario("TestEvacuateCount")?,
        "Go counters",
    )
}

/// Go: `TestConcurrentGetTTL`.
#[test]
fn concurrent_get_ttl() -> TestResult {
    let cache = Cache::with_timer(256 * 1024 * 1024, FakeTimer::at(CLOCK_START));
    let primary_key = b"hello";
    cache.set(primary_key, b"world", 2)?;

    thread::scope(|scope| -> TestResult {
        // Do concurrent mutation by adding various keys.
        let writers: Vec<_> = (0..16)
            .map(|worker| {
                let cache = &cache;
                scope.spawn(move || {
                    for idx in (worker..1000).step_by(16) {
                        let key_value = format!("counter_{idx}");
                        cache.set(key_value.as_bytes(), key_value.as_bytes(), 0)?;
                    }
                    Ok(())
                })
            })
            .collect();
        // While trying to read the TTL.
        check_eq(&cache.ttl(primary_key)?, &2, "TTL while writing")?;
        for writer in writers {
            join(writer)?;
        }
        Ok(())
    })
}

/// Go: `TestSetAndGet`.
#[test]
fn set_and_get() -> TestResult {
    let (cache, _clock) = fake_cache(1024);
    let key = b"abcd";
    let (found, result) = cache.set_and_get(key, b"efgh", 0);
    result?;
    check_eq(&found, &None, "SetAndGet unexpected found data")?;

    let (previous, stored) = cache.set_and_get(key, b"ijkl", 0);
    stored?;
    check_eq(
        &previous,
        &Some(b"efgh".to_vec()),
        "SetAndGet expected found data",
    )
}

#[test]
fn default_timer_cache() -> TestResult {
    let cache = Cache::new(0);
    cache.set(b"key", b"value", 60)?;
    let (value, expire_at) = cache.get_with_expiration(b"key")?;
    check_eq(&value, b"value", "value")?;
    let now = DefaultTimer.now();
    check(
        expire_at > now && expire_at <= now + 60,
        "expireAt should be within a minute",
    )?;
    check_eq(&cache.ttl(b"key").map(|ttl| ttl <= 60), &Ok(true), "ttl")
}

#[test]
fn peek_ignores_expiration_and_counters() -> TestResult {
    let (cache, clock) = fake_cache(0);
    cache.set(b"k", b"v", 1)?;
    clock.advance(5);
    check_eq(
        &cache.peek(b"k")?,
        b"v",
        "Go's Peek returns an expired entry",
    )?;
    check_eq(
        &cache.peek_fn(b"k", bytes_digest),
        &Ok(bytes_digest(b"v")),
        "PeekFn",
    )?;
    check_eq(
        &(cache.hit_count(), cache.miss_count()),
        &(0, 0),
        "peeks are not counted",
    )?;
    check_eq(
        &cache.get(b"k"),
        &Err(CacheError::NotFound),
        "Get expires it",
    )?;
    check_eq(
        &(cache.miss_count(), cache.expired_count()),
        &(1, 1),
        "expired get",
    )
}

#[test]
fn expire_seconds_wrap_like_go() -> TestResult {
    let (cache, clock) = fake_cache(0);
    // uint32(1<<32) is 0: the entry expires at now, so the next Get misses.
    cache.set(b"a", b"1", 1 << 32)?;
    check_eq(
        &cache.get(b"a"),
        &Err(CacheError::NotFound),
        "expireAt = now",
    )?;
    // uint32(1<<32 + 5) is 5.
    cache.set(b"b", b"2", (1 << 32) + 5)?;
    check_eq(&cache.ttl(b"b")?, &5, "truncated ttl")?;
    // now + 0xFFFFFFFF wraps to now - 1.
    cache.set(b"c", b"3", i64::MAX)?;
    check_eq(
        &cache.ttl(b"c"),
        &Err(CacheError::NotFound),
        "wrapped expiration",
    )?;
    clock.advance(1);
    check_eq(&cache.ttl(b"b")?, &4, "ttl after a second")
}

/// Go: `segmentZeroKeys`, the first `n` keys `k0`, `k1`, ... in segment 0.
fn segment_zero_keys(n: usize) -> Vec<Vec<u8>> {
    (0_u64..)
        .map(|i| format!("k{i}").into_bytes())
        .filter(|key| xxhash::sum64(key).trailing_zeros() >= 8)
        .take(n)
        .collect()
}

/// The evacuate count, entry count and average access time `scenarioSixMoves` records.
fn move_stats(cache: &FakeCache) -> String {
    format!(
        "ev:{} e:{} aat:{}",
        cache.evacuate_count(),
        cache.entry_count(),
        cache.average_access_time()
    )
}

/// Go: `scenarioSixMoves`. Evacuation moves recently used entries to the front of the ring at
/// most six times in a row (`consecutiveEvacuate > 5`); the seventh is evicted even though it
/// was used recently.
#[test]
fn six_consecutive_moves_then_evict() -> TestResult {
    let (cache, clock) = fake_cache(512 * 1024);
    let keys = segment_zero_keys(16);
    let value = [b'v'; 100];
    for key in keys.iter().take(15) {
        cache.set(key, &value, 0)?;
    }
    clock.advance(100);
    for key in keys.iter().take(7) {
        check_eq(&cache.get(key)?, &value, "recently used value")?;
    }
    let before = move_stats(&cache);
    let last = keys.last().ok_or("no keys")?;
    cache.set(last, &value, 0)?;
    let after = move_stats(&cache);
    let found: String = keys
        .iter()
        .map(|key| if cache.peek(key).is_ok() { '1' } else { '0' })
        .collect();
    check_eq(
        &(cache.evacuate_count(), found.as_str()),
        &(7, "1111110111111111"),
        "six moves, then the seventh entry is evicted",
    )?;
    check_eq(
        &format!("before:{before} after:{after} found:{found}"),
        &scenario("SixMoves")?,
        "Go counters",
    )
}

/// The cache is built on the heap like Go's `new(Cache)`: boxing a 256-segment array built on
/// the stack took over 256 KiB of stack in debug builds and over 64 KiB in release builds.
#[test]
fn construction_needs_little_stack() -> TestResult {
    let entries = thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(|| Cache::new(0).entry_count())?
        .join()
        .map_err(|panic| format!("construction panicked: {panic:?}"))?;
    check_eq(&entries, &0, "entries of a new cache")
}

#[test]
fn cache_is_send_and_sync() {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Cache>();
    assert_send_sync::<FakeCache>();
}
