//! The shutdown scenarios of `testdata/go/wire/drain.go`, replayed against [`Server::shutdown`]:
//! each scenario starts a fresh server with the main server's settings, dials its connections
//! on schedule, starts the shutdown, and compares what every connection saw, what `serve` and
//! `shutdown` returned, and the connection metrics the listener recorded, with Go's.

use core::{
    fmt::Write as _,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use std::{path::Path, sync::Arc};

use pbs_gocompat::strconv::unquote_bytes;
use pbs_gohttp_server::{ConnMetrics, Listener, Server, prebid::main_server};
use tokio::time::{Instant, sleep, sleep_until};

use super::{
    client::{Case, Outcome, Step, run_case},
    handlers::{Target, drain_handler, quiet},
    parse_send, shorten,
};

pub(super) const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/testdata/golden/shutdown.golden"
);

/// Go: `drainConn`: a case dialed `at` milliseconds into its scenario.
#[derive(Debug, Clone, Default)]
pub(super) struct Conn {
    at: u64,
    case: Case,
}

/// Go: `drainScenario`, with the `drainResult` Go recorded.
#[derive(Debug, Default)]
pub(super) struct Scenario {
    name: String,
    network: String,
    shutdown_at: u64,
    timeout: u64,
    conns: Vec<Conn>,
    serve: String,
    result: String,
    socket: String,
    metrics: String,
}

/// What a scenario observed (Go: `drainResult`).
#[derive(Debug)]
pub(super) struct Observed {
    conns: Vec<Outcome>,
    serve: String,
    result: String,
    socket: String,
    metrics: String,
}

/// Counts the [`ConnMetrics`] calls by their `success` argument (Go: `connCounts`).
#[derive(Debug, Default)]
struct Counts {
    accepted: AtomicU64,
    accept_failed: AtomicU64,
    closed: AtomicU64,
    close_failed: AtomicU64,
}

impl ConnMetrics for Counts {
    fn record_connection_accept(&self, success: bool) {
        let n = if success {
            &self.accepted
        } else {
            &self.accept_failed
        };
        let _previous = n.fetch_add(1, Ordering::SeqCst);
    }

    fn record_connection_close(&self, success: bool) {
        let n = if success {
            &self.closed
        } else {
            &self.close_failed
        };
        let _previous = n.fetch_add(1, Ordering::SeqCst);
    }
}

impl Counts {
    /// Go: `connCounts.String`.
    fn render(&self) -> String {
        format!(
            "accepted {} accept_failed {} closed {} close_failed {}",
            self.accepted.load(Ordering::SeqCst),
            self.accept_failed.load(Ordering::SeqCst),
            self.closed.load(Ordering::SeqCst),
            self.close_failed.load(Ordering::SeqCst)
        )
    }
}

/// Applies one line that belongs to the last connection of `sc`.
fn conn_line(sc: &mut Scenario, key: &str, rest: &str) -> Result<(), String> {
    let conn = sc
        .conns
        .last_mut()
        .ok_or_else(|| format!("{key} before a case"))?;
    let number = |s: &str| s.parse::<u64>().map_err(|e| format!("{s}: {e}"));
    match key {
        "at" => conn.at = number(rest)?,
        "wait" => conn.case.wait = number(rest)?,
        "send" => conn.case.steps.push(Step::Send(parse_send(rest)?)),
        "pause" => conn.case.steps.push(Step::Pause(number(rest)?)),
        "response" => conn.case.response = unquote_bytes(rest).map_err(|e| format!("{e}"))?,
        "end" => rest.clone_into(&mut conn.case.end),
        _ => return Err(format!("unknown key {key}")),
    }
    Ok(())
}

/// Parses `shutdown.golden` (Go: `writeDrainGolden`): the GOOS the generator ran on (its `goos`
/// line) and the scenarios.
pub(super) fn parse_golden(text: &str) -> Result<(String, Vec<Scenario>), String> {
    let mut generated_on = None;
    let mut scenarios: Vec<Scenario> = Vec::new();
    for (no, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = |why: &str| format!("{GOLDEN}:{}: {line}: {why}", no + 1);
        let (key, rest) = line.split_once(' ').ok_or_else(|| bad("no value"))?;
        if key == "goos" {
            if generated_on.is_some() || !scenarios.is_empty() {
                return Err(bad("goos is one line before the first scenario"));
            }
            generated_on = Some(rest.to_owned());
            continue;
        }
        if key == "scenario" {
            scenarios.push(Scenario {
                name: rest.to_owned(),
                ..Scenario::default()
            });
            continue;
        }
        let sc = scenarios
            .last_mut()
            .ok_or_else(|| bad("before a scenario"))?;
        match key {
            "network" => rest.clone_into(&mut sc.network),
            "shutdown" => {
                let (at, timeout) = rest.split_once(' ').ok_or_else(|| bad("two numbers"))?;
                sc.shutdown_at = at.parse().map_err(|e| bad(&format!("{e}")))?;
                sc.timeout = timeout.parse().map_err(|e| bad(&format!("{e}")))?;
            }
            "case" => sc.conns.push(Conn {
                at: 0,
                case: Case {
                    name: rest.to_owned(),
                    ..Case::default()
                },
            }),
            "serve" => rest.clone_into(&mut sc.serve),
            "result" => rest.clone_into(&mut sc.result),
            "socket" => rest.clone_into(&mut sc.socket),
            "metrics" => rest.clone_into(&mut sc.metrics),
            _ => conn_line(sc, key, rest).map_err(|e| bad(&e))?,
        }
    }
    let generated_on = generated_on.ok_or_else(|| format!("{GOLDEN}: no goos line"))?;
    Ok((generated_on, scenarios))
}

