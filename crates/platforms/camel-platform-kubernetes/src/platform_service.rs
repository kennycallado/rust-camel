use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::platform::{
    LeadershipEvent, LeadershipHandle, LeadershipService, NoopReadinessGate, PlatformError,
    PlatformIdentity, PlatformService, ReadinessGate,
};
use chrono::Utc;
use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta};
use kube::api::{DeleteParams, PostParams, Preconditions};
use kube::{Api, Client};
use tokio::sync::{Notify, oneshot, watch};
use tokio_util::sync::CancellationToken;
use tracing::warn;

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
    cancel: CancellationToken,
    ref_count: AtomicUsize,
    terminated: AtomicBool,
    terminated_notify: Notify,
}

impl CachedLock {
    fn new(event_tx: watch::Sender<Option<LeadershipEvent>>, is_leader: Arc<AtomicBool>) -> Self {
        Self {
            event_tx,
            is_leader,
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
            .expect("mutex poisoned: leadership locks map")
            .get(lock_name)
            .cloned()
            .filter(|lock| !lock.cancel.is_cancelled() && !lock.terminated.load(Ordering::Acquire))
        {
            return Ok(Self::create_cached_handle(existing));
        }

        let (event_tx, _event_rx) = watch::channel(None);
        let is_leader = Arc::new(AtomicBool::new(false));
        let cached_lock = Arc::new(CachedLock::new(event_tx, Arc::clone(&is_leader)));

        {
            let mut locks = self
                .locks
                .lock()
                .expect("mutex poisoned: leadership locks map");
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
        let event_tx_task = cached_lock.event_tx.clone();
        let cached_lock_task = Arc::clone(&cached_lock);
        let locks_map = Arc::clone(&self.locks);

        tokio::spawn(async move {
            let leases: Api<Lease> = Api::namespaced(client, &namespace);
            let mut currently_leader = false;

            loop {
                if cancel_task.is_cancelled() {
                    break;
                }

                let leader_now =
                    match reconcile_lease(&leases, &lease_name, &config, &holder_identity).await {
                        Ok(value) => value,
                        Err(err) => {
                            warn!(
                                lease_name = %lease_name,
                                namespace = %namespace,
                                holder_identity = %holder_identity,
                                error = %err,
                                "leader election cycle failed"
                            );
                            false
                        }
                    };

                if leader_now != currently_leader {
                    currently_leader = leader_now;
                    is_leader_task.store(leader_now, Ordering::Release);
                    let event = if leader_now {
                        LeadershipEvent::StartedLeading
                    } else {
                        LeadershipEvent::StoppedLeading
                    };
                    let _ = event_tx_task.send(Some(event));
                }

                let sleep_for = if currently_leader {
                    config.renew_deadline
                } else {
                    jittered_duration(config.retry_period, config.jitter_factor)
                };

                tokio::select! {
                    _ = cancel_task.cancelled() => break,
                    _ = tokio::time::sleep(sleep_for) => {}
                }
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
                .expect("mutex poisoned: leadership locks map");
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

pub struct KubernetesPlatformService {
    identity: PlatformIdentity,
    readiness_gate: Arc<dyn ReadinessGate>,
    leadership: Arc<KubernetesLeadershipService>,
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
        }
    }

    pub async fn try_default(config: KubernetesPlatformConfig) -> Result<Self, PlatformError> {
        config.validate()?;

        let client = Client::try_default().await.map_err(|err| {
            PlatformError::NotAvailable(format!("kubernetes client not available: {err}"))
        })?;

        let identity: PlatformIdentity = KubernetesPlatformIdentity::from_env().into();
        let leadership = Arc::new(KubernetesLeadershipService::new(
            client,
            identity.clone(),
            config,
        )?);

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
) -> Result<bool, kube::Error> {
    let now = Utc::now();

    let maybe_lease = leases.get_opt(lease_name).await?;
    let Some(mut lease) = maybe_lease else {
        let lease = Lease {
            metadata: ObjectMeta {
                name: Some(lease_name.to_string()),
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
            Ok(_) => {}
            Err(err) if is_optimistic_conflict(&err) => {
                // Another contender created the lease first. This cycle loses leadership and retries.
                return Ok(false);
            }
            Err(err) => return Err(err),
        }
        return Ok(true);
    };

    let spec = lease.spec.clone().unwrap_or_default();
    let holder = spec.holder_identity.as_deref();
    let is_ours = holder == Some(holder_identity);

    if is_ours {
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
            Ok(_) => {}
            Err(err) if is_optimistic_conflict(&err) => {
                // Replace carries resourceVersion from the fetched lease; 409 means stale generation.
                return Ok(false);
            }
            Err(err) => return Err(err),
        }
        return Ok(true);
    }

    if lease_is_expired(&spec, now) {
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
            Ok(_) => {}
            Err(err) if is_optimistic_conflict(&err) => {
                // Lease changed between read and replace; treat as lost race and retry next cycle.
                return Ok(false);
            }
            Err(err) => return Err(err),
        }
        return Ok(true);
    }

    Ok(false)
}

fn lease_is_expired(spec: &LeaseSpec, now: chrono::DateTime<Utc>) -> bool {
    let Some(lease_duration_seconds) = spec.lease_duration_seconds else {
        return true;
    };
    let Some(last_renewal) = spec.renew_time.as_ref().or(spec.acquire_time.as_ref()) else {
        return true;
    };
    let expires_at = last_renewal.0 + chrono::Duration::seconds(lease_duration_seconds as i64);
    expires_at < now
}

async fn release_lease(
    leases: &Api<Lease>,
    lease_name: &str,
    holder_identity: &str,
) -> Result<(), kube::Error> {
    let Some(lease) = leases.get_opt(lease_name).await? else {
        return Ok(());
    };

    let Some(delete_params) = delete_params_for_owned_lease(&lease, holder_identity) else {
        return Ok(());
    };

    match leases.delete(lease_name, &delete_params).await {
        Ok(_) => Ok(()),
        Err(err) if is_optimistic_conflict(&err) || is_not_found(&err) => Ok(()),
        Err(err) => Err(err),
    }
}

fn delete_params_for_owned_lease(lease: &Lease, holder_identity: &str) -> Option<DeleteParams> {
    let spec = lease.spec.as_ref()?;
    let holder = spec.holder_identity.as_deref()?;
    if holder != holder_identity {
        return None;
    }

    let uid = lease.metadata.uid.clone();
    let resource_version = lease.metadata.resource_version.clone();
    if uid.is_none() && resource_version.is_none() {
        return None;
    }

    Some(DeleteParams {
        preconditions: Some(Preconditions {
            uid,
            resource_version,
        }),
        ..DeleteParams::default()
    })
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
    use kube::error::ErrorResponse;

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
    fn delete_params_are_built_only_for_owned_lease_with_server_metadata() {
        let lease = Lease {
            metadata: ObjectMeta {
                uid: Some("uid-1".to_string()),
                resource_version: Some("rv-1".to_string()),
                ..ObjectMeta::default()
            },
            spec: Some(LeaseSpec {
                holder_identity: Some("pod-a".to_string()),
                ..LeaseSpec::default()
            }),
        };

        let delete = delete_params_for_owned_lease(&lease, "pod-a");
        let pre = delete
            .unwrap()
            .preconditions
            .expect("preconditions should be set");
        assert_eq!(pre.uid.as_deref(), Some("uid-1"));
        assert_eq!(pre.resource_version.as_deref(), Some("rv-1"));
    }

    #[test]
    fn delete_params_are_not_built_for_foreign_holder() {
        let lease = Lease {
            metadata: ObjectMeta {
                uid: Some("uid-1".to_string()),
                resource_version: Some("rv-1".to_string()),
                ..ObjectMeta::default()
            },
            spec: Some(LeaseSpec {
                holder_identity: Some("pod-b".to_string()),
                ..LeaseSpec::default()
            }),
        };

        assert!(delete_params_for_owned_lease(&lease, "pod-a").is_none());
    }

    #[test]
    fn delete_params_are_not_built_without_precondition_metadata() {
        let lease = Lease {
            metadata: ObjectMeta::default(),
            spec: Some(LeaseSpec {
                holder_identity: Some("pod-a".to_string()),
                ..LeaseSpec::default()
            }),
        };

        assert!(delete_params_for_owned_lease(&lease, "pod-a").is_none());
    }

    #[test]
    fn conflict_classification_is_explicit_for_409_api_errors() {
        let err = kube::Error::Api(ErrorResponse {
            status: "Failure".to_string(),
            message: "conflict".to_string(),
            reason: "Conflict".to_string(),
            code: 409,
        });

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
}
