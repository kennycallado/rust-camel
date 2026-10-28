//! The IMAP poll pipeline shared by the push consumer and the
//! `PollingConsumer`: open a session, list the candidates of one pass, load
//! one message, and dispose of it after delivery.
//!
//! Session rule: an error for which [`MailError::is_retryable`] holds means
//! the connection is broken (a timeout abandons a command mid-flight), so
//! the session is dropped. A [`MailError::Refused`] reply leaves it usable.
//!
//! Idempotency rule: the Message-ID is `add`ed as soon as the message is
//! delivered (push: the route succeeded; pull: the exchange is handed to
//! the caller), before the disposition runs, so a failed `\Seen`, copy,
//! move or delete cannot cause a redelivery the repository would let
//! through.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use camel_api::IdempotentRepository;
use camel_component_api::RuntimeObservability;
use tracing::{debug, info, warn};

use crate::adapter::inbound;
use crate::adapter::inbound::InboundMessage;
use crate::config::ImapEndpointConfig;
use crate::error::MailError;
use crate::store::{MailStore, MailStoreConnector, StoreCapabilities};

/// Maximum UIDs requested in one size FETCH.
pub(crate) const SIZE_FETCH_CHUNK: usize = 500;

const LOGOUT_TIMEOUT: Duration = Duration::from_secs(2);

/// Closed set of `b-prime:mail:*` metric sites (`lint-metric-labels`).
#[derive(Clone, Copy)]
pub(crate) enum Site {
    Skip,
    Dispose,
    Idempotent,
}

/// Everything a consumer needs to reach one endpoint's folder.
#[derive(Clone)]
pub(crate) struct Mailbox {
    pub(crate) config: Arc<ImapEndpointConfig>,
    pub(crate) connector: Arc<dyn MailStoreConnector>,
    pub(crate) idempotent: Option<Arc<dyn IdempotentRepository>>,
}

impl Mailbox {
    /// Connect and probe the capabilities; fail closed when the server
    /// cannot honour the configured disposition.
    pub(crate) async fn open(&self) -> Result<Session, MailError> {
        let mut store = self.connector.connect().await?;
        let caps = match store.capabilities().await {
            Ok(caps) => check_capabilities(&self.config, caps).map(|()| caps),
            Err(e) => Err(e),
        };
        match caps {
            Ok(caps) => Ok(Session {
                store,
                caps,
                broken: false,
                proven: false,
                stale: false,
            }),
            Err(e) => {
                if !e.is_retryable() {
                    logout(store.as_mut()).await;
                }
                Err(e)
            }
        }
    }
}

/// An authenticated session whose capabilities satisfy the endpoint.
pub(crate) struct Session {
    store: Box<dyn MailStore>,
    caps: StoreCapabilities,
    /// Set when a disposition step broke the connection; the session must
    /// not be reused.
    broken: bool,
    /// At least one poll completed on this session.
    proven: bool,
    /// The `SELECT` opening a cycle on a proven session failed at the
    /// connection level: the server most likely dropped the idle session.
    stale: bool,
}

impl Session {
    pub(crate) fn is_broken(&self) -> bool {
        self.broken
    }

    /// Record a completed poll; a later connection-level failure on the
    /// first command of a cycle then counts as an idle drop.
    pub(crate) fn mark_proven(&mut self) {
        self.proven = true;
    }

    /// Whether the session broke on the first command of a cycle after it
    /// had completed a poll (an idle session the server dropped).
    pub(crate) fn went_stale(&self) -> bool {
        self.stale
    }

    /// Best-effort bounded `LOGOUT`.
    pub(crate) async fn close(mut self) {
        logout(self.store.as_mut()).await;
    }
}

async fn logout(store: &mut dyn MailStore) {
    let _ = tokio::time::timeout(LOGOUT_TIMEOUT, store.logout()).await;
}

/// Fail closed when the server cannot honour the configured disposition.
pub(crate) fn check_capabilities(
    cfg: &ImapEndpointConfig,
    caps: StoreCapabilities,
) -> Result<(), MailError> {
    if cfg.delete && !caps.uidplus {
        return Err(MailError::CapabilityMissing(
            "UIDPLUS (required by delete=true; a plain EXPUNGE would remove other clients' \\Deleted mail)"
                .to_string(),
        ));
    }
    if cfg.move_to.is_some() && !caps.move_ext && !caps.uidplus {
        return Err(MailError::CapabilityMissing(
            "MOVE or UIDPLUS (required by moveTo)".to_string(),
        ));
    }
    Ok(())
}

