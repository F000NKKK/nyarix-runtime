//! I/O thread pool (see issue #45).
//!
//! A dedicated [`tokio::runtime::Runtime`], separate from whatever
//! runtime the embedding application is driving [`crate::execution_loop`]
//! on — isolating I/O-bound work onto its own worker threads so it can't
//! be starved by (or starve) CPU-bound work on [`crate::cpu_pool::CpuPool`]
//! (#46).
//!
//! **Scope note:** the issue's concrete consumers — "Обработка сетевых
//! операций (UDP, TCP, QUIC)" and "Чтение/запись TUN" — don't exist to
//! plug in yet: no network transport modules are built (M9), and
//! `nyarix-tun` is a separate repo with no code, only an issue backlog.
//! This is the pool infrastructure itself; wiring a real I/O source into
//! it is whichever issue builds that transport.

use std::future::Future;
use std::io;

use tokio::runtime::{Builder, Runtime};
use tokio::task::JoinHandle;

/// Dedicated tokio runtime for I/O-bound work.
pub struct IoPool {
    runtime: Runtime,
}

impl IoPool {
    /// Build a pool with the given number of worker threads.
    ///
    /// # Errors
    /// Returns an [`io::Error`] if the underlying runtime fails to start
    /// (see [`tokio::runtime::Builder::build`]).
    pub fn new(worker_threads: usize) -> io::Result<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(worker_threads.max(1))
            .thread_name("nyarix-io")
            .enable_all()
            .build()?;
        Ok(Self { runtime })
    }

    /// Build a pool sized to the machine's available parallelism (falling
    /// back to 1 thread if that can't be determined).
    ///
    /// # Errors
    /// Same as [`Self::new`].
    pub fn with_default_parallelism() -> io::Result<Self> {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        Self::new(threads)
    }

    /// Spawn a future onto this pool's worker threads, without blocking
    /// whatever executor the caller is currently running on.
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime.spawn(future)
    }

    /// The pool's [`tokio::runtime::Handle`], for code that wants to
    /// spawn onto it without holding a reference to the whole `IoPool`.
    #[must_use]
    pub fn handle(&self) -> tokio::runtime::Handle {
        self.runtime.handle().clone()
    }

    /// Shut the pool down without blocking the calling thread.
    ///
    /// Prefer this over just letting an `IoPool` drop when you're
    /// running inside another async runtime's context: `Runtime`'s
    /// `Drop` blocks the current thread to wait for shutdown, which
    /// tokio forbids (and panics on) from within async code. This method
    /// hands shutdown off to a background thread instead, so it's safe
    /// to call — or to let a value drop as a result of — from async
    /// code.
    pub fn shutdown_background(self) {
        self.runtime.shutdown_background();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawned_future_runs_on_the_pool_and_returns_its_result() {
        let pool = IoPool::new(2).unwrap();
        let handle = pool.spawn(async { 2 + 2 });
        assert_eq!(handle.await.unwrap(), 4);
        // Dropping `pool` here directly would panic (see
        // `shutdown_background`'s docs) — this test runs inside another
        // tokio runtime (`#[tokio::test]`'s own).
        pool.shutdown_background();
    }

    #[tokio::test]
    async fn pool_is_isolated_from_the_calling_runtime() {
        let pool = IoPool::new(1).unwrap();
        let pool_thread_name = pool
            .spawn(async { std::thread::current().name().map(str::to_string) })
            .await
            .unwrap();
        assert_eq!(pool_thread_name.as_deref(), Some("nyarix-io"));
        pool.shutdown_background();
    }

    #[test]
    fn default_parallelism_builds_successfully() {
        assert!(IoPool::with_default_parallelism().is_ok());
    }
}
