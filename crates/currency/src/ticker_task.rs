//! The periodic currency fetch (Go: `util/task.TickerTask` as `main.go` uses it for the
//! converter).

use std::{future, sync::Arc, time::Duration};

use tokio::{
    sync::watch,
    time::{self, Instant, MissedTickBehavior},
};

use crate::rate_converter::RateConverter;

/// Go's longest `time.Duration`: `math.MaxInt64` nanoseconds, about 292 years.
const GO_MAX_DURATION: Duration = Duration::from_nanos(0x7FFF_FFFF_FFFF_FFFF);

/// Runs [`RateConverter::run`] now and then every `interval` (Go: `task.NewTickerTask(
/// fetchingInterval, currencyConverter)`).
#[derive(Debug)]
pub struct TickerTask {
    interval: Duration,
    runner: Arc<RateConverter>,
    /// Go: the `done` channel; it holds `true` once [`stop`](Self::stop) has closed it.
    done: watch::Sender<bool>,
}

impl TickerTask {
    /// Go: `NewTickerTask(interval, runner)`.
    #[must_use]
    pub fn new(interval: Duration, runner: Arc<RateConverter>) -> Self {
        Self {
            interval,
            runner,
            done: watch::Sender::new(false),
        }
    }

    /// Runs the converter once, then, for a non-zero interval, spawns the periodic fetch on the
    /// current tokio runtime (Go: `Start`). Like Go, the result of each run is only logged.
    pub async fn start(&self) {
        let _result = self.runner.run().await;
        if !self.interval.is_zero() {
            drop(tokio::spawn(run_recurring(
                self.interval,
                Arc::clone(&self.runner),
                self.done.subscribe(),
            )));
        }
    }

    /// Stops the periodic fetch after any run in progress; the converter keeps its rates
    /// (Go: `Stop`, which closes the `done` channel).
    ///
    /// Like closing a Go channel, this reaches every periodic fetch the task has started and
    /// is permanent, so a later [`start`](Self::start) only runs once. Dropping the task
    /// without stopping it stops nothing, as in Go, where the channel then never closes.
    pub fn stop(&self) {
        let _was_stopped = self.done.send_replace(true);
    }
}

/// Resolves once [`TickerTask::stop`] has been called (Go: `case <-t.done`).
async fn stopped(done: &mut watch::Receiver<bool>) {
    if done.wait_for(|stopped| *stopped).await.is_err() {
        // The task was dropped without Stop: Go's done channel is never closed.
        future::pending::<()>().await;
    }
}

