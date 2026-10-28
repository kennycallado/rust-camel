//! IMAP push consumer: one poll pass routes each message and disposes of it
//! only after the route succeeded (at-least-once, bounded redelivery).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_component_api::{
    ConcurrencyModel, Consumer, ConsumerContext, NetworkRetryPolicy, RuntimeObservability,
};
use tracing::{info, warn};

use crate::adapter::inbound::InboundMessage;
use crate::error::MailError;
use crate::health::MailHealthState;
use crate::poll::{Cycle, Loaded, Mailbox, PollState, Reporter, Session};

/// How a poll pass ended without a store error.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Polled {
    /// Every candidate of the pass was handled.
    Done,
    /// The route's channel closed; the consumer must stop.
    RouteClosed,
}

/// Run one poll pass on `session`.
///
/// - `Err` with a retryable error means the session is broken and the
///   caller must reconnect; any other `Err` leaves the session usable.
/// - The caller must race the whole future against cancellation. Dropping
///   it mid-route changes no mailbox state, because `\Seen` is set only
///   after the route returned.
pub(crate) async fn poll_once(
    session: &mut Session,
    mailbox: &Mailbox,
    state: &mut PollState,
    ctx: &ConsumerContext,
    rt: &dyn RuntimeObservability,
) -> Result<Polled, MailError> {
    let reporter = Reporter::route(rt, ctx.route_id());
    let limit = mailbox.config.max_messages_per_poll;
    let mut cycle = Cycle::begin(session, mailbox, state, reporter, limit).await?;
    for uid in cycle.take_candidates() {
        let InboundMessage {
            exchange,
            message_id,
        } = match cycle.load(uid).await? {
            Loaded::Message(message) => *message,
            // A repository outage is reported by `load`; the message stays
            // untouched for the next poll.
            Loaded::Nothing | Loaded::RepositoryDown(_) => continue,
        };
        match ctx.send_and_wait(exchange).await {
            Ok(ex) if !ex.has_error() => cycle.delivered(uid, message_id.as_deref()).await?,
            Err(CamelError::ChannelClosed) => return Ok(Polled::RouteClosed),
            _ => cycle.failed(uid),
        }
    }
    Ok(Polled::Done)
}

/// Sleep for `wait`; `false` when the context was cancelled first.
async fn pause(wait: Duration, ctx: &ConsumerContext) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(wait) => true,
        _ = ctx.cancelled() => false,
    }
}

/// How a session ended.
enum SessionEnd {
    /// Cancelled or the route closed; the consumer stops cleanly.
    Stopped,
    /// The server dropped a session that had completed a poll; reconnect
    /// at once, without a backoff step.
    Stale(MailError),
    /// The session broke; reconnect with backoff.
    Broken(MailError),
}

/// IMAP polling consumer. `start` is the lifetime loop; it never restarts
/// itself (ADR-0007) and reconnects only under `NetworkRetryPolicy`
/// (ADR-0013).
pub(crate) struct MailConsumer {
    mailbox: Mailbox,
    retry: NetworkRetryPolicy,
    health: Arc<MailHealthState>,
    rt: Arc<dyn RuntimeObservability>,
}

impl MailConsumer {
    pub(crate) fn new(
        mailbox: Mailbox,
        retry: NetworkRetryPolicy,
        health: Arc<MailHealthState>,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Self {
        Self {
            mailbox,
            retry,
            health,
            rt,
        }
    }

    /// Open a session under the retry policy; `Ok(None)` when cancelled.
    /// A missing capability is not retryable, so it ends here.
    async fn connect(&self, ctx: &ConsumerContext) -> Result<Option<Session>, MailError> {
        let cancel = ctx.cancel_token();
        let metrics = self.rt.metrics();
        tokio::select! {
            biased;
            _ = ctx.cancelled() => Ok(None),
            opened = camel_component_api::retry_async_cancelable(
                &self.retry,
                self.mailbox.config.scheme.as_str(),
                "connect",
                || self.mailbox.open(),
                MailError::is_retryable,
                &cancel,
                Some(&*metrics),
            ) => opened.map(Some),
        }
    }

    /// Poll every `delay` until the session breaks, the route closes or the
    /// context is cancelled. A refused poll keeps the session and waits for
    /// the next one.
    async fn run_session(
        &self,
        mut session: Session,
        state: &mut PollState,
        failures: &mut u32,
        ctx: &ConsumerContext,
    ) -> SessionEnd {
        loop {
            let polled = tokio::select! {
                biased;
                _ = ctx.cancelled() => {
                    session.close().await;
                    return SessionEnd::Stopped;
                }
                r = poll_once(&mut session, &self.mailbox, state, ctx, &*self.rt) => r,
            };
            self.rt
                .component_metrics()
                .observe("mail", "poll", polled.is_err());
            match polled {
                Ok(Polled::RouteClosed) => {
                    session.close().await;
                    return SessionEnd::Stopped;
                }
                Ok(Polled::Done) => {
                    *failures = 0;
                    session.mark_proven();
                    self.health.record_ok();
                }
                Err(e) if session.went_stale() => return SessionEnd::Stale(e),
                Err(e) if e.is_retryable() => return SessionEnd::Broken(e),
                Err(e) => {
                    warn!(error = %e, "mail poll refused by the server; retrying at the next poll");
                    self.health.record_failure(e.to_string());
                }
            }
            if !pause(self.mailbox.config.delay, ctx).await {
                session.close().await;
                return SessionEnd::Stopped;
            }
        }
    }
}

#[async_trait]
impl Consumer for MailConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        if !pause(self.mailbox.config.initial_delay, &ctx).await {
            return Ok(());
        }
        let mut state = PollState::default();
        let mut failures: u32 = 0;
        loop {
            let session = match self.connect(&ctx).await {
                Ok(Some(session)) => session,
                Ok(None) => return Ok(()),
                Err(e) => {
                    warn!(error = %e, "mail consumer stopped: cannot open a usable session");
                    self.health.record_stopped(e.to_string());
                    return Err(CamelError::from(e));
                }
            };
            self.health.record_ok();
            let e = match self
                .run_session(session, &mut state, &mut failures, &ctx)
                .await
            {
                SessionEnd::Stopped => return Ok(()),
                SessionEnd::Stale(e) => {
                    info!(error = %e, "idle mail session dropped by the server; reconnecting");
                    continue;
                }
                SessionEnd::Broken(e) => e,
            };
            warn!(error = %e, "mail poll failed; reconnecting");
            self.health.record_failure(e.to_string());
            let wait = self.retry.delay_for(failures.min(30));
            failures = failures.saturating_add(1);
            if !pause(wait, &ctx).await {
                return Ok(());
            }
        }
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
}

#[cfg(test)]
pub(crate) mod tests;
