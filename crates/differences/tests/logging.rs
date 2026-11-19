//! Pins of `tracing` with `tracing-subscriber`'s fmt layer against glog as `logger.Warnf` writes
//! through it under the image's `-v 1 -logtostderr` (`testdata/golden/logging.json`).

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    use pbs_differences::{TestResult, check_table, golden, text};
    use regex::Regex;
    use tracing::{subscriber, warn};
    use tracing_subscriber::fmt::MakeWriter;

    /// A writer that keeps what the subscriber writes.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .map_err(|poisoned| io::Error::other(poisoned.to_string()))?
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Capture {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// glog writes `Lmmdd hh:mm:ss.uuuuuu PID file:line] message`, and through Prebid's logger
    /// facade (depth 1) the file and line are always the facade's own, `logger.go:17` for a
    /// warning; `tracing-subscriber`'s fmt layer writes an RFC 3339 time, the level padded to five
    /// characters and the event's target (its module path) with no file or line. Both are
    /// normalised the same way: the time to a pattern, glog's PID to `PID`.
    // Go: logger/logger.go:16-18 (Warnf) and logger/glog.go:23-25 with NewGlogLogger's depth 1
    // (logger/glog.go:37-41); the image runs with -v 1 -logtostderr (Dockerfile:50)
    #[test]
    fn go_difference_line_format() -> TestResult {
        let doc = golden("logging.json")?;
        let message = text(&doc, "/message")?;
        let capture = Capture::default();
        let fmt = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_ansi(false)
            .finish();
        subscriber::with_default(fmt, || {
            warn!("bidder returned no bids: {}", "appnexus");
        });
        let written = String::from_utf8(
            capture
                .0
                .lock()
                .map_err(|poisoned| poisoned.to_string())?
                .clone(),
        )?;
        let time = Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}Z")?;
        let rust = written.trim_end_matches('\n');
        check_table(
            &[vec![
                text(&doc, "/line")?.to_owned(),
                time.replace(rust, "YYYY-MM-DDThh:mm:ss.uuuuuuZ")
                    .into_owned(),
                message.to_owned(),
            ]],
            &[&[
                "WMMDD hh:mm:ss.uuuuuu PID logger.go:17] bidder returned no bids: appnexus",
                "YYYY-MM-DDThh:mm:ss.uuuuuuZ  WARN logging::tests: bidder returned no bids: \
                 appnexus",
                "bidder returned no bids: appnexus",
            ]],
            "glog, tracing-subscriber, the message",
        )
    }
}
