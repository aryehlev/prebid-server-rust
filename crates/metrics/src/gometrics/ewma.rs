//! Go: `ewma.go`.

use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering},
};

use crate::float_from_i64;

/// Go: `1 - math.Exp(-5.0/60.0/1)`, the alpha of `NewEWMA1`, as Go computes it (the bits are
/// pinned by `testdata/golden/gometrics/ewma.txt`, so the port does not depend on the platform's
/// `exp`).
pub(super) const M1_ALPHA: f64 = f64::from_bits(0x3FB4_77F8_204C_B218);
/// Go: `1 - math.Exp(-5.0/60.0/5)`, the alpha of `NewEWMA5`.
pub(super) const M5_ALPHA: f64 = f64::from_bits(0x3F90_ECDB_F525_74E0);
/// Go: `1 - math.Exp(-5.0/60.0/15)`, the alpha of `NewEWMA15`.
pub(super) const M15_ALPHA: f64 = f64::from_bits(0x3F76_B145_3A24_3300);

/// Go: `metrics.StandardEWMA`: an exponentially-weighted moving average of the events counted
/// between clock ticks, each tick assumed to come five seconds after the last.
#[derive(Debug)]
pub struct StandardEwma {
    uncounted: AtomicI64,
    alpha: f64,
    rate: AtomicU64,
    init: AtomicU32,
    mutex: Mutex<()>,
}

impl StandardEwma {
    /// Go: `Rate`, the moving average rate of events per second.
    #[must_use]
    pub fn rate(&self) -> f64 {
        f64::from_bits(self.rate.load(Ordering::SeqCst)) * 1e9
    }

    /// Go: `Snapshot`.
    #[must_use]
    pub fn snapshot(&self) -> EwmaSnapshot {
        EwmaSnapshot(self.rate())
    }

    /// Go: `Tick`: folds the events counted since the last tick into the rate; the first tick
    /// sets the rate to them.
    pub fn tick(&self) {
        // Go: an atomic check first, then a re-check under the mutex for the first tick.
        if self.init.load(Ordering::SeqCst) == 1 {
            self.update_rate(self.fetch_instant_rate());
        } else {
            let _guard = self.mutex.lock().unwrap_or_else(PoisonError::into_inner);
            if self.init.load(Ordering::SeqCst) == 1 {
                self.update_rate(self.fetch_instant_rate());
            } else {
                self.init.store(1, Ordering::SeqCst);
                self.rate
                    .store(self.fetch_instant_rate().to_bits(), Ordering::SeqCst);
            }
        }
    }

    /// Go: `fetchInstantRate`: takes the uncounted events, as events per nanosecond over five
    /// seconds.
    fn fetch_instant_rate(&self) -> f64 {
        let count = self.uncounted.load(Ordering::SeqCst);
        let _previous = self
            .uncounted
            .fetch_add(count.wrapping_neg(), Ordering::SeqCst);
        float_from_i64(count) / 5e9
    }

    /// Go: `updateRate`: `currentRate += a.alpha * (instantRate - currentRate)`, unfused as
    /// Go's amd64 build computes it.
    fn update_rate(&self, instant_rate: f64) {
        let current_rate = f64::from_bits(self.rate.load(Ordering::SeqCst));
        let step = self.alpha * (instant_rate - current_rate);
        let updated = current_rate + step;
        self.rate.store(updated.to_bits(), Ordering::SeqCst);
    }

    /// Go: `Update`: adds `n` uncounted events (wrapping like Go's `atomic.AddInt64`).
    pub fn update(&self, n: i64) {
        let _previous = self.uncounted.fetch_add(n, Ordering::SeqCst);
    }
}

/// Go: `metrics.EWMASnapshot`, a read-only copy of an EWMA.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EwmaSnapshot(f64);

impl EwmaSnapshot {
    /// Go: `Rate`, the rate when the snapshot was taken.
    #[must_use]
    pub const fn rate(self) -> f64 {
        self.0
    }
}

/// Go: `metrics.NewEWMA(alpha)`.
#[must_use]
pub const fn new_ewma(alpha: f64) -> StandardEwma {
    StandardEwma {
        uncounted: AtomicI64::new(0),
        alpha,
        rate: AtomicU64::new(0),
        init: AtomicU32::new(0),
        mutex: Mutex::new(()),
    }
}

/// Go: `metrics.NewEWMA1()`, a one-minute moving average.
#[must_use]
pub const fn new_ewma1() -> StandardEwma {
    new_ewma(M1_ALPHA)
}

/// Go: `metrics.NewEWMA5()`, a five-minute moving average.
#[must_use]
pub const fn new_ewma5() -> StandardEwma {
    new_ewma(M5_ALPHA)
}

/// Go: `metrics.NewEWMA15()`, a fifteen-minute moving average.
#[must_use]
pub const fn new_ewma15() -> StandardEwma {
    new_ewma(M15_ALPHA)
}
