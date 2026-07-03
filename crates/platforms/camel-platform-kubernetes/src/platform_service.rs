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
    identity: PlatformIdentity,
    config: KubernetesPlatformConfig,
    locks: Arc<Mutex<HashMap<String, Arc<CachedLock>>>>,
}

impl KubernetesLeadershipService {
    pub fn new(
        client: Client,
        identity: PlatformIdentity,
        config: KubernetesPlatformConfig,
    ) -> Result<Self, PlatformError> {
        config.validate()?;
        Ok(Self {
            client,
            identity,
            config,
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
        let holder_identity = self.identity.node_id.clone();
        let lock_name_owned = lock_name.to_string();
        let lease_name = format!("{}{}", config.lease_name_prefix, lock_name_owned);
        let namespace = resolve_lease_namespace(&config, &self.identity);
        let cancel_task = cached_lock.cancel.clone();
        let is_leader_task = Arc::clone(&cached_lock.is_leader);
        let leader_epoch_task = Arc::clone(&cached_lock.leader_epoch);
        let event_tx_task = cached_lock.event_tx.clone();
        let cached_lock_task = Arc::clone(&cached_lock);
        let locks_map = Arc::clone(&self.locks);

        tokio::spawn(async move {
            let leases: Api<Lease> = Api::namespaced(client, &namespace);
            let mut currently_leader = false;
            #[allow(unused_assignments)]
            let mut cancelled = false;

            loop {
                if cancel_task.is_cancelled() {
                    cancelled = true;
                    break;
                }

                let cycle_start = std::time::Instant::now();

                let (leader_now, confirmed_term) =
                    match reconcile_lease(&leases, &lease_name, &config, &holder_identity).await {
                        Ok((value, term)) => (value, term),
                        Err(err) => {
                            warn!(
                                lease_name = %lease_name,
                                namespace = %namespace,
                                holder_identity = %holder_identity,
                                error = %err,
                                "leader election cycle failed"
                            );
                            (false, None)
                        }
                    };

                if leader_now != currently_leader {
                    currently_leader = leader_now;
                    is_leader_task.store(leader_now, Ordering::Release);

                    if leader_now {
                        // Store the server-confirmed leader-term as the fencing epoch.
                        // The term is a server-authoritative annotation counter on the
                        // Lease object, incremented on each takeover via optimistic
                        // concurrency — globally monotonic across pods. See ADR-0035.
                        // Fallback to 1 if server stripped the annotation (defensive).
                        let term = confirmed_term.unwrap_or(1);
                        leader_epoch_task.store(term, Ordering::Release);
                        tracing::debug!(
                            lease_name = %lease_name,
                            leader_epoch = term,
                            "leader epoch set from lease annotation"
                        );
                    }

                    let event = if leader_now {
                        LeadershipEvent::StartedLeading
                    } else {
                        LeadershipEvent::StoppedLeading
                    };
                    let _ = event_tx_task.send(Some(event));
                } else if leader_now {
                    // Renewal (still leader) — server should return the same term.
                    // Update defensively if it ever differs.
                    if let Some(term) = confirmed_term {
                        let current = leader_epoch_task.load(Ordering::Acquire);
                        if term != current {
                            leader_epoch_task.store(term, Ordering::Release);
                        }
                    }
                }

                // Deadline-scheduled sleep: when leading, subtract cycle
                // elapsed time from renew_deadline. If the cycle took longer
                // than renew_deadline, sleep is 0 and the next cycle starts
                // immediately, detecting lease expiry and stepping down.
                let sleep_for = next_cycle_sleep(currently_leader, cycle_start.elapsed(), &config);

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

            if currently_leader {
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

/// Compute the sleep duration before the next renew cycle.
///
/// When leading: `renew_deadline.saturating_sub(cycle_elapsed)` — if the
/// reconcile cycle took longer than `renew_deadline`, the next cycle
/// starts immediately (sleep = 0), detecting lease expiry and stepping
/// down without waiting for a fresh slot.
///
/// When not leading: jittered `retry_period` (avoids thundering herd).
fn next_cycle_sleep(
    is_leader: bool,
    cycle_elapsed: Duration,
    config: &KubernetesPlatformConfig,
) -> Duration {
    if is_leader {
        config.renew_deadline.saturating_sub(cycle_elapsed)
    } else {
        jittered_duration(config.retry_period, config.jitter_factor)
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
                    camel_api::HealthStatus::Unhealthy => {
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

        let _ = rustls::crypto::ring::default_provider().install_default();

        let client = Client::try_default().await.map_err(|err| {
            PlatformError::NotAvailable(format!("kubernetes client not available: {err}"))
        })?;

        let identity: PlatformIdentity = KubernetesPlatformIdentity::from_env().into();
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

fn resolve_lease_namespace(
    config: &KubernetesPlatformConfig,
    identity: &PlatformIdentity,
) -> String {
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

async fn reconcile_lease(
    leases: &Api<Lease>,
    lease_name: &str,
    config: &KubernetesPlatformConfig,
    holder_identity: &str,
) -> Result<(bool, Option<u64>), kube::Error> {
    let now = JiffTimestamp::now();

    let maybe_lease = leases.get_opt(lease_name).await?;
    let Some(mut lease) = maybe_lease else {
        // First-time create — initialize leader-term to 1.
        let mut annotations = BTreeMap::new();
        annotations.insert(LEADER_TERM_ANNOTATION.to_string(), "1".to_string());
        let lease = Lease {
            metadata: ObjectMeta {
                name: Some(lease_name.to_string()),
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
        };
        match leases.create(&PostParams::default(), &lease).await {
            Ok(created) => {
                return Ok((true, extract_leader_term(&created)));
            }
            Err(err) if is_optimistic_conflict(&err) => {
                // Another contender created the lease first. This cycle loses leadership and retries.
                return Ok((false, None));
            }
            Err(err) => return Err(err),
        }
    };

    let spec = lease.spec.clone().unwrap_or_default();
    let holder = spec.holder_identity.as_deref();
    let is_ours = holder == Some(holder_identity);

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
            Ok(replaced) => return Ok((true, extract_leader_term(&replaced))),
            Err(err) if is_optimistic_conflict(&err) => {
                // Replace carries resourceVersion from the fetched lease; 409 means stale generation.
                return Ok((false, None));
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
            Ok(replaced) => return Ok((true, extract_leader_term(&replaced))),
            Err(err) if is_optimistic_conflict(&err) => {
                // Lease changed between read and replace; treat as lost race and retry next cycle.
                return Ok((false, None));
            }
            Err(err) => return Err(err),
        }
    }

    Ok((false, None))
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
    if spec.holder_identity.as_deref() != Some(holder_identity) {
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

    use super::*;
    use kube::core::Status;
    use kube::core::response::StatusSummary;

    #[test]
    fn namespace_resolution_uses_explicit_config_namespace_first() {
        let config = KubernetesPlatformConfig {
            namespace: "configured-ns".to_string(),
            ..KubernetesPlatformConfig::default()
        };
        let identity = PlatformIdentity {
            node_id: "pod-a".to_string(),
            namespace: Some("pod-ns".to_string()),
            labels: HashMap::new(),
        };

        assert_eq!(resolve_lease_namespace(&config, &identity), "configured-ns");
    }

    #[test]
    fn namespace_resolution_falls_back_to_identity_namespace() {
        let config = KubernetesPlatformConfig {
            namespace: "".to_string(),
            ..KubernetesPlatformConfig::default()
        };
        let identity = PlatformIdentity {
            node_id: "pod-a".to_string(),
            namespace: Some("pod-ns".to_string()),
            labels: HashMap::new(),
        };

        assert_eq!(resolve_lease_namespace(&config, &identity), "pod-ns");
    }

    #[test]
    fn namespace_resolution_falls_back_to_default_literal_when_missing() {
        let config = KubernetesPlatformConfig {
            namespace: "".to_string(),
            ..KubernetesPlatformConfig::default()
        };
        let identity = PlatformIdentity {
            node_id: "pod-a".to_string(),
            namespace: None,
            labels: HashMap::new(),
        };

        assert_eq!(resolve_lease_namespace(&config, &identity), "default");
    }

    #[test]
    fn default_config_leaves_namespace_empty_to_enable_fallback_chain() {
        assert!(KubernetesPlatformConfig::default().namespace.is_empty());
        assert_eq!(KubernetesPlatformConfig::default().jitter_factor, 0.2);
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
    fn jittered_duration_with_zero_factor_is_stable() {
        let base = Duration::from_millis(750);
        assert_eq!(jittered_duration(base, 0.0), base);
    }

    #[test]
    fn next_cycle_sleep_subtracts_elapsed_when_leading() {
        // Default config: renew_deadline = 10s, retry_period = 2s, jitter = 0.2
        let config = KubernetesPlatformConfig::default();
        let sleep = next_cycle_sleep(true, Duration::from_secs(3), &config);
        assert_eq!(sleep, Duration::from_secs(7)); // 10s - 3s
    }

    #[test]
    fn next_cycle_sleep_zero_when_elapsed_exceeds_deadline() {
        let config = KubernetesPlatformConfig::default();
        // elapsed exactly == renew_deadline
        let sleep = next_cycle_sleep(true, config.renew_deadline, &config);
        assert_eq!(sleep, Duration::ZERO);
        // elapsed > renew_deadline
        let sleep = next_cycle_sleep(
            true,
            config.renew_deadline + Duration::from_millis(100),
            &config,
        );
        assert_eq!(sleep, Duration::ZERO);
    }

    #[test]
    fn next_cycle_sleep_uses_retry_period_when_not_leading() {
        // Default config: retry_period = 2s, jitter_factor = 0.2 → [1.6s, 2.4s]
        let config = KubernetesPlatformConfig::default();
        let sleep = next_cycle_sleep(false, Duration::from_secs(5), &config);
        assert!(sleep >= Duration::from_millis(1600));
        assert!(sleep <= Duration::from_millis(2400));
    }
}
