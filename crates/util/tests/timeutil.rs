//! Go's `time` layouts and benbjohnson/clock's mock against `pbs_util::timeutil` on the goldens
//! of `testdata/go/golden/timefmt.go`: `Format(time.RFC3339)` and `Format(time.RFC3339Nano)` of
//! 50,000 seeded instants and of edge instants, and 400 seeded scenarios on clock v1.3.0's
//! `Mock`.

#[cfg(test)]
mod tests {
    use std::{error::Error, fmt::Write as _, fs, path::PathBuf, time::Duration as StdDuration};

    use pbs_util::timeutil::{
        Clock as _, MockClock, RealClock, RealTime, Ticker, Time as _, format_rfc3339,
        format_rfc3339_nano,
    };
    use serde::{Deserialize, de::DeserializeOwned};
    use sha2::{Digest as _, Sha256};
    use time::{OffsetDateTime, SignedDuration, UtcOffset};
    use tokio::{runtime, time::timeout};

    type TestResult = Result<(), Box<dyn Error>>;

    #[derive(Deserialize)]
    struct Layouts {
        seed: u64,
        count: usize,
        inputs_sha256: String,
        outputs_sha256: String,
        edges: Vec<LayoutEdge>,
    }

    #[derive(Deserialize)]
    struct LayoutEdge {
        sec: i64,
        nsec: i64,
        offset: i32,
        rfc3339: String,
        rfc3339_nano: String,
    }

    #[derive(Deserialize)]
    struct Clocks {
        scenarios: Vec<Scenario>,
    }

    #[derive(Deserialize)]
    struct Scenario {
        ops: Vec<String>,
        results: Vec<String>,
    }

    fn read<T: DeserializeOwned>(name: &str) -> Result<T, Box<dyn Error>> {
        let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "testdata", "golden", name]
            .iter()
            .collect();
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn check(ok: bool, what: &str) -> TestResult {
        if ok { Ok(()) } else { Err(what.into()) }
    }

    /// The splitmix64 generator of the Go golden program (Go: `splitmix64`).
    struct SplitMix64(u64);

    impl SplitMix64 {
        const fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Go: `intn(n)`, `next() % n`.
        fn intn(&mut self, n: u64) -> i64 {
            i64::try_from(self.next().checked_rem(n).unwrap_or_default()).unwrap_or_default()
        }
    }

    /// Go: `manifest`, SHA-256 over `len:input\n` and over `output\n`.
    #[derive(Default)]
    struct Manifest {
        inputs: Sha256,
        outputs: Sha256,
    }

    impl Manifest {
        fn add(&mut self, input: &str, output: &str) {
            self.inputs
                .update(format!("{}:{input}\n", input.len()).as_bytes());
            self.outputs.update(format!("{output}\n").as_bytes());
        }

        fn sums(self) -> (String, String) {
            let hex = |d: &[u8]| {
                d.iter().fold(String::new(), |mut s, b| {
                    let _ok = write!(s, "{b:02x}");
                    s
                })
            };
            (hex(&self.inputs.finalize()), hex(&self.outputs.finalize()))
        }
    }

    /// 9999-12-31T23:59:59Z, the last second of the time crate's default range (Go:
    /// `maxUnixSecond`).
    const MAX_UNIX_SECOND: i64 = 253_402_300_799;

    /// Go: `seededInstant`: the Unix second, the nanoseconds and the offset east of UTC.
    fn seeded_instant(r: &mut SplitMix64) -> (i64, i64, i32) {
        let sec = match r.intn(4) {
            0 => r.intn(86_400),
            1 => 946_684_800 + r.intn(2 * 86_400) - 86_400,
            2 => MAX_UNIX_SECOND - r.intn(86_400),
            _ => r.intn(253_402_300_800),
        };
        let nsec = match r.intn(4) {
            0 => 0,
            1 => r.intn(1000) * 1_000_000,
            2 => r.intn(1_000_000) * 1000,
            _ => r.intn(1_000_000_000),
        };
        let offset = match r.intn(3) {
            0 => 0,
            1 => (r.intn(2 * 18 * 60 + 1) - 18 * 60) * 60,
            _ => (r.intn(37) - 18) * 3600,
        };
        (sec, nsec, i32::try_from(offset).unwrap_or_default())
    }

    /// Go's `Z07:00` of a whole-minute offset.
    fn go_zone(offset: i32) -> String {
        if offset == 0 {
            return "Z".to_owned();
        }
        let sign = if offset < 0 { '-' } else { '+' };
        let minutes = (offset / 60).unsigned_abs();
        format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
    }

