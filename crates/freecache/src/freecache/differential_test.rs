//! Replays the goldens written by `testdata/go` from the real Go freecache and xxhash.

use std::{collections::BTreeSet, error::Error};

use super::{
    Cache,
    golden::{
        FakeCache, FakeTimer, Fnv, Runner, ScriptSpec, SplitMix64, TestResult, check, check_eq,
        counters, err_str, field, golden_lines, hex_decode, read_golden, write_trace,
    },
    xxhash,
};

/// Go: `xxhashSeed`.
const XXHASH_SEED: u64 = 0x7878_6861_7368_0001;

/// Every length up to 160, a spread up to a few KiB and the key limits, as Go's
/// `xxhashLengths`.
fn xxhash_lengths() -> Vec<usize> {
    let mut lengths: Vec<usize> = (0..=160).collect();
    lengths.extend((161..=5000).step_by(97));
    lengths.extend([65535, 65536]);
    lengths
}

#[test]
fn go_xxhash_sum64() -> TestResult {
    let golden = read_golden("xxhash.golden")?;
    let lines = golden_lines(&golden, "xxhash");
    let lengths = xxhash_lengths();
    check_eq(&lines.len(), &lengths.len(), "xxhash.golden line count")?;
    let mut rng = SplitMix64::new(XXHASH_SEED);
    for (fields, n) in lines.iter().zip(lengths) {
        check_eq(field(fields, 0)?, n.to_string().as_str(), "input length")?;
        let mut input = vec![0; n];
        rng.fill(&mut input);
        check_eq(
            format!("{:016x}", xxhash::sum64(&input)).as_str(),
            field(fields, 1)?,
            &format!("Sum64 of {n} bytes"),
        )?;
    }
    Ok(())
}

/// The colliding keys of `scripts.golden`, checked against the port's hash.
fn collisions(golden: &str) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let mut keys = Vec::new();
    for fields in golden_lines(golden, "collide") {
        let key = hex_decode(field(&fields, 0)?)?;
        check_eq(
            format!("{:08x}", xxhash::sum64(&key) & 0xFFFF_FFFF).as_str(),
            field(&fields, 1)?,
            &format!("low 32 bits of Sum64({key:x?})"),
        )?;
        keys.push(key);
    }
    Ok(keys)
}

#[test]
fn go_collisions_share_segment_slot_and_tag() -> TestResult {
    let golden = read_golden("scripts.golden")?;
    let keys = collisions(&golden)?;
    check_eq(&keys.len(), &16, "colliding keys")?;
    for pair in keys.chunks(2) {
        let hashes: Vec<u64> = pair
            .iter()
            .map(|key| xxhash::sum64(key) & 0xFFFF_FFFF)
            .collect();
        check(
            pair.len() == 2 && hashes.windows(2).all(|w| w.first() == w.last()),
            &format!("{pair:x?} share the low 32 bits"),
        )?;
    }
    Ok(())
}

/// Go: `deepTarget`, the low 16 bits (segment and slot) every deep key hashes to.
const DEEP_TARGET: u64 = 0x2A07;

/// Go: `deepCount`.
const DEEP_COUNT: usize = 1500;

/// The deep keys of `scripts.golden`, checked against the port's hash.
fn deep_keys(golden: &str) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let mut keys = Vec::new();
    for fields in golden_lines(golden, "deep") {
        let key = hex_decode(field(&fields, 0)?)?;
        check_eq(
            &(xxhash::sum64(&key) & 0xFFFF),
            &DEEP_TARGET,
            &format!("low 16 bits of Sum64({key:x?})"),
        )?;
        keys.push(key);
    }
    Ok(keys)
}

