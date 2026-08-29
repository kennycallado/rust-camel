use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::platform::{
    LeadershipEvent, LeadershipHandle, LeadershipService, NoopReadinessGate, PlatformError,
    PlatformIdentity, PlatformService, ReadinessGate,
};
use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta};
use k8s_openapi::jiff::Span;
use k8s_openapi::jiff::Timestamp as JiffTimestamp;

pub fn ensure_rustls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
use kube::api::PostParams;
use kube::{Api, Client};
use tokio::sync::{Notify, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

use crate::identity::KubernetesPlatformIdentity;
use crate::leadership_fsm::{
    AttemptFailure, CycleAction, CycleOutcome, EpochUpdate, LoopState, ReconcileVerdict,
    StepDownReason, bound_attempt, budget_exhausted, clamp_epoch, decide, remaining_budget,
};

#[derive(Debug, Clone)]
pub struct KubernetesPlatformConfig {
    pub namespace: String,
    pub lease_name_prefix: String,
    pub lease_duration: Duration,
    pub renew_deadline: Duration,
    pub retry_period: Duration,
    pub jitter_factor: f64,
}

impl KubernetesPlatformConfig {
    /// Validate lease timing invariants.
    ///
    /// `renew_deadline` must be less than `lease_duration` to guarantee
    /// the holder can renew before the lease expires. `retry_period`
    /// must be less than `renew_deadline` to allow at least one retry.
    /// `lease_duration - renew_deadline` must be at least `retry_period`
    /// to leave one full retry window of renewal slack for clock skew
    /// and renew jitter.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.renew_deadline >= self.lease_duration {
            return Err(PlatformError::Config(format!(
                "renew_deadline ({:?}) must be less than lease_duration ({:?})",
                self.renew_deadline, self.lease_duration
            )));
        }
        if self.retry_period >= self.renew_deadline {
            return Err(PlatformError::Config(format!(
                "retry_period ({:?}) must be less than renew_deadline ({:?})",
                self.retry_period, self.renew_deadline
            )));
        }
        if self.lease_duration - self.renew_deadline < self.retry_period {
            return Err(PlatformError::Config(format!(
                "lease_duration ({:?}) minus renew_deadline ({:?}) must be >= retry_period ({:?}) to leave one retry window of renewal slack",
                self.lease_duration, self.renew_deadline, self.retry_period
            )));
        }
        if !(0.0..=1.0).contains(&self.jitter_factor) {
            return Err(PlatformError::Config(format!(
                "jitter_factor ({}) must be in [0.0, 1.0]",
                self.jitter_factor
            )));
        }
        Ok(())
    }
}

impl Default for KubernetesPlatformConfig {
    fn default() -> Self {
        Self {
            namespace: "".to_string(),
            lease_name_prefix: "camel-".to_string(),
            lease_duration: Duration::from_secs(15),
            renew_deadline: Duration::from_secs(10),
            retry_period: Duration::from_secs(2),
            jitter_factor: 0.2,
        }
    }
}

struct CachedLock {
    event_tx: watch::Sender<Option<LeadershipEvent>>,
    is_leader: Arc<AtomicBool>,
    leader_epoch: Arc<AtomicU64>,
    cancel: CancellationToken,
    ref_count: AtomicUsize,
    terminated: AtomicBool,
    terminated_notify: Notify,
}

impl CachedLock {
    fn new(
        event_tx: watch::Sender<Option<LeadershipEvent>>,
        is_leader: Arc<AtomicBool>,
        leader_epoch: Arc<AtomicU64>,
    ) -> Self {
        Self {
            event_tx,
            is_leader,
            leader_epoch,
            cancel: CancellationToken::new(),
            ref_count: AtomicUsize::new(0),
            terminated: AtomicBool::new(false),
            terminated_notify: Notify::new(),
        }
    }

