//! Go `util/timeutil`, the time layouts Prebid formats with, and the clock Go injects from
//! benbjohnson/clock.
//!
//! Instants are the time crate's [`OffsetDateTime`] and durations its [`SignedDuration`], signed
//! as Go's are. [`format_rfc3339`] and [`format_rfc3339_nano`] write Go's `time.RFC3339`
//! and `time.RFC3339Nano` layouts with format descriptions (the time crate's well-known
//! `Rfc3339` writes the fraction whenever there is one). [`Clock`] is the part of
//! benbjohnson/clock's `Clock` that Prebid's analytics modules call: [`RealClock`] is its
//! `clock.New()`, on tokio's timers, and [`MockClock`] its `clock.NewMock()`, the manual clock
//! of their tests.
//!
//! ```
//! use pbs_util::timeutil::{format_rfc3339, format_rfc3339_nano};
//! use time::macros::datetime;
//!
//! // Go: stored_requests/events/http/http.go:127 writes whole seconds.
//! let t = datetime!(2026-11-18 10:20:30.120 UTC);
//! assert_eq!(format_rfc3339(t), "2026-11-18T10:20:30Z");
//! assert_eq!(format_rfc3339_nano(t), "2026-11-18T10:20:30.12Z");
//! ```

use std::{
    fmt, future,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    task::{Context, Poll, Waker},
    time::Duration as StdDuration,
};

use time::{
    OffsetDateTime, SignedDuration, format_description::BorrowedFormatItem,
    macros::format_description,
};
use tokio::{
    runtime::Handle,
    sync::mpsc,
    task::yield_now,
    time::{Instant, Interval, MissedTickBehavior, interval_at},
};

/// Go: `timeutil.Time`, the clock that code under test replaces.
pub trait Time: Send + Sync {
    /// Go: `Now`.
    fn now(&self) -> OffsetDateTime;
}

/// Go: `timeutil.RealTime`: the system clock (Go: `time.Now()`), in UTC.
///
/// Go's `time.Now` is in the local zone and carries a monotonic reading that `Sub` and
/// comparisons use; this is the UTC wall clock (the time.now-wall-clock ledger entry).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RealTime;

impl Time for RealTime {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

impl<T: Time + ?Sized> Time for &T {
    fn now(&self) -> OffsetDateTime {
        (**self).now()
    }
}

/// Go's `2006-01-02T15:04:05`, the date and time of both layouts.
const DATE_TIME: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");

/// Go's `.999999999`: the fraction without its trailing zeros, written only when it is not
/// zero.
const FRACTION: &[BorrowedFormatItem<'_>] = format_description!(".[subsecond digits:1+]");

/// Go's `Z07:00` for a non-zero offset of at least a minute.
const OFFSET: &[BorrowedFormatItem<'_>] =
    format_description!("[offset_hour sign:mandatory]:[offset_minute]");

/// Go: `t.Format(time.RFC3339)`: `2006-01-02T15:04:05Z07:00`, whole seconds and `Z` for UTC.
#[must_use]
pub fn format_rfc3339(t: OffsetDateTime) -> String {
    let mut out = t.format(DATE_TIME).unwrap_or_default();
    push_offset(&mut out, t);
    out
}

/// Go: `t.Format(time.RFC3339Nano)`: `2006-01-02T15:04:05.999999999Z07:00`, the fraction
/// without trailing zeros and none at all for a whole second.
#[must_use]
pub fn format_rfc3339_nano(t: OffsetDateTime) -> String {
    let mut out = t.format(DATE_TIME).unwrap_or_default();
    if t.nanosecond() != 0 {
        out.push_str(&t.format(FRACTION).unwrap_or_default());
    }
    push_offset(&mut out, t);
    out
}

/// Go's `Z07:00`: `Z` for a zero offset, else the offset's hours and minutes, the minutes cut
/// toward zero, so that an offset under a minute east or west is `+00:00`.
fn push_offset(out: &mut String, t: OffsetDateTime) {
    let offset = t.offset();
    if offset.is_utc() {
        out.push('Z');
    } else if offset.whole_minutes() == 0 {
        out.push_str("+00:00");
    } else {
        out.push_str(&t.format(OFFSET).unwrap_or_default());
    }
}

/// Go: benbjohnson/clock's `Clock`, as far as Prebid calls it (`Now` and `Ticker`).
pub trait Clock: Send + Sync + fmt::Debug {
    /// Go: `Now`.
    fn now(&self) -> OffsetDateTime;

    /// Go: `Ticker(d)`: a ticker that ticks every `d`, its first tick `d` from now.
    ///
    /// Go's real ticker panics for a `d` that is not positive (`time.NewTicker`) and its mock
    /// ticks forever within one `Add`; such a ticker never ticks here (the
    /// time.ticker-non-positive ledger entry).
    fn ticker(&self, d: SignedDuration) -> Ticker;
}

/// Go: `clock.New()`: the system clock and tokio's timers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn ticker(&self, d: SignedDuration) -> Ticker {
        Ticker(TickerKind::Real(RealTicker {
            period: positive(d),
            start: Instant::now(),
            interval: None,
        }))
    }
}

/// A positive duration as a std duration (Go: `d > 0`).
fn positive(d: SignedDuration) -> Option<StdDuration> {
    StdDuration::try_from(d).ok().filter(|d| !d.is_zero())
}

/// Go: `clock.Ticker`: [`tick`](Self::tick) is a receive from Go's `C`.
#[derive(Debug)]
pub struct Ticker(TickerKind);

#[derive(Debug)]
enum TickerKind {
    Real(RealTicker),
    Mock(MockTicker),
}

/// A `time.Ticker` on tokio: an interval that starts when the ticker is made and drops the
/// ticks a slow receiver misses (Go's one-tick channel), made at the first receive, where a
/// runtime is at hand.
#[derive(Debug)]
struct RealTicker {
    /// `None` once stopped, or for a period that is not positive.
    period: Option<StdDuration>,
    start: Instant,
    interval: Option<Interval>,
}

impl RealTicker {
    /// The interval, made on first use; `None` when the ticker never ticks or no tokio
    /// runtime runs.
    fn interval(&mut self) -> Option<&mut Interval> {
        if self.interval.is_none() {
            let period = self.period?;
            let first = self.start.checked_add(period)?;
            if Handle::try_current().is_err() {
                return None;
            }
            let mut interval = interval_at(first, period);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            self.interval = Some(interval);
        }
        self.interval.as_mut()
    }
}

#[derive(Debug)]
struct MockTicker {
    id: u64,
    clock: MockClock,
    rx: mpsc::Receiver<OffsetDateTime>,
    /// Keeps the channel open once the ticker is stopped, as Go's `C` stays open.
    _tx: mpsc::Sender<OffsetDateTime>,
}

impl Ticker {
    /// Go: `<-t.C`: waits for the next tick and returns its time. A stopped ticker, or one
    /// that never ticks, waits forever, as a receive from Go's `C` does.
    ///
    /// It is cancel-safe, so it can sit in a `tokio::select!` as `<-t.C` sits in Go's
    /// `select`.
    pub async fn tick(&mut self) -> OffsetDateTime {
        match &mut self.0 {
            TickerKind::Real(real) => match real.interval() {
                Some(interval) => {
                    let _deadline = interval.tick().await;
                    OffsetDateTime::now_utc()
                }
                None => future::pending().await,
            },
            TickerKind::Mock(mock) => match mock.rx.recv().await {
                Some(t) => t,
                None => future::pending().await,
            },
        }
    }

