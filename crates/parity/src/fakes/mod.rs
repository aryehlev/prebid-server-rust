//! Deterministic clock, random and UUID sources, and the map orders of parity mode
//! (Go: `parity/fakes`, `fakes.json`).
//!
//! Parity mode injects these into the adapters with internal nondeterminism (adnuntius,
//! appnexus, `cadent_aperture_mx`, eplanning, flipp, huaweiads, tappx, yieldlab), draws every
//! generated v4 UUID from a deterministic stream ([`new_v4`], [`ParityUuidGenerator`]), and
//! visits the maps whose iteration order Go leaves to chance in the order the contract names
//! ([`map_keys`], [`OrderSite`]). The values are part of the cross-language contract: the Rust
//! server injects the same values at the same sites.
//!
//! Nothing is active until the parity bootstrap installs the fakes, which only a build with the
//! `trace` feature can do. Until then (and always without the feature, where [`active`] is a
//! `const fn` returning `None`) every helper returns exactly what the original code used: the
//! system clock, the process random source, random UUIDs and the map's own iteration order.
//!
//! [`Source`] implements `pbs-util`'s [`Time`], [`RandomGenerator`] and [`UuidGenerator`], so an
//! adapter that already takes those seams can be handed the installed source directly.

mod order;
mod uuids;

#[cfg(feature = "trace")]
use core::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "trace")]
use std::sync::OnceLock;
use std::sync::{Mutex, MutexGuard, PoisonError};

use pbs_gocompat::time::Time as GoTime;
use pbs_util::{
    randomutil::{RandomGenerator, RandomNumberGenerator},
    timeutil::Time,
    uuidutil::{UuidError, UuidGenerator},
};

pub use self::{
    order::{OrderSite, map_keys},
    uuids::{ParityUuidGenerator, new_v4, sequence_uuid},
};
use crate::envelope::FakeValues;

/// The default fixed instant, 2021-08-20T15:28:50.823Z (Go: `fakes.DefaultNowUnixMS`): it
/// reproduces the huaweiads fixture nonce and falls in ISO week 33 like the yieldlab test week
/// generator.
pub const DEFAULT_NOW_UNIX_MS: i64 = 1_629_473_330_823;
/// The default fixed 63-bit value (Go: `fakes.DefaultInt63`), appnexus'
/// `FakeRandomNumberGenerator`.
pub const DEFAULT_INT63: i64 = 10;
/// The default fixed UUID and UUID-stream seed (Go: `fakes.DefaultUUID`), flipp's test UUID.
pub const DEFAULT_UUID: &str = "30470a14-2949-4110-abce-b62d57304ad5";

/// The `fakes.json` default values (Go: `fakes.Defaults`).
#[must_use]
pub fn defaults() -> FakeValues {
    FakeValues {
        now_unix_ms: DEFAULT_NOW_UNIX_MS,
        int63: DEFAULT_INT63,
        uuid: DEFAULT_UUID.to_owned(),
    }
}

/// Deterministic values (Go: `fakes.Source`). A new source holds the defaults.
#[derive(Debug)]
pub struct Source {
    state: Mutex<SourceState>,
}

#[derive(Debug)]
struct SourceState {
    values: FakeValues,
    /// The seed of the generated-UUID stream.
    seed: String,
    /// How many UUIDs the stream has generated.
    drawn: u64,
}

impl Default for Source {
    fn default() -> Self {
        Self::new()
    }
}