/// Per-process poll memory, keyed by `(uid_validity, uid)`.
#[derive(Debug, Default)]
pub(crate) struct PollState {
    uid_validity: Option<u32>,
    pub(crate) attempts: HashMap<(u32, u32), u32>,
    pub(crate) skipped: HashSet<(u32, u32)>,
}

impl PollState {
    /// Start a pass over the folder's `unseen` UIDs and return the ones not
    /// skipped, ascending. Entries for any other UIDVALIDITY or for UIDs no
    /// longer unseen are dropped, so the memory never outgrows the folder's
    /// unseen set and a UIDVALIDITY change discards it entirely.
    pub(crate) fn begin(&mut self, uid_validity: u32, unseen: Vec<u32>) -> Vec<u32> {
        if let Some(old) = self.uid_validity
            && old != uid_validity
        {
            info!(
                old,
                new = uid_validity,
                "mail folder UIDVALIDITY changed; cached UIDs discarded"
            );
        }
        self.uid_validity = Some(uid_validity);
        let current: HashSet<u32> = unseen.iter().copied().collect();
        let live = |key: &(u32, u32)| key.0 == uid_validity && current.contains(&key.1);
        self.attempts.retain(|key, _| live(key));
        self.skipped.retain(live);
        let mut candidates: Vec<u32> = current
            .into_iter()
            .filter(|uid| !self.skipped.contains(&(uid_validity, *uid)))
            .collect();
        candidates.sort_unstable();
        candidates
    }
}

/// Where skips and side-effect failures go: always a `warn!`, plus a
/// `b-prime:mail:*` error metric when there is a route to count against.
/// The `PollingConsumer` has no runtime observability, so it only logs.
#[derive(Clone, Copy)]
pub(crate) struct Reporter<'a> {
    metrics: Option<(&'a dyn RuntimeObservability, &'a str)>,
}

impl<'a> Reporter<'a> {
    pub(crate) fn route(rt: &'a dyn RuntimeObservability, route_id: &'a str) -> Self {
        Self {
            metrics: Some((rt, route_id)),
        }
    }

    pub(crate) fn log_only() -> Self {
        Self { metrics: None }
    }

    fn count(self, site: Site) {
        if let Some((rt, route_id)) = self.metrics {
            let metrics = rt.metrics();
            match site {
                Site::Skip => metrics.increment_errors(route_id, "b-prime:mail:skip"),
                Site::Dispose => metrics.increment_errors(route_id, "b-prime:mail:dispose"),
                Site::Idempotent => metrics.increment_errors(route_id, "b-prime:mail:idempotent"),
            }
        }
    }
}

/// Outcome of [`scan_sizes`]: both lists are in ascending UID order.
#[derive(Debug, Default)]
struct SizeScan {
    eligible: Vec<u32>,
    oversize: Vec<u32>,
}

/// Walk the ascending `candidates` in chunks of [`SIZE_FETCH_CHUNK`],
/// dropping oversize messages, and stop fetching sizes once `limit`
/// eligible messages are collected. UIDs the server omits from a listing
/// are logged and left alone for this pass.
async fn scan_sizes(
    store: &mut dyn MailStore,
    candidates: &[u32],
    max_message_size: u64,
    limit: usize,
) -> Result<SizeScan, MailError> {
    let mut scan = SizeScan::default();
    for chunk in candidates.chunks(SIZE_FETCH_CHUNK) {
        if scan.eligible.len() >= limit {
            break;
        }
        let mut sizes = store.fetch_sizes(chunk).await?;
        sizes.sort_by_key(|s| s.uid);
        let reported: HashSet<u32> = sizes.iter().map(|s| s.uid).collect();
        for uid in chunk.iter().filter(|uid| !reported.contains(uid)) {
            debug!(
                uid = *uid,
                "mail message missing from size listing; skipped this poll"
            );
        }
        for s in sizes {
            if s.size > max_message_size {
                scan.oversize.push(s.uid);
            } else {
                scan.eligible.push(s.uid);
            }
        }
    }
    scan.eligible.truncate(limit);
    Ok(scan)
}

/// A disposition step failed.
struct DisposeError {
    error: MailError,
    /// The failure came before `\Seen`, so a later pass fetches the message
    /// again.
    unseen: bool,
}

