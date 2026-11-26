//! Go `util/task`: a task that runs once and then periodically until stopped.
//!
//! Go's `TickerTask` drives the currency rate refresh, the GDPR vendor list refresh, the
//! stored request event pollers and the pubstack configuration refresh. This port runs on the
//! tokio runtime of the caller.
//!
//! ```
//! use std::sync::{
//!     Arc,
//!     atomic::{AtomicUsize, Ordering},
//! };
//!
//! use pbs_util::task::ticker_task_from_func;
//! use time::SignedDuration;
//!
//! # #[tokio::main(flavor = "current_thread", start_paused = true)]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let runs = Arc::new(AtomicUsize::new(0));
//! let counter = Arc::clone(&runs);
//! let task = ticker_task_from_func(SignedDuration::SECOND, move || {
//!     let counter = Arc::clone(&counter);
//!     async move {
//!         counter.fetch_add(1, Ordering::SeqCst);
//!         Ok::<(), std::convert::Infallible>(())
//!     }
//! });
//! // Go: Start runs the task before returning, then once a second.
//! task.start().await?;
//! assert_eq!(runs.load(Ordering::SeqCst), 1);
//! tokio::time::sleep(std::time::Duration::from_millis(3500)).await;
//! assert_eq!(runs.load(Ordering::SeqCst), 4);
//! task.stop();
//! # Ok(())
//! # }
//! ```

mod func_runner;
mod ticker_task;

pub use func_runner::{FuncRunner, ticker_task_from_func};
pub use ticker_task::{NoRuntimeError, Runner, TickerTask};

#[cfg(test)]
mod tests;
