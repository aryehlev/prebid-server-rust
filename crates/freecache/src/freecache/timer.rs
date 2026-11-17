//! Port of freecache `timer.go`: the clock that stamps access and expiration times.

use std::{
    rc::Rc,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use super::convert::low_u32;

/// Go: `Timer`, the source of the current time in seconds.
///
/// freecache stores every timestamp as a `uint32` of Unix seconds, so a timer returns the low
/// 32 bits of the Unix time. Tests use a fake timer to drive expiration deterministically.
pub trait Timer {
    /// Go: `Now`, the current time in seconds.
    fn now(&self) -> u32;
}

/// Go: `defaultTimer`, which reads the system clock on every call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DefaultTimer;

impl Timer for DefaultTimer {
    fn now(&self) -> u32 {
        get_unix_time()
    }
}

impl<T: Timer + ?Sized> Timer for &T {
    fn now(&self) -> u32 {
        (**self).now()
    }
}

impl<T: Timer + ?Sized> Timer for Box<T> {
    fn now(&self) -> u32 {
        (**self).now()
    }
}

impl<T: Timer + ?Sized> Timer for Rc<T> {
    fn now(&self) -> u32 {
        (**self).now()
    }
}

impl<T: Timer + ?Sized> Timer for Arc<T> {
    fn now(&self) -> u32 {
        (**self).now()
    }
}

/// Go: `getUnixTime`, `uint32(time.Now().Unix())`.
///
/// `Unix` floors toward negative infinity for instants before the epoch, and the conversion
/// keeps the low 32 bits.
fn get_unix_time() -> u32 {
    let seconds = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => {
            let before = before.duration();
            let whole = i64::try_from(before.as_secs()).unwrap_or(i64::MAX);
            if before.subsec_nanos() > 0 {
                -whole - 1
            } else {
                -whole
            }
        }
    };
    low_u32(seconds)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        super::golden::{TestResult, check},
        DefaultTimer, Timer as _,
    };

    #[test]
    fn default_timer_reads_unix_seconds() -> TestResult {
        let before = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let now = u64::from(DefaultTimer.now());
        let after = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        check(
            (before & 0xFFFF_FFFF) <= now && now <= (after & 0xFFFF_FFFF),
            &format!("DefaultTimer.now() = {now}, want between {before} and {after}"),
        )
    }
}
