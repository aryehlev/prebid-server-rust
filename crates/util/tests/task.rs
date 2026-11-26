//! Go's `util/task.TickerTask` against `pbs_util::task` on `testdata/golden/task_ticker.txt`,
//! which `testdata/go/tickergolden` writes under `testing/synctest`: the start time of every run
//! for short, long and mixed runs, stops before, during and between runs, and intervals that are
//! zero or negative.

#[cfg(test)]
mod tests {
    use std::{
        convert::Infallible, error::Error, fs, path::PathBuf, sync::Arc,
        time::Duration as StdDuration,
    };

    use pbs_util::task::{Runner, TickerTask};
    use time::SignedDuration;
    use tokio::{
        runtime,
        sync::Mutex,
        task,
        time::{Instant, sleep, sleep_until},
    };

    type TestResult = Result<(), Box<dyn Error>>;

    fn check(ok: bool, what: &str) -> TestResult {
        if ok { Ok(()) } else { Err(what.into()) }
    }

    /// The rows of `task_ticker.txt`: tab-separated Go string literals, none of which holds an
    /// escape.
    fn rows() -> Result<Vec<Vec<String>>, Box<dyn Error>> {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "testdata",
            "golden",
            "task_ticker.txt",
        ]
        .iter()
        .collect();
        fs::read_to_string(path)?
            .lines()
            .map(|line| {
                line.split('\t')
                    .map(|field| {
                        field
                            .strip_prefix('"')
                            .and_then(|f| f.strip_suffix('"'))
                            .filter(|f| !f.contains(['\\', '"']))
                            .map(str::to_owned)
                            .ok_or_else(|| format!("not a plain Go string: {field}").into())
                    })
                    .collect()
            })
            .collect()
    }

    /// A runner that records when each run starts and runs for scripted durations, in turn.
    struct Scripted {
        origin: Instant,
        durations: Vec<StdDuration>,
        starts: Mutex<Vec<StdDuration>>,
    }

    impl Runner for Scripted {
        type Error = Infallible;

        async fn run(&self) -> Result<(), Infallible> {
            let mut starts = self.starts.lock().await;
            starts.push(self.origin.elapsed());
            let runs = starts.len();
            drop(starts);
            let at = runs.saturating_sub(1) % self.durations.len().max(1);
            sleep(self.durations.get(at).copied().unwrap_or_default()).await;
            Ok(())
        }
    }

    /// Nanoseconds as a std duration.
    fn std_nanos(n: i64) -> Result<StdDuration, Box<dyn Error>> {
        Ok(StdDuration::from_nanos(u64::try_from(n)?))
    }

    /// A comma-separated list of nanoseconds.
    fn nanos_list(s: &str) -> Result<Vec<i64>, Box<dyn Error>> {
        s.split(',').map(|n| Ok(n.parse::<i64>()?)).collect()
    }

    /// Lets every task that is ready run.
    async fn settle() {
        for _ in 0..10 {
            task::yield_now().await;
        }
    }

    /// Runs one scenario on a fresh runtime whose clock is paused, so that it moves only when
    /// every task waits on a timer, as under synctest, and returns the start time of each run.
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
            let interval = SignedDuration::nanoseconds(interval);
            let ticker = Arc::new(TickerTask::new(interval, Arc::clone(&runner)));
            if stop_at < 0 {
                ticker.stop();
            } else {
                let stop_after = std_nanos(stop_at)?;
                let stopper = Arc::clone(&ticker);
                drop(tokio::spawn(async move {
                    sleep(stop_after).await;
                    stopper.stop();
                }));
            }
            ticker.start().await?;
            sleep_until(origin + horizon).await;
            settle().await;
            let starts = runner.starts.lock().await;
            starts
                .iter()
                .map(|d| Ok(i64::try_from(d.as_nanos())?))
                .collect::<Result<Vec<i64>, Box<dyn Error>>>()
        })
    }

    #[test]
    fn missed_ticks() -> TestResult {
        // Go: TickerTask's time.Ticker drops the ticks a slow run misses, so the next run
        // starts as soon as the slow one ends and the one after on the original schedule
        // (util/task/ticker_task.go), replayed from Go's run start times under synctest.
        let rows = rows()?;
        check(rows.len() == 15, "task_ticker.txt is complete")?;
        for row in &rows {
            let [name, interval, durations, stop_at, horizon, want] = row.as_slice() else {
                return Err(format!("a row of six fields: {row:?}").into());
            };
            let got = run_scenario(
                interval.parse()?,
                &nanos_list(durations)?,
                stop_at.parse()?,
                horizon.parse()?,
            )?;
            check(
                got == nanos_list(want)?,
                &format!("{name}: got {got:?}, want {want}"),
            )?;
        }
        Ok(())
    }
}
