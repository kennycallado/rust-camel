//! The shared deadline poll driver (delta spec: "Deadline poll driver
//! contract").
//!
//! Every `validate` target that polls under a deadline — partner, sql,
//! and surreal — runs this one discipline instead of its own loop:
//!
//! - **No deadline**: exactly one snapshot is taken and `decide`
//!   answers from it; the driver never sleeps.
//! - **With a deadline**: the expiry instant `until` is fixed BEFORE
//!   the first snapshot, so the window is anchored to the moment the
//!   action starts rather than to the first sample.
//! - Each iteration takes a snapshot first. A snapshot error stops the
//!   poll at once (`?`), with no sleep and no second read.
//! - The family-supplied `early` judgment runs before the expiry check;
//!   `Some` stops the poll immediately.
//! - Otherwise the expiry check reads the clock: at or past `until`
//!   the expiry snapshot decides via `decide`; before it the driver
//!   sleeps `min(interval, until - now)` — never past the window.
//!
//! A deadline never cancels an in-flight snapshot: an overrunning
//! snapshot completes, and its early judgment still precedes the
//! expiry decision. Per-family poll semantics (which judgments settle
//! early, which wait the full window) stay owned by the family
//! requirements; this module governs only the shared discipline.

use std::time::Duration;

/// Polls snapshots until a deadline, applying the shared poll
/// discipline (delta spec: "Deadline poll driver contract").
///
/// `snapshot` produces the next state (a snapshot error stops the poll
/// at once). `early` may stop the poll before the deadline with a
/// result. `decide` answers from the snapshot at or past the deadline;
/// when `deadline` is `None`, the single snapshot decides immediately.
///
/// See the module docs for the exact ordering and the in-flight
/// snapshot guarantee.
pub(super) async fn poll_until<S, F, Fut>(
    deadline: Option<Duration>,
    interval: Duration,
    mut snapshot: impl FnMut() -> Fut,
    mut early: impl FnMut(&S) -> Option<Result<(), F>>,
    mut decide: impl FnMut(&S) -> Result<(), F>,
) -> Result<(), F>
where
    Fut: std::future::Future<Output = Result<S, F>>,
{
    let Some(deadline) = deadline else {
        let s = snapshot().await?;
        return decide(&s);
    };

    let until = tokio::time::Instant::now() + deadline;
    loop {
        let s = snapshot().await?;
        if let Some(result) = early(&s) {
            return result;
        }
        let now = tokio::time::Instant::now();
        if now >= until {
            return decide(&s);
        }
        tokio::time::sleep((until - now).min(interval)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// A snapshot that returns `Ok(counter)` on each call, counting
    /// invocations, with `Ready` so the future type stays fixed.
    fn counting_snapshot(
        calls: &Cell<u32>,
    ) -> impl FnMut() -> std::future::Ready<Result<u32, u32>> + '_ {
        move || {
            let next = calls.get() + 1;
            calls.set(next);
            std::future::ready(Ok(next))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn no_deadline_takes_one_snapshot_and_decides() {
        let calls = Cell::new(0u32);
        let result = poll_until(
            None,
            Duration::from_millis(100),
            counting_snapshot(&calls),
            |_s: &u32| None,
            |_s: &u32| Ok(()),
        )
        .await;

        assert_eq!(result, Ok(()));
        assert_eq!(
            calls.get(),
            1,
            "no-deadline path must snapshot exactly once"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn early_judgment_stops_before_deadline() {
        let calls = Cell::new(0u32);
        let result = poll_until(
            Some(Duration::from_secs(5)),
            Duration::from_millis(100),
            counting_snapshot(&calls),
            |s: &u32| (*s >= 2).then_some(Ok(())),
            |_s: &u32| Err(9),
        )
        .await;

        assert_eq!(result, Ok(()));
        assert_eq!(
            calls.get(),
            2,
            "early judgment must stop the poll on the satisfying snapshot"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn expiry_snapshot_decides_absence_claim() {
        let calls = Cell::new(0u32);
        let decided_state = Cell::new(0u32);
        let result = poll_until(
            Some(Duration::from_secs(2)),
            Duration::from_millis(100),
            counting_snapshot(&calls),
            |_s: &u32| None,
            |s: &u32| {
                decided_state.set(*s);
                Ok(())
            },
        )
        .await;

        assert_eq!(result, Ok(()));
        assert!(calls.get() > 1, "a deadline poll must re-snapshot");
        assert_eq!(
            decided_state.get(),
            calls.get(),
            "decide must see the last snapshot's state"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn snapshot_error_stops_at_once() {
        let calls = Cell::new(0u32);
        let result: Result<(), u32> = poll_until(
            Some(Duration::from_secs(5)),
            Duration::from_millis(100),
            || {
                calls.set(calls.get() + 1);
                std::future::ready(Err(42u32))
            },
            |_s: &u32| None,
            |_s: &u32| Ok(()),
        )
        .await;

        assert_eq!(result, Err(42), "the same snapshot error must propagate");
        assert_eq!(calls.get(), 1, "a snapshot error stops with no second read");
    }

    #[tokio::test(start_paused = true)]
    async fn sleep_never_exceeds_remaining_window() {
        let calls = Cell::new(0u32);
        let times: RefCell<Vec<tokio::time::Instant>> = RefCell::new(Vec::new());
        let result = poll_until(
            Some(Duration::from_millis(250)),
            Duration::from_millis(100),
            || {
                times.borrow_mut().push(tokio::time::Instant::now());
                let next = calls.get() + 1;
                calls.set(next);
                std::future::ready(Ok::<u32, u32>(next))
            },
            |_s: &u32| None,
            |_s: &u32| Ok(()),
        )
        .await;

        assert_eq!(result, Ok(()));
        let times = times.into_inner();
        assert_eq!(times.len(), 4, "snapshots at 0, 100, 200, and 250 ms");
        let gaps: Vec<Duration> = times.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(
            gaps,
            vec![
                Duration::from_millis(100),
                Duration::from_millis(100),
                Duration::from_millis(50),
            ],
            "the final sleep must be clipped to the 50 ms remaining window"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn overrunning_snapshot_still_gets_early_judgment() {
        let decide_called = Cell::new(false);
        let result = poll_until(
            Some(Duration::from_millis(100)),
            Duration::from_millis(100),
            || async {
                // The deadline cannot cancel this in-flight snapshot:
                // paused time only leaves the window once advanced.
                tokio::time::advance(Duration::from_millis(150)).await;
                Ok::<u32, ()>(7)
            },
            |_s: &u32| Some(Ok(())),
            |_s: &u32| {
                decide_called.set(true);
                Err(())
            },
        )
        .await;

        assert_eq!(result, Ok(()), "early judgment must win over decide");
        assert!(
            !decide_called.get(),
            "an overrunning snapshot still gets its early judgment first"
        );
    }
}
