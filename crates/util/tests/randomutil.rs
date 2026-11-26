//! Go's `util/randomutil` against `pbs_util::randomutil`: the bounds of its numbers, which are
//! all Prebid relies on. Go draws from the process-wide `math/rand` source, which Go seeds
//! randomly at start-up; rand's sequences are different ones (the rand.sequences ledger entry).

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, error::Error};

    use pbs_util::randomutil::{RandomGenerator as _, RandomNumberGenerator};

    type TestResult = Result<(), Box<dyn Error>>;

    fn check(ok: bool, what: &str) -> TestResult {
        if ok { Ok(()) } else { Err(what.into()) }
    }

    #[test]
    fn bounds_only() -> TestResult {
        // Go: rand.Int63 returns a number in [0, 2^63) and rand.Intn(n) one in [0, n), and
        // Intn panics for an n that is not positive (util/randomutil/randomutil.go);
        // the sequences are math/rand's, which no Prebid output depends on.
        let rng = RandomNumberGenerator;
        let mut seen = BTreeSet::new();
        for _ in 0..50_000 {
            let v = rng.generate_int63();
            check(v >= 0, "Int63 is not negative")?;
            let _new = seen.insert(v);
        }
        check(seen.len() > 49_990, "Int63 is not a fixed sequence")?;
        for n in [1, 2, 3, 7, 100, 1_000_000, i64::MAX] {
            let mut drawn = BTreeSet::new();
            for _ in 0..2_000 {
                let v = rng.intn(n).ok_or("Intn(n) for a positive n")?;
                check((0..n).contains(&v), &format!("Intn({n}) is in [0, {n})"))?;
                let _new = drawn.insert(v);
            }
            let want = usize::try_from(n.min(3))?;
            check(
                drawn.len() >= want,
                &format!("Intn({n}) draws several values"),
            )?;
        }
        for n in [0, -1, i64::MIN] {
            check(
                rng.intn(n).is_none(),
                &format!("Intn({n}), where Go panics"),
            )?;
        }
        Ok(())
    }
}
