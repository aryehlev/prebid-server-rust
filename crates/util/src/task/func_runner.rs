//! Go: `util/task/func_runner.go`.

use std::{fmt, sync::Arc};

use time::SignedDuration;

use super::{Runner, TickerTask};

/// Go: `funcRunner`: a [`Runner`] that calls a function returning a future.
#[derive(Clone, Copy)]
pub struct FuncRunner<F>(pub F);

impl<F> fmt::Debug for FuncRunner<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FuncRunner").finish_non_exhaustive()
    }
}

impl<F, Fut, E> Runner for FuncRunner<F>
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), E>> + Send,
    E: Send + 'static,
{
    type Error = E;

    fn run(&self) -> impl Future<Output = Result<(), E>> + Send {
        (self.0)()
    }
}

/// Go: `task.NewTickerTaskFromFunc(interval, runner)`.
#[must_use]
pub fn ticker_task_from_func<F, Fut, E>(
    interval: SignedDuration,
    runner: F,
) -> TickerTask<FuncRunner<F>>
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), E>> + Send,
    E: Send + 'static,
{
    TickerTask::new(interval, Arc::new(FuncRunner(runner)))
}