    /// Go: a `select` on `t.C` with a `default`: the tick waiting to be received, if any.
    pub fn try_tick(&mut self) -> Option<OffsetDateTime> {
        match &mut self.0 {
            TickerKind::Real(real) => {
                let interval = real.interval()?;
                let mut cx = Context::from_waker(Waker::noop());
                match interval.poll_tick(&mut cx) {
                    Poll::Ready(_) => Some(OffsetDateTime::now_utc()),
                    Poll::Pending => None,
                }
            }
            TickerKind::Mock(mock) => mock.rx.try_recv().ok(),
        }
    }

    /// Go: `Stop`: no more ticks; one already waiting in Go's `C` stays there.
    pub fn stop(&mut self) {
        match &mut self.0 {
            TickerKind::Real(real) => {
                real.period = None;
                real.interval = None;
            }
            TickerKind::Mock(mock) => {
                let id = mock.id;
                mock.clock.lock().timers.retain(|timer| timer.id != id);
            }
        }
    }

    /// Go: `Reset(d)`: the next tick `d` from now, then every `d`.
    ///
    /// As in benbjohnson/clock, the mock's `Reset` does not restart a stopped ticker.
    pub fn reset(&mut self, d: SignedDuration) {
        match &mut self.0 {
            TickerKind::Real(real) => {
                real.period = positive(d);
                real.start = Instant::now();
                real.interval = None;
            }
            TickerKind::Mock(mock) => {
                let id = mock.id;
                let mut state = mock.clock.lock();
                let next = next_tick(state.now, d);
                if let Some(timer) = state.timers.iter_mut().find(|timer| timer.id == id) {
                    timer.period = d;
                    timer.next = next;
                }
            }
        }
    }
}

/// Go: `now.Add(d)`, the next tick of a period `d`; `None`, never, for a period that is not
/// positive or a time past the time crate's range.
const fn next_tick(now: OffsetDateTime, d: SignedDuration) -> Option<OffsetDateTime> {
    if d.is_positive() {
        now.checked_add(d)
    } else {
        None
    }
}

/// Go: `clock.NewMock()`: a clock that stands at the Unix epoch and moves only when told to,
/// ticking its tickers in time order as it passes their ticks.
///
/// Clones share the clock.
#[derive(Clone, Debug)]
pub struct MockClock(Arc<Mutex<MockState>>);

#[derive(Debug)]
struct MockState {
    now: OffsetDateTime,
    /// The running tickers in creation order, which a stable sort by next tick keeps for
    /// ticks due together (Go's `sort.Sort` over a dozen timers or fewer).
    timers: Vec<MockTimer>,
    next_id: u64,
}

#[derive(Debug)]
struct MockTimer {
    id: u64,
    next: Option<OffsetDateTime>,
    period: SignedDuration,
    tx: mpsc::Sender<OffsetDateTime>,
}

/// How often [`MockClock`] yields to the runtime where Go's mock sleeps a millisecond so that
/// other goroutines run (Go: `gosched`).
const GOSCHED_YIELDS: usize = 4;

/// Go: `gosched`: lets the tasks a tick woke run. On a current-thread runtime, as
/// `#[tokio::test]` builds, each yield runs every other ready task first.
async fn gosched() {
    for _ in 0..GOSCHED_YIELDS {
        yield_now().await;
    }
}

impl Default for MockClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MockClock {
    /// Go: `clock.NewMock()`.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(MockState {
            now: OffsetDateTime::UNIX_EPOCH,
            timers: Vec::new(),
            next_id: 0,
        })))
    }

    fn lock(&self) -> MutexGuard<'_, MockState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Go: `Add(d)`: moves the clock forward by `d`, ticking each ticker whose tick it passes,
    /// in time order. A time past the time crate's range leaves the clock where it is.
    pub async fn add(&self, d: SignedDuration) {
        let target = self.lock().now.checked_add(d);
        if let Some(target) = target {
            self.set(target).await;
        }
    }

    /// Go: `Set(t)`: moves the clock to `t`, ticking each ticker whose tick falls before it,
    /// in time order. A `t` before now moves the clock back without ticking.
    pub async fn set(&self, t: OffsetDateTime) {
        while self.run_next_timer(t) {
            gosched().await;
        }
        self.lock().now = t;
        gosched().await;
    }

    /// Go: `runNextTimer`: ticks the ticker due first, if its tick is no later than `max`,
    /// and moves the clock to that tick. A tick finding the channel full is dropped (Go's
    /// `select` with a `default`).
    fn run_next_timer(&self, max: OffsetDateTime) -> bool {
        let mut state = self.lock();
        state
            .timers
            .sort_by_key(|timer| (timer.next.is_none(), timer.next));
        let Some(timer) = state.timers.first_mut() else {
            return false;
        };
        let Some(next) = timer.next.filter(|next| *next <= max) else {
            return false;
        };
        let _delivered = timer.tx.try_send(next).is_ok();
        timer.next = next_tick(next, timer.period);
        state.now = next;
        true
    }
}

