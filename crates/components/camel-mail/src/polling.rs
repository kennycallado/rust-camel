//! Pull consumption for `pollEnrich`: one message per `receive`, disposed
//! right after the fetch because a pull has no route-completion signal.
//!
//! Delivery is at-most-once: once disposed, a message that the caller later
//! fails to process is not redelivered. The Message-ID is `add`ed when the
//! exchange is handed over, before the disposition, and the message is
//! returned even when a disposition step fails. A failure before `\Seen`
//! leaves it unseen, so a later pull without a repository returns it again,
//! up to `maxAttempts` times; a failure after `\Seen` leaves a seen message
//! in the folder.
//!
//! Divergences from the push consumer: a failed repository read ends the
//! pull with an error instead of moving on to the next message, and skips
//! and failures are logged but not counted, because a `PollingConsumer` has
//! no runtime observability.
//!
//! `receive(timeout)` bounds connecting, scanning and loading. Once a
//! message is loaded, its hand-over and disposition run to completion,
//! bounded only by the adapter's per-command I/O timeout, so the deadline
//! never drops a loaded message. On expiry before that point, or on a
//! connection-level error, the session is dropped without `LOGOUT` and the
//! next call reconnects. A cached session that fails at the connection
//! level is replaced by a fresh one once, within the same deadline.
//!
//! `receive(Duration::ZERO)` cannot return mail from a real server: the
//! deadline has passed before the first network reply arrives.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{CamelError, Exchange};
use camel_component_api::PollingConsumer;
use tokio::time::Instant;
use tracing::debug;

use crate::adapter::inbound::InboundMessage;
use crate::error::MailError;
use crate::poll::{Cycle, Loaded, Mailbox, PollState, Reporter, SIZE_FETCH_CHUNK, Session};

/// Pull-style IMAP consumer. The session sits in a `std::sync::Mutex` only
/// to make the holder `Sync` (`MailStore` is `Send`); it is reached through
/// `get_mut`, so no lock is taken and no guard crosses an `.await`.
pub(crate) struct MailPollingConsumer {
    mailbox: Mailbox,
    state: PollState,
    session: Mutex<Option<Session>>,
}

/// A loaded message whose disposition has not run yet.
struct Picked {
    uid: u32,
    uid_validity: u32,
    message: InboundMessage,
}

/// Outcome of the deadline-bounded part of one pass.
enum Pass {
    /// A message to hand over on this session.
    Found(Session, Box<Picked>),
    /// Nothing deliverable; the session stays usable.
    Empty(Session),
    /// The pass failed; the session is returned only while it is usable.
    Failed(Option<Session>, MailError),
}

impl MailPollingConsumer {
    pub(crate) fn new(mailbox: Mailbox) -> Self {
        Self {
            mailbox,
            state: PollState::default(),
            session: Mutex::new(None),
        }
    }

    fn session_slot(&mut self) -> &mut Option<Session> {
        self.session
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Scan one size chunk of eligible UIDs and load the first deliverable
    /// message, moving past messages that turn out to be skipped or
    /// duplicates. Nothing is disposed here.
    async fn load_first(&mut self, session: &mut Session) -> Result<Option<Picked>, MailError> {
        let mut cycle = Cycle::begin(
            session,
            &self.mailbox,
            &mut self.state,
            Reporter::log_only(),
            SIZE_FETCH_CHUNK,
        )
        .await?;
        let uid_validity = cycle.uid_validity();
        for uid in cycle.take_candidates() {
            match cycle.load(uid).await? {
                Loaded::Message(message) => {
                    return Ok(Some(Picked {
                        uid,
                        uid_validity,
                        message: *message,
                    }));
                }
                Loaded::Nothing => {}
                Loaded::RepositoryDown(e) => return Err(e),
            }
        }
        Ok(None)
    }

    /// One pass on `session`, connecting first when there is none.
    async fn pass_on(&mut self, session: Option<Session>) -> Pass {
        let mut session = match session {
            Some(session) => session,
            None => match self.mailbox.open().await {
                Ok(session) => session,
                Err(e) => return Pass::Failed(None, e),
            },
        };
        match self.load_first(&mut session).await {
            Ok(Some(picked)) => Pass::Found(session, Box::new(picked)),
            Ok(None) => Pass::Empty(session),
            Err(e) if e.is_retryable() => Pass::Failed(None, e),
            Err(e) => Pass::Failed(Some(session), e),
        }
    }

    /// One pass; a cached session that broke is replaced once by a fresh
    /// connection (the server may have dropped it while idle).
    async fn pass(&mut self, cached: Option<Session>) -> Pass {
        let reused = cached.is_some();
        match self.pass_on(cached).await {
            Pass::Failed(None, e) if reused => {
                debug!(error = %e, "cached mail session broke; retrying on a fresh connection");
                self.pass_on(None).await
            }
            pass => pass,
        }
    }

    /// Hand the picked message over: `add` its Message-ID and run the
    /// success disposition. The exchange is returned whatever the outcome;
    /// a connection-level failure has marked the session broken.
    async fn hand_over(&mut self, mut session: Session, picked: Picked) -> Exchange {
        let Picked {
            uid,
            uid_validity,
            message,
        } = picked;
        let mut cycle = Cycle::resume(
            &mut session,
            &self.mailbox,
            &mut self.state,
            Reporter::log_only(),
            uid_validity,
        );
        let _ = cycle.delivered(uid, message.message_id.as_deref()).await;
        if !session.is_broken() {
            *self.session_slot() = Some(session);
        }
        message.exchange
    }
}

#[async_trait]
impl PollingConsumer for MailPollingConsumer {
    async fn receive(&mut self, timeout: Duration) -> Result<Option<Exchange>, CamelError> {
        let deadline = Instant::now() + timeout;
        loop {
            let cached = self.session_slot().take();
            // Cancelling the pass at the deadline drops its session.
            let Ok(pass) = tokio::time::timeout_at(deadline, self.pass(cached)).await else {
                return Ok(None);
            };
            match pass {
                Pass::Found(session, picked) => {
                    return Ok(Some(self.hand_over(session, *picked).await));
                }
                Pass::Empty(session) => *self.session_slot() = Some(session),
                Pass::Failed(session, e) => {
                    *self.session_slot() = session;
                    return Err(CamelError::from(e));
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            tokio::time::sleep_until(deadline.min(now + self.mailbox.config.delay)).await;
        }
    }
}

#[cfg(test)]
mod tests;