    async fn wait_terminated(&self) {
        loop {
            let notified = self.terminated_notify.notified();
            if self.terminated.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

pub struct KubernetesLeadershipService {
    client: Client,
    config: KubernetesPlatformConfig,
    namespace: String,
    holder_identity: String,
    locks: Arc<Mutex<HashMap<String, Arc<CachedLock>>>>,
}

impl KubernetesLeadershipService {
    pub fn new(
        client: Client,
        identity: PlatformIdentity,
        config: KubernetesPlatformConfig,
    ) -> Result<Self, PlatformError> {
        config.validate()?;

        // Single empty-identity gate: a node without a node_id must never
        // compete for leadership.
        let namespace = canonical_namespace(&config, &identity);
        let holder_identity = holder_identity_string(&namespace, &identity.node_id)?;

        Ok(Self {
            client,
            config,
            namespace,
            holder_identity,
            locks: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn create_cached_handle(cached_lock: Arc<CachedLock>) -> LeadershipHandle {
        cached_lock.ref_count.fetch_add(1, Ordering::AcqRel);

        let handle_cancel = CancellationToken::new();
        let handle_cancel_wait = handle_cancel.clone();
        let (term_tx, term_rx) = oneshot::channel();
        let cached_for_bridge = Arc::clone(&cached_lock);

        tokio::spawn(async move {
            handle_cancel_wait.cancelled().await;

            let prev = cached_for_bridge.ref_count.fetch_sub(1, Ordering::AcqRel);
            if prev == 1 {
                cached_for_bridge.cancel.cancel();
                cached_for_bridge.wait_terminated().await;
            }
            let _ = term_tx.send(());
        });

        LeadershipHandle::new(
            cached_lock.event_tx.subscribe(),
            Arc::clone(&cached_lock.is_leader),
            Arc::clone(&cached_lock.leader_epoch),
            handle_cancel,
            term_rx,
        )
    }
}

#[async_trait]
impl LeadershipService for KubernetesLeadershipService {
    async fn start(&self, lock_name: &str) -> Result<LeadershipHandle, PlatformError> {
        if let Some(existing) = self
            .locks
            .lock()
            .expect("mutex poisoned: leadership locks map") // allow-unwrap
            .get(lock_name)
            .cloned()
            .filter(|lock| !lock.cancel.is_cancelled() && !lock.terminated.load(Ordering::Acquire))
        {
            return Ok(Self::create_cached_handle(existing));
        }

        let (event_tx, _event_rx) = watch::channel(None);
        let is_leader = Arc::new(AtomicBool::new(false));
        let leader_epoch = Arc::new(AtomicU64::new(0));
        let cached_lock = Arc::new(CachedLock::new(
            event_tx,
            Arc::clone(&is_leader),
            Arc::clone(&leader_epoch),
        ));

        {
            let mut locks = self
                .locks
                .lock()
                .expect("mutex poisoned: leadership locks map"); // allow-unwrap
            if let Some(existing) = locks.get(lock_name).cloned().filter(|lock| {
                !lock.cancel.is_cancelled() && !lock.terminated.load(Ordering::Acquire)
            }) {
                return Ok(Self::create_cached_handle(existing));
            }
            locks.insert(lock_name.to_string(), Arc::clone(&cached_lock));
        }

        let client = self.client.clone();
        let config = self.config.clone();
        let holder_identity = self.holder_identity.clone();
        let lock_name_owned = lock_name.to_string();
        let lease_name = format!("{}{}", config.lease_name_prefix, lock_name_owned);
        let namespace = self.namespace.clone();
        let cancel_task = cached_lock.cancel.clone();
        let is_leader_task = Arc::clone(&cached_lock.is_leader);
        let leader_epoch_task = Arc::clone(&cached_lock.leader_epoch);
        let event_tx_task = cached_lock.event_tx.clone();
        let cached_lock_task = Arc::clone(&cached_lock);
        let locks_map = Arc::clone(&self.locks);

        tokio::spawn(async move {
            let leases: Api<Lease> = Api::namespaced(client, &namespace);
            let mut state = LoopState {
                currently_leader: false,
                last_success: None,
            };
            let cancelled;

            // Side-effect applier for `CycleAction`: maps a decision to its
            // observable effects (is_leader flag, fencing epoch, leadership
            // events) and returns the sleep before the next cycle. `StepDown`
            // carries no sleep — it sleeps `retry_sleep` to re-enter the
            // contender path.
            let apply_action = |action: CycleAction, retry_sleep: Duration| -> Duration {
                match action {
                    CycleAction::BecomeLeader { term, sleep } => {
                        is_leader_task.store(true, Ordering::Release);
                        // Store the server-confirmed leader-term as the fencing
                        // epoch: a server-authoritative annotation counter on
                        // the Lease object, incremented on each takeover via
                        // optimistic concurrency — globally monotonic across
                        // pods. See ADR-0035.
                        leader_epoch_task.store(term, Ordering::Release);
                        tracing::debug!(
                            lease_name = %lease_name,
                            leader_epoch = term,
                            "leader epoch set from lease annotation"
                        );
                        let _ = event_tx_task.send(Some(LeadershipEvent::StartedLeading));
                        sleep
                    }
                    CycleAction::ContinueLeading { term, sleep } => {
                        // Renewal (still leader) — the stored epoch is
                        // clamped monotonic (`clamp_epoch`): a higher term
                        // is adopted, a lower one (deleted/recreated Lease)
                        // is ignored and logged in `note_renewal_epoch`.
                        // The stripped-annotation edge keeps the local
                        // epoch; the follower-path fallback to 1 for a
                        // missing term lives in `leadership_fsm::decide`'s
                        // BecomeLeader arm (ADR-0035).
                        note_renewal_epoch(&leader_epoch_task, term, &lease_name);
                        sleep
                    }
                    CycleAction::StepDown { reason } => {
                        warn!(
                            lease_name = %lease_name,
                            namespace = %namespace,
                            holder_identity = %holder_identity,
                            reason = ?reason,
                            "stepping down from leadership"
                        );
                        is_leader_task.store(false, Ordering::Release);
                        let _ = event_tx_task.send(Some(LeadershipEvent::StoppedLeading));
                        retry_sleep
                    }
                    CycleAction::SleepAcquiring { sleep } => sleep,
                }
            };

            loop {
                if cancel_task.is_cancelled() {
                    cancelled = true;
                    break;
                }

                let now = std::time::Instant::now();
                let retry_sleep = jittered_duration(config.retry_period, config.jitter_factor);

                // Pre-attempt fence: when the renewal budget is fully spent
                // before the cycle even starts, self-fence — apply the
                // `StepDown` side effects and skip the attempt. A partitioned
                // holder must not renew past its deadline.
                let sleep_for = if budget_exhausted(state.last_success, &config, now) {
                    let sleep = apply_action(
                        CycleAction::StepDown {
                            reason: StepDownReason::BudgetExhausted,
                        },
                        retry_sleep,
                    );
                    // `decide` owns the state clears on the outcome path; on
                    // the pre-attempt fence path the applier side must clear
                    // them, otherwise `budget_exhausted` stays true forever
                    // and the pod never re-enters the contender path.
                    state.currently_leader = false;
                    state.last_success = None;
                    sleep
                } else {
                    // Attempt budget: while leading, the remaining renewal
                    // budget; while contending, capped at `renew_deadline`.
                    let budget = remaining_budget(state.last_success, &config, now)
                        .unwrap_or(config.renew_deadline);

                    let outcome = match bound_attempt(
                        reconcile_lease(&leases, &lease_name, &config, &holder_identity),
                        budget,
                    )
                    .await
                    {
                        Ok(ReconcileVerdict::Acquired { term }) => CycleOutcome::Acquired { term },
                        Ok(ReconcileVerdict::Renewed { term }) => CycleOutcome::Renewed { term },
                        Ok(ReconcileVerdict::ForeignHolder) => CycleOutcome::Lost,
                        Ok(ReconcileVerdict::Conflict) => CycleOutcome::Conflict,
                        Err(AttemptFailure::Transport(err)) => {
                            warn!(
                                lease_name = %lease_name,
                                namespace = %namespace,
                                holder_identity = %holder_identity,
                                error = %err,
                                "leader election cycle failed"
                            );
                            CycleOutcome::Failed
                        }
                        Err(AttemptFailure::Deadline) => {
                            warn!(
                                lease_name = %lease_name,
                                namespace = %namespace,
                                holder_identity = %holder_identity,
                                error = "renewal budget elapsed before the server answered",
                                "leader election cycle failed"
                            );
                            CycleOutcome::Failed
                        }
                    };

                    let action = decide(
                        &mut state,
                        outcome,
                        &config,
                        retry_sleep,
                        std::time::Instant::now(),
                    );
                    apply_action(action, retry_sleep)
                };

                tokio::select! {
                    _ = cancel_task.cancelled() => {
                        cancelled = true;
                        break;
                    }
                    _ = tokio::time::sleep(sleep_for) => {}
                }
            }

            if !cancelled {
                // log-policy: system-broken
                error!(
                    lease_name = %lease_name,
                    namespace = %namespace,
                    holder_identity = %holder_identity,
                    "leader election loop terminated without cancellation"
                );
            }

            // Graceful shutdown release. The self-fence step-down does NOT
            // call release_lease: a partitioned holder cannot reach the
            // server, and a fenced holder re-enters the contender path.
            if state.currently_leader {
                if let Err(err) = release_lease(&leases, &lease_name, &holder_identity).await {
                    warn!(
                        lease_name = %lease_name,
                        namespace = %namespace,
                        holder_identity = %holder_identity,
                        error = %err,
                        "failed to release leadership lease"
                    );
                }
                is_leader_task.store(false, Ordering::Release);
                let _ = event_tx_task.send(Some(LeadershipEvent::StoppedLeading));
            }

            cached_lock_task.terminated.store(true, Ordering::Release);
            cached_lock_task.terminated_notify.notify_waiters();

            let mut locks = locks_map
                .lock()
                .expect("mutex poisoned: leadership locks map"); // allow-unwrap
            if locks
                .get(&lock_name_owned)
                .is_some_and(|current| Arc::ptr_eq(current, &cached_lock_task))
            {
                locks.remove(&lock_name_owned);
            }
        });

        Ok(Self::create_cached_handle(cached_lock))
    }
}

fn jittered_duration(base: Duration, jitter_factor: f64) -> Duration {
    let capped_ms = base.as_millis() as f64;
    if jitter_factor <= 0.0 || capped_ms <= 0.0 {
        return base;
    }
    let jitter = capped_ms * jitter_factor * (rand::random::<f64>() * 2.0 - 1.0);
    Duration::from_millis((capped_ms + jitter).max(0.0) as u64)
}

/// Apply a renewal-observed leader-term to the stored fencing epoch.
///
/// Loads the stored value, decides via `leadership_fsm::clamp_epoch`,
/// conditionally stores, and returns the prior value. Logging-free —
/// `note_renewal_epoch` owns the log surface.
/// Single-writer: only the per-lock leadership loop task may store;
/// readers load-only.
fn apply_renewal_epoch(leader_epoch: &AtomicU64, observed: Option<u64>) -> u64 {
    let prior = leader_epoch.load(Ordering::Acquire);
    if let EpochUpdate::Store(term) = clamp_epoch(prior, observed) {
        leader_epoch.store(term, Ordering::Release);
    }
    prior
}

/// Renewal-path epoch update: apply the clamp, then surface an observed
/// regression to the log.
///
/// A lower observed term proves the Lease was deleted and recreated (an
/// operator action); following it would regress the fencing epoch and
/// let a fenced writer pass the guard. Leadership itself is healthy, so
/// this is `warn!`, not an error.
fn note_renewal_epoch(leader_epoch: &AtomicU64, observed: Option<u64>, lease_name: &str) {
    let prior = apply_renewal_epoch(leader_epoch, observed);
    if let Some(term) = observed
        && term < prior
    {
        warn!(
            lease_name = %lease_name,
            prior_epoch = prior,
            observed_term = term,
            "ignoring epoch regression"
        );
    }
}

pub struct KubernetesPlatformService {
    identity: PlatformIdentity,
    readiness_gate: Arc<dyn ReadinessGate>,
    leadership: Arc<KubernetesLeadershipService>,
    health_source: Option<Arc<dyn camel_api::HealthSource>>,
    cancel_token: CancellationToken,
    health_poll_task: Option<JoinHandle<()>>,
}

impl KubernetesPlatformService {
    pub fn from_parts(
        identity: PlatformIdentity,
        readiness_gate: Arc<dyn ReadinessGate>,
        leadership: Arc<KubernetesLeadershipService>,
    ) -> Self {
        Self {
            identity,
            readiness_gate,
            leadership,
            health_source: None,
            cancel_token: CancellationToken::new(),
            health_poll_task: None,
        }
    }

    pub fn with_health_source(mut self, source: Arc<dyn camel_api::HealthSource>) -> Self {
        self.health_source = Some(Arc::clone(&source));

        let readiness_gate = Arc::clone(&self.readiness_gate);
        let cancel = self.cancel_token.clone();
        self.health_poll_task = Some(tokio::spawn(async move {
            loop {
                let status = source.readiness().await;
                match status {
                    camel_api::HealthStatus::Healthy | camel_api::HealthStatus::Degraded => {
                        if let Err(err) = readiness_gate.notify_ready().await {
                            warn!(error = %err, "failed to notify kubernetes readiness state");
                        }
                    }
                    // Unhealthy and any future variant keep the pod not-ready (fail closed).
                    _ => {
                        if let Err(err) = readiness_gate.notify_not_ready("Unhealthy").await {
                            warn!(error = %err, "failed to notify kubernetes readiness state");
                        }
                    }
                }

                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                    _ = cancel.cancelled() => break,
                }
            }
        }));

        self
    }

    /// Construct a `KubernetesPlatformService` from the default Kubernetes client and
    /// environment-detected identity.
    ///
    /// # Readiness gate fallback
    ///
    /// This method always installs [`NoopReadinessGate`] for the readiness gate, meaning
    /// pod readiness condition patches are **not** emitted. To enable cluster readiness
    /// checks, construct a [`KubernetesReadinessGate`](crate::KubernetesReadinessGate)
    /// manually and use [`from_parts`] instead.
    pub async fn try_default(config: KubernetesPlatformConfig) -> Result<Self, PlatformError> {
        config.validate()?;

        // Identity is resolved before the client so a missing identity surfaces as
        // `PlatformError::Config` instead of being masked by a client-availability failure.
        let identity: PlatformIdentity = KubernetesPlatformIdentity::try_from_env()
            .map_err(|err| {
                // log-policy: system-broken
                tracing::error!(error = %err, "kubernetes identity resolution failed");
                err
            })?
            .into_platform_identity();

        let _ = rustls::crypto::ring::default_provider().install_default();

        let client = Client::try_default().await.map_err(|err| {
            PlatformError::NotAvailable(format!("kubernetes client not available: {err}"))
        })?;

        let leadership = Arc::new(KubernetesLeadershipService::new(
            client,
            identity.clone(),
            config,
        )?);

        warn!(
            "Kubernetes client available, but using NoopReadinessGate — \
             cluster readiness checks disabled; use KubernetesReadinessGate for full integration"
        );

        Ok(Self::from_parts(
            identity,
            Arc::new(NoopReadinessGate),
            leadership,
        ))
    }
}

impl PlatformService for KubernetesPlatformService {
    fn identity(&self) -> PlatformIdentity {
        self.identity.clone()
    }

    fn readiness_gate(&self) -> Arc<dyn ReadinessGate> {
        Arc::clone(&self.readiness_gate)
    }

    fn leadership(&self) -> Arc<dyn LeadershipService> {
        Arc::clone(&self.leadership) as Arc<dyn LeadershipService>
    }
}

impl Drop for KubernetesPlatformService {
    fn drop(&mut self) {
        self.cancel_token.cancel();
        if let Some(task) = self.health_poll_task.take() {
            task.abort();
        }
    }
}

/// Canonical namespace for Lease objects: the first non-empty of
/// `config.namespace`, `identity.namespace`, then `"default"`.
fn canonical_namespace(config: &KubernetesPlatformConfig, identity: &PlatformIdentity) -> String {
    if !config.namespace.is_empty() {
        return config.namespace.clone();
    }

    if let Some(namespace) = identity.namespace.as_ref()
        && !namespace.is_empty()
    {
        return namespace.clone();
    }

    "default".to_string()
}

/// Namespaced Lease holder identity (`{namespace}/{node_id}`).
///
/// The single empty-identity gate for the whole service: a node without a
/// node_id must never compete for leadership.
fn holder_identity_string(namespace: &str, node_id: &str) -> Result<String, PlatformError> {
    if node_id.trim().is_empty() {
        return Err(PlatformError::Config(format!(
            "node_id must not be empty: a node without identity must not compete for leadership (namespace: {namespace})"
        )));
    }
    Ok(format!("{namespace}/{node_id}"))
}

/// Whether `spec` is currently held by `holder`.
fn holder_matches(spec: &LeaseSpec, holder: &str) -> bool {
    spec.holder_identity.as_deref() == Some(holder)
}

fn build_first_time_lease(
    lease_name: &str,
    holder_identity: &str,
    config: &KubernetesPlatformConfig,
    now: JiffTimestamp,
) -> Lease {
    let mut annotations = BTreeMap::new();
    annotations.insert(LEADER_TERM_ANNOTATION.to_string(), "1".to_string());
    let mut labels = BTreeMap::new();
    labels.insert("provider".to_string(), "camel".to_string());
    Lease {
        metadata: ObjectMeta {
            name: Some(lease_name.to_string()),
            labels: Some(labels),
            annotations: Some(annotations),
            ..ObjectMeta::default()
        },
        spec: Some(LeaseSpec {
            holder_identity: Some(holder_identity.to_string()),
            lease_duration_seconds: Some(config.lease_duration.as_secs() as i32),
            acquire_time: Some(MicroTime(now)),
            renew_time: Some(MicroTime(now)),
            ..LeaseSpec::default()
        }),
    }
}

async fn reconcile_lease(
    leases: &Api<Lease>,
    lease_name: &str,
    config: &KubernetesPlatformConfig,
    holder_identity: &str,
) -> Result<ReconcileVerdict, kube::Error> {
    let now = JiffTimestamp::now();

    let maybe_lease = leases.get_opt(lease_name).await?;
    let Some(mut lease) = maybe_lease else {
        // First-time create — initialize leader-term to 1.
        let lease = build_first_time_lease(lease_name, holder_identity, config, now);
        match leases.create(&PostParams::default(), &lease).await {
            Ok(created) => {
                return Ok(ReconcileVerdict::Acquired {
                    term: extract_leader_term(&created).unwrap_or(1),
                });
            }
            Err(err) if is_optimistic_conflict(&err) => {
                // Another contender created the lease first. This cycle loses leadership and retries.
                return Ok(ReconcileVerdict::Conflict);
            }
            Err(err) => return Err(err),
        }
    };

    let spec = lease.spec.clone().unwrap_or_default();
    let is_ours = holder_matches(&spec, holder_identity);

    if is_ours {
        // Ensure annotation exists (for leases created before this feature).
        // Missing annotation on renew → initialize to 1 so epoch is never 0.
        if extract_leader_term(&lease).is_none() {
            let annotations = lease.metadata.annotations.get_or_insert_with(BTreeMap::new);
            annotations.insert(LEADER_TERM_ANNOTATION.to_string(), "1".to_string());
        }
        // Renew — preserve the existing leader-term annotation unchanged.
        lease.spec = Some(LeaseSpec {
            holder_identity: Some(holder_identity.to_string()),
            lease_duration_seconds: Some(config.lease_duration.as_secs() as i32),
            acquire_time: spec.acquire_time,
            renew_time: Some(MicroTime(now)),
            ..spec
        });
        match leases
            .replace(lease_name, &PostParams::default(), &lease)
            .await
        {
            Ok(replaced) => {
                return Ok(ReconcileVerdict::Renewed {
                    term: extract_leader_term(&replaced),
                });
            }
            Err(err) if is_optimistic_conflict(&err) => {
                // Replace carries resourceVersion from the fetched lease; 409 means stale generation.
                return Ok(ReconcileVerdict::Conflict);
            }
            Err(err) => return Err(err),
        }
    }

    if lease_is_expired(&spec, now) {
        // Takeover — increment the leader-term from the current annotation.
        // A missing or malformed annotation is treated as 0 (so the new term is 1).
        let current_term = match extract_leader_term(&lease) {
            Some(term) => term,
            None => {
                let raw = lease
                    .metadata
                    .annotations
                    .as_ref()
                    .and_then(|a| a.get(LEADER_TERM_ANNOTATION));
                if raw.is_some() {
                    warn!(
                        lease_name = %lease_name,
                        annotation = ?raw,
                        "malformed camel.io/leader-term annotation, resetting to 1"
                    );
                }
                0
            }
        };
        let new_term = current_term + 1;

        // Ensure annotations map exists, then write the incremented term.
        let annotations = lease.metadata.annotations.get_or_insert_with(BTreeMap::new);
        annotations.insert(LEADER_TERM_ANNOTATION.to_string(), new_term.to_string());

        lease.spec = Some(LeaseSpec {
            holder_identity: Some(holder_identity.to_string()),
            lease_duration_seconds: Some(config.lease_duration.as_secs() as i32),
            acquire_time: Some(MicroTime(now)),
            renew_time: Some(MicroTime(now)),
            ..spec
        });
        match leases
            .replace(lease_name, &PostParams::default(), &lease)
            .await
        {
            Ok(replaced) => {
                return Ok(ReconcileVerdict::Acquired {
                    term: extract_leader_term(&replaced).unwrap_or(1),
                });
            }
            Err(err) if is_optimistic_conflict(&err) => {
                // Lease changed between read and replace; treat as lost race and retry next cycle.
                return Ok(ReconcileVerdict::Conflict);
            }
            Err(err) => return Err(err),
        }
    }

    Ok(ReconcileVerdict::ForeignHolder)
}

/// Server-authoritative annotation counter on the Lease — incremented on each
/// takeover via optimistic concurrency. Globally monotonic across pods. See
/// ADR-0035 for full design.
const LEADER_TERM_ANNOTATION: &str = "camel.io/leader-term";

/// Read the leader-term annotation from a Lease. Returns `None` if the
/// annotation is missing, not a valid `u64`, or zero (epoch 0 = no leader).
fn extract_leader_term(lease: &Lease) -> Option<u64> {
    lease
        .metadata
        .annotations
        .as_ref()
        .and_then(|anns| anns.get(LEADER_TERM_ANNOTATION))
        .and_then(|v| v.parse().ok())
        .filter(|&term| term > 0)
}

fn lease_is_expired(spec: &LeaseSpec, now: JiffTimestamp) -> bool {
    let Some(lease_duration_seconds) = spec.lease_duration_seconds else {
        return true;
    };
    let Some(last_renewal) = spec.renew_time.as_ref().or(spec.acquire_time.as_ref()) else {
        return true;
    };
    let expires_at = last_renewal.0 + Span::new().seconds(lease_duration_seconds as i64);
    expires_at < now
}

async fn release_lease(
    leases: &Api<Lease>,
    lease_name: &str,
    holder_identity: &str,
) -> Result<(), kube::Error> {
    let Some(mut lease) = leases.get_opt(lease_name).await? else {
        return Ok(());
    };

    let spec = lease.spec.clone().unwrap_or_default();

    // Only release if we still hold this lease.
    if !holder_matches(&spec, holder_identity) {
        return Ok(());
    }

    // Expire the lease by setting renewTime to the unix epoch.
    // We do NOT delete the lease — this preserves the camel.io/leader-term
    // annotation so the next acquirer increments from the last value
    // (global monotonicity, ADR-0035).
    let expired_time =
        MicroTime(JiffTimestamp::from_second(0).unwrap_or_else(|_| JiffTimestamp::now()));
    lease.spec = Some(LeaseSpec {
        holder_identity: Some(holder_identity.to_string()),
        renew_time: Some(expired_time),
        ..spec
    });

    match leases
        .replace(lease_name, &PostParams::default(), &lease)
        .await
    {
        Ok(_) => Ok(()),
        Err(err) if is_optimistic_conflict(&err) || is_not_found(&err) => Ok(()),
        Err(err) => Err(err),
    }
}

fn is_optimistic_conflict(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(resp) if resp.code == 409)
}

fn is_not_found(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(resp) if resp.code == 404)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use super::*;
    use kube::core::Status;
    use kube::core::response::StatusSummary;
    use tracing_test::traced_test;

    #[test]
    fn default_config_leaves_namespace_empty_to_enable_fallback_chain() {
        assert!(KubernetesPlatformConfig::default().namespace.is_empty());
        assert_eq!(KubernetesPlatformConfig::default().jitter_factor, 0.2);
    }

    #[test]
    fn first_time_lease_carries_provider_label_and_term_annotation() {
        let config = KubernetesPlatformConfig::default();
        let lease = build_first_time_lease("test-lock", "ns/node-a", &config, JiffTimestamp::now());

        assert_eq!(lease.metadata.name, Some("test-lock".to_string()));
        assert_eq!(
            lease.metadata.labels,
            Some(BTreeMap::from([(
                "provider".to_string(),
                "camel".to_string()
            )]))
        );
        assert_eq!(
            lease.metadata.annotations,
            Some(BTreeMap::from([(
                LEADER_TERM_ANNOTATION.to_string(),
                "1".to_string(),
            )]))
        );
        let spec = lease.spec.expect("first-time lease has a spec");
        assert_eq!(spec.holder_identity, Some("ns/node-a".to_string()));
        assert_eq!(
            spec.lease_duration_seconds,
            Some(config.lease_duration.as_secs() as i32)
        );
    }

    #[test]
    fn conflict_classification_is_explicit_for_409_api_errors() {
        let err = kube::Error::Api(Box::new(Status {
            status: Some(StatusSummary::Failure),
            message: "conflict".to_string(),
            reason: "Conflict".to_string(),
            code: 409,
            metadata: None,
            details: None,
        }));

        assert!(is_optimistic_conflict(&err));
    }

    #[test]
    fn config_rejects_renew_deadline_gte_lease_duration() {
        let config = KubernetesPlatformConfig {
            namespace: "default".to_string(),
            lease_name_prefix: "camel-".to_string(),
            lease_duration: Duration::from_secs(10),
            renew_deadline: Duration::from_secs(10),
            retry_period: Duration::from_secs(2),
            jitter_factor: 0.2,
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("renew_deadline"));
    }

    #[test]
    fn config_rejects_retry_period_gte_renew_deadline() {
        let config = KubernetesPlatformConfig {
            namespace: "default".to_string(),
            lease_name_prefix: "camel-".to_string(),
            lease_duration: Duration::from_secs(15),
            renew_deadline: Duration::from_secs(5),
            retry_period: Duration::from_secs(5),
            jitter_factor: 0.2,
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("retry_period"));
    }

    #[test]
    fn config_rejects_jitter_out_of_bounds() {
        let config = KubernetesPlatformConfig {
            jitter_factor: 1.1,
            ..KubernetesPlatformConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("jitter_factor"));
    }

    #[test]
    fn validate_defaults_pass() {
        let config = KubernetesPlatformConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_insufficient_slack_rejected() {
        let config = KubernetesPlatformConfig {
            lease_duration: Duration::from_secs(12),
            renew_deadline: Duration::from_secs(11),
            retry_period: Duration::from_secs(2),
            ..KubernetesPlatformConfig::default()
        };
        let err = config.validate().unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("lease_duration"),
            "missing lease_duration: {message}"
        );
        assert!(
            message.contains("renew_deadline"),
            "missing renew_deadline: {message}"
        );
        assert!(
            message.contains("retry_period"),
            "missing retry_period: {message}"
        );
    }

    #[test]
    fn validate_slack_equal_to_retry_period_passes() {
        let config = KubernetesPlatformConfig {
            lease_duration: Duration::from_secs(12),
            renew_deadline: Duration::from_secs(10),
            retry_period: Duration::from_secs(2),
            ..KubernetesPlatformConfig::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn jittered_duration_with_zero_factor_is_stable() {
        let base = Duration::from_millis(750);
        assert_eq!(jittered_duration(base, 0.0), base);
    }

    #[test]
    fn canonical_namespace_prefers_config() {
        let config = KubernetesPlatformConfig {
            namespace: "prod".to_string(),
            ..KubernetesPlatformConfig::default()
        };
        let identity = PlatformIdentity {
            node_id: "pod-a".to_string(),
            namespace: Some("other".to_string()),
            labels: HashMap::new(),
        };

        assert_eq!(canonical_namespace(&config, &identity), "prod");
    }

    #[test]
    fn canonical_namespace_uses_identity_when_config_empty() {
        let config = KubernetesPlatformConfig {
            namespace: "".to_string(),
            ..KubernetesPlatformConfig::default()
        };
        let identity = PlatformIdentity {
            node_id: "pod-a".to_string(),
            namespace: Some("staging".to_string()),
            labels: HashMap::new(),
        };

        assert_eq!(canonical_namespace(&config, &identity), "staging");
    }

    #[test]
    fn canonical_namespace_defaults() {
        let config = KubernetesPlatformConfig {
            namespace: "".to_string(),
            ..KubernetesPlatformConfig::default()
        };
        let identity = PlatformIdentity {
            node_id: "pod-a".to_string(),
            namespace: None,
            labels: HashMap::new(),
        };

        assert_eq!(canonical_namespace(&config, &identity), "default");
    }

    #[test]
    fn holder_identity_string_formats_namespaced() {
        assert_eq!(
            holder_identity_string("prod", "my-pod").unwrap(),
            "prod/my-pod"
        );
    }

    #[test]
    fn holder_identity_string_rejects_empty_node_id() {
        for node_id in ["", "   "] {
            let err = holder_identity_string("default", node_id).unwrap_err();
            assert!(
                err.to_string().contains("must not compete for leadership"),
                "unexpected error for node_id {node_id:?}: {err}"
            );
        }
    }

    #[test]
    fn holder_matches_round_trip() {
        let spec = LeaseSpec {
            holder_identity: Some("default/pod-a".to_string()),
            ..LeaseSpec::default()
        };

        assert!(holder_matches(&spec, "default/pod-a"));
        assert!(!holder_matches(&spec, "default/pod-b"));
        assert!(!holder_matches(&spec, "pod-a"));

        // A spec holding the empty string matches only the empty string —
        // documents why construction rejects empty identities.
        let empty_spec = LeaseSpec {
            holder_identity: Some(String::new()),
            ..LeaseSpec::default()
        };
        assert!(holder_matches(&empty_spec, ""));
        assert!(!holder_matches(&empty_spec, "default/pod-a"));
    }

    #[test]
    fn holder_matches_none_never_matches() {
        let spec = LeaseSpec {
            holder_identity: None,
            ..LeaseSpec::default()
        };

        assert!(!holder_matches(&spec, "default/pod-a"));
        assert!(!holder_matches(&spec, "pod-a"));
        assert!(!holder_matches(&spec, ""));
    }

    // --- renewal-path epoch monotonicity (k8s-lease-epoch) ---

    #[test]
    fn apply_renewal_epoch_none_keeps() {
        let epoch = AtomicU64::new(7);

        let prior = apply_renewal_epoch(&epoch, None);

        assert_eq!(prior, 7);
        assert_eq!(epoch.load(Ordering::Acquire), 7);
    }

    #[test]
    fn apply_renewal_epoch_equal_keeps() {
        let epoch = AtomicU64::new(7);

        let prior = apply_renewal_epoch(&epoch, Some(7));

        assert_eq!(prior, 7);
        assert_eq!(epoch.load(Ordering::Acquire), 7);
    }

    #[test]
    fn apply_renewal_epoch_increase_stores() {
        let epoch = AtomicU64::new(7);

        let prior = apply_renewal_epoch(&epoch, Some(9));

        assert_eq!(prior, 7);
        assert_eq!(epoch.load(Ordering::Acquire), 9);
    }

    #[test]
    fn apply_renewal_epoch_regression_keeps() {
        let epoch = AtomicU64::new(7);

        let prior = apply_renewal_epoch(&epoch, Some(1));

        assert_eq!(prior, 7);
        assert_eq!(epoch.load(Ordering::Acquire), 7);
    }

    /// Serializes the captured-log tests: `tracing-test`'s global buffer
    /// is process-wide and is not cleared between tests, so concurrent
    /// emission would make the zero-record assertion racy.
    static LOG_CAPTURE_LOCK: Mutex<()> = Mutex::new(());

    fn lock_log_capture() -> std::sync::MutexGuard<'static, ()> {
        // Poison-resilient: a failed assertion in one log test must not
        // cascade a PoisonError into its sibling.
        LOG_CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn clear_captured_logs() {
        let mut buf = tracing_test::internal::global_buf()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        buf.clear();
    }

    /// Return the captured log record lines whose message contains every
    /// needle. `#[traced_test]` writes each record as one line into the
    /// shared global buffer, so line matching approximates per-record
    /// message matching. Each returned line carries the level (e.g. ` WARN `)
    /// and the structured `key=value` fields, so callers can pin both.
    fn captured_records_containing(needles: &[&str]) -> Vec<String> {
        let buf = tracing_test::internal::global_buf()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        String::from_utf8_lossy(&buf)
            .lines()
            .filter(|line| needles.iter().all(|needle| line.contains(needle)))
            .map(str::to_owned)
            .collect()
    }

    #[traced_test]
    #[test]
    fn note_renewal_epoch_regression_logs_warning() {
        let _guard = lock_log_capture();
        clear_captured_logs();

        let epoch = AtomicU64::new(7);
        note_renewal_epoch(&epoch, Some(1), "test-lease");

        assert_eq!(epoch.load(Ordering::Acquire), 7);
        let records = captured_records_containing(&["ignoring epoch regression"]);
        assert_eq!(records.len(), 1, "expected exactly one regression record");
        let record = &records[0];
        // Level pin: the emission must stay a warning — demoting it to
        // `info!` fails here because the captured line carries the level.
        assert!(
            record.contains(" WARN "),
            "regression record is not WARN level: {record}"
        );
        // Structured-field pin: the record must carry the regression
        // context as `key=value` fields, not just in its message text.
        assert!(
            record.contains("prior_epoch=7"),
            "missing prior_epoch=7 field: {record}"
        );
        assert!(
            record.contains("observed_term=1"),
            "missing observed_term=1 field: {record}"
        );
        assert!(
            record.contains("lease_name=test-lease"),
            "missing lease_name=test-lease field: {record}"
        );
    }

    #[traced_test]
    #[test]
    fn note_renewal_epoch_equal_and_none_emit_no_epoch_log() {
        let _guard = lock_log_capture();
        clear_captured_logs();

        let epoch = AtomicU64::new(7);
        note_renewal_epoch(&epoch, Some(7), "test-lease");
        note_renewal_epoch(&epoch, None, "test-lease");

        assert_eq!(epoch.load(Ordering::Acquire), 7);
        let regressions = captured_records_containing(&["ignoring epoch regression"]);
        assert_eq!(regressions.len(), 0);
        // Spec predicate: zero records whose message contains both
        // "test-lease" and "epoch" — no epoch-update, no regression log.
        let epoch_records = captured_records_containing(&["test-lease", "epoch"]);
        assert_eq!(epoch_records.len(), 0);
    }

    // --- wiring harness: in-process fake Kubernetes API (k8s-lease-epoch) ---

    type FakeApiResponse = http::Response<kube::client::Body>;

    /// Minimal in-memory coordination.k8s.io API server. Serves exactly the
    /// operations `reconcile_lease` / `release_lease` issue against
    /// `Api<Lease>` (GET item, POST collection, PUT item), so the REAL
    /// leadership-loop task spawned by `start` can be driven end-to-end
    /// without a cluster.
    #[derive(Clone, Default)]
    struct FakeLeaseApi {
        cluster: Arc<Mutex<FakeCluster>>,
    }

    #[derive(Default)]
    struct FakeCluster {
        lease: Option<Lease>,
        resource_version: u64,
        method_paths: Vec<String>,
    }

    impl FakeCluster {
        /// Store `lease` with a fresh resourceVersion, mirroring the
        /// apiserver's write path, and return the stored object.
        fn store(&mut self, mut lease: Lease) -> Lease {
            self.resource_version += 1;
            lease.metadata.resource_version = Some(self.resource_version.to_string());
            self.lease = Some(lease.clone());
            lease
        }

        fn route(&mut self, method: &str, path: &str, body: &[u8]) -> FakeApiResponse {
            self.method_paths.push(format!("{method} {path}"));
            let is_item = path.contains("/leases/");
            match method {
                "GET" if is_item => match &self.lease {
                    Some(lease) => fake_json_response(200, lease),
                    None => fake_status_response(404),
                },
                "POST" if !is_item => {
                    let lease: Lease = serde_json::from_slice(body)
                        .expect("fake api: create body parses as Lease");
                    let stored = self.store(lease);
                    fake_json_response(201, &stored)
                }
                "PUT" if is_item => {
                    let lease: Lease = serde_json::from_slice(body)
                        .expect("fake api: replace body parses as Lease");
                    let stored = self.store(lease);
                    fake_json_response(200, &stored)
                }
                _ => fake_status_response(404),
            }
        }
    }

    fn fake_json_response(status: u16, lease: &Lease) -> FakeApiResponse {
        let body = serde_json::to_vec(lease).expect("lease serializes");
        http::Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(kube::client::Body::from(body))
            .expect("static response parts")
    }

    fn fake_status_response(status: u16) -> FakeApiResponse {
        // Shapes a real apiserver 404 Status so `get_opt` maps it to `None`.
        let body: &[u8] = br#"{
            "kind": "Status",
            "apiVersion": "v1",
            "metadata": {},
            "status": "Failure",
            "message": "lease not found",
            "reason": "NotFound",
            "code": 404
        }"#;
        http::Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(kube::client::Body::from(body.to_vec()))
            .expect("static response parts")
    }

    impl tower::Service<http::Request<kube::client::Body>> for FakeLeaseApi {
        type Response = FakeApiResponse;
        type Error = tower::BoxError;
        type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, req: http::Request<kube::client::Body>) -> Self::Future {
            let cluster = Arc::clone(&self.cluster);
            Box::pin(async move {
                let method = req.method().clone();
                let path = req.uri().path().to_string();
                let body = req
                    .into_body()
                    .collect_bytes()
                    .await
                    .map_err(tower::BoxError::from)?;
                let mut cluster = cluster
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                Ok(cluster.route(method.as_str(), &path, &body))
            })
        }
    }

    impl FakeLeaseApi {
        /// Operator action: rewrite `camel.io/leader-term` on the stored
        /// Lease (fleet history this pod did not observe directly).
        fn operator_set_leader_term(&self, term: u64) {
            let mut cluster = self
                .cluster
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let lease = cluster
                .lease
                .as_mut()
                .expect("operator_set_leader_term: no lease stored");
            let annotations = lease.metadata.annotations.get_or_insert_with(BTreeMap::new);
            annotations.insert(LEADER_TERM_ANNOTATION.to_string(), term.to_string());
            cluster.resource_version += 1;
        }

        /// Operator action: delete the Lease outright — the delete/recreate
        /// regression source this change guards against.
        fn operator_delete_lease(&self) {
            let mut cluster = self
                .cluster
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cluster.lease = None;
        }

        fn request_count(&self, method: &str) -> usize {
            let cluster = self
                .cluster
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cluster
                .method_paths
                .iter()
                .filter(|entry| entry.starts_with(method))
                .count()
        }
    }

    /// Poll `cond` until true or `timeout` elapses (5 ms cadence).
    async fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + timeout;
        while !cond() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "condition not met within {timeout:?}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Wiring test through the REAL leadership loop (`start`'s spawned task
    /// and `apply_action`'s `ContinueLeading` arm): a still-leading pod that
    /// recreates a deleted Lease at a lower term must not regress its
    /// fencing epoch, and the ignored regression must be warned.
    // The capture lock is a std mutex held for the whole async test on
    // purpose: it serializes the leadership loop's log emission against the
    // sibling captured-log tests (same pattern as route_controller_tests).
    #[allow(clippy::await_holding_lock)]
    #[traced_test]
    #[tokio::test]
    async fn recreate_after_delete_does_not_regress_term() {
        let _guard = lock_log_capture();

        let config = KubernetesPlatformConfig {
            namespace: "epoch-test".to_string(),
            lease_name_prefix: "camel-".to_string(),
            lease_duration: Duration::from_secs(15),
            renew_deadline: Duration::from_secs(10),
            retry_period: Duration::from_millis(20),
            jitter_factor: 0.0,
        };
        let fake = FakeLeaseApi::default();
        let client = kube::Client::new(fake.clone(), "epoch-test");
        let identity = PlatformIdentity::local("node-a");
        let service =
            KubernetesLeadershipService::new(client, identity, config).expect("leadership config");
        let handle = service.start("orders").await.expect("leadership handle");

        // Arrange: first-time acquire at term 1, then the fleet term jumps
        // to 7 and the next renewal adopts it (clamp_epoch increase).
        wait_until(Duration::from_secs(5), || {
            handle.is_leader() && handle.leader_epoch() == 1
        })
        .await;
        fake.operator_set_leader_term(7);
        wait_until(Duration::from_secs(5), || handle.leader_epoch() == 7).await;

        // From here on only the regression emission may reach the capture
        // buffer: renewals at term 7 are silent, the regression warns.
        clear_captured_logs();
        let creates_before_delete = fake.request_count("POST");

        // Act: operator deletes the Lease; the still-leading pod recreates
        // it at camel.io/leader-term=1 in its next cycle.
        fake.operator_delete_lease();
        wait_until(Duration::from_secs(5), || {
            fake.request_count("POST") > creates_before_delete
        })
        .await;
        // Let the recreation cycle land (ContinueLeading{Some(1)}) and at
        // least one renewal at term 1 run afterwards.
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Assert: the epoch handle never regressed below 7 and the ignored
        // regression was captured as a WARN with its structured fields.
        assert_eq!(
            handle.leader_epoch(),
            7,
            "fencing epoch must not regress after delete/recreate"
        );
        assert!(handle.is_leader(), "leadership must survive the regression");
        let records = captured_records_containing(&["ignoring epoch regression"]);
        assert!(
            !records.is_empty(),
            "expected at least one epoch-regression warning after recreate"
        );
        assert!(
            records.iter().all(|record| record.contains(" WARN ")),
            "every regression record must be WARN level: {records:?}"
        );
        assert!(
            records.iter().any(
                |record| record.contains("prior_epoch=7") && record.contains("observed_term=1")
            ),
            "regression record must carry prior_epoch=7 and observed_term=1: {records:?}"
        );

        handle.step_down().await.expect("step down");
    }
}
