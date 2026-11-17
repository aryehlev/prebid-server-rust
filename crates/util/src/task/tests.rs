//! Ports of `util/task/*_test.go` and the replay of the synctest timing golden.

use std::{
    convert::Infallible,
    error::Error,
    future,
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration as StdDuration,
};

use pbs_gocompat::time::Duration;
use tokio::{
    runtime,
    sync::Mutex,
    task,
    time::{self, Instant},
};

use super::{Runner, TickerTask, ticker_task_from_func};
use crate::golden::{TestResult, check, check_eq, fields, read_golden, text};

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
    let ticker = TickerTask::new(Duration(0), Arc::clone(&runner));
    ticker.start().await?;
    check_eq(&runner.run_count(), &1, "the run of Start")?;
    time::sleep(StdDuration::from_millis(50)).await;
    settle().await;
    check_eq(&runner.run_count(), &1, "no periodic runs")
}

#[tokio::test(start_paused = true)]
async fn start_with_periodic_run() -> TestResult {
    // Go: TestStartWithPeriodicRun: one run, then one every 10ms until Stop.
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(Duration(10 * Duration::MILLISECOND.0), Arc::clone(&runner));
    ticker.start().await?;
    time::sleep(StdDuration::from_millis(10)).await;
    settle().await;
    check_eq(&runner.run_count(), &2, "the run of Start and one tick")?;
    ticker.stop();
    time::sleep(StdDuration::from_millis(50)).await;
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
    let ticker = ticker_task_from_func(Duration::HOUR, move || {
        let counter = Arc::clone(&counter);
        async move {
            let _previous = counter.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Infallible>(())
        }
    });
    check_eq(&ticker.runner().run().await, &Ok(()), "Run")?;
    check_eq(&count.load(Ordering::SeqCst), &1, "one call")?;
    check_eq(&ticker.interval(), &Duration::HOUR, "interval")
}

#[tokio::test(start_paused = true)]
async fn negative_interval_runs_once() -> TestResult {
    // Go: `if t.interval > 0`: a negative time.Duration schedules nothing.
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(Duration(-1), Arc::clone(&runner));
    ticker.start().await?;
    time::sleep(StdDuration::from_secs(3600)).await;
    settle().await;
    check_eq(&runner.run_count(), &1, "no periodic runs")
}

#[tokio::test(start_paused = true)]
async fn stop_is_permanent_and_reaches_every_start() -> TestResult {
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(Duration::SECOND, Arc::clone(&runner));
    ticker.start().await?;
    ticker.start().await?;
    time::sleep(StdDuration::from_secs(1)).await;
    settle().await;
    check_eq(&runner.run_count(), &4, "two starts, one tick each")?;
    ticker.stop();
    ticker.stop();
    time::sleep(StdDuration::from_secs(10)).await;
    settle().await;
    check_eq(&runner.run_count(), &4, "Stop ends both")?;
    ticker.start().await?;
    time::sleep(StdDuration::from_secs(10)).await;
    settle().await;
    check_eq(&runner.run_count(), &5, "a Start after Stop only runs once")
}

#[tokio::test(start_paused = true)]
async fn dropping_the_task_does_not_stop_it() -> TestResult {
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(Duration::SECOND, Arc::clone(&runner));
    ticker.start().await?;
    drop(ticker);
    time::sleep(StdDuration::from_secs(3)).await;
    settle().await;
    check_eq(
        &runner.run_count(),
        &4,
        "the periodic runs outlive the task",
    )
}

#[tokio::test(start_paused = true)]
async fn longest_interval_never_ticks() -> TestResult {
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(Duration(i64::MAX), Arc::clone(&runner));
    ticker.start().await?;
    time::sleep(StdDuration::from_hours(24 * 365)).await;
    settle().await;
    check_eq(&runner.run_count(), &1, "only the run of Start")?;
    ticker.stop();
    settle().await;
    check_eq(&runner.run_count(), &1, "still one run after Stop")
}

