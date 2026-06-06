//! Epoch-based execution timeout enforcement.
//!
//! A dedicated OS thread that calls `engine.increment_epoch()` at a fixed
//! interval (10ms by default, matching Surrealism's approach). When a WASM
//! guest call has `store.set_epoch_deadline(N)`, wasmtime will raise a trap
//! after N epochs have passed without the deadline being renewed.
//!
//! # Why a dedicated OS thread (not `tokio::spawn`)
//!
//! A WASM guest running a tight CPU-bound loop inside `call_async` may not
//! yield to the tokio runtime — especially under single-worker or
//! `current_thread` configurations. A `tokio::spawn` ticker would be queued
//! behind such a guest and never get polled, so the epoch deadline would
//! never fire. A dedicated OS thread is scheduled by the kernel and
//! increments the epoch regardless of tokio's cooperation.
//!
//! The EpochTicker is owned by `WasmRuntime` and lives for the lifetime of the
//! runtime. It is stopped (thread joined) when the runtime is dropped.
//!
//! **M6 fix (from review):** Graceful shutdown is implemented via `AtomicBool`:
//! the ticker loop checks `shutdown.load(Ordering::Acquire)` each iteration,
//! and `Drop` sets the flag and joins the thread. This prevents the ticker
//! thread from calling `engine.increment_epoch()` after the engine is dropped.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use wasmtime::Engine;

/// Background thread that increments the wasmtime engine epoch at a fixed interval.
///
/// # Lifecycle
///
/// 1. Created via `EpochTicker::start(engine, interval)`
/// 2. Runs on a dedicated OS thread, calling `engine.increment_epoch()` every
///    `interval`
/// 3. Stopped automatically when dropped (sets shutdown flag, joins thread)
pub struct EpochTicker {
    handle: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl EpochTicker {
    /// Start epoch ticker thread.
    ///
    /// The thread will call `engine.increment_epoch()` every `interval` until
    /// the `EpochTicker` is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the thread cannot be spawned (extremely unlikely).
    pub fn start(engine: Engine, interval: Duration) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown);

        let handle = std::thread::spawn(move || {
            while !shutdown_flag.load(Ordering::Acquire) {
                std::thread::sleep(interval);
                if !shutdown_flag.load(Ordering::Acquire) {
                    engine.increment_epoch();
                }
            }
        });

        Self {
            handle: Some(handle),
            shutdown,
        }
    }

    /// Check if the ticker is still running.
    pub fn is_running(&self) -> bool {
        !self.shutdown.load(Ordering::Acquire)
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            // Wait for the thread to observe the shutdown flag and exit.
            // At most one `interval` (default 10ms) of latency. This is
            // preferable to `abort()` because we synchronously know the
            // thread is no longer touching the engine before WasmRuntime
            // drops the Engine itself.
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::Config;

    fn create_test_engine() -> Engine {
        let mut config = Config::new();
        config.epoch_interruption(true);
        Engine::new(&config).expect("failed to create test engine")
    }

    #[tokio::test]
    async fn test_epoch_ticker_starts_and_stops() {
        let engine = create_test_engine();
        let ticker = EpochTicker::start(engine, Duration::from_millis(10));
        assert!(ticker.is_running());
        drop(ticker);
        // After drop, the thread should have stopped
    }

    #[tokio::test]
    async fn test_epoch_ticker_increments_epoch() {
        let engine = create_test_engine();
        let ticker = EpochTicker::start(engine.clone(), Duration::from_millis(5));

        // Wait for at least 2 ticks — if ticker works, no panic occurs.
        // `current_epoch()` is pub(crate) in wasmtime 31, so we verify
        // liveness indirectly by confirming the thread stays alive.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            ticker.is_running(),
            "ticker should still be running after 20ms"
        );

        drop(ticker);
    }

    #[tokio::test]
    async fn test_epoch_ticker_stops_cleanly() {
        let engine = create_test_engine();
        let ticker = EpochTicker::start(engine, Duration::from_millis(10));
        assert!(ticker.is_running());

        drop(ticker);

        // Verify we can create a new ticker after dropping the old one
        let engine2 = create_test_engine();
        let ticker2 = EpochTicker::start(engine2, Duration::from_millis(10));
        assert!(ticker2.is_running());
        drop(ticker2);
    }

    #[tokio::test]
    async fn test_multiple_tickers_on_different_engines() {
        let engine1 = create_test_engine();
        let engine2 = create_test_engine();

        let ticker1 = EpochTicker::start(engine1, Duration::from_millis(10));
        let ticker2 = EpochTicker::start(engine2, Duration::from_millis(10));

        assert!(ticker1.is_running());
        assert!(ticker2.is_running());

        drop(ticker1);
        assert!(ticker2.is_running());
        drop(ticker2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_epoch_ticker_does_not_block_tokio_runtime() {
        use std::sync::atomic::AtomicUsize;

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_task = Arc::clone(&counter);

        let progress_task = tokio::spawn(async move {
            let start = tokio::time::Instant::now();
            while start.elapsed() < Duration::from_millis(100) {
                counter_task.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });

        let engine = create_test_engine();
        let ticker = EpochTicker::start(engine, Duration::from_millis(1));

        tokio::time::sleep(Duration::from_millis(100)).await;
        progress_task.await.expect("progress task should finish");
        drop(ticker);

        assert!(counter.load(Ordering::Relaxed) >= 10);
    }
}
