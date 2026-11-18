//! Go `util/timeutil`: an injectable clock.

use pbs_gocompat::time::Time as GoTime;

/// Go: `timeutil.Time`, the clock that code under test replaces.
pub trait Time: Send + Sync {
    /// Go: `Now`.
    fn now(&self) -> GoTime;
}

/// Go: `timeutil.RealTime`: the system clock (Go: `time.Now()`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RealTime;

impl Time for RealTime {
    fn now(&self) -> GoTime {
        GoTime::now()
    }
}

impl<T: Time + ?Sized> Time for &T {
    fn now(&self) -> GoTime {
        (**self).now()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use pbs_gocompat::time::Time as GoTime;

    use super::{RealTime, Time};
    use crate::golden::{TestResult, check};

    /// A fixed clock, as Go tests mock `timeutil.Time`.
    struct Fixed(GoTime);

    impl Time for Fixed {
        fn now(&self) -> GoTime {
            self.0.clone()
        }
    }

    #[test]
    fn real_time_is_the_system_clock() -> TestResult {
        let before = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
        let now = RealTime.now().unix();
        let after = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
        check(before <= now && now <= after, "RealTime.Now is time.Now")
    }

    #[test]
    fn a_mock_clock_is_injectable() -> TestResult {
        let clock: &dyn Time = &Fixed(GoTime::from_unix(1_700_000_000, 0));
        check(clock.now().unix() == 1_700_000_000, "the mock time")
    }
}
