use std::{collections::BTreeMap, thread};

use pbs_gocompat::time::Time as GoTime;
use pbs_util::{
    randomutil::RandomGenerator as _, timeutil::Time as _, uuidutil::UuidGenerator as _,
};

use super::{
    DEFAULT_INT63, DEFAULT_NOW_UNIX_MS, DEFAULT_UUID, OrderSite, ParityUuidGenerator, Source,
    active, defaults, int, map_keys, new_v4, now, sequence_uuid,
};
use crate::{
    envelope::FakeValues,
    golden::{TestResult, check, check_eq, contract_json, global_lock_blocking},
};

/// The RFC 3339 layout with milliseconds that `fakes.json` `derived` uses.
const MILLIS: &str = "2006-01-02T15:04:05.000Z07:00";

fn is_v4(u: &str) -> bool {
    let b = u.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *c == b'-'
            } else {
                c.is_ascii_digit() || (b'a'..=b'f').contains(c)
            }
        })
        && b.get(14) == Some(&b'4')
        && b.get(19).is_some_and(|c| b"89ab".contains(c))
}

/// The ISO 8601 week of a date that is not in the first or last week of its year.
fn iso_week(t: &GoTime) -> i64 {
    let iso_weekday = (t.weekday() + 6) % 7 + 1;
    (t.year_day() - iso_weekday + 10) / 7
}

// fakes.json defaults, derived values and the first generated UUIDs.
#[test]
fn defaults_match_the_registry() -> TestResult {
    let registry = contract_json("fakes.json")?;
    let want = registry.get("defaults")?;
    check_eq(
        &DEFAULT_NOW_UNIX_MS,
        &want.int("now_unix_ms")?,
        "now_unix_ms",
    )?;
    check_eq(&DEFAULT_INT63, &want.int("int63")?, "int63")?;
    check_eq(&DEFAULT_UUID.to_owned(), &want.string("uuid")?, "uuid")?;
    check_eq(
        &defaults(),
        &FakeValues {
            now_unix_ms: DEFAULT_NOW_UNIX_MS,
            int63: DEFAULT_INT63,
            uuid: DEFAULT_UUID.to_owned(),
        },
        "defaults()",
    )?;

    let derived = registry.get("derived")?;
    let fixed = Source::new().now();
    check_eq(
        &fixed.format(MILLIS),
        &derived.string("now_rfc3339")?,
        "rfc3339",
    )?;
    check_eq(
        &fixed.unix(),
        &derived.int("now_unix_seconds")?,
        "unix seconds",
    )?;
    check_eq(&iso_week(&fixed), &derived.int("now_iso_week")?, "iso week")?;

    let first = registry.get("generated_uuids")?.array("first")?;
    for (n, uuid) in (1_u64..).zip(&first) {
        check_eq(
            &sequence_uuid(DEFAULT_UUID, n),
            &uuid.text()?,
            "generated uuid",
        )?;
    }
    check_eq(&first.len(), &3, "three listed")
}

#[test]
fn orders_match_the_registry() -> TestResult {
    let orders = contract_json("fakes.json")?.array("orders")?;
    let names: Vec<String> = orders
        .iter()
        .map(|o| o.string("name"))
        .collect::<Result<_, _>>()?;
    let ours: Vec<String> = OrderSite::ALL.iter().map(|o| o.name().to_owned()).collect();
    check_eq(&ours, &names, "orders")?;
    for site in OrderSite::ALL {
        check_eq(&OrderSite::from_name(site.name()), &Some(site), "from_name")?;
    }
    check_eq(&OrderSite::from_name("nope"), &None, "unknown")?;
    check(
        !OrderSite::BidderRequestPhase.is_map_keys()
            && !OrderSite::BidderResponseOrder.is_map_keys()
            && OrderSite::Datablocks.is_map_keys(),
        "map-key sites",
    )
}

