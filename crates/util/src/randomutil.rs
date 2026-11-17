//! Go `util/randomutil`: an injectable source of random numbers.

use std::{
    hash::{BuildHasher as _, RandomState},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

/// Go: `randomutil.RandomGenerator`.
pub trait RandomGenerator: Send + Sync {
    /// Go: `GenerateInt63`: a uniformly distributed value in `[0, 2^63)`.
    fn generate_int63(&self) -> i64;

    /// Go: `Intn`: a uniformly distributed value in `[0, n)`, or `None` where Go panics
    /// (`n <= 0`).
    fn intn(&self, n: i64) -> Option<i64> {
        let n = u64::try_from(n).ok().filter(|&n| n > 0)?;
        // Rejection sampling over 63 bits keeps the result uniform.
        let span = 1_u64 << 63;
        let limit = span - span % n;
        loop {
            let v = u64::try_from(self.generate_int63()).unwrap_or(0);
            if v < limit {
                return i64::try_from(v % n).ok();
            }
        }
    }
}

/// Go: `randomutil.RandomNumberGenerator`, which reads Go's process-wide `math/rand` source.
///
/// Go seeds that source randomly at start-up, so its sequence is not reproducible and no
/// caller depends on it; this generator is a process-wide splitmix64 sequence with a random
/// seed, which is equally unpredictable to callers and not cryptographically secure either.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RandomNumberGenerator;

/// The process-wide generator state.
fn state() -> &'static AtomicU64 {
    static STATE: OnceLock<AtomicU64> = OnceLock::new();
    STATE.get_or_init(|| AtomicU64::new(RandomState::new().hash_one(0x5EED_u64)))
}

/// The splitmix64 output for a state.
const fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl RandomGenerator for RandomNumberGenerator {
    fn generate_int63(&self) -> i64 {
        const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
        let s = state()
            .fetch_add(GAMMA, Ordering::Relaxed)
            .wrapping_add(GAMMA);
        i64::try_from(mix(s) >> 1).unwrap_or(i64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{RandomGenerator, RandomNumberGenerator};
    use crate::golden::{TestResult, check, check_eq};

    #[test]
    fn generate_int63_is_non_negative_and_varies() -> TestResult {
        let values: BTreeSet<i64> = (0..1000)
            .map(|_| RandomNumberGenerator.generate_int63())
            .collect();
        check(values.iter().all(|&v| v >= 0), "non-negative")?;
        check(values.len() > 990, "distinct values")
    }

    /// A generator that replays fixed values, as Go tests mock `RandomGenerator`.
    struct Fixed(i64);

    impl RandomGenerator for Fixed {
        fn generate_int63(&self) -> i64 {
            self.0
        }
    }

    #[test]
    fn intn_is_in_range_and_rejects_go_panics() -> TestResult {
        for _ in 0..1000 {
            let v = RandomNumberGenerator.intn(10).ok_or("intn(10)")?;
            check((0..10).contains(&v), "in [0, 10)")?;
        }
        check_eq(&RandomNumberGenerator.intn(0), &None, "Go panics for 0")?;
        check_eq(&RandomNumberGenerator.intn(-1), &None, "Go panics for -1")?;
        check_eq(&Fixed(17).intn(5), &Some(2), "17 mod 5")?;
        check_eq(&Fixed(42).generate_int63(), &42, "a mock generator")
    }
}
