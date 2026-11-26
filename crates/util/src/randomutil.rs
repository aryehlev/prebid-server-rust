//! Go `util/randomutil`: an injectable source of random numbers, on rand's thread generator.
//!
//! Go reads the process-wide `math/rand` source, which Go seeds randomly at start-up, so no
//! caller can depend on its sequence; rand's sequences are different ones (the rand.sequences
//! ledger entry), and only the parity fakes, which replace the generator, need fixed values.

use rand::RngExt as _;

/// Go: `randomutil.RandomGenerator`.
pub trait RandomGenerator: Send + Sync {
    /// Go: `GenerateInt63`: a uniformly distributed value in `[0, 2^63)`.
    fn generate_int63(&self) -> i64;

    /// Go: `Intn`: a uniformly distributed value in `[0, n)`, or `None` where Go panics
    /// (`n <= 0`).
    fn intn(&self, n: i64) -> Option<i64>;
}

/// Go: `randomutil.RandomNumberGenerator`: rand's thread-local generator, seeded from the
/// operating system and not cryptographically secure in Go either.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RandomNumberGenerator;

impl RandomGenerator for RandomNumberGenerator {
    fn generate_int63(&self) -> i64 {
        // Go: rand.Int63, uniform in [0, 2^63) (Go keeps the low 63 bits of its source's
        // draw); here the top 63 bits of a 64-bit draw.
        i64::try_from(rand::rng().random::<u64>() >> 1).unwrap_or(i64::MAX)
    }

    fn intn(&self, n: i64) -> Option<i64> {
        (n > 0).then(|| rand::rng().random_range(0..n))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{RandomGenerator, RandomNumberGenerator};
    use crate::golden::{TestResult, check, check_eq};

    #[test]
    fn generate_int63_is_non_negative_and_varies() -> TestResult {
        // Go: rand.Int63 is in [0, 2^63).
        let values: BTreeSet<i64> = (0..1000)
            .map(|_| RandomNumberGenerator.generate_int63())
            .collect();
        check(values.iter().all(|&v| v >= 0), "non-negative")?;
        check(values.len() > 990, "distinct values")
    }

    /// A generator that replays a fixed value, as Go tests mock `RandomGenerator`.
    struct Fixed(i64);

    impl RandomGenerator for Fixed {
        fn generate_int63(&self) -> i64 {
            self.0
        }

        fn intn(&self, n: i64) -> Option<i64> {
            (n > 0).then_some(self.0 % n)
        }
    }

    #[test]
    fn intn_is_in_range_and_rejects_go_panics() -> TestResult {
        // Go: rand.Intn is in [0, n) and panics for n <= 0.
        for _ in 0..1000 {
            let v = RandomNumberGenerator.intn(10).ok_or("intn(10)")?;
            check((0..10).contains(&v), "in [0, 10)")?;
        }
        check_eq(&RandomNumberGenerator.intn(0), &None, "Go panics for 0")?;
        check_eq(&RandomNumberGenerator.intn(-1), &None, "Go panics for -1")?;
        let mock: &dyn RandomGenerator = &Fixed(17);
        check_eq(&mock.intn(5), &Some(2), "a mock generator")?;
        check_eq(&mock.generate_int63(), &17, "a mock generator")
    }
}
