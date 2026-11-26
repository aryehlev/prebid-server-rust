//! When the go-metrics arbiter first ticks the meters, in a test binary of its own so that its
//! meter is the process's first: the pin of the ledger's `influx.meter-ticks`.

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        thread,
        time::{Duration, Instant},
    };

    use pbs_metrics::gometrics::new_meter;

    /// A test's result, its failure a message.
    type TestResult = Result<(), Box<dyn Error>>;

    /// `influx.meter-ticks`: the arbiter's first tick comes five seconds after the process's
    /// first meter and sets the one-minute rate to the events of those five seconds. Go's comes
    /// five seconds after the package initialised, which a test process does before any test
    /// runs, so it would come sooner.
    #[test]
    fn first_tick_follows_the_first_meter() -> TestResult {
        // Go: rcrowley/go-metrics cf1acfcdf475 meter.go:233 (the arbiter's ticker, made at
        // package initialisation) and :44-46 (NewMeter starts the goroutine that reads it)
        let created = Instant::now();
        let meter = new_meter();
        meter.mark(5);
        let deadline = created
            .checked_add(Duration::from_secs(8))
            .ok_or("instant overflow")?;
        let ticked = loop {
            if meter.rate1() > 0.0 {
                break created.elapsed();
            }
            if Instant::now() > deadline {
                return Err("no tick within 8 s of the first meter".into());
            }
            thread::sleep(Duration::from_millis(10));
        };
        if ticked < Duration::from_secs(5) || ticked >= Duration::from_millis(7500) {
            return Err(format!("the first tick came {ticked:?} after the first meter").into());
        }
        // Go: fetchInstantRate, the five events over five seconds, in events per second.
        let rate = meter.rate1();
        if (rate - 1.0).abs() > 1e-9 {
            return Err(format!("the one-minute rate after the first tick is {rate}").into());
        }
        Ok(())
    }
}
