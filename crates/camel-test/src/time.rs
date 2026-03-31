// crates/camel-test/src/time.rs

use std::time::Duration;

/// Controls tokio's mock time inside a test.
///
/// Obtained from `CamelTestContextBuilder::with_time_control().build()`.
/// Wraps `tokio::time::advance` and `tokio::time::resume`.
///
/// # Contract
/// Only valid inside a `#[tokio::test]` runtime. `tokio::time::pause()`
/// must have been called before use — `CamelTestContextBuilder::build()`
/// handles this automatically.
pub struct TimeController;

impl TimeController {
    /// Advance the mocked clock by `duration`.
    ///
    /// All pending `tokio::time::sleep` and `tokio::time::interval` futures
    /// that would fire within this duration are resolved immediately.
    pub async fn advance(&self, duration: Duration) {
        tokio::time::advance(duration).await;
    }

    /// Resume real-time clock progression.
    pub fn resume(&self) {
        tokio::time::resume();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn advance_unblocks_sleep() {
        tokio::time::pause();
        let controller = TimeController;

        let sleeper = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            42
        });

        controller.advance(Duration::from_millis(100)).await;
        let got = sleeper.await.unwrap();
        assert_eq!(got, 42);

        controller.resume();
    }
}