/// Go: the listener of `runDrainScenario`, with Prebid's connection metrics.
async fn listen(
    sc: &Scenario,
    dir: &Path,
    metrics: Arc<dyn ConnMetrics>,
) -> Result<(Listener, Target), String> {
    match sc.network.as_str() {
        "tcp" => {
            let l = Listener::bind_tcp("127.0.0.1:0")
                .await
                .map_err(|e| e.to_string())?;
            let addr = l.addr().to_owned();
            Ok((l.with_metrics(metrics), Target::Tcp(addr)))
        }
        "unix" => {
            let path = dir
                .join(format!("{}.sock", sc.name))
                .to_string_lossy()
                .into_owned();
            let l = Listener::bind_unix(&path).map_err(|e| e.to_string())?;
            Ok((l.with_metrics(metrics), Target::Unix(path)))
        }
        other => Err(format!("unknown network {other}")),
    }
}

/// Go: `runDrainScenario`.
pub(super) async fn run(sc: &Scenario, dir: &Path) -> Result<Observed, String> {
    let counts = Arc::new(Counts::default());
    let (listener, target) = listen(sc, dir, Arc::<Counts>::clone(&counts)).await?;
    let addr = listener.addr().to_owned();
    let server = Arc::new(Server::new(quiet(main_server(&addr, drain_handler()?))));
    let serving = Arc::clone(&server);
    let serve_task = tokio::spawn(async move { serving.serve(listener).await });

    let start = Instant::now();
    let mut conns = Vec::with_capacity(sc.conns.len());
    for c in &sc.conns {
        let (target, case) = (target.clone(), c.case.clone());
        let dial_at = start + Duration::from_millis(c.at);
        conns.push(tokio::spawn(async move {
            sleep_until(dial_at).await;
            run_case(&target, &case).await
        }));
    }

    sleep_until(start + Duration::from_millis(sc.shutdown_at)).await;
    let timeout = (sc.timeout > 0).then(|| Duration::from_millis(sc.timeout));
    let result = server
        .shutdown(timeout)
        .await
        .map_or_else(|e| e.to_string(), |()| "<nil>".to_owned());
    let serve = match serve_task.await {
        Ok(Ok(())) => "<nil>".to_owned(),
        Ok(Err(e)) => e.to_string(),
        Err(e) => format!("serve task: {e}"),
    };
    let mut outcomes = Vec::with_capacity(conns.len());
    for t in conns {
        outcomes.push(t.await.map_err(|e| format!("connection task: {e}"))?);
    }
    // Go: a connection is counted as closed just after its client sees the end.
    sleep(Duration::from_millis(100)).await;
    let socket = match &target {
        Target::Tcp(_) => "-",
        Target::Unix(path) if Path::new(path).exists() => "kept",
        Target::Unix(_) => "removed",
    };
    Ok(Observed {
        conns: outcomes,
        serve,
        result,
        socket: socket.to_owned(),
        metrics: counts.render(),
    })
}

/// The report of every difference from Go, or `None`.
pub(super) fn compare(scenarios: &[Scenario], observed: &[Observed]) -> Option<String> {
    let mut report = String::new();
    for (sc, got) in scenarios.iter().zip(observed) {
        for (conn, out) in sc.conns.iter().zip(&got.conns) {
            if out.response != conn.case.response || out.end != conn.case.end {
                let _ok = write!(
                    report,
                    "\n--- {}/{}\n  go:   {} {}\n  rust: {} {}\n",
                    sc.name,
                    conn.case.name,
                    shorten(&conn.case.response),
                    conn.case.end,
                    shorten(&out.response),
                    out.end
                );
            }
        }
        for (what, want, have) in [
            ("serve", &sc.serve, &got.serve),
            ("shutdown", &sc.result, &got.result),
            ("socket", &sc.socket, &got.socket),
            ("metrics", &sc.metrics, &got.metrics),
        ] {
            if want != have {
                let _ok = write!(
                    report,
                    "\n--- {} {what}\n  go:   {want}\n  rust: {have}\n",
                    sc.name
                );
            }
        }
    }
    (!report.is_empty()).then_some(report)
}
