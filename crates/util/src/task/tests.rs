//! Ports of `util/task/*_test.go`; `tests/task.rs` replays the synctest timing golden.

use std::{
    convert::Infallible,
    future,
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration as StdDuration,
};

use time::SignedDuration;
use tokio::{task, time::sleep};

use super::{Runner, TickerTask, ticker_task_from_func};
use crate::golden::{TestResult, check, check_eq};

/// Go: `MockRunner`: counts its runs.
#[derive(Debug, Default)]
struct MockRunner(AtomicUsize);

impl MockRunner {
    fn run_count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

impl Runner for MockRunner {
    type Error = Infallible;

    fn run(&self) -> impl Future<Output = Result<(), Infallible>> + Send {
        let _previous = self.0.fetch_add(1, Ordering::SeqCst);
        future::ready(Ok(()))
    }
}

async fn settle() {
    for _ in 0..10 {
        task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn start_with_single_run() -> TestResult {
    // Go: TestStartWithSingleRun: a zero interval runs once.
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(SignedDuration::ZERO, Arc::clone(&runner));
    ticker.start().await?;
    check_eq(&runner.run_count(), &1, "the run of Start")?;
    sleep(StdDuration::from_millis(50)).await;
    settle().await;
    check_eq(&runner.run_count(), &1, "no periodic runs")
}

#[tokio::test(start_paused = true)]
async fn start_with_periodic_run() -> TestResult {
    // Go: TestStartWithPeriodicRun: one run, then one every 10ms until Stop.
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(SignedDuration::milliseconds(10), Arc::clone(&runner));
    ticker.start().await?;
    sleep(StdDuration::from_millis(10)).await;
    settle().await;
    check_eq(&runner.run_count(), &2, "the run of Start and one tick")?;
    ticker.stop();
    sleep(StdDuration::from_millis(50)).await;
    settle().await;
    check_eq(
        &runner.run_count(),
        &2,
        "runner should not run after Stop is called",
    )
}

#[tokio::test]
async fn new_ticker_task_from_func() -> TestResult {
    // Go: TestNewTickerTaskFromFunc: the function is the runner.
    let count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&count);
    let ticker = ticker_task_from_func(SignedDuration::HOUR, move || {
        let counter = Arc::clone(&counter);
        async move {
            let _previous = counter.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Infallible>(())
        }
    });
    check_eq(&ticker.runner().run().await, &Ok(()), "Run")?;
    check_eq(&count.load(Ordering::SeqCst), &1, "one call")?;
    check_eq(&ticker.interval(), &SignedDuration::HOUR, "interval")
}

#[tokio::test(start_paused = true)]
async fn negative_interval_runs_once() -> TestResult {
    // Go: `if t.interval > 0`: a negative time.Duration schedules nothing.
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(SignedDuration::nanoseconds(-1), Arc::clone(&runner));
    ticker.start().await?;
    sleep(StdDuration::from_secs(3600)).await;
    settle().await;
    check_eq(&runner.run_count(), &1, "no periodic runs")
}

#[tokio::test(start_paused = true)]
async fn stop_is_permanent_and_reaches_every_start() -> TestResult {
    // Go: each Start runs once and starts a goroutine, and Stop closes the one done channel
    // they all select on (util/task/ticker_task.go:27-52).
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(SignedDuration::SECOND, Arc::clone(&runner));
    ticker.start().await?;
    ticker.start().await?;
    sleep(StdDuration::from_secs(1)).await;
    settle().await;
    check_eq(&runner.run_count(), &4, "two starts, one tick each")?;
    ticker.stop();
    ticker.stop();
    sleep(StdDuration::from_secs(10)).await;
    settle().await;
    check_eq(&runner.run_count(), &4, "Stop ends both")?;
    ticker.start().await?;
    sleep(StdDuration::from_secs(10)).await;
    settle().await;
    check_eq(&runner.run_count(), &5, "a Start after Stop only runs once")
}

#[tokio::test(start_paused = true)]
async fn dropping_the_task_does_not_stop_it() -> TestResult {
    // Go: the goroutine Start starts keeps the task until Stop
    // (util/task/ticker_task.go:31, 42-52), so dropping the caller's pointer stops nothing.
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(SignedDuration::SECOND, Arc::clone(&runner));
    ticker.start().await?;
    drop(ticker);
    sleep(StdDuration::from_secs(3)).await;
    settle().await;
    check_eq(
        &runner.run_count(),
        &4,
        "the periodic runs outlive the task",
    )
}

#[tokio::test(start_paused = true)]
async fn longest_interval_never_ticks() -> TestResult {
    // Go: time.NewTicker(math.MaxInt64) first ticks 292 years on, so only the run of Start
    // happens (util/task/ticker_task.go:28-31).
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(SignedDuration::nanoseconds(i64::MAX), Arc::clone(&runner));
    ticker.start().await?;
    sleep(StdDuration::from_hours(24 * 365)).await;
    settle().await;
    check_eq(&runner.run_count(), &1, "only the run of Start")?;
    ticker.stop();
    settle().await;
    check_eq(&runner.run_count(), &1, "still one run after Stop")
}

#[test]
fn start_outside_a_runtime_runs_once_then_fails() -> TestResult {
    // Go: Start runs the runner before it schedules anything, and a zero interval schedules
    // nothing (util/task/ticker_task.go:27-33).
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(SignedDuration::SECOND, Arc::clone(&runner));
    let mut start = pin!(ticker.start());
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(result) = start.as_mut().poll(&mut cx) else {
        return Err("start did not complete".into());
    };
    check_eq(&runner.run_count(), &1, "the first run happened")?;
    check(result.is_err(), "no periodic runs without a runtime")?;
    let zero = TickerTask::new(SignedDuration::ZERO, Arc::clone(&runner));
    let mut start_once = pin!(zero.start());
    check(
        matches!(start_once.as_mut().poll(&mut cx), Poll::Ready(Ok(()))),
        "a zero interval needs no runtime",
    )
}