#[test]
fn go_deep_keys_share_segment_and_slot() -> TestResult {
    let golden = read_golden("scripts.golden")?;
    let keys = deep_keys(&golden)?;
    check_eq(&keys.len(), &DEEP_COUNT, "deep keys")?;
    let distinct: BTreeSet<&[u8]> = keys.iter().map(Vec::as_slice).collect();
    check_eq(&distinct.len(), &DEEP_COUNT, "distinct deep keys")?;
    // Several pairs also share the 16-bit tag, so lookups walk runs of equal tags deep in the
    // slot.
    let tags: BTreeSet<u64> = keys
        .iter()
        .map(|key| (xxhash::sum64(key) >> 16) & 0xFFFF)
        .collect();
    check(
        DEEP_COUNT - tags.len() >= 10,
        &format!("only {} deep keys share a tag", DEEP_COUNT - tags.len()),
    )
}

/// The scripts the tests below replay, which must be every script in the golden.
const SCRIPTS: [&str; 15] = [
    "small_evict",
    "near_limits",
    "ttl_wrap",
    "clock_back",
    "hot_keys_lru",
    "big_cache_huge_keys",
    "clear_reset_iterate",
    "int_keys",
    "empty_values",
    "negative_size",
    "deep_slot",
    "deep_slot_tiny",
    "ticking_small",
    "ticking_deep",
    "iter_mutate",
];

#[test]
fn go_scripts_are_all_replayed() -> TestResult {
    let golden = read_golden("scripts.golden")?;
    let names: BTreeSet<&str> = golden_lines(&golden, "script")
        .iter()
        .filter_map(|fields| fields.first().copied())
        .collect();
    check_eq(
        &names,
        &SCRIPTS.into_iter().collect(),
        "scripts in scripts.golden",
    )
}

/// Replays script `name` and compares every batch line: the digests of the operations, of the
/// results and counters, of the whole cache state, and the final counters.
fn replay_script(name: &str) -> TestResult {
    let golden = read_golden("scripts.golden")?;
    let collisions = collisions(&golden)?;
    let deep = deep_keys(&golden)?;
    let spec_fields = golden_lines(&golden, "script")
        .into_iter()
        .find(|fields| fields.first() == Some(&name))
        .ok_or_else(|| format!("script {name} is not in scripts.golden"))?;
    let spec = ScriptSpec::parse(&spec_fields)?;
    let batches: Vec<Vec<&str>> = golden_lines(&golden, "batch")
        .into_iter()
        .filter(|fields| fields.first() == Some(&name))
        .collect();
    let mut want_batches = batches.iter();

    let cache = spec.new_cache();
    let mut runner = Runner::new(&spec, &cache, &collisions, &deep);
    let mut ops = Fnv::new();
    let mut results = Fnv::new();
    let mut records = Vec::with_capacity(spec.batch);
    for op in 0..spec.ops {
        let (desc, res) = runner.step();
        let record = runner.record(&res);
        ops.bytes(desc.as_bytes());
        ops.bytes(b"\n");
        results.bytes(record.as_bytes());
        results.bytes(b"\n");
        records.push(format!("{op} {desc} => {record}"));
        if (op + 1) % spec.batch != 0 && op + 1 != spec.ops {
            continue;
        }
        let batch = op / spec.batch;
        let state = if batch % spec.state_every == 0 || op + 1 == spec.ops {
            let mut digest = Fnv::new();
            runner.cache.digest_state(&mut digest);
            digest.hex()
        } else {
            "-".to_owned()
        };
        let want = want_batches
            .next()
            .ok_or_else(|| format!("{name}: no golden line for batch {batch}"))?;
        let got = [
            batch.to_string(),
            ops.hex(),
            results.hex(),
            state,
            counters(runner.cache),
        ];
        for (index, (value, label)) in got
            .iter()
            .zip(["batch", "operations", "results", "state", "counters"])
            .enumerate()
        {
            let expected = field(want, index + 1)?;
            if value != expected {
                let trace = write_trace(name, batch, &records);
                return Err(format!(
                    "{name} batch {batch}: {label} {value} != Go {expected}. Rust records: \
                     {trace}; Go records: go run . -trace {name} -from {} -to {} in \
                     crates/freecache/testdata/go",
                    batch * spec.batch,
                    op + 1
                )
                .into());
            }
        }
        ops = Fnv::new();
        results = Fnv::new();
        records.clear();
    }
    check(
        want_batches.next().is_none(),
        &format!("{name}: scripts.golden has more batches than the script"),
    )
}