impl Clock for MockClock {
    fn now(&self) -> OffsetDateTime {
        self.lock().now
    }

    fn ticker(&self, d: SignedDuration) -> Ticker {
        let (tx, rx) = mpsc::channel(1);
        let mut state = self.lock();
        let id = state.next_id;
        state.next_id += 1;
        let next = next_tick(state.now, d);
        state.timers.push(MockTimer {
            id,
            next,
            period: d,
            tx: tx.clone(),
        });
        drop(state);
        Ticker(TickerKind::Mock(MockTicker {
            id,
            clock: self.clone(),
            rx,
            _tx: tx,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use time::{OffsetDateTime, SignedDuration};

    use super::{Clock as _, MockClock, RealTime, Time};
    use crate::golden::{TestResult, check, check_eq};

    /// A fixed clock, as Go tests mock `timeutil.Time`.
    struct Fixed(OffsetDateTime);

    impl Time for Fixed {
        fn now(&self) -> OffsetDateTime {
            self.0
        }
    }

    #[test]
    fn real_time_is_the_system_clock() -> TestResult {
        // Go: timeutil.RealTime.Now is time.Now.
        let before = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
        let now = RealTime.now().unix_timestamp();
        let after = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
        check(before <= now && now <= after, "RealTime.Now is time.Now")
    }

    #[test]
    fn a_mock_clock_is_injectable() -> TestResult {
        // Go: the tests' timeutil.Time mocks.
        let clock: &dyn Time = &Fixed(OffsetDateTime::from_unix_timestamp(1_700_000_000)?);
        check(
            clock.now().unix_timestamp() == 1_700_000_000,
            "the mock time",
        )
    }

    #[tokio::test]
    async fn mock_ticks_drop_while_the_channel_is_full() -> TestResult {
        // Go: benbjohnson/clock v1.3.0 clock.go:363-370, a full C drops the tick.
        let clock = MockClock::new();
        let mut ticker = clock.ticker(SignedDuration::SECOND);
        clock.add(SignedDuration::seconds(3)).await;
        check_eq(
            &ticker.try_tick().map(OffsetDateTime::unix_timestamp),
            &Some(1),
            "the first tick waits in C",
        )?;
        check_eq(&ticker.try_tick(), &None, "the next two were dropped")?;
        check_eq(&clock.now().unix_timestamp(), &3, "Add ends at its target")
    }
}