/// Success disposition from endpoint options only: `copyTo`, `\Seen`, then
/// `moveTo` (MOVE, else COPY plus the UID-scoped delete) or `delete`. The
/// first failing step stops the sequence. Never a plain `EXPUNGE`.
async fn dispose_success(
    store: &mut dyn MailStore,
    cfg: &ImapEndpointConfig,
    caps: StoreCapabilities,
    uid: u32,
) -> Result<(), DisposeError> {
    let before_seen = |error| DisposeError {
        error,
        unseen: true,
    };
    let after_seen = |error| DisposeError {
        error,
        unseen: false,
    };
    if let Some(folder) = &cfg.copy_to {
        store.copy(uid, folder).await.map_err(before_seen)?;
    }
    store.mark_seen(uid).await.map_err(before_seen)?;
    if let Some(folder) = &cfg.move_to {
        if caps.move_ext {
            store.move_to(uid, folder).await.map_err(after_seen)?;
        } else {
            store.copy(uid, folder).await.map_err(after_seen)?;
            store.delete_uid(uid).await.map_err(after_seen)?;
        }
    }
    if cfg.delete {
        store.delete_uid(uid).await.map_err(after_seen)?;
    }
    Ok(())
}

/// Outcome of [`Cycle::load`].
pub(crate) enum Loaded {
    /// A new message to deliver.
    Message(Box<InboundMessage>),
    /// Nothing to deliver: the message vanished, was skipped, or is a known
    /// duplicate (now `\Seen`).
    Nothing,
    /// The repository read failed. The failure is reported, the message is
    /// left untouched and it is not a failed attempt.
    RepositoryDown(MailError),
}

/// One pass over the folder on an open session.
pub(crate) struct Cycle<'a> {
    session: &'a mut Session,
    mailbox: &'a Mailbox,
    state: &'a mut PollState,
    reporter: Reporter<'a>,
    uid_validity: u32,
    candidates: Vec<u32>,
}

impl<'a> Cycle<'a> {
    /// `SELECT` the folder, `SEARCH UNSEEN`, and scan sizes until `limit`
    /// eligible messages are found. Oversize messages are skipped here,
    /// before any body is fetched.
    pub(crate) async fn begin(
        session: &'a mut Session,
        mailbox: &'a Mailbox,
        state: &'a mut PollState,
        reporter: Reporter<'a>,
        limit: usize,
    ) -> Result<Self, MailError> {
        let cfg = &mailbox.config;
        let uid_validity = match session.store.select(&cfg.folder).await {
            Ok(selected) => selected.uid_validity,
            Err(e) => {
                session.stale = session.proven && e.is_retryable();
                return Err(e);
            }
        };
        let store = session.store.as_mut();
        let unseen = store.search_unseen().await?;
        let candidates = state.begin(uid_validity, unseen);
        let scan = scan_sizes(store, &candidates, cfg.max_message_size, limit).await?;
        let mut cycle = Self {
            session,
            mailbox,
            state,
            reporter,
            uid_validity,
            candidates: scan.eligible,
        };
        for uid in scan.oversize {
            cycle.skip(uid, "maxMessageSize");
        }
        Ok(cycle)
    }

    /// Continue the pass that loaded a message on `session`, whose folder is
    /// still selected, without any I/O; the caller delivers that message.
    pub(crate) fn resume(
        session: &'a mut Session,
        mailbox: &'a Mailbox,
        state: &'a mut PollState,
        reporter: Reporter<'a>,
        uid_validity: u32,
    ) -> Self {
        Self {
            session,
            mailbox,
            state,
            reporter,
            uid_validity,
            candidates: Vec::new(),
        }
    }

    /// The UIDVALIDITY this pass runs under.
    pub(crate) fn uid_validity(&self) -> u32 {
        self.uid_validity
    }