/// Go: `runRecurring`: a `time.Ticker` whose first tick is one interval after the start and
/// which drops ticks that a slow run misses.
async fn run_recurring(
    interval: Duration,
    runner: Arc<RateConverter>,
    mut done: watch::Receiver<bool>,
) {
    // A Go interval cannot be longer; the clamp also keeps tokio's tick arithmetic in range.
    let interval = interval.min(GO_MAX_DURATION);
    let Some(start) = Instant::now().checked_add(interval) else {
        // The first tick is too far away to schedule, so, like Go's ticker, it never fires.
        stopped(&mut done).await;
        return;
    };
    let mut ticker = time::interval_at(start, interval);
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

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use tokio::{task, time};

    use super::TickerTask;
    use crate::{
        golden::{TestResult, check_eq},
        rate_converter::{HttpClient, HttpResponse, RateConverter, ResponseFuture},
    };

    /// Counts the fetches and answers each with an empty currency file.
    #[derive(Debug, Default)]
    struct CountingClient(AtomicUsize);

    impl HttpClient for CountingClient {
        fn get<'a>(&'a self, _url: &'a str) -> ResponseFuture<'a> {
            let _previous = self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(HttpResponse::from_bytes(200, b"{}".to_vec())) })
        }
    }

    fn converter(client: &Arc<CountingClient>) -> Arc<RateConverter> {
        Arc::new(RateConverter::new(
            Arc::<CountingClient>::clone(client),
            Duration::from_secs(60),
            "currency.fake.com".to_owned(),
            Duration::ZERO,
        ))
    }

    async fn settle() {
        for _ in 0..10 {
            task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn runs_once_then_every_interval_until_stopped() -> TestResult {
        let client = Arc::new(CountingClient::default());
        let task = TickerTask::new(Duration::from_mins(30), converter(&client));
        task.start().await;
        check_eq(
            &client.0.load(Ordering::SeqCst),
            &1,
            "Start runs the task synchronously",
        )?;

        time::sleep(Duration::from_secs(1799)).await;
        settle().await;
        check_eq(
            &client.0.load(Ordering::SeqCst),
            &1,
            "the first tick is one interval later",
        )?;

        time::sleep(Duration::from_secs(1)).await;
        settle().await;
        check_eq(&client.0.load(Ordering::SeqCst), &2, "first tick")?;

        time::sleep(Duration::from_mins(90)).await;
        settle().await;
        check_eq(&client.0.load(Ordering::SeqCst), &5, "three more ticks")?;

        task.stop();
        time::sleep(Duration::from_mins(90)).await;
        settle().await;
        check_eq(&client.0.load(Ordering::SeqCst), &5, "no runs after Stop")
    }

    #[tokio::test(start_paused = true)]
    async fn zero_interval_runs_once() -> TestResult {
        let client = Arc::new(CountingClient::default());
        let task = TickerTask::new(Duration::ZERO, converter(&client));
        task.start().await;
        time::sleep(Duration::from_hours(1)).await;
        settle().await;
        check_eq(
            &client.0.load(Ordering::SeqCst),
            &1,
            "no periodic runs without an interval",
        )
    }

    #[tokio::test(start_paused = true)]
    async fn stop_reaches_every_periodic_fetch_and_is_permanent() -> TestResult {
        // Go: Stop closes the done channel, which every runRecurring goroutine of the task
        // selects on, now and later.
        let client = Arc::new(CountingClient::default());
        let task = TickerTask::new(Duration::from_mins(30), converter(&client));
        task.start().await;
        task.start().await;
        time::sleep(Duration::from_mins(30)).await;
        settle().await;
        check_eq(
            &client.0.load(Ordering::SeqCst),
            &4,
            "two runs from Start, one tick of each periodic fetch",
        )?;

        task.stop();
        time::sleep(Duration::from_hours(2)).await;
        settle().await;
        check_eq(
            &client.0.load(Ordering::SeqCst),
            &4,
            "Stop ends both periodic fetches",
        )?;

        task.start().await;
        time::sleep(Duration::from_hours(2)).await;
        settle().await;
        check_eq(
            &client.0.load(Ordering::SeqCst),
            &5,
            "a Start after Stop only runs once: the channel stays closed",
        )
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_task_does_not_stop_it() -> TestResult {
        let client = Arc::new(CountingClient::default());
        let task = TickerTask::new(Duration::from_mins(30), converter(&client));
        task.start().await;
        drop(task);
        time::sleep(Duration::from_mins(90)).await;
        settle().await;
        check_eq(
            &client.0.load(Ordering::SeqCst),
            &4,
            "the periodic fetch outlives the task, as in Go",
        )
    }

    #[tokio::test(start_paused = true)]
    async fn an_interval_too_long_to_schedule_never_ticks() -> TestResult {
        // Go's ticker never fires within a run of the server for its longest intervals; an
        // interval whose first tick cannot even be scheduled must not tick at once instead.
        let client = Arc::new(CountingClient::default());
        let task = TickerTask::new(Duration::MAX, converter(&client));
        task.start().await;
        time::sleep(Duration::from_hours(24)).await;
        settle().await;
        check_eq(
            &client.0.load(Ordering::SeqCst),
            &1,
            "only the run of Start",
        )?;
        task.stop();
        settle().await;
        check_eq(
            &client.0.load(Ordering::SeqCst),
            &1,
            "still one run after Stop",
        )
    }
}
