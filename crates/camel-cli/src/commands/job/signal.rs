//! Signal machinery for `camel job`: the entry-registered signal
//! streams ([`JobSignals`]), the signal-first race every wait
//! operation goes through ([`await_job_operation_or_signal`]), and
//! the post-first-signal force-exit guard.
//!
//! Arming happens at the first lines of `run_job` (BEFORE config
//! load) so a signal arriving while config load, the bundle cascade,
//! discovery, or `ctx.start()` still runs is buffered by the runtime
//! instead of hitting the default disposition (mirrors `camel run`
//! step 0, rc-z5zch / rc-ukwlt). The first signal is consumed by the
//! send/drain race; ownership of the streams then moves to the
//! force-exit guard for teardown.

use std::future::Future;

/// The job's registered signal streams: SIGINT and SIGTERM on Unix,
/// the portable Ctrl+C listener elsewhere. Armed at the very start of
/// `run_job` (BEFORE config load) so a signal arriving while config
/// load, the bundle cascade, discovery, or `ctx.start()` still runs is
/// buffered by the runtime instead of hitting the default disposition
/// (mirrors `camel run` step 0, rc-z5zch / rc-ukwlt). Both streams
/// stay alive until the first signal is consumed by the send/drain
/// race; ownership then moves to the force-exit guard for teardown.
///
/// Non-Unix loss window (known, accepted): off Unix the portable
/// `ctrl_c()` listener is awaited inside [`JobSignals::next`], which
/// first runs at the send/drain race — after boot. tokio installs the
/// console handler only when the first `ctrl_c()` future is polled,
/// so a Ctrl+C before that point (the whole boot stretch) hits the
/// default disposition and terminates the process outright: no
/// `Interrupted` report, no exit 2. The buffered-during-boot guarantee
/// above is therefore Unix-only; on non-Unix the covered stretch
/// starts at the send/drain race.
pub(super) struct JobSignals {
    #[cfg(unix)]
    int: tokio::signal::unix::Signal,
    #[cfg(unix)]
    term: tokio::signal::unix::Signal,
}

impl JobSignals {
    /// Register the streams at the first lines of `run_job`.
    #[cfg(unix)]
    pub(super) fn arm() -> Self {
        Self {
            int: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .expect("Failed to install SIGINT handler"), // allow-unwrap
            term: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler"), // allow-unwrap
        }
    }

    #[cfg(not(unix))]
    pub(super) fn arm() -> Self {
        Self {}
    }

    /// Resolve on the next registered signal. The first call consumes
    /// the first signal, including one buffered during boot (tokio
    /// coalesces same-signal bursts into one delivery). Off unix a
    /// fresh `ctrl_c()` future fills the role — the global listener
    /// registration persists across calls.
    pub(super) async fn next(&mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = self.int.recv() => {}
            _ = self.term.recv() => {}
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
    }

    /// Own the streams inside the post-first-signal force-exit guard:
    /// exit 1 on the NEXT signal so a stuck teardown cannot outlive
    /// the operator's patience (mirrors `camel run` rc-kz85m —
    /// orchestrators resend the stop signal after their grace period).
    /// The first signal was already consumed by the wait race, so this
    /// only observes later signals.
    pub(super) async fn force_exit(mut self) -> ! {
        self.next().await;
        tracing::warn!("camel job: second stop signal — forcing exit");
        std::process::exit(1)
    }
}

/// Outcome of racing one job wait operation (the send under its
/// overall deadline, or the batch drain) against the first registered
/// signal.
#[derive(Debug)]
pub(super) enum JobWaitOutcome<T> {
    /// The operation completed with its value.
    Completed(T),
    /// The first registered signal fired.
    Signaled,
}

/// Race a job wait operation against the first registered signal with
/// signal-first precedence: the `biased` select polls the signal arm
/// first, so a signal ready at the same poll point as send completion
/// or deadline expiry wins deterministically (spec: signal-first tie).
/// A signal win drops the operation future, cancelling the in-flight
/// send or batch drain.
pub(super) async fn await_job_operation_or_signal<S, F, T>(
    signal: S,
    operation: F,
) -> JobWaitOutcome<T>
where
    S: Future<Output = ()>,
    F: Future<Output = T>,
{
    tokio::select! {
        biased;
        () = signal => JobWaitOutcome::Signaled,
        value = operation => JobWaitOutcome::Completed(value),
    }
}

#[cfg(test)]
mod signal_wait_tests {
    use super::{JobWaitOutcome, await_job_operation_or_signal};

    /// A signal and an operation that are both ready at the same poll
    /// point resolve to the signal branch: the biased select polls the
    /// signal arm first, so the tie is deterministically signal-first.
    #[tokio::test]
    async fn job_signal_wins_ready_tie() {
        let outcome =
            await_job_operation_or_signal(std::future::ready(()), std::future::ready("op")).await;
        assert!(
            matches!(outcome, JobWaitOutcome::Signaled),
            "ready signal must win the tie, got {outcome:?}"
        );
    }
}