// Go: parity/fakes/fakes_test.go TestSourceSet and TestSourceSeams.
#[test]
fn source_merges_values_and_restarts_the_stream() -> TestResult {
    let src = Source::default();
    check_eq(&src.values(), &defaults(), "new source holds the defaults")?;
    check_eq(&src.new_v4(), &sequence_uuid(DEFAULT_UUID, 1), "first draw")?;
    check_eq(
        &src.new_v4(),
        &sequence_uuid(DEFAULT_UUID, 2),
        "second draw",
    )?;

    src.set(Some(&FakeValues {
        now_unix_ms: 0,
        int63: 99,
        uuid: "seed".to_owned(),
    }));
    check_eq(
        &src.values(),
        &FakeValues {
            now_unix_ms: DEFAULT_NOW_UNIX_MS,
            int63: 99,
            uuid: "seed".to_owned(),
        },
        "zero fields take the defaults",
    )?;
    check_eq(
        &src.new_v4(),
        &sequence_uuid("seed", 1),
        "restarted from the new seed",
    )?;
    check_eq(
        &src.generate().ok(),
        &Some("seed".to_owned()),
        "Generate is the fixed uuid",
    )?;
    check_eq(&src.generate_int63(), &99, "GenerateInt63")?;
    check_eq(&src.int(), &99, "Int")?;
    check_eq(&src.intn(10), &Some(9), "Intn")?;
    check_eq(&src.intn(0), &Some(0), "Intn(0) does not panic")?;
    check_eq(&src.intn(-3), &Some(0), "Intn(<0) does not panic")?;
    check_eq(&src.now().unix_milli(), &DEFAULT_NOW_UNIX_MS, "Now")?;
    check_eq(&src.now().location().name(), &"UTC", "Now is UTC")?;

    src.set(None);
    check_eq(&src.values(), &defaults(), "reset")?;
    check_eq(&src.new_v4(), &sequence_uuid(DEFAULT_UUID, 1), "restarted")?;
    src.set(Some(&FakeValues {
        now_unix_ms: -1,
        int63: -7,
        uuid: String::new(),
    }));
    check_eq(&src.now().unix_milli(), &-1, "negative instant")?;
    check_eq(&src.intn(3), &Some(-1), "Go's % keeps the sign")
}

// Concurrent draws from one stream get distinct UUIDs, together exactly the first draws of the
// sequence, and every thread sees its own draws in stream order (Go: `Source.NewV4` takes the
// next counter value atomically).
#[test]
fn concurrent_draws_share_one_stream() -> TestResult {
    const THREADS: u64 = 8;
    const DRAWS: u64 = 64;
    let src = Source::new();
    src.set(Some(&FakeValues {
        uuid: "race".to_owned(),
        ..FakeValues::default()
    }));
    let per_thread = thread::scope(|scope| {
        let workers: Vec<_> = (0..THREADS)
            .map(|_| scope.spawn(|| (0..DRAWS).map(|_| src.new_v4()).collect::<Vec<_>>()))
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().map_err(|e| format!("a worker failed: {e:?}")))
            .collect::<Result<Vec<_>, _>>()
    })?;
    let position: BTreeMap<String, u64> = (1..=THREADS * DRAWS)
        .map(|n| (sequence_uuid("race", n), n))
        .collect();
    let mut all = Vec::new();
    for draws in &per_thread {
        let ns: Vec<u64> = draws
            .iter()
            .map(|u| {
                position
                    .get(u)
                    .copied()
                    .ok_or_else(|| format!("{u} is not in the stream"))
            })
            .collect::<Result<_, _>>()?;
        check(ns.windows(2).all(|w| w.first() < w.last()), "stream order")?;
        all.extend(ns);
    }
    all.sort_unstable();
    check_eq(
        &all,
        &(1..=THREADS * DRAWS).collect::<Vec<_>>(),
        "each value drawn once",
    )?;
    check_eq(
        &src.new_v4(),
        &sequence_uuid("race", THREADS * DRAWS + 1),
        "the stream continues",
    )
}

// Go: parity/fakes/uuids_test.go TestSequenceUUID.
#[test]
fn sequence_uuids_are_v4() -> TestResult {
    let mut seen = BTreeMap::new();
    for seed in ["", "a", DEFAULT_UUID] {
        for n in [1, 2, 3, u64::MAX] {
            let u = sequence_uuid(seed, n);
            check(is_v4(&u), &u)?;
            check(seen.insert(u, (seed, n)).is_none(), "distinct")?;
        }
    }
    Ok(())
}