impl Source {
    /// A source holding the defaults, with a fresh UUID stream.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(SourceState {
                values: defaults(),
                seed: DEFAULT_UUID.to_owned(),
                drawn: 0,
            }),
        }
    }

    /// Replaces the values; zero fields (or `None`) take the defaults (Go: `Source.Set`). It
    /// also restarts the generated-UUID stream from the (new) `uuid` value. The exec handler
    /// calls it per case from `envelope.fakes`.
    pub fn set(&self, v: Option<&FakeValues>) {
        let mut merged = defaults();
        if let Some(v) = v {
            if v.now_unix_ms != 0 {
                merged.now_unix_ms = v.now_unix_ms;
            }
            if v.int63 != 0 {
                merged.int63 = v.int63;
            }
            if !v.uuid.is_empty() {
                merged.uuid.clone_from(&v.uuid);
            }
        }
        let mut state = self.state();
        state.seed.clone_from(&merged.uuid);
        state.drawn = 0;
        state.values = merged;
    }

    /// The current values (Go: `Source.Values`).
    #[must_use]
    pub fn values(&self) -> FakeValues {
        self.state().values.clone()
    }

    /// The fixed value as a Go `int` (Go: `Source.Int`).
    #[must_use]
    pub fn int(&self) -> isize {
        go_int(self.state().values.int63)
    }

    /// The next UUID of the deterministic stream (Go: `Source.NewV4`); concurrent callers get
    /// distinct UUIDs, in draw order.
    #[must_use]
    pub fn new_v4(&self) -> String {
        let (seed, n) = {
            let mut state = self.state();
            state.drawn += 1;
            (state.seed.clone(), state.drawn)
        };
        sequence_uuid(&seed, n)
    }

    fn state(&self) -> MutexGuard<'_, SourceState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Time for Source {
    /// The fixed instant in UTC (Go: `Source.Now`).
    fn now(&self) -> GoTime {
        GoTime::from_unix_milli(self.state().values.now_unix_ms).utc()
    }
}

impl RandomGenerator for Source {
    /// The fixed 63-bit value (Go: `Source.GenerateInt63`).
    fn generate_int63(&self) -> i64 {
        self.state().values.int63
    }

    /// The fixed value reduced modulo `n`, or 0 when `n <= 0` (Go: `Source.Intn`, which does
    /// not panic there).
    fn intn(&self, n: i64) -> Option<i64> {
        Some(if n <= 0 {
            0
        } else {
            self.state().values.int63 % n
        })
    }
}

impl UuidGenerator for Source {
    /// The fixed UUID (Go: `Source.Generate`).
    fn generate(&self) -> Result<String, UuidError> {
        Ok(self.state().values.uuid.clone())
    }
}

/// Whether the fakes are installed.
#[cfg(feature = "trace")]
static INSTALLED: AtomicBool = AtomicBool::new(false);
/// The process-wide source.
#[cfg(feature = "trace")]
static SOURCE: OnceLock<Source> = OnceLock::new();
/// Serializes `install`.
#[cfg(feature = "trace")]
static INSTALL: Mutex<()> = Mutex::new(());

/// Activates the deterministic sources process-wide and returns them (Go: `fakes.Install`).
/// Calling it again returns the installed source unchanged; installing after [`uninstall`]
/// starts from the defaults.
#[cfg(feature = "trace")]
pub fn install() -> &'static Source {
    let _serial = INSTALL.lock().unwrap_or_else(PoisonError::into_inner);
    let src = SOURCE.get_or_init(Source::new);
    if !INSTALLED.load(Ordering::Acquire) {
        src.set(None);
        INSTALLED.store(true, Ordering::Release);
    }
    src
}

/// Deactivates the deterministic sources (Go: `fakes.Uninstall`); for tests and shutdown.
#[cfg(feature = "trace")]
pub fn uninstall() {
    let _serial = INSTALL.lock().unwrap_or_else(PoisonError::into_inner);
    INSTALLED.store(false, Ordering::Release);
}

/// The installed source, or `None` when the fakes are not installed (Go: `fakes.Active`).
#[cfg(feature = "trace")]
#[must_use]
#[inline]
pub fn active() -> Option<&'static Source> {
    if INSTALLED.load(Ordering::Acquire) {
        SOURCE.get()
    } else {
        None
    }
}

/// Always `None`: a production build never installs the fakes (Go: `fakes.Active` without
/// `fakes.Install`).
#[cfg(not(feature = "trace"))]
#[must_use]
#[inline]
pub const fn active() -> Option<&'static Source> {
    None
}

/// The fixed instant when the fakes are installed, else the system clock (Go: `fakes.Now`).
#[must_use]
#[inline]
pub fn now() -> GoTime {
    active().map_or_else(GoTime::now, Time::now)
}

/// The fixed value when the fakes are installed, else a non-negative random Go `int`
/// (Go: `fakes.Int`, `rand.Int`).
#[must_use]
#[inline]
pub fn int() -> isize {
    active().map_or_else(
        || go_int(RandomNumberGenerator.generate_int63()),
        Source::int,
    )
}

/// Go's `int(v)` of an `int64` (both are 64-bit on every supported target).
fn go_int(v: i64) -> isize {
    isize::try_from(v).unwrap_or(isize::MAX)
}

#[cfg(test)]
#[path = "fakes_test.rs"]
mod tests;
