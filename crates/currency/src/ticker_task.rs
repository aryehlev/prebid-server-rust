//! The periodic currency fetch (Go: `util/task.TickerTask` as `main.go` uses it for the
//! converter).

use std::{sync::Arc, time::Duration};

use tokio::{
    sync::Notify,
    time::{self, Instant, MissedTickBehavior},
};

use crate::rate_converter::RateConverter;

/// Runs [`RateConverter::run`] now and then every `interval` (Go: `task.NewTickerTask(
/// fetchingInterval, currencyConverter)`).
#[derive(Debug)]
pub struct TickerTask {
    interval: Duration,
    runner: Arc<RateConverter>,
    done: Arc<Notify>,
}

impl TickerTask {
    /// Go: `NewTickerTask(interval, runner)`.
    #[must_use]
    pub fn new(interval: Duration, runner: Arc<RateConverter>) -> Self {
        Self {
            interval,
            runner,
            done: Arc::new(Notify::new()),
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
                Arc::clone(&self.done),
            )));
        }
    }

    /// Stops the periodic fetch after any run in progress; the converter keeps its rates
    /// (Go: `Stop`). Dropping the task does not stop it, as in Go.
    pub fn stop(&self) {
        self.done.notify_one();
    }
}

/// Go: `runRecurring`: a `time.Ticker` whose first tick is one interval after the start and
/// which drops ticks that a slow run misses.
async fn run_recurring(interval: Duration, runner: Arc<RateConverter>, done: Arc<Notify>) {
    let start = Instant::now()
        .checked_add(interval)
        .unwrap_or_else(Instant::now);
    let mut ticker = time::interval_at(start, interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            () = done.notified() => return,
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
}