// Without an installed source every helper is the production behavior.
#[test]
fn uninstalled_helpers_are_production() -> TestResult {
    let _global = global_lock_blocking();
    check(active().is_none(), "nothing installed")?;
    let keys = map_keys(["b", "c", "a"], |x, y| x < y);
    check_eq(&keys, &vec!["b", "c", "a"], "map order kept")?;
    let before = GoTime::now().unix();
    let t = now();
    check(
        t.unix() >= before && t.unix() <= GoTime::now().unix(),
        "system clock",
    )?;
    check(int() >= 0, "rand.Int is non-negative")?;
    let a = new_v4().map_err(|e| e.to_string())?;
    let b = new_v4().map_err(|e| e.to_string())?;
    check(is_v4(&a) && is_v4(&b) && a != b, "random v4 UUIDs")?;
    let fixed = ParityUuidGenerator(Source::new());
    check_eq(
        &fixed.generate().ok(),
        &Some(DEFAULT_UUID.to_owned()),
        "delegates to the wrapped generator",
    )
}

#[cfg(feature = "trace")]
mod installed {
    use core::ptr;

    use pbs_util::uuidutil::UuidGenerator as _;

    use super::super::{
        DEFAULT_UUID, ParityUuidGenerator, Source, active, install, int, map_keys, new_v4, now,
        sequence_uuid, uninstall,
    };
    use crate::{
        envelope::FakeValues,
        golden::{TestResult, check, check_eq, global_lock_blocking},
    };

    /// Uninstalls the fakes when dropped, so a failed check leaves the process clean.
    struct Installed;

    impl Drop for Installed {
        fn drop(&mut self) {
            uninstall();
        }
    }

    // Go: parity/fakes/fakes_test.go TestInstall and parity/fakes/order_test.go TestMapKeys.
    #[test]
    fn installed_helpers_are_deterministic() -> TestResult {
        let _global = global_lock_blocking();
        let _installed = Installed;
        let src = install();
        check(ptr::eq(install(), src), "install is idempotent")?;
        check(active().is_some_and(|a| ptr::eq(a, src)), "active")?;
        check_eq(&now().unix_milli(), &1_629_473_330_823, "fixed clock")?;
        check_eq(&int(), &10, "fixed int")?;
        check_eq(
            &map_keys(["b", "c", "a"], |x, y| x < y),
            &vec!["a", "b", "c"],
            "sorted",
        )?;
        check_eq(
            &new_v4().ok(),
            &Some(sequence_uuid(DEFAULT_UUID, 1)),
            "stream",
        )?;
        let wrapped = ParityUuidGenerator(Source::new());
        check_eq(
            &wrapped.generate().ok(),
            &Some(sequence_uuid(DEFAULT_UUID, 2)),
            "the wrapper draws from the installed stream",
        )?;
        src.set(Some(&FakeValues {
            int63: 5,
            ..FakeValues::default()
        }));
        check_eq(&int(), &5, "set")?;
        uninstall();
        check(active().is_none(), "uninstalled")?;
        check_eq(
            &map_keys(["b", "a"], |x, y| x < y),
            &vec!["b", "a"],
            "unsorted again",
        )?;
        let again = install();
        check_eq(
            &again.values().int63,
            &10,
            "reinstall starts from the defaults",
        )?;
        check_eq(
            &new_v4().ok(),
            &Some(sequence_uuid(DEFAULT_UUID, 1)),
            "fresh stream",
        )
    }

    // Go's `sort.Slice` never panics whatever `less` answers. `slice::sort_by` does on this input
    // ("user-provided comparison function does not correctly implement a total order"), which
    // `map_keys` used to sort with.
    #[test]
    fn map_keys_survive_inconsistent_comparisons() -> TestResult {
        let _global = global_lock_blocking();
        let _installed = Installed;
        let _src = install();
        let keys: Vec<u64> = (0..1000_u64)
            .map(|i| i.wrapping_mul(0x9E37_79B9_7F4A_7C15))
            .collect();
        let mut state = 1_u64;
        let mut got = map_keys(keys.clone(), |_, _| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.is_multiple_of(2)
        });
        let mut want = keys.clone();
        want.sort_unstable();
        got.sort_unstable();
        check_eq(&got, &want, "a permutation of the keys")?;
        check_eq(&map_keys(keys, |a, b| a < b), &want, "sorted")?;
        check_eq(
            &map_keys([31_u32, 12, 35, 18, 3, 20], |a, b| a / 10 < b / 10),
            &vec![3, 12, 18, 20, 31, 35],
            "keys less cannot tell apart keep their order",
        )
    }
}