#[test]
fn start_outside_a_runtime_runs_once_then_fails() -> TestResult {
    let runner = Arc::new(MockRunner::default());
    let ticker = TickerTask::new(Duration::SECOND, Arc::clone(&runner));
    let mut start = pin!(ticker.start());
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(result) = start.as_mut().poll(&mut cx) else {
        return Err("start did not complete".into());
    };
    check_eq(&runner.run_count(), &1, "the first run happened")?;
    check(result.is_err(), "no periodic runs without a runtime")?;
    let zero = TickerTask::new(Duration(0), Arc::clone(&runner));
    let mut start_once = pin!(zero.start());
    check(
        matches!(start_once.as_mut().poll(&mut cx), Poll::Ready(Ok(()))),
        "a zero interval needs no runtime",
    )
}

/// A runner that records when each run starts and sleeps for scripted durations, in turn.
#[derive(Debug)]
struct Scripted {
    origin: Instant,
    durations: Vec<StdDuration>,
    starts: Mutex<Vec<StdDuration>>,
}

impl Runner for Scripted {
    type Error = Infallible;

    async fn run(&self) -> Result<(), Infallible> {
        let d = {
            let mut starts = self.starts.lock().await;
            starts.push(self.origin.elapsed());
            self.durations
                .get((starts.len() - 1) % self.durations.len().max(1))
                .copied()
                .unwrap_or_default()
        };
        time::sleep(d).await;
        Ok(())
    }
}

fn nanos_list(s: &[u8]) -> Result<Vec<i64>, Box<dyn Error>> {
    text(s)?.split(',').map(|n| Ok(n.parse::<i64>()?)).collect()
}

fn std_nanos(n: i64) -> Result<StdDuration, Box<dyn Error>> {
    Ok(StdDuration::from_nanos(u64::try_from(n)?))
}

/// Runs one golden scenario on a fresh paused runtime and returns the run start times.
fn run_scenario(
    interval: i64,
    durations: &[i64],
    stop_at: i64,
    horizon: i64,
) -> Result<Vec<i64>, Box<dyn Error>> {
    let rt = runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()?;
    let durations = durations
        .iter()
        .map(|&d| std_nanos(d))
        .collect::<Result<Vec<_>, _>>()?;
    let horizon = std_nanos(horizon)?;
    rt.block_on(async move {
        let origin = Instant::now();
        let runner = Arc::new(Scripted {
            origin,
            durations,
            starts: Mutex::new(Vec::new()),
        });
        let ticker = Arc::new(TickerTask::new(Duration(interval), Arc::clone(&runner)));
        if stop_at < 0 {
            ticker.stop();
        } else {
            let stop_after = std_nanos(stop_at)?;
            let stopper = Arc::clone(&ticker);
            drop(tokio::spawn(async move {
                time::sleep(stop_after).await;
                stopper.stop();
            }));
        }
        ticker.start().await?;
        time::sleep_until(origin + horizon).await;
        settle().await;
        let starts = runner.starts.lock().await;
        starts
            .iter()
            .map(|d| Ok(i64::try_from(d.as_nanos())?))
            .collect::<Result<Vec<i64>, Box<dyn Error>>>()
    })
}

#[test]
fn ticker_timing_golden() -> TestResult {
    // Go's TickerTask under testing/synctest: run start times for short, long and mixed
    // runs, stops before, during and between runs, and zero or negative intervals.
    let rows = read_golden("task_ticker.txt")?;
    check(rows.len() >= 15, "task_ticker.txt is complete")?;
    for row in &rows {
        let [name, interval, durations, stop_at, horizon, want] = fields::<6>(row)?;
        let got = run_scenario(
            text(interval)?.parse()?,
            &nanos_list(durations)?,
            text(stop_at)?.parse()?,
            text(horizon)?.parse()?,
        )?;
        check_eq(&got, &nanos_list(want)?, text(name)?)?;
    }
    Ok(())
}
