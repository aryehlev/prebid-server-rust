//! Go: `util/task/ticker_task.go`.

use std::{error, fmt, future, sync::Arc, time::Duration as StdDuration};

use pbs_gocompat::time::Duration;
use tokio::{
    runtime::Handle,
    sync::watch,
    time::{self, Instant, MissedTickBehavior},
};

/// Go: `task.Runner`: the work a [`TickerTask`] runs.
///
/// Go's `Run` blocks and returns an `error`, which `TickerTask` discards; here `run` is
/// asynchronous and its error is discarded the same way, so a runner reports its own failures.
pub trait Runner: Send + Sync + 'static {
    /// The error of a run (Go: the `error` result of `Run`).
    type Error: Send + 'static;

    /// Go: `Run`.
    fn run(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// The error of [`TickerTask::start`] outside a tokio runtime, where the periodic runs cannot
/// be scheduled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoRuntimeError(String);

impl fmt::Display for NoRuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "task: cannot schedule periodic runs: {}", self.0)
    }
}

impl error::Error for NoRuntimeError {}

/// Go: `task.TickerTask`: runs a [`Runner`] once on [`start`](Self::start), then every
/// `interval` until [`stop`](Self::stop).
///
/// The timing is Go's `time.Ticker`: the first periodic run is one interval after the first
/// run ends, and when a run outlasts one or more ticks the missed ticks are dropped, so the
/// next run starts as soon as the slow one ends and the run after that on the next tick of the
/// original schedule.
///
/// Differences from Go, none observable in Prebid's use:
///
/// - When a tick and [`stop`](Self::stop) are both pending, Go's `select` picks one at random, so
///   it may run once more; this port always stops.
/// - Go panics when `Stop` is called twice (it closes a closed channel); here `stop` is idempotent.
/// - For intervals of 5 ms or less, tokio catches up a tick missed by at most 5 ms instead of
///   dropping it.
#[derive(Debug)]
pub struct TickerTask<R> {
    interval: Duration,
    runner: Arc<R>,
    /// Go: the `done` channel; it holds `true` once [`stop`](Self::stop) has closed it.
    done: watch::Sender<bool>,
}

impl<R: Runner> TickerTask<R> {
    /// Go: `task.NewTickerTask(interval, runner)`.
    #[must_use]
    pub fn new(interval: Duration, runner: Arc<R>) -> Self {
        Self {
            interval,
            runner,
            done: watch::Sender::new(false),
        }
    }

    /// The interval between runs.
    #[must_use]
    pub const fn interval(&self) -> Duration {
        self.interval
    }

    /// The runner.
    #[must_use]
    pub const fn runner(&self) -> &Arc<R> {
        &self.runner
    }

    /// Go: `Start`: runs the runner and waits for it, then, for a positive interval, spawns
    /// the periodic runs on the current tokio runtime. Like Go, the result of each run is
    /// discarded.
    ///
    /// # Errors
    ///
    /// [`NoRuntimeError`] when awaited outside a tokio runtime with a positive interval; the
    /// first run has happened, but no periodic run is scheduled.
    pub async fn start(&self) -> Result<(), NoRuntimeError> {
        let _result = self.runner.run().await;
        let Some(interval) = positive(self.interval) else {
            return Ok(());
        };
        let handle = Handle::try_current().map_err(|e| NoRuntimeError(e.to_string()))?;
        drop(handle.spawn(run_recurring(
            interval,
            Arc::clone(&self.runner),
            self.done.subscribe(),
        )));
        Ok(())
    }

    /// Go: `Stop`: ends the periodic runs once any run in progress returns; the runner keeps
    /// its state.
    ///
    /// Like closing Go's `done` channel, this reaches every periodic run the task has started
    /// and is permanent, so a later [`start`](Self::start) only runs once. Dropping the task
    /// without stopping it stops nothing, as in Go, where the channel then never closes.
    pub fn stop(&self) {
        let _was_stopped = self.done.send_replace(true);
    }
}

/// A positive Go duration as a std duration (Go: `interval > 0`).
fn positive(d: Duration) -> Option<StdDuration> {
    u64::try_from(d.0)
        .ok()
        .filter(|&n| n > 0)
        .map(StdDuration::from_nanos)
}

/// Resolves once [`TickerTask::stop`] has been called (Go: `case <-t.done`).
async fn stopped(done: &mut watch::Receiver<bool>) {
    if done.wait_for(|stopped| *stopped).await.is_err() {
        // The task was dropped without Stop: Go's done channel is never closed.
        future::pending::<()>().await;
    }
}

/// Go: `runRecurring`: a `time.Ticker` whose first tick is one interval after the start and
/// which drops the ticks a slow run misses.
async fn run_recurring<R: Runner>(
    interval: StdDuration,
    runner: Arc<R>,
    mut done: watch::Receiver<bool>,
) {
    let Some(first) = Instant::now().checked_add(interval) else {
        // The first tick is too far away to schedule, so, like Go's ticker within the life of
        // a process, it never fires.
        stopped(&mut done).await;
        return;
    };
    let mut ticker = time::interval_at(first, interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            () = stopped(&mut done) => return,
            _tick = ticker.tick() => {
                let _result = runner.run().await;
            }
        }
    }
}