    /// Go's two layouts of an instant whose local time falls on 10000-01-01, past the time
    /// crate's range: the five-digit year, then the time of day as on any other day.
    fn past_9999(sec: i64, nsec: i64, offset: i32) -> Option<String> {
        let into_day = sec
            .checked_add(i64::from(offset))?
            .checked_sub(MAX_UNIX_SECOND + 1)?;
        if !(0..86_400).contains(&into_day) {
            return None;
        }
        let clock = format!(
            "10000-01-01T{:02}:{:02}:{:02}",
            into_day / 3600,
            into_day / 60 % 60,
            into_day % 60
        );
        let fraction = if nsec == 0 {
            String::new()
        } else {
            format!(".{nsec:09}").trim_end_matches('0').to_owned()
        };
        let zone = go_zone(offset);
        Some(format!("{clock}{zone}\t{clock}{fraction}{zone}"))
    }

    /// The layouts of the sweep: Go's manifests of the formatted instants, and the number of
    /// instants whose local time is past the time crate's range, written as Go writes them.
    fn sweep(g: &Layouts) -> Result<((String, String), usize), Box<dyn Error>> {
        let mut r = SplitMix64(g.seed);
        let mut manifest = Manifest::default();
        let mut past = 0_usize;
        for _ in 0..g.count {
            let (sec, nsec, offset) = seeded_instant(&mut r);
            let nanos = i128::from(sec) * 1_000_000_000 + i128::from(nsec);
            let utc = OffsetDateTime::from_unix_timestamp_nanos(nanos)?;
            let output =
                if let Some(t) = utc.checked_to_offset(UtcOffset::from_whole_seconds(offset)?) {
                    format!("{}\t{}", format_rfc3339(t), format_rfc3339_nano(t))
                } else {
                    past += 1;
                    past_9999(sec, nsec, offset).ok_or("a local time past 10000-01-01")?
                };
            manifest.add(&format!("{sec} {nsec} {offset}"), &output);
        }
        Ok((manifest.sums(), past))
    }

    #[test]
    fn layouts_like_go() -> TestResult {
        // Go: t.Format(time.RFC3339) and t.Format(time.RFC3339Nano), the layouts Prebid's code
        // picks (stored_requests/events/http/http.go:127, the analytics modules' timestamps),
        // in a fixed zone, over 50,000 seeded instants and edge instants: fractions with
        // trailing zeros, offsets of whole hours and minutes, and the first and last seconds of
        // the time crate's range.
        let g: Layouts = read("timeutil_layouts.json")?;
        check(g.count == 50_000, "50,000 seeded instants")?;
        let ((inputs, outputs), _past) = sweep(&g)?;
        check(inputs == g.inputs_sha256, "the seeded instants")?;
        check(outputs == g.outputs_sha256, "their layouts")?;
        check(g.edges.len() == 16, "the edge instants")?;
        for e in &g.edges {
            let nanos = i128::from(e.sec) * 1_000_000_000 + i128::from(e.nsec);
            let t = OffsetDateTime::from_unix_timestamp_nanos(nanos)?
                .checked_to_offset(UtcOffset::from_whole_seconds(e.offset)?)
                .ok_or("an edge in range")?;
            check(format_rfc3339(t) == e.rfc3339, &e.rfc3339)?;
            check(format_rfc3339_nano(t) == e.rfc3339_nano, &e.rfc3339_nano)?;
        }
        Ok(())
    }

    #[test]
    fn years_past_9999() -> TestResult {
        // Go: a time.Time holds years far past 9999, and Format writes a local time on
        // 10000-01-01 with five digits, as the seeded instants within a day of the end of 9999
        // at an offset east of UTC show (their Go texts are in the sweep's manifest). The time
        // crate's default range ends with 9999, so no OffsetDateTime holds them.
        let g: Layouts = read("timeutil_layouts.json")?;
        let ((_inputs, outputs), past) = sweep(&g)?;
        check(
            outputs == g.outputs_sha256,
            "Go's texts of the local times past 9999",
        )?;
        check(
            past == 1_553,
            "the seeded instants past the time crate's range",
        )?;
        let last = OffsetDateTime::from_unix_timestamp(MAX_UNIX_SECOND)?;
        check(
            last.checked_to_offset(UtcOffset::from_hms(0, 1, 0)?)
                .is_none(),
            "a minute east of the last second of 9999",
        )?;
        check(
            OffsetDateTime::from_unix_timestamp(MAX_UNIX_SECOND + 1).is_err(),
            "the first second of 10000",
        )
    }

    /// `d` milliseconds.
    fn ms(d: &str) -> Result<SignedDuration, Box<dyn Error>> {
        Ok(SignedDuration::milliseconds(d.parse()?))
    }