#[test]
fn go_script_small_evict() -> TestResult {
    replay_script("small_evict")
}

#[test]
fn go_script_near_limits() -> TestResult {
    replay_script("near_limits")
}

#[test]
fn go_script_ttl_wrap() -> TestResult {
    replay_script("ttl_wrap")
}

#[test]
fn go_script_clock_back() -> TestResult {
    replay_script("clock_back")
}

#[test]
fn go_script_hot_keys_lru() -> TestResult {
    replay_script("hot_keys_lru")
}

#[test]
fn go_script_big_cache_huge_keys() -> TestResult {
    replay_script("big_cache_huge_keys")
}

#[test]
fn go_script_clear_reset_iterate() -> TestResult {
    replay_script("clear_reset_iterate")
}

#[test]
fn go_script_int_keys() -> TestResult {
    replay_script("int_keys")
}

#[test]
fn go_script_empty_values() -> TestResult {
    replay_script("empty_values")
}

#[test]
fn go_script_negative_size() -> TestResult {
    replay_script("negative_size")
}

/// Hundreds of entries in one slot: `slotCap` grows to 512, and keys sharing a 16-bit tag sit
/// deep inside it.
#[test]
fn go_script_deep_slot() -> TestResult {
    replay_script("deep_slot")
}

/// One slot of the smallest cache: nearly every write evicts from the slot it inserts into.
#[test]
fn go_script_deep_slot_tiny() -> TestResult {
    replay_script("deep_slot_tiny")
}

/// A clock that advances as it is read: every result depends on how often and when the cache
/// reads its clock.
#[test]
fn go_script_ticking_small() -> TestResult {
    replay_script("ticking_small")
}

/// The ticking clock on deep slots, wrapping past `u32::MAX` and moving back.
#[test]
fn go_script_ticking_deep() -> TestResult {
    replay_script("ticking_deep")
}

/// An iterator kept across writes, deletes and clears.
#[test]
fn go_script_iter_mutate() -> TestResult {
    replay_script("iter_mutate")
}

/// Go: `limitKeyLens`.
const LIMIT_KEY_LENS: [usize; 12] = [
    0, 1, 8, 100, 487, 488, 489, 4096, 65511, 65512, 65535, 65536,
];

/// Go: `maxAccepted`, the longest value `set` accepts with a key of `key_len` bytes, by
/// bisection, and the error for one byte more.
fn max_accepted(cache: &FakeCache, key_len: usize) -> String {
    let key = vec![0; key_len];
    if let Err(err) = cache.set(&key, &[], 0) {
        return format!("-1 err:{err}");
    }
    let (mut lo, mut hi) = (0, 1 << 17);
    while lo < hi {
        let mid = usize::midpoint(lo + 1, hi);
        if cache.set(&key, &vec![0; mid], 0).is_ok() {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    format!("{lo} {}", err_str(&cache.set(&key, &vec![0; lo + 1], 0)))
}

/// The entry size limit (`size / 1024 - 24` for key plus value, keys up to 65535 bytes) across
/// cache sizes around the minimum, the 256 segments and the key limit.
#[test]
fn go_entry_limits() -> TestResult {
    let golden = read_golden("limits.golden")?;
    let lines = golden_lines(&golden, "limit");
    check_eq(&lines.len(), &21, "limits.golden line count")?;
    for fields in lines {
        let size: i64 = field(&fields, 0)?.parse()?;
        let cache = Cache::with_timer(size, FakeTimer::at(1_700_000_000));
        let results: Vec<String> = LIMIT_KEY_LENS
            .iter()
            .map(|&key_len| format!("k{key_len}:{}", max_accepted(&cache, key_len)))
            .collect();
        check_eq(
            results.join(",").as_str(),
            field(&fields, 1)?,
            &format!("limits of a {size}-byte cache"),
        )?;
        check_eq(
            counters(&cache).as_str(),
            field(&fields, 2)?,
            &format!("counters of a {size}-byte cache"),
        )?;
    }
    Ok(())
}