    /// The eligible UIDs of this pass, ascending.
    pub(crate) fn take_candidates(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.candidates)
    }

    /// Fetch, size-check, parse and deduplicate one candidate. A failed
    /// body download counts toward `maxAttempts` and is returned.
    pub(crate) async fn load(&mut self, uid: u32) -> Result<Loaded, MailError> {
        let cfg = &self.mailbox.config;
        let raw = match self.session.store.fetch_body(uid).await {
            Ok(Some(raw)) => raw,
            Ok(None) => {
                debug!(uid, "mail message vanished before its body was fetched");
                return Ok(Loaded::Nothing);
            }
            Err(e) => {
                self.failed(uid);
                return Err(e);
            }
        };
        if raw.len() as u64 > cfg.max_message_size {
            self.skip(uid, "maxMessageSize");
            return Ok(Loaded::Nothing);
        }
        let Some(message) = inbound::parse_inbound(&raw, uid, &cfg.folder) else {
            self.skip(uid, "unparsable");
            return Ok(Loaded::Nothing);
        };
        let (Some(repo), Some(id)) = (
            self.mailbox.idempotent.as_deref(),
            message.message_id.as_deref(),
        ) else {
            return Ok(Loaded::Message(Box::new(message)));
        };
        match repo.contains(id).await {
            Ok(false) => Ok(Loaded::Message(Box::new(message))),
            Ok(true) => {
                debug!(uid, "duplicate Message-ID; marked seen without delivery");
                match self.session.store.mark_seen(uid).await {
                    Ok(()) => Ok(Loaded::Nothing),
                    Err(e) => {
                        warn!(uid, error = %e, "mail disposition failed");
                        self.reporter.count(Site::Dispose);
                        if e.is_retryable() {
                            Err(e)
                        } else {
                            Ok(Loaded::Nothing)
                        }
                    }
                }
            }
            Err(e) => {
                warn!(uid, error = %e, "idempotent repository read failed; message left untouched");
                self.reporter.count(Site::Idempotent);
                Ok(Loaded::RepositoryDown(MailError::Idempotent(e.to_string())))
            }
        }
    }

    /// Count one failed delivery attempt; at `maxAttempts` the message is
    /// skipped.
    pub(crate) fn failed(&mut self, uid: u32) {
        let key = (self.uid_validity, uid);
        let attempts = self.state.attempts.entry(key).or_insert(0);
        *attempts += 1;
        if *attempts >= self.mailbox.config.max_attempts {
            self.state.attempts.remove(&key);
            self.skip(uid, "maxAttempts");
        }
    }

    /// The message was delivered: `add` the Message-ID, then run the
    /// success disposition. A failed `add` is reported and does not stop
    /// the disposition. A failed step is reported; one that leaves the
    /// message unseen counts as a failed attempt, so `maxAttempts` bounds
    /// the redelivery. A connection-level failure marks the session broken
    /// and is returned.
    pub(crate) async fn delivered(
        &mut self,
        uid: u32,
        message_id: Option<&str>,
    ) -> Result<(), MailError> {
        if let (Some(repo), Some(id)) = (self.mailbox.idempotent.as_deref(), message_id)
            && let Err(e) = repo.add(id).await
        {
            warn!(uid, error = %e, "idempotent repository add failed");
            self.reporter.count(Site::Idempotent);
        }
        let disposed = dispose_success(
            self.session.store.as_mut(),
            &self.mailbox.config,
            self.session.caps,
            uid,
        )
        .await;
        if let Err(DisposeError { error, unseen }) = disposed {
            warn!(uid, error = %error, "mail disposition failed");
            self.reporter.count(Site::Dispose);
            if unseen {
                self.failed(uid);
            } else {
                self.state.attempts.remove(&(self.uid_validity, uid));
            }
            if error.is_retryable() {
                self.session.broken = true;
                return Err(error);
            }
            return Ok(());
        }
        self.state.attempts.remove(&(self.uid_validity, uid));
        Ok(())
    }

    fn skip(&mut self, uid: u32, reason: &'static str) {
        self.state.skipped.insert((self.uid_validity, uid));
        warn!(
            uid,
            reason,
            folder = %self.mailbox.config.folder,
            "mail message skipped for the process lifetime"
        );
        self.reporter.count(Site::Skip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_keeps_state_for_the_same_validity() {
        let mut state = PollState::default();
        state.skipped.insert((1, 5));
        state.attempts.insert((1, 6), 1);
        assert_eq!(state.begin(1, vec![6, 5, 7]), vec![6, 7]);
        assert_eq!(state.skipped, HashSet::from([(1, 5)]));
        assert_eq!(state.attempts, HashMap::from([((1, 6), 1)]));
    }

    #[test]
    fn begin_prunes_uids_that_left_the_unseen_set() {
        let mut state = PollState::default();
        state.skipped.insert((1, 5));
        state.attempts.insert((1, 6), 2);
        assert_eq!(state.begin(1, vec![7]), vec![7]);
        assert!(state.skipped.is_empty(), "{:?}", state.skipped);
        assert!(state.attempts.is_empty(), "{:?}", state.attempts);
    }

    #[test]
    fn begin_discards_state_of_another_validity() {
        let mut state = PollState::default();
        assert_eq!(state.begin(1, vec![5]), vec![5]);
        state.skipped.insert((1, 5));
        state.attempts.insert((1, 6), 1);
        assert_eq!(state.begin(2, vec![5, 6]), vec![5, 6]);
        assert!(state.skipped.is_empty());
        assert!(state.attempts.is_empty());
    }
}
