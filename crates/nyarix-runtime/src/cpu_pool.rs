//! CPU worker pool (see issue #46).
//!
//! A dedicated [`rayon::ThreadPool`] for CPU-bound work, bridged into
//! async code via a oneshot channel so a caller can `.await` the result
//! without blocking the tokio executor thread it's running on — the
//! standard pattern for combining a sync CPU pool with an async runtime.
//!
//! **Scope note:** the issue's concrete consumers — "Криптография
//! (ChaCha20, AES)" and "Компрессия (zstd)" — don't exist yet (no crypto
//! or compression modules are built, M9). "Параллельная обработка ветвей
//! графа" is **not** routed through this pool: [`nyarix_graph::execute_parallel`]
//! (#33) already fans out via `tokio::spawn`, which is the right tool for
//! concurrent *async* branches (each one calls back into async graph
//! execution and awaits channel sends) — `rayon` is for actual blocking
//! CPU-bound work, a different concern. This pool exists for individual
//! modules' `process()` calls to offload heavy synchronous computation
//! onto, once such modules exist.

use rayon::{ThreadPool, ThreadPoolBuildError, ThreadPoolBuilder};
use tokio::sync::oneshot;

/// Dedicated rayon thread pool for CPU-bound work.
pub struct CpuPool {
    pool: ThreadPool,
}

impl CpuPool {
    /// Build a pool with the given number of worker threads.
    ///
    /// # Errors
    /// Returns a [`ThreadPoolBuildError`] if the underlying pool fails to
    /// start.
    pub fn new(num_threads: usize) -> Result<Self, ThreadPoolBuildError> {
        let pool = ThreadPoolBuilder::new()
            .num_threads(num_threads.max(1))
            .thread_name(|index| format!("nyarix-cpu-{index}"))
            // Without a panic_handler, rayon aborts the whole process
            // when a detached `spawn`-ed task panics (there's no caller
            // for the panic to unwind into, by default). Set one so a
            // panicking closure is contained instead: it's logged here,
            // and the caller finds out via `run`'s `rx.await` failing
            // (the sender never got to send a result), which panics
            // normally in the *async* task instead of aborting the
            // process.
            .panic_handler(|payload| {
                let message = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "non-string panic payload".to_string());
                tracing::error!(message, "panic in CpuPool task");
            })
            .build()?;
        Ok(Self { pool })
    }

    /// Build a pool sized to the machine's available parallelism (falling
    /// back to 1 thread if that can't be determined).
    ///
    /// # Errors
    /// Same as [`Self::new`].
    pub fn with_default_parallelism() -> Result<Self, ThreadPoolBuildError> {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        Self::new(threads)
    }

    /// Run a CPU-bound closure on the pool and asynchronously wait for
    /// its result, without blocking the calling task's executor thread.
    ///
    /// # Panics
    /// Panics if `f` itself panics on the worker thread — the panic is
    /// propagated to the caller (via the dropped oneshot sender) rather
    /// than silently swallowed.
    pub async fn run<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        self.pool.spawn(move || {
            let _ = tx.send(f());
        });
        rx.await
            .expect("CpuPool task dropped its sender — the closure must have panicked")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_closure_and_returns_its_result() {
        let pool = CpuPool::new(2).unwrap();
        let result = pool.run(|| (1..=10).sum::<u32>()).await;
        assert_eq!(result, 55);
    }

    #[tokio::test]
    async fn runs_on_a_dedicated_thread_not_the_tokio_executor() {
        let pool = CpuPool::new(1).unwrap();
        let thread_name = pool
            .run(|| std::thread::current().name().map(str::to_string))
            .await;
        assert_eq!(thread_name.as_deref(), Some("nyarix-cpu-0"));
    }

    #[test]
    fn default_parallelism_builds_successfully() {
        assert!(CpuPool::with_default_parallelism().is_ok());
    }

    #[tokio::test]
    #[should_panic(expected = "closure must have panicked")]
    async fn panicking_closure_propagates_as_a_panic() {
        let pool = CpuPool::new(1).unwrap();
        pool.run(|| -> u32 { panic!("boom") }).await;
    }
}
