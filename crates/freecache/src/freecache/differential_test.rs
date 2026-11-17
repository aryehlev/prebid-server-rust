//! Replays the goldens written by `testdata/go` from the real Go freecache and xxhash.

use std::{collections::BTreeSet, error::Error};

use super::{
    golden::{
        Fnv, Runner, ScriptSpec, SplitMix64, TestResult, check, check_eq, counters, field,
        golden_lines, hex_decode, read_golden, write_trace,
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

/// The scripts the tests below replay, which must be every script in the golden.
const SCRIPTS: [&str; 10] = [
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

    let mut runner = Runner::new(&spec, &collisions);
    let mut ops = Fnv::new();
    let mut results = Fnv::new();
    let mut records = Vec::with_capacity(spec.batch);
    for op in 0..spec.ops {
        let (desc, res) = runner.step();
        let counters = counters(&runner.cache);
        ops.bytes(desc.as_bytes());
        ops.bytes(b"\n");
        results.bytes(res.as_bytes());
        results.bytes(b" | ");
        results.bytes(counters.as_bytes());
        results.bytes(b"\n");
        records.push(format!("{op} {desc} => {res} | {counters}"));
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
        let got = [batch.to_string(), ops.hex(), results.hex(), state, counters];
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
                     {trace}; Go records: go run ./rust/crates/freecache/testdata/go -trace \
                     {name} -from {} -to {}",
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
