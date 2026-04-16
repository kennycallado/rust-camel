use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::platform::{
    LeaderElector, LeadershipEvent, LeadershipHandle, PlatformError, PlatformIdentity,
};
use chrono::Utc;
use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta};
use kube::api::{DeleteParams, PostParams};
use kube::{Api, Client};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct LeaderElectorConfig {
    pub lease_name: String,
    pub namespace: String,
    pub lease_duration: Duration,
    pub renew_deadline: Duration,
    pub retry_period: Duration,
}

impl Default for LeaderElectorConfig {
    fn default() -> Self {
        Self {
            lease_name: "camel-leader".to_string(),
            namespace: "default".to_string(),
            lease_duration: Duration::from_secs(15),
            renew_deadline: Duration::from_secs(10),
            retry_period: Duration::from_secs(2),
        }
    }
}

pub struct KubernetesLeaderElector {
    client: Client,
    config: LeaderElectorConfig,
    started: AtomicBool,
}

impl KubernetesLeaderElector {
    pub fn new(client: Client, config: LeaderElectorConfig) -> Self {
        Self {
            client,
            config,
            started: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl LeaderElector for KubernetesLeaderElector {
    async fn start(&self, identity: PlatformIdentity) -> Result<LeadershipHandle, PlatformError> {
        if self
            .started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(PlatformError::AlreadyStarted);
        }

        let (event_tx, event_rx) = watch::channel(None);
        let (term_tx, term_rx) = oneshot::channel();
        let cancel = CancellationToken::new();
        let is_leader = Arc::new(AtomicBool::new(false));

        let client = self.client.clone();
        let config = self.config.clone();
        let identity = identity.node_id;
        let cancel_task = cancel.clone();
        let is_leader_task = Arc::clone(&is_leader);

        tokio::spawn(async move {
            let leases: Api<Lease> = Api::namespaced(client, &config.namespace);
            let mut currently_leader = false;

            loop {
                if cancel_task.is_cancelled() {
                    break;
                }

                let leader_now = match reconcile_lease(&leases, &config, &identity).await {
                    Ok(value) => value,
                    Err(err) => {
                        warn!(
                            lease_name = %config.lease_name,
                            namespace = %config.namespace,
                            holder_identity = %identity,
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
                    let _ = event_tx.send(Some(event));
                }

                let sleep_for = if currently_leader {
                    config.renew_deadline
                } else {
                    config.retry_period
                };

                tokio::select! {
                    _ = cancel_task.cancelled() => break,
                    _ = tokio::time::sleep(sleep_for) => {}
                }
            }

            if currently_leader {
                if let Err(err) = release_lease(&leases, &config.lease_name).await {
                    warn!(
                        lease_name = %config.lease_name,
                        namespace = %config.namespace,
                        holder_identity = %identity,
                        error = %err,
                        "failed to release leadership lease"
                    );
                }
                is_leader_task.store(false, Ordering::Release);
                let _ = event_tx.send(Some(LeadershipEvent::StoppedLeading));
            }

            let _ = term_tx.send(());
        });

        Ok(LeadershipHandle::new(event_rx, is_leader, cancel, term_rx))
    }
}

async fn reconcile_lease(
    leases: &Api<Lease>,
    config: &LeaderElectorConfig,
    identity: &str,
) -> Result<bool, kube::Error> {
    let now = Utc::now();

    let maybe_lease = leases.get_opt(&config.lease_name).await?;
    let Some(mut lease) = maybe_lease else {
        let lease = Lease {
            metadata: ObjectMeta {
                name: Some(config.lease_name.clone()),
                namespace: Some(config.namespace.clone()),
                ..ObjectMeta::default()
            },
            spec: Some(LeaseSpec {
                holder_identity: Some(identity.to_string()),
                lease_duration_seconds: Some(config.lease_duration.as_secs() as i32),
                acquire_time: Some(MicroTime(now)),
                renew_time: Some(MicroTime(now)),
                ..LeaseSpec::default()
            }),
        };
        leases.create(&PostParams::default(), &lease).await?;
        return Ok(true);
    };

    let spec = lease.spec.clone().unwrap_or_default();
    let holder = spec.holder_identity.as_deref();
    let is_ours = holder == Some(identity);

    if is_ours {
        lease.spec = Some(LeaseSpec {
            holder_identity: Some(identity.to_string()),
            lease_duration_seconds: Some(config.lease_duration.as_secs() as i32),
            acquire_time: spec.acquire_time,
            renew_time: Some(MicroTime(now)),
            ..spec
        });
        leases
            .replace(&config.lease_name, &PostParams::default(), &lease)
            .await?;
        return Ok(true);
    }

    if lease_is_expired(&spec, now) {
        lease.spec = Some(LeaseSpec {
            holder_identity: Some(identity.to_string()),
            lease_duration_seconds: Some(config.lease_duration.as_secs() as i32),
            acquire_time: Some(MicroTime(now)),
            renew_time: Some(MicroTime(now)),
            ..spec
        });
        leases
            .replace(&config.lease_name, &PostParams::default(), &lease)
            .await?;
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

async fn release_lease(leases: &Api<Lease>, lease_name: &str) -> Result<(), kube::Error> {
    leases.delete(lease_name, &DeleteParams::default()).await?;
    Ok(())
}