    /// Go: `since(t)`, the milliseconds from the epoch to `t`.
    fn since(t: OffsetDateTime) -> String {
        (t - OffsetDateTime::UNIX_EPOCH)
            .whole_milliseconds()
            .to_string()
    }

    /// The ticker numbered `i` in creation order.
    fn nth<'t>(tickers: &'t mut [Ticker], i: &str) -> Result<&'t mut Ticker, Box<dyn Error>> {
        Ok(tickers
            .get_mut(i.parse::<usize>()?)
            .ok_or("no such ticker")?)
    }

    /// Runs one scenario op on `clock` and returns its result as Go's golden writes it.
    async fn run_op(
        clock: &MockClock,
        tickers: &mut Vec<Ticker>,
        op: &str,
    ) -> Result<String, Box<dyn Error>> {
        let words: Vec<&str> = op.split(' ').collect();
        match words.as_slice() {
            ["ticker", d] => tickers.push(clock.ticker(ms(d)?)),
            ["add", d] => {
                let d = ms(d)?;
                clock.add(d).await;
            }
            ["set", t] => {
                let since_epoch = ms(t)?;
                let to = OffsetDateTime::UNIX_EPOCH
                    .checked_add(since_epoch)
                    .ok_or("a time in range")?;
                clock.set(to).await;
            }
            ["read", i] => {
                return Ok(nth(tickers, i)?
                    .try_tick()
                    .map_or_else(|| "none".to_owned(), since));
            }
            ["stop", i] => nth(tickers, i)?.stop(),
            ["reset", i, d] => nth(tickers, i)?.reset(ms(d)?),
            ["now"] => return Ok(since(clock.now())),
            _ => return Err(format!("unknown op {op:?}").into()),
        }
        Ok(String::new())
    }

    #[test]
    fn mock_clock_like_go() -> TestResult {
        // Go: benbjohnson/clock v1.3.0's Mock, the clock the tests of Prebid's analytics
        // modules inject (analytics/agma, analytics/pubstack): 400 seeded scenarios of Ticker,
        // Add, Set, Stop, Reset, Now and receives from C that do not wait, with every result.
        // The runtime has no I/O or time driver, so the mock's yields between ticks cost no
        // system call.
        let rt = runtime::Builder::new_current_thread().build()?;
        rt.block_on(replay_scenarios())
    }

    async fn replay_scenarios() -> TestResult {
        let g: Clocks = read("timeutil_clock.json")?;
        check(g.scenarios.len() == 400, "400 scenarios")?;
        let (mut reads, mut ticks) = (0_usize, 0_usize);
        for (n, scenario) in g.scenarios.iter().enumerate() {
            check(
                scenario.ops.len() == scenario.results.len(),
                "a result per op",
            )?;
            let clock = MockClock::new();
            let mut tickers = Vec::new();
            for (op, want) in scenario.ops.iter().zip(&scenario.results) {
                let got = run_op(&clock, &mut tickers, op).await?;
                if op.starts_with("read") {
                    reads += 1;
                    ticks += usize::from(got != "none");
                }
                check(
                    &got == want,
                    &format!("scenario {n}: {op}: got {got:?}, want {want:?}"),
                )?;
            }
        }
        check(
            ticks > 0 && ticks < reads,
            "reads that receive a tick and reads that do not",
        )
    }

    #[test]
    fn now_is_the_utc_wall_clock() -> TestResult {
        // Go: timeutil.RealTime.Now is time.Now() (util/timeutil/time.go), in time.Local and
        // with a monotonic reading; RealTime is the wall clock in UTC.
        let now = RealTime.now();
        check(now.offset().is_utc(), "RealTime's offset is UTC")?;
        check(
            RealClock.now().offset().is_utc(),
            "RealClock's offset is UTC",
        )
    }

    #[tokio::test(start_paused = true)]
    async fn non_positive_tickers_never_tick() -> TestResult {
        // Go: time.NewTicker panics for a period that is not positive ("non-positive interval
        // for NewTicker"), and clock v1.3.0's Mock loops for ever in Add once such a ticker is
        // due; here such a ticker never ticks.
        let clock = MockClock::new();
        let mut zero = clock.ticker(SignedDuration::ZERO);
        let mut negative = clock.ticker(SignedDuration::seconds(-1));
        clock.add(SignedDuration::seconds(5)).await;
        check(zero.try_tick().is_none(), "the mock's zero period")?;
        check(negative.try_tick().is_none(), "the mock's negative period")?;
        let mut real = RealClock.ticker(SignedDuration::ZERO);
        let waited = timeout(StdDuration::from_secs(3600), real.tick()).await;
        check(waited.is_err(), "the real zero period")
    }
}
